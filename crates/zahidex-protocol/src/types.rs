// SPDX-License-Identifier: GPL-3.0-or-later
use serde::{Deserialize, Serialize};

/// Identificatore di una finestra, assegnato dal server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId(pub u32);

/// Rettangolo in coordinate intere (pixel, origine in alto a sinistra).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /// Costruisce un rettangolo da origine e dimensioni.
    #[must_use]
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// Formato pixel dei buffer condivisi. In v0.1 c'è un solo formato.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    /// 32 bit per pixel, little-endian; il byte più significativo è ignorato.
    Xrgb8888,
}

/// Descrizione di un buffer condiviso. L'fd viaggia fuori banda (SCM_RIGHTS),
/// quindi qui ci sono solo i metadati.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BufferInfo {
    pub width: u32,
    pub height: u32,
    /// Byte per riga; deve essere >= `width * 4` e multiplo di 4.
    pub stride: u32,
    pub format: PixelFormat,
}
