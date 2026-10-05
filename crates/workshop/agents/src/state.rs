//! Where a conversation stands, and the failure reports it makes.

/// Where a conversation's run stands between its launch and its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// The run is stepping and its effects are being performed.
    Alive,
    /// A close was requested: the run's cancel flag is set, and its
    /// outstanding effects are being answered `Dropped`.
    Closing,
    /// The run ended; nothing is outstanding.
    Closed,
}

impl SessionState {
    /// The state after a close is requested. A closed conversation stays
    /// closed: a late request has nothing left to interrupt.
    #[must_use]
    pub fn interrupted(self) -> Self {
        match self {
            Self::Alive | Self::Closing => Self::Closing,
            Self::Closed => Self::Closed,
        }
    }

    /// The state after the run ends, whatever preceded it.
    #[must_use]
    pub fn done(self) -> Self {
        match self {
            Self::Alive | Self::Closing | Self::Closed => Self::Closed,
        }
    }
}

/// What kind of failure a conversation is reporting: the machine-readable
/// fact a client classifies on. The first two are turn failures the
/// program survived (the built-in chat pcalls `models.loop` and returns
/// to waiting); the last two end the run. Deliberately not
/// `#[non_exhaustive]`: a client that labels each kind matches on it
/// exhaustively, so a new kind fails that client's build until it is
/// labelled instead of silently falling into a wildcard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// A model round failed; the program survived and is waiting again.
    ModelTurnFailed,
    /// A tool dispatch failed; the program survived and is waiting again.
    ToolCallFailed,
    /// The run itself ended in error.
    RunFailed,
    /// The run ended cancelled: a close, or a stop the program did not
    /// catch, cut it short before it finished on its own.
    Interrupted,
}

/// One operator-facing failure report: the kind is the fact code acts
/// on, the message is display text for the operator and the model. Code
/// never derives meaning from the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFailure {
    /// Which failure this is.
    pub kind: FailureKind,
    /// The sentence a client shows for it.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_goes_alive_closing_closed_and_never_back() {
        let cases: [(SessionState, SessionState, SessionState); 3] = [
            (
                SessionState::Alive,
                SessionState::Closing,
                SessionState::Closed,
            ),
            (
                SessionState::Closing,
                SessionState::Closing,
                SessionState::Closed,
            ),
            (
                SessionState::Closed,
                SessionState::Closed,
                SessionState::Closed,
            ),
        ];
        for (start, after_interrupt, after_done) in cases {
            assert_eq!(
                start.interrupted(),
                after_interrupt,
                "{start:?} interrupted"
            );
            assert_eq!(start.done(), after_done, "{start:?} done");
        }
    }
}
