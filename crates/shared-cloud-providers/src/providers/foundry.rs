//! Azure AI Foundry provider: the public descriptor plus the private
//! variance of `GET {endpoint}/openai/v1/models` - `api-key` header
//! auth and the OpenAI `ListModelsResponse` shape carrying basic info
//! only (`id`, `created`, `object`, `owned_by`). The endpoint is
//! per-resource; there is no global default.
//!
//! Extra environment variables beyond the descriptor's
//! `AZURE_FOUNDRY_API_KEY`: `AZURE_FOUNDRY_ENDPOINT` (required - the
//! per-resource endpoint origin, e.g.
//! `https://myresource.services.ai.azure.com`; absent records
//! `unavailable`).
//!
//! Docs: <https://learn.microsoft.com/en-us/rest/api/aifoundry/azureopenai/models>

use serde::Deserialize;
use shared_gateway_api::{EnvRole, ModelEntry, Tier};

use crate::providers::openai_shape::{ListResponse, base_entry};
use crate::{EnvVarSpec, FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "AZURE_FOUNDRY_API_KEY";

/// Environment variable carrying the per-resource endpoint origin.
const ENDPOINT_ENV: &str = "AZURE_FOUNDRY_ENDPOINT";

/// The Azure AI Foundry provider descriptor. `base_url` is empty: the
/// endpoint is per-resource and arrives via `AZURE_FOUNDRY_ENDPOINT`.
pub const PROVIDER: Provider = Provider {
    name: "foundry",
    display_name: "Azure AI Foundry",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "",
    openai_base_url: None,
    env_vars: &[
        EnvVarSpec {
            name: KEY_ENV,
            role: EnvRole::Key,
            default: None,
        },
        EnvVarSpec {
            name: ENDPOINT_ENV,
            role: EnvRole::Config,
            default: None,
        },
    ],
};

/// The list path under the endpoint origin.
const MODELS_PATH: &str = "/openai/v1/models";

/// Fetch and normalize Foundry's model list in a single request.
pub(crate) async fn fetch(
    client: &reqwest::Client,
    _base_url: &str,
    key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    let Some(key) = key else {
        return Err(FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: KEY_ENV,
        });
    };
    let endpoint = endpoint_from(std::env::var(ENDPOINT_ENV).ok())?;
    let response: ListResponse<WireModel> = client
        .get(format!("{endpoint}{MODELS_PATH}"))
        .header("api-key", key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mut entries: Vec<ModelEntry> = response.data.iter().map(normalize_model).collect();
    apply_taxonomy(&mut entries);
    Ok(entries)
}

/// The per-resource endpoint origin from the environment, trailing
/// slashes trimmed. Absent or blank records `unavailable` (a failed
/// fetch naming the missing variable), since Foundry has no global
/// default endpoint.
fn endpoint_from(env_value: Option<String>) -> Result<String, FetchError> {
    let value = env_value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| FetchError::MissingKey {
            name: PROVIDER.name.to_owned(),
            key_env: ENDPOINT_ENV,
        })?;
    Ok(value.trim_end_matches('/').to_owned())
}

/// One model as the wire reports it: the OpenAI `Model` shape, basic
/// info only. `object` and `owned_by` carry no sheet meaning.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    base_entry(&model.id, model.created)
}

/// Set every entry's family to its own id: the catalog is per-resource
/// deployments (`gpt-4o`, `Phi-4`), whose names are operator-chosen;
/// there is no cross-deployment naming convention to group on, and no
/// snapshot suffixes to collapse.
fn apply_taxonomy(entries: &mut [ModelEntry]) {
    for entry in entries.iter_mut() {
        entry.family = entry.id.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented OpenAI `ListModelsResponse` shape (2026-09-14
    /// research extraction): basic info only.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "gpt-4o",
      "object": "model",
      "created": 1715367049,
      "owned_by": "system"
    },
    {
      "id": "Phi-4",
      "object": "model",
      "created": 1734134400,
      "owned_by": "system"
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn absent_endpoint_records_unavailable() {
        let err = endpoint_from(None).expect_err("an absent endpoint must fail the fetch");
        assert!(
            matches!(err, FetchError::MissingKey { .. }),
            "an absent endpoint fails like a missing key: {err:?}"
        );
        assert!(
            err.to_string().contains("AZURE_FOUNDRY_ENDPOINT"),
            "the error names the missing variable: {err}"
        );
        let err = endpoint_from(Some("   ".to_owned()))
            .expect_err("a blank endpoint must fail the fetch");
        assert!(matches!(err, FetchError::MissingKey { .. }));
    }

    #[test]
    fn endpoint_trims_trailing_slashes() {
        let endpoint = endpoint_from(Some("https://myresource.services.ai.azure.com/".to_owned()))
            .expect("a set endpoint resolves");
        assert_eq!(endpoint, "https://myresource.services.ai.azure.com");
    }

    #[test]
    fn models_normalize_to_conservative_entries() {
        let entries = entries();
        assert_eq!(entries.len(), 2);
        let entry = &entries[0];
        assert_eq!(entry.id, "gpt-4o");
        assert_eq!(
            entry.display_name, "gpt-4o",
            "the id doubles as the display name"
        );
        assert_eq!(entry.kind, shared_gateway_api::ModelKind::Chat);
        assert_eq!(
            entry.released_at,
            time::Date::from_calendar_date(2024, time::Month::May, 10).ok(),
            "1715367049 is 2024-05-10T18:50:49Z"
        );
        assert_eq!(
            entry.context_window, None,
            "the endpoint reports basic info only"
        );
        assert_eq!(entry.max_output, None);
        assert!(!entry.images && !entry.tool_calling);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn deployment_ids_are_their_own_families() {
        // Foundry's catalog is per-resource deployments; there is no
        // cross-deployment naming convention to group on.
        let mut entries: Vec<ModelEntry> = ["gpt-4o", "Phi-4", "llama-3.3-70b"]
            .iter()
            .map(|id| crate::taxonomy::fixture::entry(id))
            .collect();
        apply_taxonomy(&mut entries);
        for entry in &entries {
            assert_eq!(entry.family, entry.id, "the whole id is the family");
            assert!(entry.variant_of.is_none());
        }
    }

    #[test]
    fn descriptor_declares_the_azure_variables_and_no_chat_base() {
        assert_eq!(
            PROVIDER.openai_base_url, None,
            "the endpoint is per-resource, not a fixed URL"
        );
        let vars: &[(&str, shared_gateway_api::EnvRole, Option<&str>)] = &[
            (
                "AZURE_FOUNDRY_API_KEY",
                shared_gateway_api::EnvRole::Key,
                None,
            ),
            (
                "AZURE_FOUNDRY_ENDPOINT",
                shared_gateway_api::EnvRole::Config,
                None,
            ),
        ];
        assert_eq!(PROVIDER.env_vars.len(), vars.len());
        for (spec, &(name, role, default)) in PROVIDER.env_vars.iter().zip(vars) {
            assert_eq!(spec.name, name);
            assert_eq!(spec.role, role);
            assert_eq!(spec.default, default);
        }
    }
}
