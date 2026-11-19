// SPDX-License-Identifier: GPL-3.0-or-later
//! IPC ZahideX: accetta client sul socket e gestisce l'handshake.
//!
//! Fase 3: dopo l'handshake il client viene registrato nel `FocusRegistry`
//! (il primo connesso riceve gli eventi di input) e il thread svuota la
//! coda dei messaggi in uscita mentre aspetta il prossimo messaggio, con
//! `wait_readable` (niente timeout di lettura a metà frame). Le finestre
//! sono Fase 4: i messaggi dei client vengono ancora solo registrati.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};
use zahidex_ipc::{Connection, Server};
use zahidex_protocol::{ClientMessage, ServerMessage};

use crate::error::ServerError;
use crate::focus::FocusRegistry;
use crate::handshake::handle_hello;

/// Avvia il server IPC in un thread dedicato e ritorna subito.
///
/// Il socket vive in `$XDG_RUNTIME_DIR/zahidex-0` (o `/tmp/zahidex-0`).
/// `output_size` è la dimensione dell'output, comunicata ai client in
/// `HelloOk`. `focus` è il registro condiviso con il loop principale.
pub fn start_ipc(output_size: (u32, u32), focus: Arc<FocusRegistry>) -> Result<(), ServerError> {
    let server = Server::bind_default()?;
    info!("ipc in ascolto su {:?}", server.socket_path());
    std::thread::Builder::new()
        .name("zahidex-ipc".into())
        .spawn(move || {
            loop {
                match server.accept() {
                    Ok(conn) => {
                        let focus = Arc::clone(&focus);
                        if std::thread::Builder::new()
                            .name("zahidex-client".into())
                            .spawn(move || handle_client(conn, output_size, focus))
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

/// Gestisce un client: handshake, registrazione nel focus, poi lettura dei
/// messaggi e invio della coda in uscita verso il client focalizzato.
fn handle_client(mut conn: Connection, output_size: (u32, u32), focus: Arc<FocusRegistry>) {
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
    let _ = conn.set_read_timeout(None);

    // Registrazione nel focus: il primo client connesso riceve l'input.
    let (client_id, outbound) = focus.register();
    if focus.focused() == Some(client_id) {
        let _ = focus.send_to_focused(ServerMessage::Focus { window_id: None });
    }

    // Loop: aspetta dati leggibili (altrimenti svuota la coda in uscita),
    // poi legge un messaggio intero. `wait_readable` evita il timeout di
    // lettura, che potrebbe scattare a metà di un frame.
    'client: loop {
        match conn.wait_readable(Duration::from_millis(20)) {
            Ok(true) => match conn.recv::<ClientMessage>() {
                Ok((msg, fds)) => {
                    info!(
                        "ipc: messaggio ignorato (Fase 3): {msg:?} (fd allegati: {})",
                        fds.len()
                    );
                }
                Err(e) => {
                    info!("client disconnesso: {e}");
                    break 'client;
                }
            },
            Ok(false) => {
                // Nessun dato in arrivo: svuota la coda in uscita.
                while let Ok(out) = outbound.try_recv() {
                    if let Err(e) = conn.send(&out) {
                        info!("client disconnesso in invio: {e}");
                        break 'client;
                    }
                }
            }
            Err(e) => {
                info!("client disconnesso (poll): {e}");
                break 'client;
            }
        }
    }

    // Disconnessione: se era il client focalizzato, il focus passa al
    // prossimo e va notificato.
    let was_focused = focus.focused() == Some(client_id);
    focus.unregister(client_id);
    if was_focused && focus.client_count() > 0 {
        let _ = focus.send_to_focused(ServerMessage::Focus { window_id: None });
    }
}
