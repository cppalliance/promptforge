//! Public model vocabulary: stable identity, catalog, and descriptor.
//!
//! The caller supplies a [`ModelCatalog`], and catalog entries are named by
//! their validated [`ModelId`]. These types are the shared vocabulary every
//! promptforge crate and every caller may name, with no transport, binding,
//! or invocation machinery.

use std::num::NonZeroU32;

use serde::Deserialize;

/// The stable identity of one catalogued model.
///
/// An identity pairs a server namespace with a model name. A model the
/// gateway serves uses the namespace `"gateway"` and the name the gateway
/// lists it under. That name is the `name` key of the model's entry in the
/// gateway configuration and the `id` field in the OpenAI-compatible model
/// list.
///
/// Both parts are always non-empty and free of control characters.
/// [`ModelId::new`] and [`ModelId::gateway`] reject any other value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ModelId {
    server: String,
    name: String,
}

impl ModelId {
    /// The namespace for models the gateway serves.
    pub const GATEWAY: &'static str = "gateway";

    /// Builds an identity from its server namespace and model name.
    ///
    /// # Errors
    /// Returns [`ModelIdError`] if `server` or `name` is empty or contains a
    /// control character, so every `ModelId` is usable.
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

    /// Builds an identity in the `"gateway"` namespace from the name the
    /// gateway lists the model under.
    ///
    /// # Errors
    /// Returns [`ModelIdError`] if `name` is empty or contains a control
    /// character.
    pub fn gateway(name: impl Into<String>) -> std::result::Result<ModelId, ModelIdError> {
        Self::new(Self::GATEWAY, name)
    }

    /// Builds an identity from components already known to be valid.
    ///
    /// Crate-internal: backs [`crate::detail::model_id_from_validated`].
    pub(crate) fn from_validated(server: impl Into<String>, name: impl Into<String>) -> ModelId {
        ModelId {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Validates one identity component, naming the field in any error.
    ///
    /// Rejection is by Unicode scalar, not raw byte: every control
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

    /// Returns the server namespace.
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the model name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// The reason building a [`ModelId`] from its components failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid model id: {field} {reason}")]
#[non_exhaustive]
pub struct ModelIdError {
    /// Which component was rejected (`server` or `name`).
    field: &'static str,
    /// Why it was rejected.
    reason: &'static str,
}

/// The reason building a [`ModelCatalog`] from its descriptors failed.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum ThinkingMode {
    /// The backend always runs with thinking off.
    Never,
    /// The backend always emits thinking tokens.
    Always,
    /// The client may turn thinking on or off per request.
    Switchable,
}

/// One catalogued model: its identity, description, context window, and
/// thinking mode.
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
    /// The context window is a [`NonZeroU32`], so it always holds at least
    /// one token.
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

    /// Returns the prose that describes the model.
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

/// A list of available models, each with a distinct identity.
///
/// [`ModelCatalog::new`] rejects a repeated `ModelId`, and
/// [`ModelCatalog::empty`] returns an empty catalog.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ModelCatalog {
    models: Vec<ModelDescriptor>,
}

impl ModelCatalog {
    /// Builds a catalog from descriptors, keeping the order they are given
    /// in.
    ///
    /// # Errors
    /// Returns [`ModelCatalogError::DuplicateId`] when two descriptors share one
    /// stable [`ModelId`], so each identity in the catalog names exactly one
    /// descriptor.
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

    /// Returns an empty catalog.
    #[must_use]
    pub fn empty() -> Self {
        Self { models: Vec::new() }
    }

    /// Returns every descriptor.
    #[must_use]
    pub fn models(&self) -> &[ModelDescriptor] {
        &self.models
    }

    /// Returns whether the catalog is empty.
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
        // scan would miss but a scalar `is_control` scan rejects.
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
    fn a_model_id_exposes_its_server_and_name() {
        let id = ModelId::new(ModelId::GATEWAY, "claude-sonnet-4-6").expect("a valid model id");
        assert_eq!(id.server(), "gateway");
        assert_eq!(id.name(), "claude-sonnet-4-6");
    }

    #[test]
    fn a_thinking_mode_deserializes_from_its_lowercase_wire_form() {
        let mode: ThinkingMode =
            serde_json::from_str("\"switchable\"").expect("a thinking mode deserializes");
        assert_eq!(mode, ThinkingMode::Switchable);
    }

    #[test]
    fn a_model_descriptor_keeps_its_context_window_and_thinking_mode() {
        let context = NonZeroU32::new(131_072).expect("test context window is non-zero");
        let model = ModelDescriptor::new(
            ModelId::gateway("analyst").expect("test model alias is valid"),
            "A careful analysis model",
            context,
            ThinkingMode::Switchable,
        );
        assert_eq!(model.context(), context);
        assert_eq!(model.thinking(), ThinkingMode::Switchable);
    }

    #[test]
    fn a_model_catalog_contains_each_descriptor_it_was_built_from() {
        let ctx = NonZeroU32::new(8_192).expect("test context window is non-zero");
        let id = ModelId::gateway("small").expect("test model alias is valid");
        let catalog = ModelCatalog::new([ModelDescriptor::new(
            id.clone(),
            "A tiny model",
            ctx,
            ThinkingMode::Never,
        )])
        .expect("a catalog with one descriptor builds");
        assert!(catalog.contains(&id));
        assert_eq!(catalog.models().len(), 1);
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
