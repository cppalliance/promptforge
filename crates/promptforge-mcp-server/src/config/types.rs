//! The validated newtypes a [`Config`](super::Config) is built from.
//!
//! Each parses at the configuration boundary into a value that cannot hold an
//! invalid state, so nothing downstream re-checks a secret for blankness, a
//! gateway URL for a scheme, a prompt path for traversal, a glob for
//! compilation, or a block key for shape.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use url::Url;

use crate::error::ConfigError;
use crate::relpath::reject_traversal;

/// A secret string (the shared bearer or the gateway token) that never
/// serializes and redacts in both `Debug` and `Display`.
///
/// Constructed only through its `TryFrom`, so a `Secret` cannot hold a blank
/// value: a token or key that carries nothing usable is refused at the type
/// boundary rather than compared equal to a request presenting no credential
/// or sent to the gateway as an empty bearer. The config layer maps the
/// rejection onto the field that carried it, so the public `ConfigError`
/// surface is unchanged.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct Secret(String);

impl Secret {
    /// The secret's bytes. The one place a secret is read, when building auth.
    #[must_use]
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the secret is the empty string.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether the secret carries nothing usable: empty, or only whitespace.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl TryFrom<String> for Secret {
    type Error = ConfigError;

    fn try_from(value: String) -> Result<Secret, ConfigError> {
        if value.trim().is_empty() {
            return Err(ConfigError::parse("secret must not be empty"));
        }
        Ok(Secret(value))
    }
}

impl TryFrom<&str> for Secret {
    type Error = ConfigError;

    fn try_from(value: &str) -> Result<Secret, ConfigError> {
        Secret::try_from(value.to_string())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(redacted)")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("redacted")
    }
}

/// A validated gateway base URL.
///
/// Constructed only through its `TryFrom<String>`, so a `GatewayUrl` in a
/// [`Config`](super::Config) is always non-blank and carries an `http`/`https`
/// scheme with a host. Every run's model calls go through it, so a blank or
/// malformed value is refused at the parse boundary rather than surfacing later
/// as an opaque connection failure against the gateway.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct GatewayUrl(String);

impl GatewayUrl {
    /// The URL as the gateway client reads it.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GatewayUrl {
    type Error = ConfigError;

    fn try_from(value: String) -> Result<GatewayUrl, ConfigError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ConfigError::parse("[gateway].url must not be empty"));
        }
        // Parsed as a real URL rather than prefix-matched, so a value that
        // carries an http(s) scheme but is not a usable endpoint (no host, a
        // stray space, a malformed authority) is refused here rather than
        // reaching the gateway client as something it cannot use.
        let parsed = Url::parse(trimmed).map_err(|e| {
            ConfigError::parse(format!(
                "[gateway].url must be a valid http or https URL: {value:?}: {e}"
            ))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ConfigError::parse(format!(
                "[gateway].url must be an http or https URL: {value:?}"
            )));
        }
        if parsed.host_str().is_none_or(str::is_empty) {
            return Err(ConfigError::parse(format!(
                "[gateway].url must have a host: {value:?}"
            )));
        }
        Ok(GatewayUrl(value))
    }
}

impl fmt::Debug for GatewayUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GatewayUrl").field(&self.0).finish()
    }
}

/// A prompt path or catalog pattern that stays inside the prompts directory.
///
/// Constructed only through its `TryFrom`, which runs the shared shape check
/// [`reject_traversal`], so any [`RelativePromptPath`] a [`Config`](super::Config)
/// carries is relative and free of a `..` that could climb out of the root. The
/// filesystem-level [`confined`](crate::catalog) check still runs at
/// resolution, because a symlink escape is only visible once the path is
/// canonicalized.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "PathBuf")]
pub(crate) struct RelativePromptPath(PathBuf);

impl RelativePromptPath {
    /// The path as resolution joins it to the prompts directory.
    #[must_use]
    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

impl TryFrom<PathBuf> for RelativePromptPath {
    type Error = ConfigError;

    fn try_from(value: PathBuf) -> Result<RelativePromptPath, ConfigError> {
        reject_traversal(&value).map_err(|detail| {
            ConfigError::parse(format!("[prompts] file {}: {detail}", value.display()))
        })?;
        Ok(RelativePromptPath(value))
    }
}

impl fmt::Debug for RelativePromptPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RelativePromptPath").field(&self.0).finish()
    }
}

/// A validated catalog glob pattern.
///
/// Constructed only through its `TryFrom<String>`, which runs the shared shape
/// check [`reject_traversal`] and confirms the glob compiles, so a
/// [`GlobPattern`] in a [`Config`](super::Config) is a relative, well-formed
/// pattern rather than one that would climb out of the root or fail to parse
/// mid-resolution.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct GlobPattern(String);

impl GlobPattern {
    /// The pattern as resolution expands or compiles it.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for GlobPattern {
    type Error = ConfigError;

    fn try_from(value: String) -> Result<GlobPattern, ConfigError> {
        reject_traversal(Path::new(&value)).map_err(|detail| {
            ConfigError::parse(format!("[catalog] pattern {value:?}: {detail}"))
        })?;
        glob::Pattern::new(&value)
            .map_err(|e| ConfigError::parse(format!("[catalog] pattern {value:?}: {e}")))?;
        Ok(GlobPattern(value))
    }
}

impl fmt::Debug for GlobPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GlobPattern").field(&self.0).finish()
    }
}

/// A `[prompts.NAME]` block key: the frontmatter name the block is an exception
/// for.
///
/// Constructed only through its `TryFrom<String>`, which holds the key to the
/// same `^[a-z][a-z0-9_]{0,47}$` shape a published tool name must have and
/// refuses a reserved built-in name, so a block cannot be keyed on a name no
/// prompt could ever declare or on one a built-in already answers to.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct PromptName(String);

/// The longest a prompt name may be, per `^[a-z][a-z0-9_]{0,47}$`.
const MAX_PROMPT_NAME_LEN: usize = 48;

/// The built-in tool names a `[prompts.NAME]` block key may not reuse.
///
/// Duplicated here rather than referenced so this boundary check does not pull
/// the tool layer into the config module; it mirrors `RESERVED_NAMES` in
/// `catalog/resolve.rs` (which is keyed on the `crate::tools` name constants).
/// A block keyed on one of these would name a prompt a built-in already answers
/// to, which is ambiguous to a caller and to a model alike, so it is refused at
/// the parse boundary rather than at resolution.
const RESERVED_PROMPT_NAMES: [&str; 4] = ["list_prompts", "run_prompt", "check_run", "need_prompt"];

impl PromptName {
    /// The name as the catalog spells it.
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PromptName {
    type Error = ConfigError;

    fn try_from(value: String) -> Result<PromptName, ConfigError> {
        let mut chars = value.chars();
        let well_formed = chars.next().is_some_and(|first| first.is_ascii_lowercase())
            && value.len() <= MAX_PROMPT_NAME_LEN
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !well_formed {
            return Err(ConfigError::parse(format!(
                "[prompts.{value}] key is not ^[a-z][a-z0-9_]{{0,{}}}$",
                MAX_PROMPT_NAME_LEN - 1
            )));
        }
        if RESERVED_PROMPT_NAMES.contains(&value.as_str()) {
            return Err(ConfigError::parse(format!(
                "[prompts.{value}] key is reserved: a built-in tool already answers to it"
            )));
        }
        Ok(PromptName(value))
    }
}

impl std::borrow::Borrow<str> for PromptName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PromptName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PromptName").field(&self.0).finish()
    }
}
