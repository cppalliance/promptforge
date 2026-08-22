//! The execute subtree's ambient run state.
//!
//! [`RunContext`] is built once in [`run`](super::run) and travels through
//! the execute subtree as parameter one (`ctx: &RunContext`). The
//! invariant: a new run-scoped concern becomes a field here, never a new
//! parameter.

use std::sync::Arc;

use crate::parser::Prompt;

/// The ambient state one run shares across the execute subtree.
///
/// Immutable for the run's lifetime and cheap to clone: every field is
/// shared ownership, so a clone points at the same run state.
#[derive(Clone, Debug)]
pub(crate) struct RunContext {
    /// The prompt this run executes.
    prompt: Arc<Prompt>,
}

impl RunContext {
    /// Builds the context for one run of `prompt`.
    #[must_use]
    pub(crate) fn new(prompt: &Prompt) -> Self {
        Self {
            prompt: Arc::new(prompt.clone()),
        }
    }

    /// The prompt this run executes.
    pub(crate) fn prompt(&self) -> &Prompt {
        &self.prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::NullObserver;

    fn test_prompt() -> Prompt {
        let source = concat!(
            "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n",
            "# Title\n\n## Only\n\ndone\n",
        );
        Prompt::parse(source, "run-context-test", &NullObserver).expect("the test prompt parses")
    }

    #[test]
    fn new_builds_a_context_over_the_prompt() {
        let prompt = test_prompt();
        let ctx = RunContext::new(&prompt);
        assert_eq!(ctx.prompt().title, prompt.title);
    }

    #[test]
    fn accessor_returns_the_run_prompt() {
        let prompt = test_prompt();
        let ctx = RunContext::new(&prompt);
        assert_eq!(ctx.prompt(), &prompt);
    }

    #[test]
    fn clones_share_the_prompt_allocation() {
        let ctx = RunContext::new(&test_prompt());
        let clone = ctx.clone();
        assert!(Arc::ptr_eq(&ctx.prompt, &clone.prompt));
    }
}
