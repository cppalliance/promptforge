//! Prepare-pass integration tests: capability resolution against the
//! registry (missing required reported, absent optional skipped and
//! logged), the run's services reaching `create`, activation failure
//! semantics, the per-run VFS claims isolation matrix, and model
//! satisfaction - the trivial fill binding every declared role to the
//! context's current model, the hard-keyword and context-minimum checks
//! against its descriptor, and `Environment::run` refusing an
//! unsatisfiable prompt.

use std::io;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use promptforge_api::capabilities::CapabilityRegistry;
use promptforge_api::execute::{
    Environment, RequirementCheck, RunContext, RunErrorKind, RunResult,
};
use promptforge_api::parser::Prompt;
use promptforge_tool_picker::{Catalog, Config, ToolDescriptor, ToolPicker};
use shared_promptforge_api::cancel::CancelHandle;
use shared_promptforge_api::capabilities::{
    Capability, CapabilityError, CapabilityId, Contribution, RunServices,
};
use shared_promptforge_api::models::{ModelDescriptor, ModelId, ThinkingMode};
use shared_promptforge_api::observe::NullObserver;
use shared_promptforge_api::tools::{Tool, ToolError, ToolId, ToolOutput};
use shared_vfs::{HostBackend, Origin, VfsError, VfsRef};

/// A prompt declaring `promptforge/web` as a required capability.
const DECLARES_REQUIRED: &str = concat!(
    "---\n",
    "name: declares-required\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` as an optional capability.
const DECLARES_OPTIONAL: &str = concat!(
    "---\n",
    "name: declares-optional\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - ref: promptforge/web\n",
    "    optional: true\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring no capabilities at all.
const DECLARES_NOTHING: &str = concat!(
    "---\n",
    "name: declares-nothing\n",
    "description: d\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// Parses a fixture prompt.
fn parse(source: &str, execution: &str) -> Prompt {
    Prompt::parse(source, execution, &NullObserver::default()).expect("the fixture prompt parses")
}

/// What one activation observed: the marker round-trip through the
/// services VFS and the cancellation handle it was handed.
#[derive(Debug)]
struct Activation {
    /// The marker read back through the services VFS, when it round-tripped.
    marker: Option<String>,
    /// The cancellation handle `create` received.
    cancel: CancelHandle,
}

/// A fixture capability recording each activation's services. `fail`
/// turns every activation into a [`CapabilityError`].
struct Fixture {
    id: CapabilityId,
    description: String,
    fail: bool,
    activations: Arc<Mutex<Vec<Activation>>>,
}

impl Fixture {
    /// Builds a fixture capability registered under `id`.
    fn new(id: &str, fail: bool) -> (Arc<Fixture>, Arc<Mutex<Vec<Activation>>>) {
        let activations = Arc::new(Mutex::new(Vec::new()));
        let fixture = Arc::new(Fixture {
            id: CapabilityId::parse(id).expect("the fixture id is valid"),
            description: format!("The {id} fixture capability."),
            fail,
            activations: Arc::clone(&activations),
        });
        (fixture, activations)
    }
}

impl Capability for Fixture {
    fn id(&self) -> &CapabilityId {
        &self.id
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        if self.fail {
            return Err(CapabilityError::message("the fixture cannot activate"));
        }
        let path = format!("{}/activated.txt", promptforge_vfs::STORE_MOUNT);
        let access = services
            .vfs
            .acquire(Origin::new("fixture activation"))
            .map_err(|error| {
                CapabilityError::with_source("the fixture could not acquire", error)
            })?;
        access
            .write(&path, b"active")
            .map_err(|error| CapabilityError::with_source("the fixture could not write", error))?;
        let marker = access
            .read(&path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        self.activations
            .lock()
            .expect("the activations lock is not poisoned")
            .push(Activation {
                marker,
                cancel: services.cancel.clone(),
            });
        Ok(Contribution::default())
    }
}

/// A shared buffer a fmt subscriber writes log lines into.
#[derive(Clone, Default)]
struct Buffer {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl io::Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes
            .lock()
            .expect("the buffer lock is not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Runs `f` under a fmt subscriber writing into a shared buffer and
/// returns everything the subscriber captured.
fn captured_logs(f: impl FnOnce()) -> String {
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buffer
        .bytes
        .lock()
        .expect("the buffer lock is not poisoned");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A unique temporary directory that removes itself on drop. The suite has
/// no tempfile dependency; this mirrors shared-vfs's own test helper.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "promptforge-api-prepare-{}-{unique}-{name}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("the temp dir creates");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_missing_required_capability_is_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let env = Environment::new();
    let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-missing"));
    assert!(requirements.unmet_requirements.is_empty());
    assert_eq!(
        requirements.missing_required,
        [CapabilityId::parse("promptforge/web").expect("the id is valid")]
    );
    assert!(!requirements.is_satisfied());
}

#[test]
fn an_absent_optional_capability_is_skipped_and_logged() {
    let prompt = parse(DECLARES_OPTIONAL, "declares-optional");
    let env = Environment::new();
    let logs = captured_logs(|| {
        let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-optional"));
        assert!(requirements.missing_required.is_empty());
        assert!(requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the skip log line names the capability: {logs}"
    );
}

#[test]
fn activation_receives_the_runs_own_services() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, activations) = Fixture::new("promptforge/web", false);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let cancel = CancelHandle::new();
    let (ctx, requirements) = env.prepare(
        &prompt,
        RunContext::new("prepare-services").cancel(cancel.clone()),
    );
    assert!(requirements.is_satisfied());
    // The host-supplied cancellation handle reached `create` unchanged.
    let activations = activations.lock().expect("the lock is not poisoned");
    assert_eq!(activations.len(), 1, "create ran exactly once");
    assert_eq!(activations[0].marker.as_deref(), Some("active"));
    assert!(!activations[0].cancel.is_cancelled());
    cancel.cancel();
    assert!(
        activations[0].cancel.is_cancelled(),
        "the activated handle is the run's own"
    );
    drop(activations);
    // The services VFS is the run's prepared handle: the activation's
    // marker is readable through the context's store mount.
    let access = ctx
        .vfs_handle()
        .acquire(Origin::new("post-prepare read"))
        .expect("the prepared handle acquires");
    let marker = format!("{}/activated.txt", promptforge_vfs::STORE_MOUNT);
    assert_eq!(
        access.read(&marker).expect("the marker persists"),
        b"active"
    );
}

#[test]
fn a_required_activation_failure_is_logged_and_reported() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let (fixture, _activations) = Fixture::new("promptforge/web", true);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        // A present-but-failing required capability leaves the run
        // without something the prompt declared: it is reported like an
        // absent one, and the failure is also a log line.
        let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-failing"));
        assert_eq!(
            requirements.missing_required,
            [CapabilityId::parse("promptforge/web").expect("the id is valid")]
        );
        assert!(!requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the failure log line names the capability: {logs}"
    );
}

#[test]
fn an_optional_activation_failure_is_logged_and_contributes_nothing() {
    let prompt = parse(DECLARES_OPTIONAL, "declares-optional");
    let (fixture, _activations) = Fixture::new("promptforge/web", true);
    let mut registry = CapabilityRegistry::new();
    registry.register(fixture).expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        // An optional capability that fails to activate is only a log
        // line: the prompt declared it could run without.
        let (_ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-failing"));
        assert!(requirements.is_satisfied());
    });
    assert!(
        logs.contains("promptforge/web"),
        "the failure log line names the capability: {logs}"
    );
}

#[test]
fn two_runs_writing_the_same_store_path_do_not_conflict() {
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let env = Environment::new();
    let (ctx_a, _) = env.prepare(&prompt, RunContext::new("run-a"));
    let (ctx_b, _) = env.prepare(&prompt, RunContext::new("run-b"));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    let path = format!("{}/paper.md", promptforge_vfs::STORE_MOUNT);
    // Both writes proceed while both accesses are live: each run's store
    // is its own storage under its own claims table.
    access_a.write(&path, b"from a").expect("run a writes");
    access_b.write(&path, b"from b").expect("run b writes");
    assert_eq!(access_a.read(&path).expect("run a reads"), b"from a");
    assert_eq!(access_b.read(&path).expect("run b reads"), b"from b");
}

#[test]
fn two_runs_writing_the_same_host_file_through_the_shared_base_conflict() {
    let temp = TempDir::new("shared-base");
    let base = VfsRef::builder()
        .mount(
            "/",
            HostBackend::rooted(&temp.0).expect("the temp dir roots the host backend"),
        )
        .build();
    let env = Environment::new().base_vfs(base);
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let (ctx_a, _) = env.prepare(&prompt, RunContext::new("run-a"));
    let (ctx_b, _) = env.prepare(&prompt, RunContext::new("run-b"));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    access_a
        .write("/shared.txt", b"from a")
        .expect("run a writes the host file");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    // The shared base's claims table sees two live identities on one path.
    let error = access_b
        .write("/shared.txt", b"from b")
        .expect_err("run b conflicts with run a's live claim");
    assert!(
        matches!(error, VfsError::Conflict(_)),
        "a determinism violation, not a backend error: {error}"
    );
    // The conflicting write never partially applied.
    assert_eq!(
        std::fs::read(temp.0.join("shared.txt")).expect("run a's write landed on disk"),
        b"from a"
    );
}

/// A prompt declaring one model role with a hard keyword and a context
/// minimum.
const DECLARES_ANALYST: &str = concat!(
    "---\n",
    "name: declares-analyst\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  analyst:\n",
    "    keywords: [frontier, thinking]\n",
    "    min_context: 200000\n",
    "    description: Deep analysis\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// A prompt declaring one role per soft keyword: documentation of author
/// intent, never a check.
const DECLARES_SOFT_ROLES: &str = concat!(
    "---\n",
    "name: declares-soft-roles\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  scout:\n",
    "    keywords: [frontier, fast]\n",
    "  sprinter:\n",
    "    keywords: [small, creative, chat]\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// A prompt declaring one role with the `no-thinking` hard keyword.
const DECLARES_NO_THINKING: &str = concat!(
    "---\n",
    "name: declares-no-thinking\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  triage:\n",
    "    keywords: [no-thinking]\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// Builds the host's one current model with the given context window and
/// thinking capability.
fn current_model(context: u32, thinking: ThinkingMode) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("current").expect("the id is valid"),
        "The host's current model",
        NonZeroU32::new(context).expect("the context window is non-zero"),
        thinking,
    )
}

#[test]
fn every_declared_role_resolves_to_the_current_model() {
    let prompt = parse(DECLARES_SOFT_ROLES, "declares-soft-roles");
    let env = Environment::new();
    let model = current_model(32_000, ThinkingMode::Never);
    let (ctx, requirements) = env.prepare(&prompt, RunContext::new("fill").model(model.clone()));
    // Soft keywords document intent and the roles declare no minimum:
    // nothing is reported.
    assert!(requirements.is_satisfied());
    // The trivial fill binds every declared role to the current model,
    // and handles resolve label -> id -> descriptor.
    let bindings = ctx.model_bindings();
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings.role_id("scout"), Some(model.id()));
    assert_eq!(bindings.resolve("scout"), Some(&model));
    assert_eq!(bindings.resolve("sprinter"), Some(&model));
    assert_eq!(bindings.model(model.id()), Some(&model));
    assert!(bindings.resolve("undeclared").is_none());
}

#[test]
fn a_context_minimum_above_the_current_models_is_reported() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    // min_context 200000 against the model's 32000.
    let (_ctx, requirements) = env.prepare(
        &prompt,
        RunContext::new("fill").model(current_model(32_000, ThinkingMode::Always)),
    );
    assert!(!requirements.is_satisfied());
    assert_eq!(requirements.missing_required, []);
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        panic!(
            "exactly one requirement is unmet: {:?}",
            requirements.unmet_requirements
        );
    };
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::ContextMinimum);
    assert_eq!(unmet.required, "200000");
    assert_eq!(unmet.actual, "32000");
}

#[test]
fn a_hard_keyword_the_current_model_fails_is_reported() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    // `thinking` against a Never model, with the context minimum met so
    // only the keyword check fires.
    let (_ctx, requirements) = env.prepare(
        &prompt,
        RunContext::new("fill").model(current_model(200_000, ThinkingMode::Never)),
    );
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        panic!(
            "exactly one requirement is unmet: {:?}",
            requirements.unmet_requirements
        );
    };
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "thinking");
    assert_eq!(unmet.actual, "Never");

    let prompt = parse(DECLARES_NO_THINKING, "declares-no-thinking");
    // `no-thinking` against a Switchable model.
    let (_ctx, requirements) = env.prepare(
        &prompt,
        RunContext::new("fill").model(current_model(32_000, ThinkingMode::Switchable)),
    );
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        panic!(
            "exactly one requirement is unmet: {:?}",
            requirements.unmet_requirements
        );
    };
    assert_eq!(unmet.role, "triage");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "no-thinking");
    assert_eq!(unmet.actual, "Switchable");
}

#[tokio::test]
async fn env_run_refuses_an_unsatisfiable_prompt_with_a_model_readable_notice() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    let result = env
        .run(
            &prompt,
            "",
            RunContext::new("refuse").model(current_model(32_000, ThinkingMode::Never)),
        )
        .await;
    let RunResult::Failure(error) = result else {
        panic!("an unsatisfiable prompt is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    // The notice is written to be read by a model: it names the role,
    // each failed check, and required versus actual.
    assert!(
        notice.contains("analyst"),
        "the notice names the role: {notice}"
    );
    assert!(
        notice.contains("200000") && notice.contains("32000"),
        "the notice gives required versus actual context: {notice}"
    );
    assert!(
        notice.contains("thinking") && notice.contains("Never"),
        "the notice gives required versus actual keywords: {notice}"
    );
}

#[tokio::test]
async fn env_run_refuses_a_missing_required_capability_with_a_notice_naming_it() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    // No registry: the declared required capability is absent.
    let env = Environment::new();
    let result = env
        .run(&prompt, "", RunContext::new("refuse-missing"))
        .await;
    let RunResult::Failure(error) = result else {
        panic!("a prompt missing a required capability is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("missing required capability: promptforge/web"),
        "the notice names the missing capability: {notice}"
    );
}

#[tokio::test]
async fn env_run_prepares_implicitly_and_runs_a_satisfiable_prompt() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    // The zero-burden path: no explicit prepare call, and the declared
    // role's requirements are met by the current model.
    let result = env
        .run(
            &prompt,
            "",
            RunContext::new("implicit").model(current_model(200_000, ThinkingMode::Always)),
        )
        .await;
    let RunResult::Ok(text) = result else {
        panic!("a satisfiable prompt runs through implicit prepare: {result:?}");
    };
    assert_eq!(text, "done");
}

// Catalog assembly and conflict checks: prepare assembles the activated
// capabilities' contributed tools into the run's catalog in declaration
// order, enforcing tool prefix-containment at assembly, and rejects
// capability co-activation conflicts naming both.

/// A prompt declaring `promptforge/bashkit` and `promptforge/terminal`,
/// in that order.
const DECLARES_CONFLICTING: &str = concat!(
    "---\n",
    "name: declares-conflicting\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/bashkit\n",
    "  - promptforge/terminal\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` and `promptforge/fs`, in that
/// order.
const DECLARES_TWO: &str = concat!(
    "---\n",
    "name: declares-two\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "  - promptforge/fs\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A fixture tool: a static id and description, its name segment as the
/// wire name, and an empty trusted output. The description matters: the
/// fuzzy slot fill indexes it, so picker-backed tests need a tool whose
/// description says what the tool does.
struct FixtureTool {
    id: ToolId,
    description: String,
}

#[async_trait::async_trait]
impl Tool for FixtureTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        self.id.name()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

/// A fixture capability contributing tools and declaring co-activation
/// conflicts.
struct ToolFixture {
    id: CapabilityId,
    conflicts: Vec<CapabilityId>,
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolFixture {
    /// Builds a fixture registered under `id`, contributing `tools` and
    /// conflicting with each id in `conflicts`.
    fn new(id: &str, conflicts: &[&str], tools: Vec<Arc<dyn Tool>>) -> ToolFixture {
        ToolFixture {
            id: CapabilityId::parse(id).expect("the fixture id is valid"),
            conflicts: conflicts
                .iter()
                .map(|id| CapabilityId::parse(id).expect("the conflict id is valid"))
                .collect(),
            tools,
        }
    }
}

impl Capability for ToolFixture {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A tool-contributing fixture capability."
    }

    fn conflicts(&self) -> &[CapabilityId] {
        &self.conflicts
    }

    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
        let _ = services;
        Ok(Contribution {
            tools: self.tools.clone(),
        })
    }
}

/// Builds a fixture tool arc under `id`.
fn fixture_tool(id: &str) -> Arc<dyn Tool> {
    Arc::new(FixtureTool {
        id: ToolId::parse(id).expect("the fixture tool id is valid"),
        description: "A fixture tool.".to_owned(),
    })
}

/// Builds a fixture tool arc under `id` whose description says what the
/// tool does, so the picker's fuzzy fill has real prose to index.
fn described_tool(id: &str, description: &str) -> Arc<dyn Tool> {
    Arc::new(FixtureTool {
        id: ToolId::parse(id).expect("the fixture tool id is valid"),
        description: description.to_owned(),
    })
}

#[test]
fn a_co_activation_conflict_fails_preparation_naming_both() {
    let prompt = parse(DECLARES_CONFLICTING, "declares-conflicting");
    // The check is symmetric: the conflict is found whether the earlier-
    // or the later-declared capability declares it.
    for (bashkit_conflicts, terminal_conflicts) in [
        (vec!["promptforge/terminal"], vec![]),
        (vec![], vec!["promptforge/bashkit"]),
    ] {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(Arc::new(ToolFixture::new(
                "promptforge/bashkit",
                &bashkit_conflicts,
                vec![fixture_tool("promptforge/bashkit/run")],
            )))
            .expect("bashkit registers");
        registry
            .register(Arc::new(ToolFixture::new(
                "promptforge/terminal",
                &terminal_conflicts,
                vec![fixture_tool("promptforge/terminal/run")],
            )))
            .expect("terminal registers");
        let env = Environment::new().registry(registry);
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-conflict"));
        assert!(!requirements.is_satisfied());
        let [conflict] = requirements.conflicts.as_slice() else {
            panic!(
                "exactly one conflict is reported: {:?}",
                requirements.conflicts
            );
        };
        // Both capabilities are named, in declaration order.
        assert_eq!(conflict.first.to_string(), "promptforge/bashkit");
        assert_eq!(conflict.second.to_string(), "promptforge/terminal");
        // A context gets one filesystem reality or the other, never
        // both: neither member of the conflicting pair activated, so
        // neither tool reached the catalog.
        assert!(ctx.tools().tools().is_empty());
    }
}

#[tokio::test]
async fn env_run_refuses_a_conflicting_pair_with_a_notice_naming_both() {
    let prompt = parse(DECLARES_CONFLICTING, "declares-conflicting");
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/bashkit",
            &["promptforge/terminal"],
            vec![],
        )))
        .expect("bashkit registers");
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/terminal",
            &[],
            vec![],
        )))
        .expect("terminal registers");
    let env = Environment::new().registry(registry);
    let result = env
        .run(&prompt, "", RunContext::new("refuse-conflict"))
        .await;
    let RunResult::Failure(error) = result else {
        panic!("a conflicting pair is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("promptforge/bashkit") && notice.contains("promptforge/terminal"),
        "the notice names both conflicting capabilities: {notice}"
    );
}

#[test]
fn a_contributed_tool_outside_the_capabilitys_id_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let good = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let stray = ToolId::parse("promptforge/other/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            fixture_tool("promptforge/web/fetch"),
            fixture_tool("promptforge/other/fetch"),
        ],
    );
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(fixture))
        .expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-containment"));
        // Containment is enforced at assembly, not reported: the run is
        // satisfiable and the stray tool simply never enters the catalog.
        assert!(requirements.is_satisfied());
        let catalog = ctx.tools();
        assert!(
            catalog.get(&good).is_some(),
            "the contained tool is assembled"
        );
        assert!(
            catalog.get(&stray).is_none(),
            "the containment violation is rejected at assembly"
        );
        assert_eq!(catalog.tools().len(), 1);
    });
    assert!(
        logs.contains("promptforge/other/fetch") && logs.contains("promptforge/web"),
        "the rejection log names the capability and the tool: {logs}"
    );
}

#[test]
fn the_catalog_assembles_contributed_tools_in_declaration_order() {
    let prompt = parse(DECLARES_TWO, "declares-two");
    let web = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            fixture_tool("promptforge/web/fetch"),
            fixture_tool("promptforge/web/search"),
        ],
    );
    let fs = ToolFixture::new(
        "promptforge/fs",
        &[],
        vec![fixture_tool("promptforge/fs/read")],
    );
    let mut registry = CapabilityRegistry::new();
    registry.register(Arc::new(web)).expect("web registers");
    registry.register(Arc::new(fs)).expect("fs registers");
    let env = Environment::new().registry(registry);
    let (ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-order"));
    assert!(requirements.is_satisfied());
    let ids: Vec<String> = ctx
        .tools()
        .tools()
        .iter()
        .map(|tool| tool.id().to_string())
        .collect();
    assert_eq!(
        ids,
        [
            "promptforge/web/fetch",
            "promptforge/web/search",
            "promptforge/fs/read"
        ],
        "declaration order, then contribution order within each capability"
    );
}

/// A fixture tool whose wire name is transport-illegal: identity is a
/// valid contained id, but the advertised name carries a `/` separator.
struct BadWireTool {
    id: ToolId,
    wire: String,
}

#[async_trait::async_trait]
impl Tool for BadWireTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn wire_name(&self) -> &str {
        &self.wire
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "A fixture tool with an illegal wire name."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }

    async fn call(&self, _args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::trusted(String::new()))
    }
}

#[test]
fn a_repeated_tool_id_across_contributions_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let repeated = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            fixture_tool("promptforge/web/fetch"),
            fixture_tool("promptforge/web/search"),
            // The repeat: one capability contributes the same id twice.
            fixture_tool("promptforge/web/fetch"),
        ],
    );
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(fixture))
        .expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-duplicate"));
        // The repeat is rejected at assembly, not reported: the first
        // contribution stands and the run is satisfiable.
        assert!(requirements.is_satisfied());
        let catalog = ctx.tools();
        assert!(catalog.get(&repeated).is_some());
        assert_eq!(
            catalog.tools().len(),
            2,
            "the repeated id enters the catalog exactly once"
        );
    });
    assert!(
        logs.contains("promptforge/web/fetch") && logs.contains("promptforge/web"),
        "the rejection log names the capability and the repeated tool: {logs}"
    );
}

#[test]
fn a_transport_illegal_wire_name_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let bad = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            Arc::new(BadWireTool {
                id: bad.clone(),
                wire: "fetch/v2".to_owned(),
            }),
            fixture_tool("promptforge/web/search"),
        ],
    );
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(fixture))
        .expect("the fixture registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("prepare-wire-name"));
        // One bad tool costs only itself: the run is satisfiable and
        // the well-formed tool still assembles.
        assert!(requirements.is_satisfied());
        let catalog = ctx.tools();
        assert!(
            catalog.get(&bad).is_none(),
            "the illegal wire name is rejected at assembly"
        );
        assert_eq!(catalog.tools().len(), 1);
    });
    assert!(
        logs.contains("promptforge/web/fetch") && logs.contains("promptforge/web"),
        "the rejection log names the capability and the rejected tool: {logs}"
    );
}

// ToolBindings and slot filling: exact slots fill by identity against
// the assembled catalog (an exact path's first two segments name its
// capability, so a slot whose capability is inactive is reported as
// missing), fuzzy slots fill through the picker over the assembled
// catalog with every fill journaled into the run's tool bindings, and
// an unfillable optional fuzzy slot skips with a log line.

/// A prompt declaring `promptforge/web` and one exact tool slot.
const DECLARES_EXACT_SLOT: &str = concat!(
    "---\n",
    "name: declares-exact-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring one exact tool slot whose capability is not
/// declared at all.
const DECLARES_ORPHAN_SLOT: &str = concat!(
    "---\n",
    "name: declares-orphan-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` and one fuzzy tool slot.
const DECLARES_FUZZY_SLOT: &str = concat!(
    "---\n",
    "name: declares-fuzzy-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "tools:\n",
    "  fetch:\n",
    "    want: Fetch a web page over HTTP\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` and one optional fuzzy tool slot
/// nothing in the assembled catalog matches.
const DECLARES_OPTIONAL_FUZZY: &str = concat!(
    "---\n",
    "name: declares-optional-fuzzy\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "tools:\n",
    "  email:\n",
    "    want: Send an email to the team\n",
    "    optional: true\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// The one loaded picker model for this test binary: the fuzzy fill
/// rebuilds the environment picker's model over the run's assembled
/// catalog, so the picker must carry real weights.
fn picker_model() -> &'static promptforge_tool_picker::Model {
    static MODEL: std::sync::OnceLock<promptforge_tool_picker::Model> = std::sync::OnceLock::new();
    MODEL.get_or_init(|| {
        promptforge_tool_picker::Model::load().expect("the compiled-in model must load")
    })
}

/// Builds the environment's deployment picker over the shared model. Its
/// catalog content is irrelevant to the fill - prepare re-indexes the
/// run's own assembled catalog - but the build needs one entry so the
/// real model, not a dummy, rides along.
fn test_picker() -> ToolPicker {
    let catalog = Catalog::new(vec![ToolDescriptor::new(
        ToolId::parse("promptforge/web/fetch").expect("the id is valid"),
        "Fetch a web page over HTTP",
        serde_json::json!({"type": "object", "properties": {}}),
    )]);
    ToolPicker::build_with_model(picker_model(), catalog, Config::default(), None)
        .expect("the test picker builds")
}

/// Registers `promptforge/web` contributing one described fetch tool.
fn web_registry() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/web",
            &[],
            vec![described_tool(
                "promptforge/web/fetch",
                "Fetch a web page over HTTP",
            )],
        )))
        .expect("web registers");
    registry
}

#[test]
fn an_exact_slot_fills_against_the_assembled_catalog() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    let env = Environment::new().registry(web_registry());
    let (ctx, requirements) = env.prepare(&prompt, RunContext::new("fill-exact"));
    assert!(requirements.is_satisfied());
    let id = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let bindings = ctx.tool_bindings();
    assert_eq!(bindings.len(), 1);
    // Handles resolve alias -> id -> tool.
    assert_eq!(bindings.alias_id("fetch"), Some(&id));
    assert_eq!(
        bindings.resolve("fetch").map(|tool| tool.id()),
        Some(id.clone())
    );
    assert!(bindings.tool(&id).is_some());
    assert!(bindings.resolve("undeclared").is_none());
}

#[test]
fn an_exact_slot_whose_capability_is_inactive_is_reported() {
    let prompt = parse(DECLARES_ORPHAN_SLOT, "declares-orphan-slot");
    // No registry and no declaration: the slot's capability is inactive.
    let env = Environment::new();
    let (ctx, requirements) = env.prepare(&prompt, RunContext::new("fill-orphan"));
    // The exact path's first two segments name its capability.
    assert_eq!(
        requirements.missing_required,
        [CapabilityId::parse("promptforge/web").expect("the id is valid")]
    );
    assert!(!requirements.is_satisfied());
    assert!(ctx.tool_bindings().is_empty());
}

#[test]
fn an_exact_slot_absent_from_an_active_capability_is_not_reported_missing() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    // The capability activates but contributes a different tool: the
    // slot's capability is not missing, so the run must not fail
    // unsatisfiably - installing changes nothing.
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/web",
            &[],
            vec![described_tool(
                "promptforge/web/search",
                "Search the web",
            )],
        )))
        .expect("web registers");
    let env = Environment::new().registry(registry);
    let logs = captured_logs(|| {
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("fill-absent-tool"));
        assert!(
            requirements.missing_required.is_empty(),
            "an active capability is never reported missing: {:?}",
            requirements.missing_required
        );
        assert!(requirements.is_satisfied());
        // The alias stays unbound; advertising it fails at run time.
        assert!(ctx.tool_bindings().is_empty());
    });
    assert!(
        logs.contains("fetch"),
        "the warning names the unfilled alias: {logs}"
    );
}

#[test]
fn a_fuzzy_slot_fills_via_the_picker_and_the_fill_is_journaled() {
    let prompt = parse(DECLARES_FUZZY_SLOT, "declares-fuzzy-slot");
    let env = Environment::new()
        .registry(web_registry())
        .picker(test_picker());
    let logs = captured_logs(|| {
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("fill-fuzzy"));
        assert!(requirements.is_satisfied());
        let id = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
        let bindings = ctx.tool_bindings();
        // The journaled fill: the alias resolves to the picked tool.
        assert_eq!(bindings.alias_id("fetch"), Some(&id));
        assert!(bindings.resolve("fetch").is_some());
    });
    assert!(
        logs.contains("promptforge/web/fetch"),
        "the journal records what the fuzz resolved to: {logs}"
    );
}

#[test]
fn an_optional_fuzzy_slot_with_no_match_is_skipped_and_logged() {
    let prompt = parse(DECLARES_OPTIONAL_FUZZY, "declares-optional-fuzzy");
    let env = Environment::new()
        .registry(web_registry())
        .picker(test_picker());
    let logs = captured_logs(|| {
        let (ctx, requirements) = env.prepare(&prompt, RunContext::new("fill-optional-fuzzy"));
        // An unfillable optional slot is a log line, not a report field.
        assert!(requirements.is_satisfied());
        assert!(ctx.tool_bindings().is_empty());
    });
    assert!(
        logs.contains("email"),
        "the skip log line names the alias: {logs}"
    );
}
