// SPDX-License-Identifier: GPL-3.0-or-later
use thiserror::Error;

use crate::framing::MAX_FRAME_BYTES;

/// Errori del protocollo ZahideX (puri: niente I/O).
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Errore di (de)serializzazione postcard.
    #[error("postcard error: {0}")]
    Postcard(#[from] postcard::Error),

    /// Il frame non inizia con il magic atteso.
    #[error("invalid frame magic: expected ZX01, got {0:?}")]
    InvalidMagic([u8; 4]),

    /// Il payload dichiarato supera il massimo consentito.
    #[error("frame too large: {len} bytes (max {MAX_FRAME_BYTES} bytes)")]
    FrameTooLarge { len: usize },

    /// Il buffer non contiene ancora un frame intero.
    #[error("truncated frame: need {needed} bytes, have {have}")]
    Truncated { needed: usize, have: usize },

    /// Client e server non hanno nessuna versione in comune.
    #[error("no common protocol version: client offered {offered:?}")]
    UnsupportedVersion { offered: Vec<u16> },
}
