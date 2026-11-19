// SPDX-License-Identifier: GPL-3.0-or-later
//! Test della libreria client contro un server finto in-process
//! (socket Unix reale, script di messaggi).

use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use zahidex_client::{Client, ClientError, Event};
use zahidex_ipc::{Connection, Server, ShmBuffer};
use zahidex_protocol::{ClientMessage, ServerMessage, WindowId};

/// Percorso del socket finto per un test (unico per processo + nome).
fn socket_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zahidex-client-test-{}-{name}", std::process::id()))
}

/// Avvia un server finto: accetta una connessione ed esegue `script`.
fn spawn_fake_server<F>(name: &str, script: F) -> (PathBuf, JoinHandle<()>)
where
    F: FnOnce(Connection) + Send + 'static,
{
    let path = socket_path(name);
    let server = Server::bind(&path).expect("bind del server finto");
    let handle = thread::spawn(move || {
        let conn = server.accept().expect("accept del server finto");
        script(conn);
    });
    (path, handle)
}

/// Script: handshake ok, poi chiude. Il client deve connettersi.
fn handshake_ok_script(mut conn: Connection) {
    let (msg, _) = conn.recv::<ClientMessage>().expect("Hello atteso");
    assert!(
        matches!(msg, ClientMessage::Hello { .. }),
        "atteso Hello, avuto {msg:?}"
    );
    conn.send(&ServerMessage::HelloOk {
        version: 1,
        output_width: 1920,
        output_height: 1080,
    })
    .expect("HelloOk");
}

#[test]
fn connect_handshake_ok() {
    let (path, handle) = spawn_fake_server("handshake-ok", handshake_ok_script);
    let client = Client::connect_to(&path, "test").expect("connessione");
    assert_eq!(client.version(), 1);
    assert_eq!(client.output_size(), (1920, 1080));
    handle.join().expect("thread del server finto");
}

#[test]
fn connect_handshake_rejected() {
    let (path, handle) = spawn_fake_server("handshake-rejected", |mut conn| {
        let _ = conn.recv::<ClientMessage>();
        conn.send(&ServerMessage::HelloError {
            reason: "versioni incompatibili".to_string(),
        })
        .expect("HelloError");
    });
    let err = Client::connect_to(&path, "test").expect_err("handshake rifiutato atteso");
    assert!(
        matches!(err, ClientError::HandshakeRejected { .. }),
        "errore atteso HandshakeRejected, avuto {err:?}"
    );
    handle.join().expect("thread del server finto");
}

#[test]
fn connect_no_server_fails() {
    let path = socket_path("no-server");
    let err = Client::connect_to(&path, "test").expect_err("connessione fallita attesa");
    assert!(
        matches!(err, ClientError::Io(_)),
        "errore atteso Io, avuto {err:?}"
    );
}

#[test]
fn create_window_and_commit() {
    let (path, handle) = spawn_fake_server("create-commit", |mut conn| {
        handshake_ok_script_part(&mut conn);
        // CreateWindow atteso.
        let (msg, _) = conn.recv::<ClientMessage>().expect("CreateWindow attesa");
        assert!(
            matches!(msg, ClientMessage::CreateWindow { width: 800, height: 600, ref title } if title == "Test"),
            "CreateWindow attesa, avuta {msg:?}"
        );
        conn.send(&ServerMessage::WindowCreated {
            window_id: WindowId(7),
            width: 800,
            height: 600,
        })
        .expect("WindowCreated");
        // CommitBuffer atteso, con un fd: il primo pixel deve essere
        // (r=10, g=20, b=30) → byte [30, 20, 10, 255].
        let (msg, fds) = conn.recv::<ClientMessage>().expect("CommitBuffer atteso");
        let ClientMessage::CommitBuffer {
            window_id,
            buffer,
            damage: _,
        } = msg
        else {
            panic!("CommitBuffer atteso, avuto {msg:?}");
        };
        assert_eq!(window_id, WindowId(7));
        assert_eq!(buffer.width, 800);
        assert_eq!(buffer.height, 600 - 24); // meno la barra del titolo
        assert_eq!(buffer.stride, 800 * 4);
        assert_eq!(fds.len(), 1, "CommitBuffer vuole esattamente 1 fd");
        let shm = ShmBuffer::from_fd(fds.into_iter().next().unwrap(), 800, 576, 3200)
            .expect("fd del buffer valido");
        let map = shm.map().expect("mmap del buffer");
        assert_eq!(&map[0..4], &[30, 20, 10, 255]);
        // DestroyWindow atteso alla fine.
        let (msg, _) = conn.recv::<ClientMessage>().expect("DestroyWindow attesa");
        assert!(
            matches!(
                msg,
                ClientMessage::DestroyWindow {
                    window_id: WindowId(7)
                }
            ),
            "DestroyWindow attesa, avuta {msg:?}"
        );
    });

    let mut client = Client::connect_to(&path, "test").expect("connessione");
    let window = client
        .create_window("Test", 800, 600, false)
        .expect("creazione finestra");
    assert_eq!(window.id(), WindowId(7));
    assert_eq!(window.width(), 800);
    assert_eq!(window.height(), 600);
    assert_eq!(window.content_height(), 576);
    window
        .commit_with(|pixels, _width, _height, _stride| {
            pixels[0] = 30;
            pixels[1] = 20;
            pixels[2] = 10;
            pixels[3] = 255;
        })
        .expect("commit");
    window.destroy().expect("destroy");
    handle.join().expect("thread del server finto");
}

/// Come `handshake_ok_script`, ma su una connessione già accettata.
fn handshake_ok_script_part(conn: &mut Connection) {
    let (msg, _) = conn.recv::<ClientMessage>().expect("Hello atteso");
    assert!(
        matches!(msg, ClientMessage::Hello { .. }),
        "atteso Hello, avuto {msg:?}"
    );
    conn.send(&ServerMessage::HelloOk {
        version: 1,
        output_width: 1920,
        output_height: 1080,
    })
    .expect("HelloOk");
}

#[test]
fn create_window_floating_sends_set_floating() {
    let (path, handle) = spawn_fake_server("create-floating", |mut conn| {
        handshake_ok_script_part(&mut conn);
        let (msg, _) = conn.recv::<ClientMessage>().expect("CreateWindow attesa");
        assert!(matches!(msg, ClientMessage::CreateWindow { .. }));
        conn.send(&ServerMessage::WindowCreated {
            window_id: WindowId(5),
            width: 800,
            height: 600,
        })
        .expect("WindowCreated");
        // Il client deve chiedere il floating subito dopo l'ack.
        let (msg, _) = conn.recv::<ClientMessage>().expect("SetFloating atteso");
        assert!(
            matches!(
                msg,
                ClientMessage::SetFloating {
                    window_id: WindowId(5),
                    floating: true
                }
            ),
            "SetFloating atteso, avuto {msg:?}"
        );
        // Il server conferma la geometria floating con un Configure.
        conn.send(&ServerMessage::Configure {
            window_id: WindowId(5),
            width: 400,
            height: 300,
        })
        .expect("Configure");
    });

    let mut client = Client::connect_to(&path, "test").expect("connessione");
    let window = client
        .create_window("F", 800, 600, true)
        .expect("creazione finestra floating");
    assert_eq!(window.id(), WindowId(5));
    assert!(window.is_floating());
    // Il Configure aggiorna la geometria della finestra e arriva come evento.
    let events = client
        .poll_events(Some(Duration::from_secs(2)))
        .expect("poll eventi");
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Configure {
                window_id: WindowId(5),
                width: 400,
                height: 300
            }
        )),
        "Configure atteso, avuti {events:?}"
    );
    assert_eq!(window.width(), 400);
    assert_eq!(window.height(), 300);
    handle.join().expect("thread del server finto");
}

#[test]
fn poll_events_returns_key_and_auto_answers_ping() {
    let (path, handle) = spawn_fake_server("ping-key", |mut conn| {
        handshake_ok_script_part(&mut conn);
        conn.send(&ServerMessage::Ping).expect("Ping");
        // Il client deve rispondere Pong da solo, senza che l'app lo chieda.
        conn.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout di lettura");
        let (msg, _) = conn.recv::<ClientMessage>().expect("Pong atteso");
        assert!(
            matches!(msg, ClientMessage::Pong),
            "Pong atteso, avuto {msg:?}"
        );
        conn.send(&ServerMessage::Key {
            window_id: WindowId(0),
            keycode: 30,
            pressed: true,
            keysym: 0x61,
            modifiers: 0,
        })
        .expect("Key");
    });

    let mut client = Client::connect_to(&path, "test").expect("connessione");
    let events = client
        .poll_events(Some(Duration::from_secs(2)))
        .expect("poll eventi");
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::Key {
                window_id: WindowId(0),
                keycode: 30,
                pressed: true,
                keysym: 0x61,
                modifiers: 0
            }
        )),
        "Key atteso, avuti {events:?}"
    );
    handle.join().expect("thread del server finto");
}

#[test]
fn commit_rejects_wrong_pixel_count() {
    let (path, handle) = spawn_fake_server("commit-bad-pixels", |mut conn| {
        handshake_ok_script_part(&mut conn);
        let _ = conn.recv::<ClientMessage>();
        conn.send(&ServerMessage::WindowCreated {
            window_id: WindowId(9),
            width: 64,
            height: 48,
        })
        .expect("WindowCreated");
        // Il client non deve inviare niente: il commit fallisce prima.
        conn.set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout di lettura");
        let result = conn.recv::<ClientMessage>();
        assert!(result.is_err(), "nessun messaggio atteso, avuto {result:?}");
    });

    let mut client = Client::connect_to(&path, "test").expect("connessione");
    let window = client
        .create_window("P", 64, 48, false)
        .expect("creazione finestra");
    // Contenuto atteso: 64 * (48 - 24) * 4 = 6144 byte.
    let err = window
        .commit(&[0u8; 10])
        .expect_err("commit fallito atteso");
    assert!(
        matches!(err, ClientError::InvalidPixels { .. }),
        "errore atteso InvalidPixels, avuto {err:?}"
    );
    drop(client);
    handle.join().expect("thread del server finto");
}
