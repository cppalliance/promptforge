//! Shared worker startup deadline and partial-construction cleanup.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::{DecodeMode, TranscribeError};

/// One worker's startup report: whether its optional decoder exists.
pub(crate) type Report = mpsc::Receiver<Result<bool, TranscribeError>>;

/// The production startup wait: every role's outcome is judged against one
/// deadline, taken before any worker spawns.
pub(crate) fn shared_deadline(
    timeout: Duration,
) -> Result<impl FnMut(&Report, DecodeMode) -> Result<bool, TranscribeError>, TranscribeError> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        TranscribeError::InvalidConfig("stt.startup_timeout is too large".to_owned())
    })?;
    Ok(move |init: &Report, mode: DecodeMode| outcome(init, mode, deadline))
}

pub(crate) fn outcome(
    outcome: &mpsc::Receiver<Result<bool, TranscribeError>>,
    mode: DecodeMode,
    deadline: Instant,
) -> Result<bool, TranscribeError> {
    match outcome.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(TranscribeError::WorkerGone),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(match mode {
            DecodeMode::Interim => TranscribeError::InterimStartupTimedOut,
            DecodeMode::Final => TranscribeError::FinalStartupTimedOut,
        }),
    }
}

pub(crate) fn timed_out(outcome: &Result<bool, TranscribeError>) -> bool {
    matches!(
        outcome,
        Err(TranscribeError::InterimStartupTimedOut | TranscribeError::FinalStartupTimedOut)
    )
}

pub(crate) fn pair(
    interim: Result<bool, TranscribeError>,
    final_result: Result<bool, TranscribeError>,
) -> Result<(bool, bool), TranscribeError> {
    let interim = match interim {
        Ok(true) => Ok(()),
        Ok(false) => Err(TranscribeError::InvalidConfig(
            "the interim decoder is required".to_owned(),
        )),
        Err(error) => Err(error),
    };
    match (interim, final_result) {
        (Ok(()), Ok(final_exists)) => Ok((true, final_exists)),
        (Err(interim), Err(final_error)) => Err(TranscribeError::StartupFailures {
            failures: vec![interim, final_error],
        }),
        (Err(error), Ok(_)) | (Ok(()), Err(error)) => Err(error),
    }
}
