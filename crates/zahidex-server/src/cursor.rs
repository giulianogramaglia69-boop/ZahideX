// SPDX-License-Identifier: GPL-3.0-or-later
//! Cursore software: posizione, movimento e immagine (freccia).
//!
//! Il cursore è un `MemoryBuffer` 16x16 (freccia bianca con bordo nero)
//! disegnato dal renderer GLES sopra lo sfondo. La logica di posizione è
//! pura e testabile senza hardware: movimento relativo (mouse) con clamp
//! ai bordi dell'output, posizionamento assoluto (touch/tablet) con clamp.

use smithay::backend::renderer::element::memory::MemoryBuffer;

/// Dimensione del cursore in pixel (quadrato).
pub const CURSOR_SIZE: i32 = 16;

/// Freccia del cursore, 16 righe da 16 caratteri: `X` = pixel della
/// freccia, `.` = trasparente. La freccia punta in alto a sinistra; la
/// coda va in basso a destra con la tacca classica.
const ARROW: [&str; 16] = [
    "X...............",
    "XX..............",
    "XXX.............",
    "XXXX............",
    "XXXXX...........",
    "XXXXXX..........",
    "XXXXXXX.........",
    "XXXXXXXX........",
    "XXXXXXXXX.......",
    "XXXXXX..........",
    "XXX.XXX.........",
    "XX...XXX........",
    "X.....XXX.......",
    ".......XXX......",
    ".......XXX......",
    "........XXX.....",
];

/// Stato del cursore: posizione in coordinate dell'output (pixel, origine
/// in alto a sinistra) e visibilità.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cursor {
    x: f64,
    y: f64,
    width: u32,
    height: u32,
    visible: bool,
}

impl Cursor {
    /// Crea un cursore visibile al centro dell'output `width` x `height`.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            x: f64::from(width) / 2.0,
            y: f64::from(height) / 2.0,
            width,
            height,
            visible: true,
        }
    }

    /// Posizione attuale in pixel (arrotondata).
    #[must_use]
    pub fn position(&self) -> (i32, i32) {
        (self.x.round() as i32, self.y.round() as i32)
    }

    /// `true` se il cursore va disegnato.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// Movimento relativo (mouse): somma il delta e blocca ai bordi.
    pub fn move_relative(&mut self, dx: f64, dy: f64) {
        self.x = (self.x + dx).clamp(0.0, f64::from(self.width.saturating_sub(1)));
        self.y = (self.y + dy).clamp(0.0, f64::from(self.height.saturating_sub(1)));
        self.visible = true;
    }

    /// Posizionamento assoluto (touch/tablet): imposta la posizione con clamp.
    pub fn move_absolute(&mut self, x: f64, y: f64) {
        self.x = x.clamp(0.0, f64::from(self.width.saturating_sub(1)));
        self.y = y.clamp(0.0, f64::from(self.height.saturating_sub(1)));
        self.visible = true;
    }
}

/// Byte dell'immagine della freccia (ABGR8888, R,G,B,A in memoria).
///
/// I pixel della freccia sono bianchi con bordo nero da 1px (un pixel è
/// bordo se almeno un vicino negli 8 intorno è trasparente); il resto è
/// trasparente. Logica pura, testabile senza hardware.
#[must_use]
pub fn cursor_image_bytes() -> Vec<u8> {
    let mut bytes = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    for (y, row) in ARROW.iter().enumerate() {
        for (x, cell) in row.bytes().enumerate() {
            if cell != b'X' {
                continue;
            }
            let is_border = (-1..=1).any(|dy| {
                (-1..=1).any(|dx| {
                    if dx == 0 && dy == 0 {
                        return false;
                    }
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx < 0
                        || ny < 0
                        || nx >= CURSOR_SIZE
                        || ny >= CURSOR_SIZE
                        || ARROW[ny as usize].as_bytes()[nx as usize] != b'X'
                })
            });
            let offset = ((y as i32 * CURSOR_SIZE + x as i32) * 4) as usize;
            if is_border {
                bytes[offset..offset + 4].copy_from_slice(&[0, 0, 0, 0xff]);
            } else {
                bytes[offset..offset + 4].copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
            }
        }
    }
    bytes
}

/// Costruisce l'immagine della freccia come buffer di memoria ABGR8888.
#[must_use]
pub fn cursor_image() -> MemoryBuffer {
    MemoryBuffer::from_slice(
        &cursor_image_bytes(),
        smithay::backend::allocator::Fourcc::Abgr8888,
        (CURSOR_SIZE, CURSOR_SIZE),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_centered_and_visible() {
        let cursor = Cursor::new(1920, 1080);
        assert_eq!(cursor.position(), (960, 540));
        assert!(cursor.visible());
    }

    #[test]
    fn relative_motion_clamps_to_output_edges() {
        let mut cursor = Cursor::new(800, 600);
        cursor.move_relative(10_000.0, 0.0);
        assert_eq!(cursor.position().0, 799);
        cursor.move_relative(-10_000.0, -10_000.0);
        assert_eq!(cursor.position(), (0, 0));
        cursor.move_relative(0.0, 10_000.0);
        assert_eq!(cursor.position().1, 599);
    }

    #[test]
    fn absolute_position_clamps() {
        let mut cursor = Cursor::new(800, 600);
        cursor.move_absolute(-50.0, 9999.0);
        assert_eq!(cursor.position(), (0, 599));
        cursor.move_absolute(400.5, 300.4);
        assert_eq!(cursor.position(), (401, 300));
    }

    #[test]
    fn cursor_image_has_arrow_with_border() {
        let mem = cursor_image_bytes();
        assert_eq!(mem.len(), (CURSOR_SIZE * CURSOR_SIZE * 4) as usize);
        // La punta (0,0) è bordo nero, un pixel interno (1,3) è bianco
        // (la freccia si allarga verso il basso: (1,1) e (1,2) sono ancora
        // bordo), l'angolo opposto (15,15) è trasparente. I byte sono R,G,B,A.
        let px = |x: i32, y: i32| -> [u8; 4] {
            let off = ((y * CURSOR_SIZE + x) * 4) as usize;
            [mem[off], mem[off + 1], mem[off + 2], mem[off + 3]]
        };
        assert_eq!(px(0, 0), [0, 0, 0, 0xff]);
        assert_eq!(px(1, 1), [0, 0, 0, 0xff]);
        assert_eq!(px(1, 3), [0xff, 0xff, 0xff, 0xff]);
        assert_eq!(px(15, 15), [0, 0, 0, 0]);
        let has_white = (0..CURSOR_SIZE * CURSOR_SIZE)
            .any(|i| mem[i as usize * 4 + 3] == 0xff && mem[i as usize * 4] == 0xff);
        let has_black = (0..CURSOR_SIZE * CURSOR_SIZE).any(|i| {
            mem[i as usize * 4 + 3] == 0xff
                && mem[i as usize * 4] == 0
                && mem[i as usize * 4 + 1] == 0
        });
        assert!(has_white && has_black, "la freccia ha sia bianco che nero");
    }
}
