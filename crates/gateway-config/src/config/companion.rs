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
///
/// # Examples
/// ```
/// use gateway_config::DraftTokenMax;
///
/// let max = DraftTokenMax::new(2)?;
/// assert_eq!(max.get(), 2);
/// assert!(DraftTokenMax::new(0).is_err());
/// assert!(DraftTokenMax::new(17).is_err());
/// # Ok::<(), gateway_config::DraftTokenMaxError>(())
/// ```
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
///
/// # Examples
/// ```
/// use gateway_config::{Config, SpeculationType};
///
/// let digest = "9eba819938efccfd6044f8af84e3bbfddc639a2bcf32ebc36420e6a649191919";
/// let toml = format!(r#"
/// config-version = 2
/// [server]
/// bind = "127.0.0.1:8080"
/// api_key = "secret"
///
/// [[local_model]]
/// name = "gemma-4"
/// description = "a local model"
/// source = "/models/gemma-4-E2B-it-UD-Q4_K_XL.gguf"
/// context = 131072
///
/// [local_model.speculative]
/// type = "draft-mtp"
/// source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mtp-gemma-4-E2B-it.gguf"
/// sha256 = "{digest}"
/// draft_max = 2
/// "#);
/// let config = Config::from_toml_str(&toml)?;
/// let speculative = config.local_models()[0]
///     .speculative()
///     .ok_or("missing speculative companion")?;
/// assert_eq!(speculative.kind(), SpeculationType::DraftMtp);
/// assert_eq!(speculative.draft_max().get(), 2);
/// assert_eq!(speculative.sha256(), Some(digest));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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

    /// Check the companion source rules for the model named `model_name`.
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
///
/// # Examples
/// ```
/// use gateway_config::Config;
///
/// let digest = "140be8d7849741f88c50757d529b84373ee8e27052cc2236855b537f4a8215fa";
/// let toml = format!(r#"
/// config-version = 2
/// [server]
/// bind = "127.0.0.1:8080"
/// api_key = "secret"
///
/// [[local_model]]
/// name = "gemma-4"
/// description = "a local model"
/// source = "/models/gemma-4-E2B-it-UD-Q4_K_XL.gguf"
/// context = 131072
///
/// [local_model.multimodal_projector]
/// source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mmproj-F16.gguf"
/// sha256 = "{digest}"
/// "#);
/// let config = Config::from_toml_str(&toml)?;
/// let projector = config.local_models()[0]
///     .multimodal_projector()
///     .ok_or("missing projector companion")?;
/// assert_eq!(projector.sha256(), Some(digest));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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

    /// Check the companion source rules for the model named `model_name`.
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
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.local_models()[0].speculative().is_none());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub const fn speculative(&self) -> Option<&SpeculativeConfig> {
        self.speculative.as_ref()
    }

    /// Returns the multimodal projector companion
    /// (`[local_model.multimodal_projector]`), when set. Chat kind only.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let toml = r#"
    /// # config-version = 2
    /// # [server]
    /// # bind = "127.0.0.1:8080"
    /// # api_key = "secret"
    /// #
    /// # [[local_model]]
    /// # name = "q"
    /// # description = "a local model"
    /// # source = "/models/q.gguf"
    /// # context = 4096
    /// # "#;
    /// let config = Config::from_toml_str(toml)?;
    /// assert!(config.local_models()[0].multimodal_projector().is_none());
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
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
mod tests {
    use super::*;
    use crate::config::Config;

    const HEADER: &str = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"
"#;

    const DIGEST: &str = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32";

    fn entry(body: &str) -> String {
        format!(
            "{HEADER}\n[[local_model]]\nname = \"q\"\ndescription = \"a local model\"\nsource = \"/models/q.gguf\"\ncontext = 4096\n{body}"
        )
    }

    fn parse(body: &str) -> Result<Config, crate::api_error::ConfigError> {
        Config::from_toml_str(&entry(body))
    }

    #[test]
    fn parses_remote_companions_with_pins() {
        let config = parse(&format!(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2

[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
        ))
        .unwrap();
        let model = &config.local_models()[0];
        let speculative = model.speculative().unwrap();
        assert_eq!(speculative.kind(), SpeculationType::DraftMtp);
        assert_eq!(speculative.source(), "https://example.com/q-mtp.gguf");
        assert_eq!(speculative.sha256(), Some(DIGEST));
        assert_eq!(speculative.draft_max().get(), 2);
        let projector = model.multimodal_projector().unwrap();
        assert_eq!(projector.source(), "https://example.com/q-mmproj.gguf");
        assert_eq!(projector.sha256(), Some(DIGEST));
    }

    #[test]
    fn projector_implies_images_capability() {
        let config = parse(
            r#"
[local_model.multimodal_projector]
source = "/models/q-mmproj.gguf"
"#,
        )
        .unwrap();
        assert!(config.local_models()[0].capabilities().images());
    }

    #[test]
    fn selected_profile_keeps_projector_images_capability() {
        let config = parse(
            r#"
[local_model.multimodal_projector]
source = "/models/q-mmproj.gguf"

[[profile]]
name = "work"
models = ["q"]
"#,
        )
        .unwrap();
        let selected = config
            .select_profile(&crate::ProfileName::parse("work").unwrap())
            .unwrap();
        assert!(selected.local_models()[0].capabilities().images());
    }

    #[test]
    fn no_projector_keeps_images_default() {
        let config = parse("").unwrap();
        assert!(!config.local_models()[0].capabilities().images());
    }

    #[test]
    fn rejects_unknown_speculation_type() {
        let result = parse(&format!(
            r#"
[local_model.speculative]
type = "draft-eagle3"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2
"#
        ));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_speculative_on_non_chat_kind() {
        let result = parse(&format!(
            r#"kind = "embedding"

[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2
"#
        ));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_projector_on_non_chat_kind() {
        let result = parse(&format!(
            r#"kind = "classifier"

[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
        ));
        assert!(result.is_err());
    }

    #[test]
    fn rejects_remote_speculative_without_pin() {
        let result = parse(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
draft_max = 2
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_remote_projector_without_pin() {
        let result = parse(
            r#"
[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_http_companion_sources() {
        let speculative = parse(&format!(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "http://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2
"#
        ));
        assert!(speculative.is_err());
        let projector = parse(&format!(
            r#"
[local_model.multimodal_projector]
source = "http://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
        ));
        assert!(projector.is_err());
    }

    #[test]
    fn rejects_empty_companion_sources() {
        let speculative = parse(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = ""
draft_max = 2
"#,
        );
        assert!(speculative.is_err());
        let projector = parse(
            r#"
[local_model.multimodal_projector]
source = ""
"#,
        );
        assert!(projector.is_err());
    }

    #[test]
    fn rejects_malformed_companion_pin() {
        let result = parse(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "/models/q-mtp.gguf"
sha256 = "not-hex"
draft_max = 2
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_out_of_range_draft_max() {
        for draft_max in [0, 17] {
            let result = parse(&format!(
                r#"
[local_model.speculative]
type = "draft-mtp"
source = "/models/q-mtp.gguf"
draft_max = {draft_max}
"#
            ));
            assert!(result.is_err(), "draft_max {draft_max} must be rejected");
        }
    }

    #[test]
    fn accepts_local_path_companions_without_pins() {
        let config = parse(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "/models/q-mtp.gguf"
draft_max = 1

[local_model.multimodal_projector]
source = "/models/q-mmproj.gguf"
"#,
        )
        .unwrap();
        let model = &config.local_models()[0];
        assert_eq!(model.speculative().unwrap().sha256(), None);
        assert_eq!(model.speculative().unwrap().draft_max().get(), 1);
        assert!(model.multimodal_projector().is_some());
    }

    #[test]
    fn defaults_to_no_companions() {
        let config = parse("").unwrap();
        let model = &config.local_models()[0];
        assert!(model.speculative().is_none());
        assert!(model.multimodal_projector().is_none());
    }

    #[test]
    fn whole_entry_replacement_round_trips() {
        // The rollout replacement entry: companions included end to end.
        let replaced = parse(&format!(
            r#"
[local_model.speculative]
type = "draft-mtp"
source = "https://example.com/q-mtp.gguf"
sha256 = "{DIGEST}"
draft_max = 2

[local_model.multimodal_projector]
source = "https://example.com/q-mmproj.gguf"
sha256 = "{DIGEST}"
"#
        ))
        .unwrap();
        assert!(replaced.local_models()[0].speculative().is_some());
        // The pre-replacement entry, written before companions existed, still
        // parses with both companions absent.
        let legacy = parse("").unwrap();
        let model = &legacy.local_models()[0];
        assert!(model.speculative().is_none());
        assert!(model.multimodal_projector().is_none());
    }

    #[test]
    fn draft_token_max_bounds() {
        assert_eq!(DraftTokenMax::new(1).unwrap().get(), 1);
        assert_eq!(DraftTokenMax::new(DraftTokenMax::MAX).unwrap().get(), 16);
        assert_eq!(DraftTokenMax::new(0).unwrap_err().value(), 0);
        assert_eq!(DraftTokenMax::new(17).unwrap_err().value(), 17);
    }

    #[test]
    fn draft_token_max_serializes_as_a_plain_integer() {
        let max = DraftTokenMax::new(7).unwrap();
        let json = serde_json::to_value(max).unwrap();
        assert_eq!(json, 7);
        let back: DraftTokenMax = serde_json::from_value(json).unwrap();
        assert_eq!(back, max);
    }
}
