// SPDX-License-Identifier: GPL-3.0-or-later
//! IPC ZahideX: accetta client sul socket e gestisce handshake + finestre.
//!
//! Fase 4: dopo l'handshake il client viene registrato nel `FocusRegistry`
//! e i suoi messaggi (CreateWindow, CommitBuffer, ...) vengono smistati al
//! `Compositor` condiviso (`Arc<Mutex<...>>` con il loop principale). Le
//! risposte del compositor vanno al client richiedente direttamente sul
//! socket, quelle per altri client passano dal registro focus. Ogni
//! mutazione che cambia qualcosa a video sveglia il loop principale
//! (canale calloop) così il frame viene ridisegnato.
//!
//! Il thread IPC non invia mai messaggi tenendo il lock del compositor:
//! prima raccoglie gli `Outbound`, poi rilascia il lock, poi invia.

use std::os::unix::io::OwnedFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calloop::channel::Sender;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;
use tracing::{debug, info, warn};
use zahidex_ipc::{Connection, Server, ShmBuffer};
use zahidex_protocol::{ClientMessage, PixelFormat, ServerMessage};

use crate::compositor::{Compositor, CompositorError, Outbound};
use crate::error::ServerError;
use crate::focus::{ClientId, FocusRegistry};
use crate::handshake::handle_hello;

/// Avvia il server IPC in un thread dedicato e ritorna subito.
///
/// Il socket vive in `$XDG_RUNTIME_DIR/zahidex-0` (o `/tmp/zahidex-0`).
/// `output_size` è la dimensione dell'output, comunicata ai client in
/// `HelloOk`. `focus` è il registro condiviso con il loop principale,
/// `compositor` il registro finestre condiviso, `wake` sveglia il loop
/// principale quando qualcosa cambia a video.
pub fn start_ipc(
    output_size: (u32, u32),
    focus: Arc<FocusRegistry>,
    compositor: Arc<Mutex<Compositor>>,
    wake: Sender<()>,
) -> Result<(), ServerError> {
    let server = Server::bind_default()?;
    info!("ipc in ascolto su {:?}", server.socket_path());
    std::thread::Builder::new()
        .name("zahidex-ipc".into())
        .spawn(move || {
            loop {
                match server.accept() {
                    Ok(conn) => {
                        let focus = Arc::clone(&focus);
                        let compositor = Arc::clone(&compositor);
                        let wake = wake.clone();
                        if std::thread::Builder::new()
                            .name("zahidex-client".into())
                            .spawn(move || {
                                handle_client(conn, output_size, focus, compositor, wake)
                            })
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

/// Invia i messaggi in uscita: quelli per il client che stiamo servendo
/// vanno direttamente sul socket, gli altri passano dal registro focus.
fn send_outbounds(
    conn: &mut Connection,
    client_id: ClientId,
    focus: &FocusRegistry,
    out: Vec<Outbound>,
) {
    for (target, msg) in out {
        if target == client_id {
            if let Err(e) = conn.send(&msg) {
                info!("ipc: invio al client fallito: {e}");
                return;
            }
        } else if !focus.send_to_client(target, msg) {
            debug!("ipc: client {target:?} non raggiungibile, messaggio scartato");
        }
    }
}

/// Sveglia il loop principale (ignora l'errore se il server si sta
/// spegnendo: il ricevitore è già sparito).
fn wake_main(wake: &Sender<()>) {
    let _ = wake.send(());
}

/// Gestisce un client: handshake, registrazione nel focus, poi smistamento
/// dei messaggi al compositor.
fn handle_client(
    mut conn: Connection,
    output_size: (u32, u32),
    focus: Arc<FocusRegistry>,
    compositor: Arc<Mutex<Compositor>>,
    wake: Sender<()>,
) {
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
                    if !dispatch(&mut conn, client_id, &focus, &compositor, &wake, msg, fds) {
                        break 'client;
                    }
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

    // Disconnessione: distrugge le finestre del client. I messaggi di
    // focus/configure per gli altri client li produce il compositor.
    let mut out = Vec::new();
    {
        let mut comp = compositor.lock().expect("compositor avvelenato");
        let mut owned = comp.windows_of(client_id);
        owned.reverse();
        for id in owned {
            out.extend(comp.destroy_window(id));
        }
    }
    focus.unregister(client_id);
    send_outbounds(&mut conn, client_id, &focus, out);
    wake_main(&wake);
}

/// Smista un messaggio del client al compositor. Restituisce `false` se il
/// client si è disconnesso (il chiamante deve chiudere il thread).
#[allow(clippy::too_many_arguments)]
fn dispatch(
    conn: &mut Connection,
    client_id: ClientId,
    focus: &FocusRegistry,
    compositor: &Arc<Mutex<Compositor>>,
    wake: &Sender<()>,
    msg: ClientMessage,
    fds: Vec<OwnedFd>,
) -> bool {
    match msg {
        ClientMessage::Hello { .. } => {
            warn!("ipc: secondo Hello dallo stesso client, ignorato");
        }
        ClientMessage::Pong => {
            debug!("ipc: Pong ricevuto");
        }
        ClientMessage::CreateWindow {
            width,
            height,
            title,
        } => {
            let (id, out) = {
                let mut comp = compositor.lock().expect("compositor avvelenato");
                comp.create_window(client_id, width, height, title)
            };
            info!("ipc: finestra creata {id:?} per {client_id:?}");
            send_outbounds(conn, client_id, focus, out);
            wake_main(wake);
        }
        ClientMessage::DestroyWindow { window_id } => {
            let out = {
                let mut comp = compositor.lock().expect("compositor avvelenato");
                match comp.owner_of(window_id) {
                    None => {
                        send_error(conn, format!("finestra sconosciuta: {window_id:?}"));
                        return true;
                    }
                    Some(owner) if owner != client_id => {
                        send_error(conn, format!("la finestra {window_id:?} non è tua"));
                        return true;
                    }
                    _ => comp.destroy_window(window_id),
                }
            };
            info!("ipc: finestra distrutta {window_id:?}");
            send_outbounds(conn, client_id, focus, out);
            wake_main(wake);
        }
        ClientMessage::CommitBuffer {
            window_id,
            buffer,
            damage: _damage,
        } => {
            // Il damage è ignorato in Fase 4: il buffer viene ricaricato
            // per intero a ogni commit (il damage vero arriva dopo).
            match take_commit_buffer(&buffer, fds) {
                Ok(render_buffer) => {
                    let changed = {
                        let mut comp = compositor.lock().expect("compositor avvelenato");
                        match comp.commit_buffer(
                            window_id,
                            client_id,
                            render_buffer,
                            buffer.width,
                            buffer.height,
                        ) {
                            Ok(changed) => changed,
                            Err(e) => {
                                send_error(conn, compositor_error_message(e));
                                return true;
                            }
                        }
                    };
                    if changed {
                        wake_main(wake);
                    }
                }
                Err(message) => {
                    send_error(conn, message);
                }
            }
        }
        ClientMessage::SetTitle { window_id, title } => {
            let changed = {
                let mut comp = compositor.lock().expect("compositor avvelenato");
                match comp.set_title(window_id, client_id, title) {
                    Ok(changed) => changed,
                    Err(e) => {
                        send_error(conn, compositor_error_message(e));
                        return true;
                    }
                }
            };
            if changed {
                wake_main(wake);
            }
        }
        ClientMessage::RequestResize {
            window_id,
            width,
            height,
        } => {
            let out = {
                let mut comp = compositor.lock().expect("compositor avvelenato");
                match comp.request_resize(window_id, client_id, width, height) {
                    Ok(out) => out,
                    Err(e) => {
                        send_error(conn, compositor_error_message(e));
                        return true;
                    }
                }
            };
            send_outbounds(conn, client_id, focus, out);
            wake_main(wake);
        }
        ClientMessage::SetFloating {
            window_id,
            floating,
        } => {
            let out = {
                let mut comp = compositor.lock().expect("compositor avvelenato");
                match comp.set_floating(window_id, client_id, floating) {
                    Ok(out) => out,
                    Err(e) => {
                        send_error(conn, compositor_error_message(e));
                        return true;
                    }
                }
            };
            send_outbounds(conn, client_id, focus, out);
            wake_main(wake);
        }
    }
    true
}

/// Invia un `ServerMessage::Error` al client (best-effort).
fn send_error(conn: &mut Connection, message: String) {
    warn!("ipc: errore al client: {message}");
    let _ = conn.send(&ServerMessage::Error { message });
}

/// Messaggio leggibile per un errore del compositor.
fn compositor_error_message(e: CompositorError) -> String {
    match e {
        CompositorError::UnknownWindow(id) => format!("finestra sconosciuta: {id:?}"),
        CompositorError::NotOwner(id) => format!("la finestra {id:?} non è tua"),
        CompositorError::TiledWindow(id) => {
            format!("la finestra {id:?} è in tiling: la dimensione la decide il layout")
        }
    }
}

/// Converte un `CommitBuffer` (fd + metadati) in un buffer di rendering:
/// avvolge l'fd in uno `ShmBuffer`, lo mappa, ricopia le righe in un
/// buffer compatto (stride `width * 4`) e lo carica come
/// `MemoryRenderBuffer` XRGB8888.
fn take_commit_buffer(
    buffer: &zahidex_protocol::BufferInfo,
    fds: Vec<OwnedFd>,
) -> Result<MemoryRenderBuffer, String> {
    if buffer.format != PixelFormat::Xrgb8888 {
        return Err(format!(
            "formato non supportato: {:?} (solo Xrgb8888 in v0.1)",
            buffer.format
        ));
    }
    if fds.len() != 1 {
        return Err(format!(
            "CommitBuffer vuole esattamente 1 fd, ricevuti {}",
            fds.len()
        ));
    }
    let mut fds = fds;
    let fd = fds.remove(0);
    let shm = ShmBuffer::from_fd(fd, buffer.width, buffer.height, buffer.stride)
        .map_err(|e| format!("buffer condiviso non valido: {e}"))?;
    let map = shm
        .map()
        .map_err(|e| format!("mmap del buffer fallita: {e}"))?;
    // Ricopia riga per riga: lo stride del memfd può essere più largo di
    // `width * 4`, mentre `MemoryBuffer::from_slice` vuole righe compatte.
    let row_bytes = (buffer.width * 4) as usize;
    let mut packed = Vec::with_capacity(row_bytes * buffer.height as usize);
    for row in 0..buffer.height {
        let start = (row * buffer.stride) as usize;
        packed.extend_from_slice(&map[start..start + row_bytes]);
    }
    Ok(MemoryRenderBuffer::from_slice(
        &packed,
        Fourcc::Xrgb8888,
        (buffer.width as i32, buffer.height as i32),
        1,
        Transform::Normal,
        None,
    ))
}
