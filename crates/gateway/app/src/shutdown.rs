//! The process-shutdown signal every part of the gateway watches.
//!
//! It is shared state: the serve loop selects on it beside the
//! caller-owned shutdown future, every open-ended response stream ends
//! when it fires so the graceful drain has nothing left to wait for, the
//! tray's status tick peeks at it, and `POST /shutdown` is only one of the
//! things that fires it.

use std::{
    io::Read,
    process::ExitCode,
    sync::{Arc, OnceLock},
};

use tokio_util::sync::CancellationToken;

/// The exit status a gateway uses to ask its supervisor for a restart.
/// Private to the supervisor and its child; distinct from 0 and 1 and
/// below the 128+N range shells use for signal deaths.
pub const RESTART_REQUESTED: u8 = 75;

/// Why the gateway is shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    /// Stop serving and exit.
    Stop,
    /// Stop serving and exit, requesting a restart from the supervisor.
    Restart,
}

impl ShutdownReason {
    /// Exit code associated with each exit state.
    ///
    /// A `Stop` is [ExitCode::SUCCESS]
    /// A `Restart` is [RESTART_REQUESTED]
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Stop => ExitCode::SUCCESS,
            Self::Restart => ExitCode::from(RESTART_REQUESTED),
        }
    }
}

/// The process-shutdown signal shared by the `POST /shutdown` route, the
/// serve loop (which selects on it alongside the caller-owned shutdown
/// future), and every open-ended response stream, which ends when it fires
/// so the graceful drain has nothing left to wait for.
///
/// Every clone shares the one underlying signal. It is a cancellation
/// token, not a notify: a `fire` wakes every waiter at once and stays
/// fired, so a stream that subscribes after the signal ends immediately.
#[derive(Debug, Default)]
pub(crate) struct ShutdownSignal {
    token: CancellationToken,
    reason: OnceLock<ShutdownReason>,
}

impl ShutdownSignal {
    /// Fires the signal, starting the serve loop's graceful shutdown.
    /// If fire is called more then once, only the first call's `reason` is kept,
    /// so the serve loop's own re-fire never turns a restart into a stop or vice versa.
    pub(crate) fn fire(&self, reason: ShutdownReason) {
        let _ = self.reason.set(reason);
        self.token.cancel();
    }

    /// Why the signal fired, or `None` if it has not.
    pub(crate) fn reason(&self) -> Option<ShutdownReason> {
        self.reason.get().copied()
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

/// Starts the supervised gateway's watchdog.
///
/// The watchdog covers the case where the supervisor dies without stopping its
/// child, for example when it's killed. The watchdog is a thread that blocks
/// reading stdin and fires ShutdownReason::Stop when it reads end-of-file or
/// reading errors out. The gateway process then shuts down gracefully,
/// including its llama-server processes.
///
/// The supervisor holds the write end of the supervised Gateway processes
/// stdin pipe. When the supervisor process ends for any reason, the OS closes
/// that end, and the child's next read from stdin returns end-of-file.
pub(crate) fn stdin_watchdog(shutdown_signal: Arc<ShutdownSignal>) {
    let watchdog_thread = std::thread::Builder::new()
        .name("gateway-stdin-watch".to_string())
        .spawn(move || {

            // Reads `input` until it ends, then fires `shutdown_signal` with a stop. A
            // supervised gateway's stdin is a pipe its supervisor holds open, so the
            // end of input means the supervisor is gone and the gateway must not
            // outlive it. A read error ends the watch the same way: the gateway can
            // no longer tell whether its supervisor is alive.
            // 
            // From https://doc.rust-lang.org/std/io/trait.Read.html#errors:
            // An error of the `ErrorKind::Interrupted` kind is non-fatal and the
            // read operation should be retried if there is nothing else to do
            let mut buffer = [0_u8; 256];
            let mut stdin = std::io::stdin();

            loop {
                match stdin.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        tracing::warn!("watchdog reading stdin failed: {error}; treating the supervisor as gone");
                        break;
                    }
                }
            }

            tracing::info!("the supervisor closed stdin; shutting down");
            shutdown_signal.fire(ShutdownReason::Stop);
        });

    if let Err(e) = watchdog_thread {
        tracing::error!(
            "failed to spawn the stdin watch thread: {e}; the gateway will not stop if its supervisor dies"
        );
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
        signal.fire(ShutdownReason::Stop);
        assert!(signal.is_fired(), "fire sets the peek before any wait");
    }

    #[test]
    fn an_unfired_signal_has_no_reason() {
        let signal = ShutdownSignal::default();

        assert_eq!(signal.reason(), None);
    }

    #[test]
    fn the_first_reason_survives_a_later_fire() {
        let signal = ShutdownSignal::default();

        signal.fire(ShutdownReason::Restart);
        signal.fire(ShutdownReason::Stop);

        assert!(signal.is_fired());
        assert_eq!(signal.reason(), Some(ShutdownReason::Restart));
    }
}
