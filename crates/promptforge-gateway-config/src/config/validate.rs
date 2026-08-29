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

use super::companion::validate_artifact_source;
use super::{Capabilities, Config, DominionKind, ModelKind, ThinkingMode, ToolDialect};
use crate::error::ConfigError;

impl Config {
    /// Filter the merged catalog to the profile's `models` allowlist.
    ///
    /// Runs after include-merge and before [`Config::validate`], so reference
    /// validation and the VRAM co-residency check operate on the loaded set
    /// only: a filtered-out model's own dangling references are never checked,
    /// and endpoints and dominions referenced only by filtered-out models may
    /// remain defined. An allowlist entry naming a model the merged catalog
    /// does not define is a hard validation error.
    ///
    /// # Errors
    /// Returns [`ConfigError::Validation`] when the allowlist names a model
    /// that neither `[[model]]` nor `[[local_model]]` defines.
    pub(crate) fn apply_model_allowlist(&mut self) -> Result<(), ConfigError> {
        let Some(allowlist) = &self.model_allowlist else {
            return Ok(());
        };
        let known: HashSet<&str> = self
            .models
            .iter()
            .map(|model| model.name.as_str())
            .chain(self.local_models.iter().map(|model| model.name.as_str()))
            .collect();
        for name in allowlist {
            if !known.contains(name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "models allowlist names undefined model {name}"
                )));
            }
        }
        self.models.retain(|model| allowlist.contains(&model.name));
        self.local_models
            .retain(|model| allowlist.contains(&model.name));
        Ok(())
    }

    /// Advertise `images = true` for every local model with a multimodal
    /// projector.
    ///
    /// A configured `[local_model.multimodal_projector]` makes the child
    /// image-capable (`--mmproj`), so the catalog must not report the
    /// `images` default of false. Runs after [`Self::apply_model_allowlist`]
    /// and before [`Self::validate`], so downstream code reads the resolved
    /// capability verbatim. The flag is a plain `bool`, so an explicit
    /// `images = false` cannot be told apart from an absent one; the
    /// projector wins either way because the model does accept images.
    pub(crate) fn imply_projector_images(&mut self) {
        for local_model in &mut self.local_models {
            if local_model.multimodal_projector.is_some() {
                local_model.capabilities.images = true;
            }
        }
    }

    /// Check names are unique, references resolve, URLs parse, and closed
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
        if self.server.api_key.is_empty() {
            return Err(ConfigError::Validation(
                "server.key must not be empty".to_string(),
            ));
        }
        self.validate_dominions()?;
        let endpoint_ids = self.validate_endpoints()?;
        self.validate_models(&endpoint_ids)?;
        self.validate_vram_budgets()?;
        self.validate_tools()?;
        Ok(())
    }

    /// Validate `[tools.web_search]` bounds, URL, and closed knobs at load so
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

    /// Sum the `vram_gb` estimates of the local models bound to each budgeted
    /// local dominion and reject over-booking. Runs after binding validation,
    /// so every `local_model.dominion` encountered here names a local
    /// dominion. A budget is meaningful only when it is complete: a bound
    /// model without an estimate is an error. A local dominion without a
    /// budget imposes no co-residency obligation.
    fn validate_vram_budgets(&self) -> Result<(), ConfigError> {
        for dominion in &self.dominions {
            let Some(budget) = dominion.vram_gb else {
                continue;
            };
            let mut total: u64 = 0;
            for model in &self.local_models {
                let Some(bound) = &model.dominion else {
                    continue;
                };
                if bound != &dominion.id {
                    continue;
                }
                let Some(estimate) = model.vram_gb else {
                    return Err(ConfigError::Validation(format!(
                        "local_model {} must set vram_gb: dominion {} has a vram_gb budget",
                        model.name, dominion.id
                    )));
                };
                total += u64::from(estimate);
            }
            let budget = u64::from(budget);
            if total > budget {
                return Err(ConfigError::Validation(format!(
                    "dominion {} vram_gb budget {} exceeded by {} (bound local models sum to {})",
                    dominion.id,
                    budget,
                    total - budget,
                    total
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

    fn validate_models(&self, endpoint_ids: &HashSet<&str>) -> Result<(), ConfigError> {
        let mut model_names = HashSet::new();
        for model in &self.models {
            if !model_names.insert(model.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model name {}",
                    model.name
                )));
            }
            if model.name.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "model name must not be empty".to_string(),
                ));
            }
            if model.description.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} description must not be empty",
                    model.name
                )));
            }
            if model.upstream.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} upstream must not be empty",
                    model.name
                )));
            }
            if model.context == 0 {
                return Err(ConfigError::Validation(format!(
                    "model {} context must be greater than zero",
                    model.name
                )));
            }
            if model.default_max_tokens == Some(0) {
                return Err(ConfigError::Validation(format!(
                    "model {} default_max_tokens must be greater than zero",
                    model.name
                )));
            }
            if model.endpoints.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "model {} has no endpoints",
                    model.name
                )));
            }
            let mut seen_endpoints = HashSet::new();
            for endpoint in &model.endpoints {
                if !endpoint_ids.contains(endpoint.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "model {} names undefined endpoint {endpoint}",
                        model.name
                    )));
                }
                if !seen_endpoints.insert(endpoint.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "model {} lists duplicate endpoint {endpoint}",
                        model.name
                    )));
                }
            }
            validate_kind_scope(
                "model",
                &model.name,
                model.kind,
                model.thinking,
                &model.capabilities,
                &[
                    ("default_max_tokens", model.default_max_tokens.is_some()),
                    ("tool_dialect", model.tool_dialect != ToolDialect::Openai),
                ],
            )?;
            validate_capabilities(
                "model",
                &model.name,
                model.context,
                model.thinking,
                &model.capabilities,
            )?;
        }

        self.validate_local_models(&mut model_names)
    }

    fn validate_local_models<'a>(
        &'a self,
        model_names: &mut HashSet<&'a str>,
    ) -> Result<(), ConfigError> {
        for local_model in &self.local_models {
            if local_model.name.is_empty() {
                return Err(ConfigError::Validation(
                    "local_model name must not be empty".to_string(),
                ));
            }
            if !model_names.insert(local_model.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model name {}",
                    local_model.name
                )));
            }
            if local_model.description.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "local_model {} description must not be empty",
                    local_model.name
                )));
            }
            validate_artifact_source(
                &format!("local_model {}", local_model.name),
                "source",
                &local_model.source,
                local_model.sha256.as_deref(),
            )?;
            if local_model.context < 1 {
                return Err(ConfigError::Validation(format!(
                    "local_model {} context must be at least 1",
                    local_model.name
                )));
            }
            if local_model.n_predict < 1 {
                return Err(ConfigError::Validation(format!(
                    "local_model {} n_predict must be at least 1",
                    local_model.name
                )));
            }
            if local_model.cache_type_k.is_empty() || local_model.cache_type_v.is_empty() {
                return Err(ConfigError::Validation(format!(
                    "local_model {} cache_type_k/v must not be empty",
                    local_model.name
                )));
            }
            if local_model.parallel < 1 {
                return Err(ConfigError::Validation(format!(
                    "local_model {} parallel must be at least 1",
                    local_model.name
                )));
            }
            self.validate_local_model_dominion(local_model)?;
            validate_kind_scope(
                "local_model",
                &local_model.name,
                local_model.kind,
                local_model.thinking,
                &local_model.capabilities,
                &[
                    (
                        "chat_template_file",
                        local_model.chat_template_file.is_some(),
                    ),
                    ("speculative", local_model.speculative.is_some()),
                    (
                        "multimodal_projector",
                        local_model.multimodal_projector.is_some(),
                    ),
                ],
            )?;
            if let Some(speculative) = &local_model.speculative {
                speculative.validate(&local_model.name)?;
            }
            if let Some(projector) = &local_model.multimodal_projector {
                projector.validate(&local_model.name)?;
            }
            validate_capabilities(
                "local_model",
                &local_model.name,
                local_model.context,
                local_model.thinking,
                &local_model.capabilities,
            )?;
        }
        Ok(())
    }

    /// A local model's `dominion` must name a defined local dominion.
    fn validate_local_model_dominion(
        &self,
        local_model: &super::LocalModelConfig,
    ) -> Result<(), ConfigError> {
        let Some(dominion_id) = &local_model.dominion else {
            return Ok(());
        };
        let dominion = self.dominions.iter().find(|d| d.id == *dominion_id);
        let Some(dominion) = dominion else {
            return Err(ConfigError::Validation(format!(
                "local_model {} names undefined dominion {dominion_id}",
                local_model.name
            )));
        };
        if dominion.kind != DominionKind::Local {
            return Err(ConfigError::Validation(format!(
                "local_model {} must reference a local dominion, but {dominion_id} is remote",
                local_model.name
            )));
        }
        Ok(())
    }
}

/// Validate the capability metadata of one model entry.
///
/// `default_effort` requires a non-empty `effort_levels` and must name a
/// listed level; the effort knobs are meaningless on a model that never
/// thinks; and `max_output` must fit the context window.
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
    Ok(())
}

/// Reject chat-only fields on a non-chat model kind.
///
/// `thinking` and the capability effort knobs (`effort_levels`,
/// `default_effort`, `adaptive_thinking`) are chat-only on every model type;
/// `extra` carries each model type's remaining chat-only fields as `(field,
/// is_set)` pairs. `context` applies to every kind and is never rejected
/// here. A chat model carries the default kind and passes unconditionally.
fn validate_kind_scope(
    label: &str,
    name: &str,
    kind: ModelKind,
    thinking: ThinkingMode,
    capabilities: &Capabilities,
    extra: &[(&str, bool)],
) -> Result<(), ConfigError> {
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

/// Parse `raw` and require an `http`/`https` scheme with a non-empty host.
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

#[cfg(test)]
mod tests {
    use super::{is_iso_date, is_valid_freshness, is_valid_safesearch, validate_http_url};

    #[test]
    fn http_url_accepts_http_and_https_with_host() {
        assert!(validate_http_url("ctx", "http://127.0.0.1:9").is_ok());
        assert!(validate_http_url("ctx", "https://api.example.com/res/v1").is_ok());
    }

    #[test]
    fn http_url_rejects_missing_scheme_and_bad_scheme() {
        assert!(validate_http_url("ctx", "not-a-url").is_err());
        assert!(validate_http_url("ctx", "ftp://example.com").is_err());
        assert!(validate_http_url("ctx", "127.0.0.1:9").is_err());
    }

    #[test]
    fn freshness_vocabulary() {
        for ok in ["", "pd", "pw", "pm", "py", "2024-01-01to2024-12-31"] {
            assert!(is_valid_freshness(ok), "expected {ok:?} to be valid");
        }
        for bad in [
            "daily",
            "p1",
            "2024/01/01to2024/12/31",
            "2024-1-1to2024-12-31",
        ] {
            assert!(!is_valid_freshness(bad), "expected {bad:?} to be invalid");
        }
    }

    #[test]
    fn safesearch_vocabulary() {
        for ok in ["", "off", "moderate", "strict"] {
            assert!(is_valid_safesearch(ok));
        }
        for bad in ["on", "medium", "safe"] {
            assert!(!is_valid_safesearch(bad));
        }
    }

    #[test]
    fn iso_date_shape() {
        assert!(is_iso_date("2024-01-01"));
        assert!(!is_iso_date("2024-1-01"));
        assert!(!is_iso_date("2024-01-01T"));
    }
}
