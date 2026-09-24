//! Operations on the identity types that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host. Each function builds an identity without
//! validating it; hosts build one through its checked constructor, such
//! as [`ModelId::new`].

use crate::models::ModelId;

/// Builds a model identity from components already known to be valid,
/// such as the parts of an existing [`ModelId`], where [`ModelId::new`]'s
/// validation is redundant.
#[must_use]
pub fn model_id_from_validated(server: impl Into<String>, name: impl Into<String>) -> ModelId {
    ModelId::from_validated(server, name)
}
