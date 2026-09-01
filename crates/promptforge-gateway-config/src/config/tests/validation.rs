use super::super::*;
use super::SAMPLE;

#[test]
fn rejects_remote_local_model_without_digest() {
    // ART-002: a remote (https) local_model source must be pinned by sha256.
    let toml = r#"
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
    // Integer TOML still parses into the f64 field (pre-existing configs).
    assert_eq!(model.vram_gb, Some(14.0));
}

#[test]
fn dominion_defaults_apply() {
    let toml = r#"
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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
config-version = 2
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

[[profile]]
name = "overbooked"
models = ["a", "b"]
"#;
    match Config::parse_toml(toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(
                message.contains("gpu0"),
                "expected the error to name the dominion: {message}"
            );
            assert!(
                message.contains("by 4"),
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
config-version = 2
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

[[profile]]
name = "incomplete"
models = ["a", "b"]
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
config-version = 2
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

[[profile]]
name = "exact"
models = ["a", "b"]
"#;
    assert!(Config::parse_toml(toml).is_ok());
}

#[test]
fn accepts_fractional_local_model_vram_estimate() {
    // The Discover UI writes the quant file size in GiB rounded to two
    // decimals, e.g. 1.22 for a 1.2 GiB download. A u32 schema rejected
    // that for every non-whole-GiB model (workshop finding 30).
    let toml = r#"
config-version = 2
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
vram_gb = 1.22

[[profile]]
name = "fractional"
models = ["a"]
"#;
    let config = Config::parse_toml(toml).expect("fractional vram_gb parses");
    assert_eq!(config.local_models[0].vram_gb, Some(1.22));
}

#[test]
fn rejects_non_positive_local_model_vram_estimate() {
    for value in ["0.0", "-1.0", "nan", "inf"] {
        let toml = format!(
            r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "a"
description = "prose"
source = "/models/a.gguf"
context = 4096
vram_gb = {value}
"#
        );
        match Config::parse_toml(&toml) {
            Err(ConfigError::Validation(message)) => assert!(
                message.contains("vram_gb must be finite and greater than zero"),
                "expected the vram_gb error for {value}: {message}"
            ),
            other => panic!("expected a validation error for {value}, got {other:?}"),
        }
    }
}

#[test]
fn accepts_bound_models_when_dominion_has_no_budget() {
    // A local dominion without vram_gb imposes no co-residency obligation:
    // bound models need no estimate.
    let toml = r#"
config-version = 2
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
fn selected_profile_filters_each_catalog_kind() {
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
name = "remote"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "local"
description = "prose"
source = "/models/local.gguf"
context = 4096

[[stt_model]]
name = "interim"
role = "interim"
source = "/models/base.en.bin"
vram_gb = 1.0

[[profile]]
name = "work"
models = ["remote", "interim"]
"#;
    let catalog = Config::from_toml_str(toml).expect("catalog parses");
    let selected = catalog
        .select_profile(&crate::ProfileName::parse("work").expect("profile name"))
        .expect("profile selects");
    assert_eq!(selected.models()[0].name(), "remote");
    assert!(selected.local_models().is_empty());
    assert_eq!(selected.stt_models()[0].name(), "interim");
    assert_eq!(selected.catalog_local_models()[0].name(), "local");
}

#[test]
fn every_profile_reference_must_exist() {
    let toml = format!("{SAMPLE}\n[[profile]]\nname = \"unused\"\nmodels = [\"ghost\"]\n");
    match Config::parse_toml(&toml) {
        Err(ConfigError::Validation(message)) => {
            assert!(message.contains("unused"), "profile named: {message}");
            assert!(message.contains("ghost"), "missing model named: {message}");
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn unselected_catalog_entries_still_validate_references() {
    let toml = format!(
        "{SAMPLE}\n\
         [[model]]\nname = \"dangling\"\ndescription = \"prose\"\ncontext = 1\n\
         upstream = \"u\"\nendpoints = [\"ghost\"]\n\
         [[profile]]\nname = \"work\"\nmodels = [\"m1\"]\n"
    );
    assert!(matches!(
        Config::parse_toml(&toml),
        Err(ConfigError::Validation(message)) if message.contains("ghost")
    ));
}

/// A catalog whose two local models over-book `gpu0`; `models` is one
/// profile's checklist.
fn overbooked_catalog_with_profile(models: &str) -> String {
    format!(
        r#"
config-version = 2
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

[[profile]]
name = "selected"
models = {models}
"#
    )
}

#[test]
fn each_profile_vram_check_uses_its_own_subset() {
    let toml = overbooked_catalog_with_profile("[\"a\"]");
    let config = Config::from_toml_str(&toml).expect("single model fits");
    assert_eq!(config.profiles()[0].models(), ["a"]);
}

#[test]
fn any_overbooked_profile_rejects_the_whole_catalog() {
    let toml = overbooked_catalog_with_profile("[\"a\", \"b\"]");
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

fn stt_profile_config(entries: &str, models: &str) -> String {
    format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "t"

{entries}

[[profile]]
name = "work"
models = {models}
"#
    )
}

#[test]
fn profile_names_and_membership_are_unique() {
    let duplicate_names = format!(
        "{SAMPLE}\n\
         [[profile]]\nname = \"work\"\nmodels = []\n\
         [[profile]]\nname = \"work\"\nmodels = []\n"
    );
    assert!(matches!(
        Config::parse_toml(&duplicate_names),
        Err(ConfigError::Validation(message)) if message.contains("duplicate profile")
    ));

    let illegal_name = format!("{SAMPLE}\n[[profile]]\nname = \"../work\"\nmodels = []\n");
    assert!(matches!(
        Config::parse_toml(&illegal_name),
        Err(ConfigError::Validation(message)) if message.contains("../work")
    ));

    let duplicate_member =
        format!("{SAMPLE}\n[[profile]]\nname = \"work\"\nmodels = [\"m1\", \"m1\"]\n");
    assert!(matches!(
        Config::parse_toml(&duplicate_member),
        Err(ConfigError::Validation(message)) if message.contains("duplicate model m1")
    ));
}

#[test]
fn stt_role_pairing_rejects_duplicate_slots() {
    for (role, models) in [
        ("interim", "[\"a\", \"b\"]"),
        ("final", "[\"interim\", \"a\", \"b\"]"),
    ] {
        let prefix = if role == "final" {
            "[[stt_model]]\nname = \"interim\"\nrole = \"interim\"\n\
             source = \"/models/interim.bin\"\nvram_gb = 1.0\n"
        } else {
            ""
        };
        let entries = format!(
            "{prefix}\
             [[stt_model]]\nname = \"a\"\nrole = \"{role}\"\n\
             source = \"/models/a.bin\"\nvram_gb = 1.0\n\
             [[stt_model]]\nname = \"b\"\nrole = \"{role}\"\n\
             source = \"/models/b.bin\"\nvram_gb = 1.0\n"
        );
        assert!(
            matches!(
                Config::parse_toml(&stt_profile_config(&entries, models)),
                Err(ConfigError::Validation(message))
                    if message.contains("more than one") && message.contains("work")
            ),
            "duplicate {role} slot must fail"
        );
    }
}

#[test]
fn final_without_interim_names_the_fix() {
    let entries = r#"
[[stt_model]]
name = "final"
role = "final"
source = "/models/final.bin"
vram_gb = 2.0
"#;
    match Config::parse_toml(&stt_profile_config(entries, "[\"final\"]")) {
        Err(ConfigError::Validation(message)) => {
            assert!(message.contains("final"), "final model named: {message}");
            assert!(message.contains("add one interim"), "fix named: {message}");
        }
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn interim_without_final_is_supported_degraded_mode() {
    let entries = r#"
[[stt_model]]
name = "interim"
role = "interim"
source = "/models/interim.bin"
vram_gb = 1.0
"#;
    assert!(Config::parse_toml(&stt_profile_config(entries, "[\"interim\"]")).is_ok());
}

#[test]
fn remote_stt_source_may_omit_optional_digest() {
    let entries = r#"
[[stt_model]]
name = "interim"
role = "interim"
source = "https://example.com/models/interim.bin"
vram_gb = 1.0
"#;
    assert!(Config::parse_toml(&stt_profile_config(entries, "[\"interim\"]")).is_ok());
}

#[test]
fn stt_models_count_toward_each_profile_vram_budget() {
    let entries = r#"
[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 2

[[local_model]]
name = "chat"
description = "prose"
source = "/models/chat.gguf"
context = 4096
dominion = "gpu0"
vram_gb = 1

[[stt_model]]
name = "interim"
role = "interim"
source = "/models/interim.bin"
vram_gb = 1.5
dominion = "gpu0"
"#;
    assert!(matches!(
        Config::parse_toml(&stt_profile_config(entries, "[\"chat\", \"interim\"]")),
        Err(ConfigError::Validation(message))
            if message.contains("work") && message.contains("gpu0")
    ));
}

#[test]
fn stt_catalog_validates_source_pin_vram_and_dominion() {
    for (field, entry) in [
        (
            "source",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='http://x/s.bin'\nvram_gb=1.0",
        ),
        (
            "sha256",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='/s.bin'\nsha256='bad'\nvram_gb=1.0",
        ),
        (
            "vram_gb",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='/s.bin'\nvram_gb=0.0",
        ),
        (
            "dominion",
            "[[stt_model]]\nname='s'\nrole='interim'\nsource='/s.bin'\nvram_gb=1.0\ndominion='missing'",
        ),
    ] {
        assert!(
            matches!(
                Config::parse_toml(&stt_profile_config(entry, "[\"s\"]")),
                Err(ConfigError::Validation(message)) if message.contains(field)
            ),
            "invalid STT {field} must fail"
        );
    }
}

/// A config whose only variable part is one `[[endpoint]]` block. Table-driven
/// endpoint-validation tests substitute the block to exercise one invariant each.
fn config_with_endpoint(endpoint_block: &str) -> String {
    format!(
        r#"
config-version = 2
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
base_url = "https://"
"#;
    // `https://` passes a naive `starts_with("https://")` prefix check but has no
    // host, so only a real parse rejects it (CFG-006).
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}

/// A catalog with one endpoint and one model of the given kind; `extra`
/// carries the model's variable field lines.
fn catalog_with_model_kind(kind: &str, extra: &str) -> String {
    format!(
        r#"
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
kind = "{kind}"
description = "prose"
context = 8192
upstream = "u"
endpoints = ["e"]
{extra}
"#
    )
}

/// A catalog with one local model of the given kind; `extra` carries the
/// model's variable field lines.
fn catalog_with_local_model_kind(kind: &str, extra: &str) -> String {
    format!(
        r#"
config-version = 2
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
                // effort_levels is rejected first; both names carry "effort".
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
