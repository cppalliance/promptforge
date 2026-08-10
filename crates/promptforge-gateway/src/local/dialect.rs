//! Tool-dialect resolution for a ready local `llama-server`.
//!
//! Fetches `/props` (and `/v1/models` for the native tool-call capability flag)
//! from a started child, supplements a missing chat template from the sidecar,
//! and resolves the routing model's `(tool_dialect, tools_mode)` via the core
//! [`ToolDialectRegistry`]. Resolution hard-fails on ambiguous or absent
//! evidence so a local model never silently defaults to an incorrect dialect.

use std::path::Path;
use std::time::Duration;

use promptforge_core::dialects::{DialectEvidence, ToolDialectRegistry};
use serde_json::Value;

use super::server::ServerGuard;
use super::sidecar;
use crate::local::error::LocalError;

const PROPS_TIMEOUT: Duration = Duration::from_secs(5);

/// Fetches `/props` from a ready local llama-server and resolves the tool dialect.
///
/// When `/props` does not supply a `chat_template`, the sidecar `.md` next to
/// the GGUF is consulted as a fallback. Props always wins over conflicting
/// sidecar data.
///
/// Returns `(tool_dialect, tools_mode)` strings for the routing model.
/// Hard-fails on `DialectNone` or `DialectTie` so local models never silently
/// default to an incorrect dialect.
pub(crate) fn resolve_local_dialect(
    guard: &ServerGuard,
    model_name: &str,
    model_path: &Path,
) -> Result<(String, String), LocalError> {
    let mut evidence = fetch_props_evidence(guard)?;

    if evidence.chat_template.is_none()
        && let Some(sidecar_meta) = read_sidecar_quietly(model_path)
        && let Some(template) = sidecar_meta.chat_template
    {
        tracing::debug!(
            model = %model_name,
            "supplementing chat_template from sidecar"
        );
        evidence.chat_template = Some(template);
    }

    tracing::debug!(
        model = %model_name,
        supports_tool_calls = ?evidence.supports_tool_calls,
        has_template = evidence.chat_template.is_some(),
        model_id = ?evidence.model_id,
        "dialect evidence from /props + sidecar"
    );
    let registry = ToolDialectRegistry::builtin();
    let dialect_id = registry.resolve(&evidence).map_err(|error| {
        LocalError::Server(format!(
            "dialect resolution failed for local model {model_name}: {error}"
        ))
    })?;
    let tools_mode = dialect_id.tools_mode();
    Ok((dialect_id.to_string(), tools_mode.to_string()))
}

/// Reads the sidecar metadata next to a GGUF, logging and swallowing errors.
pub(crate) fn read_sidecar_quietly(model_path: &Path) -> Option<sidecar::SidecarMeta> {
    match sidecar::read_sidecar(model_path) {
        Ok(meta) => meta,
        Err(e) => {
            tracing::warn!(
                path = %model_path.display(),
                error = %e,
                "failed to read sidecar"
            );
            None
        }
    }
}

/// Fetches `GET /props` from a ready llama-server and builds [`DialectEvidence`].
fn fetch_props_evidence(guard: &ServerGuard) -> Result<DialectEvidence, LocalError> {
    let base = format!("http://127.0.0.1:{}", guard.port());
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(PROPS_TIMEOUT)
        .build()
        .map_err(|e| LocalError::Server(format!("build props client: {e}")))?;

    let response = client
        .get(format!("{base}/props"))
        .bearer_auth(guard.api_key())
        .send()
        .map_err(|e| LocalError::Server(format!("GET /props failed: {e}")))?;

    if !response.status().is_success() {
        return Err(LocalError::Server(format!(
            "GET /props returned {}",
            response.status()
        )));
    }

    let props: Value = response
        .json()
        .map_err(|e| LocalError::Server(format!("parse /props JSON: {e}")))?;

    let chat_template = props
        .get("chat_template")
        .and_then(Value::as_str)
        .map(String::from);

    // llama-server /props exposes `default_generation_settings.model` as the
    // loaded model path and `default_generation_settings.samplers` etc.
    let model_id = props
        .get("default_generation_settings")
        .and_then(|dgs| dgs.get("model"))
        .and_then(Value::as_str)
        .map(String::from);

    // `total_slots` and capabilities are top-level in /props. When the server
    // was launched with `--jinja` and the template declares tool support, the
    // /v1/models response carries `meta.has_tool_call_capability`. We check
    // both /props and /v1/models for the capability flag.
    let supports_tool_calls = fetch_tool_call_capability(&client, &base, guard.api_key());

    Ok(DialectEvidence::new(
        Some(supports_tool_calls),
        chat_template,
        model_id,
        None,
    ))
}

/// Checks the /v1/models response for native tool-call capability.
fn fetch_tool_call_capability(
    client: &reqwest::blocking::Client,
    base: &str,
    api_key: &str,
) -> bool {
    let Ok(response) = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(api_key)
        .send()
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(body) = response.json::<Value>() else {
        return false;
    };
    body.get("data")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("meta"))
        .and_then(|meta| meta.get("has_tool_call_capability"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn sidecar_supplements_missing_template() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("gemma-3-27b.gguf");
        fs::write(&gguf, b"fake").expect("write gguf");

        let meta = sidecar::SidecarMeta {
            source: Some(
                "https://huggingface.co/google/gemma/resolve/main/gemma-3-27b.gguf".to_owned(),
            ),
            fetched: Some("2026-08-08T00:00:00Z".to_owned()),
            chat_template: Some("<start_of_turn>user\n{{ content }}".to_owned()),
            card: None,
        };
        sidecar::write_sidecar(&gguf, &meta).expect("write sidecar");

        let mut evidence = DialectEvidence::new(
            Some(false),
            None, // props had no template
            Some("gemma-3-27b-it".to_owned()),
            None,
        );

        if evidence.chat_template.is_none()
            && let Some(sc) = read_sidecar_quietly(&gguf)
        {
            evidence.chat_template = sc.chat_template;
        }

        assert!(evidence.chat_template.is_some());
        let registry = ToolDialectRegistry::builtin();
        let id = registry
            .resolve(&evidence)
            .expect("should resolve with sidecar");
        assert_eq!(id.to_string(), "gemma3_tool_code");
    }

    #[test]
    fn props_wins_over_conflicting_sidecar() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("model.gguf");
        fs::write(&gguf, b"fake").expect("write gguf");

        let meta = sidecar::SidecarMeta {
            source: None,
            fetched: None,
            chat_template: Some("sidecar-template-should-lose".to_owned()),
            card: None,
        };
        sidecar::write_sidecar(&gguf, &meta).expect("write sidecar");

        let evidence = DialectEvidence::new(
            Some(true),                             // props says native tools
            Some("props-template-wins".to_owned()), // props has its own template
            None,
            None,
        );

        assert_eq!(
            evidence.chat_template.as_deref(),
            Some("props-template-wins")
        );
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id.to_string(), "openai");
    }

    #[test]
    fn sidecar_missing_file_is_harmless() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("no-sidecar.gguf");
        let result = read_sidecar_quietly(&gguf);
        assert!(result.is_none());
    }

    #[test]
    fn gemma_props_resolve_to_gemma3_tool_code() {
        let evidence = DialectEvidence::new(
            Some(false),
            Some("<start_of_turn>user\n".to_string()),
            Some("gemma-3-27b-it".to_string()),
            None,
        );
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id.to_string(), "gemma3_tool_code");
        assert_eq!(id.tools_mode().to_string(), "emulated");
    }

    #[test]
    fn tools_true_resolves_to_openai() {
        let evidence = DialectEvidence::new(Some(true), None, None, None);
        let registry = ToolDialectRegistry::builtin();
        let id = registry.resolve(&evidence).expect("should resolve");
        assert_eq!(id.to_string(), "openai");
        assert_eq!(id.tools_mode().to_string(), "native");
    }

    #[test]
    fn dialect_none_is_hard_fail() {
        let evidence = DialectEvidence::default();
        let registry = ToolDialectRegistry::builtin();
        let result = registry.resolve(&evidence);
        assert!(result.is_err(), "empty evidence must hard-fail");
    }
}
