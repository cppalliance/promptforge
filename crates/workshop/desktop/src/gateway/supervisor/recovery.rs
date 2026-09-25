//! Recovery-candidate ownership and identity classification.

use std::time::{Duration, Instant};

use gateway_api_discovery::{ShutdownError, ValidatedConnection};

use super::super::identity::same_gateway_identity;
use super::SupervisedGatewayIdentity;

/// Separate bound for authenticated cleanup of an unpublished owned child.
const LATE_CHILD_SHUTDOWN_BUDGET: Duration = Duration::from_secs(1);

/// A validated recovery process whose pid proves it is the child we spawned.
#[derive(Debug)]
pub(crate) struct RecoveryCandidate {
    child_pid: u32,
    validated: ValidatedConnection,
    published: bool,
}

pub(crate) enum RecoveryOwnership {
    Owned(RecoveryCandidate),
    Unowned(ValidatedConnection),
}

impl RecoveryCandidate {
    /// Claims cleanup authority only when validation names the spawned pid.
    pub(crate) fn authenticate(
        child_pid: u32,
        validated: ValidatedConnection,
    ) -> RecoveryOwnership {
        if validated.pid() != child_pid {
            return RecoveryOwnership::Unowned(validated);
        }
        RecoveryOwnership::Owned(Self {
            child_pid,
            validated,
            published: false,
        })
    }

    pub(crate) fn validated(&self) -> &ValidatedConnection {
        &self.validated
    }

    pub(crate) fn published(&mut self) {
        self.published = true;
    }

    /// Shuts down the unpublished recovered child within the late-child
    /// budget.
    ///
    /// This is the blocking, error-reporting path; `Drop` only signals on
    /// a detached thread. The drop signal is disarmed either way: the
    /// caller receives the outcome, so a failed delivery is reported here
    /// rather than retried silently.
    pub(crate) fn shutdown(mut self) -> Result<(), ShutdownError> {
        if self.published {
            return Ok(());
        }
        debug_assert_eq!(self.child_pid, self.validated.pid());
        self.published = true;
        let deadline = Instant::now() + LATE_CHILD_SHUTDOWN_BUDGET;
        gateway_api_discovery::request_shutdown_before(&self.validated, deadline)
    }
}

impl Drop for RecoveryCandidate {
    fn drop(&mut self) {
        if self.published {
            return;
        }
        debug_assert_eq!(self.child_pid, self.validated.pid());
        // Drop can neither block nor report: the bounded authenticated
        // request runs on a detached thread, so a missed explicit
        // `shutdown()` still signals the unpublished gateway process.
        let validated = self.validated.clone();
        let signalled = std::thread::Builder::new()
            .name("gateway-late-child-shutdown".to_owned())
            .spawn(move || {
                let deadline = Instant::now() + LATE_CHILD_SHUTDOWN_BUDGET;
                if let Err(error) =
                    gateway_api_discovery::request_shutdown_before(&validated, deadline)
                {
                    // The detached signal has no error return channel, so
                    // diagnostics are the only place this cleanup failure
                    // can surface.
                    eprintln!("could not shut down an unpublished recovered gateway: {error}");
                }
            });
        if let Err(error) = signalled {
            eprintln!("could not signal an unpublished recovered gateway: {error}");
        }
    }
}

impl SupervisedGatewayIdentity for ValidatedConnection {
    fn same_boot(&self, other: &Self) -> bool {
        same_gateway_identity(self, other)
    }
}

#[derive(Debug)]
pub(crate) enum RecoveryIdentity {
    Stable(ValidatedConnection),
    Candidate(RecoveryCandidate),
}

impl RecoveryIdentity {
    pub(super) fn validated(&self) -> &ValidatedConnection {
        match self {
            Self::Stable(validated) => validated,
            Self::Candidate(candidate) => candidate.validated(),
        }
    }
}

impl SupervisedGatewayIdentity for RecoveryIdentity {
    fn same_boot(&self, other: &Self) -> bool {
        self.validated().same_boot(other.validated())
    }

    fn publication_succeeded(&mut self) {
        if let Self::Candidate(candidate) = self {
            candidate.published();
        }
    }

    fn shutdown_unpublished(self) {
        if let Self::Candidate(candidate) = self
            && let Err(error) = candidate.shutdown()
        {
            eprintln!("could not shut down an unpublished recovered gateway: {error}");
        }
    }
}
