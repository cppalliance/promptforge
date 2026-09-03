//! Launch-time chat-template selection and bundled asset staging.

use std::path::{Path, PathBuf};

use gateway_config::{LocalModelConfig, ModelKind};

use crate::artifacts::ArtifactStore;
use crate::chat_templates::{
    Family, KnownOverride, family_for_model, family_for_model_source, hugging_face_model_id,
    known_override,
};
use crate::error::LocalError;
use crate::sidecar;

const BUILTIN_TEMPLATE_PREFIX: &str = "builtin:";

/// The source selected for a local model's effective chat template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChatTemplateSource {
    /// The template embedded in the GGUF.
    Embedded,
    /// A bundled replacement for a known-broken embedded template.
    KnownOverride,
    /// An operator-selected bundled family.
    Builtin,
    /// An operator-selected filesystem path.
    Custom,
}

impl ChatTemplateSource {
    /// Returns the stable admin-API spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Embedded => "embedded",
            Self::KnownOverride => "known-override",
            Self::Builtin => "builtin",
            Self::Custom => "custom",
        }
    }
}

/// The effective chat-template decision for one local chat model.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ChatTemplateResolution {
    decision: Decision,
    detected_family: Option<Family>,
    reason: String,
}

impl ChatTemplateResolution {
    /// Returns the selected source category.
    #[must_use]
    pub const fn source(&self) -> ChatTemplateSource {
        match &self.decision {
            Decision::Embedded | Decision::MissingTemplate => ChatTemplateSource::Embedded,
            Decision::KnownOverride(_) => ChatTemplateSource::KnownOverride,
            Decision::Builtin(_) | Decision::UnknownFamily(_) => ChatTemplateSource::Builtin,
            Decision::Custom(_) => ChatTemplateSource::Custom,
        }
    }

    /// Returns the selected bundled family, when the source has one.
    #[must_use]
    pub const fn family(&self) -> Option<Family> {
        match &self.decision {
            Decision::KnownOverride(known) => Some(known.family),
            Decision::Builtin(family) => Some(*family),
            _ => None,
        }
    }

    /// Returns the family detected by the exact model mapper.
    #[must_use]
    pub const fn detected_family(&self) -> Option<Family> {
        self.detected_family
    }

    /// Returns the operator-facing explanation of the decision.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The decision paired with the payload its launch path consumes, so the
/// launcher matches exhaustively and a misconfigured value travels with the
/// name the error reports.
#[derive(Debug, Clone)]
enum Decision {
    Embedded,
    KnownOverride(&'static KnownOverride),
    Builtin(Family),
    UnknownFamily(String),
    MissingTemplate,
    Custom(PathBuf),
}

/// Inspects the effective chat-template decision without staging an asset.
///
/// `model_path` should be the already-present GGUF path. `None` performs the
/// same configured-value and model-ID checks without reading or downloading
/// an artifact, which lets the Config UI describe pending downloads.
///
/// # Errors
/// Returns [`LocalError`] when an available GGUF cannot be inspected.
pub fn inspect_chat_template(
    model: &LocalModelConfig,
    model_path: Option<&Path>,
) -> Result<ChatTemplateResolution, LocalError> {
    let mut detected_family = family_for_model_source(model.source());
    if let Some(configured) = model
        .chat_template_file()
        .filter(|configured| !configured.trim().is_empty())
    {
        let Some(alias) = configured.strip_prefix(BUILTIN_TEMPLATE_PREFIX) else {
            return Ok(ChatTemplateResolution {
                decision: Decision::Custom(PathBuf::from(configured)),
                detected_family,
                reason: format!("Custom template path `{configured}` is selected."),
            });
        };
        let family = Family::parse_alias(alias);
        return Ok(ChatTemplateResolution {
            decision: family.map_or_else(
                || Decision::UnknownFamily(alias.to_owned()),
                Decision::Builtin,
            ),
            detected_family,
            reason: family.map_or_else(
                || format!("Unknown built-in template family `{alias}` is configured."),
                |family| format!("Built-in {} template is selected.", family.display_label()),
            ),
        });
    }

    let mut embedded = None;
    let model_id = if let Some(path) = model_path {
        let info = crate::gguf::read_model_info_path(path)?;
        embedded = info
            .chat_template
            .filter(|template| !template.trim().is_empty());
        sidecar_model_id(path)
    } else {
        hugging_face_model_id(model.source()).map(str::to_owned)
    };
    detected_family = detected_family.or_else(|| model_id.as_deref().and_then(family_for_model));
    let (known, evidence) = if let Some(known) = known_override(embedded.as_deref(), None) {
        (Some(known), "content hash")
    } else {
        (
            known_override(None, model_id.as_deref()),
            "model identifier",
        )
    };
    if let Some(known) = known {
        return Ok(ChatTemplateResolution {
            decision: Decision::KnownOverride(known),
            detected_family: detected_family.or(Some(known.family)),
            reason: format!("Known-broken embedded template matched by {evidence}."),
        });
    }
    let artifact_available = model_path.is_some();
    let usable = embedded.is_some();
    Ok(ChatTemplateResolution {
        decision: if artifact_available && !usable {
            Decision::MissingTemplate
        } else {
            Decision::Embedded
        },
        detected_family,
        reason: if usable {
            "Auto uses the GGUF embedded template.".to_owned()
        } else if artifact_available {
            "Auto found no usable GGUF embedded template.".to_owned()
        } else {
            "Auto will inspect the GGUF embedded template when the model is available.".to_owned()
        },
    })
}

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

    let resolution = inspect_chat_template(model, Some(model_path))?;
    match resolution.decision {
        Decision::Custom(configured) => {
            tracing::info!(
                model = %model.name(),
                path = %configured.display(),
                "selected explicit chat-template file"
            );
            Ok(Some(configured))
        }
        Decision::Builtin(family) => {
            let path = stage_family_template(store, family)?;
            tracing::info!(
                model = %model.name(),
                family = %family.canonical_name(),
                path = %path.display(),
                "selected built-in chat template"
            );
            Ok(Some(path))
        }
        Decision::UnknownFamily(family) => Err(LocalError::UnknownChatTemplateFamily {
            model: model.name().to_owned(),
            family,
            valid_families: valid_family_names(),
        }),
        Decision::KnownOverride(known) => {
            let path = stage_template_asset(store, known.asset_name, known.template)?;
            tracing::info!(
                model = %model.name(),
                family = %known.family.canonical_name(),
                path = %path.display(),
                reason = %resolution.reason,
                "selected known chat-template override"
            );
            Ok(Some(path))
        }
        Decision::Embedded => {
            tracing::info!(
                model = %model.name(),
                "selected embedded GGUF chat template"
            );
            Ok(None)
        }
        Decision::MissingTemplate => Err(LocalError::MissingChatTemplate {
            model: model.name().to_owned(),
            valid_families: valid_family_names(),
        }),
    }
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
