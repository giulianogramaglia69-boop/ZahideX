// SPDX-License-Identifier: GPL-3.0-or-later
//! IPC ZahideX: accetta client sul socket e gestisce l'handshake.
//!
//! Fase 2: solo handshake + log degli altri messaggi (le finestre sono
//! Fase 4). Gira in thread dedicati: il loop DRM resta single-threaded e
//! non si blocca mai su un client lento.

use std::time::Duration;

use tracing::{info, warn};
use zahidex_ipc::{Connection, Server};
use zahidex_protocol::{ClientMessage, ServerMessage};

use crate::error::ServerError;
use crate::handshake::handle_hello;

/// Avvia il server IPC in un thread dedicato e ritorna subito.
///
/// Il socket vive in `$XDG_RUNTIME_DIR/zahidex-0` (o `/tmp/zahidex-0`).
/// `output_size` è la dimensione dell'output, comunicata ai client in
/// `HelloOk`.
pub fn start_ipc(output_size: (u32, u32)) -> Result<(), ServerError> {
    let server = Server::bind_default()?;
    info!("ipc in ascolto su {:?}", server.socket_path());
    std::thread::Builder::new()
        .name("zahidex-ipc".into())
        .spawn(move || {
            loop {
                match server.accept() {
                    Ok(conn) => {
                        if std::thread::Builder::new()
                            .name("zahidex-client".into())
                            .spawn(move || handle_client(conn, output_size))
                            .is_err()
                        {
                            warn!("ipc: impossibile creare il thread per un client");
                        }
                    }
                    Err(e) => {
                        warn!("ipc: accept fallito, chiudo il listener: {e}");
                        break;
                    }
                }
            }
        })?;
    Ok(())
}

/// Gestisce un client: handshake, poi lettura e log dei messaggi.
fn handle_client(mut conn: Connection, output_size: (u32, u32)) {
    // Timeout breve solo per l'handshake: un client che non saluta non deve
    // bloccare il thread per sempre.
    if conn.set_read_timeout(Some(Duration::from_secs(5))).is_err() {
        return;
    }
    let first = match conn.recv::<ClientMessage>() {
        Ok((msg, _fds)) => msg,
        Err(e) => {
            info!("client disconnesso durante l'handshake: {e}");
            return;
        }
    };
    let reply = handle_hello(&first, output_size.0, output_size.1);
    let ok = matches!(reply, ServerMessage::HelloOk { .. });
    if let Err(e) = conn.send(&reply) {
        info!("invio HelloOk/HelloError fallito: {e}");
        return;
    }
    if !ok {
        return;
    }
    if let ClientMessage::Hello { client_name, .. } = &first {
        info!("client connesso: {client_name}");
    }
    // Dopo l'handshake: lettura bloccante, solo log (Fase 2: niente finestre).
    let _ = conn.set_read_timeout(None);
    loop {
        match conn.recv::<ClientMessage>() {
            Ok((msg, fds)) => {
                info!(
                    "ipc: messaggio ignorato (Fase 2): {msg:?} (fd allegati: {})",
                    fds.len()
                );
            }
            Err(e) => {
                info!("client disconnesso: {e}");
                return;
            }
        }
    }
}
