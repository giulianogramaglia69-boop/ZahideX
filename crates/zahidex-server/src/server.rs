// SPDX-License-Identifier: GPL-3.0-or-later
//! Loop principale del server: calloop con DRM, sessione libseat, udev e
//! segnali.
//!
//! Lo sfondo viene ridisegnato solo quando serve (avvio e riattivazione
//! dopo un cambio VT): il resto del tempo il loop dorme in attesa di eventi.
//!
//! Uscita pulita (Ctrl+C / SIGTERM): il loop si ferma, i `Drop` rilasciano
//! DRM master e sessione, e logind ripristina la TTY precedente.

use calloop::signals::{Signal, Signals};
use calloop::{EventLoop, LoopSignal};
use smithay::backend::{
    drm::{DrmDeviceNotifier, DrmEvent},
    session::{Event as SessionEvent, libseat::LibSeatSessionNotifier},
    udev::{UdevBackend, UdevEvent},
};
use tracing::{error, info, warn};

use crate::backend::DrmBackend;
use crate::error::ServerError;

/// Stato del server nel loop eventi.
pub struct Zahidex {
    backend: DrmBackend,
    /// `true` mentre la sessione è in pausa (cambio VT): niente rendering.
    paused: bool,
    /// `true` quando lo sfondo va ridisegnato al prossimo giro.
    needs_redraw: bool,
    /// `true` se un frame è accodato e attende il VBlank.
    frame_queued: bool,
}

impl Zahidex {
    pub fn new(backend: DrmBackend) -> Self {
        Self {
            backend,
            paused: false,
            needs_redraw: true,
            frame_queued: false,
        }
    }

    /// Entra nel loop eventi. Ritorna solo all'uscita pulita o in caso di
    /// errore del loop.
    pub fn run(
        mut self,
        drm_notifier: DrmDeviceNotifier,
        session_notifier: LibSeatSessionNotifier,
        udev_backend: UdevBackend,
    ) -> Result<(), ServerError> {
        let mut event_loop =
            EventLoop::<Self>::try_new().map_err(|e| ServerError::EventLoop(e.to_string()))?;
        let loop_signal: LoopSignal = event_loop.get_signal();

        // VBlank / errori DRM.
        event_loop
            .handle()
            .insert_source(drm_notifier, |event, _, state: &mut Self| match event {
                DrmEvent::VBlank(_crtc) => {
                    if state.frame_queued {
                        if let Err(e) = state.backend.frame_submitted() {
                            error!("frame_submitted fallito: {e}");
                        }
                        state.frame_queued = false;
                    }
                }
                DrmEvent::Error(e) => error!("errore drm: {e}"),
            })
            .map_err(|e| ServerError::EventLoop(e.to_string()))?;

        // Pausa / riattivazione della sessione (cambio VT via libseat).
        event_loop
            .handle()
            .insert_source(session_notifier, |event, _, state: &mut Self| match event {
                SessionEvent::PauseSession => {
                    info!("sessione in pausa (cambio VT): rilascio DRM master");
                    state.backend.device.pause();
                    state.paused = true;
                }
                SessionEvent::ActivateSession => {
                    info!("sessione riattivata: riacquisisco DRM master");
                    match state.backend.device.activate(false) {
                        Ok(()) => {
                            state.paused = false;
                            state.needs_redraw = true;
                        }
                        Err(e) => error!("riattivazione DRM fallita: {e}"),
                    }
                }
            })
            .map_err(|e| ServerError::EventLoop(e.to_string()))?;

        // Hotplug udev: log di tutto; se sparisce la nostra GPU, spegni.
        let stop_on_gpu_removed = loop_signal.clone();
        let gpu_rdev = self.backend.gpu_rdev;
        event_loop
            .handle()
            .insert_source(
                udev_backend,
                move |event, _, _state: &mut Self| match event {
                    UdevEvent::Added { device_id, path } => {
                        info!("udev: dispositivo aggiunto {device_id} {}", path.display())
                    }
                    UdevEvent::Changed { device_id } => {
                        info!("udev: dispositivo cambiato {device_id}")
                    }
                    UdevEvent::Removed { device_id } => {
                        warn!("udev: dispositivo rimosso {device_id}");
                        if device_id == gpu_rdev {
                            error!("la GPU è stata rimossa: spengo il server");
                            stop_on_gpu_removed.stop();
                        }
                    }
                },
            )
            .map_err(|e| ServerError::EventLoop(e.to_string()))?;

        // Ctrl+C / SIGTERM -> fermano il loop.
        let stop_on_signal = loop_signal.clone();
        event_loop
            .handle()
            .insert_source(
                Signals::new(&[Signal::SIGINT, Signal::SIGTERM])
                    .map_err(|e| ServerError::EventLoop(e.to_string()))?,
                move |_, _, _state: &mut Self| {
                    info!("segnale di uscita ricevuto");
                    stop_on_signal.stop();
                },
            )
            .map_err(|e| ServerError::EventLoop(e.to_string()))?;

        info!("zahidex in esecuzione (Ctrl+C per uscire)");
        event_loop
            .run(None, &mut self, |state| {
                if state.needs_redraw && !state.paused {
                    match state.backend.render_background() {
                        Ok(queued) => {
                            state.frame_queued = queued;
                            state.needs_redraw = false;
                        }
                        Err(e) => {
                            // Non uscire: un frame fallito non deve uccidere
                            // il server (il prossimo giro riprova solo se
                            // qualcosa lo richiede).
                            error!("render dello sfondo fallito: {e}");
                            state.needs_redraw = false;
                        }
                    }
                }
            })
            .map_err(|e| ServerError::EventLoop(e.to_string()))?;

        info!("uscita pulita: rilascio DRM master e sessione");
        Ok(())
    }
}
