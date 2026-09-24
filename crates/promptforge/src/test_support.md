The engine's test drivers, for companion crates' suites.

# Driving a run in a test

[`drive_tokio`] drives a [`Run`](crate::Run) to its end on the current tokio runtime: it performs each `Chat`, `ToolCall`, and `UserInput` effect through the caller's [`Performers`], performs store operations, timers, and task-history reads itself, hands every event to the caller's sink in order, and cancels the run when the caller's [`CancelHandle`](crate::cancel::CancelHandle) fires. Once the run reports itself decided, every effect still out is answered [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped), so the run always ends with every effect answered exactly once.

Each [`Performer`] is a boxed closure handed the whole [`Effect`](crate::effect::Effect) that returns a [`BoxFuture`] of its [`EffectAnswer`](crate::effect::EffectAnswer). [`Performers::refusing`] answers every kind with its refusal - a disabled-gateway completion error, a no-implementation tool error, and unavailable input - and a suite overrides the slots it supplies.

This is a test host, enabled by the `test-support` feature from dev-dependencies only. A production host writes its own loop over [`Run::step`](crate::Run::step) and [`Run::resume`](crate::Run::resume), as the crate page shows.
