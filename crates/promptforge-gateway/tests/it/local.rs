//! Live local-inference smoke test (ignored by default).

use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::model::CompletionOptions;
use promptforge_gateway::{Config, Gateway, ProfilesContext};

use crate::support::{SCENARIO_MODEL_SHA256, SCENARIO_MODEL_URL, TestServer, catalog_ids};

/// Live local-inference smoke test. Downloads the pinned llama-server binary and
/// the tiny Qwen3-0.6B GGUF, starts a gateway with `[[local_model]]`, and checks
/// that chat completions return text. Ignored by default so CI stays fast.
#[tokio::test]
#[ignore = "downloads llama-server + Qwen3-0.6B; set PROMPTFORGE_LIVE_LOCAL=1 to opt in"]
async fn local_model_chat_completion_returns_text() {
    if std::env::var_os("PROMPTFORGE_LIVE_LOCAL").is_none() {
        eprintln!("skipping: set PROMPTFORGE_LIVE_LOCAL=1 to run this test");
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
key = "test-token"

[local]
cache_dir = "{cache}"

[[local_model]]
name = "qwen-tiny"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "{source}"
sha256 = "{sha}"
context = 4096
thinking = "never"
gpu_layers = 0
flash_attention = false
n_predict = 64
"#,
        cache = cache.path().display().to_string().replace('\\', "/"),
        source = SCENARIO_MODEL_URL,
        sha = SCENARIO_MODEL_SHA256,
    );

    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = tokio::task::spawn_blocking(move || {
        Gateway::from_config(&config, ProfilesContext::default())
    })
    .await
    .unwrap()
    .expect("assemble local gateway");
    let server = TestServer::start(gateway).await;

    let ids = catalog_ids(&reqwest::Client::new(), server.addr).await;
    assert!(ids.iter().any(|id| id == "qwen-tiny"));

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{}/v1", server.addr)).expect("valid test endpoint"),
        SecretString::new("test-token").expect("non-empty test key"),
    );
    let options = CompletionOptions::new(
        "qwen-tiny",
        promptforge_core::dialects::ToolDialectId::OpenAi,
    );
    let result = client
        .complete(
            &[promptforge_core::client::Message::user(
                "Reply with exactly the word pong and nothing else.",
            )],
            None,
            &options,
        )
        .await
        .unwrap();
    match result.result() {
        promptforge_core::client::CompletionResult::Text(text) => {
            assert!(!text.trim().is_empty(), "local model returned empty text");
        }
        other => panic!("expected text reply, got {other:?}"),
    }
}
