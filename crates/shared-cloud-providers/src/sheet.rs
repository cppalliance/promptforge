//! Sheet assembly and download: `build_sheet` walks the provider
//! registry, fetching each provider and propagating last-known-good
//! slices for failed fetches; `fetch_sheet` downloads the published
//! sheet from the release artifact.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use futures_util::stream::{FuturesUnordered, StreamExt};
use shared_gateway_api::{EnvVar, ModelEntry, ProviderSlice, Sheet, SliceStatus, Tier};
use time::OffsetDateTime;

use crate::{FetchError, Provider};

/// The current sheet schema version.
const SCHEMA_VERSION: u32 = 1;

/// The outcome of one provider fetch: the normalized models, or the
/// failure that triggers last-known-good propagation.
type BoxFetch = Pin<Box<dyn Future<Output = Result<Vec<ModelEntry>, FetchError>> + Send>>;

/// Build the complete sheet: fetch every provider, propagate
/// last-known-good slices from `previous` for failed fetches, emit
/// static slices for Niche providers, and assemble the envelope.
///
/// A failed fetch never fails the build: a provider with a previous
/// slice is copied verbatim with `status` rewritten to `stale`, and a
/// provider with no previous slice records `unavailable` with an empty
/// `models` array. Niche providers are never fetched and take nothing
/// from `previous`; their `static` slice is read from a per-provider
/// JSON file compiled into the binary.
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

/// Download and parse the current sheet from the release artifact.
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
                // `FetchError`'s Display carries provider names, env var
                // names, URLs, and reqwest errors only - never key material,
                // which travels in request headers reqwest does not echo.
                eprintln!(
                    "shared-cloud-providers: note: {} fetch failed: {err}",
                    provider.name
                );
                stale_or_unavailable(&provider, prior)
            }
        };
        slices.insert(provider.name.to_owned(), slice);
    }
    Sheet {
        schema_version: SCHEMA_VERSION,
        generated_at: now,
        providers: slices,
    }
}

/// Propagate a failed fetch: the previous slice verbatim with `status`
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

/// Convert the descriptor's const-friendly env var specs into the
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
/// `Vec<ModelEntry>`. v1 ships no Niche providers, so the match has no
/// production arms yet.
fn static_json(name: &str) -> Option<&'static str> {
    match name {
        #[cfg(test)]
        "test-niche" => Some(include_str!("../tests/fixtures/test-niche.json")),
        _ => None,
    }
}

/// A Niche provider's slice: never fetched, and nothing taken from the
/// previous sheet. The hand-curated models are compiled into the binary
/// from the provider's JSON file; `fetched_at` is always absent because
/// the slice has never been fresh.
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
mod tests {
    use shared_gateway_api::{EnvRole, ModelKind, Thinking};
    use time::format_description::well_known::Rfc3339;

    use super::*;
    use crate::EnvVarSpec;

    /// The descriptor env vars every test provider carries: one key-role
    /// entry and one config-role entry with a default.
    const TEST_ENV_VARS: &[EnvVarSpec] = &[
        EnvVarSpec {
            name: "TEST_PROVIDER_API_KEY",
            role: EnvRole::Key,
            default: None,
        },
        EnvVarSpec {
            name: "TEST_PROVIDER_REGION",
            role: EnvRole::Config,
            default: Some("us-east-1"),
        },
    ];

    fn provider(name: &'static str, display_name: &'static str, tier: Tier) -> Provider {
        Provider {
            name,
            display_name,
            tier,
            key_env: Some("TEST_PROVIDER_API_KEY"),
            base_url: "https://example.invalid",
            openai_base_url: Some("https://example.invalid/v1"),
            env_vars: TEST_ENV_VARS,
        }
    }

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
            effort_levels: Vec::new(),
            default_effort: None,
            pricing: None,
            deprecation: None,
        }
    }

    fn pinned(text: &str) -> OffsetDateTime {
        OffsetDateTime::parse(text, &Rfc3339).expect("pinned timestamp must parse")
    }

    fn prior_sheet(name: &str, slice: ProviderSlice) -> Sheet {
        Sheet {
            schema_version: 1,
            generated_at: pinned("2026-09-01T00:00:00Z"),
            providers: BTreeMap::from([(name.to_owned(), slice)]),
        }
    }

    #[tokio::test]
    async fn fresh_fetch_writes_ok_slice() {
        let registry = [provider("test-ok", "Test OK", Tier::Prime)];
        let keys = |_provider: &Provider| Some("test-key".to_owned());
        let fetch = |_client: reqwest::Client,
                     _provider: Provider,
                     _key: Option<String>|
         -> BoxFetch { Box::pin(async { Ok(vec![entry("m1"), entry("m2")]) }) };
        let client = reqwest::Client::new();
        let before = OffsetDateTime::now_utc();
        let sheet = build_sheet_with(&registry, None, &keys, &fetch, &client).await;
        let after = OffsetDateTime::now_utc();
        assert_eq!(sheet.schema_version, 1);
        assert!(
            before <= sheet.generated_at && sheet.generated_at <= after,
            "generated_at must be this run's time"
        );
        let slice = &sheet.providers["test-ok"];
        assert_eq!(slice.status, SliceStatus::Ok);
        assert_eq!(slice.display_name, "Test OK");
        assert_eq!(slice.tier, Tier::Prime);
        let fetched_at = slice.fetched_at.expect("an ok slice must carry fetched_at");
        assert!(
            before <= fetched_at && fetched_at <= after,
            "fetched_at must be this run's time"
        );
        assert_eq!(slice.models.len(), 2);
        assert_eq!(slice.models[0].id, "m1");
    }

    #[tokio::test]
    async fn failed_fetch_propagates_previous_slice_verbatim_as_stale() {
        let registry = [provider("test-stale", "Test Stale", Tier::Prime)];
        let fetched_at = pinned("2026-01-01T00:00:00Z");
        let previous = prior_sheet(
            "test-stale",
            ProviderSlice {
                display_name: "Old Name".to_owned(),
                tier: Tier::Subprime,
                status: SliceStatus::Ok,
                fetched_at: Some(fetched_at),
                openai_base_url: None,
                env_vars: Vec::new(),
                models: vec![entry("old-m1")],
            },
        );
        let keys = |_provider: &Provider| Some("test-key".to_owned());
        let fetch =
            |_client: reqwest::Client, _provider: Provider, _key: Option<String>| -> BoxFetch {
                Box::pin(async {
                    Err(FetchError::UnsupportedProvider {
                        name: "boom".to_owned(),
                    })
                })
            };
        let client = reqwest::Client::new();
        let sheet = build_sheet_with(&registry, Some(previous), &keys, &fetch, &client).await;
        let slice = &sheet.providers["test-stale"];
        assert_eq!(slice.status, SliceStatus::Stale);
        assert_eq!(
            slice.fetched_at,
            Some(fetched_at),
            "a stale slice preserves its original fetched_at"
        );
        assert_eq!(
            slice.display_name, "Old Name",
            "a stale slice is the previous slice verbatim"
        );
        assert_eq!(slice.tier, Tier::Subprime);
        assert_eq!(slice.models.len(), 1);
        assert_eq!(slice.models[0].id, "old-m1");
    }

    #[tokio::test]
    async fn failed_fetch_without_previous_records_unavailable() {
        let registry = [provider("test-down", "Test Down", Tier::Prime)];
        let keys = |_provider: &Provider| Some("test-key".to_owned());
        let fetch =
            |_client: reqwest::Client, _provider: Provider, _key: Option<String>| -> BoxFetch {
                Box::pin(async {
                    Err(FetchError::UnsupportedProvider {
                        name: "boom".to_owned(),
                    })
                })
            };
        let client = reqwest::Client::new();
        let sheet = build_sheet_with(&registry, None, &keys, &fetch, &client).await;
        let slice = &sheet.providers["test-down"];
        assert_eq!(slice.status, SliceStatus::Unavailable);
        assert!(slice.models.is_empty());
        assert_eq!(slice.fetched_at, None);
        assert_eq!(
            slice.display_name, "Test Down",
            "an unavailable slice still describes the provider"
        );
        assert_eq!(slice.tier, Tier::Prime);
    }

    #[tokio::test]
    async fn niche_provider_emits_static_slice_without_fetching() {
        let registry = [
            provider("test-niche", "Test Niche", Tier::Niche),
            provider("test-niche-empty", "Test Niche Empty", Tier::Niche),
        ];
        let previous = prior_sheet(
            "test-niche",
            ProviderSlice {
                display_name: "Test Niche".to_owned(),
                tier: Tier::Niche,
                status: SliceStatus::Static,
                fetched_at: None,
                openai_base_url: None,
                env_vars: Vec::new(),
                models: vec![entry("old-m1")],
            },
        );
        let keys = |_provider: &Provider| Some("test-key".to_owned());
        let fetch =
            |_client: reqwest::Client, provider: Provider, _key: Option<String>| -> BoxFetch {
                panic!("niche providers must never be fetched: {}", provider.name);
            };
        let client = reqwest::Client::new();
        let sheet = build_sheet_with(&registry, Some(previous), &keys, &fetch, &client).await;
        let curated = &sheet.providers["test-niche"];
        assert_eq!(curated.status, SliceStatus::Static);
        assert_eq!(
            curated.fetched_at, None,
            "a static slice is never fresh, so fetched_at is absent"
        );
        assert_eq!(
            curated.models.len(),
            1,
            "a static slice's models come from the compiled-in JSON file"
        );
        assert_eq!(curated.models[0].id, "curated-m1");
        assert_eq!(curated.models[0].display_name, "Curated M1");
        assert_eq!(curated.models[0].context_window, Some(64_000));
        assert_eq!(
            curated.models[0].released_at,
            Some(
                time::Date::from_calendar_date(2026, time::Month::January, 15)
                    .expect("fixture date must be valid")
            ),
            "the compiled-in JSON parses into full model entries"
        );
        let empty = &sheet.providers["test-niche-empty"];
        assert_eq!(empty.status, SliceStatus::Static);
        assert!(
            empty.models.is_empty(),
            "a niche provider with no compiled-in file takes nothing from `previous`"
        );
        assert_eq!(empty.fetched_at, None);
    }

    #[tokio::test]
    async fn failed_fetch_never_fails_the_build_and_never_drops_data() {
        let registry = [
            provider("test-ok", "Test OK", Tier::Prime),
            provider("test-down", "Test Down", Tier::Prime),
            provider("test-nokey", "Test Nokey", Tier::Prime),
        ];
        let previous = prior_sheet(
            "test-nokey",
            ProviderSlice {
                display_name: "Test Nokey".to_owned(),
                tier: Tier::Prime,
                status: SliceStatus::Ok,
                fetched_at: Some(pinned("2026-06-01T00:00:00Z")),
                openai_base_url: None,
                env_vars: Vec::new(),
                models: vec![entry("kept-m1")],
            },
        );
        let keys =
            |provider: &Provider| (provider.name != "test-nokey").then(|| "test-key".to_owned());
        let fetch =
            |_client: reqwest::Client, provider: Provider, key: Option<String>| -> BoxFetch {
                Box::pin(async move {
                    match (provider.name, key) {
                        ("test-ok", Some(_)) => Ok(vec![entry("m1")]),
                        ("test-nokey", None) => Err(FetchError::MissingKey {
                            name: provider.name.to_owned(),
                            key_env: "TEST_PROVIDER_API_KEY",
                        }),
                        _ => Err(FetchError::UnsupportedProvider {
                            name: provider.name.to_owned(),
                        }),
                    }
                })
            };
        let client = reqwest::Client::new();
        let sheet = build_sheet_with(&registry, Some(previous), &keys, &fetch, &client).await;
        assert_eq!(
            sheet.providers.len(),
            3,
            "one provider's failure must not fail the build"
        );
        assert_eq!(sheet.providers["test-ok"].status, SliceStatus::Ok);
        assert_eq!(
            sheet.providers["test-down"].status,
            SliceStatus::Unavailable
        );
        let nokey = &sheet.providers["test-nokey"];
        assert_eq!(
            nokey.status,
            SliceStatus::Stale,
            "a missing key propagates last-known-good like any failed fetch"
        );
        assert_eq!(
            nokey.models[0].id, "kept-m1",
            "a failed fetch never drops data"
        );
    }

    #[tokio::test]
    async fn keyless_provider_fetches_without_a_credential() {
        let registry = [Provider {
            key_env: None,
            ..provider("test-keyless", "Test Keyless", Tier::Prime)
        }];
        let keys = |_provider: &Provider| None;
        let fetch =
            |_client: reqwest::Client, provider: Provider, key: Option<String>| -> BoxFetch {
                Box::pin(async move {
                    assert!(
                        key.is_none(),
                        "a keyless provider must be fetched with no key"
                    );
                    Ok(vec![entry(&format!("{}-m1", provider.name))])
                })
            };
        let client = reqwest::Client::new();
        let sheet = build_sheet_with(&registry, None, &keys, &fetch, &client).await;
        let slice = &sheet.providers["test-keyless"];
        assert_eq!(
            slice.status,
            SliceStatus::Ok,
            "a keyless provider with no credential must fetch, not record MissingKey"
        );
        assert_eq!(slice.models[0].id, "test-keyless-m1");
    }

    #[tokio::test]
    async fn provider_fetches_overlap() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        use tokio::sync::Barrier;

        let registry = [
            provider("test-a", "Test A", Tier::Prime),
            provider("test-b", "Test B", Tier::Prime),
        ];
        let keys = |_provider: &Provider| Some("test-key".to_owned());
        let in_flight = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let fetch =
            move |_client: reqwest::Client, provider: Provider, _key: Option<String>| -> BoxFetch {
                let in_flight = Arc::clone(&in_flight);
                let barrier = Arc::clone(&barrier);
                Box::pin(async move {
                    in_flight.fetch_add(1, Ordering::SeqCst);
                    // Park until both fetches are in flight: both sides
                    // passing the barrier proves the fetches overlap, with
                    // no timing assertion. A serial loop deadlocks here, so
                    // the timeout below is a hang guard, not a clock check.
                    barrier.wait().await;
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                    Ok(vec![entry(&format!("{}-m1", provider.name))])
                })
            };
        let client = reqwest::Client::new();
        let sheet = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            build_sheet_with(&registry, None, &keys, &fetch, &client),
        )
        .await
        .expect("provider fetches must overlap: a serial loop deadlocks on the barrier");
        assert_eq!(sheet.providers["test-a"].status, SliceStatus::Ok);
        assert_eq!(sheet.providers["test-b"].status, SliceStatus::Ok);
        assert_eq!(sheet.providers["test-a"].models[0].id, "test-a-m1");
        assert_eq!(sheet.providers["test-b"].models[0].id, "test-b-m1");
    }

    #[tokio::test]
    async fn slice_construction_copies_descriptor_fields_for_every_status() {
        let assert_copied = |slice: &ProviderSlice, status: SliceStatus| {
            assert_eq!(slice.status, status);
            assert_eq!(
                slice.openai_base_url.as_deref(),
                Some("https://example.invalid/v1"),
                "a {status:?} slice must copy the descriptor's openai_base_url"
            );
            assert_eq!(slice.env_vars.len(), 2);
            assert_eq!(slice.env_vars[0].name, "TEST_PROVIDER_API_KEY");
            assert_eq!(slice.env_vars[0].role, EnvRole::Key);
            assert_eq!(slice.env_vars[0].default, None);
            assert_eq!(slice.env_vars[1].name, "TEST_PROVIDER_REGION");
            assert_eq!(slice.env_vars[1].role, EnvRole::Config);
            assert_eq!(slice.env_vars[1].default.as_deref(), Some("us-east-1"));
        };
        let registry = [
            provider("test-ok", "Test OK", Tier::Prime),
            provider("test-down", "Test Down", Tier::Prime),
            provider("test-niche", "Test Niche", Tier::Niche),
        ];
        let keys = |_provider: &Provider| Some("test-key".to_owned());
        let fetch =
            |_client: reqwest::Client, provider: Provider, _key: Option<String>| -> BoxFetch {
                Box::pin(async move {
                    match provider.name {
                        "test-ok" => Ok(vec![entry("m1")]),
                        _ => Err(FetchError::UnsupportedProvider {
                            name: provider.name.to_owned(),
                        }),
                    }
                })
            };
        let client = reqwest::Client::new();
        let sheet = build_sheet_with(&registry, None, &keys, &fetch, &client).await;
        assert_copied(&sheet.providers["test-ok"], SliceStatus::Ok);
        assert_copied(&sheet.providers["test-down"], SliceStatus::Unavailable);
        assert_copied(&sheet.providers["test-niche"], SliceStatus::Static);
    }

    #[tokio::test]
    async fn fetch_sheet_reports_http_errors() {
        let client = reqwest::Client::new();
        let result = fetch_sheet(&client, "http://127.0.0.1:1/models.json").await;
        let Err(err) = result else {
            panic!("an unreachable release URL must not parse as a sheet");
        };
        assert!(
            matches!(err, FetchError::Http(_)),
            "expected a transport error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_sheet_parses_a_successful_response() {
        use std::io::Write as _;

        let fetched_at = pinned("2026-09-14T13:00:00Z");
        let expected = Sheet {
            schema_version: 1,
            generated_at: fetched_at,
            providers: BTreeMap::from([(
                "test-provider".to_owned(),
                ProviderSlice {
                    display_name: "Test Provider".to_owned(),
                    tier: Tier::Prime,
                    status: SliceStatus::Ok,
                    fetched_at: Some(fetched_at),
                    openai_base_url: None,
                    env_vars: Vec::new(),
                    models: vec![entry("m1")],
                },
            )]),
        };
        let body = serde_json::to_string(&expected).expect("sheet fixture must serialize");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        let server = std::thread::spawn(move || {
            use std::io::Read as _;

            let (mut stream, _) = listener.accept().expect("accept fixture client");
            // Read the request first: replying before the client finishes
            // sending is an HTTP protocol error. A short read timeout bounds
            // the capture without a sleep; once the client awaits the
            // response, the next read simply times out.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let mut buf = [0_u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
        });
        let client = reqwest::Client::new();
        let sheet = fetch_sheet(&client, &format!("http://{addr}/models.json"))
            .await
            .expect("a 2xx response with a valid sheet body must parse");
        server.join().expect("fixture server must finish");
        assert_eq!(sheet.schema_version, 1);
        assert_eq!(sheet.generated_at, fetched_at);
        let slice = &sheet.providers["test-provider"];
        assert_eq!(slice.display_name, "Test Provider");
        assert_eq!(slice.tier, Tier::Prime);
        assert_eq!(slice.status, SliceStatus::Ok);
        assert_eq!(slice.fetched_at, Some(fetched_at));
        assert_eq!(slice.models.len(), 1);
        assert_eq!(slice.models[0].id, "m1");
        assert_eq!(slice.models[0].context_window, Some(200_000));
    }
}
