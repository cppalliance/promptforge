use std::sync::atomic::AtomicU64;

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::{ChainId, Provenance, TaskId};
use workshop_protocol::Activity;

use super::*;

/// A push facade wired to the real buses through the registry, with
/// the registrations kept alive by the returned guards.
fn wired_push(
    status: &workshop_status::StatusBus,
    catalog: &CatalogBus,
    menu: &MenuBus,
) -> (Push, impl std::fmt::Debug + Send + Sync + 'static + use<>) {
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

/// A content event of one shape under the fixed test coordinates: the
/// stamp rule reads the variant alone.
fn content_event(shape: &str) -> Event {
    let execution = "run".to_owned();
    let section = "chat".to_owned();
    let provenance = Provenance {
        task: TaskId::from(ChainId::root()),
        seq: 0,
    };
    match shape {
        "input" => Event::UserInput {
            execution,
            section,
            provenance,
            text: "hi".to_owned(),
        },
        "thinking" => Event::Thinking {
            execution,
            section,
            provenance,
            turn: 1,
            model: "m".to_owned(),
            text: "hmm".to_owned(),
        },
        "reply" => Event::AssistantReply {
            execution,
            section,
            provenance,
            turn: 1,
            text: "hello".to_owned(),
            finish_reason: None,
            model: "m".to_owned(),
            metrics: None,
        },
        "tool_calls" => Event::AssistantToolCalls {
            execution,
            section,
            provenance,
            turn: 1,
            model: "m".to_owned(),
            calls: Vec::new(),
        },
        "tool_result" => Event::ToolResult {
            execution,
            section,
            provenance,
            turn: 1,
            tool_call_id: "call_1".to_owned(),
            alias: "read".to_owned(),
            content: "done".to_owned(),
            trusted: false,
        },
        other => panic!("no fixture shape named {other}"),
    }
}

#[test]
fn reply_stamps_follow_the_settle_rule() {
    let mut rounds = 0;
    assert_eq!(
        reply_stamp(&content_event("input"), &mut rounds),
        None,
        "input events settle nothing"
    );
    assert_eq!(
        reply_stamp(&content_event("thinking"), &mut rounds),
        Some(0),
        "thinking carries the open round without settling it"
    );
    assert_eq!(reply_stamp(&content_event("reply"), &mut rounds), Some(0));
    assert_eq!(
        reply_stamp(&content_event("tool_calls"), &mut rounds),
        Some(1),
        "a tool-call batch settles its round exactly as a reply does"
    );
    assert_eq!(
        reply_stamp(&content_event("tool_result"), &mut rounds),
        None
    );
    assert_eq!(
        reply_stamp(&content_event("thinking"), &mut rounds),
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

/// A sink over fresh session plumbing, wired to the real status bus, with
/// the receivers a test reads the side effects from.
fn sink_fixture() -> (
    SessionSink,
    broadcast::Receiver<workshop_protocol::StatusBarUpdate>,
    broadcast::Receiver<String>,
    impl std::fmt::Debug + Send + Sync + 'static + use<>,
) {
    let status = workshop_status::StatusBus::new();
    let status_rx = status.subscribe();
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let (push, guards) = wired_push(&status, &catalog, &menu);
    let (errors, errors_rx) = broadcast::channel(ERROR_CAPACITY);
    let (supervisor_events, _events) = mpsc::unbounded_channel();
    let (cancellations, _cancellation_events) = mpsc::channel(lifecycle::CANCELLATION_CAPACITY);
    let sink = SessionSink {
        log: Arc::new(WorkshopObserver::new()),
        rounds: Arc::new(AtomicU64::new(0)),
        push,
        backoff: ReconnectBackoff::new(),
        errors,
        lifecycle: Arc::new(RunLifecycle::new(supervisor_events, cancellations)),
    };
    (sink, status_rx, errors_rx, guards)
}

/// A lifecycle event of one shape under the fixed test coordinates.
fn lifecycle_event(shape: &str) -> Event {
    let execution = "run".to_owned();
    let section = "chat".to_owned();
    let provenance = Provenance {
        task: TaskId::from(ChainId::root()),
        seq: 0,
    };
    match shape {
        "model_turn_failed" => Event::ModelTurnFailed {
            execution,
            section,
            provenance,
        },
        "tool_call_failed" => Event::ToolCallFailed {
            execution,
            section,
            provenance,
        },
        "section_started" => Event::SectionStarted {
            execution,
            section,
            provenance,
        },
        other => panic!("no fixture shape named {other}"),
    }
}

#[tokio::test]
async fn a_failed_model_turn_pushes_a_terminal_failure_status() {
    let (sink, mut status_rx, mut errors_rx, _guards) = sink_fixture();

    sink.observe(lifecycle_event("model_turn_failed"));

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
    assert!(
        sink.log.is_empty(),
        "a lifecycle event is a side effect, never a transcript entry"
    );
}

#[tokio::test]
async fn a_failed_tool_call_pushes_a_terminal_failure_status() {
    // A tool dispatch failure aborts the model loop the same way a failed
    // model round does, and the built-in chat's pcall swallows both; without
    // this frame the operator sees a tool call that never returns and a
    // status bar stuck busy.
    let (sink, mut status_rx, mut errors_rx, _guards) = sink_fixture();

    sink.observe(lifecycle_event("tool_call_failed"));

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

#[tokio::test]
async fn the_sink_logs_transcript_events_and_settles_rounds_on_replies() {
    let (sink, _status_rx, _errors_rx, _guards) = sink_fixture();
    let mut entries = sink.log.subscribe();

    sink.observe(lifecycle_event("section_started"));
    sink.observe(content_event("input"));
    sink.observe(content_event("thinking"));
    assert_eq!(
        sink.rounds.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "input and thinking leave the round open"
    );
    sink.observe(content_event("tool_calls"));
    assert_eq!(
        sink.rounds.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a tool-call batch settles its round"
    );
    sink.observe(content_event("reply"));
    assert_eq!(
        sink.rounds.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "a reply settles its round"
    );

    assert_eq!(
        sink.log.len(),
        4,
        "the four content events land in the log; the lifecycle event does not"
    );
    assert!(
        matches!(entries.try_recv(), Ok(Event::UserInput { .. })),
        "the log broadcasts in append order, content events only"
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
