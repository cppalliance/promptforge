//! The session runs' environment: the Host snapshot a client pushes
//! through the public API, which each run carries into its request.
//!
//! Everything here arrives as data. The Harness never resolves a gateway,
//! reads a menu, or names a workspace crate: the client pushes a
//! [`HostSnapshot`] whenever its selection or roots change, and each run
//! of a session, relaunches included, takes the latest one as it starts.

use std::sync::{PoisonError, RwLock};

pub use harness_runner::environment::HostSnapshot;

/// The bindings one Harness holds for every session it serves: the Host
/// snapshot, replaceable by the client.
#[derive(Debug, Default)]
pub struct Bindings {
    host: RwLock<HostSnapshot>,
}

impl Bindings {
    /// Bindings with nothing pushed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the Host snapshot; the next launch reads it.
    pub fn set_host(&self, host: HostSnapshot) {
        *self.host.write().unwrap_or_else(PoisonError::into_inner) = host;
    }

    /// The current Host snapshot.
    #[must_use]
    pub fn host(&self) -> HostSnapshot {
        self.host
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
