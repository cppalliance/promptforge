//! The sessions layer's timer: the Host clock every run of a session
//! sleeps on.

use std::time::Duration;

use harness_runner::performers::{BoxFuture, Timer};

/// Sleeps on tokio's timer wheel.
///
/// The wheel multiplexes every pending sleep, so a `Timer` effect is one
/// `tokio::time::sleep`, and the Harness's drop of the effect tears the
/// sleep down.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TokioTimer;

impl Timer for TokioTimer {
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        // The protocol bounds `seconds` to a non-negative, finite value
        // within `Duration`'s range before the effect is issued; anything
        // outside that fires at once rather than never, as the Engine's
        // own tokio test driver does.
        let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO);
        Box::pin(tokio::time::sleep(duration))
    }
}
