// SPDX-License-Identifier: GPL-3.0-or-later
use std::ffi::OsStr;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use crate::connection::Connection;
use crate::error::IpcError;

/// Nome del socket ZahideX dentro `$XDG_RUNTIME_DIR` (o `/tmp`).
pub const SOCKET_NAME: &str = "zahidex-0";

/// Percorso del socket per una data `$XDG_RUNTIME_DIR` (o fallback `/tmp`).
pub fn socket_path_for(xdg_runtime_dir: Option<&OsStr>) -> PathBuf {
    match xdg_runtime_dir {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(SOCKET_NAME),
        _ => std::env::temp_dir().join(SOCKET_NAME),
    }
}

/// Percorso predefinito del socket del server.
pub fn default_socket_path() -> PathBuf {
    socket_path_for(std::env::var_os("XDG_RUNTIME_DIR").as_deref())
}

/// Server ZahideX in ascolto su uno Unix socket.
#[derive(Debug)]
pub struct Server {
    listener: UnixListener,
    path: PathBuf,
}

impl Server {
    /// Crea il server sul socket indicato, rimuovendo un socket residuo.
    pub fn bind(path: &Path) -> Result<Self, IpcError> {
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    /// Crea il server sul percorso predefinito.
    pub fn bind_default() -> Result<Self, IpcError> {
        Self::bind(&default_socket_path())
    }

    /// Accetta la prossima connessione (bloccante).
    pub fn accept(&self) -> Result<Connection, IpcError> {
        let (stream, _) = self.listener.accept()?;
        Ok(Connection::from_stream(stream))
    }

    /// Percorso del socket.
    pub fn socket_path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_path_uses_xdg_runtime_dir() {
        assert_eq!(
            socket_path_for(Some(OsStr::new("/run/user/1000"))),
            PathBuf::from("/run/user/1000/zahidex-0")
        );
    }

    #[test]
    fn socket_path_falls_back_to_temp_dir() {
        assert_eq!(
            socket_path_for(None),
            std::env::temp_dir().join(SOCKET_NAME)
        );
        assert_eq!(
            socket_path_for(Some(OsStr::new(""))),
            std::env::temp_dir().join(SOCKET_NAME)
        );
    }
}
