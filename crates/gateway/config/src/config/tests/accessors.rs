//! Tests for the section accessors that report operator-supplied values.

use super::super::*;

/// A minimal valid `[server]` to prefix section fixtures with.
const BASE: &str =
    "config-version = 0\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n";

fn parse(extra: &str) -> Config {
    Config::from_toml_str(&format!("{BASE}{extra}")).expect("fixture parses")
}

#[test]
fn the_server_section_reports_its_bind_address_and_bearer_key() {
    let config = parse("");
    let server = config.server();

    assert_eq!(server.bind().to_string(), "127.0.0.1:8080");
    assert_eq!(server.api_key().expose(), "secret");
}

#[test]
fn an_endpoint_reports_its_id_protocol_base_url_and_key() {
    let config = parse(
        r#"
[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = "backend-key"
"#,
    );
    let endpoint = &config.endpoints()[0];

    assert_eq!(endpoint.id(), "e");
    assert_eq!(endpoint.protocol(), Protocol::Openai);
    assert_eq!(endpoint.base_url(), "http://127.0.0.1:9");
    assert_eq!(endpoint.api_key().expose(), "backend-key");
}

#[test]
fn a_remote_model_reports_its_endpoints_and_default_max_tokens() {
    let config = parse(
        r#"
[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "m"
description = "a model"
context = 8192
upstream = "u"
endpoints = ["e"]
default_max_tokens = 1024
"#,
    );
    let model = &config.models()[0];

    assert_eq!(model.endpoints(), ["e"]);
    assert_eq!(model.default_max_tokens(), Some(1024));
}

#[test]
fn a_local_model_reports_its_description_source_and_launch_settings() {
    let config = parse(
        r#"
[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
cache_type_v = "f16"
chat_template_file = "q.jinja"
"#,
    );
    let model = &config.local_models()[0];

    assert_eq!(model.description(), "a local model");
    assert_eq!(model.source(), "/models/q.gguf");
    assert_eq!(model.cache_type_v(), "f16");
    assert_eq!(model.chat_template_file(), Some("q.jinja"));
}

#[test]
fn a_local_model_reports_its_declared_capabilities() {
    let config = parse(
        r#"
[[local_model]]
name = "q"
description = "a local model"
source = "/models/q.gguf"
context = 4096
images = true
parallel_tool_calls = true
"#,
    );
    let capabilities = config.local_models()[0].capabilities();

    assert!(capabilities.images());
    assert!(capabilities.parallel_tool_calls());
}

#[test]
fn an_stt_model_reports_its_source_and_no_pin_or_dominion_when_unset() {
    let config = parse(
        r#"
[[stt_model]]
name = "speech"
role = "interim"
source = "/speech.bin"
vram_gb = 1.0
"#,
    );
    let model = &config.catalog_stt_models()[0];

    assert_eq!(model.source(), "/speech.bin");
    assert_eq!(model.sha256(), None);
    assert_eq!(model.dominion(), None);
}

#[test]
fn web_search_reports_an_operator_supplied_base_url() {
    let config = parse(
        r#"
[tools.web_search]
provider = "brave"
api_key = "k"
base_url = "https://search.example.com/res/v1"
"#,
    );
    let search = config.web_search_config().expect("web_search present");

    assert_eq!(search.base_url(), "https://search.example.com/res/v1");
}
