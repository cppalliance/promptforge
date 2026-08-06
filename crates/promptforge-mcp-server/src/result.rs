//! What one `tools/call` reports about the run it started.
//!
//! Every call that reaches a prompt answers with a [`RunResult`], carried in
//! the tool result's `structuredContent`. The `content` text block beside it
//! carries the plain product: the run's returned value when it completed, the
//! error when it failed, and - for a run that outlived its call - a line naming
//! the run id to collect with.
//!
//! The value itself is the whole product. The core writes no output files, so
//! there is no path to hand back instead, and re-emitting the body is not a
//! duplication of something the caller could have fetched.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

/// The turn count of a run that never started, or has not finished.
///
/// A prompt that will not parse, or whose tools cannot be bound, reaches no
/// model at all, so its zero is a fact rather than a missing measurement. A run
/// still going reports the same zero for the opposite reason: its tally is not
/// final, and a partial count would read as a total.
pub(crate) const NO_TURNS: u32 = 0;

/// How far a run has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunStatus {
    /// The run outlived the call that started it and is still going. Collect it
    /// by id with `check_run`.
    Running,
    /// The run finished and its value is in [`RunResult::value`].
    Completed,
    /// The run started and failed; [`RunResult::error`] says how.
    Failed,
}

/// One run, as the calling model sees it.
///
/// A completed run carries its [`value`](Self::value) and no
/// [`error`](Self::error); a failed one is the reverse.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[non_exhaustive]
pub struct RunResult {
    /// The run's identifier, unique within this process's lifetime. It is what
    /// a caller collects a still-running run by.
    pub run_id: String,
    /// The prompt's frontmatter name.
    pub prompt: String,
    /// How far the run has got.
    pub status: RunStatus,
    /// What the run returned, present only once it completed.
    pub value: Option<String>,
    /// How many model round trips the run took, as counted by whatever observed
    /// it. A caller that observed nothing reports zero.
    pub turns: u32,
    /// How long the run took, in milliseconds, measured from the moment it was
    /// admitted rather than from the moment it was asked for, so a queue wait is
    /// none of it.
    pub elapsed_ms: u64,
    /// Why the run failed, present only on a failure.
    pub error: Option<String>,
}

impl RunResult {
    /// A run that finished and returned `value`, having taken `turns` model
    /// round trips.
    ///
    /// The turn count is the caller's to supply, because only the caller knows
    /// what observed the run. A run nothing counted reports zero, and that zero
    /// is then a statement its call site made rather than a constant buried
    /// here where no caller would think to look for it.
    pub(crate) fn completed(
        run_id: String,
        prompt: &str,
        value: String,
        turns: u32,
        elapsed_ms: u64,
    ) -> RunResult {
        RunResult {
            run_id,
            prompt: prompt.to_owned(),
            status: RunStatus::Completed,
            value: Some(value),
            turns,
            elapsed_ms,
            error: None,
        }
    }

    /// A run that started and failed after `turns` model round trips, or one
    /// that could not start because the prompt it names is broken.
    ///
    /// The turn count is the caller's to supply, for the reason
    /// [`completed`](Self::completed) gives.
    pub(crate) fn failed(
        run_id: String,
        prompt: &str,
        error: String,
        turns: u32,
        elapsed_ms: u64,
    ) -> RunResult {
        RunResult {
            run_id,
            prompt: prompt.to_owned(),
            status: RunStatus::Failed,
            value: None,
            turns,
            elapsed_ms,
            error: Some(error),
        }
    }

    /// A run that outlived the call which started it and is still going in the
    /// background.
    ///
    /// It reports [`NO_TURNS`], because the tally that matters is the one the
    /// finished record carries, and `elapsed_ms` measures how long it has been
    /// going rather than how long it took.
    pub(crate) fn running(run_id: String, prompt: &str, elapsed_ms: u64) -> RunResult {
        RunResult {
            run_id,
            prompt: prompt.to_owned(),
            status: RunStatus::Running,
            value: None,
            turns: NO_TURNS,
            elapsed_ms,
            error: None,
        }
    }

    /// The text block that goes beside this result: the value on completion,
    /// the error on failure, and a collection instruction while running.
    pub(crate) fn text(&self) -> String {
        match self.status {
            RunStatus::Completed => self.value.clone().unwrap_or_default(),
            RunStatus::Failed => self.error.clone().unwrap_or_default(),
            RunStatus::Running => format!(
                "{} is still running as run {}. Collect it with check_run.",
                self.prompt, self.run_id
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RunResult, RunStatus};

    #[test]
    fn a_completed_run_texts_its_value() {
        let result = RunResult::completed("r1".into(), "echo", "hello".into(), 2, 4);
        assert_eq!(result.status, RunStatus::Completed);
        assert_eq!(result.text(), "hello");
        assert_eq!(result.turns, 2);
        assert!(result.error.is_none());
    }

    #[test]
    fn a_failed_run_texts_its_error() {
        let result = RunResult::failed("r1".into(), "echo", "lua: boom".into(), 1, 4);
        assert_eq!(result.status, RunStatus::Failed);
        assert_eq!(result.text(), "lua: boom");
        assert_eq!(result.turns, 1);
        assert!(result.value.is_none());
    }

    #[test]
    fn a_running_run_texts_how_to_collect_it() {
        let result = RunResult::running("r1".into(), "echo", 240_000);
        assert_eq!(result.status, RunStatus::Running);
        let text = result.text();
        assert!(text.contains("r1"), "the id to collect by: {text}");
        assert!(
            text.contains("check_run"),
            "the tool to collect with: {text}"
        );
        assert!(result.value.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn status_serializes_in_snake_case() {
        let json = serde_json::to_string(&RunStatus::Running).expect("a unit enum serializes");
        assert_eq!(json, "\"running\"");
    }
}
