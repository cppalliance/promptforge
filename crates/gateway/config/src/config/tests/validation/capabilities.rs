//! Tests for model capability metadata and effort knobs.

use super::*;

#[test]
fn parses_model_capabilities() {
    let toml = catalog_with_model_kind(
        "chat",
        r#"thinking = "switchable"
max_output = 4096
default_temperature = 0.7
images = true
parallel_tool_calls = true
effort_levels = ["low", "high"]
default_effort = "low"
adaptive_thinking = true"#,
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let capabilities = config.models()[0].capabilities();
    assert_eq!(capabilities.max_output(), Some(4096));
    assert_eq!(capabilities.default_temperature(), Some(0.7));
    assert!(capabilities.images());
    assert!(capabilities.parallel_tool_calls());
    assert_eq!(capabilities.effort_levels(), ["low", "high"]);
    assert_eq!(capabilities.default_effort(), Some("low"));
    assert!(capabilities.adaptive_thinking());
}

#[test]
fn capabilities_default_to_absent() {
    let toml = catalog_with_model_kind("chat", "");
    let config = Config::from_toml_str(&toml).unwrap();
    let capabilities = config.models()[0].capabilities();
    assert_eq!(capabilities.max_output(), None);
    assert_eq!(capabilities.default_temperature(), None);
    assert!(!capabilities.images());
    assert!(!capabilities.parallel_tool_calls());
    assert!(capabilities.effort_levels().is_empty());
    assert_eq!(capabilities.default_effort(), None);
    assert!(!capabilities.adaptive_thinking());
}

#[test]
fn rejects_default_effort_without_effort_levels() {
    let toml = catalog_with_model_kind(
        "chat",
        "thinking = \"switchable\"\ndefault_effort = \"low\"",
    );
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("effort_levels"),
                "expected the error to name effort_levels: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn rejects_default_effort_naming_an_unlisted_level() {
    let toml = catalog_with_model_kind(
        "chat",
        "thinking = \"switchable\"\neffort_levels = [\"low\", \"high\"]\ndefault_effort = \"max\"",
    );
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("max"),
                "expected the error to name the unlisted level: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn rejects_effort_fields_when_thinking_is_never() {
    // Effort levels meter thinking tokens; a model that never thinks has no
    // use for them. `thinking` defaults to `never`, so the fields are
    // rejected even when the key is absent.
    for extra in [
        "effort_levels = [\"low\"]",
        "effort_levels = [\"low\"]\ndefault_effort = \"low\"",
    ] {
        let toml = catalog_with_model_kind("chat", extra);
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
            "expected {extra:?} to be rejected when thinking is never"
        );
        let toml = catalog_with_local_model_kind("chat", extra);
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
            "expected local_model {extra:?} to be rejected when thinking is never"
        );
    }
}

#[test]
fn rejects_max_output_exceeding_context() {
    let toml = catalog_with_model_kind("chat", "max_output = 8193");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("max_output"),
                "expected the error to name max_output: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
    let toml = catalog_with_local_model_kind("chat", "max_output = 4097");
    assert!(matches!(
        Config::parse_toml(&toml),
        Err(ConfigError::Validation(_))
    ));
    // An exact fit is accepted.
    let toml = catalog_with_model_kind("chat", "max_output = 8192");
    assert!(Config::parse_toml(&toml).is_ok());
}

#[test]
fn rejects_nonchat_model_with_capability_effort_fields() {
    // The effort knobs and adaptive_thinking are chat-only, same as
    // `thinking` itself.
    for kind in ["embedding", "classifier"] {
        for (field, extra) in [
            ("effort_levels", "effort_levels = [\"low\"]"),
            (
                // effort_levels is rejected first; both names contain "effort".
                "effort",
                "effort_levels = [\"low\"]\ndefault_effort = \"low\"",
            ),
            ("adaptive_thinking", "adaptive_thinking = true"),
        ] {
            let toml = catalog_with_model_kind(kind, extra);
            match Config::parse_toml(&toml) {
                Err(ConfigError::Validation(message)) => {
                    assert!(
                        message.contains(field),
                        "expected the error to name {field}: {message}"
                    );
                }
                other => panic!("expected a validation error for kind {kind}, got {other:?}"),
            }
            let toml = catalog_with_local_model_kind(kind, extra);
            assert!(
                matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
                "expected local_model {field} to be rejected for kind {kind}"
            );
        }
    }
}
