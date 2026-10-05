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
/// descriptor stores `&'static str` and slice construction converts.
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
        /// The environment variable that would hold the key.
        key_env: &'static str,
    },
}

impl From<reqwest::Error> for FetchError {
    fn from(source: reqwest::Error) -> Self {
        FetchError::Http(HttpSource::from(source))
    }
}

/// Renders an error and its full `source()` chain as one line, each cause
/// separated by `; `. A variant's `Display` renders only its own message,
/// so this is how a person-facing note recovers the transport or decode
/// text underneath.
///
/// A cause that renders as an empty string, and a cause whose text the
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
#[path = "lib-tests.rs"]
mod tests;
