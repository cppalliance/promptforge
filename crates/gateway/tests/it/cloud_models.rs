//! Cloud model sheet end-to-end against the production binary: the
//! `PROMPTFORGE_MODELS_SHEET_URL` override points the launch download at
//! a loopback stub, `GET /admin/cloud-models` serves the fixture sheet,
//! and a UI-shaped `[[model]]` + `[[endpoint]]` merge staged through
//! `PUT /admin/config` and promoted by `POST /admin/config-apply` lands
//! in the live catalog, with a second model for the same provider
//! reusing the one endpoint.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;
use shared_gateway_api::{
    EnvRole, EnvVar, ModelEntry, ModelKind, ProviderSlice, Sheet, SliceStatus, Thinking, Tier,
};
use time::OffsetDateTime;

use crate::support::{
    GatewayProcess, PHASE_TIMEOUT, json_within, send_within, spawn_backend, wait_for_connection,
};

/// The provider name the fixture sheet and the merged endpoint share.
const PROVIDER: &str = "test";
/// The key-role variable the fixture slice declares; the merged
/// endpoint's `api_key` is its `${...}` indirection, and the spawned
/// gateway carries the value in its environment.
const KEY_ENV: &str = "TEST_CLOUD_KEY";

/// The two-model, one-provider sheet the loopback stub serves.
fn fixture_sheet() -> Sheet {
    Sheet {
        schema_version: 1,
        generated_at: OffsetDateTime::now_utc(),
        providers: BTreeMap::from([(
            PROVIDER.to_owned(),
            ProviderSlice {
                display_name: "Test".to_owned(),
                tier: Tier::Prime,
                status: SliceStatus::Static,
                fetched_at: None,
                openai_base_url: Some("https://api.test.example/v1".to_owned()),
                env_vars: vec![EnvVar {
                    name: KEY_ENV.to_owned(),
                    role: EnvRole::Key,
                    default: None,
                }],
                models: vec![
                    entry("test-model-a", "Test Model A"),
                    entry("test-model-b", "Test Model B"),
                ],
            },
        )]),
    }
}

/// One chat entry with a context window and max output, so the merge
/// needs no operator-supplied details.
fn entry(id: &str, display_name: &str) -> ModelEntry {
    ModelEntry {
        id: id.to_owned(),
        display_name: display_name.to_owned(),
        family: "test-family".to_owned(),
        variant_of: None,
        variant: None,
        languages: vec![],
        kind: ModelKind::Chat,
        released_at: None,
        context_window: Some(8192),
        max_output: Some(4096),
        images: false,
        pdf_input: false,
        video_input: false,
        audio_input: false,
        batch: false,
        citations: false,
        code_execution: false,
        structured_outputs: false,
        tool_calling: false,
        thinking: Thinking::default(),
        effort_levels: vec![],
        default_effort: None,
        pricing: None,
        deprecation: None,
    }
}

/// Serves the fixture sheet at `/sheet.json`, counting requests so the
/// test can prove the launch made exactly one bounded download.
async fn sheet_stub(sheet: &Sheet) -> (SocketAddr, Arc<AtomicUsize>) {
    async fn handler(State((requests, body)): State<(Arc<AtomicUsize>, Value)>) -> Json<Value> {
        requests.fetch_add(1, Ordering::AcqRel);
        Json(body)
    }
    let requests = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/sheet.json", get(handler))
        .with_state((
            Arc::clone(&requests),
            serde_json::to_value(sheet).expect("the fixture sheet serializes"),
        ));
    (spawn_backend(router).await, requests)
}

/// The spawned gateway's config: loopback bind, a known bearer key, and
/// the `main` profile the fixture command line selects.
fn write_config(temp: &tempfile::TempDir) -> PathBuf {
    let path = temp.path().join("gateway.toml");
    std::fs::write(
        &path,
        "config-version = 2\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n",
    )
    .expect("write config");
    path
}

/// Polls `GET /admin/cloud-models` until the launch download lands the
/// sheet (503 loading answers while the background task runs).
async fn wait_for_sheet(url: &str, http: &reqwest::Client) -> Value {
    for _ in 0..200 {
        let response = send_within(
            http.get(format!("{url}/admin/cloud-models"))
                .bearer_auth("test-token"),
        )
        .await;
        if response.status() == reqwest::StatusCode::OK {
            return json_within(response).await;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the downloaded sheet never arrived at /admin/cloud-models");
}

/// The running config document, the merge's input.
async fn get_config(url: &str, http: &reqwest::Client) -> Value {
    json_within(
        send_within(
            http.get(format!("{url}/admin/config"))
                .bearer_auth("test-token"),
        )
        .await,
    )
    .await
}

/// One keyed array of the config document, created when absent.
fn keyed_array<'a>(document: &'a mut Value, key: &str) -> &'a mut Vec<Value> {
    if document.get(key).is_none() {
        document[key] = Value::Array(Vec::new());
    }
    document[key]
        .as_array_mut()
        .expect("the config key is an array")
}

/// The UI's add-model merge (`cloud-merge.ts`), restated over the admin
/// JSON document: append the provider's endpoint when none carries its
/// name, append the model with the sheet's capability fields, and select
/// the model in the active profile so the reload lists it.
fn merge_cloud_model(document: &mut Value, slice: &ProviderSlice, model_index: usize, name: &str) {
    let entry = &slice.models[model_index];
    let endpoints = keyed_array(document, "endpoint");
    if !endpoints.iter().any(|item| item["id"] == PROVIDER) {
        let key_var = slice
            .env_vars
            .iter()
            .find(|variable| variable.role == EnvRole::Key)
            .expect("the fixture slice declares a key variable");
        endpoints.push(serde_json::json!({
            "id": PROVIDER,
            "protocol": "openai",
            "base_url": slice.openai_base_url,
            "api_key": format!("${{{}}}", key_var.name),
        }));
    }
    keyed_array(document, "model").push(serde_json::json!({
        "name": name,
        "kind": "chat",
        "description": entry.display_name,
        "context": entry.context_window,
        "thinking": "never",
        "upstream": entry.id,
        "endpoints": [PROVIDER],
        "images": entry.images,
        "adaptive_thinking": entry.thinking.adaptive,
        "effort_levels": entry.effort_levels,
        "max_output": entry.max_output,
    }));
    let active = document["active_profile"]
        .as_str()
        .expect("the document names the active profile")
        .to_owned();
    let profile = keyed_array(document, "profile")
        .iter_mut()
        .find(|profile| profile["name"] == active)
        .expect("the active profile is in the document");
    profile["models"]
        .as_array_mut()
        .expect("the profile models key is an array")
        .push(Value::String(name.to_owned()));
}

/// Stages the merged document and promotes it, asserting each half of
/// the save/apply flow accepts it.
async fn put_and_apply(url: &str, http: &reqwest::Client, document: &Value) {
    let save = send_within(
        http.put(format!("{url}/admin/config"))
            .bearer_auth("test-token")
            .json(document),
    )
    .await;
    assert_eq!(
        save.status(),
        reqwest::StatusCode::OK,
        "the merged document validates and stages"
    );
    let apply = send_within(
        http.post(format!("{url}/admin/config-apply"))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(
        apply.status(),
        reqwest::StatusCode::OK,
        "the apply promotes the shadow"
    );
    let reply = json_within(apply).await;
    assert_eq!(
        reply["reloaded"], true,
        "the apply reloads the active profile"
    );
}

/// Polls `/v1/models` until the catalog is exactly `expected`, observing
/// the apply's hot-swap without a fixed sleep.
async fn wait_for_catalog(url: &str, http: &reqwest::Client, expected: &[&str]) {
    let mut ids = Vec::new();
    for _ in 0..100 {
        let catalog = json_within(
            send_within(
                http.get(format!("{url}/v1/models"))
                    .bearer_auth("test-token"),
            )
            .await,
        )
        .await;
        ids = catalog["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect::<Vec<_>>();
        if ids == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(ids, expected, "the applied models land in the catalog");
}

#[tokio::test]
async fn the_sheet_downloads_and_merged_models_apply_end_to_end() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    std::fs::create_dir_all(&home).expect("create the isolated home");
    let config = write_config(&temp);
    let sheet = fixture_sheet();
    let (stub, requests) = sheet_stub(&sheet).await;
    let sheet_url = format!("http://{stub}/sheet.json");
    let mut gateway = GatewayProcess::spawn_with_env(
        &config,
        &home,
        &[
            ("PROMPTFORGE_MODELS_SHEET_URL", sheet_url.as_str()),
            (KEY_ENV, "test-key-secret"),
        ],
    );
    let connection = wait_for_connection(&home.join(".promptforge").join("run"), PHASE_TIMEOUT);
    let url = format!("http://127.0.0.1:{}", connection.port);
    let http = reqwest::Client::new();

    // The launch download pulls the fixture sheet from the loopback stub
    // and the admin route serves it.
    let served = wait_for_sheet(&url, &http).await;
    assert_eq!(
        served["providers"][PROVIDER]["models"][0]["id"],
        "test-model-a"
    );
    assert_eq!(
        requests.load(Ordering::Acquire),
        1,
        "the launch made exactly one bounded download"
    );
    assert!(
        home.join(".promptforge")
            .join("cloud-provider-models.json")
            .is_file(),
        "the download landed in the profile cache beside the runtime state"
    );

    // The first add stages endpoint + model, applies, and lists live.
    let mut document = get_config(&url, &http).await;
    merge_cloud_model(
        &mut document,
        &sheet.providers[PROVIDER],
        0,
        "test-model-one",
    );
    put_and_apply(&url, &http, &document).await;
    wait_for_catalog(&url, &http, &["test-model-one"]).await;

    // The second add for the same provider reuses the one endpoint.
    let mut document = get_config(&url, &http).await;
    merge_cloud_model(
        &mut document,
        &sheet.providers[PROVIDER],
        1,
        "test-model-two",
    );
    put_and_apply(&url, &http, &document).await;
    wait_for_catalog(&url, &http, &["test-model-one", "test-model-two"]).await;
    let document = get_config(&url, &http).await;
    let endpoints = document["endpoint"].as_array().unwrap();
    assert_eq!(
        endpoints
            .iter()
            .filter(|item| item["id"] == PROVIDER)
            .count(),
        1,
        "the second model reused the provider endpoint: {endpoints:?}"
    );

    // Graceful shutdown through the HTTP surface, like the tray's Quit.
    let shutdown = send_within(
        http.post(format!("{url}/shutdown"))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(shutdown.status(), reqwest::StatusCode::ACCEPTED);
    let status = gateway.wait_for_exit(PHASE_TIMEOUT);
    assert!(status.success(), "the gateway exits cleanly: {status}");
}
