//! Tests for gateway binding changes rebuilding the environment's registry
//! and client, and for the first-party registry holding user input on
//! every gateway.

use std::path::Path;

use harness_capabilities::{CapabilityId, InputBroker, InputError};
use harness_log::{RunLog, RunOutcome};
use harness_runner::effect_loop::{SharedLog, drive_run};
use harness_runner::performers::{BoxFuture, ChatPerformer};
use harness_runner::prepare::{Services, prepare_source};
use promptforge::cancel::CancelHandle;
use promptforge::model::{Completion, CompletionOptions, Message, ModelBinding, ToolSchema};
use promptforge::vfs::VfsRef;

use super::*;

fn binding(generation: u64) -> GatewayBinding {
    GatewayBinding {
        base_url: format!("http://127.0.0.1:{}", 8000 + generation),
        key: format!("key-{generation}"),
        generation,
    }
}

#[test]
fn a_generation_change_rebuilds_the_registry_and_client() {
    let bindings = Bindings::new();
    assert!(bindings.set_gateway(binding(1)), "the first push builds");
    let first = bindings.gateway().expect("resources exist after a push");
    assert_eq!(first.generation(), 1);
    assert!(
        first.registry().get(&web_id()).is_some(),
        "a valid binding builds web into the registry"
    );
    assert!(
        first.client().is_some(),
        "a valid binding builds the client"
    );

    assert!(
        bindings.set_gateway(binding(2)),
        "a new generation rebuilds"
    );
    let second = bindings.gateway().expect("resources exist after a rebuild");
    assert_eq!(second.generation(), 2);
    assert_eq!(second.binding().base_url, "http://127.0.0.1:8002");
    assert!(
        !Arc::ptr_eq(first.registry(), second.registry()),
        "the registry is a fresh build, not the first generation's"
    );
    assert_eq!(
        *bindings.subscribe_gateway().borrow(),
        Some(2),
        "the watch holds the rebuilt generation"
    );
}

#[test]
fn a_repeated_generation_keeps_the_built_resources() {
    let bindings = Bindings::new();
    assert!(bindings.set_gateway(binding(3)));
    let built = bindings.gateway().expect("resources exist");
    assert!(
        !bindings.set_gateway(GatewayBinding {
            base_url: "http://127.0.0.1:9999".to_owned(),
            ..binding(3)
        }),
        "the same generation is the client's word that nothing changed"
    );
    let kept = bindings.gateway().expect("resources still exist");
    assert!(
        Arc::ptr_eq(&built, &kept),
        "no rebuild happened for a repeated generation"
    );
}

/// A binding whose root is not a URL and whose key is empty.
fn unusable_binding() -> GatewayBinding {
    GatewayBinding {
        base_url: "not a url".to_owned(),
        key: String::new(),
        generation: 1,
    }
}

fn user_input_id() -> CapabilityId {
    CapabilityId::parse("promptforge/user-input").expect("a valid capability id")
}

fn web_id() -> CapabilityId {
    CapabilityId::parse("promptforge/web").expect("a valid capability id")
}

#[test]
fn an_unusable_binding_keeps_a_registry_but_builds_no_client() {
    let resources = GatewayResources::build(unusable_binding());
    let registry = resources.registry();
    assert!(
        registry.get(&user_input_id()).is_some(),
        "user input needs no gateway"
    );
    assert!(
        registry.get(&web_id()).is_none(),
        "no web capability from a bad root"
    );
    assert!(resources.client().is_none(), "no client from an empty key");
}

#[test]
fn the_registry_holds_user_input_whether_or_not_the_gateway_can_build_web() {
    let usable = first_party_registry("http://127.0.0.1:8000/v1", "key");
    assert!(usable.get(&user_input_id()).is_some());
    assert!(usable.get(&web_id()).is_some());

    for (root, token) in [("not a url", "key"), ("http://127.0.0.1:8000/v1", "")] {
        let registry = first_party_registry(root, token);
        assert!(
            registry.get(&user_input_id()).is_some(),
            "user input is registered for root {root:?} and token {token:?}"
        );
        assert!(
            registry.get(&web_id()).is_none(),
            "web is not registered for root {root:?} and token {token:?}"
        );
    }
}

/// A prompt that needs only user input and returns the operator's answer.
const ASKS: &str = "---\nname: asks\ndescription: d\npromptforge: 0\n\
    capabilities:\n  - promptforge/user-input\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn (input.ask())\n```\n";

/// A broker whose operator always types the same text.
struct Typed(&'static str);

#[async_trait::async_trait]
impl InputBroker for Typed {
    async fn wait(&self) -> Result<String, InputError> {
        Ok(self.0.to_owned())
    }
}

/// A chat performer for a run that makes no model round.
struct NoChat;

impl ChatPerformer for NoChat {
    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _stream: bool,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        unreachable!("the prompt makes no model round")
    }
}

#[tokio::test]
async fn a_prompt_that_needs_only_user_input_prepares_and_runs_on_an_unusable_gateway() {
    let resources = GatewayResources::build(unusable_binding());
    let log: SharedLog = Arc::new(tokio::sync::Mutex::new(
        RunLog::in_memory().await.expect("the log opens"),
    ));
    let services = Services {
        registry: Some(Arc::clone(resources.registry())),
        vfs: VfsRef::default(),
        cancel: CancelHandle::new(),
        log: Arc::clone(&log),
        chat: Arc::new(NoChat),
        input: Some(Arc::new(Typed("hello"))),
        session_id: "session-1".to_owned(),
        agent: "asks".to_owned(),
        model: None,
        ui: None,
    };
    let prepared = prepare_source(ASKS, Path::new("asks.md"), "", services)
        .await
        .expect("the prompt is not refused");
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        log,
        prepared.run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .expect("the loop reaches an outcome");
    assert_eq!(
        outcome,
        RunOutcome::Completed {
            final_text: "hello".to_owned()
        }
    );
}

#[test]
fn a_gateway_binding_never_prints_its_key() {
    let rendered = format!("{:?}", binding(7));
    assert!(
        !rendered.contains("key-7"),
        "the bearer key leaked into Debug output: {rendered}"
    );
    assert!(rendered.contains("generation: 7"));
}

#[test]
fn the_host_snapshot_serves_the_first_root_and_the_selection() {
    let host = HostSnapshot {
        selected_model: Some("gpt".to_owned()),
        workspace_roots: vec![PathBuf::from("/w/one"), PathBuf::from("/w/two")],
    };
    let ui = host.ui();
    assert_eq!(ui["selected_model"], "gpt");
    assert_eq!(
        ui["workspace_root"],
        PathBuf::from("/w/one").display().to_string()
    );
    let empty = HostSnapshot::default().ui();
    assert!(empty["selected_model"].is_null());
    assert!(empty["workspace_root"].is_null());
}

#[tokio::test]
async fn no_selection_and_no_catalog_binds_no_model_without_a_fetch() {
    // No selection and an empty catalog: nothing to resolve, so nothing
    // is fetched from the (unreachable) gateway and the roles stay
    // unbound.
    let model = current_model(
        &HostSnapshot::default(),
        Some(&CatalogBinding::default()),
        &binding(1),
    )
    .await
    .expect("no fetch is attempted");
    assert!(model.is_none());
}
