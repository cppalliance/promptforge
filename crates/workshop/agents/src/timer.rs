//! The Host's timer: the clock every conversation's run sleeps on.

use std::time::Duration;

use harness::{BoxFuture, Timer};

/// Sleeps on tokio's timer wheel.
///
/// The wheel multiplexes every pending sleep, so a `Timer` effect is one
/// `tokio::time::sleep`, and the Harness's drop of the effect tears the
/// sleep down.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioTimer;

impl Timer for TokioTimer {
    fn sleep(&self, duration: Duration) -> BoxFuture<()> {
        Box::pin(tokio::time::sleep(duration))
    }
}
