//! Tiered descriptors for cloud model providers, the provider registry,
//! and the fetch seam behind which each provider's private variance lives.
//!
//! One Rust file per provider; each file defines a public descriptor while
//! auth header shape, pagination, and response mapping stay private to the
//! file. The crate does double duty: a library linked into the Gateway, and
//! a binary the aggregation workflow compiles and runs.

use shared_gateway_api::{ModelEntry, Tier};

pub mod providers;
mod sheet;

pub use sheet::{build_sheet, fetch_sheet};

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
}

/// Every known provider.
#[must_use]
pub fn providers() -> &'static [Provider] {
    &[
        providers::anthropic::PROVIDER,
        providers::baidu::PROVIDER,
        providers::cohere::PROVIDER,
        providers::deepgram::PROVIDER,
        providers::deepseek::PROVIDER,
        providers::elevenlabs::PROVIDER,
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
pub enum FetchError {
    /// The registry has no fetch implementation for this provider.
    #[error("no fetch implementation for provider `{name}`")]
    UnsupportedProvider {
        /// The provider registry key.
        name: String,
    },
    /// The HTTP request to the provider failed.
    #[error("provider request failed: {0}")]
    Http(#[from] reqwest::Error),
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

/// Fetch and normalize one provider's model list; the per-provider
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
        "baidu" => providers::baidu::fetch(client, provider.base_url, key).await,
        "cohere" => providers::cohere::fetch(client, provider.base_url, key).await,
        "deepgram" => providers::deepgram::fetch(client, provider.base_url, key).await,
        "deepseek" => providers::deepseek::fetch(client, provider.base_url, key).await,
        "elevenlabs" => providers::elevenlabs::fetch(client, provider.base_url, key).await,
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

    use shared_gateway_api::Tier;

    use super::{FetchError, Provider, fetch_models, providers};

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
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
        ),
        ("moonshot", "Moonshot AI", "https://api.moonshot.ai/v1"),
        ("meta", "Meta", "https://api.meta.ai/v1"),
        ("elevenlabs", "ElevenLabs", "https://api.elevenlabs.io"),
        ("deepgram", "Deepgram", "https://api.deepgram.com"),
    ];

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
    fn all_prime_providers_are_registered() {
        let registered: BTreeSet<&str> = providers().iter().map(|provider| provider.name).collect();
        for &(name, ..) in PRIME {
            assert!(
                registered.contains(name),
                "prime provider `{name}` from the decision record is not registered"
            );
            let provider = providers()
                .iter()
                .find(|provider| provider.name == name)
                .expect("checked above");
            assert_eq!(
                provider.tier,
                Tier::Prime,
                "decision-record provider `{name}` must be Tier::Prime"
            );
        }
    }

    #[test]
    fn prime_descriptors_match_decision_record() {
        for provider in providers() {
            if provider.tier != Tier::Prime {
                continue;
            }
            let Some(&(_, display_name, base_url)) =
                PRIME.iter().find(|(name, ..)| *name == provider.name)
            else {
                panic!(
                    "prime provider `{}` is not in the decision record",
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
            assert!(
                provider.key_env.is_some_and(|key_env| !key_env.is_empty()),
                "prime provider `{}` must name its key env var",
                provider.name
            );
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
}
