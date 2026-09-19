//! Per-run context and resource limits: [`RunContext`] and [`RunLimits`].

use std::fmt;
use std::sync::Arc;

#[path = "config-limits.rs"]
mod limits;

use promptforge_api_types::replay::Flags;
use promptforge_api_types::timestamp::Timestamp;

pub use limits::RunLimits;

use crate::cancel::CancelHandle;
use crate::client::{GatewayClient, StreamDelta};
use crate::debug::DebugCapture;
use crate::input::InputBroker;
use crate::model::ModelDescriptor;
use crate::observe::{NullObserver, Observer};
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::bindings::{ModelBindings, ToolBindings};

/// One run. Created by the host from the
/// [`Environment`](super::Environment) carrying the per-run inputs,
/// enriched at prepare, owned by the executor during
/// [`run`](super::run). Never shared between runs.
///
/// `RunContext` is owned (no borrows), so its observer and debug sinks reach
/// the nested `models.infer` path that a borrowed option could not.
///
/// The engine reads no clock and draws no randomness of its own: the
/// run's `seed` and `started_at` are inputs the host supplies to
/// [`new`](RunContext::new) (a harness draws both, records both, and a
/// replay hands back the recorded values), so given the same inputs and
/// the same answers a run reproduces its nonces, `sys.when`, effects, and
/// events. There is no default for either: a host that runs once draws
/// the seed from its own CSPRNG and stamps its own clock.
///
/// # Examples
/// ```
/// use promptforge_api_runtime::execute::{RunContext, RunLimits};
/// use promptforge_api_types::timestamp::Timestamp;
///
/// let ctx = RunContext::new("example-run", 7, Timestamp::from_unix_millis(951_782_400_000))
///     .limits(RunLimits::new());
/// assert_eq!(ctx.name(), "example-run");
/// assert_eq!(ctx.seed(), 7);
/// assert_eq!(ctx.started_at().to_rfc3339(), "2000-02-29T00:00:00Z");
/// ```
#[non_exhaustive]
pub struct RunContext {
    /// Run identity, carried on every report and event.
    pub(crate) name: String,
    /// The run's seed: host-drawn, the source of the untrusted-envelope
    /// nonce (and of every future in-run random choice).
    pub(crate) seed: u64,
    /// The behavior flags the run records; empty until an engine change
    /// gates itself behind one.
    pub(crate) flags: Flags,
    /// When the run began, as the host stamped it: rendered as `sys.when`
    /// in every section, the H1 pass included.
    pub(crate) started_at: Timestamp,
    /// Model-orchestrated prompt-tool nesting depth: 0 for a root run.
    /// Always 0 today - the sub-run adapter that increments it lands with
    /// the deferred prompt-pack.
    pub(crate) depth: u32,
    pub(crate) observer: Arc<dyn Observer>,
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    pub(crate) client: Option<GatewayClient>,
    /// The run's cancel flag: minted once at construction, replaced by
    /// [`cancel`](RunContext::cancel), and shared from here by the
    /// activated capabilities, every section VM's instruction hook, and
    /// the run's own `cancel`, so one flag reaches them all.
    pub(crate) cancel: CancelHandle,
    pub(crate) limits: RunLimits,
    pub(crate) input: Option<Arc<dyn InputBroker>>,
    /// The host-state snapshot the `ui()` global serves, taken by the host
    /// at run start; its presence is the Agent-window context.
    pub(crate) ui: Option<serde_json::Value>,
    pub(crate) on_delta: Option<Arc<dyn Fn(StreamDelta) + Send + Sync>>,
    pub(crate) vfs: VfsRef,
    /// The run's current model: the host's selection (in Workshop, the
    /// dropdown), set before prepare. Input to prepare's fill function,
    /// which binds every declared role to it. Grows into a catalog or
    /// policy in the deferred multi-model future - a field change, never
    /// a signature change.
    pub(crate) model: Option<ModelDescriptor>,
    /// The run's model satisfaction, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function: which concrete model each declared role is bound to.
    pub(crate) model_bindings: ModelBindings,
    /// The run's assembled tool catalog: the activated capabilities'
    /// contributed tools in declaration order, with tool
    /// prefix-containment enforced at assembly. Written by
    /// [`Environment::prepare`](super::Environment::prepare); the
    /// slot-filling step fills the prompt's tool slots against it.
    pub(crate) tools: ToolCatalog,
    /// The run's tool bindings, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s slot
    /// fill against the assembled catalog: which concrete tool each
    /// declared alias is bound to, with every fill journaled.
    pub(crate) tool_bindings: ToolBindings,
}

impl RunContext {
    /// Builds a context for the run `name` under the host's `seed` and
    /// `started_at`, with default observer, no client, no capture, a fresh
    /// cancel flag, no input broker, no `ui` snapshot, no delta callback,
    /// default [`RunLimits`], empty [`Flags`], and the stock store handle
    /// (`promptforge_vfs::empty()`).
    ///
    /// `seed` is the source of the untrusted-envelope nonce, so a live host
    /// draws it from a CSPRNG (a predictable seed is a guessable nonce);
    /// `started_at` is the instant every section reads as `sys.when`. The
    /// engine reads neither the OS RNG nor the clock: both are the host's,
    /// recorded by a harness and handed back verbatim by a replay.
    #[must_use]
    pub fn new(name: impl Into<String>, seed: u64, started_at: Timestamp) -> RunContext {
        RunContext {
            name: name.into(),
            seed,
            flags: Flags::EMPTY,
            started_at,
            depth: 0,
            observer: Arc::new(NullObserver::default()),
            debug: None,
            client: None,
            cancel: CancelHandle::new(),
            limits: RunLimits::new(),
            input: None,
            ui: None,
            on_delta: None,
            vfs: promptforge_vfs::empty(),
            model: None,
            model_bindings: ModelBindings::default(),
            tools: ToolCatalog::default(),
            tool_bindings: ToolBindings::default(),
        }
    }

    /// Sets the progress observer, retained for the whole run and its infer hook.
    #[must_use]
    pub fn observer(mut self, observer: Arc<dyn Observer>) -> RunContext {
        self.observer = observer;
        self
    }

    /// Sets the opt-in raw request/response capture sink.
    #[must_use]
    pub fn debug(mut self, debug: Arc<dyn DebugCapture>) -> RunContext {
        self.debug = Some(debug);
        self
    }

    /// Sets the gateway client, overriding the
    /// [`Environment`](super::Environment)'s; `None` builds one from the
    /// process environment on first use.
    #[must_use]
    pub fn client(mut self, client: GatewayClient) -> RunContext {
        self.client = Some(client);
        self
    }

    /// Sets the run's cancellation flag: the synchronous
    /// [`CancelHandle`](promptforge_api_types::cancel::sync::CancelHandle)
    /// the engine polls between chain steps and from the Lua instruction
    /// hook, and the one the activated capabilities are handed. A host
    /// that cancels through an awaitable token bridges it to this flag
    /// (set the flag when the token fires). Replaces the flag minted at
    /// construction, so it must be set before
    /// [`Environment::prepare`](super::Environment::prepare) hands the
    /// flag to the capabilities.
    #[must_use]
    pub fn cancel(mut self, handle: CancelHandle) -> RunContext {
        self.cancel = handle;
        self
    }

    /// Sets the resource limits honored across the run.
    #[must_use]
    pub fn limits(mut self, limits: RunLimits) -> RunContext {
        self.limits = limits;
        self
    }

    /// Sets the run's input broker, the host policy behind `user_input()`
    /// and the model-visible input tool. The default (`None`) is the
    /// unavailable-fallback policy: every input request resolves to
    /// [`INPUT_UNAVAILABLE_FALLBACK`](crate::input::INPUT_UNAVAILABLE_FALLBACK)
    /// with `available` false.
    #[must_use]
    pub fn input_broker(mut self, broker: Arc<dyn InputBroker>) -> RunContext {
        self.input = Some(broker);
        self
    }

    /// Sets the run's host-state snapshot and, with it, the Agent-window
    /// context: section VMs gain a `ui()` global serving this snapshot
    /// (taken by the host at run start, so a host-state change takes
    /// effect on the next run), and `models.get` resolves an undeclared
    /// alias as a raw gateway catalog model id, so the Workshop Agent
    /// window can run `models.loop(models.get(ui().selected_model), ...)`
    /// without declaring its model. The default (`None`) installs no `ui`
    /// global and keeps strict declared-alias resolution.
    #[must_use]
    pub fn ui(mut self, snapshot: serde_json::Value) -> RunContext {
        self.ui = Some(snapshot);
        self
    }

    /// Sets the behavior flags the run records. Empty is the only value
    /// this engine produces; a replay hands back the recorded set.
    #[must_use]
    pub fn flags(mut self, flags: Flags) -> RunContext {
        self.flags = flags;
        self
    }

    /// Sets the live streaming-delta callback that `models.loop` rounds
    /// forward their chunks to. The default (`None`) drops deltas at the
    /// leaf.
    #[must_use]
    pub fn on_delta(mut self, hook: Arc<dyn Fn(StreamDelta) + Send + Sync>) -> RunContext {
        self.on_delta = Some(hook);
        self
    }

    /// Sets the run's current model: the host's selection (in Workshop,
    /// the dropdown). Input to
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function, which binds every declared role to it and checks the
    /// roles' hard keywords and context minimums against its descriptor.
    /// The default (`None`) fills nothing: declared roles stay unbound and
    /// selecting one at run time fails.
    #[must_use]
    pub fn model(mut self, model: ModelDescriptor) -> RunContext {
        self.model = Some(model);
        self
    }

    /// Sets the run's VFS handle, which carries the store mount every
    /// section's `store` table operates on. The default is the stock
    /// handle (`promptforge_vfs::empty()`), a fresh memory backend at the
    /// store mount.
    ///
    /// [`Environment::prepare`](super::Environment::prepare) - and so
    /// [`Environment::run`](super::Environment::run) - replaces this
    /// handle unconditionally with the per-run router (the shared base
    /// mounted at `/` plus the run's fresh store), so a handle set here
    /// is discarded on the zero-burden path. Hosts that seed before the
    /// run or extract after it go through the prepared handle
    /// ([`vfs_handle`](RunContext::vfs_handle)) instead.
    #[must_use]
    pub fn vfs(mut self, vfs: VfsRef) -> RunContext {
        self.vfs = vfs;
        self
    }

    /// Returns the run's VFS handle. After
    /// [`Environment::prepare`](super::Environment::prepare) this is the
    /// per-run router - the shared base mounted at `/` plus the run's
    /// fresh store - and hosts extract run output through it.
    ///
    /// Named `vfs_handle` because the builder half already owns
    /// [`vfs`](RunContext::vfs).
    #[must_use]
    pub fn vfs_handle(&self) -> &VfsRef {
        &self.vfs
    }

    /// Returns the run's current model, when the host set one.
    #[must_use]
    pub fn current_model(&self) -> Option<&ModelDescriptor> {
        self.model.as_ref()
    }

    /// Returns the run's model satisfaction, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function: which concrete model each declared role is bound to, and
    /// the descriptors this run may use. Handles resolve
    /// label -> id -> descriptor.
    #[must_use]
    pub fn model_bindings(&self) -> &ModelBindings {
        &self.model_bindings
    }

    /// Returns the run's assembled tool catalog, written by
    /// [`Environment::prepare`](super::Environment::prepare) from the
    /// activated capabilities' contributions in declaration order. Empty
    /// on a caller-built context that was never prepared.
    #[must_use]
    pub fn tools(&self) -> &ToolCatalog {
        &self.tools
    }

    /// Returns the run's tool bindings, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s slot
    /// fill: which concrete tool each declared alias is bound to, with
    /// every fill journaled. Handles resolve alias -> id -> tool.
    /// Empty on a caller-built context that was never prepared.
    #[must_use]
    pub fn tool_bindings(&self) -> &ToolBindings {
        &self.tool_bindings
    }

    /// Returns the run identity shared by every report.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the run's seed, as the host drew it.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Returns the behavior flags the run records. Named `run_flags`
    /// because the builder half already owns [`flags`](RunContext::flags).
    #[must_use]
    pub fn run_flags(&self) -> Flags {
        self.flags
    }

    /// Returns when the run began, as the host stamped it.
    #[must_use]
    pub fn started_at(&self) -> Timestamp {
        self.started_at
    }

    /// Returns the model-orchestrated prompt-tool nesting depth (always 0
    /// for a root run; the sub-run adapter that increments it lands with
    /// the deferred prompt-pack).
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.depth
    }
}

impl fmt::Debug for RunContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunContext")
            .field("name", &self.name)
            .field("seed", &self.seed)
            .field("flags", &self.flags)
            .field("started_at", &self.started_at)
            .field("depth", &self.depth)
            .field("observer", &"<dyn Observer>")
            .field("client", &self.client)
            .field("debug", &self.debug.as_ref().map(|_| "<dyn DebugCapture>"))
            .field("cancel", &self.cancel)
            .field("limits", &self.limits)
            .field("input", &self.input.is_some())
            .field("ui", &self.ui)
            .field("on_delta", &self.on_delta.is_some())
            .field("vfs", &self.vfs)
            .field("model", &self.model)
            .field("model_bindings", &self.model_bindings)
            .field("tools", &self.tools)
            .field("tool_bindings", &self.tool_bindings)
            .finish()
    }
}
