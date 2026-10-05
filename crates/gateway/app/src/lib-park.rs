//! The test-only phase rendezvous for the boot load and the apply command.

use tokio::sync::Notify;

/// One command phase a test can park.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// The boot load's artifact download, before anything is promised
    /// as loading.
    Download,
    /// The boot load's child spawn, once the local models are promised
    /// as loading.
    Spawn,
    /// A config apply's commit, before it takes the apply lock: the
    /// captured shadows are not yet promoted and nothing is live.
    ApplyCommit,
}

/// Parks the command at `phase` until the test releases it. Single use:
/// each notify stores one permit, so a release before the command
/// arrives is not lost.
#[derive(Debug)]
pub(crate) struct PhasePark {
    phase: Phase,
    entered: Notify,
    release: Notify,
}

impl PhasePark {
    pub(crate) fn at(phase: Phase) -> PhasePark {
        PhasePark {
            phase,
            entered: Notify::new(),
            release: Notify::new(),
        }
    }

    /// Resolves once the command has parked at the phase.
    pub(crate) async fn entered(&self) {
        self.entered.notified().await;
    }

    /// Lets the parked command continue.
    pub(crate) fn release(&self) {
        self.release.notify_one();
    }

    pub(crate) async fn park(&self, phase: Phase) {
        if phase == self.phase {
            self.entered.notify_one();
            self.release.notified().await;
        }
    }
}
