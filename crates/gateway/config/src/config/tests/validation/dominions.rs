//! Tests for `[[dominion]]` parsing and the bindings that reference it.

use super::*;

#[test]
fn parses_dominions_and_bindings() {
    let toml = r#"
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
config-version = 0
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
