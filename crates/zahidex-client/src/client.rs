// SPDX-License-Identifier: GPL-3.0-or-later
//! Client ZahideX: connessione, finestre e invio di buffer.
//!
//! Il client è single-thread (`Rc`/`RefCell` interni, non `Send`).
//! I messaggi del server che arrivano mentre si aspetta una risposta
//! (es. `WindowCreated`) finiscono in una coda interna e vengono
//! restituiti dal prossimo `poll_event`/`poll_events`.
//!
//! `Ping` non arriva mai all'applicazione: la libreria risponde `Pong`
//! da sola, sia durante l'attesa di una risposta che durante il poll.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::path::Path;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use zahidex_ipc::{Connection, ShmBuffer};
use zahidex_protocol::{
    ClientMessage, PROTOCOL_VERSION, Rect, ServerMessage, TITLE_BAR_HEIGHT, WindowId,
};

use crate::error::{ClientError, map_ipc};
use crate::event::Event;

/// Tempo massimo di attesa per la risposta all'handshake e a `CreateWindow`.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Una connessione al server ZahideX.
#[derive(Debug)]
pub struct Client {
    conn: Rc<RefCell<Connection>>,
    version: u16,
    output_width: u32,
    output_height: u32,
    /// Messaggi già ricevuti ma non ancora consegnati all'applicazione.
    pending: VecDeque<ServerMessage>,
    /// Finestre create da questo client (riferimenti deboli: la geometria
    /// si aggiorna da sola quando arriva un `Configure`).
    windows: Vec<Weak<WindowInner>>,
}

impl Client {
    /// Connette al server sul socket predefinito
    /// (`$XDG_RUNTIME_DIR/zahidex-0`, fallback `/tmp/zahidex-0`) e fa
    /// l'handshake con negoziazione versione.
    pub fn connect() -> Result<Self, ClientError> {
        Self::connect_as("zahidex-client")
    }

    /// Come [`Client::connect`], con un nome client a scelta (visibile nel
    /// log del server).
    pub fn connect_as(client_name: &str) -> Result<Self, ClientError> {
        Self::connect_to(&zahidex_ipc::default_socket_path(), client_name)
    }

    /// Come [`Client::connect`], su un percorso di socket esplicito.
    pub fn connect_to(path: &Path, client_name: &str) -> Result<Self, ClientError> {
        let mut conn = Connection::connect(path).map_err(map_ipc)?;
        conn.send(&ClientMessage::Hello {
            versions: vec![PROTOCOL_VERSION],
            client_name: client_name.to_string(),
        })
        .map_err(map_ipc)?;
        if !conn.wait_readable(REPLY_TIMEOUT).map_err(map_ipc)? {
            return Err(ClientError::Timeout);
        }
        let (msg, _fds) = conn.recv::<ServerMessage>().map_err(map_ipc)?;
        match msg {
            ServerMessage::HelloOk {
                version,
                output_width,
                output_height,
            } => Ok(Self {
                conn: Rc::new(RefCell::new(conn)),
                version,
                output_width,
                output_height,
                pending: VecDeque::new(),
                windows: Vec::new(),
            }),
            ServerMessage::HelloError { reason } => Err(ClientError::HandshakeRejected { reason }),
            other => Err(ClientError::UnexpectedMessage(format!("{other:?}"))),
        }
    }

    /// Versione del protocollo negoziata all'handshake.
    #[must_use]
    pub fn version(&self) -> u16 {
        self.version
    }

    /// Dimensioni dell'output del server (larghezza, altezza) in pixel.
    #[must_use]
    pub fn output_size(&self) -> (u32, u32) {
        (self.output_width, self.output_height)
    }

    /// Crea una finestra e attende l'ack del server (`WindowCreated`).
    ///
    /// `width`/`height` sono la dimensione richiesta (larghezza e altezza
    /// totali, barra del titolo inclusa); la geometria assegnata dal layout
    /// è quella restituita in `WindowCreated` e leggibile con
    /// [`Window::width`]/[`Window::height`]. Se `floating` è `true`, la
    /// finestra nasce in modalità floating: la geometria finale (centrata,
    /// 800×600 o metà output) arriva con il `Configure` successivo —
    /// ricommitta il buffer quando lo ricevi.
    pub fn create_window(
        &mut self,
        title: &str,
        width: u32,
        height: u32,
        floating: bool,
    ) -> Result<Window, ClientError> {
        let width = width.max(1);
        let height = height.max(1);
        self.conn
            .borrow_mut()
            .send(&ClientMessage::CreateWindow {
                width,
                height,
                title: title.to_string(),
            })
            .map_err(map_ipc)?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(ClientError::Timeout);
            }
            let Some(msg) = self.recv_message(Some(remaining))? else {
                return Err(ClientError::Timeout);
            };
            match msg {
                ServerMessage::WindowCreated {
                    window_id,
                    width,
                    height,
                } => {
                    let window = Window::new(
                        Rc::clone(&self.conn),
                        window_id,
                        width,
                        height,
                        title.to_string(),
                    );
                    self.windows.push(Rc::downgrade(window.inner()));
                    if floating {
                        window.set_floating(true)?;
                    }
                    return Ok(window);
                }
                ServerMessage::Ping => {
                    self.conn
                        .borrow_mut()
                        .send(&ClientMessage::Pong)
                        .map_err(map_ipc)?;
                }
                ServerMessage::Error { message } => {
                    return Err(ClientError::ServerError { message });
                }
                ServerMessage::HelloOk { .. } | ServerMessage::HelloError { .. } => {
                    return Err(ClientError::UnexpectedMessage(format!("{msg:?}")));
                }
                other => self.pending.push_back(other),
            }
        }
    }

    /// Restituisce il prossimo evento, aspettando al massimo `timeout`.
    ///
    /// `None` = attende senza scadenza; `Some(Duration::ZERO)` = non
    /// bloccante (restituisce subito `Ok(None)` se non c'è niente).
    /// Restituisce `Ok(None)` solo in caso di timeout.
    pub fn poll_event(&mut self, timeout: Option<Duration>) -> Result<Option<Event>, ClientError> {
        loop {
            if let Some(msg) = self.pending.pop_front() {
                if let Some(event) = self.handle_message(msg)? {
                    return Ok(Some(event));
                }
                continue;
            }
            let Some(msg) = self.recv_message(timeout)? else {
                return Ok(None);
            };
            if let Some(event) = self.handle_message(msg)? {
                return Ok(Some(event));
            }
        }
    }

    /// Restituisce tutti gli eventi disponibili: attende almeno un evento
    /// (fino a `timeout`, come [`Client::poll_event`]), poi svuota senza
    /// bloccare quelli già arrivati. `Ok(vec![])` solo in caso di timeout.
    pub fn poll_events(&mut self, timeout: Option<Duration>) -> Result<Vec<Event>, ClientError> {
        let mut events = Vec::new();
        if let Some(event) = self.poll_event(timeout)? {
            events.push(event);
            // Svuota senza bloccare ciò che è già arrivato. Se nel mentre
            // il server ha chiuso la connessione, gli eventi già letti
            // restano validi: la disconnessione verrà segnalata al prossimo
            // poll (che la propagherà come errore).
            loop {
                match self.poll_event(Some(Duration::ZERO)) {
                    Ok(Some(event)) => events.push(event),
                    Ok(None) => break,
                    Err(ClientError::Disconnected) => break,
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(events)
    }

    /// Legge un messaggio grezzo dal socket. Con `Some(timeout)` aspetta al
    /// massimo quel tempo (`Ok(None)` = timeout); con `None` blocca finché
    /// non arriva qualcosa (o il server chiude la connessione).
    fn recv_message(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<ServerMessage>, ClientError> {
        match timeout {
            None => {
                let (msg, _fds) = self
                    .conn
                    .borrow_mut()
                    .recv::<ServerMessage>()
                    .map_err(map_ipc)?;
                Ok(Some(msg))
            }
            Some(timeout) => {
                if !self.conn.borrow().wait_readable(timeout).map_err(map_ipc)? {
                    return Ok(None);
                }
                let (msg, _fds) = self
                    .conn
                    .borrow_mut()
                    .recv::<ServerMessage>()
                    .map_err(map_ipc)?;
                Ok(Some(msg))
            }
        }
    }

    /// Converte un messaggio grezzo in un evento: `Ping` → `Pong`
    /// automatico, `Configure` → aggiorna la geometria della finestra,
    /// `WindowCreated`/`HelloOk`/`HelloError` → ignorati (li consumano
    /// `connect`/`create_window`; se arrivano qui sono spuri).
    fn handle_message(&mut self, msg: ServerMessage) -> Result<Option<Event>, ClientError> {
        match msg {
            ServerMessage::Ping => {
                self.conn
                    .borrow_mut()
                    .send(&ClientMessage::Pong)
                    .map_err(map_ipc)?;
                Ok(None)
            }
            ServerMessage::Configure {
                window_id,
                width,
                height,
            } => {
                self.update_window_geometry(window_id, width, height);
                Ok(Some(Event::Configure {
                    window_id,
                    width,
                    height,
                }))
            }
            ServerMessage::WindowCreated { .. }
            | ServerMessage::HelloOk { .. }
            | ServerMessage::HelloError { .. } => Ok(None),
            ServerMessage::Focus { window_id } => Ok(Some(Event::Focus { window_id })),
            ServerMessage::PointerEnter { window_id } => {
                Ok(Some(Event::PointerEnter { window_id }))
            }
            ServerMessage::PointerLeave { window_id } => {
                Ok(Some(Event::PointerLeave { window_id }))
            }
            ServerMessage::PointerMotion { window_id, x, y } => {
                Ok(Some(Event::PointerMotion { window_id, x, y }))
            }
            ServerMessage::PointerButton {
                window_id,
                button,
                pressed,
            } => Ok(Some(Event::PointerButton {
                window_id,
                button,
                pressed,
            })),
            ServerMessage::PointerAxis {
                window_id,
                axis,
                delta_v120,
                delta_px,
            } => Ok(Some(Event::PointerAxis {
                window_id,
                axis,
                delta_v120,
                delta_px,
            })),
            ServerMessage::Key {
                window_id,
                keycode,
                pressed,
                keysym,
                modifiers,
            } => Ok(Some(Event::Key {
                window_id,
                keycode,
                pressed,
                keysym,
                modifiers,
            })),
            ServerMessage::CloseWindow { window_id } => Ok(Some(Event::CloseWindow { window_id })),
            ServerMessage::Error { message } => Ok(Some(Event::ServerError { message })),
        }
    }

    /// Aggiorna la geometria della finestra registrata con questo id
    /// (quando arriva un `Configure`); i riferimenti morti vengono potati.
    fn update_window_geometry(&mut self, id: WindowId, width: u32, height: u32) {
        self.windows.retain(|weak| {
            if let Some(inner) = weak.upgrade() {
                if inner.id == id {
                    inner.width.set(width);
                    inner.height.set(height);
                }
                true
            } else {
                false
            }
        });
    }
}

/// Stato condiviso di una finestra: connessione + geometria corrente.
/// La geometria si aggiorna da sola quando il `Client` riceve un
/// `Configure` per questa finestra.
#[derive(Debug)]
pub(crate) struct WindowInner {
    conn: Rc<RefCell<Connection>>,
    id: WindowId,
    width: Cell<u32>,
    height: Cell<u32>,
    title: RefCell<String>,
    floating: Cell<bool>,
}

/// Una finestra ZahideX.
///
/// Il contenuto è un buffer XRGB8888 largo `width` e alto
/// `height - TITLE_BAR_HEIGHT` (la barra del titolo la disegna il server,
/// non fa parte del buffer). Il buffer si crea a ogni commit: il client
/// disegna in un memfd e lo passa al server via SCM_RIGHTS.
#[derive(Debug, Clone)]
pub struct Window(Rc<WindowInner>);

impl Window {
    pub(crate) fn new(
        conn: Rc<RefCell<Connection>>,
        id: WindowId,
        width: u32,
        height: u32,
        title: String,
    ) -> Self {
        Self(Rc::new(WindowInner {
            conn,
            id,
            width: Cell::new(width),
            height: Cell::new(height),
            title: RefCell::new(title),
            floating: Cell::new(false),
        }))
    }

    pub(crate) fn inner(&self) -> &Rc<WindowInner> {
        &self.0
    }

    /// Id assegnato dal server.
    #[must_use]
    pub fn id(&self) -> WindowId {
        self.0.id
    }

    /// Larghezza totale della finestra (come assegnata dal layout).
    #[must_use]
    pub fn width(&self) -> u32 {
        self.0.width.get()
    }

    /// Altezza totale della finestra (barra del titolo inclusa).
    #[must_use]
    pub fn height(&self) -> u32 {
        self.0.height.get()
    }

    /// Altezza dell'area di contenuto (= altezza totale meno la barra del
    /// titolo): è l'altezza dei pixel passati a [`Window::commit`].
    #[must_use]
    pub fn content_height(&self) -> u32 {
        self.height().saturating_sub(TITLE_BAR_HEIGHT).max(1)
    }

    /// Titolo corrente della finestra.
    #[must_use]
    pub fn title(&self) -> String {
        self.0.title.borrow().clone()
    }

    /// `true` se la finestra è in modalità floating.
    #[must_use]
    pub fn is_floating(&self) -> bool {
        self.0.floating.get()
    }

    /// Presenta un nuovo contenuto: `pixels` sono byte XRGB8888
    /// (B, G, R, X per pixel), `width * content_height * 4` byte in tutto.
    /// Il damage inviato è l'intero contenuto (il damage parziale è lavoro
    /// futuro).
    pub fn commit(&self, pixels: &[u8]) -> Result<(), ClientError> {
        let width = self.width();
        let height = self.content_height();
        let expected = width as usize * height as usize * 4;
        if pixels.len() != expected {
            return Err(ClientError::InvalidPixels {
                expected,
                actual: pixels.len(),
                width,
                height,
            });
        }
        let buffer = ShmBuffer::new(width, height).map_err(map_ipc)?;
        {
            let mut map = buffer.map_mut().map_err(map_ipc)?;
            map.copy_from_slice(pixels);
        }
        self.send_commit(&buffer)
    }

    /// Come [`Window::commit`], ma il contenuto viene disegnato dalla
    /// closure direttamente nel buffer condiviso: riceve i byte del buffer
    /// (righe da `stride` byte, `width * 4` byte utili per riga), la
    /// larghezza, l'altezza del contenuto e lo stride.
    pub fn commit_with(
        &self,
        draw: impl FnOnce(&mut [u8], u32, u32, u32),
    ) -> Result<(), ClientError> {
        let width = self.width();
        let height = self.content_height();
        let buffer = ShmBuffer::new(width, height).map_err(map_ipc)?;
        let stride = buffer.stride();
        {
            let mut map = buffer.map_mut().map_err(map_ipc)?;
            draw(&mut map, width, height, stride);
        }
        self.send_commit(&buffer)
    }

    /// Invia il buffer al server come nuovo contenuto della finestra.
    fn send_commit(&self, buffer: &ShmBuffer) -> Result<(), ClientError> {
        let width = self.width();
        let height = self.content_height();
        self.0
            .conn
            .borrow_mut()
            .send_commit_buffer(self.id(), buffer, Rect::new(0, 0, width, height))
            .map_err(map_ipc)
    }

    /// Cambia il titolo della finestra.
    pub fn set_title(&self, title: &str) -> Result<(), ClientError> {
        self.0
            .conn
            .borrow_mut()
            .send(&ClientMessage::SetTitle {
                window_id: self.id(),
                title: title.to_string(),
            })
            .map_err(map_ipc)?;
        *self.0.title.borrow_mut() = title.to_string();
        Ok(())
    }

    /// Passa la finestra in modalità floating (`true`) o tiling (`false`).
    /// La nuova geometria arriva con il `Configure` successivo.
    pub fn set_floating(&self, floating: bool) -> Result<(), ClientError> {
        self.0
            .conn
            .borrow_mut()
            .send(&ClientMessage::SetFloating {
                window_id: self.id(),
                floating,
            })
            .map_err(map_ipc)?;
        self.0.floating.set(floating);
        Ok(())
    }

    /// Chiede al server di ridimensionare la finestra. Per le finestre in
    /// tiling il server può rifiutare (la dimensione la decide il layout)
    /// con un `ServerMessage::Error`; la geometria effettiva arriva con
    /// il `Configure` successivo.
    pub fn request_resize(&self, width: u32, height: u32) -> Result<(), ClientError> {
        self.0
            .conn
            .borrow_mut()
            .send(&ClientMessage::RequestResize {
                window_id: self.id(),
                width: width.max(1),
                height: height.max(1),
            })
            .map_err(map_ipc)
    }

    /// Distrugge la finestra (consuma l'handle).
    pub fn destroy(self) -> Result<(), ClientError> {
        self.0
            .conn
            .borrow_mut()
            .send(&ClientMessage::DestroyWindow {
                window_id: self.id(),
            })
            .map_err(map_ipc)
    }
}
