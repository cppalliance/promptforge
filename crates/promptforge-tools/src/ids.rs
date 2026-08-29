//! Stable tool identity and its validation errors.

/// The stable identity of a live tool.
///
/// Identity is structural over the server and tool name. The wire name used
/// in a model request is deliberately not identity: later capability binding
/// can advertise a selected tool under a prompt-local alias without changing
/// the live tool it dispatches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ToolId {
    server: String,
    name: String,
}

impl ToolId {
    /// Builds an identity from its server and stable tool name.
    ///
    /// # Errors
    /// Returns [`ToolIdError`] if `server` or `name` is empty or contains the
    /// `/` namespace separator or a control character.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch")?;
    /// assert_eq!(id.server(), "promptforge");
    /// assert_eq!(id.name(), "web_fetch");
    /// # Ok::<(), promptforge_tools::ToolIdError>(())
    /// ```
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Result<ToolId, ToolIdError> {
        let server = server.into();
        let name = name.into();
        Self::validate("server", &server)?;
        Self::validate("name", &name)?;
        Ok(Self { server, name })
    }

    /// Builds an identity from components already known to be valid.
    ///
    /// For internal callers whose inputs are static tool names or come from an
    /// existing [`ToolId`], so the validation in [`ToolId::new`] is redundant.
    /// Hidden from the public API: downstream callers use [`ToolId::new`].
    #[doc(hidden)]
    #[must_use]
    pub fn from_validated(server: impl Into<String>, name: impl Into<String>) -> ToolId {
        ToolId {
            server: server.into(),
            name: name.into(),
        }
    }

    /// Validates one identity component, naming the field in any error.
    fn validate(field: &'static str, value: &str) -> Result<(), ToolIdError> {
        validate_identifier(field, value)
    }

    /// Returns the server that owns this identity namespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch")?;
    /// assert_eq!(id.server(), "promptforge");
    /// # Ok::<(), promptforge_tools::ToolIdError>(())
    /// ```
    #[must_use]
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Returns the stable name within the publisher's namespace.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_tools::ToolId;
    ///
    /// let id = ToolId::new("promptforge", "web_fetch")?;
    /// assert_eq!(id.name(), "web_fetch");
    /// # Ok::<(), promptforge_tools::ToolIdError>(())
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}
/// A stable, matchable classification of a [`ToolIdError`].
///
/// Every public error exposes a `kind()` classifier so callers can branch on the
/// failure without matching a private representation (DESIGN-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolIdErrorKind {
    /// A component was empty.
    Empty,
    /// A component contained the `/` namespace separator.
    Separator,
    /// A component contained a control character.
    Control,
}

/// The reason a [`ToolId`] (or a validated wire name) could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tool {field}: {reason}")]
#[non_exhaustive]
pub struct ToolIdError {
    /// Which component was rejected (`server`, `name`, or `wire name`).
    field: &'static str,
    /// A stable classification of why it was rejected.
    kind: ToolIdErrorKind,
    /// A human-readable reason.
    reason: &'static str,
}

impl ToolIdError {
    /// Returns the stable classification of this error (DESIGN-5).
    #[must_use]
    pub fn kind(&self) -> ToolIdErrorKind {
        self.kind
    }

    /// Returns which component was rejected (`server`, `name`, or `wire name`).
    #[must_use]
    pub fn field(&self) -> &str {
        self.field
    }

    /// The crate-internal human-readable reason, reused when a wire-name
    /// rejection is re-reported as a [`crate::ToolCatalogError`].
    pub(crate) fn reason(&self) -> &'static str {
        self.reason
    }
}

/// Validates one identity-shaped component (server/name/wire name).
///
/// A component must be non-empty and free of the `/` namespace separator and any
/// control character. Shared so [`ToolId`] components and tool wire names are
/// held to one rule set (tools.rs F4).
pub(crate) fn validate_identifier(field: &'static str, value: &str) -> Result<(), ToolIdError> {
    if value.is_empty() {
        return Err(ToolIdError {
            field,
            kind: ToolIdErrorKind::Empty,
            reason: "must not be empty",
        });
    }
    if value.contains('/') {
        return Err(ToolIdError {
            field,
            kind: ToolIdErrorKind::Separator,
            reason: "must not contain the '/' separator",
        });
    }
    if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(ToolIdError {
            field,
            kind: ToolIdErrorKind::Control,
            reason: "must not contain a control character",
        });
    }
    Ok(())
}
