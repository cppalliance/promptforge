//! The Gateway's public wire vocabulary (`gateway-api-types`): the provider
//! model sheet schema, one versioned JSON snapshot of every provider's models
//! published as a release artifact and consumed by the Gateway and the
//! Workshop UI; the model metadata types; and the [`Progress`] snapshot the
//! Gateway streams to its status consumers.
//!
//! This crate is pure vocabulary: it depends only on `serde` and `time` and
//! on no other workspace crate, so every product crate may depend on it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

mod metadata;
pub mod progress;

pub use metadata::{Capabilities, ModelInfo, ModelKind, ThinkingMode};
pub use progress::Progress;

/// The sheet schema version this reader accepts: the writer in
/// `gateway-cloud-providers` stamps it and the gateway reader gates on it.
/// Bump only on removals and renames; additive fields carry
/// `#[serde(default)]` so a lagging reader survives them.
pub const ACCEPTED_SHEET_SCHEMA_VERSION: u32 = 1;

/// The sheet envelope: one atomic snapshot of every provider's models.
/// Future additive fields carry `#[serde(default)]` so a lagging reader
/// survives them; `schema_version` bumps only on removals and renames.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sheet {
    /// Bumped on breaking change. Stays 1: additive fields carry
    /// `#[serde(default)]`; the bump is reserved for removals and renames.
    pub schema_version: u32,
    /// RFC 3339; always this run's time.
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    /// Keyed by provider name, e.g. "anthropic".
    pub providers: BTreeMap<String, ProviderSlice>,
}

/// One provider's slice of the sheet. Self-describing: the descriptor's
/// public fields are copied in at build time so consumers can render a
/// provider dropdown from the sheet alone. Future additive fields carry
/// `#[serde(default)]`; the schema version bump is reserved for removals
/// and renames.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSlice {
    /// UI-facing name, e.g. "Anthropic".
    pub display_name: String,
    /// Curated product opinion, not a vendor fact.
    pub tier: Tier,
    /// Freshness of this slice.
    pub status: SliceStatus,
    /// Last fresh fetch; absent for `static` slices.
    #[serde(with = "time::serde::rfc3339::option")]
    pub fetched_at: Option<OffsetDateTime>,
    /// The base URL of the provider's OpenAI-compatible chat API - the
    /// value an `[[endpoint]]` needs - or `None` when the provider has
    /// no such API.
    pub openai_base_url: Option<String>,
    /// The provider's environment variables, copied from the descriptor
    /// at build time so a consumer can render from the sheet alone.
    pub env_vars: Vec<EnvVar>,
    /// The provider's normalized model entries.
    pub models: Vec<ModelEntry>,
}

/// One environment variable a provider reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvVar {
    /// The variable name, e.g. "ANTHROPIC_API_KEY".
    pub name: String,
    /// How the variable is used.
    pub role: EnvRole,
    /// The value the provider assumes when the variable is unset.
    pub default: Option<String>,
}

/// How a provider uses an environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum EnvRole {
    /// A credential: the variable carries API key material.
    Key,
    /// Configuration: the variable selects a region, endpoint, or similar.
    Config,
}

/// Curated product opinion, not a vendor fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Tier {
    /// The frontier providers.
    Prime,
    /// Credible challengers.
    Subprime,
    /// Specialized or regional providers.
    Niche,
    /// Resellers of other providers' models.
    Aggregator,
}

/// Freshness of one provider's slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SliceStatus {
    /// Fetched fresh this run.
    Ok,
    /// Copied verbatim from the previous sheet after a failed fetch.
    Stale,
    /// No previous slice and the fetch failed; `models` is empty.
    Unavailable,
    /// Curated by hand; never fetched.
    Static,
}

/// One normalized model entry. Future additive fields carry
/// `#[serde(default)]`; the schema version bump is reserved for removals
/// and renames.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the modality and capability booleans are the sheet schema itself; a builder or sub-struct would only obscure the wire shape"
)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// The upstream slug.
    pub id: String,
    /// UI-facing name.
    pub display_name: String,
    /// The UI's first grouping level, e.g. "claude-opus", "gpt-5.4".
    pub family: String,
    /// The canonical alias this entry is a dated snapshot or SKU variant
    /// of; set only when that alias exists in the same provider's list.
    pub variant_of: Option<String>,
    /// The stripped suffix, e.g. `2026-03-05`, `free`, `batch`, for the
    /// UI's expander label.
    pub variant: Option<String>,
    /// The provider's own language codes, passed through as reported;
    /// empty when unknown or not applicable.
    pub languages: Vec<String>,
    /// The workload: chat, embedding, classifier, speech (TTS),
    /// transcription (STT), image, video.
    #[serde(default)]
    pub kind: ModelKind,
    /// Optional: not every provider reports it.
    pub released_at: Option<Date>,
    /// Optional: the IDs-only providers omit it.
    pub context_window: Option<u32>,
    /// Optional maximum completion tokens.
    pub max_output: Option<u32>,
    // Modalities.
    /// Accepts image input.
    pub images: bool,
    /// Accepts PDF input.
    pub pdf_input: bool,
    /// Accepts video input.
    pub video_input: bool,
    /// Accepts audio input.
    pub audio_input: bool,
    // Capabilities.
    /// Supports batch submission.
    pub batch: bool,
    /// Returns grounded citations.
    pub citations: bool,
    /// Can execute code server-side.
    pub code_execution: bool,
    /// Honors response schemas.
    pub structured_outputs: bool,
    /// Emits tool calls.
    pub tool_calling: bool,
    /// Reasoning capability.
    pub thinking: Thinking,
    /// The provider's own level names, e.g. `["low", "high", "max"]`;
    /// never mapped to a cross-provider scale.
    pub effort_levels: Vec<String>,
    /// The provider's own default level name.
    pub default_effort: Option<String>,
    /// Normalized to per-million-token units.
    pub pricing: Option<Pricing>,
    /// Sunset information, when the provider reports it.
    pub deprecation: Option<Deprecation>,
}

/// Reasoning capability.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Thinking {
    /// Any reasoning capability at all.
    pub supported: bool,
    /// Manual budget mode (Anthropic "enabled").
    pub enabled: bool,
    /// Model-chosen thinking depth.
    pub adaptive: bool,
}

/// Token pricing, normalized to per-million-token units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pricing {
    /// ISO 4217, e.g. "USD", "CNY".
    pub currency: String,
    /// Prompt price per million tokens.
    pub prompt_per_mtok: f64,
    /// Completion price per million tokens.
    pub completion_per_mtok: f64,
}

/// Sunset information, when the provider reports it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deprecation {
    /// Provider's own lifecycle label, e.g. "LEGACY".
    pub status: String,
    /// The sunset date, when known.
    pub date: Option<Date>,
    /// The successor model id, when named.
    pub replacement: Option<String>,
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;

    use super::*;

    /// The example sheet from the implementation contract, verbatim.
    const CONTRACT_EXAMPLE: &str = r#"{
  "schema_version": 1,
  "generated_at": "2026-09-14T13:00:00Z",
  "providers": {
    "anthropic": {
      "display_name": "Anthropic",
      "tier": "prime",
      "status": "ok",
      "fetched_at": "2026-09-14T13:00:00Z",
      "openai_base_url": "https://api.anthropic.com/v1",
      "env_vars": [
        { "name": "ANTHROPIC_API_KEY", "role": "key", "default": null }
      ],
      "models": [
        {
          "id": "claude-opus-5",
          "display_name": "Claude Opus 5",
          "family": "claude-opus",
          "variant_of": null,
          "variant": null,
          "languages": [],
          "released_at": "2026-07-24",
          "context_window": 1000000,
          "max_output": 128000,
          "images": true,
          "pdf_input": true,
          "video_input": false,
          "audio_input": false,
          "batch": true,
          "citations": true,
          "code_execution": true,
          "structured_outputs": true,
          "tool_calling": true,
          "thinking": { "supported": true, "enabled": false, "adaptive": true },
          "effort_levels": ["low", "medium", "high", "xhigh", "max"],
          "default_effort": "high",
          "pricing": { "currency": "USD", "prompt_per_mtok": 5.0, "completion_per_mtok": 25.0 },
          "deprecation": null
        }
      ]
    }
  }
}"#;

    fn entry(id: &str) -> ModelEntry {
        ModelEntry {
            id: id.to_owned(),
            display_name: id.to_owned(),
            family: "test-family".to_owned(),
            variant_of: None,
            variant: None,
            languages: Vec::new(),
            kind: ModelKind::Chat,
            released_at: None,
            context_window: Some(200_000),
            max_output: Some(8_192),
            images: false,
            pdf_input: false,
            video_input: false,
            audio_input: false,
            batch: false,
            citations: false,
            code_execution: false,
            structured_outputs: true,
            tool_calling: true,
            thinking: Thinking::default(),
            effort_levels: vec!["low".to_owned(), "high".to_owned()],
            default_effort: None,
            pricing: None,
            deprecation: None,
        }
    }

    fn slice(name: &str, models: Vec<ModelEntry>) -> ProviderSlice {
        ProviderSlice {
            display_name: name.to_owned(),
            tier: Tier::Prime,
            status: SliceStatus::Ok,
            fetched_at: None,
            openai_base_url: None,
            env_vars: Vec::new(),
            models,
        }
    }

    fn sheet_with(names: &[&str]) -> Sheet {
        Sheet {
            schema_version: 1,
            generated_at: OffsetDateTime::parse("2026-09-14T13:00:00Z", &Rfc3339)
                .expect("pinned timestamp must parse"),
            providers: names
                .iter()
                .map(|name| ((*name).to_owned(), slice(name, vec![entry("m1")])))
                .collect(),
        }
    }

    #[test]
    fn schema_round_trip() {
        let sheet = sheet_with(&["anthropic", "openai"]);
        let line = serde_json::to_string(&sheet).expect("sheet must serialize");
        let back: Sheet = serde_json::from_str(&line).expect("its own output must parse");
        let line2 = serde_json::to_string(&back).expect("parsed sheet must re-serialize");
        assert_eq!(line, line2, "round-trip must be lossless");
        assert_eq!(back.schema_version, 1);
        assert_eq!(back.providers["anthropic"].models[0].id, "m1");
    }

    #[test]
    fn provider_ordering_is_byte_deterministic() {
        // Insertion order is not sorted; the emitted bytes must be.
        let sheet = sheet_with(&["openai", "anthropic", "gemini"]);
        let line = serde_json::to_string(&sheet).expect("sheet must serialize");
        let anthropic = line.find("\"anthropic\"").expect("key must appear");
        let gemini = line.find("\"gemini\"").expect("key must appear");
        let openai = line.find("\"openai\"").expect("key must appear");
        assert!(
            anthropic < gemini && gemini < openai,
            "provider keys must serialize in sorted order: {line}"
        );
        let again = serde_json::to_string(&sheet).expect("sheet must serialize twice");
        assert_eq!(line, again, "repeated serialization must be identical");
    }

    #[test]
    fn generated_at_serializes_as_rfc3339_with_z() {
        let sheet = sheet_with(&[]);
        let line = serde_json::to_string(&sheet).expect("sheet must serialize");
        assert!(
            line.contains("\"generated_at\":\"2026-09-14T13:00:00Z\""),
            "generated_at must be RFC 3339 with a literal Z: {line}"
        );
    }

    #[test]
    fn new_fields_round_trip_losslessly() {
        let mut model = entry("claude-fable-5-1-2026-09-01");
        model.family = "claude-fable".to_owned();
        model.variant_of = Some("claude-fable-5-1".to_owned());
        model.variant = Some("2026-09-01".to_owned());
        model.languages = vec!["en".to_owned(), "fr".to_owned()];
        let mut slice = slice("anthropic", vec![model]);
        slice.openai_base_url = Some("https://api.anthropic.com/v1".to_owned());
        slice.env_vars = vec![
            EnvVar {
                name: "ANTHROPIC_API_KEY".to_owned(),
                role: EnvRole::Key,
                default: None,
            },
            EnvVar {
                name: "ANTHROPIC_REGION".to_owned(),
                role: EnvRole::Config,
                default: Some("us-east-1".to_owned()),
            },
        ];
        let sheet = Sheet {
            schema_version: 1,
            generated_at: OffsetDateTime::parse("2026-09-14T13:00:00Z", &Rfc3339)
                .expect("pinned timestamp must parse"),
            providers: BTreeMap::from([("anthropic".to_owned(), slice)]),
        };
        let line = serde_json::to_string(&sheet).expect("sheet must serialize");
        let back: Sheet = serde_json::from_str(&line).expect("its own output must parse");
        let slice = &back.providers["anthropic"];
        assert_eq!(
            slice.openai_base_url.as_deref(),
            Some("https://api.anthropic.com/v1"),
            "openai_base_url must survive the round trip"
        );
        assert_eq!(slice.env_vars.len(), 2);
        assert_eq!(slice.env_vars[0].name, "ANTHROPIC_API_KEY");
        assert_eq!(slice.env_vars[0].role, EnvRole::Key);
        assert_eq!(slice.env_vars[0].default, None);
        assert_eq!(slice.env_vars[1].name, "ANTHROPIC_REGION");
        assert_eq!(slice.env_vars[1].role, EnvRole::Config);
        assert_eq!(slice.env_vars[1].default.as_deref(), Some("us-east-1"));
        let model = &slice.models[0];
        assert_eq!(model.family, "claude-fable");
        assert_eq!(model.variant_of.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(model.variant.as_deref(), Some("2026-09-01"));
        assert_eq!(model.languages, ["en", "fr"]);
    }

    #[test]
    fn missing_new_fields_fail_to_parse() {
        // The new fields are required: no serde defaults, so an old-shape
        // document must be rejected rather than silently defaulted.
        let mut value = serde_json::to_value(entry("m1")).expect("entry must serialize");
        value
            .as_object_mut()
            .expect("an entry serializes as an object")
            .remove("family");
        assert!(
            serde_json::from_value::<ModelEntry>(value).is_err(),
            "an entry without `family` must fail to parse"
        );
        let mut value =
            serde_json::to_value(slice("anthropic", vec![])).expect("slice must serialize");
        value
            .as_object_mut()
            .expect("a slice serializes as an object")
            .remove("env_vars");
        assert!(
            serde_json::from_value::<ProviderSlice>(value).is_err(),
            "a slice without `env_vars` must fail to parse"
        );
    }

    #[test]
    fn contract_example_parses() {
        let sheet: Sheet =
            serde_json::from_str(CONTRACT_EXAMPLE).expect("contract example must parse");
        assert_eq!(sheet.schema_version, 1);
        let anthropic = &sheet.providers["anthropic"];
        assert_eq!(anthropic.display_name, "Anthropic");
        assert_eq!(anthropic.tier, Tier::Prime);
        assert_eq!(anthropic.status, SliceStatus::Ok);
        let model = &anthropic.models[0];
        assert_eq!(model.id, "claude-opus-5");
        assert_eq!(model.kind, ModelKind::Chat, "absent kind defaults to chat");
        assert_eq!(model.max_output, Some(128_000));
        assert!(model.thinking.adaptive);
        assert!(!model.thinking.enabled);
        assert_eq!(model.effort_levels.len(), 5);
        assert!(model.deprecation.is_none());
        let pricing = model.pricing.as_ref().expect("pricing must parse");
        assert_eq!(pricing.currency, "USD");
    }
}
