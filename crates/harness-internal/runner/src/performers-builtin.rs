//! The performers the runner supplies itself: the timer and the store.
//! Each is machinery the runner already holds - tokio's timer wheel and
//! the Engine's store operation over the effect's own access - so
//! neither needs a crate of its own. The chat and tool performers reach
//! outward (a gateway and the activated capabilities) and live with what
//! they reach.

use std::time::Duration;

use promptforge::vfs::Access;
use promptforge::vfs::{StoreOp, StoreOutcome, VfsError, perform_store_op};

use super::{BoxFuture, StorePerformer, TimerPerformer};

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

/// Performs a store operation through the Engine's store facade over the
/// store view the effect carries.
///
/// Synchronous: the loop runs it on the blocking pool. When the access
/// drops never affects correctness: claims follow happens-before within
/// the run's scope, and the run ends that scope at `Done`.
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsStore;

impl StorePerformer for VfsStore {
    fn perform(&self, access: &Access, op: StoreOp) -> Result<StoreOutcome, VfsError> {
        perform_store_op(access, op)
    }
}
