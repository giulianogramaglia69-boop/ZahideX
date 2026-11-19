// SPDX-License-Identifier: GPL-3.0-or-later
//! `zahidex` — server grafico ZahideX.
//!
//! Fase 2: accende lo schermo via DRM/KMS e disegna uno sfondo a colore
//! pieno con GlesRenderer (Smithay 0.7, solo backend). Accetta client IPC
//! (solo handshake: le finestre arrivano in Fase 4).
//!
//! Avvio (sul laptop, da una TTY libera):
//! ```sh
//! cargo build --release -p zahidex-server
//! ./target/release/zahidex
//! ```
//! Uscita: Ctrl+C o SIGTERM — il server rilascia DRM master e sessione, e
//! logind ripristina la TTY precedente.

mod backend;
mod error;
mod handshake;
mod ipc;
mod mode;
mod server;

use std::process::ExitCode;

use crate::error::ServerError;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn print_help() {
    println!("zahidex {VERSION} — server grafico ZahideX (Fase 2)");
    println!();
    println!("Uso: zahidex [opzioni]");
    println!();
    println!("Opzioni:");
    println!("  --dry-run        controlla i prerequisiti senza accendere niente");
    println!("  --seat <nome>    seat logind da usare (predefinito: seat0)");
    println!("  --help, -h       mostra questo aiuto");
    println!("  --version, -V    mostra la versione");
    println!();
    println!("Avvio normale: da una TTY libera (Ctrl+Alt+F3), mai da SSH o da");
    println!("dentro un altro ambiente grafico. Uscita: Ctrl+C o SIGTERM.");
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
    match run(dry_run, &seat) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(dry_run: bool, seat: &str) -> Result<(), ServerError> {
    if dry_run {
        return crate::backend::dry_run(seat);
    }
    let parts = crate::backend::DrmBackend::open(seat)?;
    let output_size = parts.backend.output_size;
    crate::ipc::start_ipc(output_size)?;
    let server = crate::server::Zahidex::new(parts.backend);
    server.run(
        parts.drm_notifier,
        parts.session_notifier,
        parts.udev_backend,
    )
}
