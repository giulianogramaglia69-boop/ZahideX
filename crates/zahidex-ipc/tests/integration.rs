// SPDX-License-Identifier: GPL-3.0-or-later
//! Test di integrazione: server e client reali su Unix socket temporaneo.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use zahidex_ipc::{Connection, Server, ShmBuffer};
use zahidex_protocol::{ClientMessage, Rect, ServerMessage, WindowId, negotiate_version};

fn temp_socket_path(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "zahidex-test-{tag}-{}-{n}.sock",
        std::process::id()
    ))
}

#[test]
fn handshake_window_and_commit_flow() {
    let path = temp_socket_path("flow");
    let server = Server::bind(&path).unwrap();
    let server_thread = thread::spawn(move || -> Result<(), String> {
        let mut conn = server.accept().map_err(|e| e.to_string())?;
        conn.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;

        // 1. Hello + negoziazione versione.
        let (msg, fds): (ClientMessage, _) = conn.recv().map_err(|e| e.to_string())?;
        assert!(fds.is_empty());
        let ClientMessage::Hello {
            versions,
            client_name,
        } = msg
        else {
            return Err("expected Hello".into());
        };
        assert_eq!(client_name, "zahidex-test-client");
        let version = negotiate_version(&versions).map_err(|e| e.to_string())?;
        conn.send(&ServerMessage::HelloOk {
            version,
            output_width: 1920,
            output_height: 1080,
        })
        .map_err(|e| e.to_string())?;

        // 2. CreateWindow.
        let (msg, _): (ClientMessage, _) = conn.recv().map_err(|e| e.to_string())?;
        let ClientMessage::CreateWindow {
            width,
            height,
            title,
        } = msg
        else {
            return Err("expected CreateWindow".into());
        };
        assert_eq!((width, height), (64, 64));
        assert_eq!(title, "finestra di prova");
        let window_id = WindowId(1);
        conn.send(&ServerMessage::WindowCreated {
            window_id,
            width,
            height,
        })
        .map_err(|e| e.to_string())?;

        // 3. CommitBuffer con fd: il server mappa il memfd e verifica i pixel.
        let (msg, fds): (ClientMessage, _) = conn.recv().map_err(|e| e.to_string())?;
        let ClientMessage::CommitBuffer {
            window_id: w,
            buffer,
            damage,
        } = msg
        else {
            return Err("expected CommitBuffer".into());
        };
        assert_eq!(w, window_id);
        assert_eq!(damage, Rect::new(0, 0, 64, 64));
        assert_eq!(fds.len(), 1);
        let shm = ShmBuffer::from_fd(
            fds.into_iter().next().unwrap(),
            buffer.width,
            buffer.height,
            buffer.stride,
        )
        .map_err(|e| e.to_string())?;
        let map = shm.map().map_err(|e| e.to_string())?;
        assert_eq!(map[0], 0x11);
        assert_eq!(map[map.len() - 1], 0x11);

        // 4. Ping/Pong.
        conn.send(&ServerMessage::Ping).map_err(|e| e.to_string())?;
        let (msg, _): (ClientMessage, _) = conn.recv().map_err(|e| e.to_string())?;
        assert_eq!(msg, ClientMessage::Pong);
        Ok(())
    });

    let mut conn = Connection::connect(&path).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    conn.send(&ClientMessage::Hello {
        versions: vec![1],
        client_name: "zahidex-test-client".into(),
    })
    .unwrap();
    let (msg, _): (ServerMessage, _) = conn.recv().unwrap();
    assert_eq!(
        msg,
        ServerMessage::HelloOk {
            version: 1,
            output_width: 1920,
            output_height: 1080
        }
    );
    conn.send(&ClientMessage::CreateWindow {
        width: 64,
        height: 64,
        title: "finestra di prova".into(),
    })
    .unwrap();
    let (msg, _): (ServerMessage, _) = conn.recv().unwrap();
    assert_eq!(
        msg,
        ServerMessage::WindowCreated {
            window_id: WindowId(1),
            width: 64,
            height: 64
        }
    );
    let buffer = ShmBuffer::new(64, 64).unwrap();
    {
        let mut map = buffer.map_mut().unwrap();
        map.fill(0x11);
    }
    conn.send_commit_buffer(WindowId(1), &buffer, Rect::new(0, 0, 64, 64))
        .unwrap();
    let (msg, _): (ServerMessage, _) = conn.recv().unwrap();
    assert_eq!(msg, ServerMessage::Ping);
    conn.send(&ClientMessage::Pong).unwrap();

    server_thread.join().unwrap().unwrap();
}

#[test]
fn version_negotiation_rejected() {
    let path = temp_socket_path("version");
    let server = Server::bind(&path).unwrap();
    let server_thread = thread::spawn(move || -> Result<(), String> {
        let mut conn = server.accept().map_err(|e| e.to_string())?;
        conn.set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let (msg, _): (ClientMessage, _) = conn.recv().map_err(|e| e.to_string())?;
        let ClientMessage::Hello { versions, .. } = msg else {
            return Err("expected Hello".into());
        };
        match negotiate_version(&versions) {
            Ok(_) => Err("negotiation should have failed".into()),
            Err(e) => {
                conn.send(&ServerMessage::HelloError {
                    reason: e.to_string(),
                })
                .map_err(|e| e.to_string())?;
                Ok(())
            }
        }
    });

    let mut conn = Connection::connect(&path).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    conn.send(&ClientMessage::Hello {
        versions: vec![7, 8],
        client_name: "vecchio-client".into(),
    })
    .unwrap();
    let (msg, _): (ServerMessage, _) = conn.recv().unwrap();
    let ServerMessage::HelloError { reason } = msg else {
        panic!("expected HelloError, got {msg:?}");
    };
    assert!(reason.contains("version"));
    server_thread.join().unwrap().unwrap();
}
