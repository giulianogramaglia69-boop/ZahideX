// SPDX-License-Identifier: GPL-3.0-or-later
//! Traduzione dei tasti con xkbcommon (layout italiano di default).
//!
//! I keycode in arrivo da libinput (via Smithay) sono già keycode XKB
//! (evdev + 8): `xkbcommon` li traduce in keysym tenendo conto dei
//! modificatori premuti. La maschera dei modificatori usa i bit del
//! protocollo (`zahidex_protocol::modifiers`).
//!
//! I keycode evdev sono quelli di `<linux/input-event-codes.h>` (es.
//! `KEY_A = 30`); il keycode XKB corrispondente è `evdev + 8`.

use xkbcommon::xkb::{self, KeyDirection, Keycode};

use crate::error::ServerError;
use zahidex_protocol::modifiers;

/// Layout tastiera predefinito.
pub const DEFAULT_LAYOUT: &str = "it";

/// Risultato della traduzione di un evento tastiera.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatedKey {
    /// Keysym XKB (`0` = nessun simbolo per questo tasto in questo layout).
    pub keysym: u32,
    /// Nome del keysym (es. `"a"`, `"A"`, `"BackSpace"`), `""` se nessuno.
    pub keysym_name: String,
    /// Maschera dei modificatori attivi dopo l'evento.
    pub modifiers: u32,
}

/// Maschera dei modificatori dai quattro flag booleani (logica pura).
#[must_use]
pub fn modifiers_mask(shift: bool, ctrl: bool, alt: bool, super_: bool) -> u32 {
    let mut mask = 0;
    if shift {
        mask |= modifiers::SHIFT;
    }
    if ctrl {
        mask |= modifiers::CTRL;
    }
    if alt {
        mask |= modifiers::ALT;
    }
    if super_ {
        mask |= modifiers::SUPER;
    }
    mask
}

/// Keymap XKB: contesto + keymap compilata + stato.
///
/// `state` è dichiarato prima di `keymap` perché i campi vengono
/// distrutti in ordine di dichiarazione: lo stato (che in C trattiene
/// un riferimento alla keymap) deve morire prima della keymap stessa.
pub struct Keymap {
    state: xkb::State,
    /// La keymap compilata: `state` in C trattiene un riferimento a questa
    /// keymap, quindi il campo va tenuto vivo (e distrutto dopo `state`)
    /// anche se non viene mai letto direttamente.
    #[allow(dead_code)]
    keymap: xkb::Keymap,
}

impl Keymap {
    /// Compila la keymap per il layout indicato (es. `"it"`, `"us"`).
    pub fn new(layout: &str) -> Result<Self, ServerError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| ServerError::Keymap(format!("layout '{layout}' non caricabile")))?;
        let state = xkb::State::new(&keymap);
        Ok(Self { state, keymap })
    }

    /// Traduce un evento tastiera: aggiorna lo stato dei modificatori e
    /// restituisce keysym, nome e maschera modificatori.
    ///
    /// `keycode` è il keycode XKB (evdev + 8), come lo fornisce Smithay.
    pub fn translate(&mut self, keycode: u32, pressed: bool) -> TranslatedKey {
        let direction = if pressed {
            KeyDirection::Down
        } else {
            KeyDirection::Up
        };
        let code = Keycode::new(keycode);
        self.state.update_key(code, direction);
        let sym = self.state.key_get_one_sym(code);
        let keysym: u32 = sym.into();
        let keysym_name = if keysym == 0 {
            String::new()
        } else {
            xkb::keysym_get_name(sym)
        };
        let modifiers = modifiers_mask(
            self.state
                .mod_name_is_active("Shift", xkb::STATE_MODS_EFFECTIVE),
            self.state
                .mod_name_is_active("Control", xkb::STATE_MODS_EFFECTIVE),
            self.state
                .mod_name_is_active("Mod1", xkb::STATE_MODS_EFFECTIVE),
            self.state
                .mod_name_is_active("Mod4", xkb::STATE_MODS_EFFECTIVE),
        );
        TranslatedKey {
            keysym,
            keysym_name,
            modifiers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keycode evdev (linux/input-event-codes.h) -> XKB = evdev + 8.
    const EVDEV_Q: u32 = 16;
    const EVDEV_A: u32 = 30;
    const EVDEV_LEFTSHIFT: u32 = 42;
    const EVDEV_BACKSPACE: u32 = 14;

    #[test]
    fn modifiers_mask_bits() {
        assert_eq!(modifiers_mask(false, false, false, false), 0);
        assert_eq!(modifiers_mask(true, false, false, false), modifiers::SHIFT);
        assert_eq!(modifiers_mask(false, true, false, false), modifiers::CTRL);
        assert_eq!(modifiers_mask(false, false, true, false), modifiers::ALT);
        assert_eq!(modifiers_mask(false, false, false, true), modifiers::SUPER);
        assert_eq!(
            modifiers_mask(true, true, true, true),
            modifiers::SHIFT | modifiers::CTRL | modifiers::ALT | modifiers::SUPER
        );
    }

    #[test]
    fn key_a_is_lowercase_a_in_italian_layout() {
        let mut keymap = Keymap::new("it").unwrap();
        let t = keymap.translate(EVDEV_A + 8, true);
        assert_eq!(t.keysym, 0x61, "keysym di 'a'");
        assert_eq!(t.keysym_name, "a");
        assert_eq!(t.modifiers, 0);
        let _ = keymap.translate(EVDEV_A + 8, false);
    }

    #[test]
    fn shift_a_is_uppercase_a() {
        let mut keymap = Keymap::new("it").unwrap();
        let shift = keymap.translate(EVDEV_LEFTSHIFT + 8, true);
        assert_eq!(shift.modifiers & modifiers::SHIFT, modifiers::SHIFT);
        let t = keymap.translate(EVDEV_A + 8, true);
        assert_eq!(t.keysym, 0x41, "keysym di 'A' con Shift");
        assert_eq!(t.keysym_name, "A");
        assert_eq!(t.modifiers & modifiers::SHIFT, modifiers::SHIFT);
        let _ = keymap.translate(EVDEV_A + 8, false);
        let shift_up = keymap.translate(EVDEV_LEFTSHIFT + 8, false);
        assert_eq!(shift_up.modifiers & modifiers::SHIFT, 0);
    }

    #[test]
    fn backspace_and_q_keysyms() {
        let mut keymap = Keymap::new("it").unwrap();
        let bs = keymap.translate(EVDEV_BACKSPACE + 8, true);
        assert_eq!(bs.keysym, 0xff08);
        assert_eq!(bs.keysym_name, "BackSpace");
        let q = keymap.translate(EVDEV_Q + 8, true);
        assert_eq!(q.keysym, 0x71);
        assert_eq!(q.keysym_name, "q");
    }
}
