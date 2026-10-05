//! Stopping runs from any thread: one parent cancel ends every run whose
//! context holds one of its children, and a handle's `Cancelled` future
//! wakes when another thread cancels.

use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use promptforge::cancel::CancelHandle;
use promptforge::effect::EffectAnswer;
use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

/// Writes a note to the store and reads it back.
const NOTE: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "return store.read('note.md')\n",
    "```\n",
);

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
fn one_parent_cancel_ends_every_run_holding_one_of_its_children() -> Result<(), Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(NOTE, "greeter");
    let prompt = Arc::new(parsed?);

    let parent = CancelHandle::new();
    let mut runs = Vec::new();
    for name in ["greeter-1", "greeter-2"] {
        let ctx = RunContext::new(name, 7, Timestamp::UNIX_EPOCH).cancel(parent.child());
        runs.push(Run::new(Arc::clone(&prompt), "", ctx));
    }

    let mut held = Vec::new();
    for run in &mut runs {
        let Step::Pending { effects, .. } = run.step() else {
            return Err("the greeter waits on the store before it can finish".into());
        };
        held.push(
            effects
                .into_iter()
                .map(|(id, _provenance, _effect)| id)
                .collect::<Vec<_>>(),
        );
    }

    parent.cancel();

    let mut results = Vec::new();
    for (mut run, mut ids) in runs.into_iter().zip(held) {
        let result = loop {
            match run.step() {
                Step::Pending { effects, .. } => {
                    ids.extend(effects.into_iter().map(|(id, _provenance, _effect)| id));
                    for id in ids.drain(..) {
                        run.resume(id, EffectAnswer::Dropped);
                    }
                }
                Step::Done { result, .. } => break result,
            }
        };
        results.push(result);
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
fn a_cancelled_future_wakes_when_another_thread_cancels_the_run() -> Result<(), Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(NOTE, "greeter");
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let run = Run::new(Arc::new(parsed?), "", ctx);
    let handle = run.cancel_handle();
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
