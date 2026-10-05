//! Tests for chat-only fields on non-chat model kinds and speech voices.

use super::*;

#[test]
fn rejects_embedding_model_with_thinking() {
    for kind in ["embedding", "classifier"] {
        let toml = catalog_with_model_kind(kind, "thinking = \"always\"");
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("model m"),
                    "expected the error to name the model: {message}"
                );
                assert!(
                    message.contains("thinking"),
                    "expected the error to name the field: {message}"
                );
            }
            other => panic!("expected a validation error for kind {kind}, got {other:?}"),
        }
    }
}

#[test]
fn rejects_classifier_model_with_default_max_tokens() {
    for kind in ["embedding", "classifier"] {
        let toml = catalog_with_model_kind(kind, "default_max_tokens = 1024");
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("model m"),
                    "expected the error to name the model: {message}"
                );
                assert!(
                    message.contains("default_max_tokens"),
                    "expected the error to name the field: {message}"
                );
            }
            other => panic!("expected a validation error for kind {kind}, got {other:?}"),
        }
    }
}

#[test]
fn rejects_embedding_local_model_with_thinking() {
    for kind in ["embedding", "classifier"] {
        let toml = catalog_with_local_model_kind(kind, "thinking = \"switchable\"");
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("local_model q"),
                    "expected the error to name the model: {message}"
                );
                assert!(
                    message.contains("thinking"),
                    "expected the error to name the field: {message}"
                );
            }
            other => panic!("expected a validation error for kind {kind}, got {other:?}"),
        }
    }
}

#[test]
fn rejects_classifier_local_model_with_chat_template_file() {
    for kind in ["embedding", "classifier"] {
        let toml = catalog_with_local_model_kind(kind, "chat_template_file = \"q.jinja\"");
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("local_model q"),
                    "expected the error to name the model: {message}"
                );
                assert!(
                    message.contains("chat_template_file"),
                    "expected the error to name the field: {message}"
                );
            }
            other => panic!("expected a validation error for kind {kind}, got {other:?}"),
        }
    }
}

#[test]
fn accepts_nonchat_models_with_context_and_inference_knobs() {
    // `context` applies to every kind, and the llama.cpp launch knobs
    // (gpu_layers, flash_attention, cache types, parallel, vram_gb) are not
    // chat-only: only thinking and chat-template/generation defaults are.
    let toml = catalog_with_model_kind("embedding", "");
    assert!(Config::parse_toml(&toml).is_ok());
    let toml = catalog_with_local_model_kind(
        "classifier",
        "gpu_layers = 40\nflash_attention = false\nparallel = 2",
    );
    assert!(Config::parse_toml(&toml).is_ok());
}

#[test]
fn accepts_chat_models_with_chat_only_fields() {
    let toml = catalog_with_model_kind(
        "chat",
        "thinking = \"switchable\"\ndefault_max_tokens = 1024",
    );
    assert!(Config::parse_toml(&toml).is_ok());
    let toml = catalog_with_local_model_kind(
        "chat",
        "thinking = \"always\"\nchat_template_file = \"q.jinja\"",
    );
    assert!(Config::parse_toml(&toml).is_ok());
}

#[test]
fn parses_speech_model_with_voices() {
    // The speech kind and its voice catalog parse and re-serialize verbatim.
    let toml = catalog_with_model_kind("speech", "voices = [\"alloy\", \"nova\"]");
    let config = Config::from_toml_str(&toml).unwrap();
    let model = &config.models()[0];
    assert_eq!(model.kind().to_string(), "speech");
    let json = serde_json::to_value(model).expect("serializes");
    assert_eq!(json["kind"], "speech");
    assert_eq!(json["voices"], serde_json::json!(["alloy", "nova"]));

    let toml = catalog_with_local_model_kind("speech", "voices = [\"alloy\"]");
    let config = Config::from_toml_str(&toml).unwrap();
    let json = serde_json::to_value(&config.local_models()[0]).expect("serializes");
    assert_eq!(json["kind"], "speech");
    assert_eq!(json["voices"], serde_json::json!(["alloy"]));
}

#[test]
fn accepts_speech_model_with_empty_or_absent_voices() {
    // An empty list stays valid; the route skips the voice check for it.
    let toml = catalog_with_model_kind("speech", "voices = []");
    assert!(Config::parse_toml(&toml).is_ok());
    let toml = catalog_with_model_kind("speech", "");
    assert!(Config::parse_toml(&toml).is_ok());
    let toml = catalog_with_local_model_kind("speech", "");
    assert!(Config::parse_toml(&toml).is_ok());
}

#[test]
fn rejects_chat_only_fields_on_speech_models() {
    // Speech is a non-chat kind: the chat-only discipline covers it.
    for (field, extra) in [
        ("thinking", "thinking = \"always\""),
        ("default_max_tokens", "default_max_tokens = 1024"),
        ("tool_dialect", "tool_dialect = \"gemma3_tool_code\""),
        ("effort_levels", "effort_levels = [\"low\"]"),
        ("default_effort", "default_effort = \"low\""),
        ("adaptive_thinking", "adaptive_thinking = true"),
    ] {
        let toml = catalog_with_model_kind("speech", extra);
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains(field),
                    "expected the error to name {field}: {message}"
                );
            }
            other => panic!("expected a validation error for {field}, got {other:?}"),
        }
    }
    for (field, extra) in [
        ("thinking", "thinking = \"always\""),
        ("chat_template_file", "chat_template_file = \"q.jinja\""),
        ("effort_levels", "effort_levels = [\"low\"]"),
        ("adaptive_thinking", "adaptive_thinking = true"),
        (
            "speculative",
            "[local_model.speculative]\ntype = \"draft-mtp\"\nsource = \"/models/d.gguf\"\nsha256 = \"b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32\"\ndraft_max = 7",
        ),
        (
            "multimodal_projector",
            "[local_model.multimodal_projector]\nsource = \"/models/p.gguf\"\nsha256 = \"b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32\"",
        ),
    ] {
        let toml = catalog_with_local_model_kind("speech", extra);
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains(field),
                    "expected the error to name {field}: {message}"
                );
            }
            other => panic!("expected a validation error for local {field}, got {other:?}"),
        }
    }
}

#[test]
fn rejects_voices_on_non_speech_models() {
    // `voices` is speech-only, symmetric with the chat-only discipline.
    for kind in ["chat", "embedding", "classifier"] {
        let toml = catalog_with_model_kind(kind, "voices = [\"alloy\"]");
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("voices"),
                    "expected the error to name voices: {message}"
                );
            }
            other => panic!("expected a validation error for kind {kind}, got {other:?}"),
        }
        let toml = catalog_with_local_model_kind(kind, "voices = [\"alloy\"]");
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("voices"),
                    "expected the error to name voices: {message}"
                );
            }
            other => panic!("expected a local validation error for kind {kind}, got {other:?}"),
        }
    }
}

#[test]
fn rejects_empty_voice_entries() {
    for extra in ["voices = [\"\"]", "voices = [\"alloy\", \"\", \"nova\"]"] {
        let toml = catalog_with_model_kind("speech", extra);
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => {
                assert!(
                    message.contains("voices"),
                    "expected the error to name voices: {message}"
                );
            }
            other => panic!("expected a validation error for {extra:?}, got {other:?}"),
        }
        let toml = catalog_with_local_model_kind("speech", extra);
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
            "expected local_model {extra:?} to be rejected"
        );
    }
}

#[test]
fn rejects_duplicate_voices() {
    let toml = catalog_with_model_kind("speech", "voices = [\"alloy\", \"nova\", \"alloy\"]");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("alloy"),
                "expected the error to name the duplicate: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
    let toml = catalog_with_local_model_kind("speech", "voices = [\"alloy\", \"alloy\"]");
    assert!(
        matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
        "expected local_model duplicate voices to be rejected"
    );
}
