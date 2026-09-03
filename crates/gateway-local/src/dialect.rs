//! Tool-dialect resolution for a ready local `llama-server`.
//!
//! Fetches `/props` (and `/v1/models` for the native tool-call capability flag)
//! from a started child, supplements a missing chat template from the sidecar,
//! and resolves the routing model's tool dialect from that evidence.
//! Resolution hard-fails on ambiguous or absent evidence so a local model
//! never silently defaults to an incorrect dialect.

use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::server::ServerGuard;
use super::sidecar;
use crate::error::LocalError;

const PROPS_TIMEOUT: Duration = Duration::from_secs(5);

/// Byte ceiling for a dialect-probe JSON body (HYGIENE-BOUNDS-001).
const MAX_PROBE_BODY: u64 = gateway_protocol::http_util::MAX_JSON_BODY as u64;

/// Evidence from a local child's `/props`, `/v1/models`, and sidecar metadata
/// used to select a tool-calling dialect.
///
/// Fields are `Option` so only what the probes actually reported is supplied.
#[derive(Debug, Clone, Default)]
struct DialectEvidence {
    /// Whether the endpoint advertises native tool-call support.
    supports_tool_calls: Option<bool>,
    /// The raw Jinja chat template string, when available.
    chat_template: Option<String>,
    /// The model identifier from the endpoint metadata.
    model_id: Option<String>,
}

/// Why dialect resolution failed for a local model.
#[derive(Debug, thiserror::Error)]
pub enum DialectResolveError {
    /// No dialect scored on the provided evidence.
    #[error("no tool dialect matched the provided evidence")]
    NoMatch,
    /// The dialects tied for the highest detection score.
    #[error("tool dialect detection tied among: {candidates:?}")]
    Tie {
        /// The dialect identifiers that shared the top score.
        candidates: Vec<&'static str>,
    },
}

/// Scores the OpenAI native-tool dialect against the evidence.
///
/// F6: a template match requires a *conjunction* of a request-side and a
/// response/result marker for each template family, not a single broad
/// substring that a mere mention of "tool_call" would satisfy.
fn openai_score(evidence: &DialectEvidence) -> Option<u8> {
    // Positive support authoritatively selects the native dialect.
    if evidence.supports_tool_calls == Some(true) {
        return Some(80);
    }
    // Unknown, or an unreliable negative (e.g. llama.cpp `/props`, which can
    // deny tool support a GGUF template actually provides): fall through to
    // structured template evidence.
    let template = evidence.chat_template.as_deref().unwrap_or("");
    // Qwen/ChatML: the `<|im_start|>` turn framing plus the `<tool_call>`
    // request tag together indicate a genuine tool-calling template.
    let chatml_tools = template.contains("<|im_start|>") && template.contains("<tool_call>");
    // Mistral Tekken / Small Instruct: the `[AVAILABLE_TOOLS]` declaration
    // plus a call or result marker.
    let mistral_tools = template.contains("[AVAILABLE_TOOLS]")
        && (template.contains("[TOOL_CALLS]") || template.contains("[TOOL_RESULTS]"));
    // Gemma-4: pipe-wrapped request and response markers; the ChatML
    // conjunction misses these because the template has no `<|im_start|>`.
    let gemma4_tools = template.contains("<|tool_call|>") && template.contains("<|tool_response|>");
    (chatml_tools || mistral_tools || gemma4_tools).then_some(70)
}

/// Scores the Gemma3 `tool_code` fence dialect against the evidence.
///
/// Never matches an endpoint with explicit native tool-call support;
/// otherwise requires a Gemma fingerprint (template turn marker or model id),
/// so other tools-unsupported models (e.g. some Qwen GGUFs) do not resolve
/// here from caps alone.
fn gemma3_tool_code_score(evidence: &DialectEvidence) -> Option<u8> {
    if evidence.supports_tool_calls == Some(true) {
        return None;
    }
    // `<bos>` alone is too common; require the Gemma turn marker.
    let gemma_template = evidence
        .chat_template
        .as_deref()
        .is_some_and(|template| template.contains("<start_of_turn>"));
    let gemma_model = evidence
        .model_id
        .as_deref()
        .is_some_and(|id| id.to_ascii_lowercase().contains("gemma"));
    if !gemma_template && !gemma_model {
        return None;
    }
    let mut score = 0;
    if evidence.supports_tool_calls == Some(false) {
        score += 40;
    }
    if gemma_template {
        score += 30;
    }
    if gemma_model {
        score += 20;
    }
    Some(score)
}

/// Resolves evidence into a single dialect id, failing on a tie or no match.
fn resolve_dialect(evidence: &DialectEvidence) -> Result<&'static str, DialectResolveError> {
    // Single scan tracking the best score and every id tied at it.
    let mut best: Option<u8> = None;
    let mut tied: Vec<&'static str> = Vec::new();
    for (id, score) in [
        ("openai", openai_score(evidence)),
        (
            gateway_routing::GEMMA3_TOOL_CODE,
            gemma3_tool_code_score(evidence),
        ),
    ] {
        let Some(score) = score else { continue };
        match best {
            Some(current) if score < current => {}
            Some(current) if score == current => tied.push(id),
            _ => {
                best = Some(score);
                tied.clear();
                tied.push(id);
            }
        }
    }
    if best.is_none() {
        return Err(DialectResolveError::NoMatch);
    }
    if tied.len() > 1 {
        return Err(DialectResolveError::Tie { candidates: tied });
    }
    Ok(tied[0])
}

/// Reads a blocking probe response with a byte cap, rejecting oversize rather
/// than truncating, then decodes it as JSON (HYGIENE-BOUNDS-001).
fn read_probe_json(
    operation: &'static str,
    response: reqwest::blocking::Response,
) -> Result<Value, LocalError> {
    let mut buf = Vec::new();
    response
        .take(MAX_PROBE_BODY + 1)
        .read_to_end(&mut buf)
        .map_err(|source| LocalError::DialectRead { operation, source })?;
    if buf.len() as u64 > MAX_PROBE_BODY {
        return Err(LocalError::DialectRead {
            operation,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("probe body exceeds {MAX_PROBE_BODY} bytes"),
            ),
        });
    }
    decode_probe_json(operation, &buf)
}

/// Decodes probe body bytes as JSON (pure; unit-tested).
fn decode_probe_json(operation: &'static str, bytes: &[u8]) -> Result<Value, LocalError> {
    serde_json::from_slice(bytes).map_err(|source| LocalError::DialectDecode { operation, source })
}

/// Fetches `/props` from a ready local llama-server and resolves the tool dialect.
///
/// When `/props` does not supply a `chat_template`, the sidecar `.md` next to
/// the GGUF is consulted as a fallback. Props always wins over conflicting
/// sidecar data.
///
/// Returns the tool-dialect id for the routing model. Hard-fails when no
/// dialect matches or two tie, so local models never silently default to an
/// incorrect dialect.
pub(crate) fn resolve_local_dialect(
    guard: &ServerGuard,
    model_name: &str,
    model_path: &Path,
) -> Result<&'static str, LocalError> {
    let evidence = fetch_props_evidence(guard)?;
    let had_props_template = evidence.chat_template.is_some();
    let evidence = supplement_evidence(evidence, read_sidecar_quietly(model_path).as_ref());
    if !had_props_template && evidence.chat_template.is_some() {
        tracing::debug!(
            model = %model_name,
            "supplementing chat_template from sidecar"
        );
    }

    tracing::debug!(
        model = %model_name,
        supports_tool_calls = ?evidence.supports_tool_calls,
        has_template = evidence.chat_template.is_some(),
        model_id = ?evidence.model_id,
        "dialect evidence from /props + sidecar"
    );
    resolve_dialect(&evidence).map_err(|source| LocalError::DialectResolution {
        model: model_name.to_owned(),
        source,
    })
}

/// Resolves the final tool-dialect evidence from props plus optional sidecar
/// metadata (pure; unit-tested through this seam - MOD-009).
///
/// Precedence is fixed: props always wins. The sidecar `chat_template` is used
/// only to fill a template that props did not supply; every other evidence
/// field comes from props unchanged.
fn supplement_evidence(
    mut props: DialectEvidence,
    sidecar: Option<&sidecar::SidecarMeta>,
) -> DialectEvidence {
    if props.chat_template.is_none()
        && let Some(template) = sidecar.and_then(|meta| meta.chat_template.clone())
    {
        props.chat_template = Some(template);
    }
    props
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
        .map_err(|source| LocalError::DialectProbe {
            operation: "build props client",
            source,
        })?;

    let response = client
        .get(format!("{base}/props"))
        .bearer_auth(guard.api_key())
        .send()
        .map_err(|source| LocalError::DialectProbe {
            operation: "GET /props",
            source,
        })?;

    if !response.status().is_success() {
        return Err(LocalError::DialectProbeStatus {
            operation: "GET /props",
            status: response.status().to_string(),
        });
    }

    let props: Value = read_probe_json("read /props body", response)?;

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

    // Props-first precedence: `chat_template_caps.supports_tool_calls` is the
    // authoritative capability source when present; only its absence falls
    // back to the /v1/models probe. When the server was launched with
    // `--jinja` and the template declares tool support, the /v1/models
    // response carries `meta.has_tool_call_capability`. A probe failure is
    // surfaced (the server just passed readiness, so it is an anomaly), while
    // a reachable response whose field is absent yields `None` rather than a
    // bogus definitive `false`. (MOD-003)
    let supports_tool_calls = match props_supports_tool_calls(&props) {
        Some(supported) => Some(supported),
        None => fetch_tool_call_capability(&client, &base, guard.api_key())?,
    };

    Ok(DialectEvidence {
        supports_tool_calls,
        chat_template,
        model_id,
    })
}

/// Extracts `chat_template_caps.supports_tool_calls` from a `/props` body.
///
/// Returns `None` when the field is absent, so callers can fall back to the
/// `/v1/models` capability probe (props-first precedence, MOD-003).
fn props_supports_tool_calls(props: &Value) -> Option<bool> {
    props
        .get("chat_template_caps")
        .and_then(|caps| caps.get("supports_tool_calls"))
        .and_then(Value::as_bool)
}

/// Reads native tool-call capability from `/v1/models`.
///
/// # Errors
/// Returns a [`LocalError`] when the probe request fails, returns a non-success
/// status, or the body is not JSON. `Ok(None)` means the endpoint answered but
/// did not report the capability (unknown, not `false`).
fn fetch_tool_call_capability(
    client: &reqwest::blocking::Client,
    base: &str,
    api_key: &str,
) -> Result<Option<bool>, LocalError> {
    let response = client
        .get(format!("{base}/v1/models"))
        .bearer_auth(api_key)
        .send()
        .map_err(|source| LocalError::DialectProbe {
            operation: "GET /v1/models for tool capability",
            source,
        })?;
    if !response.status().is_success() {
        return Err(LocalError::DialectProbeStatus {
            operation: "GET /v1/models for tool capability",
            status: response.status().to_string(),
        });
    }
    let body: Value = read_probe_json("read /v1/models body", response)?;
    Ok(tool_call_capability_from_body(&body))
}

/// Extracts `data[0].meta.has_tool_call_capability` from a `/v1/models` body.
///
/// Returns `None` when the field is absent, so callers can distinguish
/// "unknown" from a definitive `Some(false)`.
fn tool_call_capability_from_body(body: &Value) -> Option<bool> {
    body.get("data")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| model.get("meta"))
        .and_then(|meta| meta.get("has_tool_call_capability"))
        .and_then(Value::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn decode_probe_json_accepts_valid_and_rejects_malformed() {
        // HYGIENE-BOUNDS-001: bodies are decoded from bounded bytes; malformed
        // JSON is a typed decode error, not a swallowed empty value.
        let ok = decode_probe_json("op", br#"{"chat_template":"x"}"#).expect("valid json");
        assert_eq!(ok["chat_template"], "x");
        let err = decode_probe_json("op", b"not json").unwrap_err();
        assert!(matches!(err, LocalError::DialectDecode { .. }));
    }

    fn sidecar_with_template(template: &str) -> sidecar::SidecarMeta {
        sidecar::SidecarMeta {
            source: None,
            fetched: None,
            chat_template: Some(template.to_owned()),
            card: None,
        }
    }

    #[test]
    fn supplement_evidence_fills_absent_props_template_from_sidecar() {
        // MOD-009: the production merge seam supplies a template only when props
        // lacked one; resolution then succeeds through it.
        let props = DialectEvidence {
            supports_tool_calls: Some(false),
            model_id: Some("gemma-3-27b-it".to_owned()),
            ..DialectEvidence::default()
        };
        let sidecar = sidecar_with_template("<start_of_turn>user\n{{ content }}");
        let merged = supplement_evidence(props, Some(&sidecar));
        assert_eq!(
            merged.chat_template.as_deref(),
            Some("<start_of_turn>user\n{{ content }}")
        );
        let id = resolve_dialect(&merged).expect("should resolve with sidecar");
        assert_eq!(id, "gemma3_tool_code");
    }

    #[test]
    fn supplement_evidence_prefers_props_template_over_sidecar() {
        // MOD-009: props always wins; a conflicting sidecar template is ignored.
        let props = DialectEvidence {
            supports_tool_calls: Some(true),
            chat_template: Some("props-template-wins".to_owned()),
            ..DialectEvidence::default()
        };
        let sidecar = sidecar_with_template("sidecar-template-should-lose");
        let merged = supplement_evidence(props, Some(&sidecar));
        assert_eq!(merged.chat_template.as_deref(), Some("props-template-wins"));
        assert_eq!(resolve_dialect(&merged).expect("resolve"), "openai");
    }

    #[test]
    fn supplement_evidence_leaves_template_absent_when_neither_has_one() {
        // MOD-009: no props template and no sidecar leaves the field unresolved.
        let props = DialectEvidence {
            supports_tool_calls: Some(false),
            ..DialectEvidence::default()
        };
        assert!(supplement_evidence(props, None).chat_template.is_none());
        let props = DialectEvidence {
            supports_tool_calls: Some(false),
            ..DialectEvidence::default()
        };
        let empty = sidecar::SidecarMeta::default();
        assert!(
            supplement_evidence(props, Some(&empty))
                .chat_template
                .is_none()
        );
    }

    #[test]
    fn read_sidecar_round_trips_into_the_merge_seam() {
        // The on-disk read path feeds the same seam used in production.
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("gemma-3-27b.gguf");
        fs::write(&gguf, b"fake").expect("write gguf");
        sidecar::write_sidecar(
            &gguf,
            &sidecar_with_template("<start_of_turn>user\n{{ content }}"),
        )
        .expect("write sidecar");

        let props = DialectEvidence {
            supports_tool_calls: Some(false),
            model_id: Some("gemma-3-27b-it".to_owned()),
            ..DialectEvidence::default()
        };
        let merged = supplement_evidence(props, read_sidecar_quietly(&gguf).as_ref());
        assert!(merged.chat_template.is_some());
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
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("<start_of_turn>user\n".to_string()),
            model_id: Some("gemma-3-27b-it".to_string()),
        };
        let id = resolve_dialect(&evidence).expect("should resolve");
        assert_eq!(id, "gemma3_tool_code");
    }

    #[test]
    fn tools_true_resolves_to_openai() {
        let evidence = DialectEvidence {
            supports_tool_calls: Some(true),
            ..DialectEvidence::default()
        };
        assert_eq!(
            resolve_dialect(&evidence).expect("should resolve"),
            "openai"
        );
    }

    #[test]
    fn tool_call_capability_distinguishes_absent_from_false() {
        let present_true = serde_json::json!({
            "data": [{ "meta": { "has_tool_call_capability": true } }]
        });
        let present_false = serde_json::json!({
            "data": [{ "meta": { "has_tool_call_capability": false } }]
        });
        let absent = serde_json::json!({ "data": [{ "meta": {} }] });
        let no_data = serde_json::json!({ "object": "list" });
        assert_eq!(tool_call_capability_from_body(&present_true), Some(true));
        assert_eq!(tool_call_capability_from_body(&present_false), Some(false));
        assert_eq!(tool_call_capability_from_body(&absent), None);
        assert_eq!(tool_call_capability_from_body(&no_data), None);
    }

    #[test]
    fn dialect_none_is_hard_fail() {
        let result = resolve_dialect(&DialectEvidence::default());
        assert!(result.is_err(), "empty evidence must hard-fail");
    }

    #[test]
    fn gemma4_template_markers_resolve_to_openai() {
        // Gemma-4 uses pipe-wrapped markers with no `<|im_start|>` and no
        // `<start_of_turn>`; without the Gemma-4 conjunction this evidence
        // hard-fails with NoMatch when both capability probes are silent.
        let evidence = DialectEvidence {
            chat_template: Some(
                "<|turn>user\n{{ content }}<|tool_call|>call<|tool_response|>result".to_owned(),
            ),
            ..DialectEvidence::default()
        };
        assert_eq!(
            resolve_dialect(&evidence).expect("should resolve"),
            "openai"
        );
    }

    #[test]
    fn gemma4_markers_outscore_gemma_model_fingerprint() {
        // Regression: a "gemma" model id alone would score for
        // gemma3_tool_code; the Gemma-4 template conjunction must outrank it.
        let evidence = DialectEvidence {
            chat_template: Some("<|turn>user<|tool_call|><|tool_response|>".to_owned()),
            model_id: Some("gemma-4-31b-it".to_owned()),
            ..DialectEvidence::default()
        };
        assert_eq!(
            resolve_dialect(&evidence).expect("should resolve"),
            "openai"
        );
    }

    #[test]
    fn props_supports_tool_calls_distinguishes_absent_from_false() {
        // Props-first precedence: a present field is authoritative, so the
        // parse must not collapse absent into Some(false).
        let present_true = serde_json::json!({"chat_template_caps": {"supports_tool_calls": true}});
        let present_false =
            serde_json::json!({"chat_template_caps": {"supports_tool_calls": false}});
        let absent = serde_json::json!({"chat_template": "x"});
        let wrong_type = serde_json::json!({"chat_template_caps": {"supports_tool_calls": "yes"}});
        assert_eq!(props_supports_tool_calls(&present_true), Some(true));
        assert_eq!(props_supports_tool_calls(&present_false), Some(false));
        assert_eq!(props_supports_tool_calls(&absent), None);
        assert_eq!(props_supports_tool_calls(&wrong_type), None);
    }

    #[test]
    fn props_caps_true_resolves_to_openai() {
        // The /props capability field feeds the same evidence field the
        // /v1/models probe fills, so Some(true) selects the native dialect.
        let props = serde_json::json!({"chat_template_caps": {"supports_tool_calls": true}});
        let evidence = DialectEvidence {
            supports_tool_calls: props_supports_tool_calls(&props),
            ..DialectEvidence::default()
        };
        assert_eq!(
            resolve_dialect(&evidence).expect("should resolve"),
            "openai"
        );
    }

    #[test]
    fn gemma4_markers_resolve_despite_unreliable_caps_false() {
        // Regression for the existing fall-through: a Some(false) capability
        // is an unreliable negative, so template evidence still decides.
        let evidence = DialectEvidence {
            supports_tool_calls: Some(false),
            chat_template: Some("<|turn>user<|tool_call|><|tool_response|>".to_owned()),
            ..DialectEvidence::default()
        };
        assert_eq!(
            resolve_dialect(&evidence).expect("should resolve"),
            "openai"
        );
    }
}
