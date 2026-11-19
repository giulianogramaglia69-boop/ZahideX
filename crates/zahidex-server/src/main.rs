// SPDX-License-Identifier: GPL-3.0-or-later
//! `zahidex` — server grafico ZahideX.
//!
//! Fase 4: compositor di finestre (tiling + floating).
//!
//! Accende lo schermo via DRM/KMS e disegna le finestre dei client con
//! GlesRenderer (Smithay 0.7, solo backend): layout tiling (master 60% +
//! stack) e floating, barra del titolo 24 px, bordo di focus 2 px, focus
//! per-finestra, drag della barra del titolo. Input libinput (tastiera con
//! keymap xkb italiana, pointer, touch) e cursore software.
//!
//! Avvio (sul laptop, da una TTY libera):
//! ```sh
//! cargo build --release -p zahidex-server
//! ./target/release/zahidex
//! ```
//! Uscita: Ctrl+C, SIGTERM, Ctrl+Alt+BackSpace o Super+Q senza finestre —
//! il server rilascia DRM master e sessione, e logind ripristina la TTY
//! precedente. Super+Q con finestre chiude la finestra con focus.

mod backend;
mod compositor;
mod cursor;
mod error;
mod focus;
mod handshake;
mod input;
mod ipc;
mod keymap;
mod mode;
mod server;

use std::process::ExitCode;

use crate::compositor::Compositor;
use crate::error::ServerError;
use crate::focus::FocusRegistry;
use crate::keymap::DEFAULT_LAYOUT;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("zahidex {VERSION} — server grafico ZahideX (Fase 4)");
    println!();
    println!("Uso: zahidex [opzioni]");
    println!();
    println!("Opzioni:");
    println!("  --dry-run        controlla i prerequisiti senza accendere niente");
    println!("  --seat <nome>    seat logind da usare (predefinito: seat0)");
    println!("  --layout <nome>  layout tastiera xkb (predefinito: {DEFAULT_LAYOUT})");
    println!("  --help, -h       mostra questo aiuto");
    println!("  --version, -V    mostra la versione");
    println!();
    println!("Avvio normale: da una TTY libera (Ctrl+Alt+F3), mai da SSH o da");
    println!("dentro un altro ambiente grafico. Uscita: Ctrl+C, Ctrl+Alt+BackSpace");
    println!("o Super+Q senza finestre. Scorciatoie finestre: Super+J/K ciclo focus,");
    println!("Super+F tiling<->floating, Super+Q chiude la finestra con focus.");
    println!("Log: stdout (imposta RUST_LOG=debug per i log di Smithay).");
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stdout)
        .try_init();
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dry_run = false;
    let mut seat = "seat0".to_string();
    let mut layout = DEFAULT_LAYOUT.to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--dry-run" => dry_run = true,
            "--seat" => {
                i += 1;
                match args.get(i) {
                    Some(value) => seat = value.clone(),
                    None => {
                        eprintln!("--seat richiede un valore");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--layout" => {
                i += 1;
                match args.get(i) {
                    Some(value) => layout = value.clone(),
                    None => {
                        eprintln!("--layout richiede un valore");
                        return ExitCode::FAILURE;
                    }
                }
            }
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            "--version" | "-V" => {
                println!("zahidex {VERSION}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("opzione sconosciuta: {other}");
                print_help();
                return ExitCode::FAILURE;
            }
        }
        i += 1;
    }

    init_logging();
    match run(dry_run, &seat, &layout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(dry_run: bool, seat: &str, layout: &str) -> Result<(), ServerError> {
    if dry_run {
        return crate::backend::dry_run(seat);
    }
    let parts = crate::backend::DrmBackend::open(seat)?;
    let output_size = parts.backend.output_size;
    let focus = FocusRegistry::new_shared();
    let compositor = std::sync::Arc::new(std::sync::Mutex::new(Compositor::new(
        output_size.0,
        output_size.1,
    )));
    let (wake_tx, wake_rx) = calloop::channel::channel();
    crate::ipc::start_ipc(
        output_size,
        std::sync::Arc::clone(&focus),
        std::sync::Arc::clone(&compositor),
        wake_tx,
    )?;
    let server = crate::server::Zahidex::new(parts.backend, focus, compositor, layout)?;
    server.run(
        parts.drm_notifier,
        parts.session_notifier,
        parts.udev_backend,
        parts.input_backend,
        wake_rx,
    )
}
