//! Local Gateway attachment and validated process identity.

use shared_sidecar::ValidatedConnection;

/// How boot connected the Gateway.
#[derive(Debug)]
pub(crate) enum GatewayAttachment {
    /// A local sidecar Gateway the shell attached to or launched.
    Sidecar(ValidatedConnection),
    /// An explicit-config Gateway that the shell does not own.
    Config,
}

impl GatewayAttachment {
    /// Returns the validated sidecar identity, when the Gateway is local.
    pub(crate) fn sidecar_identity(&self) -> Option<&ValidatedConnection> {
        match self {
            Self::Sidecar(identity) => Some(identity),
            Self::Config => None,
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
