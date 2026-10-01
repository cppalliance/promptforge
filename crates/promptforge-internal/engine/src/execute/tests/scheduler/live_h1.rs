//! The live H1 pass on the scheduler. Each submodule holds one topic and
//! reaches the context builders below through `use super::*`.

use super::*;
use crate::test_support::tokio_driver::TokioDriver;

/// Builds the run context for a scheduler live-H1 test: the shared model
/// set starts empty - the live H1 pass under test records its own
/// bindings.
fn h1_context(prompt: &Prompt) -> (RunState, RunHost) {
    h1_context_on(prompt, &TestStore::new(), Arc::new(NullObserver::default()))
}

/// Builds the H1 run context and its observing Harness on the given store and
/// observer, so a pass test can inspect the store's contents and the
/// observation stream afterward. The context's model bindings are filled the
/// way prepare's trivial fill does: every declared role bound to the test
/// model.
fn h1_context_on(
    prompt: &Prompt,
    store: &TestStore,
    observer: Arc<dyn Observer>,
) -> (RunState, RunHost) {
    let mut ctx = test_context(EXECUTION);
    for (label, _) in prompt.frontmatter().models().iter() {
        ctx.model_bindings.bind(
            label,
            ModelDescriptor::new(
                ModelId::gateway("claude-sonnet-4-6").expect("the test model id is valid"),
                "A general model for tests",
                NonZeroU32::new(131_072).expect("131072 is non-zero"),
                ThinkingMode::Switchable,
            ),
        );
    }
    let state = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &store.vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &ctx,
    );
    (state, RunHost::new().observer(observer))
}

mod control_flow;
mod pass;
mod returns_and_prose;
