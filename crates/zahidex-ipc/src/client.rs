// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

use crate::connection::Connection;
use crate::error::IpcError;
use crate::server::default_socket_path;

/// Connette al server ZahideX sul socket indicato.
pub fn connect(path: &Path) -> Result<Connection, IpcError> {
    Connection::connect(path)
}

/// Connette al server ZahideX sul percorso predefinito.
pub fn connect_default() -> Result<Connection, IpcError> {
    connect(&default_socket_path())
}
