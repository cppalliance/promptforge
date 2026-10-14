//! Stopping runs from any thread: one parent cancel ends every run whose
//! context holds one of its children, and a handle's `Cancelled` future
//! wakes when another thread cancels.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use promptforge::RunResult;
use promptforge::cancel::CancelHandle;
use promptforge::effect::Effect;

use super::support::{Scenario, TestResult, greeter};

/// Writes a note to the store and reads it back.
const NOTE: &str = "
    store.write('note.md', 'hello')
    return store.read('note.md')
";

/// A std-only executor's waker: waking unparks the waiting thread.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread, parking between polls, until it is
/// ready.
fn block_on<F: Future + Unpin>(mut future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = Pin::new(&mut future).poll(&mut cx) {
            return output;
        }
        thread::park();
    }
}

#[test]
fn one_parent_cancel_ends_every_run_holding_one_of_its_children() -> TestResult {
    let source = greeter(NOTE);
    let parent = CancelHandle::new();
    let mut drivers = Vec::new();
    for _ in 0..2 {
        let parent = parent.clone();
        let mut driver = Scenario::default()
            .hold(|effect| matches!(effect, Effect::Vfs { .. }))
            .context(move |ctx| ctx.cancel(parent.child()))
            .start(&source)?;
        driver.pump_until_held()?;
        drivers.push(driver);
    }

    parent.cancel();

    let mut results = Vec::new();
    for driver in drivers {
        results.push(driver.finish()?.result);
    }
    assert!(
        results.len() == 2
            && results
                .iter()
                .all(|result| matches!(result, RunResult::Cancelled))
    );
    Ok(())
}

#[test]
fn a_cancelled_future_wakes_when_another_thread_cancels_the_run() -> TestResult {
    let driver = Scenario::default().start(&greeter(NOTE))?;
    let handle = driver.run.cancel_handle();
    let waiting = handle.cancelled();

    let remote = handle.clone();
    let canceller = thread::spawn(move || remote.cancel());

    block_on(waiting);
    canceller
        .join()
        .map_err(|_| "the cancelling thread panicked")?;
    assert!(handle.is_cancelled());
    Ok(())
}
