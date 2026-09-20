//! Tiered descriptors for cloud model providers, the provider registry,
//! and the fetch seam behind which each provider's private variance lives.
//!
//! One Rust file per provider; each file defines a public descriptor while
//! auth header shape, pagination, and response mapping stay private to the
//! file. The crate does double duty: a library linked into the Gateway, and
//! a binary the aggregation workflow compiles and runs.

use gateway_api_types::{EnvRole, ModelEntry, Tier};

pub mod providers;
mod sheet;
mod taxonomy;

pub use sheet::{build_sheet, fetch_sheet};

// The transport cause behind `FetchError::Http`. A caller that needs the
// transport error itself names `shared_error_source` directly; this crate does
// not re-export the wrapper, so there is one name for the cause across the
// workspace rather than one per crate.
use shared_error_source::HttpSource;

/// The const-friendly twin of the schema's `EnvVar`: the schema type
/// holds `String`s and cannot sit in a `const` descriptor, so the
/// descriptor carries `&'static str` and slice construction converts.
#[derive(Debug, Clone, Copy)]
pub struct EnvVarSpec {
    /// The variable name, e.g. "ANTHROPIC_API_KEY".
    pub name: &'static str,
    /// How the variable is used.
    pub role: EnvRole,
    /// The value the provider assumes when the variable is unset.
    pub default: Option<&'static str>,
}

/// The public descriptor for one provider. Everything else about the
/// provider - auth header shape, pagination, response mapping - is
/// private to its file.
#[derive(Debug, Clone, Copy)]
pub struct Provider {
    /// Registry key, e.g. "anthropic".
    pub name: &'static str,
    /// UI-facing name, e.g. "Anthropic".
    pub display_name: &'static str,
    /// Curated product opinion, not a vendor fact.
    pub tier: Tier,
    /// Environment variable the API key arrives under; matches the
    /// GitHub secret name. `None` marks a keyless provider: it is
    /// fetched with no credential and never produces
    /// [`FetchError::MissingKey`].
    pub key_env: Option<&'static str>,
    /// Default base URL for the model-list endpoint.
    pub base_url: &'static str,
    /// The base URL of the provider's OpenAI-compatible chat API - the
    /// value an `[[endpoint]]` needs - or `None` when the provider has
    /// no such API. Distinct from `base_url`, which is the model-list
    /// endpoint's base and stays private to the fetch.
    pub openai_base_url: Option<&'static str>,
    /// Every environment variable the provider reads, key-role and
    /// config-role; copied into the provider's slice at build time.
    /// `key_env` stays the single key-role entry the binary passes to
    /// `fetch_models`; the provider file reads any further entries
    /// privately.
    pub env_vars: &'static [EnvVarSpec],
}

/// Every known provider.
#[must_use]
pub fn providers() -> &'static [Provider] {
    &[
        providers::anthropic::PROVIDER,
        providers::azure_speech::PROVIDER,
        providers::baidu::PROVIDER,
        providers::bedrock::PROVIDER,
        providers::cohere::PROVIDER,
        providers::deepgram::PROVIDER,
        providers::deepseek::PROVIDER,
        providers::elevenlabs::PROVIDER,
        providers::foundry::PROVIDER,
        providers::gemini::PROVIDER,
        providers::groq::PROVIDER,
        providers::leonardo::PROVIDER,
        providers::meta::PROVIDER,
        providers::minimax::PROVIDER,
        providers::mistral::PROVIDER,
        providers::moonshot::PROVIDER,
        providers::nvidia::PROVIDER,
        providers::openai::PROVIDER,
        providers::openrouter::PROVIDER,
        providers::qwen::PROVIDER,
        providers::soniox::PROVIDER,
        providers::stepfun::PROVIDER,
        providers::xai::PROVIDER,
    ]
}

/// A failed provider fetch or sheet download. Never fatal to a sheet
/// build: the caller propagates last-known-good data instead.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// The registry has no fetch implementation for this provider.
    #[error("no fetch implementation for provider `{name}`")]
    UnsupportedProvider {
        /// The provider registry key.
        name: String,
    },
    /// The HTTP request to the provider failed. The transport cause is
    /// the `source()`; renderers walk the chain for it.
    #[error("the provider request did not complete")]
    Http(#[source] HttpSource),
    /// The previous release's sheet URL answered HTTP 404: the release
    /// does not exist yet.
    #[error("no sheet at `{url}` (HTTP 404)")]
    NotFound {
        /// The release URL that answered 404.
        url: String,
    },
    /// The provider's API key is not available in the environment.
    #[error("missing API key for provider `{name}`: environment variable `{key_env}` is not set")]
    MissingKey {
        /// The provider registry key.
        name: String,
        /// The environment variable that would carry the key.
        key_env: &'static str,
    },
}

impl From<reqwest::Error> for FetchError {
    fn from(source: reqwest::Error) -> Self {
        FetchError::Http(HttpSource::from(source))
    }
}

/// Renders an error and its full `source()` chain as one line, each cause
/// separated by `; `. A variant's `Display` carries only its own message,
/// so this is how a person-facing note recovers the transport or decode
/// text underneath.
///
/// A cause that renders as nothing, and a cause whose text the
/// accumulated rendering already contains, are both skipped: some
/// variants copy their source's text into their own message, and
/// appending that cause again would print it twice. The check is a plain
/// substring test on the text rendered so far.
#[must_use]
pub fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !cause_text.is_empty() && !text.contains(&cause_text) {
            text.push_str("; ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Fetches and normalizes one provider's model list; the per-provider
/// variance lives behind this seam. The client is injected by the
/// caller (the Gateway's bounded client, or the binary's own).
///
/// # Errors
///
/// Returns [`FetchError::UnsupportedProvider`] when the registry has no
/// fetch implementation for the provider, [`FetchError::MissingKey`]
/// when a keyed provider is fetched with no key, and
/// [`FetchError::Http`] when the provider request fails.
pub async fn fetch_models(
    client: &reqwest::Client,
    provider: &Provider,
    key: Option<&str>,
) -> Result<Vec<ModelEntry>, FetchError> {
    match provider.name {
        "anthropic" => providers::anthropic::fetch(client, provider.base_url, key).await,
        "azure_speech" => providers::azure_speech::fetch(client, provider.base_url, key).await,
        "baidu" => providers::baidu::fetch(client, provider.base_url, key).await,
        "bedrock" => providers::bedrock::fetch(client, provider.base_url, key).await,
        "cohere" => providers::cohere::fetch(client, provider.base_url, key).await,
        "deepgram" => providers::deepgram::fetch(client, provider.base_url, key).await,
        "deepseek" => providers::deepseek::fetch(client, provider.base_url, key).await,
        "elevenlabs" => providers::elevenlabs::fetch(client, provider.base_url, key).await,
        "foundry" => providers::foundry::fetch(client, provider.base_url, key).await,
        "gemini" => providers::gemini::fetch(client, provider.base_url, key).await,
        "groq" => providers::groq::fetch(client, provider.base_url, key).await,
        "leonardo" => providers::leonardo::fetch(client, provider.base_url, key).await,
        "meta" => providers::meta::fetch(client, provider.base_url, key).await,
        "minimax" => providers::minimax::fetch(client, provider.base_url, key).await,
        "mistral" => providers::mistral::fetch(client, provider.base_url, key).await,
        "moonshot" => providers::moonshot::fetch(client, provider.base_url, key).await,
        "nvidia" => providers::nvidia::fetch(client, provider.base_url, key).await,
        "openai" => providers::openai::fetch(client, provider.base_url, key).await,
        "openrouter" => providers::openrouter::fetch(client, provider.base_url, key).await,
        "qwen" => providers::qwen::fetch(client, provider.base_url, key).await,
        "soniox" => providers::soniox::fetch(client, provider.base_url, key).await,
        "stepfun" => providers::stepfun::fetch(client, provider.base_url, key).await,
        "xai" => providers::xai::fetch(client, provider.base_url, key).await,
        _ => Err(FetchError::UnsupportedProvider {
            name: provider.name.to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
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
            panic!("the http variant carries its transport cause as source()");
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
    const AGGREGATOR: &[(&str, &str, &str)] =
        &[("openrouter", "OpenRouter", "https://openrouter.ai")];

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

    /// A cause that renders as nothing.
    #[derive(Debug, thiserror::Error)]
    #[error("")]
    struct Silent;

    /// An outer error whose cause renders as nothing.
    #[derive(Debug, thiserror::Error)]
    #[error("model sheet unavailable")]
    struct OverSilent(#[source] Silent);

    #[test]
    fn a_cause_the_outer_message_already_carries_renders_once() {
        let error = Copying {
            message: "connection reset".to_owned(),
            source: Leaf,
        };
        let rendered = error_chain(&error);
        assert_eq!(
            rendered, "model sheet unavailable: connection reset",
            "a cause whose text the outer message already carries is skipped"
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
}
