//! Attach-or-launch lifecycle for the desktop shell's Gateway sidecar.
//!
//! Boot planning and one-shot launch, validated identity, and continuous
//! supervision are private sibling modules with one-way dependencies.

mod boot;
mod identity;
mod supervisor;

pub(crate) use boot::ensure_gateway;
pub(crate) use identity::GatewayAttachment;
pub(crate) use supervisor::{GatewaySupervisor, SupervisorShutdown, supervise};

#[cfg(test)]
mod tests;
