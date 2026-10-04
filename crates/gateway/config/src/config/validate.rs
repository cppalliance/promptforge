//! Semantic validation of a parsed [`Config`].
//!
//! A `Config` value cannot hold an invalid state: construction runs [`Config::validate`],
//! which rejects empty ids, unresolved references, kind-incompatible dominion
//! payloads, chat-only fields on non-chat model kinds, malformed HTTP(S) URLs,
//! out-of-vocabulary web-search knobs, and VRAM over-booking of a local
//! dominion. Downstream code therefore never re-validates or clamps operator
//! input.

use std::collections::HashSet;

use url::Url;

use super::{Capabilities, Config, DominionKind, ModelKind, ThinkingMode};
use crate::error::ConfigError;

mod models;
mod profiles;
#[cfg(test)]
mod tests;

impl Config {
    /// Advertises `images = true` for every local model with a multimodal
    /// projector.
    ///
    /// A configured `[local_model.multimodal_projector]` makes the child
    /// image-capable (`--mmproj`), so the catalog must not report the
    /// `images` default of false. Runs before [`Self::validate`], so
    /// downstream code reads the resolved
    /// capability verbatim. The flag is a plain `bool`, so an explicit
    /// `images = false` cannot be told apart from an absent one; the
    /// projector wins either way because the model does accept images.
    pub(crate) fn imply_projector_images(&mut self) {
        for models in [&mut self.local_models, &mut self.catalog_local_models] {
            for local_model in models {
                if local_model.multimodal_projector.is_some() {
                    local_model.capabilities.images = true;
                }
            }
        }
    }

    /// Checks names are unique, references resolve, URLs parse, and closed
    /// vocabularies hold.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] on any failed invariant: an empty or
    /// duplicate id, a model with no or duplicate endpoints, a model naming an
    /// undefined endpoint, a malformed endpoint or web-search URL, an
    /// out-of-vocabulary freshness/safesearch default, an invalid
    /// `[[local_model]]`, a `parallel` below 1, a chat-only field set on a
    /// non-chat model kind, or a `[[dominion]]` violation
    /// (duplicate or empty id, `max_concurrency` or `max_queue` below 1,
    /// `vram_gb` on a remote dominion, a binding to an undefined or
    /// wrong-kind dominion, or a VRAM co-residency failure: a local
    /// dominion's `vram_gb` budget exceeded by the bound models' estimates,
    /// or a bound model with no estimate).
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.version != 0 {
            return Err(ConfigError::Validation(format!(
                "config-version must be 0, got {}",
                self.version
            )));
        }
        if self.server.api_key.is_empty() {
            return Err(ConfigError::Validation(
                "server.key must not be empty".to_string(),
            ));
        }
        self.validate_dominions()?;
        let endpoint_ids = self.validate_endpoints()?;
        self.validate_models(&endpoint_ids)?;
        self.validate_stt_models()?;
        self.validate_profiles()?;
        self.validate_tools()?;
        Ok(())
    }

    /// Validates `[tools.web_search]` bounds, URL, and closed knobs at load so
    /// downstream code never has to clamp or re-parse operator input (CFG-006).
    fn validate_tools(&self) -> Result<(), ConfigError> {
        let Some(web_search) = self.web_search_config() else {
            return Ok(());
        };
        if web_search.default_count < 1 {
            return Err(ConfigError::Validation(
                "tools.web_search.default_count must be at least 1".to_string(),
            ));
        }
        if web_search.max_count < 1 {
            return Err(ConfigError::Validation(
                "tools.web_search.max_count must be at least 1".to_string(),
            ));
        }
        if web_search.default_count > web_search.max_count {
            return Err(ConfigError::Validation(
                "tools.web_search.default_count must not exceed max_count".to_string(),
            ));
        }
        if web_search.max_per_host < 1 {
            return Err(ConfigError::Validation(
                "tools.web_search.max_per_host must be at least 1".to_string(),
            ));
        }
        // Parse the base URL, don't just prefix-match it (CFG-006).
        validate_http_url("tools.web_search.base_url", web_search.base_url.trim())?;
        if !is_valid_freshness(&web_search.default_freshness) {
            return Err(ConfigError::Validation(format!(
                "tools.web_search.default_freshness {:?} is not one of pd/pw/pm/py, a \
                 YYYY-MM-DDtoYYYY-MM-DD range, or empty",
                web_search.default_freshness
            )));
        }
        if !is_valid_safesearch(&web_search.default_safesearch) {
            return Err(ConfigError::Validation(format!(
                "tools.web_search.default_safesearch {:?} is not off/moderate/strict or empty",
                web_search.default_safesearch
            )));
        }
        Ok(())
    }

    fn validate_dominions(&self) -> Result<(), ConfigError> {
        let mut dominion_ids = HashSet::new();
        for dominion in &self.dominions {
            if dominion.id.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "dominion id must not be empty".to_string(),
                ));
            }
            if !dominion_ids.insert(dominion.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate dominion id {}",
                    dominion.id
                )));
            }
            if let Some(max_concurrency) = dominion.max_concurrency
                && max_concurrency < 1
            {
                return Err(ConfigError::Validation(format!(
                    "dominion {} max_concurrency must be at least 1",
                    dominion.id
                )));
            }
            if dominion.max_queue < 1 {
                return Err(ConfigError::Validation(format!(
                    "dominion {} max_queue must be at least 1",
                    dominion.id
                )));
            }
            // Kind-incompatible payloads are rejected, same spirit as
            // CFG-004: a VRAM budget is meaningful only for a local GPU.
            if dominion.kind == DominionKind::Remote && dominion.vram_gb.is_some() {
                return Err(ConfigError::Validation(format!(
                    "remote dominion {} must not set vram_gb",
                    dominion.id
                )));
            }
        }
        Ok(())
    }

    fn validate_endpoints(&self) -> Result<HashSet<&str>, ConfigError> {
        let mut endpoint_ids = HashSet::new();
        for endpoint in &self.endpoints {
            // A blank id can never be referenced by a model and silently
            // shadows the "unnamed" slot; reject it at the boundary (CFG-003).
            if endpoint.id.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "endpoint id must not be empty".to_string(),
                ));
            }
            if !endpoint_ids.insert(endpoint.id.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate endpoint id {}",
                    endpoint.id
                )));
            }
            // Parse and validate the base URL at load; the upstream adapter then
            // joins the request path onto a known-good origin instead of
            // concatenating an arbitrary string (CFG-003, UP-005).
            validate_http_url(
                &format!("endpoint {} base_url", endpoint.id),
                endpoint.base_url.trim(),
            )?;
            if let Some(dominion_id) = &endpoint.dominion {
                let dominion = self.dominions.iter().find(|d| d.id == *dominion_id);
                let Some(dominion) = dominion else {
                    return Err(ConfigError::Validation(format!(
                        "endpoint {} names undefined dominion {dominion_id}",
                        endpoint.id
                    )));
                };
                if dominion.kind != DominionKind::Remote {
                    return Err(ConfigError::Validation(format!(
                        "endpoint {} references non-remote dominion {dominion_id}",
                        endpoint.id
                    )));
                }
            }
        }
        Ok(endpoint_ids)
    }
}

/// Validates the capability metadata of one model entry.
///
/// `default_effort` requires a non-empty `effort_levels` and must name a
/// listed level; the effort knobs are meaningless on a model that never
/// thinks; `max_output` must fit the context window; and `voices` entries
/// must be non-empty and unique.
fn validate_capabilities(
    label: &str,
    name: &str,
    context: u32,
    thinking: ThinkingMode,
    capabilities: &Capabilities,
) -> Result<(), ConfigError> {
    if let Some(default_effort) = &capabilities.default_effort {
        if capabilities.effort_levels.is_empty() {
            return Err(ConfigError::Validation(format!(
                "{label} {name} default_effort requires a non-empty effort_levels"
            )));
        }
        if !capabilities.effort_levels.contains(default_effort) {
            return Err(ConfigError::Validation(format!(
                "{label} {name} default_effort {default_effort:?} is not in effort_levels"
            )));
        }
    }
    if thinking == ThinkingMode::Never
        && (!capabilities.effort_levels.is_empty() || capabilities.default_effort.is_some())
    {
        return Err(ConfigError::Validation(format!(
            "{label} {name} must not set effort fields when thinking is never"
        )));
    }
    if let Some(max_output) = capabilities.max_output
        && max_output > context
    {
        return Err(ConfigError::Validation(format!(
            "{label} {name} max_output {max_output} exceeds context {context}"
        )));
    }
    let mut seen_voices = HashSet::new();
    for voice in &capabilities.voices {
        if voice.is_empty() {
            return Err(ConfigError::Validation(format!(
                "{label} {name} voices entries must not be empty"
            )));
        }
        if !seen_voices.insert(voice.as_str()) {
            return Err(ConfigError::Validation(format!(
                "{label} {name} lists duplicate voice {voice}"
            )));
        }
    }
    Ok(())
}

/// Rejects chat-only fields on a non-chat model kind and the speech-only
/// `voices` list on a non-speech kind.
///
/// `thinking` and the capability effort knobs (`effort_levels`,
/// `default_effort`, `adaptive_thinking`) are chat-only on every model type;
/// `extra` lists each model type's remaining chat-only fields as `(field,
/// is_set)` pairs. `context` applies to every kind and is never rejected
/// here. A chat model has the default kind and passes unconditionally.
fn validate_kind_scope(
    label: &str,
    name: &str,
    kind: ModelKind,
    thinking: ThinkingMode,
    capabilities: &Capabilities,
    extra: &[(&str, bool)],
) -> Result<(), ConfigError> {
    if kind != ModelKind::Speech && !capabilities.voices.is_empty() {
        return Err(ConfigError::Validation(format!(
            "{kind} {label} {name} must not set voices (speech-only)"
        )));
    }
    if kind == ModelKind::Chat {
        return Ok(());
    }
    if thinking != ThinkingMode::Never {
        return Err(ConfigError::Validation(format!(
            "{kind} {label} {name} must not set thinking (chat-only)"
        )));
    }
    for (field, is_set) in [
        ("effort_levels", !capabilities.effort_levels.is_empty()),
        ("default_effort", capabilities.default_effort.is_some()),
        ("adaptive_thinking", capabilities.adaptive_thinking),
    ]
    .into_iter()
    .chain(extra.iter().copied())
    {
        if is_set {
            return Err(ConfigError::Validation(format!(
                "{kind} {label} {name} must not set {field} (chat-only)"
            )));
        }
    }
    Ok(())
}

/// Parses `raw` and requires an `http`/`https` scheme with a non-empty host.
///
/// This is the single URL gate for operator-supplied origins: a value that
/// passes here is a real, absolute HTTP(S) URL, so adapters can join a path
/// onto it structurally rather than concatenating an unvalidated string.
pub(super) fn validate_http_url(context: &str, raw: &str) -> Result<(), ConfigError> {
    let url = Url::parse(raw).map_err(|error| {
        ConfigError::Validation(format!("{context} is not a valid URL: {error}"))
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigError::Validation(format!(
            "{context} must use http or https, got {:?}",
            url.scheme()
        )));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(ConfigError::Validation(format!(
            "{context} must include a host"
        )));
    }
    Ok(())
}

/// Whether `value` is an accepted Brave freshness knob: empty (omit), one of
/// `pd`/`pw`/`pm`/`py`, or a `YYYY-MM-DDtoYYYY-MM-DD` date range.
fn is_valid_freshness(value: &str) -> bool {
    if value.is_empty() || matches!(value, "pd" | "pw" | "pm" | "py") {
        return true;
    }
    value
        .split_once("to")
        .is_some_and(|(from, to)| is_iso_date(from) && is_iso_date(to))
}

/// Whether `value` is `YYYY-MM-DD` (digits and dashes in the right positions).
fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
}

/// Whether `value` is an accepted safesearch knob: empty (omit), `off`,
/// `moderate`, or `strict`.
fn is_valid_safesearch(value: &str) -> bool {
    matches!(value, "" | "off" | "moderate" | "strict")
}
