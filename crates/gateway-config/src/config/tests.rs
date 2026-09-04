use super::*;

const SAMPLE: &str = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""

[[model]]
name = "m1"
description = "a small test model"
context = 8192
upstream = "u1"
endpoints = ["anthropic"]
"#;

#[test]
fn parses_a_valid_config() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert_eq!(config.endpoints.len(), 1);
    assert_eq!(config.models[0].name, "m1");
    assert_eq!(config.models[0].description, "a small test model");
    assert_eq!(config.models[0].context, 8192);
    assert_eq!(config.models[0].thinking, ThinkingMode::Never);
    assert_eq!(config.models[0].upstream, "u1");
}

#[test]
fn rejects_model_missing_description() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_model_missing_context() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
upstream = "u"
endpoints = ["anthropic"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_empty_server_key() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = ""

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn server_trust_loopback_defaults_to_true() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert!(
        config.server().trust_loopback(),
        "a config naming no `trust_loopback` keeps the keyless-loopback default"
    );
}

#[test]
fn server_trust_loopback_explicit_false_parses() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"
trust_loopback = false
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert!(!config.server().trust_loopback());
}

#[test]
fn server_trust_loopback_round_trips_through_json() {
    for (toml, expected) in [
        (
            "config-version = 2\n[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"t\"\n",
            true,
        ),
        (
            "config-version = 2\n[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"t\"\ntrust_loopback = false\n",
            false,
        ),
    ] {
        let raw: RawConfig = toml::from_str(toml).unwrap();
        let json = serde_json::to_value(&raw).unwrap();
        assert_eq!(json["server"]["trust_loopback"], expected, "{toml}");
        let back: RawConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.server.trust_loopback, expected, "{toml}");
    }
}

#[test]
fn rejects_web_search_default_count_over_max() {
    let toml = r#"
config-version = 2
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
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[tools.web_search]
provider = "brave"
api_key = "k"
base_url = "https://api.search.brave.com/res/v1"
default_count = 30
max_count = 10
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_web_search_non_http_base_url() {
    let toml = r#"
config-version = 2
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
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[tools.web_search]
provider = "brave"
api_key = "k"
base_url = "ftp://nope"
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_plaintext_http_local_model_source() {
    let sha = "a".repeat(64);
    let toml = format!(
        r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "m"
description = "a local model"
source = "http://example.com/m.gguf"
sha256 = "{sha}"
context = 4096
"#
    );
    assert!(matches!(
        Config::parse_toml(&toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_unknown_model_key() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
mystery = true
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn parses_thinking_modes() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
thinking = "switchable"
upstream = "u"
endpoints = ["anthropic"]
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.models[0].thinking, ThinkingMode::Switchable);
}

#[test]
fn model_kind_defaults_to_chat() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert_eq!(config.models[0].kind, ModelKind::Chat);

    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.local_models[0].kind, ModelKind::Chat);
}

#[test]
fn parses_model_kinds() {
    let toml = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "embed"
kind = "embedding"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "rerank"
kind = "classifier"
description = "prose"
source = "/models/r.gguf"
context = 4096
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.models[0].kind, ModelKind::Embedding);
    assert_eq!(config.local_models[0].kind, ModelKind::Classifier);
}

#[test]
fn rejects_unknown_model_kind() {
    let toml = r#"
config-version = 2
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
kind = "rerank"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn tool_dialect_defaults_to_openai() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert_eq!(config.models[0].tool_dialect, ToolDialect::Openai);
}

#[test]
fn parses_gemma3_tool_code_dialect() {
    let toml = r#"
config-version = 2
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
description = "prose"
context = 8192
tool_dialect = "gemma3_tool_code"
upstream = "u"
endpoints = ["e"]
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.models[0].tool_dialect, ToolDialect::Gemma3ToolCode);
}

#[test]
fn rejects_unknown_tool_dialect() {
    let toml = r#"
config-version = 2
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
description = "prose"
context = 8192
tool_dialect = "anthropic"
upstream = "u"
endpoints = ["e"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn rejects_tool_dialect_on_a_non_chat_model() {
    let toml = r#"
config-version = 2
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
kind = "embedding"
description = "prose"
context = 8192
tool_dialect = "gemma3_tool_code"
upstream = "u"
endpoints = ["e"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn interpolates_and_escapes() {
    // SAFETY-free: reading is fine; this test sets no env vars.
    assert_eq!(interpolate("a$$b").unwrap(), "a$b");
    assert_eq!(interpolate("no vars here").unwrap(), "no vars here");
}

#[test]
fn interpolation_ignores_comments_and_keys() {
    // CFG-007: a `${VAR}` inside a comment is not interpolated, so an unset
    // variable there must not fail the load. Raw-text interpolation would have
    // tried to resolve it and errored.
    let toml = r#"
# a comment mentioning ${PROMPTFORGE_DEFINITELY_UNSET_VAR_XYZ}
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "a $$-priced model"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
    let config = Config::from_toml_str(toml).expect("comment vars are not interpolated");
    // `$$` in a string value still unescapes to a single `$`.
    assert_eq!(config.models[0].description, "a $-priced model");
}

#[test]
fn unresolved_variable_is_an_error() {
    let missing = "${PROMPTFORGE_DEFINITELY_UNSET_VAR_XYZ}";
    assert!(matches!(
        interpolate(missing),
        Err(ConfigError::UnresolvedVar(_))
    ));
}

#[test]
fn unclosed_interpolation_is_an_error() {
    assert!(matches!(
        interpolate("${OPEN"),
        Err(ConfigError::Interpolation(_))
    ));
}

mod schema;
mod serialize;
mod validation;
