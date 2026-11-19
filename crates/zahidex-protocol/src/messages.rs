// SPDX-License-Identifier: GPL-3.0-or-later
//! Messaggi del protocollo ZahideX v0.1.
//!
//! Regola di versionamento: i messaggi sono enum chiusi. Le nuove varianti e
//! i nuovi campi si aggiungono solo in coda; mai rimuovere o rinumerare.

use serde::{Deserialize, Serialize};

use crate::types::{BufferInfo, Rect, WindowId};

/// Messaggi dal client al server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Primo messaggio della connessione: versioni supportate e nome del client.
    Hello {
        versions: Vec<u16>,
        client_name: String,
    },

    /// Chiede una nuova finestra. Il server risponde `WindowCreated`.
    CreateWindow {
        width: u32,
        height: u32,
        title: String,
    },

    /// Distrugge una finestra esistente.
    DestroyWindow { window_id: WindowId },

    /// Presenta un nuovo contenuto per la finestra. L'fd del buffer memfd
    /// viaggia fuori banda via SCM_RIGHTS (esattamente un fd per messaggio).
    CommitBuffer {
        window_id: WindowId,
        buffer: BufferInfo,
        damage: Rect,
    },

    /// Cambia il titolo di una finestra.
    SetTitle { window_id: WindowId, title: String },

    /// Il client chiede di essere ridimensionato (il server risponde con
    /// `Configure` o con `Error`).
    RequestResize {
        window_id: WindowId,
        width: u32,
        height: u32,
    },

    /// Il client chiede la modalità floating (`true`) o tiling (`false`).
    SetFloating { window_id: WindowId, floating: bool },

    /// Risposta a `ServerMessage::Ping`.
    Pong,
}

/// Messaggi dal server al client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    /// Handshake riuscito: versione scelta e dimensione dell'output.
    HelloOk {
        version: u16,
        output_width: u32,
        output_height: u32,
    },

    /// Handshake fallito (es. nessuna versione in comune).
    HelloError { reason: String },

    /// Conferma di `CreateWindow`: id assegnato e dimensioni effettive.
    WindowCreated {
        window_id: WindowId,
        width: u32,
        height: u32,
    },

    /// Il server impone (o conferma) nuove dimensioni alla finestra.
    Configure {
        window_id: WindowId,
        width: u32,
        height: u32,
    },

    /// Cambio di focus. `None` = nessuna finestra focalizzata.
    Focus { window_id: Option<WindowId> },

    /// Il puntatore è entrato nella finestra.
    PointerEnter { window_id: WindowId },

    /// Il puntatore è uscito dalla finestra.
    PointerLeave { window_id: WindowId },

    /// Movimento del puntatore, in coordinate della finestra.
    PointerMotion { window_id: WindowId, x: i32, y: i32 },

    /// Pulsante del puntatore premuto o rilasciato.
    PointerButton {
        window_id: WindowId,
        button: u32,
        pressed: bool,
    },

    /// Tasto tastiera: keycode evdev grezzo e stato. In v0.1 non c'è ancora
    /// la traduzione in keysym (arriva con xkbcommon in Fase 3).
    Key {
        window_id: WindowId,
        keycode: u32,
        pressed: bool,
    },

    /// Il server chiede al client di chiudere la finestra.
    CloseWindow { window_id: WindowId },

    /// Keep-alive: il client deve rispondere `ClientMessage::Pong`.
    Ping,

    /// Errore generico (es. id finestra sconosciuto).
    Error { message: String },
}
