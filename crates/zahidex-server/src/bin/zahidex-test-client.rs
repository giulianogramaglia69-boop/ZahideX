// SPDX-License-Identifier: GPL-3.0-or-later
//! `zahidex-test-client` — client di test minimale per il compositor.
//!
//! Crea due finestre (una rossa, una verde), committa i loro buffer,
//! riceve i `Configure` del server e ricommitta alla nuova dimensione,
//! risponde ai `Ping` e stampa gli eventi di input che riceve.
//!
//! Uso (sul laptop, con il server in esecuzione su un'altra TTY):
//! ```sh
//! ./target/release/zahidex-test-client [secondi]
//! ```
//! Senza argomenti resta in ascolto finché il server non chiude le
//! finestre o finché non premi Ctrl+C (la disconnessione fa distruggere
//! le finestre dal server). Con un numero di secondi, dopo quel tempo
//! distrugge le finestre ed esce da solo.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use zahidex_ipc::{ShmBuffer, connect_default};
use zahidex_protocol::{
    ClientMessage, PROTOCOL_VERSION, Rect, ServerMessage, TITLE_BAR_HEIGHT, WindowId,
};

/// Una finestra del client: buffer + colore + dimensioni correnti.
struct TestWindow {
    buffer: ShmBuffer,
    color: (u8, u8, u8),
    width: u32,
    height: u32,
}

impl TestWindow {
    /// Alloca il buffer (area di contenuto: altezza totale meno la barra
    /// del titolo) e lo riempie di un colore pieno.
    fn new(width: u32, height: u32, color: (u8, u8, u8)) -> Result<Self, String> {
        let content_height = height.saturating_sub(TITLE_BAR_HEIGHT).max(1);
        let buffer = ShmBuffer::new(width, content_height)
            .map_err(|e| format!("allocazione buffer: {e}"))?;
        let mut window = Self {
            buffer,
            color,
            width,
            height,
        };
        window.fill();
        Ok(window)
    }

    /// Riempie il buffer di un colore pieno (XRGB8888: byte B, G, R, X).
    fn fill(&mut self) {
        let (r, g, b) = self.color;
        let stride = self.buffer.stride() as usize;
        let width = self.buffer.width() as usize;
        let height = self.buffer.height() as usize;
        if let Ok(mut map) = self.buffer.map_mut() {
            for row in 0..height {
                let start = row * stride;
                for col in 0..width {
                    let px = start + col * 4;
                    map[px] = b;
                    map[px + 1] = g;
                    map[px + 2] = r;
                    map[px + 3] = 0xff;
                }
            }
        }
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let duration = args
        .first()
        .map(|a| a.parse::<u64>().map_err(|_| "argomento: numero di secondi"))
        .transpose()?;
    let deadline = duration.map(|s| Instant::now() + Duration::from_secs(s));

    let mut conn = connect_default().map_err(|e| format!("connessione: {e}"))?;
    conn.send(&ClientMessage::Hello {
        versions: vec![PROTOCOL_VERSION],
        client_name: "zahidex-test-client".into(),
    })
    .map_err(|e| format!("invio Hello: {e}"))?;
    let (hello, _) = conn
        .recv::<ServerMessage>()
        .map_err(|e| format!("ricezione HelloOk: {e}"))?;
    let ServerMessage::HelloOk {
        version,
        output_width,
        output_height,
    } = hello
    else {
        return Err(format!("handshake fallito: {hello:?}"));
    };
    println!("connesso: protocollo v{version}, output {output_width}x{output_height}");

    // Due finestre: la prima diventa master (sinistra), la seconda stack
    // (destra). I colori distinguono le finestre a colpo d'occhio.
    conn.send(&ClientMessage::CreateWindow {
        width: 800,
        height: 600,
        title: "Rosso".into(),
    })
    .map_err(|e| format!("CreateWindow: {e}"))?;
    conn.send(&ClientMessage::CreateWindow {
        width: 800,
        height: 600,
        title: "Verde".into(),
    })
    .map_err(|e| format!("CreateWindow: {e}"))?;

    let mut windows: HashMap<WindowId, TestWindow> = HashMap::new();
    // Colori in ordine di creazione: prima finestra rossa, seconda verde.
    let mut colors = [(255u8, 0u8, 0u8), (0u8, 255u8, 0u8)].into_iter();

    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            println!("tempo scaduto: distruggo le finestre ed esco");
            for id in windows.keys().copied().collect::<Vec<_>>() {
                let _ = conn.send(&ClientMessage::DestroyWindow { window_id: id });
            }
            return Ok(());
        }
        let readable = conn
            .wait_readable(Duration::from_millis(100))
            .map_err(|e| format!("poll: {e}"))?;
        if !readable {
            continue;
        }
        let (msg, _) = conn
            .recv::<ServerMessage>()
            .map_err(|e| format!("ricezione: {e}"))?;
        match msg {
            ServerMessage::WindowCreated {
                window_id,
                width,
                height,
            } => {
                let color = colors.next().unwrap_or((255, 255, 255));
                let window = TestWindow::new(width, height, color)?;
                println!("finestra {window_id:?} creata: {width}x{height}");
                conn.send_commit_buffer(window_id, &window.buffer, Rect::new(0, 0, width, height))
                    .map_err(|e| format!("CommitBuffer: {e}"))?;
                windows.insert(window_id, window);
            }
            ServerMessage::Configure {
                window_id,
                width,
                height,
            } => {
                println!("Configure {window_id:?}: {width}x{height}");
                if let Some(window) = windows.get_mut(&window_id) {
                    if window.width != width || window.height != height {
                        let color = window.color;
                        *window = TestWindow::new(width, height, color)?;
                    }
                    let buffer = &window.buffer;
                    conn.send_commit_buffer(window_id, buffer, Rect::new(0, 0, width, height))
                        .map_err(|e| format!("CommitBuffer: {e}"))?;
                }
            }
            ServerMessage::Focus { window_id } => {
                println!("focus: {window_id:?}");
            }
            ServerMessage::PointerEnter { window_id } => {
                println!("pointer entra in {window_id:?}");
            }
            ServerMessage::PointerLeave { window_id } => {
                println!("pointer esce da {window_id:?}");
            }
            ServerMessage::PointerMotion { window_id, x, y } => {
                println!("pointer in {window_id:?} a ({x},{y})");
            }
            ServerMessage::PointerButton {
                window_id,
                button,
                pressed,
            } => {
                println!(
                    "pulsante {button:#x} {} in {window_id:?}",
                    if pressed { "premuto" } else { "rilasciato" }
                );
            }
            ServerMessage::PointerAxis {
                window_id,
                axis,
                delta_v120,
                delta_px,
            } => {
                println!("scroll {axis:?} in {window_id:?}: v120={delta_v120:?} px={delta_px:?}");
            }
            ServerMessage::Key {
                window_id,
                keycode,
                pressed,
                keysym,
                modifiers,
            } => {
                println!(
                    "tasto evdev={keycode} keysym={keysym:#x} mods={modifiers:#x} {} in {window_id:?}",
                    if pressed { "premuto" } else { "rilasciato" }
                );
            }
            ServerMessage::CloseWindow { window_id } => {
                println!("il server chiude {window_id:?}");
                windows.remove(&window_id);
                if windows.is_empty() {
                    println!("nessuna finestra rimasta: esco");
                    return Ok(());
                }
            }
            ServerMessage::Ping => {
                let _ = conn.send(&ClientMessage::Pong);
            }
            ServerMessage::Error { message } => {
                eprintln!("errore dal server: {message}");
            }
            ServerMessage::HelloOk { .. } | ServerMessage::HelloError { .. } => {
                println!("messaggio inatteso dopo l'handshake: {msg:?}");
            }
        }
    }
}
