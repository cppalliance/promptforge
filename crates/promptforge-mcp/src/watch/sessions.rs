//! The sessions a reload announces itself to, and the seam it announces through.
//!
//! A reload that changed what `tools/list` reports sends
//! `notifications/tools/list_changed` to every session currently connected.
//! Most clients ignore it - a tool list is cached for the life of the client
//! process - but a client that honors it is strictly better off, and sending it
//! costs one notification per session per save.
//!
//! [`ListChanged`] is why the reload logic needs no client to be tested: the
//! announcement is a trait with one method, so a test drives a recorder and the
//! server drives [`Sessions`].

use std::fmt;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rmcp::service::{Peer, RoleServer};

/// Where a reload announces that the published tool set changed.
///
/// The method is synchronous and must not block: it is called from the
/// watcher's own task, immediately after the catalog swap, and a peer that has
/// stopped reading its stream must cost itself the notification rather than
/// costing the next reload its window.
pub trait ListChanged: fmt::Debug + Send + Sync {
    /// Announces `notifications/tools/list_changed` to whoever is listening.
    ///
    /// # Panics
    /// An implementation may require a Tokio runtime, since sending a
    /// notification is an `await` and this method is not one; [`Sessions`] does,
    /// and panics when called outside one.
    fn list_changed(&self);
}

/// Every MCP session currently connected to this server.
///
/// A session registers itself once the client has initialized, and is dropped
/// from the list on the first announcement after its transport closed, so a
/// long-lived process does not accumulate dead peers.
pub struct Sessions {
    /// The peers, one per initialized session.
    peers: Mutex<Vec<Peer<RoleServer>>>,
}

impl Sessions {
    /// An empty registry, ready for the first session.
    #[must_use]
    pub fn new() -> Sessions {
        Sessions {
            peers: Mutex::new(Vec::new()),
        }
    }

    /// Adds a session, first forgetting any whose transport has closed.
    pub fn register(&self, peer: Peer<RoleServer>) {
        let mut peers = self.peers();
        peers.retain(|peer| !peer.is_transport_closed());
        peers.push(peer);
    }

    /// How many sessions are currently registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.peers().len()
    }

    /// Whether no session is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The peers, recovering a poisoned lock.
    ///
    /// Poisoning means a panic while the list was held. The list is plain data
    /// no panic can leave half-updated, so refusing every later announcement
    /// over it would cost more than it buys.
    fn peers(&self) -> MutexGuard<'_, Vec<Peer<RoleServer>>> {
        self.peers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for Sessions {
    fn default() -> Sessions {
        Sessions::new()
    }
}

impl fmt::Debug for Sessions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sessions")
            .field("sessions", &self.len())
            .finish()
    }
}

impl ListChanged for Sessions {
    /// # Panics
    /// Panics if called outside a Tokio runtime: the notifications go out on a
    /// task, because sending one is an `await` and this method is not.
    fn list_changed(&self) {
        // The live peers are cloned out under the lock and notified on a task of
        // their own: sending a notification is an `await`, and the guard must
        // not be held across one.
        let live = {
            let mut peers = self.peers();
            peers.retain(|peer| !peer.is_transport_closed());
            peers.clone()
        };
        if live.is_empty() {
            return;
        }
        tracing::debug!("announcing tools/list_changed to {} session(s)", live.len());
        // Detached deliberately: a notification nobody read is not worth
        // holding the next reload for.
        let _announcement = tokio::spawn(async move {
            for peer in live {
                if let Err(error) = peer.notify_tool_list_changed().await {
                    tracing::debug!("session did not take tools/list_changed: {error}");
                }
            }
        });
    }
}
