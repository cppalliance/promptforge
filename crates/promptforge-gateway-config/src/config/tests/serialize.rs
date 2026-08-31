use super::super::*;

/// A fixture exercising every config struct, every enum spelling, and all
/// three `Secret` fields.
const FULL: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "server-secret-value"

[local]
cache_dir = "/tmp/pf-cache"

[[dominion]]
id = "gpu0"
kind = "local"
max_concurrency = 4
max_queue = 50
policy = "reject"
fair_scheduling = false
vram_gb = 24

[[dominion]]
id = "remote-pool"
kind = "remote"

[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "endpoint-secret-value"
dominion = "remote-pool"

[[model]]
name = "gpt"
kind = "chat"
description = "a remote model"
context = 8192
thinking = "switchable"
upstream = "gpt-x"
endpoints = ["openai"]
default_max_tokens = 1024
tool_dialect = "gemma3_tool_code"
max_output = 4096
default_temperature = 0.5
images = true
parallel_tool_calls = true
effort_levels = ["low", "high"]
default_effort = "low"
adaptive_thinking = true

[[local_model]]
name = "gemma"
kind = "chat"
description = "a local model"
source = "/models/gemma.gguf"
sha256 = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32"
dominion = "gpu0"
parallel = 2
vram_gb = 20
context = 131072
thinking = "always"
gpu_layers = 40
flash_attention = false
cache_type_k = "f16"
cache_type_v = "q8_0"
n_predict = 2048
chat_template_file = "/models/gemma.jinja"
max_output = 8192

[local_model.speculative]
type = "draft-mtp"
source = "/models/gemma-mtp.gguf"
sha256 = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32"
draft_max = 7

[local_model.multimodal_projector]
source = "/models/gemma-mmproj.gguf"
sha256 = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32"

[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://example.com/ggml-base.en.bin"
sha256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
vram_gb = 1.0
dominion = "gpu0"

[tools.web_search]
provider = "brave"
api_key = "search-secret-value"
base_url = "https://api.search.brave.com/res/v1"
default_count = 5
max_count = 15
max_per_host = 3
default_freshness = "pw"
default_safesearch = "strict"
strip_tracking = false

[workshop]
bind = "127.0.0.1:7999"
open_browser = true

[workshop.stt]
window_seconds = 8
interval_ms = 250
vocabulary = ["MCP", "GGUF"]

[workshop.tape]
path = "session.jsonl"

[[profile]]
name = "work"
models = ["gpt", "gemma", "whisper-base-en"]
"#;

const MINIMAL: &str = r#"
config-version = 2
[server]
bind = "127.0.0.1:8081"
api_key = "k"
"#;

fn raw(fixture: &str) -> RawConfig {
    toml::from_str(fixture).expect("fixture parses")
}

#[test]
fn raw_config_round_trips_through_json() {
    let json = serde_json::to_value(raw(FULL)).expect("serializes");
    let back: RawConfig = serde_json::from_value(json.clone()).expect("deserializes");
    let again = serde_json::to_value(&back).expect("re-serializes");
    assert_eq!(json, again);
}

#[test]
fn minimal_config_round_trips_through_json() {
    let json = serde_json::to_value(raw(MINIMAL)).expect("serializes");
    let back: RawConfig = serde_json::from_value(json.clone()).expect("deserializes");
    let again = serde_json::to_value(&back).expect("re-serializes");
    assert_eq!(json, again);
}

#[test]
fn serialized_shape_uses_the_toml_key_names() {
    let json = serde_json::to_value(raw(FULL)).expect("serializes");
    let top = json.as_object().expect("a top-level table");
    for key in [
        "server",
        "local",
        "dominion",
        "endpoint",
        "model",
        "local_model",
        "stt_model",
        "profile",
        "tools",
        "workshop",
    ] {
        assert!(top.contains_key(key), "missing top-level key `{key}`");
    }
    assert_eq!(json["local_model"][0]["speculative"]["type"], "draft-mtp");
}

#[test]
fn every_secret_field_serializes_as_redacted() {
    let json = serde_json::to_value(raw(FULL)).expect("serializes");
    assert_eq!(json["server"]["api_key"], "***");
    assert_eq!(json["endpoint"][0]["api_key"], "***");
    assert_eq!(json["tools"]["web_search"]["api_key"], "***");
}

#[test]
fn serialized_output_never_contains_a_secret_value() {
    let text = serde_json::to_string(&raw(FULL)).expect("serializes");
    for secret in [
        "server-secret-value",
        "endpoint-secret-value",
        "search-secret-value",
    ] {
        assert!(
            !text.contains(secret),
            "serialized config leaked `{secret}`"
        );
    }
}

#[test]
fn enums_round_trip_with_their_toml_spellings() {
    fn check<T>(value: T, spelling: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug + Copy,
    {
        let json = serde_json::to_value(value).expect("serializes");
        assert_eq!(json, spelling);
        let back: T = serde_json::from_value(json).expect("deserializes");
        assert_eq!(value, back);
    }

    check(Protocol::Openai, "openai");
    check(DominionKind::Remote, "remote");
    check(DominionKind::Local, "local");
    check(QueuePolicy::Queue, "queue");
    check(QueuePolicy::Reject, "reject");
    check(SearchProvider::Brave, "brave");
    check(SpeculationType::DraftMtp, "draft-mtp");
    check(ThinkingMode::Never, "never");
    check(ThinkingMode::Always, "always");
    check(ThinkingMode::Switchable, "switchable");
    check(ToolDialect::Openai, "openai");
    check(ToolDialect::Gemma3ToolCode, "gemma3_tool_code");
    check(ModelKind::Chat, "chat");
    check(ModelKind::Embedding, "embedding");
    check(ModelKind::Classifier, "classifier");
    check(SttRole::Interim, "interim");
    check(SttRole::Final, "final");
}

#[test]
fn capabilities_round_trip_through_json() {
    let capabilities = Capabilities {
        max_output: Some(4096),
        default_temperature: Some(0.5),
        images: true,
        parallel_tool_calls: true,
        effort_levels: vec!["low".to_owned(), "high".to_owned()],
        default_effort: Some("low".to_owned()),
        adaptive_thinking: true,
    };
    let json = serde_json::to_value(&capabilities).expect("serializes");
    let back: Capabilities = serde_json::from_value(json).expect("deserializes");
    assert_eq!(capabilities, back);
}
