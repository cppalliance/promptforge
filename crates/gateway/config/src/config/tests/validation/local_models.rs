//! Tests for `[[local_model]]` parsing and validation.

use super::*;

#[test]
fn rejects_remote_local_model_without_digest() {
    // ART-002: a remote (https) local_model source must be pinned by sha256.
    let toml = r#"
config-version = 0
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
fn parses_local_model_with_defaults() {
    let toml = r#"
config-version = 0
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
config-version = 0
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
fn parses_local_backend_selection_and_server_path() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
llama_backend = "cuda-blackwell"
llama_server_path = "/opt/llama/llama-server.exe"
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.local.llama_backend(), LlamaBackend::CudaBlackwell);
    assert_eq!(
        config.local.llama_server_path(),
        Some("/opt/llama/llama-server.exe")
    );
}

#[test]
fn local_backend_defaults_to_auto_with_no_path_override() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"
"#;
    let config = Config::from_toml_str(toml).unwrap();
    assert_eq!(config.local.llama_backend(), LlamaBackend::Auto);
    assert!(config.local.llama_server_path().is_none());
}

#[test]
fn rejects_an_unknown_local_backend() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[local]
llama_backend = "tensorrt"
"#;
    assert!(Config::from_toml_str(toml).is_err());
}

#[test]
fn parses_each_whisper_backend() {
    for (spelling, backend) in [
        ("auto", WhisperBackend::Auto),
        ("cpu", WhisperBackend::Cpu),
        ("cuda", WhisperBackend::Cuda),
    ] {
        let toml = format!(
            "config-version = 0\n[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"t\"\n\
             [stt]\nwhisper_backend = \"{spelling}\"\n"
        );
        let config = Config::from_toml_str(&toml).unwrap();
        assert_eq!(
            config.stt().map(SttPipelineConfig::whisper_backend),
            Some(backend),
            "whisper_backend = \"{spelling}\""
        );
    }
}

#[test]
fn rejects_an_unknown_whisper_backend_naming_the_accepted_values() {
    let toml = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[stt]
whisper_backend = "vulkan"
"#;
    let error = Config::from_toml_str(toml).unwrap_err();
    assert_eq!(error.kind(), crate::ConfigErrorKind::Parse);
    let mut chain = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(inner) = source {
        chain.push_str(": ");
        chain.push_str(&inner.to_string());
        source = inner.source();
    }
    for name in ["`vulkan`", "`auto`", "`cpu`", "`cuda`"] {
        assert!(chain.contains(name), "the error names {name}: {chain}");
    }
}

#[test]
fn rejects_duplicate_name_across_remote_and_local() {
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
config-version = 0
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
config-version = 0
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
fn rejects_zero_local_model_parallel() {
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
parallel = 0
"#;
    assert!(matches!(
        Config::parse_toml(toml),
        Err(ConfigError::Validation(_))
    ));
}
