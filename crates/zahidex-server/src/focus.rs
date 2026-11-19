// SPDX-License-Identifier: GPL-3.0-or-later
//! Focus e registro dei client connessi.
//!
//! Fase 3: senza finestre, il focus è "output attivo" — il primo client
//! connesso riceve gli eventi di input. Quando si disconnette, il focus
//! passa al prossimo client rimasto, altrimenti a nessuno.
//!
//! Il registro è condiviso (`Arc`) tra i thread IPC (che registrano i
//! client e svuotano la coda in uscita verso il socket) e il loop
//! principale (che inoltra gli eventi di input al client focalizzato).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use zahidex_protocol::ServerMessage;

/// Identificatore di una connessione client (assegnato dal server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientId(pub u64);

/// Registro condiviso dei client connessi e del focus.
#[derive(Debug, Default)]
pub struct FocusRegistry {
    inner: Mutex<RegistryInner>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    /// Client registrati, in ordine di connessione. Il primo è focalizzato.
    order: Vec<ClientId>,
    /// Canale di invio verso ciascun client.
    senders: HashMap<ClientId, mpsc::Sender<ServerMessage>>,
    next_id: u64,
}

impl FocusRegistry {
    /// Crea un registro vuoto, già dentro un `Arc` condivisibile.
    #[must_use]
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registra un nuovo client e restituisce il suo id e il canale da cui
    /// leggerà i messaggi in uscita (il thread IPC lo svuota sul socket).
    pub fn register(&self) -> (ClientId, mpsc::Receiver<ServerMessage>) {
        let (tx, rx) = mpsc::channel();
        let mut inner = self.inner.lock().expect("focus registry avvelenato");
        let id = ClientId(inner.next_id);
        inner.next_id += 1;
        inner.order.push(id);
        inner.senders.insert(id, tx);
        (id, rx)
    }

    /// Rimuove un client (disconnessione). Se era quello focalizzato, il
    /// focus passa al prossimo rimasto.
    pub fn unregister(&self, id: ClientId) {
        let mut inner = self.inner.lock().expect("focus registry avvelenato");
        inner.order.retain(|other| *other != id);
        inner.senders.remove(&id);
    }

    /// Il client attualmente focalizzato, se ce n'è uno.
    #[must_use]
    pub fn focused(&self) -> Option<ClientId> {
        self.inner
            .lock()
            .expect("focus registry avvelenato")
            .order
            .first()
            .copied()
    }

    /// Quanti client sono connessi.
    #[must_use]
    pub fn client_count(&self) -> usize {
        self.inner
            .lock()
            .expect("focus registry avvelenato")
            .order
            .len()
    }

    /// Invia un messaggio al client focalizzato. Restituisce `false` se non
    /// c'è nessun client focalizzato o se il canale è chiuso (in quel caso
    /// il client viene rimosso dal registro).
    pub fn send_to_focused(&self, msg: ServerMessage) -> bool {
        let (id, tx) = {
            let inner = self.inner.lock().expect("focus registry avvelenato");
            match inner.order.first().copied() {
                Some(id) => match inner.senders.get(&id) {
                    Some(tx) => (id, tx.clone()),
                    None => return false,
                },
                None => return false,
            }
        };
        match tx.send(msg) {
            Ok(()) => true,
            Err(_) => {
                self.unregister(id);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zahidex_protocol::WindowId;

    #[test]
    fn first_registered_client_is_focused() {
        let registry = FocusRegistry::default();
        assert_eq!(registry.focused(), None);
        let (first, _rx) = registry.register();
        assert_eq!(registry.focused(), Some(first));
        let (second, _rx) = registry.register();
        assert_eq!(registry.focused(), Some(first));
        assert_eq!(registry.client_count(), 2);
        let _ = second;
    }

    #[test]
    fn focus_moves_to_next_client_on_disconnect() {
        let registry = FocusRegistry::default();
        let (first, _rx1) = registry.register();
        let (second, _rx2) = registry.register();
        registry.unregister(first);
        assert_eq!(registry.focused(), Some(second));
        registry.unregister(second);
        assert_eq!(registry.focused(), None);
        assert_eq!(registry.client_count(), 0);
    }

    #[test]
    fn send_to_focused_delivers_and_fails_without_clients() {
        let registry = FocusRegistry::default();
        assert!(!registry.send_to_focused(ServerMessage::Ping));
        let (first, rx) = registry.register();
        assert!(registry.send_to_focused(ServerMessage::Focus { window_id: None }));
        assert_eq!(
            rx.try_recv().unwrap(),
            ServerMessage::Focus { window_id: None }
        );
        let _ = first;
    }

    #[test]
    fn send_to_focused_unregisters_dead_client() {
        let registry = FocusRegistry::default();
        let (first, rx) = registry.register();
        drop(rx);
        assert!(!registry.send_to_focused(ServerMessage::Ping));
        assert_eq!(registry.focused(), None);
        assert_eq!(registry.client_count(), 0);
        let _ = first;
    }

    #[test]
    fn key_message_roundtrips_through_registry() {
        use zahidex_protocol::modifiers;
        let registry = FocusRegistry::default();
        let (_id, rx) = registry.register();
        assert!(registry.send_to_focused(ServerMessage::Key {
            window_id: WindowId(0),
            keycode: 30,
            pressed: true,
            keysym: 0x61,
            modifiers: modifiers::SHIFT,
        }));
        assert_eq!(
            rx.try_recv().unwrap(),
            ServerMessage::Key {
                window_id: WindowId(0),
                keycode: 30,
                pressed: true,
                keysym: 0x61,
                modifiers: modifiers::SHIFT,
            }
        );
    }
}
