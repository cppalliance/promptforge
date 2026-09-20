use gateway_config::Config;

use crate::test_support::serve;

const CONFIG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "template-key"
# Strict bearer auth: the tests below pin that a missing key is refused.
trust_loopback = false

[[local_model]]
name = "mapped-auto"
kind = "chat"
description = "mapped model"
source = "https://huggingface.co/qwen/qwen3-8b/resolve/main/model.gguf"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
context = 4096

[[local_model]]
name = "known-broken"
kind = "chat"
description = "known override"
source = "https://huggingface.co/unsloth/gemma-4-e2b-it-GGUF/resolve/main/model.gguf"
sha256 = "1111111111111111111111111111111111111111111111111111111111111111"
context = 4096

[[local_model]]
name = "builtin"
kind = "chat"
description = "built in"
source = "models/builtin.gguf"
context = 4096
chat_template_file = "builtin:phi-4"

[[local_model]]
name = "custom"
kind = "chat"
description = "custom path"
source = "models/custom.gguf"
context = 4096
chat_template_file = "templates/custom.jinja"
"#;

#[tokio::test]
async fn catalog_requires_the_gateway_bearer() {
    let config = Config::from_toml_str(CONFIG).expect("config parses");
    let addr = serve(config).await;

    let response = reqwest::Client::new()
        .get(format!("http://{addr}/admin/chat-templates"))
        .send()
        .await
        .expect("request sends");

    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn catalog_serializes_labels_mappings_and_effective_resolutions() {
    let config = Config::from_toml_str(CONFIG).expect("config parses");
    let addr = serve(config).await;

    let response = reqwest::Client::new()
        .get(format!("http://{addr}/admin/chat-templates"))
        .bearer_auth("template-key")
        .send()
        .await
        .expect("request sends");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("body is JSON");
    assert_eq!(body["families"][0]["slug"], "chatml");
    assert_eq!(body["families"][0]["label"], "ChatML");
    assert!(
        body["mappings"]
            .as_array()
            .expect("mappings array")
            .iter()
            .any(|mapping| {
                mapping["model_id"] == "qwen/qwen3-8b" && mapping["family"] == "qwen-3"
            })
    );
    let models = body["models"].as_array().expect("models array");
    let named = |name: &str| {
        models
            .iter()
            .find(|model| model["name"] == name)
            .expect("named model")
    };
    assert_eq!(named("mapped-auto")["effective_source"], "embedded");
    assert_eq!(named("mapped-auto")["detected_family"], "qwen-3");
    assert_eq!(named("known-broken")["effective_source"], "known-override");
    assert!(
        named("known-broken")["reason"]
            .as_str()
            .expect("reason")
            .contains("Known-broken")
    );
    assert_eq!(named("builtin")["effective_family"], "phi-4");
    assert_eq!(named("custom")["effective_source"], "custom");
}
