//! Attach-or-launch lifecycle for the desktop app's Gateway sidecar.
//!
//! The sidecar is the local `promptforge-gateway` process found through
//! its gateway discovery file or launched detached beside the desktop app.
//! An explicit `workshop.toml` `[gateway]` endpoint is not a sidecar and
//! gets no supervision.
//!
//! Boot planning and detached spawn (`boot`), validated identity
//! (`identity`), and launch-and-wait plus continuous supervision
//! (`supervisor`) are private sibling modules whose imports run both
//! ways. `boot` and `supervisor` import each other: boot launches through
//! the supervisor's cancellable launch-and-wait and authenticates the
//! child as its `RecoveryCandidate`, and the supervisor relaunches
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
