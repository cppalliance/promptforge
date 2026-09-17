//! Anthropic provider: the public descriptor plus the private variance of
//! `GET /v1/models` - `x-api-key` and required `anthropic-version`
//! headers, cursor pagination on `after_id`, and normalization of the
//! verified 2026-09-14 response shape (`id`, `display_name`, `created_at`,
//! `max_input_tokens`, `max_tokens`, `capabilities`) into `ModelEntry`.
//!
//! Docs: <https://docs.anthropic.com/en/api/models-list>

use gateway_api::{EnvRole, ModelEntry, ModelKind, Thinking, Tier};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};

use crate::{EnvVarSpec, FetchError, Provider};

#[path = "anthropic-taxonomy.rs"]
pub(crate) mod taxonomy;

/// Environment variable the API key arrives under; matches the GitHub
/// secret name.
const KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// The Anthropic provider descriptor.
pub const PROVIDER: Provider = Provider {
    name: "anthropic",
    display_name: "Anthropic",
    tier: Tier::Prime,
    key_env: Some(KEY_ENV),
    base_url: "https://api.anthropic.com",
    openai_base_url: Some("https://api.anthropic.com/v1"),
    env_vars: &[EnvVarSpec {
        name: KEY_ENV,
        role: EnvRole::Key,
        default: None,
    }],
};

/// The API version the endpoint requires on every request.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Page size for the list request: the endpoint maximum, so the full
/// catalog arrives in one page today and pagination only engages as the
/// lineup grows.
const PAGE_LIMIT: u32 = 1000;

/// Fetch and normalize Anthropic's model list, following the cursor until
/// the final page.
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
    let mut entries = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut request = client
            .get(format!("{base_url}/v1/models"))
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .query(&[("limit", PAGE_LIMIT)]);
        if let Some(after) = &cursor {
            request = request.query(&[("after_id", after)]);
        }
        let page: Page = request.send().await?.error_for_status()?.json().await?;
        entries.extend(page.data.iter().map(normalize_model));
        let Some(next) = next_cursor(&page) else {
            break;
        };
        cursor = Some(next);
    }
    taxonomy::apply(&mut entries);
    Ok(entries)
}

/// One page of the list response.
#[derive(Debug, Deserialize)]
struct Page {
    data: Vec<WireModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

/// One model as the wire reports it.
#[derive(Debug, Deserialize)]
struct WireModel {
    id: String,
    display_name: String,
    created_at: String,
    max_input_tokens: Option<u32>,
    max_tokens: Option<u32>,
    capabilities: Option<WireCapabilities>,
}

/// The `capabilities` object; every member is optional in the wire schema.
#[derive(Debug, Deserialize)]
struct WireCapabilities {
    batch: Option<Support>,
    citations: Option<Support>,
    code_execution: Option<Support>,
    image_input: Option<Support>,
    pdf_input: Option<Support>,
    structured_outputs: Option<Support>,
    thinking: Option<WireThinking>,
    effort: Option<WireEffort>,
}

/// The `{ "supported": bool }` leaf every capability reports.
#[derive(Debug, Deserialize)]
struct Support {
    supported: bool,
}

/// The thinking capability and its per-type support flags.
#[derive(Debug, Deserialize)]
struct WireThinking {
    supported: bool,
    types: Option<WireThinkingTypes>,
}

/// Support for each thinking type configuration.
#[derive(Debug, Deserialize)]
struct WireThinkingTypes {
    adaptive: Option<Support>,
    enabled: Option<Support>,
}

/// The effort capability: one support flag per level name.
#[derive(Debug, Deserialize)]
struct WireEffort {
    low: Option<Support>,
    medium: Option<Support>,
    high: Option<Support>,
    xhigh: Option<Support>,
    max: Option<Support>,
}

/// The cursor for the next page: `last_id` while the endpoint reports
/// more results. A missing `last_id` stops traversal even when `has_more`
/// is true, so a malformed page can never loop the fetch forever.
fn next_cursor(page: &Page) -> Option<String> {
    if page.has_more {
        page.last_id.clone()
    } else {
        None
    }
}

/// Whether an optional capability leaf reports support.
fn supported(capability: Option<&Support>) -> bool {
    capability.is_some_and(|c| c.supported)
}

/// The provider's own supported effort level names in canonical knob
/// order; unsupported and absent levels drop out, and no cross-provider
/// ordinal scale is invented.
fn effort_levels(effort: Option<&WireEffort>) -> Vec<String> {
    let Some(effort) = effort else {
        return Vec::new();
    };
    [
        ("low", &effort.low),
        ("medium", &effort.medium),
        ("high", &effort.high),
        ("xhigh", &effort.xhigh),
        ("max", &effort.max),
    ]
    .into_iter()
    .filter(|(_, support)| supported(support.as_ref()))
    .map(|(name, _)| name.to_owned())
    .collect()
}

/// Normalize one wire model into a sheet entry.
fn normalize_model(model: &WireModel) -> ModelEntry {
    let caps = model.capabilities.as_ref();
    let capability = |pick: fn(&WireCapabilities) -> &Option<Support>| {
        supported(caps.map(pick).and_then(Option::as_ref))
    };
    let thinking = caps.and_then(|c| c.thinking.as_ref());
    let thinking_type = |pick: fn(&WireThinkingTypes) -> &Option<Support>| {
        supported(
            thinking
                .and_then(|t| t.types.as_ref())
                .map(pick)
                .and_then(Option::as_ref),
        )
    };
    ModelEntry {
        id: model.id.clone(),
        display_name: model.display_name.clone(),
        family: String::new(),
        variant_of: None,
        variant: None,
        languages: Vec::new(),
        kind: ModelKind::Chat,
        released_at: parse_release_date(&model.created_at),
        context_window: model.max_input_tokens,
        max_output: model.max_tokens,
        images: capability(|c| &c.image_input),
        pdf_input: capability(|c| &c.pdf_input),
        video_input: false,
        audio_input: false,
        batch: capability(|c| &c.batch),
        citations: capability(|c| &c.citations),
        code_execution: capability(|c| &c.code_execution),
        structured_outputs: capability(|c| &c.structured_outputs),
        // Curated: tool use is core to every Anthropic chat model; the
        // list endpoint does not report it.
        tool_calling: true,
        thinking: Thinking {
            supported: thinking.is_some_and(|t| t.supported),
            enabled: thinking_type(|t| &t.enabled),
            adaptive: thinking_type(|t| &t.adaptive),
        },
        effort_levels: effort_levels(caps.and_then(|c| c.effort.as_ref())),
        // The endpoint reports no default level, no pricing, and no
        // deprecation status.
        default_effort: None,
        pricing: None,
        deprecation: None,
    }
}

/// Parse the release date. The endpoint substitutes the epoch when the
/// release date is unknown; that sentinel normalizes to `None`, as does
/// an unparseable value.
fn parse_release_date(created_at: &str) -> Option<Date> {
    let date = OffsetDateTime::parse(created_at, &Rfc3339).ok()?.date();
    (date != OffsetDateTime::UNIX_EPOCH.date()).then_some(date)
}

#[cfg(test)]
mod tests {
    use time::Month;

    use super::*;

    /// First page of the recorded 2026-09-14 live payload shape: a fully
    /// capable model, with `has_more` set so pagination continues.
    const PAGE_1: &str = r#"{
  "data": [
    {
      "type": "model",
      "id": "claude-opus-5",
      "display_name": "Claude Opus 5",
      "created_at": "2026-07-24T00:00:00Z",
      "max_input_tokens": 1000000,
      "max_tokens": 128000,
      "capabilities": {
        "batch": { "supported": true },
        "citations": { "supported": true },
        "code_execution": { "supported": true },
        "context_management": {
          "clear_thinking_20251015": { "supported": true },
          "clear_tool_uses_20250919": { "supported": true },
          "compact_20260112": { "supported": true },
          "supported": true
        },
        "effort": {
          "low": { "supported": true },
          "medium": { "supported": true },
          "high": { "supported": true },
          "xhigh": { "supported": true },
          "max": { "supported": true },
          "supported": true
        },
        "image_input": { "supported": true },
        "pdf_input": { "supported": true },
        "structured_outputs": { "supported": true },
        "thinking": {
          "supported": true,
          "types": {
            "adaptive": { "supported": true },
            "enabled": { "supported": true }
          }
        }
      }
    }
  ],
  "first_id": "claude-opus-5",
  "has_more": true,
  "last_id": "claude-opus-5"
}"#;

    /// Second and final page: a leaner model exercising partial support
    /// flags, an absent `xhigh`/`max` effort level, and the epoch
    /// release-date sentinel.
    const PAGE_2: &str = r#"{
  "data": [
    {
      "type": "model",
      "id": "claude-haiku-4-5",
      "display_name": "Claude Haiku 4.5",
      "created_at": "1970-01-01T00:00:00Z",
      "max_input_tokens": 200000,
      "max_tokens": 64000,
      "capabilities": {
        "batch": { "supported": true },
        "citations": { "supported": false },
        "code_execution": { "supported": false },
        "effort": {
          "low": { "supported": true },
          "medium": { "supported": false },
          "high": { "supported": true },
          "supported": true
        },
        "image_input": { "supported": false },
        "pdf_input": { "supported": true },
        "structured_outputs": { "supported": true },
        "thinking": {
          "supported": true,
          "types": {
            "adaptive": { "supported": true },
            "enabled": { "supported": false }
          }
        }
      }
    }
  ],
  "first_id": "claude-haiku-4-5",
  "has_more": false,
  "last_id": "claude-haiku-4-5"
}"#;

    fn page(json: &str) -> Page {
        serde_json::from_str(json).expect("fixture must parse as a page")
    }

    fn only_model(json: &str) -> ModelEntry {
        let page = page(json);
        assert_eq!(page.data.len(), 1, "fixture holds exactly one model");
        normalize_model(&page.data[0])
    }

    #[test]
    fn normalizes_full_capability_entry() {
        let entry = only_model(PAGE_1);
        assert_eq!(entry.id, "claude-opus-5");
        assert_eq!(entry.display_name, "Claude Opus 5");
        assert_eq!(entry.kind, ModelKind::Chat);
        assert_eq!(
            entry.released_at,
            Date::from_calendar_date(2026, Month::July, 24).ok(),
            "created_at maps to released_at"
        );
        assert_eq!(entry.context_window, Some(1_000_000));
        assert_eq!(entry.max_output, Some(128_000));
        assert!(entry.images);
        assert!(entry.pdf_input);
        assert!(!entry.video_input);
        assert!(!entry.audio_input);
        assert!(entry.batch);
        assert!(entry.citations);
        assert!(entry.code_execution);
        assert!(entry.structured_outputs);
        assert!(
            entry.tool_calling,
            "tool use is core to every Anthropic chat model"
        );
        assert!(
            entry.thinking.supported && entry.thinking.enabled && entry.thinking.adaptive,
            "all three thinking flags map from the types object: {:?}",
            entry.thinking
        );
        assert_eq!(
            entry.effort_levels,
            ["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(entry.default_effort, None);
        assert!(entry.pricing.is_none());
        assert!(entry.deprecation.is_none());
    }

    #[test]
    fn partial_capabilities_map_conservatively() {
        let entry = only_model(PAGE_2);
        assert!(!entry.images, "image_input.supported false maps to false");
        assert!(entry.pdf_input);
        assert!(!entry.citations);
        assert!(!entry.code_execution);
        assert!(
            entry.thinking.supported && !entry.thinking.enabled && entry.thinking.adaptive,
            "per-type flags map independently of the top-level flag: {:?}",
            entry.thinking
        );
        assert_eq!(
            entry.effort_levels,
            ["low", "high"],
            "unsupported and absent levels drop out, canonical order holds"
        );
        assert_eq!(
            entry.released_at, None,
            "the epoch sentinel means the release date is unknown"
        );
        assert_eq!(entry.context_window, Some(200_000));
        assert_eq!(entry.max_output, Some(64_000));
    }

    #[test]
    fn null_capabilities_yield_a_conservative_entry() {
        let entry = only_model(
            r#"{
              "data": [
                {
                  "type": "model",
                  "id": "claude-legacy",
                  "display_name": "Claude Legacy",
                  "created_at": "2024-03-04T00:00:00Z",
                  "max_input_tokens": null,
                  "max_tokens": null,
                  "capabilities": null
                }
              ],
              "first_id": "claude-legacy",
              "has_more": false,
              "last_id": "claude-legacy"
            }"#,
        );
        assert_eq!(entry.context_window, None);
        assert_eq!(entry.max_output, None);
        assert!(!entry.batch);
        assert!(!entry.images);
        assert!(!entry.thinking.supported);
        assert!(!entry.thinking.enabled);
        assert!(!entry.thinking.adaptive);
        assert!(entry.effort_levels.is_empty());
        assert!(
            entry.tool_calling,
            "curated: Anthropic chat models take tools even when the endpoint is silent"
        );
    }

    #[test]
    fn pagination_follows_last_id_while_has_more() {
        let first = page(PAGE_1);
        let cursor = next_cursor(&first).expect("has_more page must yield a cursor");
        assert_eq!(cursor, "claude-opus-5", "the cursor is the page's last_id");
        let second = page(PAGE_2);
        assert_eq!(next_cursor(&second), None, "the final page ends traversal");
    }

    #[test]
    fn pagination_stops_when_last_id_is_absent() {
        let page = page(
            r#"{
              "data": [],
              "first_id": null,
              "has_more": true,
              "last_id": null
            }"#,
        );
        assert_eq!(
            next_cursor(&page),
            None,
            "has_more without a cursor must not loop forever"
        );
    }
}
