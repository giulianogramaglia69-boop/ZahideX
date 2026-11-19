// SPDX-License-Identifier: GPL-3.0-or-later
//! Handshake IPC: logica pura, testabile senza socket.
//!
//! Il primo messaggio di ogni client deve essere `Hello` con le versioni
//! supportate. Il server risponde `HelloOk` (con la versione negoziata e la
//! dimensione dell'output) oppure `HelloError`.

use zahidex_protocol::{ClientMessage, ServerMessage, negotiate_version};

/// Gestisce il primo messaggio di un client e produce la risposta.
pub fn handle_hello(msg: &ClientMessage, output_width: u32, output_height: u32) -> ServerMessage {
    match msg {
        ClientMessage::Hello { versions, .. } => match negotiate_version(versions) {
            Ok(version) => ServerMessage::HelloOk {
                version,
                output_width,
                output_height,
            },
            Err(e) => ServerMessage::HelloError {
                reason: e.to_string(),
            },
        },
        _ => ServerMessage::Error {
            message: "il primo messaggio deve essere Hello".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zahidex_protocol::WindowId;

    #[test]
    fn hello_v1_ok_with_output_size() {
        let reply = handle_hello(
            &ClientMessage::Hello {
                versions: vec![1],
                client_name: "test".to_string(),
            },
            1920,
            1080,
        );
        assert_eq!(
            reply,
            ServerMessage::HelloOk {
                version: 1,
                output_width: 1920,
                output_height: 1080,
            }
        );
    }

    #[test]
    fn hello_incompatible_versions_errors() {
        let reply = handle_hello(
            &ClientMessage::Hello {
                versions: vec![2, 3],
                client_name: "test".to_string(),
            },
            1920,
            1080,
        );
        assert!(matches!(reply, ServerMessage::HelloError { .. }));
    }

    #[test]
    fn non_hello_first_message_errors() {
        let reply = handle_hello(
            &ClientMessage::CreateWindow {
                width: 100,
                height: 100,
                title: "x".to_string(),
            },
            1920,
            1080,
        );
        assert!(matches!(reply, ServerMessage::Error { .. }));
        // WindowId non serve qui, ma il test compila solo se il tipo esiste.
        let _ = WindowId(0);
    }
}
