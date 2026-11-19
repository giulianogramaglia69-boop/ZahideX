// SPDX-License-Identifier: GPL-3.0-or-later
use std::io;

use thiserror::Error;
use zahidex_protocol::ProtocolError;

/// Errori del trasporto IPC di ZahideX.
#[derive(Debug, Error)]
pub enum IpcError {
    /// Errore di I/O sullo Unix socket o sul memfd.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Errore del protocollo (framing, versione, (de)serializzazione).
    #[error("protocol error: {0}")]
    Protocol(#[from] ProtocolError),

    /// Errore di una chiamata di sistema via `nix`.
    #[error("nix error: {0}")]
    Nix(#[from] nix::Error),

    /// Lo stream si è chiuso prima che un frame fosse completo.
    #[error("unexpected end of stream")]
    UnexpectedEof,

    /// Dimensioni del buffer non valide (zero, stride troppo piccolo o overflow).
    #[error("invalid buffer dimensions: {width}x{height} (stride {stride})")]
    InvalidDimensions {
        width: u32,
        height: u32,
        stride: u32,
    },

    /// La dimensione reale del memfd non corrisponde a stride x altezza.
    #[error("buffer size mismatch: expected {expected} bytes, got {actual}")]
    InvalidBufferSize { expected: usize, actual: usize },

    /// I dati ancillari sono stati troncati (troppi fd in un messaggio).
    #[error("ancillary data truncated: too many file descriptors in one message")]
    ControlTruncated,
}
