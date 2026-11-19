// SPDX-License-Identifier: GPL-3.0-or-later
//! Trasporto IPC di ZahideX: Unix socket, framing del protocollo,
//! passaggio di fd (SCM_RIGHTS) e buffer condivisi memfd.

mod client;
mod connection;
mod error;
mod server;
mod shm;

pub use client::{connect, connect_default};
pub use connection::Connection;
pub use error::IpcError;
pub use server::{SOCKET_NAME, Server, default_socket_path, socket_path_for};
pub use shm::{BYTES_PER_PIXEL, ShmBuffer};
