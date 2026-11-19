// SPDX-License-Identifier: GPL-3.0-or-later
use crate::error::ProtocolError;

/// Versione del protocollo implementata da questa crate.
pub const PROTOCOL_VERSION: u16 = 1;

/// Versioni accettate dal server.
pub const SUPPORTED_VERSIONS: &[u16] = &[PROTOCOL_VERSION];

/// Sceglie la versione più alta offerta dal client e supportata dal server.
///
/// Restituisce [`ProtocolError::UnsupportedVersion`] se non c'è intersezione.
pub fn negotiate_version(offered: &[u16]) -> Result<u16, ProtocolError> {
    offered
        .iter()
        .copied()
        .filter(|v| SUPPORTED_VERSIONS.contains(v))
        .max()
        .ok_or_else(|| ProtocolError::UnsupportedVersion {
            offered: offered.to_vec(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiates_highest_common_version() {
        assert_eq!(negotiate_version(&[1]).unwrap(), 1);
        assert_eq!(negotiate_version(&[2, 1]).unwrap(), 1);
        assert_eq!(negotiate_version(&[1, 2, 3]).unwrap(), 1);
    }

    #[test]
    fn rejects_incompatible_versions() {
        assert!(negotiate_version(&[2, 3]).is_err());
        assert!(negotiate_version(&[]).is_err());
    }
}
