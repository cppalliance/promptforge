Cooperative cancellation for a run.

# Cancelling a run

The engine is a pure state machine, so it never awaits a cancellation: it polls a synchronous flag between chain steps and from the Lua instruction hook, and the host sets that flag from whichever thread it likes. [`CancelHandle`] is the flag. [`Run::cancel`](crate::Run::cancel) sets the run's own, and [`Run::cancel_handle`](crate::Run::cancel_handle) returns it for a host that cancels from another thread.

Cancellation is a request, not a synchronous stop. Running Lua observes the flag from its instruction hook, so a running chunk aborts promptly; the next [`step`](crate::Run::step) tears every chain down, and once the outstanding effects are answered the run ends with [`RunResult::Cancelled`](crate::RunResult::Cancelled). A host cancelling a run answers each effect it abandons with [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped).

# Sharing one flag

A run mints its flag with its [`RunContext`](crate::RunContext), and [`RunContext::cancel`](crate::RunContext::cancel) replaces it with the host's. A host that cancels through an awaitable token bridges the token to this flag, setting the flag when the token fires, and hands the same flag to the capabilities it activates, so one cancel reaches the run and everything working for it.

# The cancellation tree

Clones of a handle share one flag. [`CancelHandle::child`] mints a handle that reports cancelled when its own flag or any ancestor's is set, while a child's cancel never reaches its parent or its siblings: the run holds the root and each task a child, so cancelling the run cancels every task and one task can be cancelled alone. A cancel is idempotent and irreversible.

A host that must wait on the flag rather than poll it awaits [`CancelHandle::cancelled`]; the [`Cancelled`] future is woken by the cancel itself, needs no async runtime, and never spins.
