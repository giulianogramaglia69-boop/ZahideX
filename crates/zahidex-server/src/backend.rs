// SPDX-License-Identifier: GPL-3.0-or-later
//! Backend DRM di ZahideX: sessione libseat, udev, DRM/KMS, GBM, EGL,
//! GlesRenderer, DrmSurface e DrmCompositor.
//!
//! Solo moduli backend di Smithay 0.7 — nessun codice Wayland: il protocollo
//! resta il nostro (crate `zahidex-protocol` / `zahidex-ipc`).
//!
//! Sequenza di avvio:
//! 1. `LibSeatSession::new()` — sessione logind, niente root.
//! 2. `primary_gpu()` (udev) — trova `/dev/dri/cardN`.
//! 3. `session.open()` — fd DRM autorizzato.
//! 4. `DrmDevice::new()` + `GbmDevice::new()` + `EGLDisplay` / `EGLContext` /
//!    `GlesRenderer`.
//! 5. Scelta di connector/mode/crtc (`mode::pick_mode_index`).
//! 6. `Output` + `DrmSurface` (modeset) + `DrmCompositor`.
//!
//! Su questa VM non c'è `/dev/dri`: il crate compila e i test puri passano,
//! ma l'avvio vero richiede il laptop da TTY.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use drm::control::{Device as ControlDevice, Mode, connector};
use drm_fourcc::DrmFourcc;
use rustix::fs::OFlags;
use smithay::backend::{
    allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
    drm::{
        DrmDevice, DrmDeviceFd, DrmDeviceNotifier, DrmSurface,
        compositor::{DrmCompositor, FrameFlags},
        exporter::gbm::GbmFramebufferExporter,
    },
    egl::{EGLContext, EGLDisplay},
    libinput::LibinputInputBackend,
    renderer::{
        Color32F, ImportMem,
        element::{
            Kind,
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            render_elements,
            solid::{SolidColorBuffer, SolidColorRenderElement},
        },
        gles::GlesRenderer,
    },
    session::{
        Session,
        libseat::{LibSeatSession, LibSeatSessionNotifier},
    },
    udev::{UdevBackend, all_gpus, primary_gpu},
};
use smithay::output::{Output, PhysicalProperties, Scale, Subpixel};
use smithay::utils::{DeviceFd, Physical, Point, Size, Transform};
use tracing::{debug, info, warn};

use crate::compositor::{FramePlan, FrameWindow};
use crate::cursor::cursor_image;
use crate::error::ServerError;
use crate::input::create_libinput;
use crate::mode::pick_mode_index;

/// Colore di sfondo: blu scuro `#0B1D33`.
pub const BACKGROUND_COLOR: [f32; 4] = [
    0x0b as f32 / 255.0,
    0x1d as f32 / 255.0,
    0x33 as f32 / 255.0,
    1.0,
];

/// Spessore del bordo di focus (px).
const FOCUS_BORDER_WIDTH: i32 = 2;

/// Barra del titolo con focus: blu `#2563EB`.
const TITLE_FOCUSED: Color32F = Color32F::new(
    0x25 as f32 / 255.0,
    0x63 as f32 / 255.0,
    0xeb as f32 / 255.0,
    1.0,
);
/// Barra del titolo senza focus: grigio-blu `#334155`.
const TITLE_UNFOCUSED: Color32F = Color32F::new(
    0x33 as f32 / 255.0,
    0x41 as f32 / 255.0,
    0x55 as f32 / 255.0,
    1.0,
);
/// Bordo della finestra con focus: blu `#0A84FF`.
const FOCUS_BORDER: Color32F = Color32F::new(
    0x0a as f32 / 255.0,
    0x84 as f32 / 255.0,
    0xff as f32 / 255.0,
    1.0,
);
/// Contenuto assente (finestra senza buffer): `#16283D`.
const PLACEHOLDER: Color32F = Color32F::new(
    0x16 as f32 / 255.0,
    0x28 as f32 / 255.0,
    0x3d as f32 / 255.0,
    1.0,
);

render_elements! {
    /// Elementi di un frame: texture da memfd oppure rettangoli pieni.
    pub OutputRenderElements<R> where R: ImportMem;
    Memory = MemoryRenderBufferRenderElement<R>,
    Solid = SolidColorRenderElement,
}

/// Formati colore provati in ordine per lo swapchain del primary plane.
const COLOR_FORMATS: &[DrmFourcc] = &[DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

/// Il compositor DRM di ZahideX: allocator GBM, exporter GBM, niente dati
/// utente per i frame (`()`), device GBM su `DrmDeviceFd`.
pub type ZahidexCompositor =
    DrmCompositor<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;

/// Stato DRM del server (tutto ciò che serve per disegnare).
///
/// I campi `session`, `output` e `gpu_path` non sono letti in Fase 2, ma
/// vanno tenuti vivi: la sessione tiene aperti i device, l'output verrà
/// usato dal compositor in Fase 4 e il percorso GPU serve per la diagnostica.
#[allow(dead_code)]
pub struct DrmBackend {
    /// Sessione libseat (apre i device, gestisce VT switch).
    pub session: LibSeatSession,
    /// Device DRM (modesetting, page flip, eventi VBlank).
    pub device: DrmDevice,
    /// Compositor DRM (swapchain + damage tracking + scan-out).
    pub compositor: ZahidexCompositor,
    /// Renderer GLES (disegna nei buffer GBM).
    pub renderer: GlesRenderer,
    /// Output logico (nome, modalità corrente, scala).
    pub output: Output,
    /// Dimensioni dell'output in pixel (per `HelloOk`).
    pub output_size: (u32, u32),
    /// Percorso della GPU (`/dev/dri/cardN`).
    pub gpu_path: PathBuf,
    /// dev_t della GPU (per riconoscere la sua rimozione via udev).
    pub gpu_rdev: u64,
    /// Immagine del cursore software (freccia 16x16, CPU).
    pub cursor_buffer: MemoryRenderBuffer,
}

/// Tutto ciò che `DrmBackend::new` costruisce: il backend più le sorgenti
/// eventi da inserire nel loop calloop.
pub struct BackendParts {
    pub backend: DrmBackend,
    pub drm_notifier: DrmDeviceNotifier,
    pub session_notifier: LibSeatSessionNotifier,
    pub udev_backend: UdevBackend,
    pub input_backend: LibinputInputBackend,
}

impl DrmBackend {
    /// Costruisce il backend: sessione, GPU, EGL, output, surface, compositor.
    ///
    /// Non disegna ancora niente: il primo frame arriva da
    /// [`DrmBackend::render_background`].
    ///
    /// Il nome è `open` (non `new`) perché restituisce `BackendParts`, non
    /// `Self`.
    pub fn open(seat: &str) -> Result<BackendParts, ServerError> {
        let (mut session, session_notifier) =
            LibSeatSession::new().map_err(|e| ServerError::Session(e.to_string()))?;
        info!("sessione libseat attiva (seat {})", session.seat());

        let mut udev_backend =
            UdevBackend::new(seat).map_err(|e| ServerError::Message(format!("udev: {e}")))?;

        let gpu_path = find_gpu(&mut udev_backend, seat)?;
        info!("GPU: {}", gpu_path.display());
        let gpu_rdev = std::fs::metadata(&gpu_path)
            .map_err(ServerError::Io)?
            .rdev();

        let fd = session
            .open(&gpu_path, OFlags::RDWR | OFlags::CLOEXEC)
            .map_err(|e| {
                ServerError::Message(format!("apertura {} fallita: {e}", gpu_path.display()))
            })?;
        let drm_fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (mut device, drm_notifier) =
            DrmDevice::new(drm_fd.clone(), false).map_err(|e| ServerError::Drm(e.to_string()))?;
        let gbm =
            GbmDevice::new(drm_fd.clone()).map_err(|e| ServerError::Drm(format!("gbm: {e}")))?;
        let display = unsafe { EGLDisplay::new(gbm.clone()) }
            .map_err(|e| ServerError::Gl(format!("EGLDisplay: {e}")))?;
        let (egl_major, egl_minor) = display.get_egl_version();
        info!("EGL {egl_major}.{egl_minor} su GBM");
        let context =
            EGLContext::new(&display).map_err(|e| ServerError::Gl(format!("EGLContext: {e}")))?;
        let renderer = unsafe { GlesRenderer::new(context) }
            .map_err(|e| ServerError::Gl(format!("GlesRenderer: {e}")))?;
        info!("renderer GLES pronto");

        let (output, surface, output_size) = setup_output(&mut device)?;
        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let exporter = GbmFramebufferExporter::new(gbm.clone(), None);
        let renderer_formats = display.dmabuf_render_formats().clone();
        let compositor = DrmCompositor::new(
            &output,
            surface,
            None,
            allocator,
            exporter,
            COLOR_FORMATS.iter().copied(),
            renderer_formats,
            device.cursor_size(),
            Some(gbm),
        )
        .map_err(|e| ServerError::Drm(format!("DrmCompositor: {e}")))?;
        info!(
            "compositor DRM pronto (output {}x{})",
            output_size.0, output_size.1
        );

        // Input libinput: la sessione si clona (libseat) e apre i device
        // /dev/input/event* con i permessi logind.
        let input_backend = create_libinput(session.clone(), seat)?;
        info!("input libinput pronto (seat {seat})");

        // Immagine del cursore software (freccia 16x16, buffer CPU).
        let cursor_buffer =
            MemoryRenderBuffer::from_memory(cursor_image(), 1, Transform::Normal, None);

        Ok(BackendParts {
            backend: DrmBackend {
                session,
                device,
                compositor,
                renderer,
                output,
                output_size,
                gpu_path,
                gpu_rdev,
                cursor_buffer,
            },
            drm_notifier,
            session_notifier,
            udev_backend,
            input_backend,
        })
    }

    /// Disegna il piano del compositor (finestre in ordine z) sopra lo
    /// sfondo, con il cursore software sopra (solo se il piano lo richiede).
    ///
    /// Restituisce `true` se un frame è stato accodato (e va confermato con
    /// [`DrmBackend::frame_submitted`] al prossimo VBlank), `false` se non
    /// c'era niente da ridisegnare (nessun danno).
    pub fn render(&mut self, plan: &FramePlan) -> Result<bool, ServerError> {
        let mut elements: Vec<OutputRenderElements<GlesRenderer>> = Vec::new();
        for window in &plan.windows {
            self.render_window(&mut elements, window)?;
        }
        if plan.cursor_visible {
            let cursor_element = MemoryRenderBufferRenderElement::from_buffer(
                &mut self.renderer,
                Point::<f64, Physical>::from((plan.cursor_pos.0 as f64, plan.cursor_pos.1 as f64)),
                &self.cursor_buffer,
                Some(1.0),
                None,
                None,
                Kind::Cursor,
            )
            .map_err(|e| ServerError::Drm(format!("elemento cursore: {e}")))?;
            elements.push(OutputRenderElements::Memory(cursor_element));
        }
        let result = self
            .compositor
            .render_frame(
                &mut self.renderer,
                &elements,
                BACKGROUND_COLOR,
                FrameFlags::DEFAULT,
            )
            .map_err(|e| ServerError::Drm(format!("render_frame: {e}")))?;
        if result.is_empty {
            debug!("frame vuoto (nessun danno): niente da presentare");
            return Ok(false);
        }
        self.compositor
            .queue_frame(())
            .map_err(|e| ServerError::Drm(format!("queue_frame: {e}")))?;
        Ok(true)
    }

    /// Aggiunge gli elementi di una finestra: bordo di focus (2 px) dietro
    /// tutto, contenuto (texture o segnaposto scuro) e barra del titolo
    /// (24 px, colore diverso se focus).
    fn render_window(
        &mut self,
        elements: &mut Vec<OutputRenderElements<GlesRenderer>>,
        window: &FrameWindow,
    ) -> Result<(), ServerError> {
        let geo = window.geometry;
        if window.focused {
            let border = SolidColorBuffer::new(
                (
                    geo.width as i32 + 2 * FOCUS_BORDER_WIDTH,
                    geo.height as i32 + 2 * FOCUS_BORDER_WIDTH,
                ),
                FOCUS_BORDER,
            );
            elements.push(OutputRenderElements::Solid(
                SolidColorRenderElement::from_buffer(
                    &border,
                    Point::from((geo.x - FOCUS_BORDER_WIDTH, geo.y - FOCUS_BORDER_WIDTH)),
                    1.0,
                    1.0,
                    Kind::Unspecified,
                ),
            ));
        }
        let content = window.content_rect();
        match &window.content {
            Some(buffer) => {
                let element = MemoryRenderBufferRenderElement::from_buffer(
                    &mut self.renderer,
                    Point::<f64, Physical>::from((content.x as f64, content.y as f64)),
                    buffer,
                    Some(1.0),
                    None,
                    Some(Size::from((content.width as i32, content.height as i32))),
                    Kind::Unspecified,
                )
                .map_err(|e| ServerError::Drm(format!("elemento contenuto: {e}")))?;
                elements.push(OutputRenderElements::Memory(element));
            }
            None => {
                let placeholder = SolidColorBuffer::new(
                    (content.width as i32, content.height as i32),
                    PLACEHOLDER,
                );
                elements.push(OutputRenderElements::Solid(
                    SolidColorRenderElement::from_buffer(
                        &placeholder,
                        Point::from((content.x, content.y)),
                        1.0,
                        1.0,
                        Kind::Unspecified,
                    ),
                ));
            }
        }
        let title = window.title_bar_rect();
        let color = if window.focused {
            TITLE_FOCUSED
        } else {
            TITLE_UNFOCUSED
        };
        let title_buffer = SolidColorBuffer::new((title.width as i32, title.height as i32), color);
        elements.push(OutputRenderElements::Solid(
            SolidColorRenderElement::from_buffer(
                &title_buffer,
                Point::from((title.x, title.y)),
                1.0,
                1.0,
                Kind::Unspecified,
            ),
        ));
        Ok(())
    }

    /// Conferma che il frame accodato è stato presentato (dopo VBlank).
    pub fn frame_submitted(&mut self) -> Result<(), ServerError> {
        self.compositor
            .frame_submitted()
            .map_err(|e| ServerError::Drm(format!("frame_submitted: {e}")))?;
        Ok(())
    }
}

/// Trova la GPU da usare: la primaria del seat, altrimenti la prima
/// enumerata da udev.
fn find_gpu(udev_backend: &mut UdevBackend, seat: &str) -> Result<PathBuf, ServerError> {
    if let Some(path) =
        primary_gpu(seat).map_err(|e| ServerError::Message(format!("udev primary_gpu: {e}")))?
    {
        return Ok(path);
    }
    let _ = udev_backend;
    let gpus = all_gpus(seat).map_err(|e| ServerError::Message(format!("udev all_gpus: {e}")))?;
    gpus.into_iter()
        .next()
        .ok_or_else(|| ServerError::NoGpu(seat.to_string()))
}

/// Sceglie il primo connector collegato con una modalità, crea l'`Output` e
/// la `DrmSurface` (il modeset avviene qui).
fn setup_output(device: &mut DrmDevice) -> Result<(Output, DrmSurface, (u32, u32)), ServerError> {
    let resources = device
        .resource_handles()
        .map_err(|e| ServerError::Drm(e.to_string()))?;
    for (index, &conn_handle) in resources.connectors().iter().enumerate() {
        let conn = match device.get_connector(conn_handle, false) {
            Ok(conn) => conn,
            Err(e) => {
                warn!("connector {conn_handle:?}: lettura fallita: {e}");
                continue;
            }
        };
        if conn.state() != connector::State::Connected || conn.modes().is_empty() {
            debug!("connector {conn_handle:?}: non collegato o senza modalità");
            continue;
        }
        let crtc = conn
            .encoders()
            .iter()
            .filter_map(|encoder| device.get_encoder(*encoder).ok())
            .find_map(|encoder| encoder.crtc())
            .or_else(|| resources.crtcs().first().copied());
        let Some(crtc) = crtc else {
            warn!("connector {conn_handle:?}: nessun CRTC disponibile");
            continue;
        };
        let Some(mode_idx) = pick_mode_index(conn.modes()) else {
            continue;
        };
        let mode: Mode = conn.modes()[mode_idx];
        let (width, height) = mode.size();
        let size_mm = conn.size().unwrap_or((0, 0));
        let name = format!("ZX-{index}");
        let output = Output::new(
            name.clone(),
            PhysicalProperties {
                size: Size::from((size_mm.0 as i32, size_mm.1 as i32)),
                make: "ZahideX".into(),
                model: "DRM".into(),
                subpixel: Subpixel::Unknown,
            },
        );
        // `change_current_state` vuole la `Mode` di Smithay, non quella DRM.
        let output_mode: smithay::output::Mode = mode.into();
        output.change_current_state(
            Some(output_mode),
            None,
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        let surface = device
            .create_surface(crtc, mode, &[conn_handle])
            .map_err(|e| ServerError::Drm(format!("create_surface {name}: {e}")))?;
        info!(
            "output {name}: {}x{} @ {} mHz (connector {conn_handle:?}, crtc {crtc:?})",
            width,
            height,
            mode.vrefresh()
        );
        return Ok((output, surface, (width as u32, height as u32)));
    }
    Err(ServerError::NoOutput(
        "nessun monitor collegato trovato".to_string(),
    ))
}

/// Controllo dei prerequisiti senza accendere niente. Utile sulla VM e sul
/// laptop prima del primo avvio vero. Esce sempre con successo: è un report,
/// non un test pass/fail.
pub fn dry_run(seat: &str) -> Result<(), ServerError> {
    println!("zahidex --dry-run: controllo i prerequisiti (non accendo niente)");
    let mut session = None;
    match LibSeatSession::new() {
        Ok((s, _)) => {
            println!("[ok] sessione libseat attiva (seat {})", s.seat());
            session = Some(s);
        }
        Err(e) => {
            println!("[no] sessione libseat: {e}");
            println!("     -> lancia zahidex da una TTY libera con logind attivo");
        }
    }
    match primary_gpu(seat) {
        Ok(Some(path)) => println!("[ok] GPU primaria: {}", path.display()),
        Ok(None) => {
            println!("[no] nessuna GPU primaria sul seat {seat}");
            match all_gpus(seat) {
                Ok(gpus) if !gpus.is_empty() => {
                    for gpu in gpus {
                        println!("     gpu: {}", gpu.display());
                    }
                }
                _ => println!("     nessuna GPU trovata via udev"),
            }
        }
        Err(e) => println!("[no] enumerazione udev fallita: {e}"),
    }
    match session {
        Some(s) => {
            let devices = crate::input::list_input_devices(s, seat);
            if devices.is_empty() {
                println!("[..] input: nessun dispositivo visto da libinput");
            } else {
                println!(
                    "[ok] input: {} dispositivi visti da libinput:",
                    devices.len()
                );
                for device in devices {
                    println!("     - {device}");
                }
            }
        }
        None => println!("[..] input: salto il controllo (serve la sessione libseat)"),
    }
    println!("[..] /dev/dri presente: {}", Path::new("/dev/dri").exists());
    println!("[..] socket IPC: {:?}", zahidex_ipc::default_socket_path());
    println!("[..] sfondo: #0B1D33, renderer GlesRenderer su GBM (Smithay 0.7)");
    println!(
        "[..] compositor: tiling master 60% + stack, floating, barra titolo 24 px, bordo focus 2 px"
    );
    println!("[..] cursore: freccia software 16x16; uscita: Ctrl+C, Ctrl+Alt+BackSpace, Super+Q");
    println!(
        "[..] scorciatoie finestre: Super+J/K ciclo focus, Super+F tiling<->floating, Super+Q chiude"
    );
    println!("[..] client di test: ./target/release/zahidex-test-client [secondi]");
    println!("dry-run completato.");
    Ok(())
}
