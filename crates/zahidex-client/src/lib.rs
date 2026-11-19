// SPDX-License-Identifier: GPL-3.0-or-later
//! Libreria client ZahideX: connettersi al server, aprire finestre,
//! disegnare in buffer condivisi (memfd) e ricevere eventi di input.
//!
//! Il client è **single-thread**: `Client` e `Window` non sono `Send`
//! (condividono la connessione via `Rc<RefCell<...>>`). Per usare il
//! client da un altro thread, apri una connessione dedicata con
//! [`Client::connect`].
//!
//! # Esempio minimo
//!
//! ```no_run
//! use zahidex_client::{Client, Event};
//!
//! let mut client = Client::connect().unwrap();
//! let window = client
//!     .create_window("Ciao", 800, 600, false)
//!     .unwrap();
//! // Riempie il contenuto di rosso (XRGB8888: byte B, G, R, X).
//! window
//!     .commit_with(|pixels, width, height, _stride| {
//!         for px in pixels.chunks_exact_mut(4) {
//!             px[0] = 0;
//!             px[1] = 0;
//!             px[2] = 255;
//!             px[3] = 255;
//!         }
//!         let _ = (width, height);
//!     })
//!     .unwrap();
//! loop {
//!     for event in client.poll_events(None).unwrap() {
//!         if let Event::CloseWindow { .. } = event {
//!             return;
//!         }
//!     }
//! }
//! ```

mod client;
mod error;
mod event;

pub use client::{Client, Window};
pub use error::ClientError;
pub use event::Event;
