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
fn rejects_legacy_queue_section() {
    // The legacy `[queue]` section is gone (absorbed into `[[dominion]]`);
    // `deny_unknown_fields` on the root DTO rejects it at parse time.
    let toml = r#"
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
    // `endpoint.concurrency`/`device` are gone: one way to cap is a dominion
    // binding, so the legacy keys fail `deny_unknown_fields` at parse time.
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
fn parses_dominions_and_bindings() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "runpod-pool"
kind = "remote"
max_concurrency = 4
max_queue = 50
policy = "reject"
fair_scheduling = false

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
dominion = "runpod-pool"

[[model]]
name = "m"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
dominion = "gpu0"
parallel = 4
vram_gb = 14
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.dominions.len(), 2);
    let remote = &config.dominions[0];
    assert_eq!(remote.id, "runpod-pool");
    assert_eq!(remote.kind, DominionKind::Remote);
    assert_eq!(remote.max_concurrency, Some(4));
    assert_eq!(remote.max_queue, 50);
    assert_eq!(remote.policy, QueuePolicy::Reject);
    assert!(!remote.fair_scheduling);
    assert_eq!(remote.vram_gb, None);
    let local = &config.dominions[1];
    assert_eq!(local.kind, DominionKind::Local);
    assert_eq!(local.vram_gb, Some(24));
    assert_eq!(config.endpoints[0].dominion.as_deref(), Some("runpod-pool"));
    let model = &config.local_models[0];
    assert_eq!(model.dominion.as_deref(), Some("gpu0"));
    assert_eq!(model.parallel, 4);
    assert_eq!(model.vram_gb, Some(14));
}

#[test]
fn dominion_defaults_apply() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "runpod-pool"
kind = "remote"
"#;
    let config = Config::from_toml_str(toml).unwrap();
    let dominion = &config.dominions[0];
    assert_eq!(dominion.max_concurrency, None);
    assert_eq!(dominion.max_queue, 100);
    assert_eq!(dominion.policy, QueuePolicy::Queue);
    assert!(dominion.fair_scheduling);
    assert_eq!(dominion.vram_gb, None);
}

#[test]
fn rejects_empty_dominion_id() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = ""
kind = "remote"
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_duplicate_dominion_id() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "dup"
kind = "remote"

[[dominion]]
id = "dup"
kind = "local"
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_zero_dominion_max_concurrency() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "runpod-pool"
kind = "remote"
max_concurrency = 0
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_zero_dominion_max_queue() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "runpod-pool"
kind = "remote"
max_queue = 0
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_remote_dominion_with_vram_gb() {
    // Kind-incompatible payloads are rejected, same spirit as CFG-004: a VRAM
    // budget is meaningful only for a local GPU.
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "runpod-pool"
kind = "remote"
vram_gb = 24
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_endpoint_naming_undefined_dominion() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
dominion = "missing"

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
fn rejects_endpoint_naming_local_dominion() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""
dominion = "gpu0"

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
fn rejects_local_model_naming_undefined_dominion() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
dominion = "missing"
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_local_model_naming_remote_dominion() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "runpod-pool"
kind = "remote"

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
dominion = "runpod-pool"
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_zero_local_model_parallel() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "prose"
source = "/models/q.gguf"
context = 4096
parallel = 0
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

#[test]
fn rejects_vram_budget_overflow() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14
"#;
    match Config::parse_toml(toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
            assert!(
                message.contains("exceeded by 4"),
                "expected the error to name the overflow amount: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn rejects_bound_model_without_vram_estimate() {
    // Budgets must be complete to be meaningful: a model bound to a budgeted
    // dominion without its own estimate is an error.
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
"#;
    match Config::parse_toml(toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("local_model b "),
                "expected the error to name the model: {message}"
            );
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn accepts_exact_vram_fit() {
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 10
"#;
    assert!(Config::parse_toml(toml).is_ok());
}

#[test]
fn accepts_bound_models_when_dominion_has_no_budget() {
    // A local dominion without vram_gb imposes no co-residency obligation:
    // bound models need no estimate.
    let toml = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14
"#;
    assert!(Config::parse_toml(toml).is_ok());
}

#[test]
fn allowlist_selects_a_subset_of_the_catalog() {
    // The top-level `models` key coexists with the `[[model]]` definition
    // array and filters both remote and local models to the listed names.
    let toml = r#"
models = ["m1", "q1"]

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m1"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "m2"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "q1"
description = "prose"
source = "/models/q1.gguf"
context = 4096

[[local_model]]
name = "q2"
description = "prose"
source = "/models/q2.gguf"
context = 4096
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(
        config.model_allowlist(),
        Some(&["m1".to_string(), "q1".to_string()][..])
    );
    let remote: Vec<&str> = config.models().iter().map(ModelConfig::name).collect();
    let local: Vec<&str> = config
        .local_models()
        .iter()
        .map(LocalModelConfig::name)
        .collect();
    assert_eq!(remote, ["m1"]);
    assert_eq!(local, ["q1"]);
}

#[test]
fn allowlist_unknown_name_is_a_validation_error() {
    let toml = r#"
models = ["m", "ghost"]

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
"#;
    match Config::parse_toml(toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("ghost"),
                "expected the error to name the unknown model: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn absent_allowlist_loads_the_full_catalog() {
    let config = Config::from_toml_str(SAMPLE).unwrap();
    assert_eq!(config.model_allowlist(), None);
    assert_eq!(config.models().len(), 1);
}

#[test]
fn allowlist_filter_runs_before_reference_validation() {
    // The filtered-out model names an undefined endpoint; because the filter
    // runs before validation, its dangling reference is never checked.
    let toml = r#"
models = ["good"]

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "good"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "dangling"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["ghost"]
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.models().len(), 1);
    assert_eq!(config.models()[0].name(), "good");
}

/// A catalog whose two local models over-book `gpu0` in total; the variable
/// part is the top-level `models` allowlist.
fn overbooked_catalog_with_allowlist(allowlist: &str) -> String {
    format!(
        r#"
models = {allowlist}

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14

[[local_model]]
name = "b"
description = "prose"
source = "/models/b.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 14
"#
    )
}

#[test]
fn allowlist_filter_runs_before_the_vram_check() {
    // The full catalog over-books gpu0 (14 + 14 > 24), but the profile's
    // selection fits: the VRAM co-residency check operates on the loaded set.
    let toml = overbooked_catalog_with_allowlist("[\"a\"]");
    let config = Config::from_toml_str(&toml).unwrap();
    assert_eq!(config.local_models().len(), 1);
    assert_eq!(config.local_models()[0].name(), "a");
}

#[test]
fn allowlisted_overbooking_still_fails() {
    let toml = overbooked_catalog_with_allowlist("[\"a\", \"b\"]");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
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
