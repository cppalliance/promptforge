//! Tests for endpoint, remote model routing, and web-search tool validation.

use super::*;

#[test]
fn rejects_duplicate_endpoint_names() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "dup"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[endpoint]]
id = "dup"
protocol = "openai"
base_url = "http://b"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["dup"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_model_naming_undefined_endpoint() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "real"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["ghost"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_model_with_no_endpoints() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "real"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = []
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn parses_web_search_tool_config() {
    let toml = r#"
config-version = 0
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

[tools.web_search]
provider = "brave"
api_key = "secret-key"
"#;
    let config = Config::from_toml_str(toml).unwrap();
    let tools = config.tools.expect("tools section present");
    let web_search = tools.web_search.expect("web_search section present");
    assert_eq!(web_search.provider, SearchProvider::Brave);
    assert_eq!(web_search.api_key.expose(), "secret-key");
    assert_eq!(web_search.base_url, "https://api.search.brave.com/res/v1");
    assert_eq!(web_search.default_count, 10);
    assert_eq!(web_search.max_count, 20);
    assert_eq!(web_search.max_per_host, 2);
    assert_eq!(web_search.default_freshness, "");
    assert_eq!(web_search.default_safesearch, "");
    assert!(web_search.strip_tracking);
}

#[test]
fn parses_web_search_tool_config_explicit_defaults() {
    let toml = r#"
config-version = 0
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

[tools.web_search]
provider = "brave"
api_key = "secret-key"
default_count = 5
max_count = 15
max_per_host = 3
default_freshness = "pw"
default_safesearch = "moderate"
strip_tracking = false
"#;
    let config = Config::from_toml_str(toml).unwrap();
    let tools = config.tools.expect("tools section present");
    let web_search = tools.web_search.expect("web_search section present");
    assert_eq!(web_search.default_count, 5);
    assert_eq!(web_search.max_count, 15);
    assert_eq!(web_search.max_per_host, 3);
    assert_eq!(web_search.default_freshness, "pw");
    assert_eq!(web_search.default_safesearch, "moderate");
    assert!(!web_search.strip_tracking);
}

#[test]
fn parses_config_without_tools_section() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert!(config.tools.is_none());
}

#[test]
fn rejects_empty_endpoint_id() {
    // CFG-003: a blank endpoint id can never be referenced and shadows the
    // unnamed slot.
    let toml = config_with_endpoint(
        r#"[[endpoint]]
id = ""
protocol = "openai"
base_url = "http://a"
api_key = """#,
    );
    assert!(matches!(
        Config::parse_toml(&toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_malformed_endpoint_base_url() {
    // CFG-003 / UP-005: base_url must parse as an absolute HTTP(S) URL, not any
    // string that later gets concatenated with a path.
    for bad in ["not-a-url", "127.0.0.1:9", "ftp://example.com", ""] {
        let toml = config_with_endpoint(&format!(
            r#"[[endpoint]]
id = "e"
protocol = "openai"
base_url = "{bad}"
api_key = """#
        ));
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
            "expected base_url {bad:?} to be rejected"
        );
    }
}

#[test]
fn accepts_well_formed_endpoint_base_url() {
    for good in ["http://127.0.0.1:9", "https://api.example.com/v1"] {
        let toml = config_with_endpoint(&format!(
            r#"[[endpoint]]
id = "e"
protocol = "openai"
base_url = "{good}"
api_key = """#
        ));
        assert!(
            Config::parse_toml(&toml).is_ok(),
            "expected base_url {good:?} to be accepted"
        );
    }
}

/// A config whose only variable part is the two web-search knob lines under a
/// valid `[tools.web_search]` section.
fn config_with_web_search_knobs(freshness: &str, safesearch: &str) -> String {
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
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[tools.web_search]
provider = "brave"
api_key = "k"
default_freshness = "{freshness}"
default_safesearch = "{safesearch}"
"#
    )
}

#[test]
fn rejects_invalid_web_search_freshness() {
    // CFG-006: freshness is a closed vocabulary, not an arbitrary string.
    for bad in ["daily", "p1", "yesterday"] {
        let toml = config_with_web_search_knobs(bad, "");
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
            "expected freshness {bad:?} to be rejected"
        );
    }
}

#[test]
fn rejects_invalid_web_search_safesearch() {
    // CFG-006: safesearch is off/moderate/strict (or empty), nothing else.
    for bad in ["on", "medium", "safe"] {
        let toml = config_with_web_search_knobs("", bad);
        assert!(
            matches!(Config::parse_toml(&toml), Err(ConfigError::Validation(_))),
            "expected safesearch {bad:?} to be rejected"
        );
    }
}

#[test]
fn accepts_valid_web_search_knobs() {
    for (freshness, safesearch) in [
        ("", ""),
        ("pd", "off"),
        ("pw", "moderate"),
        ("2024-01-01to2024-12-31", "strict"),
    ] {
        let toml = config_with_web_search_knobs(freshness, safesearch);
        assert!(
            Config::parse_toml(&toml).is_ok(),
            "expected freshness {freshness:?}/safesearch {safesearch:?} to be accepted"
        );
    }
}

#[test]
fn rejects_web_search_non_url_base() {
    // CFG-006: the base URL is parsed, not prefix-matched.
    let toml = r#"
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
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[tools.web_search]
provider = "brave"
api_key = "k"
base_url = "https://"
"#;
    // `https://` passes a naive `starts_with("https://")` prefix check but has no
    // host, so only a real parse rejects it (CFG-006).
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}
