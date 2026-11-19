// SPDX-License-Identifier: GPL-3.0-or-later
//! Scelta della modalità video: logica pura, testabile senza hardware.
//!
//! Una modalità è descritta da risoluzione, refresh e flag "preferita"
//! (quella che il monitor dichiara via EDID). La regola di scelta è:
//! preferita > risoluzione (area) > refresh.

use drm::control::{Mode, ModeTypeFlags};

/// Modalità video in forma minimale e confrontabile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMode {
    /// Larghezza in pixel.
    pub width: u32,
    /// Altezza in pixel.
    pub height: u32,
    /// Refresh in millihertz (es. 60000 = 60 Hz).
    pub refresh_millihz: u32,
    /// Il monitor la dichiara come preferita (EDID).
    pub preferred: bool,
}

impl VideoMode {
    /// Converte una modalità DRM in [`VideoMode`].
    pub fn from_drm(mode: &Mode) -> Self {
        let (width, height) = mode.size();
        Self {
            width: width as u32,
            height: height as u32,
            refresh_millihz: mode.vrefresh(),
            preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
        }
    }

    /// Area in pixel (per confrontare le risoluzioni).
    pub fn area(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

/// Sceglie la modalità migliore: prima la preferita, poi la risoluzione più
/// grande, poi il refresh più alto. `None` se la lista è vuota.
pub fn pick_mode(modes: &[VideoMode]) -> Option<VideoMode> {
    modes
        .iter()
        .copied()
        .max_by_key(|m| (m.preferred, m.area(), m.refresh_millihz))
}

/// Come [`pick_mode`], ma restituisce l'indice nella lista di modalità DRM
/// originale (così il chiamante ritrova la `Mode` da usare per il modeset).
pub fn pick_mode_index(modes: &[Mode]) -> Option<usize> {
    let video_modes: Vec<VideoMode> = modes.iter().map(VideoMode::from_drm).collect();
    let picked = pick_mode(&video_modes)?;
    video_modes.iter().position(|m| *m == picked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(width: u32, height: u32, refresh_millihz: u32, preferred: bool) -> VideoMode {
        VideoMode {
            width,
            height,
            refresh_millihz,
            preferred,
        }
    }

    #[test]
    fn empty_list_returns_none() {
        assert_eq!(pick_mode(&[]), None);
    }

    #[test]
    fn preferred_wins_over_bigger_resolution() {
        let modes = [
            mode(3840, 2160, 30000, false),
            mode(1920, 1080, 60000, true),
        ];
        assert_eq!(pick_mode(&modes), Some(mode(1920, 1080, 60000, true)));
    }

    #[test]
    fn biggest_resolution_wins_without_preferred() {
        let modes = [
            mode(1280, 720, 60000, false),
            mode(1920, 1080, 60000, false),
            mode(1600, 900, 60000, false),
        ];
        assert_eq!(pick_mode(&modes), Some(mode(1920, 1080, 60000, false)));
    }

    #[test]
    fn highest_refresh_wins_on_equal_resolution() {
        let modes = [
            mode(1920, 1080, 60000, false),
            mode(1920, 1080, 144000, false),
            mode(1920, 1080, 120000, false),
        ];
        assert_eq!(pick_mode(&modes), Some(mode(1920, 1080, 144000, false)));
    }

    #[test]
    fn area_beats_refresh() {
        // 2560x1440 a 60 Hz batte 1920x1080 a 144 Hz: la risoluzione pesa
        // più del refresh.
        let modes = [
            mode(1920, 1080, 144000, false),
            mode(2560, 1440, 60000, false),
        ];
        assert_eq!(pick_mode(&modes), Some(mode(2560, 1440, 60000, false)));
    }
}
