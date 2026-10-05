//! Validation of the remote, local, and speech-to-text model catalog entries.

use std::collections::HashSet;

use super::super::companion::validate_artifact_source;
use super::super::{Config, DominionKind, SttModelConfig, ToolDialect};
use super::{validate_capabilities, validate_http_url, validate_kind_scope};
use crate::error::ConfigError;

impl Config {
    pub(super) fn validate_models(&self, endpoint_ids: &HashSet<&str>) -> Result<(), ConfigError> {
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
        for local_model in &self.catalog_local_models {
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
            if let Some(vram_gb) = local_model.vram_gb
                && (!vram_gb.is_finite() || vram_gb <= 0.0)
            {
                return Err(ConfigError::Validation(format!(
                    "local_model {} vram_gb must be finite and greater than zero",
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

    pub(super) fn validate_stt_models(&self) -> Result<(), ConfigError> {
        let mut names: HashSet<&str> = self
            .models
            .iter()
            .map(|model| model.name.as_str())
            .chain(
                self.catalog_local_models
                    .iter()
                    .map(|model| model.name.as_str()),
            )
            .collect();
        for model in &self.catalog_stt_models {
            if model.name.trim().is_empty() {
                return Err(ConfigError::Validation(
                    "stt_model name must not be empty".to_owned(),
                ));
            }
            if !names.insert(model.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate model name {}",
                    model.name
                )));
            }
            if model.source.trim().is_empty() {
                return Err(ConfigError::Validation(format!(
                    "stt_model {} source must not be empty",
                    model.name
                )));
            }
            if model.source.starts_with("http://") {
                return Err(ConfigError::Validation(format!(
                    "stt_model {} source must use https, not plaintext http",
                    model.name
                )));
            }
            if model.source.starts_with("https://") {
                validate_http_url(&format!("stt_model {} source", model.name), &model.source)?;
            }
            if let Some(sha256) = &model.sha256
                && !super::super::is_sha256_hex(sha256)
            {
                return Err(ConfigError::Validation(format!(
                    "stt_model {} sha256 must be 64 lowercase hex characters",
                    model.name
                )));
            }
            if !model.vram_gb.is_finite() || model.vram_gb <= 0.0 {
                return Err(ConfigError::Validation(format!(
                    "stt_model {} vram_gb must be finite and greater than zero",
                    model.name
                )));
            }
            self.validate_stt_model_dominion(model)?;
        }
        Ok(())
    }

    fn validate_stt_model_dominion(&self, model: &SttModelConfig) -> Result<(), ConfigError> {
        let Some(dominion_id) = &model.dominion else {
            return Ok(());
        };
        let Some(dominion) = self
            .dominions
            .iter()
            .find(|dominion| dominion.id == *dominion_id)
        else {
            return Err(ConfigError::Validation(format!(
                "stt_model {} names undefined dominion {dominion_id}",
                model.name
            )));
        };
        if dominion.kind != DominionKind::Local {
            return Err(ConfigError::Validation(format!(
                "stt_model {} must reference a local dominion, but {dominion_id} is remote",
                model.name
            )));
        }
        Ok(())
    }

    /// A local model's `dominion` must name a defined local dominion.
    fn validate_local_model_dominion(
        &self,
        local_model: &super::super::LocalModelConfig,
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
