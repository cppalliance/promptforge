//! Per-run context and resource limits: [`RunContext`] and [`RunLimits`].

use std::fmt;

#[path = "config-limits.rs"]
mod limits;

use promptforge_types::emitter::DebugMode;
use promptforge_types::replay::Flags;
use promptforge_types::timestamp::Timestamp;

pub use limits::RunLimits;

use crate::cancel::CancelHandle;
use crate::model::ModelDescriptor;
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::bindings::{ModelBindings, ToolBindings};

/// One run. Created by the host from the
/// [`Environment`](super::Environment) holding the per-run inputs,
/// enriched at prepare, owned by the engine for the run. Never shared
/// between runs.
///
/// The context is the engine's input and nothing else: it holds no
/// observer, client, tool implementation, broker, or capture. Those are
/// the host's; the engine reports events and issues effects as values and
/// never reaches for a host seam.
///
/// The engine takes its clock and randomness from the host: the run's
/// `seed` and `started_at` are inputs the host supplies to
/// [`new`](RunContext::new) (a harness draws both, records both, and a
/// replay hands back the recorded values), so given the same inputs and
/// the same answers a run reproduces its nonces, `sys.when`, effects, and
/// events. There is no default for either: a host that runs once draws
/// the seed from its own CSPRNG and stamps its own clock.
///
/// # Examples
/// ```
/// use promptforge::timestamp::Timestamp;
/// use promptforge::{RunContext, RunLimits};
///
/// let ctx = RunContext::new("example-run", 7, Timestamp::from_unix_millis(951_782_400_000))
///     .limits(RunLimits::new());
/// assert_eq!(ctx.name(), "example-run");
/// assert_eq!(ctx.seed(), 7);
/// assert_eq!(ctx.started_at().to_rfc3339(), "2000-02-29T00:00:00Z");
/// ```
#[non_exhaustive]
pub struct RunContext {
    /// Run identity, stamped on every report and event.
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
    /// Where the root task's provenance sequence starts: 0 by default. A
    /// host that logged the prompt's parse events (stamped under task `0`
    /// from zero) ahead of the run passes their count, so the run's root
    /// task continues the sequence and `(task, seq)` is unique across the
    /// parse/run boundary.
    pub(crate) provenance_start: u32,
    /// Model-orchestrated prompt-tool nesting depth: 0 for a root run.
    /// Always 0 today - the sub-run adapter that increments it lands with
    /// the deferred prompt-pack.
    pub(crate) depth: u32,
    /// Whether the run reports each model round's raw request and response
    /// bodies as `Request` and `Response` events. Off by default: the
    /// bodies already travel in the `Chat` effect and its answer, so a host
    /// that logs effects has them, and the events are for a host that
    /// wants the pair in the event stream too.
    pub(crate) report_debug: DebugMode,
    /// The run's cancel flag: minted once at construction, replaced by
    /// [`cancel`](RunContext::cancel), and shared from here by every
    /// section VM's instruction hook and the run's own `cancel`, so one
    /// flag reaches them all; the host hands the same flag to the
    /// capabilities it activates.
    pub(crate) cancel: CancelHandle,
    pub(crate) limits: RunLimits,
    /// The host-state snapshot the `ui()` global serves, taken by the host
    /// at run start; its presence also turns on the raw-model-id
    /// `models.get` fallback.
    pub(crate) ui: Option<serde_json::Value>,
    pub(crate) vfs: VfsRef,
    /// Whether the host set `vfs` itself ([`vfs`](RunContext::vfs)), in
    /// which case prepare keeps it rather than building the per-run router.
    pub(crate) vfs_explicit: bool,
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
    // No test-only host field: the in-crate suites assemble a
    // `RunHost` themselves and pass it to the tokio driver, so this
    // production struct carries only the engine's inputs.
}

impl RunContext {
    /// Builds a context for the run `name` under the host's `seed` and
    /// `started_at`, with a fresh cancel flag, no `ui` snapshot, no debug
    /// reporting, default [`RunLimits`], empty [`Flags`], and the stock
    /// store handle, a fresh memory backend at the store mount.
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
            provenance_start: 0,
            depth: 0,
            report_debug: DebugMode::Off,
            cancel: CancelHandle::new(),
            limits: RunLimits::new(),
            ui: None,
            vfs: promptforge_vfs::empty(),
            vfs_explicit: false,
            model: None,
            model_bindings: ModelBindings::default(),
            tools: ToolCatalog::default(),
            tool_bindings: ToolBindings::default(),
        }
    }

    /// Sets whether the run reports each model round's raw request and
    /// response bodies as `Request` and `Response` events. The default
    /// ([`DebugMode::Off`]) reports neither; a host that wants the pair in
    /// the event stream (a debug capture) passes [`DebugMode::On`].
    #[must_use]
    pub fn report_debug(mut self, mode: DebugMode) -> RunContext {
        self.report_debug = mode;
        self
    }

    /// Sets the run's cancellation flag: the synchronous [`CancelHandle`]
    /// the engine polls between chain steps and from the Lua instruction
    /// hook. A host that cancels through an awaitable token bridges it to
    /// this flag (set the flag when the token fires), and hands the same
    /// flag to the capabilities it activates so one cancel reaches them
    /// all. Replaces the flag minted at construction.
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

    /// Sets the run's host-state snapshot. Section VMs gain a `ui()` global
    /// serving this snapshot (taken by the host at run start, so a
    /// host-state change takes effect on the next run), and `models.get`
    /// resolves an undeclared alias as a raw gateway catalog model id, so a
    /// prompt can run `models.loop(models.get(ui().selected_model), ...)`
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

    /// Sets where the root task's provenance sequence starts. The default
    /// (0) is a run recorded on its own. A host that records the prompt's
    /// parse events ahead of the run in one stream passes their count:
    /// `Prompt::parse` stamps them under task `0` from zero, and this seeds
    /// the run's root counter past them so every `(task, seq)` in the
    /// stream is unique. Spawned tasks are unaffected and count from zero.
    #[must_use]
    pub fn provenance_start(mut self, start: u32) -> RunContext {
        self.provenance_start = start;
        self
    }

    /// Sets the run's current model: the host's selection (in Workshop,
    /// the dropdown). Input to
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function, which binds every declared role to it and checks the
    /// roles' hard keywords and context minimums against its descriptor.
    /// With the default (`None`), declared roles stay unbound and
    /// selecting one at run time fails.
    #[must_use]
    pub fn model(mut self, model: ModelDescriptor) -> RunContext {
        self.model = Some(model);
        self
    }

    /// Sets the run's VFS handle, which holds the store mount every
    /// section's `store` table operates on. The default is the stock
    /// handle, a fresh memory backend at the store mount.
    ///
    /// A handle set here is the host's: [`Environment::prepare`]
    /// keeps it rather than building the per-run router, so a host that
    /// activates capabilities before prepare builds the run's router
    /// first ([`Environment::run_vfs`]), hands it to
    /// activation's services and to this builder, and the capabilities
    /// and the run share one store. Without it, prepare builds the router
    /// (the shared base mounted at `/` plus the run's fresh store) and
    /// hosts that seed before the run or extract after it go through the
    /// prepared handle ([`vfs_handle`](RunContext::vfs_handle)).
    ///
    /// [`Environment::prepare`]: super::Environment::prepare
    /// [`Environment::run_vfs`]: super::Environment::run_vfs
    #[must_use]
    pub fn vfs(mut self, vfs: VfsRef) -> RunContext {
        self.vfs = vfs;
        self.vfs_explicit = true;
        self
    }

    /// Returns the run's VFS handle. After
    /// [`Environment::prepare`](super::Environment::prepare) this is the
    /// per-run router - the shared base mounted at `/` plus the run's
    /// fresh store, or the handle the host set - and hosts extract run
    /// output through it.
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

    /// Returns the run's cancel flag: the handle the host hands to the
    /// capabilities it activates so one cancel reaches them and the run.
    /// Named `cancel_handle` because the builder half already owns
    /// [`cancel`](RunContext::cancel).
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
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
        let mut state = f.debug_struct("RunContext");
        state
            .field("name", &self.name)
            .field("seed", &self.seed)
            .field("flags", &self.flags)
            .field("started_at", &self.started_at)
            .field("provenance_start", &self.provenance_start)
            .field("depth", &self.depth)
            .field("report_debug", &self.report_debug)
            .field("cancel", &self.cancel)
            .field("limits", &self.limits)
            .field("ui", &self.ui)
            .field("vfs", &self.vfs)
            .field("vfs_explicit", &self.vfs_explicit)
            .field("model", &self.model)
            .field("model_bindings", &self.model_bindings)
            .field("tools", &self.tools)
            .field("tool_bindings", &self.tool_bindings)
            .finish()
    }
}
