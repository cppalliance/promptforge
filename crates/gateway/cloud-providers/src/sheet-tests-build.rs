//! Tests for `build_sheet`: slice statuses, last-known-good propagation, and fetch fan-out.

use gateway_api_types::EnvRole;

use super::*;
use crate::EnvVarSpec;

/// The descriptor env vars every test provider lists: one key-role
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

fn prior_sheet(name: &str, slice: ProviderSlice) -> Sheet {
    Sheet {
        schema_version: 1,
        generated_at: pinned("2026-09-01T00:00:00Z"),
        providers: BTreeMap::from([(name.to_owned(), slice)]),
    }
}

#[tokio::test]
async fn build_sheet_emits_accepted_schema_version() {
    let registry = [provider("test-ok", "Test OK", Tier::Prime)];
    let keys = |_provider: &Provider| Some("test-key".to_owned());
    let fetch = |_client: reqwest::Client, _provider: Provider, _key: Option<String>| -> BoxFetch {
        Box::pin(async { Ok(vec![entry("m1")]) })
    };
    let client = reqwest::Client::new();
    let sheet = build_sheet_with(&registry, None, &keys, &fetch, &client).await;
    assert_eq!(
        sheet.schema_version,
        gateway_api_types::ACCEPTED_SHEET_SCHEMA_VERSION,
        "the writer must emit the schema version the gateway reader accepts"
    );
}

#[tokio::test]
async fn fresh_fetch_writes_ok_slice() {
    let registry = [provider("test-ok", "Test OK", Tier::Prime)];
    let keys = |_provider: &Provider| Some("test-key".to_owned());
    let fetch = |_client: reqwest::Client, _provider: Provider, _key: Option<String>| -> BoxFetch {
        Box::pin(async { Ok(vec![entry("m1"), entry("m2")]) })
    };
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
    let fetched_at = slice.fetched_at.expect("an ok slice must set fetched_at");
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
    let fetch = |_client: reqwest::Client, _provider: Provider, _key: Option<String>| -> BoxFetch {
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
    let fetch = |_client: reqwest::Client, _provider: Provider, _key: Option<String>| -> BoxFetch {
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
    let fetch = |_client: reqwest::Client, provider: Provider, _key: Option<String>| -> BoxFetch {
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
    let keys = |provider: &Provider| (provider.name != "test-nokey").then(|| "test-key".to_owned());
    let fetch = |_client: reqwest::Client, provider: Provider, key: Option<String>| -> BoxFetch {
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
    let fetch = |_client: reqwest::Client, provider: Provider, key: Option<String>| -> BoxFetch {
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
    let fetch = |_client: reqwest::Client, provider: Provider, _key: Option<String>| -> BoxFetch {
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
