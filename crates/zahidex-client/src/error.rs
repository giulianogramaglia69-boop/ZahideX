// SPDX-License-Identifier: GPL-3.0-or-later
//! Errori della libreria client ZahideX.

use thiserror::Error;
use zahidex_ipc::IpcError;

/// Errori del client ZahideX.
#[derive(Debug, Error)]
pub enum ClientError {
    /// Errore di I/O sullo Unix socket o sul memfd.
    #[error("{0}")]
    Io(#[from] IpcError),

    /// Il server ha rifiutato l'handshake (nessuna versione in comune).
    #[error("handshake rifiutato dal server: {reason}")]
    HandshakeRejected { reason: String },

    /// Il server ha inviato un messaggio che non ci si aspettava qui
    /// (es. un secondo `HelloOk` dopo l'handshake).
    #[error("messaggio inatteso dal server: {0}")]
    UnexpectedMessage(String),

    /// Il server ha segnalato un errore per una nostra richiesta
    /// (es. finestra sconosciuta, dimensione non consentita).
    #[error("errore dal server: {message}")]
    ServerError { message: String },

    /// Il server non ha risposto entro il tempo atteso.
    #[error("timeout in attesa di risposta dal server")]
    Timeout,

    /// I pixel passati a `Window::commit` non hanno la dimensione giusta.
    #[error("pixel non validi: attesi {expected} byte per {width}x{height}, ricevuti {actual}")]
    InvalidPixels {
        expected: usize,
        actual: usize,
        width: u32,
        height: u32,
    },

    /// Il server ha chiuso la connessione.
    #[error("il server ha chiuso la connessione")]
    Disconnected,
}

/// Converte un errore IPC: la chiusura dello stream diventa `Disconnected`.
pub(crate) fn map_ipc(error: IpcError) -> ClientError {
    match error {
        IpcError::UnexpectedEof => ClientError::Disconnected,
        other => ClientError::Io(other),
    }
}
