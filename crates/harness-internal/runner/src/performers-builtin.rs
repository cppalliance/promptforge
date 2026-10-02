//! The performer the runner supplies itself: the timer. It is machinery
//! the runner already holds - tokio's timer wheel - so it needs no crate
//! of its own. The chat and tool performers reach outward (a gateway and
//! the activated capabilities) and live with what they reach.

use std::time::Duration;

use super::{BoxFuture, TimerPerformer};

/// Sleeps on tokio's timer wheel.
///
/// The wheel multiplexes every pending sleep, so the timer machinery is
/// tokio's; a `Timer` effect is one `tokio::time::sleep`, and the loop's
/// abort of the performer task tears the sleep down.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioTimer;

impl TimerPerformer for TokioTimer {
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        // The protocol bounds `seconds` to a non-negative, finite value
        // within `Duration`'s range before the effect is issued; anything
        // outside that fires at once rather than never, as the Engine's
        // own tokio test driver does.
        let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO);
        Box::pin(tokio::time::sleep(duration))
    }
}
