//! Shared fixtures for the activation suite: the activate-then-prepare
//! ceremony, the run driver, fixture capabilities and tools, and the log
//! capture.

use std::io;
use std::sync::{Arc, Mutex};

use harness_capabilities::{
    Activation, Capability, CapabilityError, CapabilityId, CapabilityRegistry, Contribution,
    RunServices, Tool, activate,
};
use promptforge::Prompt;
use promptforge::Run;
use promptforge::Step;
use promptforge::cancel::CancelHandle;
use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolError, ToolId, ToolOutput};
use promptforge::vfs::{Origin, perform_store_op};
use promptforge::{Environment, Requirements, RunContext, RunResult};

/// The run's store mount inside its VFS, where `store.read('x')` resolves
/// `x`. The engine's private `promptforge-vfs` crate names it; the harness
/// cannot, so the suite pins the path.
pub(super) const STORE_MOUNT: &str = "/_promptforge/store";

/// A [`RunContext`] for the run `name` under fixed host inputs: no fixture
/// here asserts on the nonce or `sys.when`.
pub(super) fn context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, 1, Timestamp::UNIX_EPOCH)
}

/// Parses a fixture prompt.
pub(super) fn parse(source: &str, execution: &str) -> Prompt {
    Prompt::parse(source, execution)
        .0
        .expect("the fixture prompt parses")
}

/// The harness's activate-then-prepare ceremony spelled out, so a test can
/// inspect what the run path folds into one refusal: builds the run's VFS
/// from `env`, activates the prompt's declared capabilities against
/// `registry` with the run's own services, installs the resulting catalog,
/// prepares the context over that VFS, and merges activation's report
/// into prepare's. Returns the prepared context, the merged report, and
/// the activation (for its implementation table).
pub(super) fn prepare_activated(
    env: Environment,
    registry: Option<&CapabilityRegistry>,
    prompt: &Prompt,
    ctx: RunContext,
) -> (RunContext, Requirements, Activation) {
    let vfs = env.run_vfs();
    let services = RunServices::new(vfs.clone(), ctx.cancel_handle());
    let activation = activate(registry, prompt, &services);
    let env = env.tools(activation.catalog.clone());
    let (ctx, mut requirements) = env.prepare(prompt, ctx.vfs(vfs));
    requirements.merge(activation.requirements.clone());
    (ctx, requirements, activation)
}

/// The harness's run path with capabilities: activates against
/// `registry`, installs the catalog, prepares, merges the activation
/// report, refuses an unsatisfiable prompt, and otherwise drives the run
/// on the store-only loop below (no fixture here performs a chat, tool,
/// or input effect).
pub(super) fn run_activated(
    registry: &CapabilityRegistry,
    prompt: &Prompt,
    ctx: RunContext,
) -> RunResult {
    let (ctx, requirements, _activation) =
        prepare_activated(Environment::new(), Some(registry), prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return RunResult::Failure(refusal);
    }
    drive_store_only(Run::new(Arc::new(prompt.clone()), "", ctx))
}

/// Drives `run` to its result, answering [`Effect::Store`] through
/// [`perform_store_op`] and panicking on any other effect, which names a
/// fixture that issues something this suite does not host.
fn drive_store_only(mut run: Run) -> RunResult {
    loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    let Effect::Store { access, op } = effect else {
                        panic!("the activation suite performs only store effects: {effect:?}");
                    };
                    run.resume(id, EffectAnswer::Store(perform_store_op(&access, op)));
                }
            }
            Step::Done { result, .. } => return result,
        }
    }
}

/// What one activation observed: the marker round-trip through the
/// services VFS and the cancellation handle it was handed.
#[derive(Debug)]
pub(super) struct Observed {
    /// The marker read back through the services VFS, when it round-tripped.
    pub(super) marker: Option<String>,
    /// The cancellation handle `create` received.
    pub(super) cancel: CancelHandle,
}

/// A fixture capability recording each activation's services. `fail`
/// turns every activation into a [`CapabilityError`].
pub(super) struct Fixture {
    id: CapabilityId,
    description: String,
    fail: bool,
    activations: Arc<Mutex<Vec<Observed>>>,
}

impl Fixture {
    /// Builds a fixture capability registered under `id`.
    pub(super) fn new(id: &str, fail: bool) -> (Arc<Fixture>, Arc<Mutex<Vec<Observed>>>) {
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
        let path = format!("{STORE_MOUNT}/activated.txt");
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
            .push(Observed {
                marker,
                cancel: services.cancel.clone(),
            });
        Ok(Contribution::default())
    }
}

/// A fixture tool: a static id and description, its name segment as the
/// wire name, and an empty trusted output.
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

/// A fixture tool whose wire name is transport-illegal: identity is a
/// valid contained id, but the advertised name contains a `/` separator.
pub(super) struct BadWireTool {
    pub(super) id: ToolId,
    pub(super) wire: String,
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

/// A fixture capability contributing tools and declaring co-activation
/// conflicts.
pub(super) struct ToolFixture {
    id: CapabilityId,
    conflicts: Vec<CapabilityId>,
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolFixture {
    /// Builds a fixture registered under `id`, contributing `tools` and
    /// conflicting with each id in `conflicts`.
    pub(super) fn new(id: &str, conflicts: &[&str], tools: Vec<Arc<dyn Tool>>) -> ToolFixture {
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
pub(super) fn fixture_tool(id: &str) -> Arc<dyn Tool> {
    Arc::new(FixtureTool {
        id: ToolId::parse(id).expect("the fixture tool id is valid"),
        description: "A fixture tool.".to_owned(),
    })
}

/// Builds a fixture tool arc under `id` with an explicit description.
pub(super) fn described_tool(id: &str, description: &str) -> Arc<dyn Tool> {
    Arc::new(FixtureTool {
        id: ToolId::parse(id).expect("the fixture tool id is valid"),
        description: description.to_owned(),
    })
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
pub(super) fn captured_logs(f: impl FnOnce()) -> String {
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
