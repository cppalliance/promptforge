//! Attach-or-launch lifecycle for the desktop app's Gateway sidecar.
//!
//! Boot planning and one-shot launch (`boot`), validated identity
//! (`identity`), and continuous supervision (`supervisor`) are private
//! sibling modules whose imports run both ways. `boot` and `supervisor`
//! import each other: boot authenticates the child it launched as the
//! supervisor's `RecoveryCandidate`, and the supervisor relaunches
//! through boot's sibling lookup and detached spawn. `identity` and
//! `supervisor` import each other too: `GatewayAttachment` wraps that
//! candidate, and recovery compares processes with
//! `same_gateway_identity`. `boot` also returns `identity`'s
//! `GatewayAttachment`, and `identity` never imports `boot`.

mod boot;
mod identity;
mod supervisor;

pub(crate) use boot::ensure_gateway;
pub(crate) use identity::GatewayAttachment;
pub(crate) use supervisor::{GatewaySupervisor, SupervisorShutdown, supervise};

#[cfg(test)]
mod tests;
