use std::num::NonZeroU32;
use std::sync::atomic::AtomicU64;

use shared_promptforge_api::events::RuntimeEventKind;
use shared_promptforge_api::models::{ModelDescriptor, ModelId, ThinkingMode};
use shared_promptforge_api::observe::{Observation, Observer};
use workshop_protocol::Activity;

use super::*;

/// A push facade wired to the real buses through the registry, with
/// the registrations kept alive by the returned guards.
fn wired_push(
    status: &workshop_status::StatusBus,
    catalog: &CatalogBus,
    menu: &MenuBus,
) -> (Push, impl std::fmt::Debug + Send + Sync + 'static) {
    let registry = Registry::new();
    let status_guards = workshop_status::register(&registry, status);
    let menu_guards = workshop_menu::register(&registry, catalog, menu);
    (registry.push(), (status_guards, menu_guards))
}

#[test]
fn discovery_lists_sorted_markdown_stems_and_tolerates_a_missing_dir() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("zeta.md"), "# zeta").expect("seed zeta");
    std::fs::write(dir.path().join("alpha.md"), "# alpha").expect("seed alpha");
    std::fs::write(dir.path().join("notes.txt"), "not an agent").expect("seed noise");
    std::fs::write(dir.path().join("legacy.lua"), "return 1").expect("seed a retired Lua program");
    std::fs::create_dir(dir.path().join("nested.md")).expect("seed a decoy directory");
    assert_eq!(
        discover_agents(dir.path()),
        vec!["alpha".to_owned(), "chat".to_owned(), "zeta".to_owned()],
        "discovery lists .md file stems plus the built-in chat, sorted, \
         and skips everything else - a .lua file is never an agent"
    );
    assert_eq!(
        discover_agents(&dir.path().join("missing")),
        vec!["chat".to_owned()],
        "a missing agents directory still offers the built-in chat rather than failing"
    );
}

#[test]
fn the_built_in_chat_is_always_offered_and_a_dir_file_shadows_its_source() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    assert_eq!(
        discover_agents(dir.path()),
        vec!["chat".to_owned()],
        "an empty agents directory still offers the built-in chat"
    );
    assert_eq!(
        agent_source(dir.path(), "chat").expect("the built-in serves"),
        AgentSource::Markdown(BUILTIN_CHAT_SOURCE.to_owned()),
        "with no directory file, the embedded source is what launches"
    );

    std::fs::write(dir.path().join("chat.md"), "# shadowed").expect("seed the shadow");
    assert_eq!(
        discover_agents(dir.path()),
        vec!["chat".to_owned()],
        "a directory chat.md lists once, never beside the built-in"
    );
    assert_eq!(
        agent_source(dir.path(), "chat").expect("the shadow reads"),
        AgentSource::Markdown("# shadowed".to_owned()),
        "a directory chat.md shadows the embedded source"
    );

    std::fs::remove_file(dir.path().join("chat.md")).expect("clear the shadow");
    std::fs::write(dir.path().join("chat.lua"), "-- retired").expect("seed a retired shadow");
    assert_eq!(
        agent_source(dir.path(), "chat").expect("the built-in still serves"),
        AgentSource::Markdown(BUILTIN_CHAT_SOURCE.to_owned()),
        "a directory chat.lua shadows nothing: the Lua path is retired"
    );

    assert_eq!(
        agent_source(dir.path(), "ghost")
            .expect_err("only the built-in name falls back to embedded source")
            .kind(),
        io::ErrorKind::NotFound,
        "a non-built-in name surfaces its filesystem error"
    );
}

#[test]
fn an_unreadable_chat_md_surfaces_its_error_rather_than_the_built_in() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A directory named chat.md cannot be read as a file on any
    // platform, and its failure is never NotFound - the one kind
    // that falls back to the embedded source.
    std::fs::create_dir(dir.path().join("chat.md")).expect("seed the unreadable shadow");
    agent_source(dir.path(), "chat").expect_err(
        "an existing chat.md that cannot be read surfaces its error; \
         silently serving the built-in would mask the operator's own file",
    );
}

#[test]
fn reply_stamps_follow_the_settle_rule() {
    let mut rounds = 0;
    assert_eq!(
        reply_stamp(RuntimeEventKind::UserInput, &mut rounds),
        None,
        "input events settle nothing"
    );
    assert_eq!(
        reply_stamp(RuntimeEventKind::Thinking, &mut rounds),
        Some(0),
        "thinking carries the open round without settling it"
    );
    assert_eq!(
        reply_stamp(RuntimeEventKind::AssistantReply, &mut rounds),
        Some(0)
    );
    assert_eq!(
        reply_stamp(RuntimeEventKind::AssistantToolCalls, &mut rounds),
        Some(1),
        "a tool-call batch settles its round exactly as a reply does"
    );
    assert_eq!(reply_stamp(RuntimeEventKind::ToolResult, &mut rounds), None);
    assert_eq!(
        reply_stamp(RuntimeEventKind::Thinking, &mut rounds),
        Some(2),
        "the next round opens where the last one settled"
    );
}

#[test]
fn the_ui_snapshot_serves_the_selection_and_first_granted_root() {
    let catalog = CatalogBus::default();
    let menu = MenuBus::new(catalog.clone(), None);
    let registry = Registry::new();
    let ui = ui_provider(&menu, &registry);
    assert_eq!(
        ui(),
        serde_json::json!({ "selected_model": null, "workspace_root": null }),
        "absent producers serve null, never a missing key"
    );

    catalog.publish(vec![serde_json::json!({ "id": "test-model" })]);
    menu.set_selected("test-model")
        .expect("the id is in the catalog");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let granted = dir.path().to_path_buf();
    let _roots = registry.register_state::<dyn workshop_registry::WorkspaceRoots>(Arc::new(
        workshop_registry::WorkspaceRootsAdapter::new({
            let granted = granted.clone();
            move || vec![granted.clone()]
        }),
    ));
    let snapshot = ui();
    assert_eq!(snapshot["selected_model"], "test-model");
    assert_eq!(
        snapshot["workspace_root"],
        serde_json::json!(granted.display().to_string()),
        "workspace_root is the first granted root, read through the registry slot"
    );
}

#[test]
fn a_launch_without_a_usable_client_is_refused() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("echo.md"), "# echo").expect("seed echo");
    let catalog = CatalogBus::default();
    let menu = MenuBus::new(catalog.clone(), None);
    let registry = Registry::new();
    let sessions = AgentSessions::new(
        dir.path().to_path_buf(),
        dir.path().join("sessions"),
        GatewayBinding::new("http://127.0.0.1:1", "")
            .expect("the unusable model binding still builds its HTTP client"),
        SessionHost::new(registry, ReconnectBackoff::new(), menu, catalog),
    );
    // A plain #[test] doubles as ordering proof: the refusal returns
    // before anything is spawned, or this panics outside a runtime.
    let refusal = sessions
        .launch("echo")
        .expect_err("a discovered agent must still refuse without a model client");
    assert!(
        matches!(refusal, LaunchRefusal::GatewayUnusable),
        "the refusal names the gateway configuration, not the agent: {refusal}"
    );
    assert!(
        sessions.lock().is_empty(),
        "a refused launch registers no session"
    );
}

#[tokio::test]
async fn a_failed_model_turn_pushes_a_terminal_failure_status() {
    let status = workshop_status::StatusBus::new();
    let mut status_rx = status.subscribe();
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let (push, _guards) = wired_push(&status, &catalog, &menu);
    let (errors, mut errors_rx) = broadcast::channel(ERROR_CAPACITY);
    let (supervisor_events, _events) = mpsc::unbounded_channel();
    let (cancellations, _cancellation_events) = mpsc::channel(lifecycle::CANCELLATION_CAPACITY);
    let observer = SessionObserver {
        log: Arc::new(WorkshopObserver::new(None).expect("a memory log")),
        rounds: Arc::new(AtomicU64::new(0)),
        push,
        backoff: ReconnectBackoff::new(),
        errors,
        lifecycle: Arc::new(RunLifecycle::new(supervisor_events, cancellations)),
    };

    observer.observe("run", "chat", Observation::ModelTurnFailed);

    let update = status_rx
        .recv()
        .await
        .expect("the failed round pushes a terminal status");
    assert_eq!(update.severity, workshop_protocol::Severity::Error);
    assert_eq!(
        update.activity,
        Activity::General,
        "a non-thinking activity releases the status bar's sustained amber LED"
    );
    assert_eq!(
        errors_rx.recv().await.expect("the error frame is sent"),
        "Model turn failed in agent `chat`"
    );
}

#[tokio::test]
async fn a_failed_tool_call_pushes_a_terminal_failure_status() {
    // A tool dispatch failure aborts the model loop the same way a failed
    // model round does, and the built-in chat's pcall swallows both; without
    // this frame the operator sees a tool call that never returns and a
    // status bar stuck busy.
    let status = workshop_status::StatusBus::new();
    let mut status_rx = status.subscribe();
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let (push, _guards) = wired_push(&status, &catalog, &menu);
    let (errors, mut errors_rx) = broadcast::channel(ERROR_CAPACITY);
    let (supervisor_events, _events) = mpsc::unbounded_channel();
    let (cancellations, _cancellation_events) = mpsc::channel(lifecycle::CANCELLATION_CAPACITY);
    let observer = SessionObserver {
        log: Arc::new(WorkshopObserver::new(None).expect("a memory log")),
        rounds: Arc::new(AtomicU64::new(0)),
        push,
        backoff: ReconnectBackoff::new(),
        errors,
        lifecycle: Arc::new(RunLifecycle::new(supervisor_events, cancellations)),
    };

    observer.observe("run", "chat", Observation::ToolCallFailed);

    let update = status_rx
        .recv()
        .await
        .expect("the failed dispatch pushes a terminal status");
    assert_eq!(update.severity, workshop_protocol::Severity::Error);
    assert_eq!(
        update.activity,
        Activity::General,
        "a non-thinking activity releases the status bar's sustained amber LED"
    );
    assert_eq!(
        errors_rx.recv().await.expect("the error frame is sent"),
        "Tool call failed in agent `chat`"
    );
}

#[test]
fn the_model_client_requires_a_usable_key_and_url() {
    assert!(
        agent_client("http://127.0.0.1:8081", "k").is_some(),
        "a keyed gateway builds the agent model client"
    );
    assert!(
        agent_client("http://127.0.0.1:8081", "").is_none(),
        "an empty key cannot authenticate: agents report it at launch"
    );
    assert!(agent_client("not a url", "k").is_none());
}

/// The descriptor the chat unit runs bind the declared `chat` role to; its
/// window clears the role's declared minimum.
fn test_model() -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("test-model").expect("the test model id is valid"),
        "test model",
        NonZeroU32::new(200_000).expect("200000 is non-zero"),
        ThinkingMode::Never,
    )
}

/// Runs the embedded chat prompt on the unified runtime with the given
/// broker configuration, against a client no model call can survive. The
/// environment carries the first-party capabilities exactly as the
/// session wiring builds them, and the context carries the current model,
/// because the prompt now declares its contract in frontmatter.
async fn run_builtin_chat(
    broker: Option<Arc<dyn promptforge_api::input::InputBroker>>,
) -> Result<String, promptforge_api::execute::RunError> {
    use promptforge_api::{Prompt, RunContext, RunResult};
    let observer: Arc<dyn Observer> = Arc::new(WorkshopObserver::new(None).expect("memory log"));
    let prompt = Prompt::parse(BUILTIN_CHAT_SOURCE, "chat-unit", observer.as_ref())
        .expect("the embedded chat prompt parses");
    let env = session_environment("http://127.0.0.1:9", "k")
        .expect("a well-shaped gateway root builds the session environment");
    let mut ctx = RunContext::new("chat-unit")
        .observer(observer)
        .model(test_model());
    if let Some(broker) = broker {
        ctx = ctx.input_broker(broker);
    }
    match env.run(&prompt, "", ctx).await {
        RunResult::Ok(text) => Ok(text),
        RunResult::Cancelled => panic!("the chat unit run is never cancelled"),
        RunResult::Failure(error) => Err(error),
    }
}

#[test]
fn the_builtin_chat_declares_its_contract_in_frontmatter() {
    let prompt = promptforge_api::Prompt::parse(
        BUILTIN_CHAT_SOURCE,
        "chat-unit",
        &shared_promptforge_api::observe::NullObserver::default(),
    )
    .expect("the embedded chat prompt parses");
    let frontmatter = prompt.frontmatter();
    let capabilities = frontmatter.capabilities();
    assert_eq!(
        capabilities.len(),
        1,
        "chat declares exactly one capability"
    );
    assert_eq!(capabilities[0].id().to_string(), "promptforge/web");
    assert!(
        !capabilities[0].is_optional(),
        "the built-in host always installs its own web capability"
    );
    let tools = frontmatter.tools();
    assert_eq!(tools.len(), 2, "both web tools get exact slots");
    assert!(tools.get("fetch").is_some(), "the fetch slot is declared");
    assert!(tools.get("search").is_some(), "the search slot is declared");
    let chat = frontmatter
        .models()
        .get("chat")
        .expect("the chat role is declared for the host's current model");
    assert_eq!(
        chat.min_context(),
        NonZeroU32::new(32768),
        "the role declares the window its web tools need, so an undersized \
         binding is refused at prepare instead of failing mid-conversation"
    );
}

#[tokio::test]
async fn an_undersized_model_is_refused_before_the_first_turn() {
    // The scenario this pins: a launch that bound a small descriptor (the
    // catalog fetch's fallback window) used to run, then fail the first
    // conversation that outgrew it. The declared minimum turns that into a
    // refusal at prepare naming the role.
    use promptforge_api::execute::RunErrorKind;
    use promptforge_api::{Prompt, RunContext, RunResult};
    let observer: Arc<dyn Observer> = Arc::new(WorkshopObserver::new(None).expect("memory log"));
    let prompt = Prompt::parse(BUILTIN_CHAT_SOURCE, "chat-unit", observer.as_ref())
        .expect("the embedded chat prompt parses");
    let env = session_environment("http://127.0.0.1:9", "k")
        .expect("a well-shaped gateway root builds the session environment");
    let small = ModelDescriptor::new(
        ModelId::gateway("small-model").expect("the test model id is valid"),
        "small model",
        NonZeroU32::new(8192).expect("8192 is non-zero"),
        ThinkingMode::Never,
    );
    let ctx = RunContext::new("chat-unit").observer(observer).model(small);

    let RunResult::Failure(error) = env.run(&prompt, "", ctx).await else {
        panic!("an 8192-token model cannot satisfy the chat role");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("chat") && notice.contains("32768") && notice.contains("8192"),
        "the notice names the role, the minimum, and the actual window: {notice}"
    );
}

#[tokio::test]
async fn the_builtin_chat_returns_without_a_broker_beneath_it() {
    // No broker is the unavailable-fallback policy: user_input()
    // resumes unavailable, the prompt returns, and no model call is
    // ever attempted (the run carries no client at all).
    let result = run_builtin_chat(None).await;
    assert!(
        result.is_ok(),
        "the unavailable fallback ends the run cleanly: {result:?}"
    );
}

#[tokio::test]
async fn a_failing_broker_fails_the_builtin_chat_as_typed_input() {
    struct FailingBroker;

    #[async_trait::async_trait]
    impl promptforge_api::input::InputBroker for FailingBroker {
        async fn user_input(
            &self,
            _execution: &str,
            _section: &str,
        ) -> Result<promptforge_api::input::InputOutcome, promptforge_api::input::InputError>
        {
            Err(promptforge_api::input::InputError::message(
                "the input device is gone",
            ))
        }
    }

    let error = run_builtin_chat(Some(Arc::new(FailingBroker)))
        .await
        .expect_err("the broker failure fails the run");
    assert!(
        matches!(error.kind(), promptforge_api::execute::RunErrorKind::Input),
        "a broker failure is the typed input failure: {error}"
    );
}
