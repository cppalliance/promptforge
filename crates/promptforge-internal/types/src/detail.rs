//! Operations on the identity types that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host. Each function builds an identity without
//! validating it; hosts build identities through the checked constructors
//! ([`ModelId::new`], [`CapabilityId::parse`], [`ToolId::parse`]).

use crate::capabilities::CapabilityId;
use crate::models::ModelId;
use crate::tools::ToolId;

/// Builds a model identity from components already known to be valid,
/// such as the parts of an existing [`ModelId`], where [`ModelId::new`]'s
/// validation is redundant.
#[must_use]
pub fn model_id_from_validated(server: impl Into<String>, name: impl Into<String>) -> ModelId {
    ModelId::from_validated(server, name)
}

/// Builds a capability identity from a string already known to be a valid
/// 2-segment id, where [`CapabilityId::parse`]'s validation is redundant.
#[must_use]
pub fn capability_id_from_validated(id: &str) -> CapabilityId {
    CapabilityId::from_validated(id)
}

/// Builds a tool identity from a string already known to be a valid
/// 3-segment id, where [`ToolId::parse`]'s validation is redundant.
#[must_use]
pub fn tool_id_from_validated(id: &str) -> ToolId {
    ToolId::from_validated(id)
}
