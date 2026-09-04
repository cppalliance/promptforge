//! The `GET /admin/orphans` route: files in the artifact cache's `models/`
//! tree that no loaded `[[local_model]]` entry references, so an operator can
//! adopt or delete leftovers.
//!
//! The scan is blocking filesystem work, so it runs inside
//! `tokio::task::spawn_blocking` like every store operation (Amendment D).
//! The diff itself lives in the local crate beside the blob cache, which owns
//! the slot layout and the sidecar records.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;

use gateway_config::SttModelConfig;

use crate::auth::Caller;
use crate::error::GatewayError;
use crate::local::{cache::orphans, resolve_cache_root};
use crate::{AppState, check_auth};

/// The `GET /admin/orphans` route: bearer-authed, scans `<cache_dir>/models/`
/// and reports every file no loaded `[[local_model]]` entry references as
/// `{"orphans": [{"path", "size_bytes", "sha256"}]}`.
///
/// `path` is relative to the resolved cache root (`/`-separated on every
/// platform). `sha256` comes from the blob's cache sidecar and is null for
/// files the cache API never downloaded: blobs are multi-gigabyte, so their
/// bytes are never re-hashed here. A missing cache or `models/` directory
/// reports an empty list.
pub(crate) async fn admin_orphans(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<serde_json::Value>, GatewayError> {
    check_auth(&state, &caller).await?;
    // The retained running config carries both the `[local].cache_dir` the
    // scan resolves and the `[[local_model]]` entries it diffs against, so
    // the two can never come from different profiles.
    let config = {
        let live = state.live.read().await;
        Arc::clone(&live.config)
    };
    let entries = tokio::task::spawn_blocking(move || {
        let root = resolve_cache_root(config.local().cache_dir())?;
        let stt_sources: Vec<&str> = config
            .stt_models()
            .iter()
            .map(SttModelConfig::source)
            .collect();
        orphans(&root, config.local_models(), &stt_sources)
    })
    .await
    .map_err(GatewayError::cache)?
    .map_err(GatewayError::cache)?;
    Ok(Json(serde_json::json!({ "orphans": entries })))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gateway_config::Config;

    use crate::test_support::serve;

    /// A profile rooting the cache at `cache_dir` with one `[[local_model]]`
    /// whose path source is `configured`.
    fn orphan_config(cache_dir: &Path, configured: &Path) -> Config {
        Config::from_toml_str(&format!(
            r#"
config-version = 2

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
}
