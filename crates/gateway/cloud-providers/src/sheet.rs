//! Sheet assembly and download: `build_sheet` walks the provider
//! registry, fetching each provider and propagating last-known-good
//! slices for failed fetches; `fetch_sheet` downloads the published
//! sheet from the release artifact.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use futures_util::stream::{FuturesUnordered, StreamExt};
use gateway_api_types::{
    ACCEPTED_SHEET_SCHEMA_VERSION, EnvVar, ModelEntry, ProviderSlice, Sheet, SliceStatus, Tier,
};
use time::OffsetDateTime;

use crate::{FetchError, Provider};

/// The outcome of one provider fetch: the normalized models, or the
/// failure that triggers last-known-good propagation.
type BoxFetch = Pin<Box<dyn Future<Output = Result<Vec<ModelEntry>, FetchError>> + Send>>;

/// Builds the complete sheet: fetches every provider, propagates
/// last-known-good slices from `previous` for failed fetches, emits
/// static slices for Niche providers, and assembles the envelope.
///
/// A failed fetch never fails the build: a provider with a previous
/// slice is copied verbatim with `status` rewritten to `stale`, and a
/// provider with no previous slice records `unavailable` with an empty
/// `models` array. A Niche provider's `static` slice comes from a
/// per-provider JSON file compiled into the binary rather than from the
/// network or `previous`.
pub async fn build_sheet(
    client: &reqwest::Client,
    previous: Option<Sheet>,
    keys: &dyn Fn(&Provider) -> Option<String>,
) -> Sheet {
    build_sheet_with(
        crate::providers(),
        previous,
        keys,
        &|client: reqwest::Client, provider: Provider, key: Option<String>| {
            Box::pin(async move {
                match (provider.key_env, key) {
                    (Some(key_env), None) => Err(FetchError::MissingKey {
                        name: provider.name.to_owned(),
                        key_env,
                    }),
                    (_, key) => crate::fetch_models(&client, &provider, key.as_deref()).await,
                }
            })
        },
        client,
    )
    .await
}

/// Downloads and parses the current sheet from the release artifact.
///
/// # Errors
///
/// Returns [`FetchError::NotFound`] on HTTP 404 and [`FetchError::Http`]
/// when the download fails, the response is any other non-success, or
/// the body does not parse as a [`Sheet`].
pub async fn fetch_sheet(client: &reqwest::Client, release_url: &str) -> Result<Sheet, FetchError> {
    let response = client.get(release_url).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(FetchError::NotFound {
            url: release_url.to_owned(),
        });
    }
    Ok(response.error_for_status()?.json().await?)
}

/// The testable core of [`build_sheet`]: the registry and the fetch seam
/// are parameters so tests can inject providers and canned outcomes.
async fn build_sheet_with(
    registry: &[Provider],
    previous: Option<Sheet>,
    keys: &dyn Fn(&Provider) -> Option<String>,
    fetch: &dyn Fn(reqwest::Client, Provider, Option<String>) -> BoxFetch,
    client: &reqwest::Client,
) -> Sheet {
    let mut previous = previous.map_or_else(BTreeMap::new, |sheet| sheet.providers);
    let now = OffsetDateTime::now_utc();
    let mut slices = BTreeMap::new();
    // Fan the provider fetches out on the current task: no `tokio::spawn`,
    // so the `&dyn Fn` seams need no `Send`/`Sync` bound. Niche providers
    // are never fetched and take their static slice immediately.
    let mut fetches = FuturesUnordered::new();
    for provider in registry {
        let prior = previous.remove(provider.name);
        if provider.tier == Tier::Niche {
            slices.insert(provider.name.to_owned(), static_slice(provider));
        } else {
            let provider = *provider;
            fetches.push(async move {
                let outcome = fetch(client.clone(), provider, keys(&provider)).await;
                (provider, prior, outcome)
            });
        }
    }
    // Completion order is irrelevant: slices collect into a `BTreeMap`, so
    // the emitted sheet stays byte-deterministic regardless of which fetch
    // finishes first.
    while let Some((provider, prior, outcome)) = fetches.next().await {
        let slice = match outcome {
            Ok(models) => ProviderSlice {
                display_name: provider.display_name.to_owned(),
                tier: provider.tier,
                status: SliceStatus::Ok,
                fetched_at: Some(now),
                openai_base_url: provider.openai_base_url.map(str::to_owned),
                env_vars: env_vars(&provider),
                models,
            },
            Err(err) => {
                // Surface the failure cause: the sheet records only
                // stale/unavailable, so the stderr note is the run report.
                // `FetchError`'s chain holds provider names, env var names,
                // URLs, and reqwest errors only - never key material, which
                // travels in request headers reqwest does not echo.
                eprintln!(
                    "shared-cloud-providers: note: {} fetch failed: {}",
                    provider.name,
                    crate::error_chain(&err)
                );
                stale_or_unavailable(&provider, prior)
            }
        };
        slices.insert(provider.name.to_owned(), slice);
    }
    Sheet {
        schema_version: ACCEPTED_SHEET_SCHEMA_VERSION,
        generated_at: now,
        providers: slices,
    }
}

/// Propagates a failed fetch: the previous slice verbatim with `status`
/// rewritten to `stale`, or `unavailable` with an empty model list when
/// there is nothing to propagate.
fn stale_or_unavailable(provider: &Provider, prior: Option<ProviderSlice>) -> ProviderSlice {
    match prior {
        Some(mut slice) => {
            slice.status = SliceStatus::Stale;
            slice
        }
        None => ProviderSlice {
            display_name: provider.display_name.to_owned(),
            tier: provider.tier,
            status: SliceStatus::Unavailable,
            fetched_at: None,
            openai_base_url: provider.openai_base_url.map(str::to_owned),
            env_vars: env_vars(provider),
            models: Vec::new(),
        },
    }
}

/// Converts the descriptor's const-friendly env var specs into the
/// schema's owned form for the slice.
fn env_vars(provider: &Provider) -> Vec<EnvVar> {
    provider
        .env_vars
        .iter()
        .map(|spec| EnvVar {
            name: spec.name.to_owned(),
            role: spec.role,
            default: spec.default.map(str::to_owned),
        })
        .collect()
}

/// The compiled-in model list for a Niche provider: one JSON file per
/// provider in the repo, pulled in with `include_str!` and parsed as
/// `Vec<ModelEntry>`. The Niche tier is empty, so the only arm is the test
/// fixture.
fn static_json(name: &str) -> Option<&'static str> {
    match name {
        #[cfg(test)]
        "test-niche" => Some(include_str!("../tests/fixtures/test-niche.json")),
        _ => None,
    }
}

/// A Niche provider's slice, built entirely from the hand-curated models
/// compiled into the binary from the provider's JSON file. `fetched_at`
/// is always absent because the slice has never been fresh.
fn static_slice(provider: &Provider) -> ProviderSlice {
    let models = static_json(provider.name).map_or_else(Vec::new, |json| {
        serde_json::from_str(json).unwrap_or_else(|err| {
            panic!(
                "compiled-in static model file for `{}` must parse: {err}",
                provider.name
            )
        })
    });
    ProviderSlice {
        display_name: provider.display_name.to_owned(),
        tier: provider.tier,
        status: SliceStatus::Static,
        fetched_at: None,
        openai_base_url: provider.openai_base_url.map(str::to_owned),
        env_vars: env_vars(provider),
        models,
    }
}

#[cfg(test)]
#[path = "sheet-tests.rs"]
mod tests;
