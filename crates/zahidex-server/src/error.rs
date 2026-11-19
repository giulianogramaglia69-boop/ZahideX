// SPDX-License-Identifier: GPL-3.0-or-later
//! Errori del server ZahideX.

use thiserror::Error;

/// Errore del server. I messaggi sono pensati per essere letti da un umano
/// sul terminale del laptop (niente codici): dicono cosa è fallito e, quando
/// possibile, cosa fare.
#[derive(Debug, Error)]
pub enum ServerError {
    /// Nessuna GPU trovata sul seat.
    #[error(
        "nessuna GPU trovata sul seat {0} (controlla che /dev/dri esista e che la sessione logind sia attiva)"
    )]
    NoGpu(String),

    /// Nessun output collegato.
    #[error("nessun monitor collegato trovato sulla GPU {0}")]
    NoOutput(String),

    /// La sessione libseat non si apre: quasi sempre significa che il server
    /// non gira da una TTY con sessione logind attiva.
    #[error(
        "sessione libseat non disponibile: {0} (lancia zahidex da una TTY libera, mai da SSH o da dentro un altro ambiente grafico)"
    )]
    Session(String),

    /// Errore DRM/KMS (modesetting, surface, compositor).
    #[error("drm: {0}")]
    Drm(String),

    /// Errore EGL / GLES.
    #[error("egl/gles: {0}")]
    Gl(String),

    /// Errore IPC (socket ZahideX).
    #[error("ipc: {0}")]
    Ipc(#[from] zahidex_ipc::IpcError),

    /// Errore I/O generico.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Errore del loop eventi (calloop).
    #[error("event loop: {0}")]
    EventLoop(String),

    /// Errore generico con messaggio libero.
    #[error("{0}")]
    Message(String),
}
