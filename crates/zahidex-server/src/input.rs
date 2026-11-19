// SPDX-License-Identifier: GPL-3.0-or-later
//! Input libinput + xkbcommon.
//!
//! - `SessionInterface`: apre i device `/dev/input/event*` tramite la
//!   sessione libseat (permessi logind), come richiede `input::Libinput`.
//! - `create_libinput`: costruisce il backend libinput di Smithay per un
//!   seat e lo assegna via udev (rilevamento e hotplug dei dispositivi:
//!   gli eventi `DeviceAdded`/`DeviceRemoved` di libinput sono il
//!   meccanismo di hotplug, non serve un secondo watcher udev).
//! - `Zahidex::handle_input_event`: traduce gli eventi libinput in stato
//!   interno (tastiera, cursore) e messaggi protocollo verso il client
//!   focalizzato.

use std::os::unix::io::OwnedFd;
use std::path::Path;

use input::{Libinput, LibinputInterface};
use rustix::fs::OFlags;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, ButtonState, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::backend::libinput::LibinputInputBackend;
use smithay::backend::session::{AsErrno, Session, libseat::LibSeatSession};
use tracing::{debug, info};
use zahidex_protocol::{ServerMessage, WindowId, modifiers};

use crate::error::ServerError;
use crate::keymap::TranslatedKey;
use crate::server::Zahidex;

/// `WindowId` sentinella per gli eventi di input a livello output
/// (Fase 3: non ci sono ancora finestre; le finestre sono Fase 4).
pub const OUTPUT_WINDOW: WindowId = WindowId(0);

/// Codice evdev di `BTN_LEFT` (usato per il tocco: tocco = click sinistro).
const BTN_LEFT: u32 = 0x110;

/// Interfaccia libinput che apre i device tramite la sessione libseat.
pub struct SessionInterface {
    session: LibSeatSession,
}

impl SessionInterface {
    pub fn new(session: LibSeatSession) -> Self {
        Self { session }
    }
}

impl LibinputInterface for SessionInterface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let oflags = OFlags::from_bits_retain(flags as u32);
        self.session
            .open(path, oflags)
            .map_err(|e| e.as_errno().unwrap_or(libc::EIO))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

/// Costruisce il backend libinput di Smithay: contesto udev sul seat dato,
/// con i device aperti via sessione libseat.
pub fn create_libinput(
    session: LibSeatSession,
    seat: &str,
) -> Result<LibinputInputBackend, ServerError> {
    let mut context = Libinput::new_with_udev(SessionInterface::new(session));
    context.udev_assign_seat(seat).map_err(|()| {
        ServerError::Input(format!("libinput: impossibile assegnare il seat {seat}"))
    })?;
    Ok(LibinputInputBackend::new(context))
}

/// Elenca i dispositivi di input visti da libinput (best-effort, per il
/// `--dry-run`): nomi e capacità. Non fallisce mai: in caso di errore
/// restituisce una lista vuota.
pub fn list_input_devices(session: LibSeatSession, seat: &str) -> Vec<String> {
    use input::event::{DeviceEvent, Event, EventTrait};

    let mut out = Vec::new();
    let mut context = Libinput::new_with_udev(SessionInterface::new(session));
    if context.udev_assign_seat(seat).is_err() || context.dispatch().is_err() {
        return out;
    }
    for event in context.by_ref() {
        if let Event::Device(DeviceEvent::Added(added)) = event {
            let device = added.device();
            let mut caps = Vec::new();
            if device.has_capability(input::DeviceCapability::Keyboard) {
                caps.push("tastiera");
            }
            if device.has_capability(input::DeviceCapability::Pointer) {
                caps.push("pointer");
            }
            if device.has_capability(input::DeviceCapability::Touch) {
                caps.push("touch");
            }
            if device.has_capability(input::DeviceCapability::TabletTool) {
                caps.push("tablet");
            }
            out.push(format!("{} [{}]", device.name(), caps.join(", ")));
        }
    }
    out
}

/// Converte un asse libinput nell'asse del protocollo ZahideX.
///
/// (Funzione libera e non `impl From`: entrambe le enum sono definite in
/// altri crate, la regola degli orfani vieta un impl `From` esterno.)
pub fn to_protocol_axis(axis: Axis) -> zahidex_protocol::Axis {
    match axis {
        Axis::Vertical => zahidex_protocol::Axis::Vertical,
        Axis::Horizontal => zahidex_protocol::Axis::Horizontal,
    }
}

/// `true` se il tasto tradotto è una scorciatoia di uscita:
/// Ctrl+Alt+BackSpace oppure Super+Q.
fn is_exit_shortcut(t: &TranslatedKey) -> bool {
    let ctrl_alt = modifiers::CTRL | modifiers::ALT;
    (t.keysym == 0xff08 && t.modifiers & ctrl_alt == ctrl_alt)
        || ((t.keysym == 0x71 || t.keysym == 0x51)
            && t.modifiers & modifiers::SUPER == modifiers::SUPER)
}

impl Zahidex {
    /// Invia un messaggio al client focalizzato. Se non c'è nessun client
    /// l'evento va perso (normale finché nessun client è connesso).
    fn send_to_focused(&self, msg: ServerMessage) {
        if !self.focus.send_to_focused(msg) {
            debug!("input: nessun client focalizzato, evento scartato");
        }
    }

    /// Gestisce un evento libinput: aggiorna tastiera/cursore e inoltra
    /// al client focalizzato. Il cursore mosso richiede un ridisegno.
    pub fn handle_input_event(&mut self, event: InputEvent<LibinputInputBackend>) {
        match event {
            InputEvent::DeviceAdded { device } => {
                info!(
                    "input: dispositivo aggiunto: {} ({})",
                    device.name(),
                    device.sysname()
                );
            }
            InputEvent::DeviceRemoved { device } => {
                info!(
                    "input: dispositivo rimosso: {} ({})",
                    device.name(),
                    device.sysname()
                );
            }
            InputEvent::Keyboard { event } => {
                let xkb_code = event.key_code().raw();
                let pressed = event.state() == KeyState::Pressed;
                // Il protocollo vuole il keycode evdev grezzo; Smithay
                // fornisce già il keycode XKB (evdev + 8).
                let evdev = xkb_code.saturating_sub(8);
                let translated = self.keymap.translate(xkb_code, pressed);
                debug!(
                    "tastiera: evdev {evdev} keysym {:#x} ({}) mods {:#x}",
                    translated.keysym, translated.keysym_name, translated.modifiers
                );
                if pressed && is_exit_shortcut(&translated) {
                    info!("scorciatoia di uscita, spengo il server");
                    self.exit_requested = true;
                    return;
                }
                self.send_to_focused(ServerMessage::Key {
                    window_id: OUTPUT_WINDOW,
                    keycode: evdev,
                    pressed,
                    keysym: translated.keysym,
                    modifiers: translated.modifiers,
                });
            }
            InputEvent::PointerMotion { event } => {
                self.cursor.move_relative(event.delta_x(), event.delta_y());
                self.needs_redraw = true;
                let (x, y) = self.cursor.position();
                self.send_to_focused(ServerMessage::PointerMotion {
                    window_id: OUTPUT_WINDOW,
                    x,
                    y,
                });
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let x = event.x_transformed(self.output_size.0 as i32);
                let y = event.y_transformed(self.output_size.1 as i32);
                self.cursor.move_absolute(x, y);
                self.needs_redraw = true;
                let (x, y) = self.cursor.position();
                self.send_to_focused(ServerMessage::PointerMotion {
                    window_id: OUTPUT_WINDOW,
                    x,
                    y,
                });
            }
            InputEvent::PointerButton { event } => {
                let pressed = event.state() == ButtonState::Pressed;
                let code = event.button_code();
                debug!(
                    "pointer: pulsante {code:#x} {}",
                    if pressed { "premuto" } else { "rilasciato" }
                );
                self.send_to_focused(ServerMessage::PointerButton {
                    window_id: OUTPUT_WINDOW,
                    button: code,
                    pressed,
                });
            }
            InputEvent::PointerAxis { event } => {
                for axis in [Axis::Vertical, Axis::Horizontal] {
                    let delta_v120 = event.amount_v120(axis).map(|v| v as i32);
                    let delta_px = event.amount(axis);
                    if delta_v120.is_some() || delta_px.is_some() {
                        debug!("pointer: scroll {axis:?} v120={delta_v120:?} px={delta_px:?}");
                        self.send_to_focused(ServerMessage::PointerAxis {
                            window_id: OUTPUT_WINDOW,
                            axis: to_protocol_axis(axis),
                            delta_v120,
                            delta_px,
                        });
                    }
                }
            }
            InputEvent::TouchDown { event } => {
                // Il tocco è posizionamento assoluto + click sinistro.
                let x = event.x_transformed(self.output_size.0 as i32);
                let y = event.y_transformed(self.output_size.1 as i32);
                self.cursor.move_absolute(x, y);
                self.needs_redraw = true;
                let (x, y) = self.cursor.position();
                self.send_to_focused(ServerMessage::PointerMotion {
                    window_id: OUTPUT_WINDOW,
                    x,
                    y,
                });
                self.send_to_focused(ServerMessage::PointerButton {
                    window_id: OUTPUT_WINDOW,
                    button: BTN_LEFT,
                    pressed: true,
                });
            }
            InputEvent::TouchMotion { event } => {
                let x = event.x_transformed(self.output_size.0 as i32);
                let y = event.y_transformed(self.output_size.1 as i32);
                self.cursor.move_absolute(x, y);
                self.needs_redraw = true;
                let (x, y) = self.cursor.position();
                self.send_to_focused(ServerMessage::PointerMotion {
                    window_id: OUTPUT_WINDOW,
                    x,
                    y,
                });
            }
            InputEvent::TouchUp { .. } | InputEvent::TouchCancel { .. } => {
                self.send_to_focused(ServerMessage::PointerButton {
                    window_id: OUTPUT_WINDOW,
                    button: BTN_LEFT,
                    pressed: false,
                });
            }
            _ => {
                debug!("input: evento non gestito (gesto/tablet/switch)");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::modifiers_mask;

    #[test]
    fn exit_shortcut_ctrl_alt_backspace() {
        let t = TranslatedKey {
            keysym: 0xff08,
            keysym_name: "BackSpace".to_string(),
            modifiers: modifiers_mask(false, true, true, false),
        };
        assert!(is_exit_shortcut(&t));
        // Senza Alt non è una scorciatoia.
        let t = TranslatedKey {
            keysym: 0xff08,
            keysym_name: "BackSpace".to_string(),
            modifiers: modifiers_mask(false, true, false, false),
        };
        assert!(!is_exit_shortcut(&t));
    }

    #[test]
    fn exit_shortcut_super_q() {
        for keysym in [0x71, 0x51] {
            let t = TranslatedKey {
                keysym,
                keysym_name: "q".to_string(),
                modifiers: modifiers_mask(false, false, false, true),
            };
            assert!(is_exit_shortcut(&t), "keysym {keysym:#x}");
        }
        // La 'q' senza Super non esce.
        let t = TranslatedKey {
            keysym: 0x71,
            keysym_name: "q".to_string(),
            modifiers: 0,
        };
        assert!(!is_exit_shortcut(&t));
        // Super+A non esce.
        let t = TranslatedKey {
            keysym: 0x61,
            keysym_name: "a".to_string(),
            modifiers: modifiers_mask(false, false, false, true),
        };
        assert!(!is_exit_shortcut(&t));
    }

    #[test]
    fn axis_conversion() {
        assert_eq!(
            to_protocol_axis(Axis::Vertical),
            zahidex_protocol::Axis::Vertical
        );
        assert_eq!(
            to_protocol_axis(Axis::Horizontal),
            zahidex_protocol::Axis::Horizontal
        );
    }
}
