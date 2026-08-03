//! What one `tools/call` reports about the run it started.
//!
//! Every call that reaches a prompt answers with a [`RunResult`], carried in
//! the tool result's `structuredContent`. The `content` text block beside it
//! carries the plain product: the run's returned value when it completed, the
//! error when it failed, and - once a run can outlive its call - a line naming
//! the run id to collect with.
//!
//! The value itself is the whole product. The core writes no output files, so
//! there is no path to hand back instead, and re-emitting the body is not a
//! duplication of something the caller could have fetched.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

/// How far a run has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunStatus {
    /// The run outlived the call that started it and is still going. Collect it
    /// by id. Nothing produces this yet: the deadline race that does arrives
    /// with the run registry.
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
    /// The prompt's frontmatter contract version, zero for a broken prompt.
    pub version: u32,
    /// How far the run has got.
    pub status: RunStatus,
    /// What the run returned, present only once it completed.
    pub value: Option<String>,
    /// How many model round trips the run took, as counted by whatever observed
    /// it. A caller that observed nothing reports zero.
    pub turns: u32,
    /// How long the run took, in milliseconds, measured around the call.
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
        version: u32,
        value: String,
        turns: u32,
        elapsed_ms: u64,
    ) -> RunResult {
        RunResult {
            run_id,
            prompt: prompt.to_owned(),
            version,
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
        version: u32,
        error: String,
        turns: u32,
        elapsed_ms: u64,
    ) -> RunResult {
        RunResult {
            run_id,
            prompt: prompt.to_owned(),
            version,
            status: RunStatus::Failed,
            value: None,
            turns,
            elapsed_ms,
            error: Some(error),
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
        let result = RunResult::completed("r1".into(), "echo", 1, "hello".into(), 2, 4);
        assert_eq!(result.status, RunStatus::Completed);
        assert_eq!(result.text(), "hello");
        assert_eq!(result.turns, 2);
        assert!(result.error.is_none());
    }

    #[test]
    fn a_failed_run_texts_its_error() {
        let result = RunResult::failed("r1".into(), "echo", 1, "lua: boom".into(), 1, 4);
        assert_eq!(result.status, RunStatus::Failed);
        assert_eq!(result.text(), "lua: boom");
        assert_eq!(result.turns, 1);
        assert!(result.value.is_none());
    }

    #[test]
    fn status_serializes_in_snake_case() {
        let json = serde_json::to_string(&RunStatus::Running).expect("a unit enum serializes");
        assert_eq!(json, "\"running\"");
    }
}
