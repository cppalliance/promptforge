//! Tests for model resolution and the routing table read from config.

use super::*;
use gateway_config::{Capabilities, ConfigErrorKind, Secret, ThinkingMode};

fn model_named(name: &str) -> Arc<Model> {
    let endpoint = Arc::new(Endpoint {
        id: "e".to_owned(),
        upstream: Arc::new(OpenAiUpstream::new(
            "http://127.0.0.1:9",
            Secret::new(String::new()),
        )),
        queue: DominionQueue::unlimited(),
    });
    Arc::new(Model {
        name: name.to_owned(),
        kind: ModelKind::Chat,
        description: "d".to_owned(),
        context: 8192,
        thinking: ThinkingMode::Never,
        capabilities: Capabilities::default(),
        tool_dialect: "openai".to_owned(),
        upstream_name: "u".to_owned(),
        endpoint,
    })
}

fn routing_from(toml: &str) -> Routing {
    let document = if toml.contains("config-version") {
        toml.to_owned()
    } else {
        format!("config-version = 0\n{toml}")
    };
    let config = Config::from_toml_str(&document).unwrap();
    Routing::from_config(&config).unwrap()
}

fn routing() -> Routing {
    let toml = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "known"
description = "a known test model"
context = 8192
upstream = "backend-name"
endpoints = ["e"]
"#;
    routing_from(toml)
}

#[test]
fn resolves_a_known_model() {
    let r = routing();
    let m = r.model("known").unwrap();
    assert_eq!(m.upstream_name, "backend-name");
    assert_eq!(m.endpoint.id, "e");
}

#[test]
fn unknown_model_errors() {
    let r = routing();
    assert!(matches!(
        r.model("nope"),
        Err(GatewayError::UnknownModel(_))
    ));
}

#[cfg(feature = "local")]
#[test]
fn without_drops_the_named_model_and_keeps_the_rest() {
    let r = routing_from(
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "first"
description = "the model being unloaded"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "second"
description = "the model that stays"
context = 8192
upstream = "u"
endpoints = ["e"]
"#,
    );
    let filtered = r.without("first");
    assert!(
        matches!(filtered.model("first"), Err(GatewayError::UnknownModel(_))),
        "the named model no longer routes"
    );
    assert!(
        filtered.model("second").is_ok(),
        "the other model still routes"
    );
    assert_eq!(
        filtered.models().len(),
        1,
        "the catalog listing loses exactly the named model"
    );
    // The source table is untouched: `without` copies.
    assert!(r.model("first").is_ok());
    // A name the table never held filters to an identical table.
    assert_eq!(r.without("ghost").models().len(), 2);
}

#[test]
fn from_config_reads_model_kinds() {
    let toml = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "chatty"
description = "a chat model"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "embed"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
    let routing = routing_from(toml);
    assert_eq!(routing.model("chatty").unwrap().kind, ModelKind::Chat);
    assert_eq!(routing.model("embed").unwrap().kind, ModelKind::Embedding);
}

#[test]
fn from_config_reads_the_tool_dialect() {
    let toml = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "plain"
description = "a native-tools model"
context = 8192
upstream = "u"
endpoints = ["e"]

[[model]]
name = "gemma"
description = "an emulated-tools model"
context = 8192
tool_dialect = "gemma3_tool_code"
upstream = "u"
endpoints = ["e"]
"#;
    let routing = routing_from(toml);
    let plain = routing.model("plain").unwrap();
    assert_eq!(plain.tool_dialect, "openai");
    let gemma = routing.model("gemma").unwrap();
    assert_eq!(gemma.tool_dialect, "gemma3_tool_code");
}

#[test]
fn remote_model_defaults_to_openai_dialect() {
    let toml = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "remote"
description = "a remote model"
context = 8192
upstream = "u"
endpoints = ["e"]
"#;
    let routing = routing_from(toml);
    let model = routing.model("remote").unwrap();
    assert_eq!(model.tool_dialect, "openai");
}

#[test]
fn new_rejects_duplicate_model_names() {
    let dup = Routing::new(vec![model_named("m"), model_named("m")]);
    assert!(matches!(
        dup,
        Err(e) if e.kind() == ConfigErrorKind::Validation
    ));
}

#[test]
fn new_preserves_catalog_order() {
    let r = Routing::new(vec![model_named("a"), model_named("b"), model_named("c")])
        .expect("distinct names");
    let names: Vec<&str> = r.models().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["a", "b", "c"]);
}

#[test]
fn merge_rejects_duplicate_model_names() {
    let base = Routing::new(vec![model_named("known")]).expect("distinct");
    let merged = base.merge([model_named("known")]);
    assert!(matches!(
        merged,
        Err(e) if e.kind() == ConfigErrorKind::Validation
    ));
}

#[test]
fn merge_appends_after_existing_models() {
    let base = Routing::new(vec![model_named("a")]).expect("distinct");
    let merged = base.merge([model_named("b")]).expect("distinct extra");
    let names: Vec<&str> = merged.models().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[tokio::test]
async fn endpoints_on_one_dominion_share_one_limit() {
    // The new behavior dominions introduce: two endpoints bound to one
    // dominion compete for a single pool of slots. Filling the queue
    // through one endpoint blocks the other.
    let toml = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = 1

[[endpoint]]
id = "a"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""
dominion = "pool"

[[endpoint]]
id = "b"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""
dominion = "pool"

[[model]]
name = "ma"
description = "model on endpoint a"
context = 8192
upstream = "ua"
endpoints = ["a"]

[[model]]
name = "mb"
description = "model on endpoint b"
context = 8192
upstream = "ub"
endpoints = ["b"]
"#;
    let routing = routing_from(toml);
    let queue_a = routing.model("ma").unwrap().endpoint.queue.clone();
    let queue_b = routing.model("mb").unwrap().endpoint.queue.clone();

    // Fill the dominion's only slot through endpoint A.
    let held = queue_a.admit("client").await.unwrap();

    // Endpoint B's admit cannot proceed: it parks as a waiter on the SAME
    // shared queue instead of getting a slot of its own.
    let queue_b_spawn = queue_b.clone();
    let blocked = tokio::spawn(async move { queue_b_spawn.admit("client").await });
    while queue_a.waiter_count() != 1 {
        tokio::task::yield_now().await;
    }

    // Releasing A's permit hands the shared slot to B's waiter.
    drop(held);
    let _permit = blocked.await.unwrap().unwrap();
}

#[tokio::test]
async fn dominion_without_max_concurrency_is_unlimited() {
    // Absent max_concurrency means unlimited: admits never wait, so a
    // bound max_queue and reject policy have no full in-flight set to
    // act on.
    let toml = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[dominion]]
id = "pool"
kind = "remote"
max_queue = 1
policy = "reject"

[[endpoint]]
id = "a"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""
dominion = "pool"

[[model]]
name = "ma"
description = "model on endpoint a"
context = 8192
upstream = "ua"
endpoints = ["a"]
"#;
    let routing = routing_from(toml);
    let queue = routing.model("ma").unwrap().endpoint.queue.clone();
    let _first = queue.admit("client").await.unwrap();
    let _second = queue.admit("client").await.unwrap();
    let _third = queue.admit("client").await.unwrap();
}
