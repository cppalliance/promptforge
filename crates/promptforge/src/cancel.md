[`CancelHandle`] lets your program stop a [run](crate) from any thread.

You need this when you stop runs on a user's request, on a timeout, or at shutdown.

# Where this fits

You already know how to build a run from a [`RunContext`](crate::RunContext) and step it to its result, from [Run a prompt](crate#run-a-prompt). You give a run its handle through [`RunContext::cancel`](crate::RunContext::cancel), so this page picks up where running a prompt leaves off. It adds one handle per run, a parent that stops them all, and a future your async code can wait on.

# Stop many runs at once

Your program runs several prompts. On shutdown or a user's request, you need to stop all of them with one call, or just one of them.

A handle feels like an `Arc<AtomicBool>`: clones share one flag, and any thread can set it.

Unlike a plain flag, a handle can make children, and a cancel flows down to them but never up. So handles form a tree, and a handle reports cancelled when its own flag or any ancestor's flag is set. That shared flag in a tree is a [`CancelHandle`].

````
use promptforge::cancel::CancelHandle;
# use std::sync::Arc;
# use promptforge::effect::EffectAnswer;
# use promptforge::timestamp::Timestamp;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "return store.read('note.md')\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = Arc::new(parsed?);

// 1. Make one parent handle, and give each run its own child through `RunContext::cancel`.
let parent = CancelHandle::new();
let mut runs = Vec::new();
for name in ["greeter-1", "greeter-2"] {
    let ctx = RunContext::new(name, 7, Timestamp::UNIX_EPOCH).cancel(parent.child());
    runs.push(Run::new(Arc::clone(&prompt), "", ctx));
}

// 2. Step each run once, and hold the store effect it now waits on.
let mut held = Vec::new();
for run in &mut runs {
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the greeter waits on the store before it can finish");
    };
    held.push(effects.into_iter().map(|(id, _provenance, _effect)| id).collect::<Vec<_>>());
}

// 3. Cancel the parent once.
parent.cancel();

// 4. Drop what each run still holds, and step it to `Step::Done`: both runs end cancelled.
# let mut results = Vec::new();
# for (mut run, mut ids) in runs.into_iter().zip(held) {
#     let result = loop {
#         match run.step() {
#             Step::Pending { effects, .. } => {
#                 ids.extend(effects.into_iter().map(|(id, _provenance, _effect)| id));
#                 for id in ids.drain(..) {
#                     run.resume(id, EffectAnswer::Dropped);
#                 }
#             }
#             Step::Done { result, .. } => break result,
#         }
#     };
#     results.push(result);
# }
assert!(results.len() == 2 && results.iter().all(|result| matches!(result, RunResult::Cancelled)));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. [`CancelHandle::new`] makes an uncancelled root. [`CancelHandle::child`] makes a new handle under it, and [`RunContext::cancel`](crate::RunContext::cancel) installs that child on one run's context. Both runs share the greeter, parsed once and cloned with [`Arc::clone`](std::sync::Arc::clone).
2. Each run takes one step and stops at its first request for outside work, a store *effect*, so [`Run::step`](crate::Run::step) returns [`Step::Pending`](crate::Step::Pending). Each pending effect arrives as three parts: the id you pass back to [`Run::resume`](crate::Run::resume), its [provenance](crate::ids), and the effect itself. This example keeps only the id and leaves the effect unanswered, so both runs are still waiting when the cancel lands.
3. The example calls [`CancelHandle::cancel`] on the parent alone, never on a child. That one call makes every child and grandchild report cancelled, so it stops every run below the parent. In your own code, confirm it landed by calling [`Run::cancel_handle`](crate::Run::cancel_handle) on each run: it returns a clone of the flag the run holds, and `is_cancelled` on that clone returns `true`.
4. A run still needs an answer for each effect it holds before it reaches [`Step::Done`](crate::Step::Done). After a cancel, the next `Run::step` resumes no chain. It tears every chain down, and any answer you then give, dropped or real, is discarded. You still owe one answer per held effect, and the run then ends as [`RunResult::Cancelled`](crate::RunResult::Cancelled). The hidden loop calls `Run::step` on each run. While it returns `Step::Pending`, it passes every held id, plus any new one, to `run.resume(id, EffectAnswer::Dropped)`. When `Run::step` returns `Step::Done`, it keeps the result. [Stop a run](crate#stop-a-run) finishes a run the same way. A dropped answer resumes the waiting Lua with the cancelled error, and the greeter does not catch that error. So the final assert would pass even without the cancel. The cancel is proved by the flag check from step 3, which this example does not make.

The tree has one parent and one child per run, and a cancel moves in one direction only.

````text
              ┌────────────────────┐
              │       parent       │   parent.cancel() reaches every child
              └─────────┬──────────┘
             ┌──────────┴──────────┐
             │ child()             │ child()
             v                     v
    ┌─────────────────┐   ┌─────────────────┐
    │  run greeter-1  │   │  run greeter-2  │   a child's cancel stops its own
    └─────────────────┘   └─────────────────┘   run and never travels up

    a cancel travels down the tree (v), never up
````

Calling `cancel` on one child leaves its parent and its siblings uncancelled. That lets you stop one run on a user's request without touching the others.

Cloning a handle shares its flag, so cancelling any clone cancels every clone. Keep a clone for yourself before you hand the handle to a run, and cancel through that clone.

A child made from a parent that is already cancelled reports cancelled at once. So a run started after shutdown began stops right away instead of slipping through.

`cancel` takes `&self` and works from any thread. Calling it twice is harmless, and nothing ever clears the flag. So give each new run a fresh handle, never one a past cancel already set. A timeout thread and a shutdown path can both call it without coordinating.

You might expect `child()` to behave like `clone()` and share the parent's flag. Instead, a child gets a new flag of its own: cancelling it stops only that run, while cancelling the parent still reaches it.

A cancel travels down the tree, never up. Next, [Wait for a cancel](#wait-for-a-cancel) shows how your own async code waits for one.

# Wait for a cancel

Your program's async code waits on models, tools, and timers. It must stop waiting the moment a handle is cancelled.

Awaiting a cancel feels like awaiting a oneshot receiver: the future finishes when the other side fires. Unlike a channel, there is no sender to hold; a cancel on the handle or any ancestor fires it. [`CancelHandle::cancelled`] turns the flag into a future that the cancel itself wakes, so your code never checks the flag on a timer. That future is a [`Cancelled`].

````
# use std::sync::Arc;
# use promptforge::timestamp::Timestamp;
# use promptforge::{Prompt, Run, RunContext};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

// 1. A small std-only executor: the waker unparks the waiting thread.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

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

# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "return store.read('note.md')\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
# let run = Run::new(Arc::new(parsed?), "", ctx);
// 2. Take the greeter run's handle, and draw a `Cancelled` future from it.
let handle = run.cancel_handle();
let waiting = handle.cancelled();

// 3. A second thread cancels the run through a clone of the handle.
let remote = handle.clone();
let canceller = thread::spawn(move || remote.cancel());

// 4. Block on the future until the cancel wakes it, then read the flag.
block_on(waiting);
canceller.join().map_err(|_| "the cancelling thread panicked")?;
assert!(handle.is_cancelled());
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `Unpark` and `block_on` build an executor from std alone. The waker unparks the waiting thread, and `block_on` polls through [`Pin::new`](std::pin::Pin::new) and parks between polls. `Cancelled` uses only std, so any executor can drive it: this one, tokio, another runtime, or none. `block_on` requires `Unpin`, and `Cancelled` is `Unpin`, so you can poll it through `&mut` without pinning it first. That lets you reuse one future across the iterations of a select loop.
2. Every [`RunContext`](crate::RunContext) starts with its own root handle, so a run built without [`RunContext::cancel`](crate::RunContext::cancel) still has one; `RunContext::cancel` replaces it. [`Run::cancel_handle`](crate::Run::cancel_handle) returns the greeter run's handle, and the next line draws a `Cancelled` future from it, with output `()`. The future owns its own clone of the handle, so it does not borrow `handle`.
3. A second thread calls `cancel` on a clone of the handle. `cancel` takes `&self`, so any thread can set the flag. The cancel may land before the first poll or while the executor is parked.
4. `block_on` returns when the cancel itself wakes the waiting thread, with no timer or polling loop. When the cancel already landed, it returns on its first poll without parking. [`CancelHandle::is_cancelled`] then reads `true`.

`is_cancelled` returns whether this handle, a clone, or an ancestor was cancelled. Once it returns `true`, it never returns `false` again. It is the check for code that cannot await.

If the handle is already cancelled, the future completes on its first poll. A cancel that lands while you start waiting is never missed, so you need no extra check before you wait.

Dropping the future before the cancel only stops the wait; it changes no flag. So a select that drops the losing branch is safe.

A run itself never awaits its handle. Instead, it reads the flag at two points. A run is a state machine your program steps with [`Run::step`](crate::Run::step), a plain function with no executor behind it, so it has nothing to await with and reads the flag instead.

So the run checks the flag in two places. The first is inside each call to `Run::step`, before it runs each ready [chain](crate::ids), where a chain is one walk over sibling sections, so a cancel that lands while every chain waits is seen on the next `Run::step`. The second is inside running Lua, through a hook that the Lua VM calls on its own every so many instructions. That is why a cancel stops even Lua that never yields. Calling `cancel` is all a run needs, and async waiting is only for your own code.

You might expect `cancelled()` to wait for the next cancel, the way a notify waits for the next signal. Instead, it completes at once when the handle is already cancelled, so a cancel that came first is never lost.

Await `cancelled()` in your code; the run checks the flag on its own. Next, go back to the [crate page](crate) for the rest of what a run does.

# Reference

## CancelHandle

[`CancelHandle`] stops a run from any thread. Install it with [`RunContext::cancel`](crate::RunContext::cancel), and keep a clone to call `cancel` on. A cancel on a parent stops every run whose handle descends from it, as [Stop many runs at once](#stop-many-runs-at-once) shows. Nothing on it returns an error or panics. When your host runs on tokio with its own cancellation token, your code waits on that token and calls `cancel` on this flag when that token fires.

- [`CancelHandle::new`]: makes an uncancelled root, the same as [`CancelHandle::default`], independent of every other handle until cloned or given children.
- [`CancelHandle::child`]: returns a new node whose own flag starts unset; it reports cancelled when its flag or any ancestor's flag is set.
- [`CancelHandle::cancel`]: marks this handle, every clone, and every descendant cancelled, and wakes every [`Cancelled`] future drawn from it or a descendant.
- [`CancelHandle::cancelled`]: returns a future that owns a clone of the handle, so it does not borrow the handle it came from.
- [`CancelHandle::is_cancelled`]: never turns `true` from a cancel on a child or sibling. Once `true`, it stays `true`, so make a fresh handle for each run.

## Cancelled

[`Cancelled`] is a future, with output `()`, that completes when its handle or any ancestor is cancelled. Await it or select over it beside your other sources instead of polling [`CancelHandle::is_cancelled`] on a timer, as [Wait for a cancel](#wait-for-a-cancel) shows. It completes at once when the cancel already landed, and it never misses one that lands while it is polled. Dropping it changes no flag, so a select can drop it safely. It never times out, and any executor can drive it.
