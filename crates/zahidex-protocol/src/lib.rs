// SPDX-License-Identifier: GPL-3.0-or-later
//! Protocollo ZahideX v0.1: tipi dei messaggi, framing e negoziazione versione.
//!
//! Questa crate è pura: niente I/O, niente dipendenze di sistema. Dipende solo
//! da `serde`, `postcard` e `thiserror`.
//!
//! # Framing
//! Ogni messaggio viaggia come frame:
//! `[magic "ZX01" (4 byte)] [lunghezza payload u32 LE] [payload postcard]`.
//!
//! # Versionamento
//! La versione si negozia all'handshake (`Hello` / `HelloOk`). Regole ferree:
//! - i messaggi sono enum chiusi;
//! - le nuove varianti e i nuovi campi si aggiungono solo in coda;
//! - mai rimuovere o rinumerare varianti esistenti.

mod error;
mod framing;
mod messages;
mod types;
mod version;

pub use error::ProtocolError;
pub use framing::{FrameDecoder, MAGIC, MAX_FRAME_BYTES, decode_frame, encode_frame};
pub use messages::{ClientMessage, ServerMessage};
pub use types::{Axis, BufferInfo, PixelFormat, Rect, TITLE_BAR_HEIGHT, WindowId, modifiers};
pub use version::{PROTOCOL_VERSION, SUPPORTED_VERSIONS, negotiate_version};
