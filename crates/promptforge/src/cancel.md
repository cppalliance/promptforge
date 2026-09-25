The cancel flag that stops a run, shared by cloning, arranged into parent and child handles, and set from any thread.

A run is a pure state machine, so it never awaits a cancel. Instead it checks a synchronous flag between chain steps and from the Lua instruction hook, and your program sets that flag from whichever thread it likes. [`CancelHandle`] is that flag. With it you can stop a run from another thread, even a run stuck in a Lua loop that never yields, and you can wire one cancel to reach the run and every capability working for it. By the end of this page you can cancel a run from anywhere, give a run your own flag, build trees of handles, wait on a cancel as a future, and tell a cancelled run apart from a failed one.

# Where this fits

Every [`RunContext`](crate::RunContext) holds a flag. [`RunContext::new`](crate::RunContext::new) mints a fresh one, and the [`RunContext::cancel`](crate::RunContext::cancel) builder swaps in a handle that the host keeps. [`RunContext::cancel_handle`](crate::RunContext::cancel_handle) returns the context's flag so the host can hand it to its activated capabilities. [`Run::new`](crate::Run::new) keeps that same flag. Once the run exists, [`Run::cancel`](crate::Run::cancel) sets it, and [`Run::cancel_handle`](crate::Run::cancel_handle) returns a clone for another thread.

After a cancel, the next [`Run::step`](crate::Run::step) tears every chain down. The host answers each outstanding [`Effect`](crate::effect::Effect) with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped), steps again, and gets [`Step::Done`](crate::Step::Done) with [`RunResult::Cancelled`](crate::RunResult::Cancelled). [The host loop](crate#the-host-loop) on the crate page walks through that shutdown with a worked example.

# Cancelling from another thread

This program runs a section that loops forever. The main thread steps the run, which blocks inside the Lua loop. A second thread cancels it through the run's handle.

````
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

let source = concat!(
    "---\n",
    "name: spin\n",
    "description: loops until cancelled\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Spin\n",
    "\n",
    "## Loop\n",
    "\n",
    "```lua\n",
    "local n = 0\n",
    "while true do n = n + 1 end\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "spin");
let ctx = RunContext::new("spin", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(Arc::new(parsed?), "", ctx);

let handle = run.cancel_handle();
let canceller = thread::spawn(move || {
    thread::sleep(Duration::from_millis(50));
    handle.cancel();
});

let step = run.step();
canceller.join().map_err(|_| "the cancelling thread panicked")?;
assert!(matches!(step, Step::Done { result: RunResult::Cancelled, .. }));
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Take the handle.** [`Run::cancel_handle`](crate::Run::cancel_handle) returns a clone of the run's flag. [`CancelHandle`] is [`Send`], [`Sync`], and `'static`, so the clone moves into the other thread while the run stays with the thread that steps it. Calling [`CancelHandle::cancel`] on the clone has exactly the same effect as calling [`Run::cancel`](crate::Run::cancel) on the run, because both set one flag.
2. **Step.** The section's Lua loop is legal. The Lua instruction hook's trip budget is effectively unlimited, so the run's cancel flag is the only thing that aborts such a loop. Every block coroutine gets the same hook, and once the flag is set the hook fails the running chunk with "lua execution cancelled".
3. **Read the result.** This run had no effects outstanding, so the step that observes the cancel is already [`Step::Done`](crate::Step::Done) with [`RunResult::Cancelled`](crate::RunResult::Cancelled).

Setting the flag is a request, not a synchronous stop. Running Lua aborts at its next hook firing, and then the shutdown in [Where this fits](#where-this-fits) follows. [`Step::Done`](crate::Step::Done) arrives only after every outstanding effect has been answered.

A run holds its flag from the moment [`Run::new`](crate::Run::new) returns, even a run that cannot start. On a run whose first step will report a startup failure, [`Run::cancel`](crate::Run::cancel) and [`Run::cancel_handle`](crate::Run::cancel_handle) still work before that first step.

# Giving a run your own flag

Cloning a [`CancelHandle`] gives another handle over the same flag, and a cancel through any clone is seen by every clone. That is how one flag is shared across threads and components.

A host that wants to own the flag builds one with [`CancelHandle::new`] and passes it to the [`RunContext::cancel`](crate::RunContext::cancel) builder, which replaces the flag that [`RunContext::new`](crate::RunContext::new) minted. Whether or not the host does this, [`RunContext::cancel_handle`](crate::RunContext::cancel_handle) returns the context's flag. It is named `cancel_handle` because the builder method already has the name [`RunContext::cancel`](crate::RunContext::cancel). Hand the flag to your activated capabilities, and one cancel reaches the run and everything working for it.

````
use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::timestamp::Timestamp;
use promptforge::{Prompt, Run, RunContext};

let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: says hi\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Greeter\n",
    "\n",
    "## Say hi\n",
    "\n",
    "Say hello.\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greeter");

let host = CancelHandle::new();
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).cancel(host.clone());
let for_capabilities = ctx.cancel_handle();
let mut run = Run::new(Arc::new(parsed?), "", ctx);
assert!(!host.is_cancelled());

run.cancel();
assert!(host.is_cancelled());
assert!(for_capabilities.is_cancelled());
# Ok::<(), Box<dyn std::error::Error>>(())
````

The host's clone, the capabilities' handle, and the run all hold one flag, so [`Run::cancel`](crate::Run::cancel) is visible through each of them. The same works in the other direction: calling [`CancelHandle::cancel`] on `host` stops the run.

**Bridging an async token.** A host whose async runtime cancels through an awaitable token bridges it to the run by setting this synchronous flag when the token fires. The capabilities that hold the same flag stop too.

# Trees of handles

[`CancelHandle::child`] mints a fresh handle with its own flag. The child reports cancelled when its own flag is set or when any ancestor's flag is set. Cancelling a parent cancels every descendant, and cancelling one child leaves its parent and its siblings running.

The engine never builds a tree itself. Within a run it installs the one run flag on every section's Lua state and never calls [`CancelHandle::child`]. There are no per-task child handles inside a run. A script that cancels one of its own tasks with `task_cancel` goes through the engine's task table, not through a handle. So [`CancelHandle::child`] is a host-side tool, for arranging runs and capabilities into trees of your own.

This host keeps one root and gives each of two runs a child:

````
use promptforge::cancel::CancelHandle;

let host = CancelHandle::new();
let first_run = host.child();
let second_run = host.child();

first_run.cancel();
assert!(first_run.is_cancelled());
assert!(!host.is_cancelled() && !second_run.is_cancelled());

host.cancel();
assert!(second_run.is_cancelled());
assert!(host.child().child().is_cancelled());
````

Pass a child to [`RunContext::cancel`](crate::RunContext::cancel), and the run becomes one node in the tree. Cancelling the parent from another thread then stops the run, including a Lua loop that never yields. That is the program from [Cancelling from another thread](#cancelling-from-another-thread) with two changes: the context gets a child of a host-held parent through [`RunContext::cancel`](crate::RunContext::cancel), and the other thread calls [`CancelHandle::cancel`] on the parent.

Trees nest to any depth. A child minted from a parent that is already cancelled starts out cancelled, which is why the grandchild on the example's last line reports cancelled. A child holds its parent and never the reverse, so a tree has no reference cycles and nothing needs to unregister when a handle drops. Cloning a child shares the child's flag, not the parent's.

# Waiting for a cancel

Checking [`CancelHandle::is_cancelled`] is enough for a host that polls. A host that waits calls [`CancelHandle::cancelled`], which returns a [`Cancelled`] future that completes with `()` once the handle reports cancelled. It works without an async runtime or a timer. The cancel that sets the flag wakes it, so it never spins.

````
use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use promptforge::cancel::CancelHandle;

let host = CancelHandle::new();
let mut waiting = pin!(host.child().cancelled());
let mut cx = Context::from_waker(Waker::noop());
assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Pending);

host.cancel();
assert_eq!(waiting.as_mut().poll(&mut cx), Poll::Ready(()));

let mut late = pin!(host.cancelled());
assert_eq!(late.as_mut().poll(&mut cx), Poll::Ready(()));
````

A waiter on a child is woken by a cancel anywhere up its ancestor chain, exactly once. A descendant's cancel never wakes a waiter on its parent. A future drawn from a handle that is already cancelled is ready at its first poll, as the last two lines show.

**Selecting beside effect answers.** A tokio host can drive a run from one task that waits on either the next effect answer from its workers or the flag's [`CancelHandle::cancelled`] future. When the flag fires, the task calls [`Run::cancel`](crate::Run::cancel), so the next [`Run::step`](crate::Run::step) observes it at once. Both arms are event-driven, so a fully suspended run costs no wakeups while it waits.

# Cancelled versus failed runs

A run cancelled through its flag ends with [`RunResult::Cancelled`](crate::RunResult::Cancelled). The flag is not the only way to get there. Answering an effect with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) resumes the waiting chain with a cancelled error, and if nothing handles that error, the run also ends with [`RunResult::Cancelled`](crate::RunResult::Cancelled), without any call to [`Run::cancel`](crate::Run::cancel).

A failure that the host's cancel caused carries [`RunErrorKind::Cancelled`](crate::RunErrorKind::Cancelled), and [`RunError::is_cancelled`](crate::RunError::is_cancelled) returns `true` for it. The [`Run`](crate::Run) interface reports cancellation as [`RunResult::Cancelled`](crate::RunResult::Cancelled), so a host driving a [`Run`](crate::Run) normally sees that variant, and treats it as a clean stop rather than a failure.

# Reference

## CancelHandle

[`CancelHandle`] is a cloneable, thread-safe cancel flag that can have a parent. The engine checks it before each chain step and from the Lua instruction hook, and the host sets it from any thread to stop a run or one branch of a tree of handles.

The host gets one in four ways: [`CancelHandle::new`] or [`CancelHandle::default`] for a root, [`CancelHandle::child`] for a descendant, [`Clone`] for another handle over the same flag, or a run's flag from [`Run::cancel_handle`](crate::Run::cancel_handle) or [`RunContext::cancel_handle`](crate::RunContext::cancel_handle). It is [`Send`], [`Sync`], [`Unpin`], and `'static`.

- [`CancelHandle::new`] takes no arguments and returns a root handle with no parent, not cancelled to start. It stays independent of every other handle until it is cloned or given children. [`RunContext::new`](crate::RunContext::new) mints its own flag this way. The result is `#[must_use]`. [`CancelHandle::default`] returns the same thing.
- [`CancelHandle::child`] takes `&self`, the parent, which may be any handle, cancelled or not. It returns a fresh handle with its own flag that reports cancelled when its own flag or any ancestor's flag is set. A child of a cancelled parent is cancelled from the start. Cancelling the child never affects the parent or siblings. The result is `#[must_use]`. It cannot fail.
- [`CancelHandle::cancel`] takes `&self`, so it works through a shared reference from any thread, and returns nothing. Afterwards this handle, every clone, and every descendant report cancelled. The parent and siblings are untouched. It is idempotent and irreversible: calls after the first do nothing, and the flag never clears. A host that needs a fresh flag builds a new handle. The call also wakes every [`Cancelled`] future waiting on this handle or on a descendant. It cannot fail.
- [`CancelHandle::cancelled`] takes `&self`, the handle to wait on, and returns a [`Cancelled`] future over a clone of that handle. The future completes at once if the handle already reports cancelled, and otherwise when a cancel lands on the handle or on any ancestor. Await it, pin and poll it, or select over it beside other event sources. It cannot fail.
- [`CancelHandle::is_cancelled`] takes `&self` and returns a [`bool`]: `true` if [`CancelHandle::cancel`] has been called on this handle, any clone, or any ancestor, and `false` otherwise. It is monotonic, so once it returns `true` it never returns `false` again. It walks the ancestor chain with one atomic load per level, so its cost grows with nesting depth. The result is `#[must_use]`. It cannot fail.

[`CancelHandle`] implements [`Debug`](std::fmt::Debug) as `CancelHandle { cancelled: <bool>, depth: <usize> }`, where `depth` is the number of ancestors, `0` for a root. Use it to inspect a handle's state and nesting while debugging.

````
use promptforge::cancel::CancelHandle;

let root = CancelHandle::new();
assert_eq!(format!("{root:?}"), "CancelHandle { cancelled: false, depth: 0 }");
assert_eq!(format!("{:?}", root.child()), "CancelHandle { cancelled: false, depth: 1 }");

root.cancel();
root.cancel();
assert!(root.is_cancelled());
assert_eq!(format!("{root:?}"), "CancelHandle { cancelled: true, depth: 0 }");
````

## Cancelled

[`Cancelled`] is the future that [`CancelHandle::cancelled`] returns. It lets a host wait on a cancel instead of checking the flag on a timer. Hosts only receive it from [`CancelHandle::cancelled`]. It has no public constructor, fields, or methods.

It implements [`Future`](std::future::Future) with an output of `()`. A poll returns [`Poll::Ready`](std::task::Poll::Ready) once the handle, a clone, or an ancestor is cancelled, and [`Poll::Pending`](std::task::Poll::Pending) otherwise. Each poll registers the task's [`Waker`](std::task::Waker) on every handle up the ancestor chain before it reads the flag, so a cancel that lands between the two is not lost. The cancel itself wakes the task, and the future never times out or spins.

It owns a clone of its handle, so it can be held across awaits. It is [`Send`], [`Sync`], and [`Unpin`]. It is `#[must_use]`, because a future does nothing unless polled.
