Stop async work at its next safe point, instead of dropping it halfway through a step.

You need this when your program handles Ctrl-C, or stops helper tasks on its own schedule, and every session must stay in a clean state.

# Where this fits

This page stops your program's own async work, such as the loop that reads a session's events and the helper tasks beside it. It does not stop the session's run; [`Session::cancel`](crate::Session::cancel) and [`Session::close`](crate::Session::close) do that. The tool is one shared stop flag, called a *cancel handle*. You install it by wrapping a piece of work in [`scope`], and after that any code running inside that work can find the flag without being passed it.

# Stop work on Ctrl-C

`desk` is the Host you built on the main page, meaning your own program that launches agents and relays what they say. It runs the `chat` agent for one person, and its loop reads the session's events. When the operator presses Ctrl-C, dropping the loop's future could stop it halfway through handling an event. You want a flag you set from one task, which the loop checks at points it chooses. That flag is a [`CancelHandle`].

A cancel handle feels like a shared [`Arc<AtomicBool>`](std::sync::atomic::AtomicBool) stop flag that your loop checks. Unlike a bare flag, you can also await it, and code deep inside the work can find it without you passing it down.

````
use harness::cancel::{self, CancelHandle};
use harness::Session;
# use harness::capability::{CapabilityRegistry, HostServices, UserInput};
# use harness::record::MemoryRecorder;
# use harness::{Harness, HarnessConfig, HostSnapshot, LaunchRequest, WaitFrame};
# use std::error::Error;
# use std::sync::Arc;
# fn desk() -> Harness {
#     use harness::{BoxFuture, InferenceBroker, OnDelta};
#     use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
#     struct Offline;
#     impl InferenceBroker for Offline {
#         fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#             Box::pin(async { Ok(ModelCatalog::empty()) })
#         }
#         fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: Option<OnDelta>) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#             let kind = CompletionErrorKind::Unavailable;
#             Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
#         }
#     }
#     let mut capabilities = CapabilityRegistry::new();
#     capabilities.register(Arc::new(UserInput::new())).expect("an empty registry takes user input");
#     let config = HarnessConfig { agents_path: "desk/agents".into() };
#     let harness = Harness::new(config, Arc::new(MemoryRecorder::new()), Arc::new(Offline), capabilities, HostServices::new());
#     harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });
#     harness
# }
# fn chat() -> LaunchRequest {
#     LaunchRequest { agent: "chat".into(), args: String::new(), input_text: None }
# }
# async fn say(session: &Session, text: &str) -> Result<(), Box<dyn Error>> {
#     let mut waits = session.subscribe_waits();
#     session.resend_waits();
#     loop {
#         if let WaitFrame::Required { token } = waits.recv().await? {
#             return Ok(session.send_input(&token, text.into(), || {})?);
#         }
#     }
# }

// 1. desk's loop races the session's events against `wait_cancelled`, its safe point.
async fn desk_loop(session: &Session) -> Result<usize, Box<dyn Error>> {
    let mut events = session.subscribe_events();
    let mut shown = 0;
    loop {
        tokio::select! {
            () = cancel::wait_cancelled() => return Ok(shown),
            event = events.recv() => {
                if event?.event["kind"] == "assistant_reply" {
                    shown += 1;
                }
            }
        }
    }
}

async fn stop_on_ctrl_c() -> Result<(), Box<dyn Error>> {
#     let harness = desk();
#     let session = harness.launch(chat()).await?;
#     say(&session, "Hello, desk.").await?;
    // 2. Make one flag, and keep a clone for the Ctrl-C task.
    let handle = CancelHandle::new();
    let ctrl_c = handle.clone();

    // 3. A task standing in for `tokio::signal::ctrl_c` cancels through its clone.
    let signal = tokio::spawn(async move { ctrl_c.cancel() });

    // 4. Install the flag with `scope` around desk's loop.
    let outcome = cancel::scope(handle.clone(), desk_loop(&session)).await;
    signal.await?;

    // 5. The loop returned on its own, and no flag stays installed after `scope` exits.
    assert!(outcome.is_ok() && handle.is_cancelled());
    assert!(cancel::current().is_none());
    session.close();
    Ok(())
}

// 6. Outside every `scope` no flag is installed, so the free `is_cancelled` reads `false`.
assert!(cancel::current().is_none() && !cancel::is_cancelled());
````

The doc test wraps this block in its own `main` and never calls `desk_loop` or `stop_on_ctrl_c`, so only step 6's `assert!` runs. The hidden `desk` builds the Harness on an offline broker that lists no model and refuses every round, and counting a finished reply needs a broker that answers.

1. Step 1 defines `desk_loop`, which races each event from [`Session::subscribe_events`](crate::Session::subscribe_events) against [`wait_cancelled`] in [`tokio::select!`](https://docs.rs/tokio/latest/tokio/macro.select.html), and returns when the wait completes. Each [`SessionEvent`](crate::SessionEvent) carries the event as JSON in its `event` field, and a `kind` of `assistant_reply` marks one finished model answer, so the loop counts finished replies. The loop takes no flag, yet reaches the installed one, and stops only between events.
2. Step 2 makes one flag with [`CancelHandle::new`], the same as [`Default`], and a clone for the Ctrl-C task. Cancelling any clone cancels them all.
3. Step 3 spawns a task that calls [`CancelHandle::cancel`] on its clone. In your program, that task awaits [`tokio::signal::ctrl_c`](https://docs.rs/tokio/latest/tokio/signal/fn.ctrl_c.html) first. Here it cancels at once, maybe before `desk_loop` first runs. Either order ends the same way, because `wait_cancelled` returns at once when the flag is already cancelled, so the loop returns `Ok`, possibly with zero replies.
4. Step 4 installs the flag by wrapping `desk_loop` in [`scope`], which returns the loop's own output.
5. Step 5 asserts the loop returned `Ok`, the flag reads cancelled, and [`current`] is `None` once `scope` exits.
6. Step 6 runs outside every `scope`, where `current` returns `None` and the free function [`is_cancelled`] returns `false`.

Installing the flag does not stop the work. It stops only at a safe point, where it checks the flag:

- Async code awaits the free function `wait_cancelled()`, as `desk_loop` does.
- Synchronous code, such as a loop with no `.await`, polls the free function `is_cancelled()`, which returns `false` when no flag is installed.
- Code that holds the handle puts [`handle.cancelled()`](CancelHandle::cancelled) in `select!` beside the session's event channel.

Outside any `scope`, `wait_cancelled()` never completes and never errors, so call `current()` first to check for a flag.

A task you spawn does not inherit the flag. `scope` installs the flag only while the future it wraps runs, so code outside that future, even on the same task, does not see it. Inside a task started by [`tokio::spawn`](https://docs.rs/tokio/latest/tokio/fn.spawn.html), `current()` returns `None` and `wait_cancelled()` never completes. Read `current()` before you spawn, move the clone into the task, and install it again with `scope` there, or the task keeps running after the cancel.

A spawned waiter needs its own clone, too, because a spawned future must own everything it uses, and the future that `cancelled` returns borrows the handle. Move a clone into the task with `async move`, and call `cancelled` on it there.

When your function accepts an optional handle from its caller, use [`maybe_scope`]. A function such as `run_turn(cancel: Option<CancelHandle>)` passes it straight through as `maybe_scope(cancel, work).await`. `Some(handle)` behaves exactly like `scope(handle, fut)`. `None` installs nothing, so a flag from an enclosing `scope` stays visible.

Nested scopes shadow each other, so code sees the innermost flag. Once cancelled, a flag stays cancelled, and `cancelled()` resolves at once every time, so a late waiter still stops.

This `CancelHandle` is unrelated to [`promptforge::cancel::CancelHandle`], which shares its name, so importing the wrong one gives confusing type errors. A *run*, one execution of the agent file inside a session as the [main page](crate) describes, checks that other, synchronous flag and never this handle, and no public call connects the two. Cancelling this handle never stops a session's run; call [`Session::cancel`](crate::Session::cancel) to stop the turn, or [`Session::close`](crate::Session::close) to end the session, as the main page's [Stop a turn](crate#stop-a-turn) tour teaches.

You might expect `cancel` to abort the work the way dropping a future or calling [`JoinHandle::abort`](https://docs.rs/tokio/latest/tokio/task/struct.JoinHandle.html#method.abort) does. Instead, the work keeps running until it reaches a point that awaits `wait_cancelled()` or polls `is_cancelled()`.

Install one flag, check it at safe points, and hand it across every spawn yourself. Next, [Stop one helper](#stop-one-helper) stops one task while the rest keep running.

# Stop one helper

`desk` runs two helper tasks beside its main loop, such as a reply printer. You want to stop one helper while the other and the loop keep running. A clone of the loop's flag will not do, because cancelling any clone cancels them all. You need a flag that hears its parent's cancel but keeps its own cancel to itself. That is a *child flag*, made with [`CancelHandle::child`].

A child flag behaves like a clone of its parent when the parent is cancelled. Unlike a clone, cancelling the child stays with the child. A cancel travels down from a flag to its children, never up to its parent or across to its siblings.

The example's hidden setup stands in for the first tour's loop with a task that installs `desk_flag` and only waits, so it ends exactly when `desk_flag` is cancelled. Here `desk_loop` is that task's `JoinHandle`, not the async function of the same name from the first tour.

````
use harness::cancel::{self, CancelHandle};
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let desk_flag = CancelHandle::new();
# let desk_loop = tokio::spawn(cancel::scope(desk_flag.clone(), cancel::wait_cancelled()));

// 1. A desk helper, such as its reply printer, installs its own flag and runs until it is cancelled.
fn helper(flag: CancelHandle) -> tokio::task::JoinHandle<&'static str> {
    tokio::spawn(cancel::scope(flag, async {
        cancel::wait_cancelled().await;
        "stopped"
    }))
}

// 2. Give each helper its own child of the loop's flag, moved into its task.
let (first_flag, second_flag) = (desk_flag.child(), desk_flag.child());
let first = helper(first_flag.clone());
let second = helper(second_flag.clone());

// 3. Stop the first helper alone by cancelling its child.
first_flag.cancel();
assert_eq!(first.await?, "stopped");

// 4. The cancel went neither up to the loop's flag nor across to the second helper.
assert!(!desk_flag.is_cancelled() && !second_flag.is_cancelled());
assert!(!second.is_finished() && !desk_loop.is_finished());

// 5. Cancel the loop's flag: the cancel travels down, so the second helper stops too.
desk_flag.cancel();
assert_eq!(second.await?, "stopped");
desk_loop.await?;
# Ok(())
# }
````

1. Step 1 defines `helper`, which spawns a task that installs its flag with [`scope`] inside the task, then waits on [`wait_cancelled`]. A spawned task sees a flag only when it installs one itself.
2. Step 2 makes two children of the loop's flag with `child`, and moves one into each helper. Each helper now holds a flag you can cancel alone.
3. Step 3 cancels the first child with [`CancelHandle::cancel`] and awaits the first helper. Cancelling a child wakes that child's `wait_cancelled`.
4. Step 4 asserts, with [`CancelHandle::is_cancelled`], that the loop's flag and the second child read not cancelled, and that the second helper and the loop are still running. A cancel never travels up to the parent or across to a sibling.
5. Step 5 cancels the loop's flag, then awaits the second helper and the loop. A child still hears its parent's cancel after a sibling was cancelled.

The diagram shows which way a cancel travels. A cancel on the loop's flag reaches both children, while a cancel on the first child reaches neither the loop's flag nor the second helper.

````text
              +---------------+
              |  loop's flag  |   desk_flag.cancel() reaches both
              +-------+-------+
            child()   |   child()
          +-----------+-----------+
          |                       |
          v                       v
  +---------------+       +---------------+
  | first helper  |  -X-  | second helper |
  +---------------+       +---------------+
          |
          X   first_flag.cancel() reaches neither
              the loop's flag nor the second helper

  a cancel travels down (v), never up or across (X)
````

A child flag is cancelled when its parent, or any ancestor above it, is cancelled. So one Ctrl-C on the top flag still stops every helper. Give each helper its own child, moved into the task and installed there with `scope`, because spawned tasks do not inherit the flag. Cancelling a child leaves the parent and its siblings running, and those siblings still stop on a later parent cancel.

`child()` is not `clone()`. A clone is the same flag under a second name: cancelling either one cancels both. A child is a new flag: it hears its parent's cancel, but its own cancel does not reach the parent. Clones of a child share the child's flag, not the parent's. If you hand a helper a clone instead of a child, cancelling the helper stops everything.

Children nest to any depth, so a parent cancel reaches a grandchild, and helpers can start their own helpers and still obey the top flag.

Nested scopes shadow each other: `wait_cancelled` and the free `is_cancelled` read only the innermost flag. So a helper that wraps its work in `scope(CancelHandle::new(), ...)` inside the loop's scope no longer hears the loop's cancel. Give the helper `child()` of the outer flag instead, and it still stops on the loop's cancel.

A child made from a parent that is already cancelled starts out cancelled. A helper launched during shutdown stops at once instead of running on.

Calling `cancel` a second time does nothing, because a cancel is permanent and `is_cancelled` never returns to `false`. Any number of shutdown paths can cancel the same flag without coordinating.

Dropping a flag, or a [`cancelled()`](CancelHandle::cancelled) future that has not resolved, changes nothing for other clones and never panics. A helper that exits on its own does not cancel or break the others.

You might expect `child()` to be another name for `clone()`, so that cancelling a helper's flag stops the whole program. Instead, a child hears its parent's cancel, but its own cancel reaches only itself, its clones, and all of its descendants, including grandchildren.

Cancels flow down the tree, never up. Next, the [record page](crate::record) shows how to record every run and read a session's transcript.

# Reference

## CancelHandle

[`CancelHandle`] is a shared cancel flag you hand to a piece of work, so it stops at its next safe point instead of being dropped mid-step. Install it around the work with [`scope`], and cancel it from your Ctrl-C task; none of its methods fail. Clones share one state, the handle is `Send`, `Sync`, and `'static`, and dropping a clone changes nothing for the others. It is unrelated to [`promptforge::cancel::CancelHandle`], and [Stop work on Ctrl-C](#stop-work-on-ctrl-c) teaches it.

- [`CancelHandle::new`]: makes a handle that is not cancelled, the same as `Default`. A struct literal cannot build one.
- [`CancelHandle::child`]: cancelled whenever this handle or an ancestor is, and starts cancelled if this handle is. Its own cancel never reaches the parent or siblings.
- [`CancelHandle::cancel`]: cancels this handle, every clone, and every descendant, and wakes all waiters. Calling it again does nothing.
- [`CancelHandle::is_cancelled`]: reports, without waiting, whether this handle, a clone, or an ancestor was cancelled. Once `true`, it never returns `false`.
- [`CancelHandle::cancelled`]: resolves on cancel, at once if already cancelled, and never misses a cancel that lands before the await. It borrows the handle.

## current

[`current`] returns a clone of the [`CancelHandle`] that [`scope`] installed on this task, or `None` outside any scope. Call it before you spawn a task, because the task does not inherit the handle. Move the clone into the task and install it there with `scope`, or install its [`child`](CancelHandle::child) to cancel that task on its own. The clone shares state, so it sees a later cancel, and [Stop work on Ctrl-C](#stop-work-on-ctrl-c) uses it; it never fails.

## is_cancelled

[`is_cancelled`] checks, without waiting, whether the handle installed on this task has been cancelled. Use it in synchronous code that polls at its own safe points, and when it returns `true`, finish the current step and return. It reads the handle installed by the innermost [`scope`], not one you hold; for that, call [`CancelHandle::is_cancelled`]. It returns `false` when no handle is installed, so a missing `scope` makes the work silently uncancellable. [Stop work on Ctrl-C](#stop-work-on-ctrl-c) shows it.

## maybe_scope

[`maybe_scope`] runs a future under [`scope`] when you have a [`CancelHandle`], or runs it plainly when you do not, and returns its output. Use it when your function accepts an optional handle from its caller, and pass the caller's `Option<CancelHandle>` straight through. With `Some(handle)` it behaves exactly like `scope(handle, fut)`. With `None` it installs nothing, so a handle from an enclosing `scope` still applies. It never fails, and [Stop work on Ctrl-C](#stop-work-on-ctrl-c) introduces it.

## scope

[`scope`] runs a future with a [`CancelHandle`] installed on this task, and returns the future's output. Use it around your own async work, such as an event loop or a helper task. Installing does not stop the future; inside it, await [`wait_cancelled`] or poll [`is_cancelled`] at safe points. Tasks the future spawns do not inherit the handle, so install a clone or a [`child`](CancelHandle::child) in each. Nested scopes shadow, so code sees the innermost handle, and [Stop work on Ctrl-C](#stop-work-on-ctrl-c) teaches it.

## wait_cancelled

[`wait_cancelled`] waits until the handle installed on this task, or any ancestor of it, is cancelled, and returns at once if it already is. Race it against the work in [`tokio::select!`](https://docs.rs/tokio/latest/tokio/macro.select.html). With no handle installed it never completes and raises no error, so make sure a [`scope`] encloses the call. It reads the handle when first polled, so a future created before `scope` and awaited inside it still sees it. [Stop work on Ctrl-C](#stop-work-on-ctrl-c) teaches it.

