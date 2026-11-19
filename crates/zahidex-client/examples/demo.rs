// SPDX-License-Identifier: GPL-3.0-or-later
//! `zahideX-demo` — client demo interattivo per ZahideX.
//!
//! Una finestra con gradiente di sfondo e un quadrato arancione che si
//! muove. Il demo usa solo la libreria `zahidex-client`: è l'esempio di
//! riferimento per scrivere un client ZahideX.
//!
//! Uso (sul laptop, con il server in esecuzione su un'altra TTY):
//! ```sh
//! ./target/release/zahidex-demo
//! ```
//!
//! Tasti:
//! - frecce o WASD: muovono il quadrato (24 px per pressione)
//! - click sinistro: il quadrato salta sotto il cursore
//! - click sul pulsante rosso "X" in alto a destra: chiude la finestra
//! - Q oppure Esc: chiude la finestra ed esce

use std::time::Duration;

use zahidex_client::{Client, Event, Window};
use zahidex_protocol::TITLE_BAR_HEIGHT;

// Keysym XKB (indipendenti dal layout per i tasti speciali; le lettere
// sono quelle del layout italiano, uguali a quelle US per WASD/Q).
const KEYSYM_ESCAPE: u32 = 0xff1b;
const KEYSYM_LEFT: u32 = 0xff51;
const KEYSYM_UP: u32 = 0xff52;
const KEYSYM_RIGHT: u32 = 0xff53;
const KEYSYM_DOWN: u32 = 0xff54;
const KEYSYM_Q: u32 = 0x71;
const KEYSYM_W: u32 = 0x77;
const KEYSYM_A: u32 = 0x61;
const KEYSYM_S: u32 = 0x73;
const KEYSYM_D: u32 = 0x64;

/// Bottone sinistro del mouse (evdev BTN_LEFT).
const BTN_LEFT: u32 = 0x110;

/// Lato del quadrato e passo di movimento (px).
const SQUARE_SIZE: i32 = 80;
const STEP: i32 = 24;

/// Pulsante di chiusura (in coordinate del contenuto, in alto a destra).
const CLOSE_WIDTH: i32 = 96;
const CLOSE_HEIGHT: i32 = 30;
const CLOSE_MARGIN: i32 = 12;

/// Stato del demo.
struct Demo {
    /// Posizione (angolo in alto a sinistra) del quadrato, in coordinate
    /// del contenuto.
    square_x: i32,
    square_y: i32,
    /// Posizione corrente del puntatore, in coordinate del contenuto.
    pointer_x: i32,
    pointer_y: i32,
    /// Il puntatore è sopra il pulsante di chiusura.
    hover_close: bool,
}

impl Demo {
    fn new(window: &Window) -> Self {
        Self {
            square_x: (window.width() as i32 - SQUARE_SIZE) / 2,
            square_y: (window.content_height() as i32 - SQUARE_SIZE) / 2,
            pointer_x: window.width() as i32 / 2,
            pointer_y: window.content_height() as i32 / 2,
            hover_close: false,
        }
    }

    /// Rettangolo del pulsante di chiusura, per la larghezza corrente.
    fn close_rect(&self, width: u32) -> (i32, i32, i32, i32) {
        let x = width as i32 - CLOSE_MARGIN - CLOSE_WIDTH;
        (x, CLOSE_MARGIN, CLOSE_WIDTH, CLOSE_HEIGHT)
    }

    fn close_contains(&self, width: u32, x: i32, y: i32) -> bool {
        let (rx, ry, rw, rh) = self.close_rect(width);
        x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }

    /// Muove il quadrato di (dx, dy), tenendolo dentro l'area di contenuto.
    fn move_square(&mut self, window: &Window, dx: i32, dy: i32) {
        let max_x = window.width() as i32 - SQUARE_SIZE;
        let max_y = window.content_height() as i32 - SQUARE_SIZE;
        self.square_x = (self.square_x + dx).clamp(0, max_x.max(0));
        self.square_y = (self.square_y + dy).clamp(0, max_y.max(0));
    }

    /// Centra il quadrato sul punto cliccato (coordinate del contenuto).
    fn jump_to(&mut self, window: &Window, x: i32, y: i32) {
        let max_x = window.width() as i32 - SQUARE_SIZE;
        let max_y = window.content_height() as i32 - SQUARE_SIZE;
        self.square_x = (x - SQUARE_SIZE / 2).clamp(0, max_x.max(0));
        self.square_y = (y - SQUARE_SIZE / 2).clamp(0, max_y.max(0));
    }

    /// Ridisegna tutto il contenuto: gradiente, quadrato, pulsante.
    fn draw(&self, window: &Window) -> Result<(), String> {
        let state = self;
        window
            .commit_with(|pixels, width, height, stride| {
                draw_frame(pixels, width, height, stride, state);
            })
            .map_err(|e| format!("commit: {e}"))
    }
}

/// Disegna un frame nel buffer: gradiente verticale, quadrato arancione
/// con bordo bianco, pulsante di chiusura con X bianca.
fn draw_frame(pixels: &mut [u8], width: u32, height: u32, stride: u32, state: &Demo) {
    // Gradiente verticale: blu scuro #0B1D33 in alto → blu petrolio #105070
    // in basso.
    let (tr, tg, tb) = (11u8, 29u8, 51u8);
    let (br, bg, bb) = (16u8, 80u8, 112u8);
    for row in 0..height as usize {
        let t = if height > 1 {
            row as f32 / (height - 1) as f32
        } else {
            0.0
        };
        let r = lerp(tr, br, t);
        let g = lerp(tg, bg, t);
        let b = lerp(tb, bb, t);
        let start = row * stride as usize;
        for col in 0..width as usize {
            let px = start + col * 4;
            pixels[px] = b;
            pixels[px + 1] = g;
            pixels[px + 2] = r;
            pixels[px + 3] = 0xff;
        }
    }

    // Quadrato arancione #F59E0B con bordo bianco di 2 px.
    let square = Rect2d {
        x: state.square_x,
        y: state.square_y,
        w: SQUARE_SIZE,
        h: SQUARE_SIZE,
    };
    fill_rect(pixels, stride, square, (245, 158, 11));
    draw_border(pixels, stride, square, 2, (255, 255, 255));

    // Pulsante di chiusura: rosso scuro, più chiaro quando il puntatore
    // ci sta sopra, con una X bianca al centro.
    let (cx, cy, cw, ch) = state.close_rect(width);
    let color = if state.hover_close {
        (239, 68, 68)
    } else {
        (220, 38, 38)
    };
    fill_rect(
        pixels,
        stride,
        Rect2d {
            x: cx,
            y: cy,
            w: cw,
            h: ch,
        },
        color,
    );
    // X: due diagonali spesse 3 px nel quadrato centrale 12x12.
    let x0 = cx + cw / 2 - 6;
    let y0 = cy + ch / 2 - 6;
    for i in 0..12 {
        for t in 0..3 {
            put_pixel(pixels, stride, x0 + i, y0 + i + t - 1, (255, 255, 255));
            put_pixel(pixels, stride, x0 + 11 - i, y0 + i + t - 1, (255, 255, 255));
        }
    }
}

/// Interpolazione lineare tra due componenti di colore.
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

/// Scrive un pixel XRGB8888 (coordinate assolute nel buffer).
fn put_pixel(pixels: &mut [u8], stride: u32, x: i32, y: i32, color: (u8, u8, u8)) {
    if x < 0 || y < 0 {
        return;
    }
    let (r, g, b) = color;
    let idx = y as usize * stride as usize + x as usize * 4;
    if idx + 3 < pixels.len() {
        pixels[idx] = b;
        pixels[idx + 1] = g;
        pixels[idx + 2] = r;
        pixels[idx + 3] = 0xff;
    }
}

/// Rettangolo in coordinate intere (per raggruppare gli argomenti delle
/// funzioni di disegno ed evitare il lint `too_many_arguments`).
#[derive(Debug, Clone, Copy)]
struct Rect2d {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Riempie un rettangolo (coordinate assolute, ritagliato al buffer).
fn fill_rect(pixels: &mut [u8], stride: u32, rect: Rect2d, color: (u8, u8, u8)) {
    for row in rect.y..rect.y + rect.h {
        for col in rect.x..rect.x + rect.w {
            put_pixel(pixels, stride, col, row, color);
        }
    }
}

/// Disegna il bordo di un rettangolo (spessore in px).
fn draw_border(pixels: &mut [u8], stride: u32, rect: Rect2d, thickness: i32, color: (u8, u8, u8)) {
    for t in 0..thickness {
        // Lati orizzontali.
        for col in rect.x..rect.x + rect.w {
            put_pixel(pixels, stride, col, rect.y + t, color);
            put_pixel(pixels, stride, col, rect.y + rect.h - 1 - t, color);
        }
        // Lati verticali.
        for row in rect.y..rect.y + rect.h {
            put_pixel(pixels, stride, rect.x + t, row, color);
            put_pixel(pixels, stride, rect.x + rect.w - 1 - t, row, color);
        }
    }
}

fn main() -> Result<(), String> {
    let mut client = Client::connect_as("zahidex-demo").map_err(|e| format!("connessione: {e}"))?;
    let (out_w, out_h) = client.output_size();
    println!(
        "connesso: protocollo v{}, output {out_w}x{out_h}",
        client.version()
    );
    println!("frecce/WASD muovono il quadrato, click lo sposta, X rossa o Q/Esc chiudono");

    let window = client
        .create_window("ZahideX Demo", 800, 600, false)
        .map_err(|e| format!("CreateWindow: {e}"))?;
    println!(
        "finestra {:?}: {}x{}",
        window.id(),
        window.width(),
        window.height()
    );

    let mut demo = Demo::new(&window);
    demo.draw(&window)?;

    loop {
        let events = client
            .poll_events(Some(Duration::from_millis(100)))
            .map_err(|e| format!("eventi: {e}"))?;
        let mut dirty = false;
        for event in events {
            match event {
                Event::Configure { width, height, .. } => {
                    println!("Configure: {width}x{height}");
                    demo.square_x = demo.square_x.clamp(0, (width as i32 - SQUARE_SIZE).max(0));
                    demo.square_y = demo.square_y.clamp(
                        0,
                        (height as i32 - TITLE_BAR_HEIGHT as i32 - SQUARE_SIZE).max(0),
                    );
                    dirty = true;
                }
                Event::Focus { window_id } => {
                    println!("focus: {window_id:?}");
                }
                Event::PointerEnter { .. } | Event::PointerLeave { .. } => {}
                Event::PointerMotion { x, y, .. } => {
                    // Le coordinate sono relative alla finestra (barra del
                    // titolo inclusa): il contenuto inizia sotto di essa.
                    let cx = x;
                    let cy = y - TITLE_BAR_HEIGHT as i32;
                    demo.pointer_x = cx;
                    demo.pointer_y = cy;
                    let hover = demo.close_contains(window.width(), cx, cy);
                    if hover != demo.hover_close {
                        demo.hover_close = hover;
                        dirty = true;
                    }
                }
                Event::PointerButton {
                    button, pressed, ..
                } => {
                    if button == BTN_LEFT && pressed {
                        if demo.close_contains(window.width(), demo.pointer_x, demo.pointer_y) {
                            println!("click sul pulsante di chiusura: esco");
                            window
                                .destroy()
                                .map_err(|e| format!("DestroyWindow: {e}"))?;
                            return Ok(());
                        }
                        demo.jump_to(&window, demo.pointer_x, demo.pointer_y);
                        dirty = true;
                    }
                }
                Event::PointerAxis { .. } => {}
                Event::Key {
                    keysym, pressed, ..
                } => {
                    if !pressed {
                        continue;
                    }
                    match keysym {
                        KEYSYM_Q | KEYSYM_ESCAPE => {
                            println!("chiusura richiesta da tastiera: esco");
                            window
                                .destroy()
                                .map_err(|e| format!("DestroyWindow: {e}"))?;
                            return Ok(());
                        }
                        KEYSYM_LEFT | KEYSYM_A => {
                            demo.move_square(&window, -STEP, 0);
                            dirty = true;
                        }
                        KEYSYM_RIGHT | KEYSYM_D => {
                            demo.move_square(&window, STEP, 0);
                            dirty = true;
                        }
                        KEYSYM_UP | KEYSYM_W => {
                            demo.move_square(&window, 0, -STEP);
                            dirty = true;
                        }
                        KEYSYM_DOWN | KEYSYM_S => {
                            demo.move_square(&window, 0, STEP);
                            dirty = true;
                        }
                        _ => {}
                    }
                }
                Event::CloseWindow { window_id } => {
                    println!("il server chiude {window_id:?}: esco");
                    return Ok(());
                }
                Event::ServerError { message } => {
                    eprintln!("errore dal server: {message}");
                }
            }
        }
        if dirty {
            demo.draw(&window)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// I pixel disegnati sono quelli attesi: gradiente in alto e in basso,
    /// quadrato arancione al centro, pulsante di chiusura rosso.
    /// (XRGB8888: byte B, G, R, X.)
    #[test]
    fn gradient_square_and_close_button_pixels() {
        let width = 400u32;
        let height = 300u32;
        let stride = width * 4;
        let mut pixels = vec![0u8; (stride * height) as usize];
        let state = Demo {
            square_x: 10,
            square_y: 10,
            pointer_x: 0,
            pointer_y: 0,
            hover_close: false,
        };
        draw_frame(&mut pixels, width, height, stride, &state);

        // Angolo in alto a sinistra: alto del gradiente #0B1D33.
        assert_eq!(&pixels[0..4], &[51, 29, 11, 255]);
        // Ultima riga: basso del gradiente #105070.
        let last = (height as usize - 1) * stride as usize;
        assert_eq!(&pixels[last..last + 4], &[112, 80, 16, 255]);
        // Centro del quadrato (50, 50): arancione #F59E0B.
        let idx = 50 * stride as usize + 50 * 4;
        assert_eq!(&pixels[idx..idx + 4], &[11, 158, 245, 255]);
        // Bordo del quadrato (10, 10): bianco.
        let idx = 10 * stride as usize + 10 * 4;
        assert_eq!(&pixels[idx..idx + 4], &[255, 255, 255, 255]);
        // Pulsante di chiusura: rosso #DC2626.
        let (cx, cy, _, _) = state.close_rect(width);
        let idx = (cy as usize + 5) * stride as usize + (cx as usize + 5) * 4;
        assert_eq!(&pixels[idx..idx + 4], &[38, 38, 220, 255]);
    }
}
