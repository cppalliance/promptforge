//! Leonardo provider: the public descriptor plus the private variance
//! of `GET /platformModels` under
//! `https://cloud.leonardo.ai/api/rest/v1` - Bearer auth and a
//! single-page `custom_models` envelope of public platform models
//! (`id`, `name`, `description`). Every listed model is image
//! generation, so the image kind is set; `description`, `featured`,
//! `nsfw`, and `generated_image` have no sheet fields and are not
//! parsed. No pagination, no token limits, no pricing.
//!
//! Docs: <https://docs.leonardo.ai/v1.0/reference/listplatformmodels>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, ModelKind, Tier};

use crate::providers::openai_shape::base_entry;
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "LEONARDO_API_KEY";

/// The Leonardo provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "leonardo",
    display_name: "Leonardo",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://cloud.leonardo.ai/api/rest/v1",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/platformModels";

/// Fetch and normalize Leonardo's platform model list in a single
/// request.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    base_url: &str,
    key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    let Some(key) = key else {
        return Err(FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: KEY_ENV,
        });
    };
    let response: ListResponse = client
        .get(format!("{base_url}{MODELS_PATH}"))
        .bearer_auth(key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(response.custom_models.iter().map(normalize_model).collect())
}

/// The list envelope: platform models arrive under the (documented)
/// `custom_models` key.
#[derive(Debug, Deserialize)]
struct ListResponse {
    custom_models: Vec<WireModel>,
}

/// One model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    name: Option<String>,
}

/// Normalize one wire model: every platform model is image generation,
/// so the entry is the conservative base with the image kind.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, None);
    if let Some(name) = &model.name {
        entry.display_name.clone_from(name);
    }
    entry.kind = ModelKind::Image;
    entry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented response shape (2026-09-14 research extraction
    /// and the v1 OpenAPI schema): two public platform models.
    const LIST: &str = r#"{
  "custom_models": [
    {
      "id": "aa77f04e-3eec-4034-9c07-d0f619684628",
      "name": "Leonardo Phoenix",
      "description": "Leonardo's flagship image model.",
      "featured": true,
      "nsfw": false,
      "generated_image": null
    },
    {
      "id": "b24e16ff-06e3-43eb-8d33-4416c2d75876",
      "name": null,
      "description": "FLUX variant.",
      "featured": false,
      "nsfw": false,
      "generated_image": null
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let response: ListResponse =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        response.custom_models.iter().map(normalize_model).collect()
    }

    #[test]
    fn platform_models_normalize_to_image_kind() {
        let entries = entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "aa77f04e-3eec-4034-9c07-d0f619684628");
        assert_eq!(
            entries[0].display_name, "Leonardo Phoenix",
            "the wire name is the display name"
        );
        for entry in &entries {
            assert_eq!(
                entry.kind,
                ModelKind::Image,
                "every platform model is image generation"
            );
            assert_eq!(entry.context_window, None, "the endpoint reports no limits");
            assert!(entry.pricing.is_none());
            assert!(entry.deprecation.is_none());
        }
    }

    #[test]
    fn missing_name_falls_back_to_the_id() {
        let entry = &entries()[1];
        assert_eq!(
            entry.display_name, entry.id,
            "a null name falls back to the id"
        );
    }
}
