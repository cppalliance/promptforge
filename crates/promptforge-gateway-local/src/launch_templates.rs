//! Launch-time chat-template selection and bundled asset staging.

use std::path::{Path, PathBuf};

use promptforge_gateway_config::{LocalModelConfig, ModelKind};

use crate::artifacts::ArtifactStore;
use crate::chat_templates::{Family, known_override};
use crate::error::LocalError;
use crate::sidecar;

const BUILTIN_TEMPLATE_PREFIX: &str = "builtin:";

/// Resolves the optional file override for one local chat model.
///
/// A configured custom path wins, followed by a configured bundled family,
/// a known broken-template replacement selected by embedded hash then
/// sidecar model ID, and finally a non-empty embedded template. Embedded
/// templates need no file argument because `llama-server` reads them from the
/// GGUF under the always-present `--jinja` flag.
pub(super) fn resolve_chat_template_file(
    store: &ArtifactStore,
    model: &LocalModelConfig,
    model_path: &Path,
) -> Result<Option<PathBuf>, LocalError> {
    if model.kind() != ModelKind::Chat {
        return Ok(None);
    }

    if let Some(configured) = model
        .chat_template_file()
        .filter(|configured| !configured.trim().is_empty())
    {
        let Some(alias) = configured.strip_prefix(BUILTIN_TEMPLATE_PREFIX) else {
            tracing::info!(
                model = %model.name(),
                path = %configured,
                "selected explicit chat-template file"
            );
            return Ok(Some(PathBuf::from(configured)));
        };
        let family =
            Family::parse_alias(alias).ok_or_else(|| LocalError::UnknownChatTemplateFamily {
                model: model.name().to_owned(),
                family: alias.to_owned(),
                valid_families: valid_family_names(),
            })?;
        let path = stage_family_template(store, family)?;
        tracing::info!(
            model = %model.name(),
            family = %family.canonical_name(),
            path = %path.display(),
            "selected built-in chat template"
        );
        return Ok(Some(path));
    }

    let info = crate::gguf::read_model_info_path(model_path)?;
    let embedded = info
        .chat_template
        .as_deref()
        .filter(|template| !template.trim().is_empty());
    let model_id = sidecar_model_id(model_path);
    let (known, evidence) = if let Some(known) = known_override(embedded, None) {
        (Some(known), "embedded template hash")
    } else {
        (
            known_override(None, model_id.as_deref()),
            "sidecar model ID",
        )
    };
    if let Some(known) = known {
        let path = stage_template_asset(store, known.asset_name, known.template)?;
        tracing::info!(
            model = %model.name(),
            family = %known.family.canonical_name(),
            path = %path.display(),
            evidence,
            "selected known chat-template override"
        );
        return Ok(Some(path));
    }
    if embedded.is_some() {
        tracing::info!(
            model = %model.name(),
            "selected embedded GGUF chat template"
        );
        return Ok(None);
    }

    Err(LocalError::MissingChatTemplate {
        model: model.name().to_owned(),
        valid_families: valid_family_names(),
    })
}

fn stage_family_template(store: &ArtifactStore, family: Family) -> Result<PathBuf, LocalError> {
    let asset_name = format!("{}.jinja", family.canonical_name());
    stage_template_asset(store, &asset_name, family.template())
}

fn stage_template_asset(
    store: &ArtifactStore,
    asset_name: &str,
    template: &str,
) -> Result<PathBuf, LocalError> {
    store.stage_verified_asset(
        &Path::new("chat-templates").join(asset_name),
        template.as_bytes(),
    )
}

fn sidecar_model_id(model_path: &Path) -> Option<String> {
    match sidecar::read_sidecar(model_path) {
        Ok(Some(metadata)) => metadata.source_model_id(),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(
                path = %sidecar::sidecar_path(model_path).display(),
                error = %error,
                "could not read model ID from chat-template sidecar"
            );
            None
        }
    }
}

fn valid_family_names() -> String {
    Family::ALL
        .map(Family::canonical_name)
        .as_slice()
        .join(", ")
}

#[cfg(test)]
mod tests;
