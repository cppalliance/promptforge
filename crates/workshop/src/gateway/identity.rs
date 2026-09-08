//! Local Gateway attachment and validated process identity.

use shared_sidecar::ValidatedConnection;

use super::supervisor::RecoveryCandidate;

/// How boot connected the Gateway.
#[derive(Debug)]
pub(crate) enum GatewayAttachment {
    /// A local sidecar Gateway the shell attached to.
    Sidecar(ValidatedConnection),
    /// A child launched by this boot that has not yet entered server state.
    Launched(RecoveryCandidate),
    /// An explicit-config Gateway that the shell does not own.
    Config,
}

impl GatewayAttachment {
    /// Returns the validated sidecar identity, when the Gateway is local.
    pub(crate) fn sidecar_identity(&self) -> Option<&ValidatedConnection> {
        match self {
            Self::Sidecar(identity) => Some(identity),
            Self::Launched(candidate) => Some(candidate.validated()),
            Self::Config => None,
        }
    }

    /// Reconciles the shell's candidate with the identity the server actually
    /// published, disarming launched-child cleanup only for an exact match.
    pub(crate) fn reconcile_publication(self, published: Option<ValidatedConnection>) -> Self {
        match (self, published) {
            (Self::Launched(mut candidate), Some(published))
                if candidate.validated().same_boot(&published) =>
            {
                candidate.published();
                Self::Launched(candidate)
            }
            (_, Some(published)) => Self::Sidecar(published),
            (_, None) => Self::Config,
        }
    }
}

/// Whether two capabilities prove the same Gateway process boot.
pub(super) fn same_gateway_identity(
    left: &ValidatedConnection,
    right: &ValidatedConnection,
) -> bool {
    left.same_boot(right)
}
