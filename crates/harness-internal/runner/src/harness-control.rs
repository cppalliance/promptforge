//! The control a Host keeps for one run: a stop that drops the round in
//! flight, and a cancel that ends the run. Both are plain flags any thread
//! can set; the run's effect loop is the one waiter.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use futures_util::task::AtomicWaker;
use promptforge::cancel::CancelHandle;

/// A handle that drops a run's in-flight round or cancels the run, from
/// outside the future that drives it.
///
/// [`stop_round`](Self::stop_round) drops the work in flight, and the run
/// goes on. [`cancel`](Self::cancel) ends the run.
///
/// Cloning is cheap, and every clone controls the same run. Take one from
/// [`Harness::control`](crate::Harness::control) before you call
/// [`Harness::run`](crate::Harness::run), because `run` consumes the
/// Harness.
#[derive(Clone)]
pub struct RunControl {
    cancel: CancelHandle,
    stop: Arc<StopSignal>,
}

impl RunControl {
    /// The control over a run whose Engine cancel flag is `cancel`.
    pub(super) fn new(cancel: CancelHandle) -> Self {
        Self {
            cancel,
            stop: Arc::default(),
        }
    }

    /// Drops the effects in flight, except questions to the operator, and
    /// lets the run go on.
    ///
    /// A question to the operator is a call to the
    /// `promptforge/user-input/ask` tool. Every other effect in flight is
    /// aborted and answered `Dropped`. The run's cancel flag stays clear,
    /// so the run continues. A `pcall` around a dropped call catches its
    /// cancelled error. An uncaught one ends the run cancelled.
    ///
    /// A stop reaches only the effects in flight when the loop that drives
    /// the run sees it. The loop looks for a stop while it waits for an
    /// answer and before it starts a step's effects. A stop raised while
    /// nothing it can drop is in flight changes nothing. A stop never
    /// reaches an effect that starts after the loop saw it.
    pub fn stop_round(&self) {
        self.stop.raise();
    }

    /// Cancels the run.
    ///
    /// Every effect in flight, questions to the operator included, is
    /// answered `Dropped`, and the run ends cancelled. A cancel raised
    /// before the run begins ends the run before it reaches the recorder.
    /// Calling `cancel` again has no further effect.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The run's Engine cancel flag.
    pub(super) fn cancel_handle(&self) -> &CancelHandle {
        &self.cancel
    }

    /// The run's stop signal.
    pub(super) fn stop_signal(&self) -> &Arc<StopSignal> {
        &self.stop
    }
}

impl fmt::Debug for RunControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunControl")
            .field("cancelled", &self.cancel.is_cancelled())
            .field("stop_raised", &self.stop.raised.load(Ordering::SeqCst))
            .finish()
    }
}

/// A stop raised from any thread and lowered by the run's effect loop once
/// it has dropped what the stop reached.
#[derive(Default)]
pub(crate) struct StopSignal {
    raised: AtomicBool,
    waker: AtomicWaker,
}

impl StopSignal {
    /// Raises the stop and wakes the loop waiting on it.
    fn raise(&self) {
        self.raised.store(true, Ordering::SeqCst);
        self.waker.wake();
    }

    /// Whether a stop is raised.
    pub(crate) fn is_raised(&self) -> bool {
        self.raised.load(Ordering::SeqCst)
    }

    /// Ready while a stop is raised, or registers `cx` to be woken by the
    /// next one. The flag is checked again after the registration, so a
    /// stop raised between the two is never lost.
    pub(crate) fn poll_raised(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.is_raised() {
            return Poll::Ready(());
        }
        self.waker.register(cx.waker());
        if self.is_raised() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Lowers the stop. A stop raised while the loop was dropping what an
    /// earlier one reached is lowered with it, since nothing it could
    /// reach is left in flight.
    pub(crate) fn lower(&self) {
        self.raised.store(false, Ordering::SeqCst);
    }
}
