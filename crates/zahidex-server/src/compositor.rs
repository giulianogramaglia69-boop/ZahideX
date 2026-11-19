// SPDX-License-Identifier: GPL-3.0-or-later
//! Compositor di finestre ZahideX (Fase 4): registro, layout tiling,
//! finestre floating, focus per-finestra, drag, hit-test.
//!
//! Logica pura: niente DRM, niente I/O, niente thread. Il rendering
//! (texture, elementi) vive in `backend.rs`; l'inoltro dei messaggi ai
//! client vive in `ipc.rs` / `input.rs`: i metodi qui restituiscono
//! messaggi in uscita da inviare, non li inviano.
//!
//! # Modello
//!
//! - `windows` è in ordine z: il primo è il più in basso, l'ultimo il più
//!   in alto. Le finestre tiled stanno sempre sotto le floating; una
//!   floating focalizzata sale in cima.
//! - Layout tiling: master a sinistra (60% della larghezza) + stack a
//!   destra diviso in parti uguali in verticale. Una sola finestra tiled
//!   occupa tutto l'output.
//! - Ogni finestra ha una barra del titolo alta [`TITLE_BAR_HEIGHT`] px:
//!   l'area di contenuto è la geometria meno la barra.
//! - Il focus è per-finestra (`Option<WindowId>`): la tastiera va alla
//!   finestra focalizzata, il puntatore alla finestra sotto il cursore.

use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use zahidex_protocol::{Rect, ServerMessage, TITLE_BAR_HEIGHT, WindowId};

use crate::focus::ClientId;

/// Frazione di larghezza del master nel layout tiling (3/5 = 60%).
pub const MASTER_WIDTH_NUM: u32 = 3;
/// Denominatore della frazione del master.
pub const MASTER_WIDTH_DEN: u32 = 5;
/// Dimensioni iniziali di una finestra floating (se il client non ne
/// chiede altre).
pub const FLOAT_DEFAULT_WIDTH: u32 = 800;
/// Dimensioni iniziali di una finestra floating (altezza).
pub const FLOAT_DEFAULT_HEIGHT: u32 = 600;
/// Dimensioni minime di una finestra floating.
pub const MIN_FLOAT_WIDTH: u32 = 100;
/// Dimensioni minime di una finestra floating (altezza).
pub const MIN_FLOAT_HEIGHT: u32 = 100;
/// Distanza dal bordo (px) entro cui un click inizia il ridimensionamento.
pub const RESIZE_EDGE: i32 = 8;

/// Messaggio in uscita verso un client: (destinatario, messaggio).
pub type Outbound = (ClientId, ServerMessage);

/// Errore di un'operazione del compositor (il chiamante lo traduce in
/// `ServerMessage::Error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompositorError {
    /// Finestra sconosciuta.
    UnknownWindow(WindowId),
    /// La finestra appartiene a un altro client.
    NotOwner(WindowId),
    /// L'operazione non ha senso su una finestra in tiling (es. resize
    /// richiesto dal client: la dimensione la decide il layout).
    TiledWindow(WindowId),
}

/// Bordo di una finestra floating su cui si può iniziare un resize.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeEdge {
    /// Bordo destro.
    Right,
    /// Bordo inferiore.
    Bottom,
    /// Angolo in basso a destra.
    BottomRight,
}

/// Trascinamento in corso (solo finestre floating).
#[derive(Debug, Clone, Copy)]
enum Drag {
    /// Spostamento: la finestra segue il cursore con questo offset
    /// (posizione cursore − angolo finestra al momento del click).
    Move {
        id: WindowId,
        offset_x: i32,
        offset_y: i32,
    },
    /// Ridimensionamento: geometria all'inizio del drag + punto di partenza.
    Resize {
        id: WindowId,
        edge: ResizeEdge,
        start_x: i32,
        start_y: i32,
        start: Rect,
    },
}

/// Una finestra gestita dal compositor.
#[derive(Debug)]
pub struct Window {
    /// Id assegnato dal server (`WindowId(0)` è la sentinella output).
    pub id: WindowId,
    /// Client proprietario (riceve gli eventi della finestra).
    pub owner: ClientId,
    /// Titolo (mostrato nella barra solo come colore per ora; il testo
    /// arriva dopo).
    pub title: String,
    /// Dimensioni richieste dal client in `CreateWindow` (usate come
    /// geometria iniziale quando la finestra diventa floating).
    pub requested: (u32, u32),
    /// `true` = floating (sopra le tiled, spostabile/ridimensionabile).
    pub floating: bool,
    /// Geometria completa (barra del titolo inclusa).
    pub geometry: Rect,
    /// Ultimo contenuto committato (buffer CPU, da convertire in texture
    /// a ogni frame). `None` finché il client non committa niente.
    pub content: Option<MemoryRenderBuffer>,
    /// Dimensioni del contenuto committato.
    pub content_size: (u32, u32),
}

impl Window {
    /// Rettangolo della barra del titolo.
    #[must_use]
    pub fn title_bar_rect(&self) -> Rect {
        Rect::new(
            self.geometry.x,
            self.geometry.y,
            self.geometry.width,
            TITLE_BAR_HEIGHT,
        )
    }

    /// `true` se il punto (coordinate output) cade dentro la finestra.
    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        contains_rect(self.geometry, x, y)
    }
}

/// `true` se il punto cade dentro il rettangolo (estremo superiore
/// incluso, inferiore escluso).
fn contains_rect(rect: Rect, x: i32, y: i32) -> bool {
    x >= rect.x && x < rect.x + rect.width as i32 && y >= rect.y && y < rect.y + rect.height as i32
}

/// Una finestra pronta per il rendering (dati clonati fuori dal lock).
#[derive(Debug)]
pub struct FrameWindow {
    /// Geometria completa (barra del titolo inclusa).
    pub geometry: Rect,
    /// `true` se è la finestra focalizzata (bordo + barra evidenziati).
    pub focused: bool,
    /// Contenuto committato, se c'è.
    pub content: Option<MemoryRenderBuffer>,
}

impl FrameWindow {
    /// Area di contenuto: geometria meno la barra del titolo.
    #[must_use]
    pub fn content_rect(&self) -> Rect {
        let height = self.geometry.height.saturating_sub(TITLE_BAR_HEIGHT);
        Rect::new(
            self.geometry.x,
            self.geometry.y + TITLE_BAR_HEIGHT as i32,
            self.geometry.width,
            height,
        )
    }

    /// Rettangolo della barra del titolo.
    #[must_use]
    pub fn title_bar_rect(&self) -> Rect {
        Rect::new(
            self.geometry.x,
            self.geometry.y,
            self.geometry.width,
            TITLE_BAR_HEIGHT,
        )
    }
}

/// Piano di un frame: finestre in ordine z (prima = più in basso) +
/// cursore.
#[derive(Debug)]
pub struct FramePlan {
    /// Finestre da disegnare, in ordine z.
    pub windows: Vec<FrameWindow>,
    /// Posizione del cursore.
    pub cursor_pos: (i32, i32),
    /// `true` se il cursore va disegnato.
    pub cursor_visible: bool,
}

/// Il compositor: registro finestre, layout, focus, drag.
#[derive(Debug)]
pub struct Compositor {
    /// Dimensioni dell'output (origine sempre 0,0).
    output: Rect,
    /// Finestre in ordine z (primo = più in basso).
    windows: Vec<Window>,
    /// Prossimo id da assegnare (`WindowId(0)` è la sentinella output).
    next_id: u32,
    /// Finestra focalizzata, se ce n'è una.
    focused: Option<WindowId>,
    /// Trascinamento in corso (solo floating).
    drag: Option<Drag>,
}

impl Compositor {
    /// Crea un compositor vuoto per un output `width` x `height`.
    #[must_use]
    pub fn new(output_width: u32, output_height: u32) -> Self {
        Self {
            output: Rect::new(0, 0, output_width, output_height),
            windows: Vec::new(),
            next_id: 1,
            focused: None,
            drag: None,
        }
    }

    /// Quante finestre ci sono.
    #[cfg(test)]
    #[must_use]
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// La finestra focalizzata, se ce n'è una.
    #[must_use]
    pub fn focused_id(&self) -> Option<WindowId> {
        self.focused
    }

    /// Il proprietario della finestra focalizzata, se ce n'è una.
    #[must_use]
    pub fn focused_owner(&self) -> Option<ClientId> {
        self.focused.and_then(|id| self.window(id).map(|w| w.owner))
    }

    /// La finestra con questo id, se esiste.
    #[must_use]
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Il proprietario di una finestra, se esiste.
    #[must_use]
    pub fn owner_of(&self, id: WindowId) -> Option<ClientId> {
        self.window(id).map(|w| w.owner)
    }

    /// Id delle finestre di un client, in ordine z (prima = più in basso).
    #[must_use]
    pub fn windows_of(&self, owner: ClientId) -> Vec<WindowId> {
        self.windows
            .iter()
            .filter(|w| w.owner == owner)
            .map(|w| w.id)
            .collect()
    }

    /// `true` se c'è un trascinamento in corso.
    #[cfg(test)]
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Crea una finestra tiled (default) per `owner` e la focalizza.
    ///
    /// Restituisce l'id assegnato e i messaggi in uscita: `WindowCreated`
    /// (con la geometria assegnata) al proprietario, `Focus` al nuovo e
    /// al vecchio focalizzato, `Configure` alle finestre tiled la cui
    /// geometria è cambiata per il nuovo layout.
    pub fn create_window(
        &mut self,
        owner: ClientId,
        width: u32,
        height: u32,
        title: String,
    ) -> (WindowId, Vec<Outbound>) {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        let mut out = Vec::new();
        // Inserimento in ordine z: le tiled stanno sotto le floating, in
        // ordine di creazione (prima floating = punto di inserimento).
        let insert_at = self
            .windows
            .iter()
            .position(|w| w.floating)
            .unwrap_or(self.windows.len());
        self.windows.insert(
            insert_at,
            Window {
                id,
                owner,
                title,
                requested: (width.max(1), height.max(1)),
                floating: false,
                geometry: Rect::new(0, 0, width.max(1), height.max(1)),
                content: None,
                content_size: (0, 0),
            },
        );
        let changed = self.relayout();
        let geometry = self.window(id).expect("finestra appena creata").geometry;
        out.push((
            owner,
            ServerMessage::WindowCreated {
                window_id: id,
                width: geometry.width,
                height: geometry.height,
            },
        ));
        for (changed_id, rect) in changed {
            if changed_id == id {
                continue; // la geometria è già in WindowCreated
            }
            if let Some(w) = self.window(changed_id) {
                out.push((
                    w.owner,
                    ServerMessage::Configure {
                        window_id: changed_id,
                        width: rect.width,
                        height: rect.height,
                    },
                ));
            }
        }
        out.extend(self.set_focus(id));
        (id, out)
    }

    /// Distrugge una finestra. Se era focalizzata, il focus passa alla
    /// finestra più in alto rimasta. Ricalcola il layout tiled.
    /// Finestra sconosciuta: nessun effetto, lista vuota.
    pub fn destroy_window(&mut self, id: WindowId) -> Vec<Outbound> {
        let Some(pos) = self.windows.iter().position(|w| w.id == id) else {
            return Vec::new();
        };
        let was_focused = self.focused == Some(id);
        if self.drag.is_some_and(|d| d.window_id() == id) {
            self.drag = None;
        }
        self.windows.remove(pos);
        let mut out = Vec::new();
        if was_focused {
            self.focused = None;
            if let Some(top) = self.windows.last().map(|w| w.id) {
                out.extend(self.set_focus(top));
            }
        }
        for (changed_id, rect) in self.relayout() {
            if let Some(w) = self.window(changed_id) {
                out.push((
                    w.owner,
                    ServerMessage::Configure {
                        window_id: changed_id,
                        width: rect.width,
                        height: rect.height,
                    },
                ));
            }
        }
        out
    }

    /// Cambia il titolo di una finestra. Restituisce `true` se la finestra
    /// esiste (e il titolo è cambiato): serve un ridisegno.
    pub fn set_title(
        &mut self,
        id: WindowId,
        owner: ClientId,
        title: String,
    ) -> Result<bool, CompositorError> {
        let w = self
            .windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(CompositorError::UnknownWindow(id))?;
        if w.owner != owner {
            return Err(CompositorError::NotOwner(id));
        }
        if w.title == title {
            return Ok(false);
        }
        w.title = title;
        Ok(true)
    }

    /// Passa una finestra da tiling a floating e viceversa.
    /// Da tiling a floating: prende la geometria iniziale floating
    /// (dimensione richiesta, centrata) e va in cima allo z-order.
    /// Da floating a tiling: rientra nel layout.
    pub fn set_floating(
        &mut self,
        id: WindowId,
        owner: ClientId,
        floating: bool,
    ) -> Result<Vec<Outbound>, CompositorError> {
        let pos = self
            .windows
            .iter()
            .position(|w| w.id == id)
            .ok_or(CompositorError::UnknownWindow(id))?;
        if self.windows[pos].owner != owner {
            return Err(CompositorError::NotOwner(id));
        }
        if self.windows[pos].floating == floating {
            return Ok(Vec::new());
        }
        self.windows[pos].floating = floating;
        let requested = self.windows[pos].requested;
        let mut out = Vec::new();
        if floating {
            let geo = self.initial_floating_geometry(requested.0, requested.1);
            let mut window = self.windows.remove(pos);
            window.geometry = geo;
            self.windows.push(window);
            out.push((
                owner,
                ServerMessage::Configure {
                    window_id: id,
                    width: geo.width,
                    height: geo.height,
                },
            ));
        } else {
            let window = self.windows.remove(pos);
            let insert_at = self
                .windows
                .iter()
                .position(|w| w.floating)
                .unwrap_or(self.windows.len());
            self.windows.insert(insert_at, window);
        }
        for (changed_id, rect) in self.relayout() {
            if let Some(w) = self.window(changed_id) {
                out.push((
                    w.owner,
                    ServerMessage::Configure {
                        window_id: changed_id,
                        width: rect.width,
                        height: rect.height,
                    },
                ));
            }
        }
        Ok(out)
    }

    /// Il client chiede di essere ridimensionato. Solo le floating possono
    /// decidere la dimensione: per le tiled risponde il layout (errore).
    pub fn request_resize(
        &mut self,
        id: WindowId,
        owner: ClientId,
        width: u32,
        height: u32,
    ) -> Result<Vec<Outbound>, CompositorError> {
        let w = self
            .windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(CompositorError::UnknownWindow(id))?;
        if w.owner != owner {
            return Err(CompositorError::NotOwner(id));
        }
        if !w.floating {
            return Err(CompositorError::TiledWindow(id));
        }
        let (ow, oh) = (self.output.width, self.output.height);
        let new_w = width.clamp(MIN_FLOAT_WIDTH, ow);
        let new_h = height.clamp(MIN_FLOAT_HEIGHT, oh);
        w.geometry.width = new_w;
        w.geometry.height = new_h;
        w.geometry.x = w.geometry.x.clamp(0, (ow.saturating_sub(new_w)) as i32);
        w.geometry.y = w.geometry.y.clamp(0, (oh.saturating_sub(new_h)) as i32);
        Ok(vec![(
            owner,
            ServerMessage::Configure {
                window_id: id,
                width: new_w,
                height: new_h,
            },
        )])
    }

    /// Memorizza un nuovo contenuto per una finestra (già convertito in
    /// buffer di rendering dal chiamante). Restituisce `true` se la
    /// finestra esiste: serve un ridisegno.
    pub fn commit_buffer(
        &mut self,
        id: WindowId,
        owner: ClientId,
        buffer: MemoryRenderBuffer,
        width: u32,
        height: u32,
    ) -> Result<bool, CompositorError> {
        let w = self
            .windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(CompositorError::UnknownWindow(id))?;
        if w.owner != owner {
            return Err(CompositorError::NotOwner(id));
        }
        w.content = Some(buffer);
        w.content_size = (width, height);
        Ok(true)
    }

    /// Imposta il focus su una finestra. Una floating focalizzata sale in
    /// cima allo z-order. Restituisce i `Focus` da inviare (al nuovo
    /// proprietario `Focus { Some(id) }`, al vecchio proprietario — se
    /// diverso — `Focus { None }`). Nessun cambiamento: lista vuota.
    pub fn set_focus(&mut self, id: WindowId) -> Vec<Outbound> {
        if self.focused == Some(id) {
            return Vec::new();
        }
        let Some(window) = self.window(id) else {
            return Vec::new();
        };
        let new_owner = window.owner;
        let old_owner = self.focused_owner();
        if window.floating {
            let pos = self
                .windows
                .iter()
                .position(|w| w.id == id)
                .expect("id noto");
            let window = self.windows.remove(pos);
            self.windows.push(window);
        }
        self.focused = Some(id);
        let mut out = Vec::new();
        if let Some(old) = old_owner
            && old != new_owner
        {
            out.push((old, ServerMessage::Focus { window_id: None }));
        }
        out.push((
            new_owner,
            ServerMessage::Focus {
                window_id: Some(id),
            },
        ));
        out
    }

    /// Sposta il focus alla finestra successiva (`direction > 0`) o
    /// precedente (`direction < 0`) in ordine z, con avvolgimento. Senza
    /// finestre: lista vuota. Senza focus: focalizza la prima.
    pub fn cycle_focus(&mut self, direction: i32) -> Vec<Outbound> {
        if self.windows.is_empty() {
            return Vec::new();
        }
        let current = self
            .focused
            .and_then(|id| self.windows.iter().position(|w| w.id == id));
        let next = match current {
            Some(i) => {
                let n = self.windows.len() as i32;
                (i as i32 + direction).rem_euclid(n) as usize
            }
            None => 0,
        };
        let id = self.windows[next].id;
        self.set_focus(id)
    }

    /// Passa la finestra focalizzata da tiling a floating e viceversa.
    /// Senza focus: lista vuota.
    pub fn toggle_floating_focused(&mut self) -> Vec<Outbound> {
        let Some(id) = self.focused else {
            return Vec::new();
        };
        let Some(window) = self.window(id) else {
            return Vec::new();
        };
        let (owner, floating) = (window.owner, window.floating);
        self.set_floating(id, owner, !floating).unwrap_or_default()
    }

    /// Chiude la finestra focalizzata: invia `CloseWindow` al proprietario
    /// e la distrugge subito (il client che risponde con `DestroyWindow`
    /// riceve un errore: la finestra non c'è più). Senza focus: lista vuota.
    pub fn close_focused(&mut self) -> Vec<Outbound> {
        let Some(id) = self.focused else {
            return Vec::new();
        };
        let Some(owner) = self.owner_of(id) else {
            return Vec::new();
        };
        let mut out = vec![(owner, ServerMessage::CloseWindow { window_id: id })];
        out.extend(self.destroy_window(id));
        out
    }

    /// Finestra più in alto che contiene il punto (coordinate output),
    /// se ce n'è una.
    #[must_use]
    pub fn hit_test(&self, x: i32, y: i32) -> Option<WindowId> {
        self.windows
            .iter()
            .rev()
            .find(|w| w.contains(x, y))
            .map(|w| w.id)
    }

    /// Finestra floating la cui barra del titolo contiene il punto.
    #[must_use]
    pub fn hit_test_title_bar(&self, x: i32, y: i32) -> Option<WindowId> {
        self.windows
            .iter()
            .rev()
            .filter(|w| w.floating)
            .find(|w| contains_rect(w.title_bar_rect(), x, y))
            .map(|w| w.id)
    }

    /// Finestra floating il cui bordo destro/inferiore contiene il punto.
    #[must_use]
    pub fn hit_test_resize(&self, x: i32, y: i32) -> Option<(WindowId, ResizeEdge)> {
        for w in self.windows.iter().rev().filter(|w| w.floating) {
            let geo = w.geometry;
            let near_right =
                x >= geo.x + geo.width as i32 - RESIZE_EDGE && x < geo.x + geo.width as i32;
            let near_bottom =
                y >= geo.y + geo.height as i32 - RESIZE_EDGE && y < geo.y + geo.height as i32;
            let inside_x = x >= geo.x && x < geo.x + geo.width as i32;
            let inside_y = y >= geo.y && y < geo.y + geo.height as i32;
            if near_right && near_bottom && inside_x && inside_y {
                return Some((w.id, ResizeEdge::BottomRight));
            }
            if near_right && inside_y {
                return Some((w.id, ResizeEdge::Right));
            }
            if near_bottom && inside_x {
                return Some((w.id, ResizeEdge::Bottom));
            }
        }
        None
    }

    /// Inizia un trascinamento con il tasto sinistro nel punto dato:
    /// barra del titolo di una floating → spostamento; bordo di una
    /// floating → resize; altrimenti solo focus alla finestra cliccata.
    /// Restituisce i messaggi di focus (o lista vuota se non ha cliccato
    /// nessuna finestra).
    pub fn begin_drag(&mut self, x: i32, y: i32) -> Vec<Outbound> {
        if let Some(id) = self.hit_test_title_bar(x, y) {
            let geo = self.window(id).expect("id noto").geometry;
            self.drag = Some(Drag::Move {
                id,
                offset_x: x - geo.x,
                offset_y: y - geo.y,
            });
            return self.set_focus(id);
        }
        if let Some((id, edge)) = self.hit_test_resize(x, y) {
            let geo = self.window(id).expect("id noto").geometry;
            self.drag = Some(Drag::Resize {
                id,
                edge,
                start_x: x,
                start_y: y,
                start: geo,
            });
            return self.set_focus(id);
        }
        match self.hit_test(x, y) {
            Some(id) => self.set_focus(id),
            None => Vec::new(),
        }
    }

    /// Aggiorna la geometria durante un trascinamento. Restituisce `true`
    /// se la geometria è cambiata (serve un ridisegno).
    pub fn drag_to(&mut self, x: i32, y: i32) -> bool {
        let Some(drag) = self.drag else {
            return false;
        };
        let (ow, oh) = (self.output.width, self.output.height);
        match drag {
            Drag::Move {
                id,
                offset_x,
                offset_y,
            } => {
                let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
                    self.drag = None;
                    return false;
                };
                let new_x = (x - offset_x).clamp(0, (ow.saturating_sub(w.geometry.width)) as i32);
                let new_y = (y - offset_y).clamp(0, (oh.saturating_sub(w.geometry.height)) as i32);
                if new_x == w.geometry.x && new_y == w.geometry.y {
                    return false;
                }
                w.geometry.x = new_x;
                w.geometry.y = new_y;
                true
            }
            Drag::Resize {
                id,
                edge,
                start_x,
                start_y,
                start,
            } => {
                let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
                    self.drag = None;
                    return false;
                };
                let mut new_w = start.width;
                let mut new_h = start.height;
                if matches!(edge, ResizeEdge::Right | ResizeEdge::BottomRight) {
                    new_w = (start.width as i32 + (x - start_x))
                        .clamp(MIN_FLOAT_WIDTH as i32, ow as i32 - start.x)
                        as u32;
                }
                if matches!(edge, ResizeEdge::Bottom | ResizeEdge::BottomRight) {
                    new_h = (start.height as i32 + (y - start_y))
                        .clamp(MIN_FLOAT_HEIGHT as i32, oh as i32 - start.y)
                        as u32;
                }
                if new_w == w.geometry.width && new_h == w.geometry.height {
                    return false;
                }
                w.geometry.width = new_w;
                w.geometry.height = new_h;
                true
            }
        }
    }

    /// Termina il trascinamento. Restituisce id e geometria finale (per
    /// inviare `Configure` al proprietario) se c'era un drag in corso.
    pub fn end_drag(&mut self) -> Option<(WindowId, Rect)> {
        let drag = self.drag.take()?;
        let id = drag.window_id();
        self.window(id).map(|w| (id, w.geometry))
    }

    /// Piano del frame corrente: finestre in ordine z con i contenuti
    /// clonati (il buffer è `Arc`, la copia è economica) + cursore.
    #[must_use]
    pub fn frame_plan(&self, cursor_pos: (i32, i32), cursor_visible: bool) -> FramePlan {
        FramePlan {
            windows: self
                .windows
                .iter()
                .map(|w| FrameWindow {
                    geometry: w.geometry,
                    focused: Some(w.id) == self.focused,
                    content: w.content.clone(),
                })
                .collect(),
            cursor_pos,
            cursor_visible,
        }
    }

    /// Geometria iniziale di una floating: dimensione richiesta (o il
    /// default 800×600), centrata; metà output se l'output è più piccolo
    /// del default.
    fn initial_floating_geometry(&self, mut width: u32, mut height: u32) -> Rect {
        let (ow, oh) = (self.output.width, self.output.height);
        if width == 0 {
            width = FLOAT_DEFAULT_WIDTH;
        }
        if height == 0 {
            height = FLOAT_DEFAULT_HEIGHT;
        }
        if ow < FLOAT_DEFAULT_WIDTH || oh < FLOAT_DEFAULT_HEIGHT {
            width = ow / 2;
            height = oh / 2;
        } else {
            width = width.min(ow);
            height = height.min(oh);
        }
        width = width.max(MIN_FLOAT_WIDTH).min(ow);
        height = height.max(MIN_FLOAT_HEIGHT).min(oh);
        Rect::new(
            ((ow - width) / 2) as i32,
            ((oh - height) / 2) as i32,
            width,
            height,
        )
    }

    /// Ricalcola la geometria delle finestre tiled (master 60% a sinistra,
    /// stack a destra diviso in parti uguali). Restituisce id e nuova
    /// geometria delle finestre cambiate.
    fn relayout(&mut self) -> Vec<(WindowId, Rect)> {
        let tiled: Vec<WindowId> = self
            .windows
            .iter()
            .filter(|w| !w.floating)
            .map(|w| w.id)
            .collect();
        let (w, h) = (self.output.width, self.output.height);
        let mut changed = Vec::new();
        match tiled.len() {
            0 => {}
            1 => {
                let rect = Rect::new(0, 0, w, h);
                changed.extend(self.apply_geometry(tiled[0], rect));
            }
            _ => {
                let master_w = w * MASTER_WIDTH_NUM / MASTER_WIDTH_DEN;
                let stack_w = w - master_w;
                changed.extend(self.apply_geometry(tiled[0], Rect::new(0, 0, master_w, h)));
                let n = (tiled.len() - 1) as u32;
                for (i, &id) in tiled[1..].iter().enumerate() {
                    let i = i as u32;
                    let y = h * i / n;
                    let y2 = h * (i + 1) / n;
                    changed.extend(
                        self.apply_geometry(
                            id,
                            Rect::new(master_w as i32, y as i32, stack_w, y2 - y),
                        ),
                    );
                }
            }
        }
        changed
    }

    /// Applica una geometria a una finestra: restituisce la coppia
    /// (id, geometria) solo se è cambiata.
    fn apply_geometry(&mut self, id: WindowId, rect: Rect) -> Vec<(WindowId, Rect)> {
        let Some(w) = self.windows.iter_mut().find(|w| w.id == id) else {
            return Vec::new();
        };
        if w.geometry == rect {
            return Vec::new();
        }
        w.geometry = rect;
        vec![(id, rect)]
    }
}

impl Drag {
    fn window_id(&self) -> WindowId {
        match *self {
            Drag::Move { id, .. } | Drag::Resize { id, .. } => id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::backend::allocator::Fourcc;
    use smithay::utils::Transform;

    fn comp() -> Compositor {
        Compositor::new(1920, 1080)
    }

    fn owner(n: u64) -> ClientId {
        ClientId(n)
    }

    fn dummy_buffer(width: u32, height: u32) -> MemoryRenderBuffer {
        let bytes = vec![0u8; (width * height * 4) as usize];
        MemoryRenderBuffer::from_slice(
            &bytes,
            Fourcc::Xrgb8888,
            (width as i32, height as i32),
            1,
            Transform::Normal,
            None,
        )
    }

    #[test]
    fn tiling_one_window_takes_full_output() {
        let mut c = comp();
        let (id, out) = c.create_window(owner(1), 800, 600, "A".into());
        assert_eq!(c.window(id).unwrap().geometry, Rect::new(0, 0, 1920, 1080));
        assert!(out.iter().any(|(_, m)| matches!(
            m,
            ServerMessage::WindowCreated {
                window_id,
                width: 1920,
                height: 1080,
            } if *window_id == id
        )));
        assert_eq!(c.focused_id(), Some(id));
    }

    #[test]
    fn tiling_two_windows_master_and_stack() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        assert_eq!(c.window(a).unwrap().geometry, Rect::new(0, 0, 1152, 1080));
        assert_eq!(c.window(b).unwrap().geometry, Rect::new(1152, 0, 768, 1080));
    }

    #[test]
    fn tiling_three_windows_stack_splits_evenly() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        let (d, _) = c.create_window(owner(1), 800, 600, "C".into());
        assert_eq!(c.window(a).unwrap().geometry, Rect::new(0, 0, 1152, 1080));
        assert_eq!(c.window(b).unwrap().geometry, Rect::new(1152, 0, 768, 540));
        assert_eq!(
            c.window(d).unwrap().geometry,
            Rect::new(1152, 540, 768, 540)
        );
    }

    #[test]
    fn tiling_four_windows_stack_splits_in_thirds() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        let (d, _) = c.create_window(owner(1), 800, 600, "C".into());
        let (e, _) = c.create_window(owner(1), 800, 600, "D".into());
        assert_eq!(c.window(a).unwrap().geometry, Rect::new(0, 0, 1152, 1080));
        assert_eq!(c.window(b).unwrap().geometry, Rect::new(1152, 0, 768, 360));
        assert_eq!(
            c.window(d).unwrap().geometry,
            Rect::new(1152, 360, 768, 360)
        );
        assert_eq!(
            c.window(e).unwrap().geometry,
            Rect::new(1152, 720, 768, 360)
        );
    }

    #[test]
    fn floating_window_is_centered_at_default_size() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "A".into());
        c.set_floating(id, owner(1), true).unwrap();
        assert_eq!(
            c.window(id).unwrap().geometry,
            Rect::new(560, 240, 800, 600)
        );
        assert!(c.window(id).unwrap().floating);
    }

    #[test]
    fn floating_on_small_output_is_half_output() {
        let mut c = Compositor::new(640, 480);
        let (id, _) = c.create_window(owner(1), 800, 600, "A".into());
        c.set_floating(id, owner(1), true).unwrap();
        assert_eq!(
            c.window(id).unwrap().geometry,
            Rect::new(160, 120, 320, 240)
        );
    }

    #[test]
    fn hit_test_floating_beats_tiled_below() {
        let mut c = comp();
        let (tiled, _) = c.create_window(owner(1), 800, 600, "T".into());
        let (floating, _) = c.create_window(owner(2), 800, 600, "F".into());
        c.set_floating(floating, owner(2), true).unwrap();
        // La floating è centrata (560,240,800,600): il punto (600,300) è
        // dentro entrambe, vince la floating.
        assert_eq!(c.hit_test(600, 300), Some(floating));
        // Il punto (100,100) è solo nella tiled (master a sinistra).
        assert_eq!(c.hit_test(100, 100), Some(tiled));
        // Fuori da tutto: niente.
        assert_eq!(c.hit_test(1919, 1079), Some(tiled));
    }

    #[test]
    fn hit_test_title_bar_and_resize_edges() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "F".into());
        c.set_floating(id, owner(1), true).unwrap();
        // Barra del titolo: y 240..264, x 560..1360.
        assert_eq!(c.hit_test_title_bar(600, 250), Some(id));
        assert_eq!(c.hit_test_title_bar(600, 300), None);
        // Bordo destro: x 1352..1360; bordo inferiore: y 832..840.
        assert_eq!(c.hit_test_resize(1355, 500), Some((id, ResizeEdge::Right)));
        assert_eq!(c.hit_test_resize(700, 835), Some((id, ResizeEdge::Bottom)));
        assert_eq!(
            c.hit_test_resize(1355, 835),
            Some((id, ResizeEdge::BottomRight))
        );
        assert_eq!(c.hit_test_resize(700, 500), None);
    }

    #[test]
    fn destroy_relayouts_and_moves_focus() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        let (d, _) = c.create_window(owner(1), 800, 600, "C".into());
        assert_eq!(c.focused_id(), Some(d));
        // Distrugge la finestra di mezzo (stack in alto).
        let out = c.destroy_window(b);
        assert_eq!(c.window_count(), 2);
        // La C sale nello stack: ora è a (1152,0,768,1080).
        assert_eq!(c.window(d).unwrap().geometry, Rect::new(1152, 0, 768, 1080));
        assert_eq!(c.window(a).unwrap().geometry, Rect::new(0, 0, 1152, 1080));
        // Il focus era sulla C, resta sulla C.
        assert_eq!(c.focused_id(), Some(d));
        // Configure inviata alla C (geometria cambiata).
        assert!(out.iter().any(|(_, m)| matches!(
            m,
            ServerMessage::Configure {
                window_id,
                width: 768,
                height: 1080,
            } if *window_id == d
        )));
    }

    #[test]
    fn destroy_focused_moves_focus_to_topmost() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(2), 800, 600, "B".into());
        assert_eq!(c.focused_id(), Some(b));
        let out = c.destroy_window(b);
        assert_eq!(c.focused_id(), Some(a));
        assert!(out.iter().any(|(client, m)| *client == owner(1)
            && matches!(m, ServerMessage::Focus { window_id: Some(id) } if *id == a)));
    }

    #[test]
    fn destroy_unknown_window_is_noop() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        assert!(c.destroy_window(WindowId(99)).is_empty());
        assert_eq!(c.window_count(), 1);
        assert_eq!(c.focused_id(), Some(a));
    }

    #[test]
    fn cycle_focus_wraps_around() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        let (d, _) = c.create_window(owner(1), 800, 600, "C".into());
        assert_eq!(c.focused_id(), Some(d));
        c.cycle_focus(1);
        assert_eq!(c.focused_id(), Some(a));
        c.cycle_focus(1);
        assert_eq!(c.focused_id(), Some(b));
        c.cycle_focus(-1);
        assert_eq!(c.focused_id(), Some(a));
        c.cycle_focus(-1);
        assert_eq!(c.focused_id(), Some(d));
    }

    #[test]
    fn toggle_floating_moves_between_layout_and_free() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        // B è stack (1152,0,768,1080): diventa floating, prende la
        // geometria iniziale (richiesta 800x600, centrata).
        c.toggle_floating_focused();
        assert!(c.window(b).unwrap().floating);
        assert_eq!(c.window(b).unwrap().geometry, Rect::new(560, 240, 800, 600));
        // A ora occupa tutto l'output da sola.
        assert_eq!(c.window(a).unwrap().geometry, Rect::new(0, 0, 1920, 1080));
        // Torna tiling: B riprende il suo posto nello stack.
        c.toggle_floating_focused();
        assert!(!c.window(b).unwrap().floating);
        assert_eq!(c.window(b).unwrap().geometry, Rect::new(1152, 0, 768, 1080));
    }

    #[test]
    fn drag_move_updates_geometry_and_clamps() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "F".into());
        c.set_floating(id, owner(1), true).unwrap();
        // Click sulla barra del titolo (600,250): offset (40,10).
        c.begin_drag(600, 250);
        assert!(c.is_dragging());
        c.drag_to(700, 400);
        assert_eq!(
            c.window(id).unwrap().geometry,
            Rect::new(660, 390, 800, 600)
        );
        // Trascina oltre il bordo destro: si blocca a 1920-800=1120.
        c.drag_to(5000, 400);
        assert_eq!(c.window(id).unwrap().geometry.x, 1120);
        let (ended_id, geo) = c.end_drag().unwrap();
        assert_eq!(ended_id, id);
        assert_eq!(geo, Rect::new(1120, 390, 800, 600));
        assert!(!c.is_dragging());
    }

    #[test]
    fn drag_resize_updates_size() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "F".into());
        c.set_floating(id, owner(1), true).unwrap();
        // Click sull'angolo in basso a destra (1355,835).
        c.begin_drag(1355, 835);
        assert!(c.is_dragging());
        c.drag_to(1500, 900);
        assert_eq!(
            c.window(id).unwrap().geometry,
            Rect::new(560, 240, 945, 665)
        );
        let (_, geo) = c.end_drag().unwrap();
        assert_eq!(geo.width, 945);
        assert_eq!(geo.height, 665);
    }

    #[test]
    fn drag_on_tiled_window_only_focuses() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        c.begin_drag(100, 100);
        assert!(!c.is_dragging());
        assert_eq!(c.focused_id(), Some(a));
        let _ = b;
    }

    #[test]
    fn close_focused_destroys_and_sends_close_window() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(2), 800, 600, "B".into());
        let out = c.close_focused();
        assert_eq!(c.window_count(), 1);
        assert_eq!(c.focused_id(), Some(a));
        assert!(out.iter().any(|(client, m)| *client == owner(2)
            && matches!(m, ServerMessage::CloseWindow { window_id } if *window_id == b)));
        assert!(
            c.close_focused()
                .iter()
                .any(|(_, m)| matches!(m, ServerMessage::CloseWindow { .. }))
        );
        // Chiude anche l'ultima: nessuna finestra, nessun focus.
        c.close_focused();
        assert_eq!(c.window_count(), 0);
        assert_eq!(c.focused_id(), None);
        assert!(c.close_focused().is_empty());
    }

    #[test]
    fn commit_buffer_stores_content() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "A".into());
        let buf = dummy_buffer(1920, 1080);
        assert!(c.commit_buffer(id, owner(1), buf, 1920, 1080).unwrap());
        let w = c.window(id).unwrap();
        assert!(w.content.is_some());
        assert_eq!(w.content_size, (1920, 1080));
        // Altro client: rifiutato.
        let buf = dummy_buffer(10, 10);
        assert_eq!(
            c.commit_buffer(id, owner(2), buf, 10, 10),
            Err(CompositorError::NotOwner(id))
        );
        // Finestra sconosciuta: errore.
        let buf = dummy_buffer(10, 10);
        assert_eq!(
            c.commit_buffer(WindowId(99), owner(1), buf, 10, 10),
            Err(CompositorError::UnknownWindow(WindowId(99)))
        );
    }

    #[test]
    fn request_resize_only_for_floating() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "A".into());
        assert_eq!(
            c.request_resize(id, owner(1), 400, 300),
            Err(CompositorError::TiledWindow(id))
        );
        c.set_floating(id, owner(1), true).unwrap();
        let out = c.request_resize(id, owner(1), 400, 300).unwrap();
        assert_eq!(
            c.window(id).unwrap().geometry,
            Rect::new(560, 240, 400, 300)
        );
        assert!(out.iter().any(|(_, m)| matches!(
            m,
            ServerMessage::Configure {
                window_id,
                width: 400,
                height: 300,
            } if *window_id == id
        )));
        // Sotto il minimo: bloccato a 100x100.
        c.request_resize(id, owner(1), 10, 10).unwrap();
        assert_eq!(c.window(id).unwrap().geometry.width, 100);
        assert_eq!(c.window(id).unwrap().geometry.height, 100);
    }

    #[test]
    fn set_title_and_wrong_owner() {
        let mut c = comp();
        let (id, _) = c.create_window(owner(1), 800, 600, "A".into());
        assert!(c.set_title(id, owner(1), "Nuovo".into()).unwrap());
        assert_eq!(c.window(id).unwrap().title, "Nuovo");
        assert!(!c.set_title(id, owner(1), "Nuovo".into()).unwrap());
        assert_eq!(
            c.set_title(id, owner(2), "X".into()),
            Err(CompositorError::NotOwner(id))
        );
        assert_eq!(
            c.set_title(WindowId(99), owner(1), "X".into()),
            Err(CompositorError::UnknownWindow(WindowId(99)))
        );
    }

    #[test]
    fn focused_floating_window_goes_on_top() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(2), 800, 600, "B".into());
        c.set_floating(a, owner(1), true).unwrap();
        c.set_floating(b, owner(2), true).unwrap();
        // A e B sono entrambe centrate (560,240,800,600): si sovrappongono
        // del tutto, B sopra (creata dopo).
        assert_eq!(c.hit_test(600, 300), Some(b));
        // Rimpicciolisci B e spostala: B diventa (860,490,400,300).
        c.request_resize(b, owner(2), 400, 300).unwrap();
        c.begin_drag(700, 250); // barra del titolo di B
        c.drag_to(1000, 500);
        c.end_drag();
        assert_eq!(c.window(b).unwrap().geometry, Rect::new(860, 490, 400, 300));
        // Ora (600,300) è solo in A: il click focalizza A e A sale in cima.
        assert_eq!(c.hit_test(600, 300), Some(a));
        c.begin_drag(600, 300);
        assert_eq!(c.focused_id(), Some(a));
        assert_eq!(c.hit_test(600, 300), Some(a));
    }

    #[test]
    fn frame_plan_is_in_z_order_with_focus_flag() {
        let mut c = comp();
        let (a, _) = c.create_window(owner(1), 800, 600, "A".into());
        let (b, _) = c.create_window(owner(1), 800, 600, "B".into());
        let plan = c.frame_plan((100, 100), true);
        assert_eq!(plan.windows.len(), 2);
        assert_eq!(plan.windows[0].geometry, Rect::new(0, 0, 1152, 1080));
        assert!(!plan.windows[0].focused);
        assert!(plan.windows[1].focused);
        assert_eq!(plan.cursor_pos, (100, 100));
        assert!(plan.cursor_visible);
        let _ = (a, b);
    }
}
