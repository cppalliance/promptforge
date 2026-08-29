//! Stable model identity and the catalog/identity validation errors.

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
    /// use promptforge_gateway_client::model::ModelId;
    ///
    /// let id = ModelId::new(ModelId::GATEWAY, "claude-sonnet-4-6")?;
    /// assert_eq!(id.server(), "gateway");
    /// assert_eq!(id.name(), "claude-sonnet-4-6");
    /// # Ok::<(), promptforge_gateway_client::model::ModelIdError>(())
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

    /// The `RS` (U+001E) record separator the model picker uses to delimit
    /// encoded identities. Accepting it inside a component would let an id
    /// collide or corrupt that encoding, so it is rejected explicitly.
    pub(crate) const PICKER_SEPARATOR: char = '\u{001e}';

    /// Validates one identity component, naming the field in any error.
    ///
    /// Rejection is by Unicode scalar, not raw byte (MODEL-004): every control
    /// character is refused, including C1 controls such as U+0085 (NEL) whose
    /// UTF-8 encoding a byte-range scan would miss, and the picker separator
    /// U+001E in particular.
    fn validate(field: &'static str, value: &str) -> std::result::Result<(), ModelIdError> {
        if value.is_empty() {
            return Err(ModelIdError {
                field,
                reason: "must not be empty",
            });
        }
        if value
            .chars()
            .any(|c| c.is_control() || c == Self::PICKER_SEPARATOR)
        {
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

/// The reason a [`crate::model::ModelCatalog`] could not be built from its
/// descriptors.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_c0_c1_and_picker_separator_controls() {
        // The picker record separator (U+001E) must never survive into an id.
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
}
