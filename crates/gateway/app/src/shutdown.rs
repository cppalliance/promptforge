//! The process-shutdown signal every part of the gateway watches.
//!
//! It is shared state, not a route: the serve loop selects on it beside
//! the caller-owned shutdown future, every open-ended response stream ends
//! when it fires so the graceful drain has nothing left to wait for, the
//! tray's status tick peeks at it, and `POST /shutdown` is only one of the
//! things that fires it.

use tokio_util::sync::CancellationToken;

/// The process-shutdown signal shared by the `POST /shutdown` route, the
/// serve loop (which selects on it alongside the caller-owned shutdown
/// future), and every open-ended response stream, which ends when it fires
/// so the graceful drain has nothing left to wait for.
///
/// Every clone shares the one underlying signal. It is a cancellation
/// token, not a notify: a `fire` wakes every waiter at once and stays
/// fired, so a stream that subscribes after the signal ends immediately.
#[derive(Debug, Clone, Default)]
pub(crate) struct ShutdownSignal {
    token: CancellationToken,
}

impl ShutdownSignal {
    /// Fires the signal, starting the serve loop's graceful shutdown.
    pub(crate) fn fire(&self) {
        self.token.cancel();
    }

    /// Whether the signal has been fired; the tray's status tick reads it
    /// to tell a requested shutdown apart from a serve-loop failure.
    pub(crate) fn is_fired(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Resolves once the signal has fired.
    pub(crate) async fn fired(&self) {
        self.token.cancelled().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tray's status tick reads `is_fired` synchronously to tell a
    /// requested shutdown apart from a serve-loop failure; the method is
    /// gated on the tray backends like its only callers.
    #[test]
    fn fire_sets_the_synchronous_peek() {
        let signal = ShutdownSignal::default();
        assert!(!signal.is_fired(), "a fresh signal reads unfired");
        signal.fire();
        assert!(signal.is_fired(), "fire sets the peek before any wait");
    }
}
