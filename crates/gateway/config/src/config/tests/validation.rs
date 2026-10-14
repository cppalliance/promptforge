//! Tests for the config validation rules that reject malformed or unknown sections.

use super::super::*;
use super::SAMPLE;

#[test]
fn rejects_zero_stt_pipeline_bounds() {
    for field in ["window_seconds = 0", "interval_ms = 0"] {
        let toml = format!(
            "config-version = 0\n[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"t\"\n\
             [stt]\n{field}\n"
        );
        assert!(
            matches!(
                Config::from_toml_str(&toml),
                Err(error) if error.kind() == crate::ConfigErrorKind::Validation
            ),
            "zero STT bound must fail: {field}"
        );
    }
}

#[test]
fn secret_redacts() {
    let s = Secret::new("hunter2".to_string());
    assert_eq!(format!("{s}"), "redacted");
    assert_eq!(format!("{s:?}"), "Secret(redacted)");
    assert_eq!(s.expose(), "hunter2");
}

#[test]
fn rejects_an_unknown_root_section() {
    // `deny_unknown_fields` on the root DTO rejects an unknown section at
    // parse time.
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[unknown]
key = 50
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_an_unknown_endpoint_key() {
    let toml = config_with_endpoint(
        r#"[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
unknown = 4"#,
    );
    assert!(matches!(
        Config::parse_toml(&toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_an_unknown_local_model_key() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
unknown = "x"
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

/// A config whose only variable part is one `[[endpoint]]` block. Table-driven
/// endpoint-validation tests substitute the block to exercise one invariant each.
fn config_with_endpoint(endpoint_block: &str) -> String {
    format!(
        r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

{endpoint_block}

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
"#
    )
}

/// A catalog with one endpoint and one model of the given kind; `extra`
/// supplies the model's variable field lines.
fn catalog_with_model_kind(kind: &str, extra: &str) -> String {
    format!(
        r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
kind = "{kind}"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
{extra}
"#
    )
}

/// A catalog with one local model of the given kind; `extra` supplies the
/// model's variable field lines.
fn catalog_with_local_model_kind(kind: &str, extra: &str) -> String {
    format!(
        r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
kind = "{kind}"
description = "prose"
source = "/models/q.gguf"
context = 4096
{extra}
"#
    )
}

mod capabilities;
mod dominions;
mod endpoints;
mod kinds;
mod local_models;
mod profiles;
mod vram;
