use super::super::*;
use super::SAMPLE;

#[test]
fn rejects_remote_local_model_without_digest() {
    // ART-002: a remote (https) local_model source must be pinned by sha256.
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "unpinned remote"
source = "https://example.com/model.gguf"
context = 1024
"#;
    assert!(matches!(
        Config::from_toml_str(toml),
        Err(err) if err.kind() == crate::ConfigErrorKind::Validation
    ));
}

#[test]
fn rejects_duplicate_endpoint_names() {
    let toml = r#"
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
fn secret_redacts() {
    let s = Secret::new("hunter2".to_string());
    assert_eq!(format!("{s}"), "redacted");
    assert_eq!(format!("{s:?}"), "Secret(redacted)");
    assert_eq!(s.expose(), "hunter2");
}

#[test]
fn parses_queue_and_endpoint_concurrency() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[queue]
max_depth = 50
fair_scheduling = false

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = ""
concurrency = 4

[[model]]
name = "m1"
description = "a small test model"
context = 8192
upstream = "u1"
endpoints = ["anthropic"]
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.queue.max_depth, 50);
    assert!(!config.queue.fair_scheduling);
    assert_eq!(config.endpoints[0].concurrency, Some(4));
}

#[test]
fn queue_defaults_when_section_absent() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert_eq!(config.queue.max_depth, 100);
    assert!(config.queue.fair_scheduling);
    assert_eq!(config.endpoints[0].concurrency, None);
}

#[test]
fn rejects_zero_endpoint_concurrency() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""
concurrency = 0

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_zero_queue_max_depth() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[queue]
max_depth = 0

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
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn parses_local_model_with_defaults() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "qwen-local"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "/models/model.gguf"
context = 65536
thinking = "never"
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert!(config.endpoints.is_empty());
    assert!(config.models.is_empty());
    assert_eq!(config.local_models.len(), 1);
    let model = &config.local_models[0];
    assert_eq!(model.name, "qwen-local");
    assert_eq!(model.context, 65536);
    assert_eq!(model.thinking, ThinkingMode::Never);
    assert_eq!(model.gpu_layers, 99);
    assert!(model.flash_attention);
    assert_eq!(model.cache_type_k, "q8_0");
    assert_eq!(model.cache_type_v, "q4_0");
    assert_eq!(model.n_predict, 8192);
    assert!(model.sha256.is_none());
    assert!(config.local.cache_dir.is_none());
}

#[test]
fn parses_local_model_knobs_and_cache_dir() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
cache_dir = "/tmp/pf-models"

[[local_model]]
name = "qwen-local"
description = "prose"
source = "https://example.com/model.gguf"
sha256 = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8"
context = 4096
gpu_layers = 40
flash_attention = false
cache_type_k = "f16"
cache_type_v = "f16"
n_predict = 256
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.local.cache_dir.as_deref(), Some("/tmp/pf-models"));
    let model = &config.local_models[0];
    assert_eq!(
        model.sha256.as_deref(),
        Some("03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8")
    );
    assert_eq!(model.gpu_layers, 40);
    assert!(!model.flash_attention);
    assert_eq!(model.cache_type_k, "f16");
    assert_eq!(model.n_predict, 256);
}

#[test]
fn rejects_duplicate_name_across_remote_and_local() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "shared"
description = "remote"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "shared"
description = "local"
source = "https://example.com/model.gguf"
context = 4096
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_invalid_local_model_sha256() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "https://example.com/model.gguf"
sha256 = "not-a-digest"
context = 4096
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_empty_local_model_source() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = ""
context = 4096
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn parses_devices_lanes_and_endpoint_device() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[device]]
id = "anthropic"
type = "remote"
concurrency = 7

[[device]]
id = "local-gpu"
type = "local"

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 1

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""
device = "anthropic"

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["anthropic"]

[[local_model]]
name = "q"
description = "prose"
source = "/models/m.gguf"
device = "local-gpu"
lane = "generative"
context = 4096
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.devices.len(), 2);
    assert_eq!(config.devices[0].kind, DeviceKind::Remote);
    assert_eq!(config.devices[0].concurrency, Some(7));
    assert_eq!(config.devices[1].lanes.len(), 1);
    assert_eq!(config.devices[1].lanes[0].id, "generative");
    assert_eq!(config.endpoint_concurrency(&config.endpoints[0]), Some(7));
    assert_eq!(
        config
            .local_model_concurrency(&config.local_models[0])
            .unwrap(),
        1
    );
}

#[test]
fn rejects_endpoint_naming_undefined_device() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
device = "missing"

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

/// A config whose only variable part is one `[[endpoint]]` block. Table-driven
/// endpoint-validation tests substitute the block to exercise one invariant each.
fn config_with_endpoint(endpoint_block: &str) -> String {
    format!(
        r#"
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
