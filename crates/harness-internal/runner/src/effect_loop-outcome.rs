//! The recorded outcome of a run: what the loop hands the recorder when a
//! run ends, and what a refused preparation records in its place.

use runvil::{RunError, RunResult};

use crate::display_chain::display_chain;
use crate::recorder::RunOutcome;

/// The recorded outcome for the run's result. An outcome kind this Harness
/// does not know is recorded as an internal failure naming it, since the
/// result enum is `#[non_exhaustive]`.
pub(super) fn outcome_of(result: RunResult) -> RunOutcome {
    match result {
        RunResult::Ok(final_text) => RunOutcome::Completed { final_text },
        RunResult::Cancelled => RunOutcome::Cancelled,
        RunResult::Failure(error) => failed_outcome(&error),
        unknown => RunOutcome::Failed {
            kind: "Internal".to_owned(),
            message: format!(
                "the run ended with an outcome this Harness does not know: {unknown:?}"
            ),
        },
    }
}

/// The failed outcome for an Engine error: the failure's kind is the
/// error kind's debug name and its message the error's text with its
/// cause chain. The one derivation for a run that failed under the loop
/// and a run preparation refused, so the two agree in the record.
pub(crate) fn failed_outcome(error: &RunError) -> RunOutcome {
    RunOutcome::Failed {
        kind: format!("{:?}", error.kind()),
        message: display_chain(error),
    }
}
