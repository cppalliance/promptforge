//! What one `tools/call` reports about the run it started.
//!
//! Every call that reaches a prompt produces a [`RunResult`]: the record the
//! registry keeps and the runner returns. What crosses the MCP boundary is its
//! wire form, [`RunResultWire`], carried in the tool result's
//! `structuredContent`. The `content` text block beside it carries the plain
//! product: the run's returned value when it completed, the error when it
//! failed, and - for a run that outlived its call - a line naming the run id to
//! collect with.
//!
//! The two types are kept apart on purpose. [`RunResult`] is internal and is
//! never serialized; the protocol contract a client depends on is
//! [`RunResultWire`] alone, so the record can change without moving the wire
//! shape underneath a caller, and no internal type crosses the boundary merely
//! because it happened to derive `Serialize`.
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

/// How far a run has got, as the wire form reports it.
///
/// This is the flat tag the [`RunResultWire`] carries. The internal record
/// keeps the status and its payload together in [`Outcome`] instead, so a
/// completed run cannot exist without its value nor a failed one without its
/// error; [`RunResult::status`] projects that back to this tag for the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum RunStatus {
    /// The run outlived the call that started it and is still going. Collect it
    /// by id with `check_run`.
    Running,
    /// The run finished and returned a value.
    Completed,
    /// The run started and failed.
    Failed,
}

/// A run's status and the payload that status owns, kept as one value so no
/// combination can contradict itself.
///
/// A completed run carries its value and its turn count; a failed one carries
/// its error and its turn count; a running one carries neither, and reports
/// [`NO_TURNS`] because its tally is not yet final. There is no representable
/// state in which a run is `Completed` without a value, `Failed` without an
/// error, or terminal with a payload for the other outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// The run is still going in the background.
    Running,
    /// The run finished and returned `value` after `turns` model round trips.
    Completed { value: String, turns: u32 },
    /// The run started and failed with `error` after `turns` model round trips.
    Failed { error: String, turns: u32 },
}

/// One run, as the crate tracks it internally.
///
/// The status and its payload are held together in a single [`Outcome`], so a
/// completed run always carries its value and a failed one its error and no
/// field can contradict another. This is the record the registry stores and the
/// runner returns; [`to_wire`](Self::to_wire) renders the [`RunResultWire`] that
/// actually crosses the MCP boundary. Its fields are read through accessors
/// rather than exposed, so the outcome and its payload cannot be set apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunResult {
    /// The run's identifier, unique within this process's lifetime. It is what
    /// a caller collects a still-running run by.
    run_id: String,
    /// The prompt's frontmatter name.
    prompt: String,
    /// How long the run took, in milliseconds, measured from the moment it was
    /// admitted rather than from the moment it was asked for, so a queue wait is
    /// none of it.
    elapsed_ms: u64,
    /// The run's status and the payload that status owns.
    outcome: Outcome,
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
            elapsed_ms,
            outcome: Outcome::Completed { value, turns },
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
            elapsed_ms,
            outcome: Outcome::Failed { error, turns },
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
            elapsed_ms,
            outcome: Outcome::Running,
        }
    }

    /// The run's identifier.
    pub(crate) fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The prompt's frontmatter name.
    pub(crate) fn prompt(&self) -> &str {
        &self.prompt
    }

    /// How long the run took, in milliseconds, measured from admission.
    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }

    /// How far the run has got, projected to the flat wire tag.
    pub(crate) fn status(&self) -> RunStatus {
        match self.outcome {
            Outcome::Running => RunStatus::Running,
            Outcome::Completed { .. } => RunStatus::Completed,
            Outcome::Failed { .. } => RunStatus::Failed,
        }
    }

    /// How many model round trips the run took. A run still going reports
    /// [`NO_TURNS`], because its tally is not yet final.
    pub(crate) fn turns(&self) -> u32 {
        match &self.outcome {
            Outcome::Running => NO_TURNS,
            Outcome::Completed { turns, .. } | Outcome::Failed { turns, .. } => *turns,
        }
    }

    /// What the run returned, present only once it completed.
    pub(crate) fn value(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Completed { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Why the run failed, present only on a failure.
    pub(crate) fn error(&self) -> Option<&str> {
        match &self.outcome {
            Outcome::Failed { error, .. } => Some(error),
            _ => None,
        }
    }

    /// The text block that goes beside this result: the value on completion,
    /// the error on failure, and a collection instruction while running.
    ///
    /// The payload is taken straight from the outcome that owns it, so there is
    /// no absent value to mask: a completed run has its value and a failed one
    /// its error by construction.
    pub(crate) fn text(&self) -> String {
        match &self.outcome {
            Outcome::Completed { value, .. } => value.clone(),
            Outcome::Failed { error, .. } => error.clone(),
            Outcome::Running => format!(
                "{} is still running as run {}. Collect it with check_run.",
                self.prompt, self.run_id
            ),
        }
    }

    /// This result in its wire form, which is the only shape serialized into a
    /// tool result's `structuredContent`.
    pub(crate) fn to_wire(&self) -> RunResultWire {
        RunResultWire {
            run_id: self.run_id.clone(),
            prompt: self.prompt.clone(),
            status: self.status(),
            value: self.value().map(str::to_owned),
            turns: self.turns(),
            elapsed_ms: self.elapsed_ms,
            error: self.error().map(str::to_owned),
        }
    }
}

/// The wire form of a [`RunResult`]: the flat JSON object a `tools/call` carries
/// in `structuredContent`.
///
/// This is the stable protocol contract. It is a separate type from
/// [`RunResult`] so the internal record can change without moving the wire shape
/// underneath a client, and so the only thing serialized across the boundary is
/// this purpose-built shape rather than an internal type that happens to derive
/// `Serialize`.
///
/// A completed run carries its [`value`](Self::value) and no
/// [`error`](Self::error); a failed one is the reverse; a running one carries
/// neither and reports [`NO_TURNS`].
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[non_exhaustive]
pub(crate) struct RunResultWire {
    /// The run's identifier, unique within this process's lifetime.
    pub(crate) run_id: String,
    /// The prompt's frontmatter name.
    pub(crate) prompt: String,
    /// How far the run has got.
    pub(crate) status: RunStatus,
    /// What the run returned, present only once it completed.
    pub(crate) value: Option<String>,
    /// How many model round trips the run took.
    pub(crate) turns: u32,
    /// How long the run took, in milliseconds, measured from admission.
    pub(crate) elapsed_ms: u64,
    /// Why the run failed, present only on a failure.
    pub(crate) error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{RunResult, RunStatus};

    #[test]
    fn a_completed_run_texts_its_value() {
        let result = RunResult::completed("r1".into(), "echo", "hello".into(), 2, 4);
        assert_eq!(result.status(), RunStatus::Completed);
        assert_eq!(result.text(), "hello");
        assert_eq!(result.turns(), 2);
        assert!(result.error().is_none());
    }

    #[test]
    fn a_failed_run_texts_its_error() {
        let result = RunResult::failed("r1".into(), "echo", "lua: boom".into(), 1, 4);
        assert_eq!(result.status(), RunStatus::Failed);
        assert_eq!(result.text(), "lua: boom");
        assert_eq!(result.turns(), 1);
        assert!(result.value().is_none());
    }

    #[test]
    fn a_running_run_texts_how_to_collect_it() {
        let result = RunResult::running("r1".into(), "echo", 240_000);
        assert_eq!(result.status(), RunStatus::Running);
        let text = result.text();
        assert!(text.contains("r1"), "the id to collect by: {text}");
        assert!(
            text.contains("check_run"),
            "the tool to collect with: {text}"
        );
        assert!(result.value().is_none());
        assert!(result.error().is_none());
    }

    #[test]
    fn equal_runs_compare_equal_and_differ_from_other_states() {
        let a = RunResult::completed("r1".into(), "echo", "hello".into(), 2, 4);
        let b = RunResult::completed("r1".into(), "echo", "hello".into(), 2, 4);
        let failed = RunResult::failed("r1".into(), "echo", "boom".into(), 2, 4);
        assert_eq!(a, b);
        assert_ne!(a, failed);
    }

    #[test]
    fn status_serializes_in_snake_case() {
        let json = serde_json::to_string(&RunStatus::Running).expect("a unit enum serializes");
        assert_eq!(json, "\"running\"");
    }

    #[test]
    fn a_completed_wire_object_is_exactly_its_fields() {
        let wire = RunResult::completed("r1".into(), "echo", "hello".into(), 2, 4).to_wire();
        assert_eq!(
            serde_json::to_value(&wire).expect("the wire form serializes"),
            serde_json::json!({
                "run_id": "r1",
                "prompt": "echo",
                "status": "completed",
                "value": "hello",
                "turns": 2,
                "elapsed_ms": 4,
                "error": null,
            })
        );
    }

    #[test]
    fn a_failed_wire_object_carries_its_error_and_a_null_value() {
        let wire = RunResult::failed("r1".into(), "echo", "lua: boom".into(), 1, 4).to_wire();
        assert_eq!(
            serde_json::to_value(&wire).expect("the wire form serializes"),
            serde_json::json!({
                "run_id": "r1",
                "prompt": "echo",
                "status": "failed",
                "value": null,
                "turns": 1,
                "elapsed_ms": 4,
                "error": "lua: boom",
            })
        );
    }

    #[test]
    fn a_running_wire_object_reports_no_turns_and_no_payload() {
        let wire = RunResult::running("r1".into(), "echo", 240_000).to_wire();
        assert_eq!(
            serde_json::to_value(&wire).expect("the wire form serializes"),
            serde_json::json!({
                "run_id": "r1",
                "prompt": "echo",
                "status": "running",
                "value": null,
                "turns": 0,
                "elapsed_ms": 240_000,
                "error": null,
            })
        );
    }
}
