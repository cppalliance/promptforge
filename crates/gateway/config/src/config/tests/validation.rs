//! Tests for the config validation rules that reject malformed or unknown sections.

use super::super::*;
use super::SAMPLE;

#[test]
fn rejects_legacy_stt_section() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[workshop.stt]
window_seconds = 8
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_canonical_and_legacy_stt_sections_together() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[stt]
window_seconds = 8

[workshop.stt]
interval_ms = 250
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

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
fn rejects_legacy_queue_section() {
    // `deny_unknown_fields` on the root DTO rejects a `[queue]` section at
    // parse time.
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[queue]
max_depth = 50
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_legacy_device_section() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[device]]
id = "gpu"
type = "remote"
concurrency = 4
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_legacy_endpoint_concurrency_and_device() {
    // `endpoint.concurrency` and `endpoint.device` are unknown keys, so they
    // fail `deny_unknown_fields` at parse time.
    for legacy_key in ["concurrency = 4", "device = \"runpod\""] {
        let toml = config_with_endpoint(&format!(
            r#"[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
{legacy_key}"#
        ));
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Parse { .. })),
            "expected legacy endpoint key {legacy_key:?} to be rejected"
        );
    }
}

#[test]
fn rejects_legacy_local_model_device_and_lane() {
    for legacy_key in ["device = \"gpu0\"", "lane = \"generative\""] {
        let toml = format!(
            r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
{legacy_key}
"#
        );
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Parse { .. })),
            "expected legacy local_model key {legacy_key:?} to be rejected"
        );
    }
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
