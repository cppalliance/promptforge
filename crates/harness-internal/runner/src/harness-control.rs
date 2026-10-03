//! The control a Host keeps for one run: a stop that drops the round in
//! flight, and a cancel that ends the run. Both are plain flags any thread
//! can set; the run's effect loop is the one waiter.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use futures_util::task::AtomicWaker;
use promptforge::cancel::CancelHandle;

/// Steers one run from outside its future: [`stop_round`](Self::stop_round)
/// drops the work in flight and leaves the run going, and
/// [`cancel`](Self::cancel) ends the run.
///
/// Cheap to clone; every clone steers the same run. Take one from
/// [`Harness::control`](crate::Harness::control) before calling
/// [`Harness::run`](crate::Harness::run), which consumes the Harness.
#[derive(Clone)]
pub struct RunControl {
    cancel: CancelHandle,
    stop: Arc<StopSignal>,
}

impl RunControl {
    /// The control over a run whose Engine cancel flag is `cancel`.
    pub(crate) fn new(cancel: CancelHandle) -> Self {
        Self {
            cancel,
            stop: Arc::default(),
        }
    }

    /// Drops every effect in flight except questions to the operator, and
    /// answers each `Dropped`. The run's cancel flag stays clear, so the
    /// run goes on: a `pcall` around the dropped call catches the
    /// cancelled error, and an uncaught one ends the run cancelled. A stop
    /// raised while nothing is in flight drops what is in flight when the
    /// run next waits.
    pub fn stop_round(&self) {
        self.stop.raise();
    }

    /// Cancels the run: every effect in flight, questions to the operator
    /// included, is answered `Dropped`, and the run ends cancelled. A
    /// cancel before the run begins ends it before it reaches the
    /// recorder. Idempotent.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// The run's Engine cancel flag.
    pub(crate) fn cancel_handle(&self) -> &CancelHandle {
        &self.cancel
    }

    /// The run's stop signal.
    pub(crate) fn stop_signal(&self) -> &Arc<StopSignal> {
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

/// A stop raised from any thread and taken by the run's effect loop.
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

    /// Takes a raised stop, lowering it, or registers `cx` to be woken by
    /// the next one. The flag is checked again after the registration, so
    /// a stop raised between the two is never lost.
    pub(crate) fn poll_take(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.raised.swap(false, Ordering::SeqCst) {
            return Poll::Ready(());
        }
        self.waker.register(cx.waker());
        if self.raised.swap(false, Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}
