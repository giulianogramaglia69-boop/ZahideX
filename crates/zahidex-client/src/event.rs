// SPDX-License-Identifier: GPL-3.0-or-later
//! Eventi che il server invia al client dopo l'handshake.
//!
//! `Ping` non arriva mai qui: la libreria risponde `Pong` da sola.
//! `WindowCreated` e `HelloOk`/`HelloError` non arrivano mai qui: li
//! consumano `Client::connect` e `Client::create_window`.

use zahidex_protocol::{Axis, WindowId};

/// Un evento del server per il client.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Il server impone (o conferma) nuove dimensioni alla finestra.
    /// La geometria della `Window` è già aggiornata quando l'evento arriva.
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

    /// Movimento del puntatore, in coordinate della finestra
    /// (origine in alto a sinistra della finestra, barra del titolo inclusa).
    PointerMotion { window_id: WindowId, x: i32, y: i32 },

    /// Pulsante del puntatore premuto o rilasciato.
    PointerButton {
        window_id: WindowId,
        button: u32,
        pressed: bool,
    },

    /// Scorrimento su un asse (rotella o trackpad). Esattamente uno dei due
    /// delta è valorizzato (vedi `zahidex_protocol::ServerMessage::PointerAxis`).
    PointerAxis {
        window_id: WindowId,
        axis: Axis,
        delta_v120: Option<i32>,
        delta_px: Option<f64>,
    },

    /// Tasto tastiera: keycode evdev grezzo, stato, keysym XKB e maschera
    /// dei modificatori (`zahidex_protocol::modifiers`).
    Key {
        window_id: WindowId,
        keycode: u32,
        pressed: bool,
        keysym: u32,
        modifiers: u32,
    },

    /// Il server chiede al client di chiudere la finestra.
    CloseWindow { window_id: WindowId },

    /// Errore generico dal server (es. id finestra sconosciuto).
    ServerError { message: String },
}
