//! Local-model companions: speculative-decoding drafters and multimodal
//! projectors attached to chat `[[local_model]]` entries.
//!
//! Companions are declarative configuration only: this module parses and
//! validates operator input and never touches the network or a process. A
//! companion source follows the same rule as the main model source: an
//! `https` URL pinned by SHA-256, or an operator-controlled local path that
//! may be unpinned. Plaintext `http` and empty sources are rejected, and
//! companions on a non-chat model kind fail validation.

use std::num::NonZeroU32;

use serde::{Deserialize, Serialize};

use super::{LocalModelConfig, is_sha256_hex, validate::validate_http_url};
use crate::error::ConfigError;

/// The maximum number of tokens a speculative drafter may propose per step
/// (`--spec-draft-n-max`).
///
/// Bounded to `1..=16`. The pinned llama.cpp server enforces no explicit
/// range on the argument (its default is 3: `common.h` sets
/// `common_params_speculative_draft::n_max = 3` at submodule commit
/// fb0e6b6), and the MTP implementation clamps the value to the drafter's
/// nextn layer count at runtime (`common/speculative.cpp`), so 16 is a
/// documented, generous ceiling rather than an upstream limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub struct DraftTokenMax(NonZeroU32);

impl DraftTokenMax {
    /// The largest accepted draft-token maximum.
    pub const MAX: u32 = 16;

    /// Bounds `value` to the supported range `1..=16`.
    ///
    /// # Errors
    /// Returns [`DraftTokenMaxError`] when `value` is zero or exceeds
    /// [`DraftTokenMax::MAX`].
    pub fn new(value: u32) -> Result<Self, DraftTokenMaxError> {
        let Some(inner) = NonZeroU32::new(value) else {
            return Err(DraftTokenMaxError { value });
        };
        if value > Self::MAX {
            return Err(DraftTokenMaxError { value });
        }
        Ok(Self(inner))
    }

    /// Returns the bounded value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl<'de> Deserialize<'de> for DraftTokenMax {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for DraftTokenMax {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u32(self.get())
    }
}

/// A draft-token maximum outside the supported range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "draft-token maximum {value} is outside the supported range 1..={}",
    DraftTokenMax::MAX
)]
#[non_exhaustive]
pub struct DraftTokenMaxError {
    value: u32,
}

impl DraftTokenMaxError {
    /// Returns the rejected value.
    #[must_use]
    pub const fn value(&self) -> u32 {
        self.value
    }
}

/// The speculation algorithm a drafter companion runs.
///
/// Only `draft-mtp` (multi-token prediction) is supported initially. The
/// serialized spelling matches the server's `--spec-type` vocabulary, so an
/// unknown type fails at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum SpeculationType {
    /// Multi-token-prediction drafter (`--spec-type draft-mtp`).
    DraftMtp,
}

/// A speculative-decoding drafter companion for a chat `[[local_model]]`.
///
/// Parsed from a `[local_model.speculative]` sub-table with a `type` (only
/// `draft-mtp` is supported), a `source`, a `sha256` pin when the source is
/// remote, and a `draft_max` in the supported llama.cpp range.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SpeculativeConfig {
    /// The speculation algorithm. Only `draft-mtp` is supported.
    #[serde(rename = "type")]
    kind: SpeculationType,
    /// Drafter GGUF source: an `https` URL or a local filesystem path.
    source: String,
    /// SHA-256 pin (lowercase hex); required when `source` is remote.
    #[serde(default)]
    sha256: Option<String>,
    /// Maximum tokens drafted per step (`--spec-draft-n-max`).
    draft_max: DraftTokenMax,
}

impl SpeculativeConfig {
    /// Returns the speculation algorithm the drafter runs.
    #[must_use]
    pub const fn kind(&self) -> SpeculationType {
        self.kind
    }

    /// Returns the drafter source: an `https` URL or a local filesystem
    /// path.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the SHA-256 pin (lowercase hex) verified after download, when
    /// set. Always set for a remote source.
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// Returns the maximum number of tokens drafted per step
    /// (`--spec-draft-n-max`).
    #[must_use]
    pub const fn draft_max(&self) -> DraftTokenMax {
        self.draft_max
    }

    /// Checks the companion source rules for the model named `model_name`.
    pub(crate) fn validate(&self, model_name: &str) -> Result<(), ConfigError> {
        validate_artifact_source(
            &format!("local_model {model_name}"),
            "speculative.source",
            &self.source,
            self.sha256.as_deref(),
        )
    }
}

/// A multimodal projector companion for a chat `[[local_model]]`
/// (`--mmproj`).
///
/// Parsed from a `[local_model.multimodal_projector]` sub-table with a
/// `source` and a `sha256` pin when the source is remote.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct MultimodalProjectorConfig {
    /// Projector GGUF source: an `https` URL or a local filesystem path.
    source: String,
    /// SHA-256 pin (lowercase hex); required when `source` is remote.
    #[serde(default)]
    sha256: Option<String>,
}

impl MultimodalProjectorConfig {
    /// Returns the projector source: an `https` URL or a local filesystem
    /// path.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the SHA-256 pin (lowercase hex) verified after download, when
    /// set. Always set for a remote source.
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// Checks the companion source rules for the model named `model_name`.
    pub(crate) fn validate(&self, model_name: &str) -> Result<(), ConfigError> {
        validate_artifact_source(
            &format!("local_model {model_name}"),
            "multimodal_projector.source",
            &self.source,
            self.sha256.as_deref(),
        )
    }
}

impl LocalModelConfig {
    /// Returns the speculative-decoding drafter companion
    /// (`[local_model.speculative]`), when set. Chat kind only.
    #[must_use]
    pub const fn speculative(&self) -> Option<&SpeculativeConfig> {
        self.speculative.as_ref()
    }

    /// Returns the multimodal projector companion
    /// (`[local_model.multimodal_projector]`), when set. Chat kind only.
    #[must_use]
    pub const fn multimodal_projector(&self) -> Option<&MultimodalProjectorConfig> {
        self.multimodal_projector.as_ref()
    }
}

/// The shared artifact-source gate: non-empty, `https`-or-local, remote
/// pinned.
///
/// `label` scopes the diagnostic (for example `local_model gemma-4`) and
/// `field` names the offending key (for example `source` or
/// `speculative.source`). A local filesystem source is operator-controlled
/// and may be unpinned; a remote artifact must be pinned by digest
/// (ART-002).
pub(crate) fn validate_artifact_source(
    label: &str,
    field: &str,
    source: &str,
    sha256: Option<&str>,
) -> Result<(), ConfigError> {
    if source.is_empty() {
        return Err(ConfigError::Validation(format!(
            "{label} {field} must not be empty"
        )));
    }
    if source.starts_with("http://") {
        return Err(ConfigError::Validation(format!(
            "{label} {field} must use https, not plaintext http"
        )));
    }
    if source.starts_with("https://") {
        validate_http_url(&format!("{label} {field}"), source)?;
        if sha256.is_none() {
            return Err(ConfigError::Validation(format!(
                "{label} {field} is remote and must set a sha256 pin"
            )));
        }
    }
    if let Some(sha) = sha256
        && !is_sha256_hex(sha)
    {
        return Err(ConfigError::Validation(format!(
            "{label} {field} sha256 must be 64 lowercase hex characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "companion-tests.rs"]
mod tests;
