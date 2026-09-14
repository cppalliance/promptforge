//! Host-facing model vocabulary: stable identity, catalog, and descriptor.
//!
//! A host builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a
//! pinned offline entry) and names catalog entries by their validated
//! [`ModelId`]. These types carry no transport, binding, or invocation
//! machinery; they are the shared vocabulary every promptforge crate and
//! host may name.

use std::num::NonZeroU32;

use serde::Deserialize;

/// Stable identity of one catalogued model.
///
/// v0 uses the `"gateway"` namespace plus the caller-facing model name (the
/// gateway `[[model]].name` / OpenAI `id`).
///
/// `#[non_exhaustive]` so the invariant-bearing identity is only ever built
/// through [`ModelId::new`]/[`ModelId::gateway`], never by a struct literal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ModelId {
    server: String,
    name: String,
}

impl ModelId {
    /// The v0 gateway identity namespace.
    pub const GATEWAY: &'static str = "gateway";

    /// Builds an identity from its server namespace and model name.
    ///
    /// # Errors
    /// Returns [`ModelIdError`] if `server` or `name` is empty or contains a
    /// control character, so an unusable identity is unrepresentable.
    ///
    /// # Examples
    ///
    /// ```
    /// use shared_promptforge_api::models::ModelId;
    ///
    /// let id = ModelId::new(ModelId::GATEWAY, "claude-sonnet-4-6")?;
    /// assert_eq!(id.server(), "gateway");
    /// assert_eq!(id.name(), "claude-sonnet-4-6");
    /// # Ok::<(), shared_promptforge_api::models::ModelIdError>(())
    /// ```
    pub fn new(
        server: impl Into<String>,
        name: impl Into<String>,
    ) -> std::result::Result<ModelId, ModelIdError> {
        let server = server.into();
        let name = name.into();
        Self::validate("server", &server)?;
        Self::validate("name", &name)?;
        Ok(Self { server, name })
    }

    /// Builds a gateway-namespaced identity from a caller-facing model name.
    ///
    /// # Errors
    /// Returns [`ModelIdError`] if `name` is empty or contains a control
    /// character.
    pub fn gateway(name: impl Into<String>) -> std::result::Result<ModelId, ModelIdError> {
        Self::new(Self::GATEWAY, name)
    }

    /// Builds an identity from components already known to be valid.
    ///
    /// `#[doc(hidden)]`: a cross-crate seam for workspace-internal callers
    /// reconstructing an identity from an existing [`ModelId`]'s parts, where
    /// [`ModelId::new`]'s validation is redundant. Not host API.
    #[doc(hidden)]
    pub fn from_validated(server: impl Into<String>, name: impl Into<String>) -> ModelId {
        ModelId {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Validates one identity component, naming the field in any error.
    ///
    /// Rejection is by Unicode scalar, not raw byte (MODEL-004): every control
    /// character is refused, including C1 controls such as U+0085 (NEL) whose
    /// UTF-8 encoding a byte-range scan would miss.
    fn validate(field: &'static str, value: &str) -> std::result::Result<(), ModelIdError> {
        if value.is_empty() {
            return Err(ModelIdError {
                field,
                reason: "must not be empty",
            });
        }
        if value.chars().any(char::is_control) {
            return Err(ModelIdError {
                field,
                reason: "must not contain a control character",
            });
        }
        Ok(())
    }

    /// Returns the identity namespace.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the caller-facing model name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The reason a [`ModelId`] could not be built from its components.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid model id: {field} {reason}")]
#[non_exhaustive]
pub struct ModelIdError {
    /// Which component was rejected (`server` or `name`).
    field: &'static str,
    /// Why it was rejected.
    reason: &'static str,
}

/// The reason a [`ModelCatalog`] could not be built from its descriptors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ModelCatalogError {
    /// Two descriptors shared one stable [`ModelId`], which would make lookups
    /// ambiguous.
    #[error("duplicate model identity in catalog: {server}/{name}")]
    #[non_exhaustive]
    DuplicateId {
        /// The repeated identity's server namespace.
        server: String,
        /// The repeated identity's model name.
        name: String,
    },
}

/// Whether a catalogued model can emit thinking tokens.
///
/// # Examples
///
/// ```
/// use shared_promptforge_api::models::ThinkingMode;
///
/// // Deserialized from the lowercase gateway wire form.
/// let mode: ThinkingMode = serde_json::from_str("\"switchable\"")?;
/// assert_eq!(mode, ThinkingMode::Switchable);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingMode {
    /// The backend never emits thinking tokens.
    Never,
    /// The backend always emits thinking tokens.
    Always,
    /// The client may turn thinking on or off per request.
    Switchable,
}

/// One catalogued model with live-resolution metadata.
///
/// `#[non_exhaustive]` so the descriptor is only ever built through
/// [`ModelDescriptor::new`] and its validated context window is preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelDescriptor {
    id: ModelId,
    description: String,
    context: NonZeroU32,
    thinking: ThinkingMode,
}

impl ModelDescriptor {
    /// Builds a descriptor from its identity and catalog fields.
    ///
    /// The context window is a [`NonZeroU32`], so a zero-token window is
    /// unrepresentable.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use shared_promptforge_api::models::{ModelDescriptor, ModelId, ThinkingMode};
    ///
    /// let context = NonZeroU32::new(131_072).ok_or("context is non-zero")?;
    /// let model = ModelDescriptor::new(
    ///     ModelId::gateway("analyst")?,
    ///     "A careful analysis model",
    ///     context,
    ///     ThinkingMode::Switchable,
    /// );
    /// assert_eq!(model.context(), context);
    /// assert_eq!(model.thinking(), ThinkingMode::Switchable);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(
        id: ModelId,
        description: impl Into<String>,
        context: NonZeroU32,
        thinking: ThinkingMode,
    ) -> Self {
        Self {
            id,
            description: description.into(),
            context,
            thinking,
        }
    }

    /// Returns the stable identity.
    #[must_use]
    pub fn id(&self) -> &ModelId {
        &self.id
    }

    /// Returns the prose used for semantic resolve.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the context window size in tokens (always non-zero).
    #[must_use]
    pub fn context(&self) -> NonZeroU32 {
        self.context
    }

    /// Returns the thinking capability.
    #[must_use]
    pub fn thinking(&self) -> ThinkingMode {
        self.thinking
    }
}

/// Complete live model set for one bind pass.
///
/// `#[non_exhaustive]` so the collision-free catalog invariant is only ever
/// established through [`ModelCatalog::new`]/[`ModelCatalog::empty`].
// No `Eq`: bindings carry `f64` temperatures transitively.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog from descriptors in host order.
    ///
    /// # Errors
    /// Returns [`ModelCatalogError::DuplicateId`] when two descriptors share one
    /// stable [`ModelId`], so an ambiguous catalog is unrepresentable.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU32;
    /// use shared_promptforge_api::models::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
    ///
    /// let ctx = NonZeroU32::new(8_192).ok_or("context is non-zero")?;
    /// let id = ModelId::gateway("small")?;
    /// let catalog = ModelCatalog::new([ModelDescriptor::new(
    ///     id.clone(),
    ///     "A tiny model",
    ///     ctx,
    ///     ThinkingMode::Never,
    /// )])?;
    /// assert!(catalog.contains(&id));
    /// assert_eq!(catalog.models().len(), 1);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(
        models: impl IntoIterator<Item = ModelDescriptor>,
    ) -> std::result::Result<ModelCatalog, ModelCatalogError> {
        let models: Vec<ModelDescriptor> = models.into_iter().collect();
        for (index, model) in models.iter().enumerate() {
            if models[..index].iter().any(|prior| prior.id() == model.id()) {
                return Err(ModelCatalogError::DuplicateId {
                    server: model.id().server().to_owned(),
                    name: model.id().name().to_owned(),
                });
            }
        }
        Ok(Self { models })
    }

    /// Builds a catalog from descriptors already known to be collision-free.
    ///
    /// Used by internal callers whose inputs are already validated, where
    /// duplicate checking is redundant.
    pub(crate) fn from_validated(models: Vec<ModelDescriptor>) -> ModelCatalog {
        Self { models }
    }

    /// An empty catalog; every `models.bind` resolves as absent.
    #[must_use]
    pub fn empty() -> Self {
        Self::from_validated(Vec::new())
    }

    /// Returns every descriptor.
    #[must_use]
    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }

    /// Returns whether the catalog has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Looks up a descriptor by stable identity.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&ModelDescriptor> {
        self.models.iter().find(|model| model.id() == id)
    }

    /// Returns whether the catalog contains a descriptor with `id`.
    #[must_use]
    pub fn contains(&self, id: &ModelId) -> bool {
        self.get(id).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_c0_c1_and_other_control_scalars() {
        // The C0 record separator (U+001E) must never survive into an id.
        assert!(ModelId::new(ModelId::GATEWAY, "a\u{001e}b").is_err());
        // A C1 control (NEL, U+0085) whose UTF-8 bytes (0xC2 0x85) a byte-range
        // scan would miss but a scalar `is_control` scan rejects (MODEL-004).
        assert!(ModelId::new(ModelId::GATEWAY, "a\u{0085}b").is_err());
        // DEL (U+007F) and NUL are refused too.
        assert!(ModelId::new(ModelId::GATEWAY, "a\u{007f}b").is_err());
        assert!(ModelId::new("srv\u{0000}", "name").is_err());
        // A benign multi-byte non-ASCII name is still accepted.
        assert!(ModelId::new(ModelId::GATEWAY, "café-模型").is_ok());
    }

    #[test]
    fn model_id_rejects_empty_and_control_characters() {
        assert!(ModelId::gateway("").is_err());
        assert!(ModelId::new("", "name").is_err());
        assert!(ModelId::new("server", "").is_err());
        assert!(ModelId::new("server", "na\nme").is_err());
        assert!(ModelId::gateway("valid-alias").is_ok());
    }

    #[test]
    fn model_catalog_rejects_duplicate_ids() {
        let ctx = NonZeroU32::new(8_192).expect("test context window is non-zero");
        let descriptor = |name: &str| {
            ModelDescriptor::new(
                ModelId::gateway(name).expect("test model alias is valid"),
                "d",
                ctx,
                ThinkingMode::Never,
            )
        };
        let err = ModelCatalog::new([descriptor("dup"), descriptor("dup")])
            .expect_err("a catalog with duplicate ids must be rejected");
        assert!(matches!(err, ModelCatalogError::DuplicateId { .. }));
        assert!(ModelCatalog::new([descriptor("a"), descriptor("b")]).is_ok());
    }
}
