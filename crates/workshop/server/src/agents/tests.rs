//! The agent conversations' Harness setup: a prompt declaring
//! `web` prepares over the server's installed Plugins, a Host without the
//! search provider installs web as unavailable and refuses the prompt
//! naming that service, outside a runtime the services leave the runtime
//! out, and the ask id names the ask tool under the name user-input was
//! installed as. The `mcp.json` reader returns the servers sorted by
//! name, and the install loop adds each under its lowercased name, skipping
//! a name that does not parse or is taken. A remote `mcp.json` entry reads
//! the runtime the server provides, so it fails only on its connection.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use harness::plugin::{HostContext, HostServices};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, RunRequest};
use harness_gateway_client::{CompletionError, CompletionErrorKind};
use plugin_web::SEARCH_PROVIDER;
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use promptforge::tools::ToolId;
use promptforge_plugin::{ServiceKey, TOKIO_RUNTIME};
use workshop_agents::{Conversations, SessionState, TokioTimer};
use workshop_registry::Registry;

use super::{host_context, read_mcp_servers, services, with_plugins};

/// The runtime's key, built from the Plugin contract's name as any Host
/// builds it.
const RUNTIME: ServiceKey<tokio::runtime::Handle> = ServiceKey::new(TOKIO_RUNTIME);

/// A prompt that requires the Plugin `plugin` and returns a fixed text.
fn requiring(plugin: &str) -> String {
    format!(
        "---\nname: browses\ndescription: needs {plugin}\npromptforge: 0\n\
        plugins:\n  - {plugin}\n---\n\n\
        # Browses\n\n## Only\n\n```lua\nreturn 'browsed'\n```\n"
    )
}

/// A local `mcp.json` entry, which `plugin-mcp` refuses at install, so a
/// test installs it without opening a connection.
fn local_entry() -> serde_json::Value {
    serde_json::json!({ "command": "never-started" })
}

/// What `plugin-mcp` refuses `local_entry` with, which a run requiring the
/// installed server is refused naming.
const LOCAL_REFUSAL: &str = "local MCP servers (command) are not supported yet";

/// Lists no model and refuses every round as `Unavailable`, so a run
/// binds no model and nothing is fetched.
struct OfflineBroker;

impl InferenceBroker for OfflineBroker {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// Runs the prompt `source` as one conversation over `host`, then reads
/// the conversation's state and the outcome of its one run.
async fn run_prompt(host: &Arc<HostContext>, source: String) -> Option<RunOutcome> {
    let recorder = Arc::new(MemoryRecorder::new());
    let conversation = Conversations::new().open("browses");
    let harness = Harness::new(
        conversation.recorder(recorder.clone()),
        Arc::new(OfflineBroker),
        Arc::new(TokioTimer),
        Arc::clone(host),
        conversation.run_services(),
    );
    let request = RunRequest {
        name: conversation.id().to_string(),
        source,
        args: String::new(),
        input_text: None,
        vfs: VfsRef::default(),
        host: HostSnapshot::default(),
    };
    tokio::time::timeout(Duration::from_secs(10), conversation.run(harness, request))
        .await
        .expect("the run ends in time");
    assert_eq!(
        conversation.state(),
        SessionState::Closed,
        "the conversation ends with its run"
    );
    recorder.outcome(conversation.run_id().expect("the recorder began the run"))
}

/// Whether a run requiring `plugin` is refused as that Plugin being
/// installed but unavailable for the local-entry reason.
async fn installed_as_a_refused_local_server(host: &Arc<HostContext>, plugin: &str) -> bool {
    match run_prompt(host, requiring(plugin)).await {
        Some(RunOutcome::Failed { message, .. }) => {
            message.contains(&format!("- {plugin} is unavailable: {LOCAL_REFUSAL}"))
        }
        _ => false,
    }
}

/// An `mcp.json` file in a new directory holding `text`.
fn mcp_file(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, text).expect("write fixture");
    (dir, path)
}

#[tokio::test]
async fn a_prompt_declaring_web_prepares_on_the_servers_installed_plugins() {
    let (host, _ask) = host_context(&Registry::new(), None);
    assert_eq!(
        run_prompt(&Arc::new(host), requiring("web")).await,
        Some(RunOutcome::Completed {
            final_text: "browsed".to_owned(),
        }),
        "the server installs web and provides both services its install reads"
    );
}

#[tokio::test]
async fn a_host_without_the_search_provider_refuses_a_prompt_requiring_web() {
    let mut runtime_only = HostServices::new();
    runtime_only
        .provide(&RUNTIME, Arc::new(tokio::runtime::Handle::current()))
        .expect("an empty map takes the runtime");
    let (host, _ask) = with_plugins(runtime_only, Vec::new());

    let outcome = run_prompt(&Arc::new(host), requiring("web")).await;
    let Some(RunOutcome::Failed { kind, message }) = outcome else {
        panic!("the run is refused as it prepares: {outcome:?}");
    };
    assert_eq!(kind, "RequirementsUnmet");
    assert!(
        message.contains(
            "- web is unavailable: web needs promptforge/search-provider, and this host provides none"
        ),
        "the refusal names the Plugin and the missing service: {message}"
    );
}

#[test]
fn the_ask_id_is_the_ask_tool_under_the_name_user_input_was_installed_as() {
    let (_host, ask) = host_context(&Registry::new(), None);
    assert_eq!(
        ask,
        Some(ToolId::parse("user-input/ask").expect("the id parses")),
        "the server installs user-input under its default name"
    );
}

#[test]
fn outside_a_runtime_the_services_leave_the_runtime_out() {
    let services = services(&Registry::new());
    assert!(services.provides(&SEARCH_PROVIDER.id()));
    assert!(
        !services.provides(&RUNTIME.id()),
        "a synchronous caller has no runtime to provide"
    );
}

#[test]
fn no_setting_reads_no_file_and_gives_no_servers() {
    assert!(read_mcp_servers(None).is_empty());
}

#[test]
fn a_missing_mcp_file_gives_no_servers() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    assert!(read_mcp_servers(Some(&dir.path().join("absent.json"))).is_empty());
}

#[test]
fn an_unreadable_mcp_file_gives_no_servers() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A directory in the file's place fails the read for a reason other
    // than a missing file.
    let path = dir.path().join("mcp.json");
    std::fs::create_dir(&path).expect("directory in the file's place");
    assert!(read_mcp_servers(Some(&path)).is_empty());
}

#[test]
fn a_malformed_mcp_file_gives_no_servers() {
    for text in ["{ \"mcpServers\": ", "[]", "{ \"mcpServers\": [] }"] {
        let (_dir, path) = mcp_file(text);
        assert!(
            read_mcp_servers(Some(&path)).is_empty(),
            "`{text}` is not an object with an mcpServers object"
        );
    }
}

#[test]
fn a_file_without_mcp_servers_gives_no_servers() {
    let (_dir, path) = mcp_file(r#"{ "inputs": [] }"#);
    assert!(read_mcp_servers(Some(&path)).is_empty());
}

#[test]
fn an_entry_that_is_not_an_object_is_handed_on_for_the_plugin_to_refuse() {
    let (_dir, path) = mcp_file(r#"{ "mcpServers": { "a": 1 } }"#);
    assert_eq!(
        read_mcp_servers(Some(&path)),
        [("a".to_owned(), serde_json::json!(1))],
        "one bad entry does not drop the file's other servers"
    );
}

#[test]
fn a_valid_mcp_file_reads_back_sorted_by_name_and_ignores_other_keys() {
    let (_dir, path) = mcp_file(
        r#"{
            "inputs": [{ "id": "token" }],
            "mcpServers": {
                "wg21": { "url": "https://example.test/mcp" },
                "github": { "command": "x", "args": ["a"] },
                "docs": { "url": "https://example.test/docs" }
            }
        }"#,
    );
    let servers = read_mcp_servers(Some(&path));
    let names: Vec<&str> = servers.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, ["docs", "github", "wg21"]);
    assert_eq!(
        servers[1].1,
        serde_json::json!({ "command": "x", "args": ["a"] }),
        "each entry is handed on verbatim"
    );
}

#[tokio::test]
async fn each_entry_installs_once_under_its_lowercased_name() {
    let (host, _ask) = with_plugins(
        services(&Registry::new()),
        vec![
            ("GitHub".to_owned(), local_entry()),
            ("WG21-Papers".to_owned(), local_entry()),
        ],
    );
    let host = Arc::new(host);
    assert!(installed_as_a_refused_local_server(&host, "github").await);
    assert!(installed_as_a_refused_local_server(&host, "wg21-papers").await);
}

#[tokio::test]
async fn a_name_that_does_not_parse_is_skipped_and_the_other_servers_install() {
    let (host, _ask) = with_plugins(
        services(&Registry::new()),
        vec![
            ("has/slash".to_owned(), local_entry()),
            ("has space".to_owned(), local_entry()),
            ("docs".to_owned(), local_entry()),
        ],
    );
    assert!(
        installed_as_a_refused_local_server(&Arc::new(host), "docs").await,
        "the entry after the invalid names still installs"
    );
}

#[tokio::test]
async fn a_taken_name_or_punctuation_twin_is_skipped_and_the_other_servers_install() {
    let (host, _ask) = with_plugins(
        services(&Registry::new()),
        vec![
            ("web".to_owned(), local_entry()),
            ("a-b".to_owned(), local_entry()),
            ("A_B".to_owned(), local_entry()),
            ("docs".to_owned(), local_entry()),
        ],
    );
    let host = Arc::new(host);
    assert!(installed_as_a_refused_local_server(&host, "a-b").await);
    assert!(
        !installed_as_a_refused_local_server(&host, "a_b").await,
        "the twin of a-b is skipped, not installed beside it"
    );
    assert!(
        installed_as_a_refused_local_server(&host, "docs").await,
        "the entry after the skipped ones still installs"
    );
}

#[tokio::test]
async fn a_server_named_web_does_not_displace_the_web_plugin() {
    let (host, _ask) = with_plugins(
        services(&Registry::new()),
        vec![("web".to_owned(), local_entry())],
    );
    assert_eq!(
        run_prompt(&Arc::new(host), requiring("web")).await,
        Some(RunOutcome::Completed {
            final_text: "browsed".to_owned(),
        }),
        "web installs first, so the entry named web is the one refused"
    );
}

#[tokio::test]
async fn the_host_context_installs_the_servers_of_the_named_file() {
    let (_dir, path) = mcp_file(r#"{ "mcpServers": { "GitHub": { "command": "x" } } }"#);
    let (host, _ask) = host_context(&Registry::new(), Some(Path::new(&path)));
    assert!(installed_as_a_refused_local_server(&Arc::new(host), "github").await);
}

/// A loopback URL nothing listens on: the port is bound to learn it, then
/// freed, so a connection to it is refused at once.
fn closed_loopback_url() -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a loopback port");
    let port = listener.local_addr().expect("the bound address").port();
    drop(listener);
    format!("http://127.0.0.1:{port}/mcp")
}

#[tokio::test]
async fn a_remote_mcp_entry_reads_the_runtime_the_server_provides() {
    // The server's own `services` provides the runtime, and `plugin-mcp`
    // reads it through a key of its own, so only a run over both shows the
    // two keys name the same service.
    let (host, _ask) = with_plugins(
        services(&Registry::new()),
        vec![(
            "docs".to_owned(),
            serde_json::json!({ "url": closed_loopback_url() }),
        )],
    );

    let outcome = run_prompt(&Arc::new(host), requiring("docs")).await;
    let Some(RunOutcome::Failed { kind, message }) = outcome else {
        panic!("the run is refused as the closed port fails the handshake: {outcome:?}");
    };
    assert_eq!(kind, "RequirementsUnmet");
    assert!(
        message.contains("- docs is unavailable: the MCP handshake failed"),
        "the server got past the runtime read and failed at the connection: {message}"
    );
    assert!(
        !message.contains(TOKIO_RUNTIME),
        "the server's runtime reaches plugin-mcp's key: {message}"
    );
}
