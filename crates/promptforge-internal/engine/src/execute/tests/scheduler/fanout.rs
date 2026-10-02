//! Fanout mechanics and failure semantics on the scheduler. Each submodule
//! holds one topic and reaches the helpers below through `use super::*`.

use super::*;
use crate::test_support::tokio_driver::TokioDriver;

/// Builds the run context and its silent Harness for a scheduler fanout test
/// with the given limits, so an admission test can narrow the ceiling.
fn scheduler_context_with_limits(prompt: &Prompt, limits: RunLimits) -> (RunState, RunHarness) {
    scheduler_context_from(
        prompt,
        &TestStore::new(),
        &test_context(EXECUTION).limits(limits),
        RunHarness::new(),
    )
}

/// The prompt in each gateway request, in arrival order.
pub(super) fn request_prompts(gateway: &ScriptedChat) -> Vec<String> {
    gateway
        .requests()
        .iter()
        .map(|body| body.messages[0].content().to_owned())
        .collect()
}

mod arms;
mod failure_semantics;
mod scheduling;
