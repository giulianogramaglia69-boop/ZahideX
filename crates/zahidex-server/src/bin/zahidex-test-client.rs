// SPDX-License-Identifier: GPL-3.0-or-later
//! `zahidex-test-client` — client di test minimale per il compositor.
//!
//! Crea due finestre (una rossa, una verde), committa i loro buffer,
//! riceve i `Configure` del server e ricommitta alla nuova dimensione,
//! stampa gli eventi di input che riceve. Usa la libreria
//! `zahidex-client` (stesso comportamento della versione grezza della
//! Fase 4, riscritta sopra l'API pubblica).
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

use zahidex_client::{Client, Event, Window};
use zahidex_protocol::WindowId;

/// Una finestra del client: handle della libreria + colore pieno.
struct TestWindow {
    window: Window,
    color: (u8, u8, u8),
}

impl TestWindow {
    /// Riempie il buffer di un colore pieno (XRGB8888: byte B, G, R, X)
    /// e lo committa.
    fn commit(&self) -> Result<(), String> {
        let (r, g, b) = self.color;
        self.window
            .commit_with(|pixels, _width, _height, _stride| {
                for px in pixels.as_chunks_mut::<4>().0 {
                    px[0] = b;
                    px[1] = g;
                    px[2] = r;
                    px[3] = 0xff;
                }
            })
            .map_err(|e| format!("CommitBuffer: {e}"))
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let duration = args
        .first()
        .map(|a| a.parse::<u64>().map_err(|_| "argomento: numero di secondi"))
        .transpose()?;
    let deadline = duration.map(|s| Instant::now() + Duration::from_secs(s));

    let mut client =
        Client::connect_as("zahidex-test-client").map_err(|e| format!("connessione: {e}"))?;
    let (out_w, out_h) = client.output_size();
    println!(
        "connesso: protocollo v{}, output {out_w}x{out_h}",
        client.version()
    );

    // Due finestre: la prima diventa master (sinistra), la seconda stack
    // (destra). I colori distinguono le finestre a colpo d'occhio.
    let red = client
        .create_window("Rosso", 800, 600, false)
        .map_err(|e| format!("CreateWindow: {e}"))?;
    let green = client
        .create_window("Verde", 800, 600, false)
        .map_err(|e| format!("CreateWindow: {e}"))?;

    let mut windows: HashMap<WindowId, TestWindow> = HashMap::new();
    for (window, color) in [(red, (255u8, 0u8, 0u8)), (green, (0u8, 255u8, 0u8))] {
        let id = window.id();
        println!(
            "finestra {id:?} creata: {}x{}",
            window.width(),
            window.height()
        );
        let test_window = TestWindow { window, color };
        test_window.commit()?;
        windows.insert(id, test_window);
    }

    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            println!("tempo scaduto: distruggo le finestre ed esco");
            for (_, test_window) in windows.drain() {
                let _ = test_window.window.destroy();
            }
            return Ok(());
        }
        let events = client
            .poll_events(Some(Duration::from_millis(100)))
            .map_err(|e| format!("eventi: {e}"))?;
        for event in events {
            match event {
                Event::Configure {
                    window_id,
                    width,
                    height,
                } => {
                    println!("Configure {window_id:?}: {width}x{height}");
                    if let Some(window) = windows.get(&window_id) {
                        window.commit()?;
                    }
                }
                Event::Focus { window_id } => {
                    println!("focus: {window_id:?}");
                }
                Event::PointerEnter { window_id } => {
                    println!("pointer entra in {window_id:?}");
                }
                Event::PointerLeave { window_id } => {
                    println!("pointer esce da {window_id:?}");
                }
                Event::PointerMotion { window_id, x, y } => {
                    println!("pointer in {window_id:?} a ({x},{y})");
                }
                Event::PointerButton {
                    window_id,
                    button,
                    pressed,
                } => {
                    println!(
                        "pulsante {button:#x} {} in {window_id:?}",
                        if pressed { "premuto" } else { "rilasciato" }
                    );
                }
                Event::PointerAxis {
                    window_id,
                    axis,
                    delta_v120,
                    delta_px,
                } => {
                    println!(
                        "scroll {axis:?} in {window_id:?}: v120={delta_v120:?} px={delta_px:?}"
                    );
                }
                Event::Key {
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
                Event::CloseWindow { window_id } => {
                    println!("il server chiude {window_id:?}");
                    windows.remove(&window_id);
                    if windows.is_empty() {
                        println!("nessuna finestra rimasta: esco");
                        return Ok(());
                    }
                }
                Event::ServerError { message } => {
                    eprintln!("errore dal server: {message}");
                }
            }
        }
    }
}
