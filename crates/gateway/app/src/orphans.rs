//! The `GET /admin/orphans` route: files in the artifact cache's `models/`
//! tree that no `[[local_model]]` or `[[stt_model]]` declared in the catalog
//! references, so an operator can adopt or delete leftovers.
//!
//! The scan is blocking filesystem work, so it runs inside
//! `tokio::task::spawn_blocking` like every store operation (Amendment D).
//! The diff itself lives in the local crate beside the blob cache, which owns
//! the slot layout and the sidecar records.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;

use gateway_config::SttModelConfig;

use crate::AppState;
use crate::auth::AuthedCaller;
use crate::error::GatewayError;
use crate::local::{cache::orphans, resolve_cache_root};

/// The `GET /admin/orphans` route: bearer-authed, scans `<cache_dir>/models/`
/// and reports every file no `[[local_model]]` or `[[stt_model]]` declared in
/// the catalog references as `{"orphans": [{"path", "size_bytes", "sha256"}]}`.
///
/// `path` is relative to the resolved cache root (`/`-separated on every
/// platform). `sha256` comes from the blob's cache sidecar and is null for
/// files the cache API never downloaded: blobs are multi-gigabyte, so their
/// bytes are never re-hashed here. A missing cache or `models/` directory
/// reports an empty list.
pub(crate) async fn admin_orphans(
    State(state): State<AppState>,
    _caller: AuthedCaller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    // The retained running config carries both the `[local].cache_dir` the
    // scan resolves and the catalog it diffs against: every `[[local_model]]`
    // and `[[stt_model]]` the document declares, whether or not the running
    // profile selects it. The catalog does not move on an apply, which
    // republishes the document with no profile selected.
    let config = {
        let live = state.live.read().await;
        Arc::clone(&live.config)
    };
    let entries = tokio::task::spawn_blocking(move || {
        let root = resolve_cache_root(config.local().cache_dir())?;
        let stt_sources: Vec<&str> = config
            .catalog_stt_models()
            .iter()
            .map(SttModelConfig::source)
            .collect();
        orphans(&root, config.catalog_local_models(), &stt_sources)
    })
    .await
    .map_err(GatewayError::cache)?
    .map_err(GatewayError::cache)?;
    Ok(Json(serde_json::json!({ "orphans": entries })))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    use gateway_config::{Config, ProfileName};
    use tokio_util::sync::CancellationToken;

    use crate::commands::Command;
    use crate::test_support::{app_state, parking_executor, serve, serve_state};

    /// A profile rooting the cache at `cache_dir` with one `[[local_model]]`
    /// whose path source is `configured`.
    fn orphan_config(cache_dir: &Path, configured: &Path) -> Config {
        Config::from_toml_str(&format!(
            r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
# Strict bearer auth: the tests below pin that a missing key is refused.
trust_loopback = false

[local]
cache_dir = '{cache_dir}'

[[local_model]]
name = "adopted"
description = "a configured local model"
source = '{configured}'
context = 4096
"#,
            cache_dir = cache_dir.display(),
            configured = configured.display(),
        ))
        .expect("the fixture profile parses")
    }

    #[tokio::test]
    async fn admin_orphans_lists_only_unconfigured_files() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let models = temp.path().join("models");
        let slot = models.join("0123456789abcdef");
        std::fs::create_dir_all(&slot).expect("mkdir slot");
        let adopted = models.join("adopted.gguf");
        std::fs::write(&adopted, b"adopted-model-bytes").expect("write adopted");
        std::fs::write(models.join("stray.gguf"), b"stray-bytes").expect("write stray");
        let cached_body: &[u8] = b"cached-bytes";
        let cached_digest = "a".repeat(64);
        std::fs::write(slot.join("cached.gguf"), cached_body).expect("write cached");
        std::fs::write(
            slot.join("cached.gguf.meta.json"),
            serde_json::json!({
                "source": "http://seeded.example/cached.gguf",
                "sha256": cached_digest,
                "size_bytes": cached_body.len(),
            })
            .to_string(),
        )
        .expect("write sidecar");

        let addr = serve(orphan_config(temp.path(), &adopted)).await;
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/orphans"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(
            body,
            serde_json::json!({
                "orphans": [
                    {
                        "path": "models/0123456789abcdef/cached.gguf",
                        "size_bytes": cached_body.len(),
                        "sha256": cached_digest,
                    },
                    {
                        "path": "models/stray.gguf",
                        "size_bytes": b"stray-bytes".len(),
                        "sha256": null,
                    },
                ]
            })
        );
    }

    #[tokio::test]
    async fn admin_orphans_with_no_models_directory_is_empty() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let missing = temp.path().join("models").join("never-provisioned.gguf");
        let addr = serve(orphan_config(temp.path(), &missing)).await;
        let response = reqwest::Client::new()
            .get(format!("http://{addr}/admin/orphans"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: serde_json::Value = response.json().await.expect("a JSON body");
        assert_eq!(body, serde_json::json!({ "orphans": [] }));
    }

    #[tokio::test]
    async fn admin_orphans_requires_bearer_auth() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let missing = temp.path().join("models").join("never-provisioned.gguf");
        let addr = serve(orphan_config(temp.path(), &missing)).await;
        let http = reqwest::Client::new();

        let unauthenticated = http
            .get(format!("http://{addr}/admin/orphans"))
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            unauthenticated.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "a request without a bearer token is refused"
        );

        let wrong_key = http
            .get(format!("http://{addr}/admin/orphans"))
            .bearer_auth("wrong-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(
            wrong_key.status(),
            reqwest::StatusCode::UNAUTHORIZED,
            "a request with the wrong bearer token is refused"
        );
    }

    /// A catalog of two `[[local_model]]` entries whose profile `work`
    /// selects only `adopted`; `shelved` stays declared but unselected.
    fn profiled_toml(cache_dir: &Path, adopted: &Path, shelved: &Path) -> String {
        format!(
            r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = '{cache_dir}'

[[local_model]]
name = "adopted"
description = "the running profile's local model"
source = '{adopted}'
context = 4096

[[local_model]]
name = "shelved"
description = "declared in the catalog, outside the running profile"
source = '{shelved}'
context = 4096

[[profile]]
name = "work"
models = ["adopted"]
"#,
            cache_dir = cache_dir.display(),
            adopted = adopted.display(),
            shelved = shelved.display(),
        )
    }

    /// Parses `toml` with the `work` profile selected, as the boot does.
    fn booted(toml: &str) -> Config {
        Config::from_toml_str(toml)
            .expect("the fixture profile parses")
            .select_profile(Some(&ProfileName::parse("work").expect("profile name")))
            .expect("the work profile selects")
    }

    async fn get_json(addr: std::net::SocketAddr, route: &str) -> serde_json::Value {
        let response = reqwest::Client::new()
            .get(format!("http://{addr}{route}"))
            .bearer_auth("test-token")
            .send()
            .await
            .expect("the request sends");
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        response.json().await.expect("a JSON body")
    }

    /// After an apply, the live document is republished with no profile
    /// selected, so `local_models()` is empty for the rest of the process.
    /// The orphan scan and the status `configured` flag read the catalog,
    /// which the apply does not move. `configured` reaches the wire only as
    /// `provisioning` (configured, not ready, and a command active), so the
    /// test holds a parked command while it reads the status.
    #[tokio::test]
    async fn admin_orphans_and_configured_survive_an_apply_with_no_selection() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let models = temp.path().join("models");
        std::fs::create_dir_all(&models).expect("mkdir models");
        let adopted = models.join("adopted.gguf");
        let shelved = models.join("shelved.gguf");
        std::fs::write(&adopted, b"adopted-model-bytes").expect("write adopted");
        std::fs::write(&shelved, b"shelved-model-bytes").expect("write shelved");
        std::fs::write(models.join("stray.gguf"), b"stray-bytes").expect("write stray");
        let toml = profiled_toml(temp.path(), &adopted, &shelved);

        let state = app_state(booted(&toml), None);
        let addr = serve_state(state.clone()).await;

        // What `capture_apply` publishes for a `[[model]]`-only shadow: the
        // same document, parsed with no profile selected.
        let applied = Config::from_toml_str(&toml)
            .expect("the applied document parses")
            .select_profile(None)
            .expect("no selection");
        assert!(applied.local_models().is_empty());
        state.live.write().await.config = Arc::new(applied);

        let orphans = get_json(addr, "/admin/orphans").await;
        assert_eq!(
            orphans,
            serde_json::json!({
                "orphans": [{
                    "path": "models/stray.gguf",
                    "size_bytes": b"stray-bytes".len(),
                    "sha256": null,
                }]
            }),
            "no declared model's artifact is an orphan after the apply"
        );

        let worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        let _load = state.commands.enqueue(Command::load_profile(
            ProfileName::parse("work").expect("profile name"),
            CancellationToken::new(),
        ));
        tokio::time::timeout(Duration::from_secs(10), async {
            while state.commands.active_command().is_none() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the command goes active");

        let status = get_json(addr, "/admin/status").await;
        let chat = status["endpoints"]
            .as_array()
            .expect("endpoints are an array")
            .iter()
            .find(|entry| entry["path"] == "/v1/chat/completions")
            .expect("the chat endpoint is listed");
        assert_eq!(
            chat["provisioning"], true,
            "the catalog's local chat model keeps the endpoint configured: {status}"
        );

        state.commands.cancel_active();
        state.commands.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    /// An orphan is a cache file no catalog entry references: a model the
    /// running profile leaves out is still declared, so its artifact stays.
    #[tokio::test]
    async fn admin_orphans_keeps_catalog_models_outside_the_running_profile() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let models = temp.path().join("models");
        std::fs::create_dir_all(&models).expect("mkdir models");
        let adopted = models.join("adopted.gguf");
        let shelved = models.join("shelved.gguf");
        std::fs::write(&adopted, b"adopted-model-bytes").expect("write adopted");
        std::fs::write(&shelved, b"shelved-model-bytes").expect("write shelved");
        let toml = profiled_toml(temp.path(), &adopted, &shelved);

        let addr = serve(booted(&toml)).await;
        let orphans = get_json(addr, "/admin/orphans").await;
        assert_eq!(
            orphans,
            serde_json::json!({ "orphans": [] }),
            "a declared model outside the running profile is not an orphan"
        );
    }
}
