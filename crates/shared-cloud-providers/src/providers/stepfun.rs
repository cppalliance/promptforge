//! StepFun provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.stepfun.ai/v1` - Bearer auth and
//! the IDs-only OpenAI response shape: no pagination, no token limits,
//! and no capability reporting, so every entry is the conservative base
//! entry. The international `.ai` host is used; the China host is
//! `api.stepfun.com`. Vision support is only inferable from model IDs,
//! never from a field, so it is not mapped.
//!
//! Docs: <https://platform.stepfun.ai/docs/en/api-reference/models/list>

use serde::Deserialize;
use shared_gateway_api::{ModelEntry, Tier};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "STEPFUN_API_KEY";

/// The StepFun provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "stepfun",
    display_name: "StepFun",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.stepfun.ai/v1",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/models";

/// Fetch and normalize StepFun's model list in a single request.
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
    let models: Vec<WireModel> =
        fetch_list(client, &format!("{base_url}{MODELS_PATH}"), key).await?;
    Ok(models.iter().map(normalize_model).collect())
}

/// One model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
}

/// Normalize one wire model: the endpoint is IDs-only, so the entry is
/// the conservative base.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented example response shape (2026-09-14 research
    /// extraction): two flash models and a vision-suffixed model whose
    /// vision capability must NOT be inferred.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "step-3.7-flash",
      "object": "model",
      "created": 1713196800,
      "owned_by": "stepai"
    },
    {
      "id": "step-3.5-flash",
      "object": "model",
      "created": 1713974400,
      "owned_by": "stepai"
    },
    {
      "id": "step-1o-turbo-vision",
      "object": "model",
      "created": 1711015200,
      "owned_by": "stepai"
    }
  ]
}"#;

    #[test]
    fn ids_only_entries_are_conservative() {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        let entries: Vec<ModelEntry> = page.data.iter().map(normalize_model).collect();
        assert_eq!(entries.len(), 3, "all listed models normalize");
        let entry = &entries[0];
        assert_eq!(entry.id, "step-3.7-flash");
        assert_eq!(entry.display_name, "step-3.7-flash");
        assert_eq!(entry.context_window, None, "IDs-only providers omit limits");
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling && !entry.thinking.supported);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn vision_suffix_is_not_inferred() {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        let entry = normalize_model(&page.data[2]);
        assert_eq!(entry.id, "step-1o-turbo-vision");
        assert!(
            !entry.images,
            "the `-vision` id suffix is not a reported field and must not set images"
        );
    }
}
