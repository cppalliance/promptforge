//! Tests for the provider registry, the fetch dispatch, and error chain rendering.

use std::collections::BTreeSet;

use gateway_api_types::{ModelEntry, Tier};

use super::{FetchError, HttpSource, Provider, error_chain, fetch_models, providers};

#[test]
fn the_http_variant_reaches_the_transport_error_through_the_shared_wrapper() {
    let Err(transport) = reqwest::Proxy::all("http://") else {
        panic!("a proxy URL with an empty host must not build");
    };
    let error = FetchError::from(transport);
    let Some(cause) = std::error::Error::source(&error) else {
        panic!("the http variant returns its transport cause from source()");
    };
    let Some(wrapper) = cause.downcast_ref::<HttpSource>() else {
        panic!("the transport cause is the shared HttpSource");
    };
    assert!(wrapper.as_inner().is_builder());
}

/// Applies one provider's private taxonomy rules to a list of
/// entries, by registry name. The production path applies the rules
/// inside each provider's fetch; this dispatch lets the registry
/// tests apply them to fixture entries.
fn apply_provider_taxonomy(name: &str, entries: &mut [ModelEntry]) -> bool {
    match name {
        "anthropic" => crate::providers::anthropic::taxonomy::apply(entries),
        "azure_speech" => crate::providers::azure_speech::apply_taxonomy(entries),
        "baidu" => crate::providers::baidu::apply_taxonomy(entries),
        "bedrock" => crate::providers::bedrock::taxonomy::apply(entries),
        "cohere" => crate::providers::cohere::apply_taxonomy(entries),
        "deepgram" => crate::providers::deepgram::apply_taxonomy(entries),
        "deepseek" => crate::providers::deepseek::apply_taxonomy(entries),
        "elevenlabs" => crate::providers::elevenlabs::apply_taxonomy(entries),
        "foundry" => crate::providers::foundry::taxonomy::apply(entries),
        "gemini" => crate::providers::gemini::apply_taxonomy(entries),
        "groq" => crate::providers::groq::apply_taxonomy(entries),
        "leonardo" => crate::providers::leonardo::apply_taxonomy(entries),
        "meta" => crate::providers::meta::apply_taxonomy(entries),
        "minimax" => crate::providers::minimax::apply_taxonomy(entries),
        "mistral" => crate::providers::mistral::taxonomy::apply(entries),
        "moonshot" => crate::providers::moonshot::apply_taxonomy(entries),
        "nvidia" => crate::providers::nvidia::apply_taxonomy(entries),
        "openai" => crate::providers::openai::apply_taxonomy(entries),
        "openrouter" => crate::providers::openrouter::taxonomy::apply(entries),
        "qwen" => crate::providers::qwen::apply_taxonomy(entries),
        "soniox" => crate::providers::soniox::apply_taxonomy(entries),
        "stepfun" => crate::providers::stepfun::apply_taxonomy(entries),
        "xai" => crate::providers::xai::apply_taxonomy(entries),
        _ => return false,
    }
    true
}

/// The 2026-09-14 fixture excerpts, one per provider that has one.
const FIXTURES: &[(&str, &str)] = &[
    (
        "anthropic",
        include_str!("../tests/fixtures/2026-09-14-anthropic.json"),
    ),
    (
        "cohere",
        include_str!("../tests/fixtures/2026-09-14-cohere.json"),
    ),
    (
        "deepgram",
        include_str!("../tests/fixtures/2026-09-14-deepgram.json"),
    ),
    (
        "deepseek",
        include_str!("../tests/fixtures/2026-09-14-deepseek.json"),
    ),
    (
        "gemini",
        include_str!("../tests/fixtures/2026-09-14-gemini.json"),
    ),
    (
        "meta",
        include_str!("../tests/fixtures/2026-09-14-meta.json"),
    ),
    (
        "mistral",
        include_str!("../tests/fixtures/2026-09-14-mistral.json"),
    ),
    (
        "moonshot",
        include_str!("../tests/fixtures/2026-09-14-moonshot.json"),
    ),
    (
        "nvidia",
        include_str!("../tests/fixtures/2026-09-14-nvidia.json"),
    ),
    (
        "openai",
        include_str!("../tests/fixtures/2026-09-14-openai.json"),
    ),
    (
        "openrouter",
        include_str!("../tests/fixtures/2026-09-14-openrouter.json"),
    ),
    (
        "qwen",
        include_str!("../tests/fixtures/2026-09-14-qwen.json"),
    ),
    ("xai", include_str!("../tests/fixtures/2026-09-14-xai.json")),
];

#[test]
fn every_fixture_entry_has_a_family() {
    for &(name, json) in FIXTURES {
        assert!(
            providers().iter().any(|provider| provider.name == name),
            "fixture provider `{name}` is not registered"
        );
        let mut entries = crate::taxonomy::fixture::entries(json);
        assert!(
            apply_provider_taxonomy(name, &mut entries),
            "fixture provider `{name}` is not in the taxonomy dispatch"
        );
        for entry in &entries {
            assert!(
                !entry.family.is_empty(),
                "{name}: {} has an empty family",
                entry.id
            );
        }
    }
}

/// The Prime tier settled in the decision record (2026-09-14):
/// `(name, display_name, base_url)`.
const PRIME: &[(&str, &str, &str)] = &[
    ("anthropic", "Anthropic", "https://api.anthropic.com"),
    ("openai", "OpenAI", "https://api.openai.com/v1"),
    (
        "gemini",
        "Google Gemini",
        "https://generativelanguage.googleapis.com",
    ),
    ("xai", "xAI", "https://api.x.ai"),
    ("deepseek", "DeepSeek", "https://api.deepseek.com"),
    (
        "qwen",
        "Alibaba Qwen",
        "https://dashscope-intl.aliyuncs.com/compatible-mode/v1",
    ),
    ("moonshot", "Moonshot AI", "https://api.moonshot.ai/v1"),
    ("meta", "Meta", "https://api.meta.ai/v1"),
    ("elevenlabs", "ElevenLabs", "https://api.elevenlabs.io"),
    ("deepgram", "Deepgram", "https://api.deepgram.com"),
];

/// The Subprime tier settled in the decision record (2026-09-14):
/// `(name, display_name, base_url)`.
const SUBPRIME: &[(&str, &str, &str)] = &[
    ("mistral", "Mistral AI", "https://api.mistral.ai"),
    ("cohere", "Cohere", "https://api.cohere.com"),
    ("baidu", "Baidu", "https://qianfan.baidubce.com"),
    ("minimax", "MiniMax", "https://api.minimax.io/v1"),
    ("stepfun", "StepFun", "https://api.stepfun.ai/v1"),
    (
        "bedrock",
        "Amazon Bedrock",
        "https://bedrock.us-east-1.amazonaws.com",
    ),
    (
        "foundry",
        "Azure AI Foundry",
        "https://api.catalog.azureml.ms",
    ),
    ("nvidia", "NVIDIA", "https://integrate.api.nvidia.com/v1"),
    ("groq", "Groq", "https://api.groq.com/openai/v1"),
    ("soniox", "Soniox", "https://api.soniox.com"),
    (
        "azure_speech",
        "Azure Speech",
        "https://{region}.cognitiveservices.azure.com",
    ),
    (
        "leonardo",
        "Leonardo",
        "https://cloud.leonardo.ai/api/rest/v1",
    ),
];

/// The Aggregator tier settled in the decision record (2026-09-14):
/// `(name, display_name, base_url)`.
const AGGREGATOR: &[(&str, &str, &str)] = &[("openrouter", "OpenRouter", "https://openrouter.ai")];

/// The keyless providers settled in the decision record: fetched
/// with no credential, so `key_env` must be `None`. Foundry joined
/// them when its slice moved from the per-resource deployment list
/// to the global catalog endpoint.
const KEYLESS: &[&str] = &["foundry", "nvidia", "openrouter"];

/// The full decision record: `(name, display_name, base_url, tier)`.
fn decision_record() -> impl Iterator<Item = (&'static str, &'static str, &'static str, Tier)> {
    PRIME
        .iter()
        .map(|&(name, display, url)| (name, display, url, Tier::Prime))
        .chain(
            SUBPRIME
                .iter()
                .map(|&(name, display, url)| (name, display, url, Tier::Subprime)),
        )
        .chain(
            AGGREGATOR
                .iter()
                .map(|&(name, display, url)| (name, display, url, Tier::Aggregator)),
        )
}

#[test]
fn registry_names_are_unique() {
    let mut seen = BTreeSet::new();
    for provider in providers() {
        assert!(
            seen.insert(provider.name),
            "duplicate provider name in the registry: {}",
            provider.name
        );
    }
}

#[test]
fn registry_key_envs_are_unique() {
    let mut seen = BTreeSet::new();
    for provider in providers() {
        let Some(key_env) = provider.key_env else {
            continue;
        };
        assert!(
            seen.insert(key_env),
            "duplicate key env in the registry: {key_env}"
        );
    }
}

#[test]
fn all_decision_record_providers_are_registered() {
    let registered: BTreeSet<&str> = providers().iter().map(|provider| provider.name).collect();
    for (name, _, _, tier) in decision_record() {
        assert!(
            registered.contains(name),
            "decision-record provider `{name}` is not registered"
        );
        let provider = providers()
            .iter()
            .find(|provider| provider.name == name)
            .expect("checked above");
        assert_eq!(
            provider.tier, tier,
            "decision-record provider `{name}` must be {tier:?}"
        );
    }
}

#[test]
fn descriptors_match_decision_record() {
    for provider in providers() {
        let Some((_, display_name, base_url, _)) =
            decision_record().find(|(name, ..)| *name == provider.name)
        else {
            panic!(
                "registered provider `{}` is not in the decision record",
                provider.name
            );
        };
        assert_eq!(
            provider.display_name, display_name,
            "display name for `{}`",
            provider.name
        );
        assert_eq!(
            provider.base_url, base_url,
            "base URL for `{}`",
            provider.name
        );
        if KEYLESS.contains(&provider.name) {
            assert!(
                provider.key_env.is_none(),
                "keyless provider `{}` must not name a key env var",
                provider.name
            );
        } else {
            assert!(
                provider.key_env.is_some_and(|key_env| !key_env.is_empty()),
                "keyed provider `{}` must name its key env var",
                provider.name
            );
        }
    }
}

#[tokio::test]
async fn fetch_models_rejects_unsupported_provider() {
    let provider = Provider {
        name: "no-such-provider",
        display_name: "No Such Provider",
        tier: Tier::Niche,
        key_env: Some("NO_SUCH_PROVIDER_API_KEY"),
        base_url: "https://example.invalid",
        openai_base_url: None,
        env_vars: &[],
    };
    let client = reqwest::Client::new();
    let Err(err) = fetch_models(&client, &provider, Some("test-key")).await else {
        panic!("a provider with no fetch implementation must not succeed");
    };
    assert!(
        matches!(err, FetchError::UnsupportedProvider { .. }),
        "expected UnsupportedProvider, got {err:?}"
    );
    assert!(
        err.to_string().contains("no-such-provider"),
        "the error must name the provider: {err}"
    );
}

/// A leaf cause with its own text.
#[derive(Debug, thiserror::Error)]
#[error("connection reset")]
struct Leaf;

/// An outer error that copies its cause's text into its own message.
#[derive(Debug, thiserror::Error)]
#[error("model sheet unavailable: {message}")]
struct Copying {
    message: String,
    #[source]
    source: Leaf,
}

/// A cause that renders as an empty string.
#[derive(Debug, thiserror::Error)]
#[error("")]
struct Silent;

/// An outer error whose cause renders as an empty string.
#[derive(Debug, thiserror::Error)]
#[error("model sheet unavailable")]
struct OverSilent(#[source] Silent);

#[test]
fn a_cause_the_outer_message_already_contains_renders_once() {
    let error = Copying {
        message: "connection reset".to_owned(),
        source: Leaf,
    };
    let rendered = error_chain(&error);
    assert_eq!(
        rendered, "model sheet unavailable: connection reset",
        "a cause whose text the outer message already contains is skipped"
    );
    assert_eq!(
        rendered.matches("connection reset").count(),
        1,
        "the cause text appears exactly once"
    );
    assert_eq!(
        error_chain(&OverSilent(Silent)),
        "model sheet unavailable",
        "a cause that renders as nothing adds no trailing separator"
    );
}
