//! Tool-dialect resolution tests.

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
    let present_false = serde_json::json!({"chat_template_caps": {"supports_tool_calls": false}});
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
