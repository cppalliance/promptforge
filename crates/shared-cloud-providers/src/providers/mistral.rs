//! Mistral provider: the public descriptor plus the private variance of
//! `GET /v1/models` under `https://api.mistral.ai` - Bearer auth and the
//! OpenAI list envelope carrying Mistral's `BaseModelCard`: a
//! `capabilities` object (`completion_chat`, `completion_fim`,
//! `function_calling`, `vision`, and more), `max_context_length`, and
//! `deprecation` with `deprecation_replacement_model`. Of the capability
//! booleans only `function_calling` and `vision` have sheet fields; FIM
//! and the rest drop out. No pagination, no pricing, no max-output
//! field.
//!
//! Docs: <https://docs.mistral.ai/api/endpoint/models>

use serde::Deserialize;
use shared_gateway_api::{Deprecation, ModelEntry, Tier};
use time::format_description::well_known::Rfc3339;
use time::{Date, Month, OffsetDateTime};

use crate::providers::openai_shape::{base_entry, fetch_list};
use crate::{FetchError, Provider};

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "MISTRAL_API_KEY";

/// The Mistral provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "mistral",
    display_name: "Mistral AI",
    tier: Tier::Subprime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.mistral.ai",
};

/// The list path under the base URL.
const MODELS_PATH: &str = "/v1/models";

/// Fetch and normalize Mistral's model list in a single request.
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

/// One model as the wire reports it. `owned_by`, `description`,
/// `aliases`, `default_model_temperature`, and the fine-tuning card
/// fields carry no sheet meaning and are not parsed.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    created: Option<i64>,
    name: Option<String>,
    capabilities: Option<WireCapabilities>,
    max_context_length: Option<u32>,
    deprecation: Option<String>,
    deprecation_replacement_model: Option<String>,
}

/// The capability booleans with sheet meaning. The remaining booleans
/// (`completion_chat`, `completion_fim`, `fine_tuning`,
/// `classification`, the audio family, `moderation`, `ocr`) have no
/// sheet field and drop out.
#[derive(Debug, Deserialize)]
struct WireCapabilities {
    function_calling: Option<bool>,
    vision: Option<bool>,
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let mut entry = base_entry(&model.id, model.created);
    if let Some(name) = &model.name {
        entry.display_name.clone_from(name);
    }
    entry.context_window = model.max_context_length;
    entry.tool_calling = model
        .capabilities
        .as_ref()
        .and_then(|c| c.function_calling)
        .unwrap_or(false);
    entry.images = model
        .capabilities
        .as_ref()
        .and_then(|c| c.vision)
        .unwrap_or(false);
    entry.deprecation = model.deprecation.as_ref().map(|deprecation| Deprecation {
        status: "deprecated".to_owned(),
        date: parse_deprecation_date(deprecation),
        replacement: model.deprecation_replacement_model.clone(),
    });
    entry
}

/// Parse the deprecation timestamp: RFC 3339 first, then a bare
/// `YYYY-MM-DD` calendar date; an unparseable value keeps the status
/// with no date.
fn parse_deprecation_date(value: &str) -> Option<Date> {
    if let Ok(moment) = OffsetDateTime::parse(value, &Rfc3339) {
        return Some(moment.date());
    }
    let mut parts = value.split('-');
    let (Ok(year), Ok(month), Ok(day)) = (
        parts.next().unwrap_or_default().parse::<i32>(),
        parts.next().unwrap_or_default().parse::<u8>(),
        parts.next().unwrap_or_default().parse::<u8>(),
    ) else {
        return None;
    };
    Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::openai_shape::ListResponse;

    /// The documented `BaseModelCard` shape (2026-09-14 research
    /// extraction): a flagship chat model, a FIM-only code model, and a
    /// deprecated model naming its replacement.
    const LIST: &str = r#"{
  "object": "list",
  "data": [
    {
      "id": "mistral-large-latest",
      "object": "model",
      "created": 1767225600,
      "owned_by": "mistralai",
      "capabilities": {
        "completion_chat": true,
        "completion_fim": false,
        "function_calling": true,
        "fine_tuning": true,
        "vision": true,
        "classification": false
      },
      "name": "Mistral Large 3",
      "description": "Flagship model.",
      "max_context_length": 262144,
      "aliases": ["mistral-large-2512"],
      "deprecation": null,
      "deprecation_replacement_model": null,
      "type": "base"
    },
    {
      "id": "codestral-latest",
      "object": "model",
      "created": 1751500800,
      "owned_by": "mistralai",
      "capabilities": {
        "completion_chat": false,
        "completion_fim": true,
        "function_calling": false,
        "fine_tuning": false,
        "vision": false,
        "classification": false
      },
      "name": "Codestral",
      "description": "Code completion model.",
      "max_context_length": 256000,
      "aliases": ["codestral-2508"],
      "deprecation": null,
      "deprecation_replacement_model": null,
      "type": "base"
    },
    {
      "id": "mistral-small-2409",
      "object": "model",
      "created": 1725321600,
      "owned_by": "mistralai",
      "capabilities": {
        "completion_chat": true,
        "completion_fim": false,
        "function_calling": true,
        "fine_tuning": false,
        "vision": false,
        "classification": false
      },
      "name": null,
      "description": "Deprecated small model.",
      "max_context_length": 131072,
      "aliases": [],
      "deprecation": "2026-03-31T00:00:00Z",
      "deprecation_replacement_model": "mistral-small-latest",
      "type": "base"
    }
  ]
}"#;

    fn entries() -> Vec<ModelEntry> {
        let page: ListResponse<WireModel> =
            serde_json::from_str(LIST).expect("fixture must parse as a list");
        page.data.iter().map(normalize_model).collect()
    }

    #[test]
    fn capabilities_map_to_entry_fields() {
        let entry = &entries()[0];
        assert_eq!(entry.id, "mistral-large-latest");
        assert_eq!(
            entry.display_name, "Mistral Large 3",
            "the card's name is the display name"
        );
        assert_eq!(entry.kind, shared_gateway_api::ModelKind::Chat);
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::January, 1).ok(),
            "1767225600 is 2026-01-01T00:00:00Z"
        );
        assert_eq!(
            entry.context_window,
            Some(262_144),
            "max_context_length maps to context_window"
        );
        assert!(entry.tool_calling, "function_calling maps to tool_calling");
        assert!(entry.images, "vision maps to images");
        assert_eq!(entry.max_output, None, "the endpoint reports no max output");
        assert!(entry.pricing.is_none(), "the endpoint reports no pricing");
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn fim_only_capability_has_no_sheet_field() {
        let entry = &entries()[1];
        assert_eq!(entry.id, "codestral-latest");
        assert_eq!(
            entry.kind,
            shared_gateway_api::ModelKind::Chat,
            "completion_fim has no sheet field; the kind stays chat"
        );
        assert!(!entry.tool_calling);
        assert!(!entry.images);
        assert_eq!(entry.context_window, Some(256_000));
    }

    #[test]
    fn deprecation_maps_with_date_and_replacement() {
        let entry = &entries()[2];
        assert_eq!(
            entry.display_name, "mistral-small-2409",
            "a null name falls back to the id"
        );
        let deprecation = entry.deprecation.as_ref().expect("deprecation must map");
        assert_eq!(deprecation.status, "deprecated");
        assert_eq!(
            deprecation.date,
            Date::from_calendar_date(2026, Month::March, 31).ok(),
            "the deprecation timestamp maps to a calendar date"
        );
        assert_eq!(
            deprecation.replacement.as_deref(),
            Some("mistral-small-latest"),
            "deprecation_replacement_model names the successor"
        );
    }

    #[test]
    fn absent_capabilities_are_conservative() {
        let page: ListResponse<WireModel> = serde_json::from_str(
            r#"{
              "object": "list",
              "data": [
                {
                  "id": "mistral-legacy",
                  "object": "model",
                  "created": null,
                  "capabilities": null,
                  "name": null,
                  "max_context_length": null,
                  "deprecation": null,
                  "deprecation_replacement_model": null
                }
              ]
            }"#,
        )
        .expect("fixture must parse as a list");
        let entry = normalize_model(&page.data[0]);
        assert!(!entry.tool_calling);
        assert!(!entry.images);
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.released_at, None);
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn bare_calendar_date_deprecation_parses() {
        assert_eq!(
            parse_deprecation_date("2026-12-31"),
            Date::from_calendar_date(2026, Month::December, 31).ok(),
            "a bare YYYY-MM-DD deprecation date parses"
        );
        assert_eq!(
            parse_deprecation_date("soon"),
            None,
            "an unparseable deprecation value keeps no date"
        );
    }
}
