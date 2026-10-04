//! Per-run context and resource limits: [`RunContext`] and [`RunLimits`].

use std::fmt;

#[path = "config-limits.rs"]
mod limits;

use promptforge_types::capabilities::Prelude;
use promptforge_types::emitter::DebugMode;
use promptforge_types::replay::Flags;
use promptforge_types::timestamp::Timestamp;

pub use limits::RunLimits;

use crate::cancel::CancelHandle;
use crate::model::ModelDescriptor;
use crate::tools::ToolCatalog;
use promptforge_vfs::VfsRef;

use super::bindings::{ModelBindings, ToolBindings};

/// One run. Created by the Harness from the
/// [`Environment`](super::Environment) holding the per-run inputs,
/// enriched at prepare, owned by the Engine for the run. Never shared
/// between runs.
///
/// The context is the Engine's input and nothing else: it holds no
/// observer, client, tool implementation, broker, or capture. Those are
/// the Harness's; the Engine reports events and issues effects as values
/// and hands each one to the Harness through `step`.
///
/// The Engine takes its clock and randomness from the Harness: the run's
/// `seed` and `started_at` are inputs the Harness supplies to
/// [`new`](RunContext::new) (a Harness draws both, records both, and a
/// replay hands back the recorded values), so given the same inputs and
/// the same answers a run reproduces its nonces, `sys.when`, effects, and
/// events. Both are required: for a live run the Harness draws
/// the seed from its own CSPRNG and stamps its own clock.
#[non_exhaustive]
pub struct RunContext {
    /// Run identity, stamped on every report and event.
    pub(super) name: String,
    /// The run's seed: Harness-drawn, the source of the untrusted-envelope
    /// nonce (and of every future in-run random choice).
    pub(super) seed: u64,
    /// The behavior flags the run records; empty until an Engine change
    /// gates itself behind one.
    flags: Flags,
    /// When the run began, as the Harness stamped it: rendered as `sys.when`
    /// in every section, the H1 pass included.
    pub(super) started_at: Timestamp,
    /// Where the root task's provenance sequence starts: 0 by default. When
    /// the Harness logged the prompt's parse events (stamped under task `0`
    /// from zero) ahead of the run, it passes their count, so the run's root
    /// task continues the sequence and `(task, seq)` is unique across the
    /// parse/run boundary.
    pub(super) provenance_start: u32,
    /// Model-orchestrated prompt-tool nesting depth: 0 for a root run.
    /// Always 0 today - the sub-run adapter that increments it lands with
    /// the deferred prompt-pack.
    depth: u32,
    /// Whether the run reports each model round's raw request and response
    /// bodies as `Request` and `Response` events. Off by default: the
    /// bodies already travel in the `Chat` effect and its answer, so the
    /// Harness's effect log has them, and the events serve a Harness that
    /// wants the pair in the event stream too.
    pub(super) report_debug: DebugMode,
    /// The run's cancel flag: minted once at construction, replaced by
    /// [`cancel`](RunContext::cancel), and shared from here by every
    /// section VM's instruction hook and the run's own `cancel`, so one
    /// flag reaches them all; the Harness hands the same flag to the
    /// capabilities it activates.
    pub(super) cancel: CancelHandle,
    pub(crate) limits: RunLimits,
    /// The Host-state snapshot the `ui()` global serves, taken by the Host
    /// at run start; its presence also turns on the raw-model-id
    /// `models.get` fallback.
    pub(super) ui: Option<serde_json::Value>,
    /// The run's whole filesystem: real directories and the declared
    /// store. The default is a fresh memory store at `/`; the Harness
    /// mounts the run's real directories and declared store and sets the
    /// handle with [`vfs`](RunContext::vfs).
    pub(super) vfs: VfsRef,
    /// The run's current model: the Host's selection (in Workshop, the
    /// dropdown), set before prepare. Input to prepare's fill function,
    /// which binds every declared role to it. Grows into a catalog or
    /// policy in the deferred multi-model future - a field change, never
    /// a signature change.
    pub(super) model: Option<ModelDescriptor>,
    /// The run's model satisfaction, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function: which concrete model each declared role is bound to.
    pub(super) model_bindings: ModelBindings,
    /// The run's assembled tool catalog: the activated capabilities'
    /// contributed tools in declaration order, with tool
    /// prefix-containment enforced at assembly. Written by
    /// [`Environment::prepare`](super::Environment::prepare); the
    /// slot-filling step fills the prompt's tool slots against it.
    pub(super) tools: ToolCatalog,
    /// The run's tool bindings, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s slot
    /// fill against the assembled catalog: which concrete tool each
    /// declared alias is bound to, with every fill journaled.
    pub(super) tool_bindings: ToolBindings,
    /// The run's capability preludes, in install order. Written by
    /// [`Environment::prepare`](super::Environment::prepare) from the
    /// environment's list; every section VM installs each one before the
    /// shared library replays. Empty on a caller-built context that was
    /// never prepared.
    pub(super) preludes: Vec<Prelude>,
}

impl RunContext {
    /// Builds a context for the run `name` under the Harness's `seed` and
    /// `started_at`, with a fresh cancel flag, no `ui` snapshot, no debug
    /// reporting, default [`RunLimits`], empty [`Flags`], and the default
    /// filesystem, a fresh memory store at `/`.
    ///
    /// `seed` is the source of the untrusted-envelope nonce, so for a live
    /// run the Harness draws it from a CSPRNG (a predictable seed is a
    /// guessable nonce); `started_at` is the instant every section reads as
    /// `sys.when`. The Engine takes both from its caller: the Harness draws
    /// and records them, and a replay hands them back verbatim.
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
            vfs: VfsRef::default(),
            model: None,
            model_bindings: ModelBindings::default(),
            tools: ToolCatalog::default(),
            tool_bindings: ToolBindings::default(),
            preludes: Vec::new(),
        }
    }

    /// Sets whether the run reports each model round's raw request and
    /// response bodies as `Request` and `Response` events. The default
    /// ([`DebugMode::Off`]) reports neither; when the Harness wants the pair in
    /// the event stream (a debug capture), it passes [`DebugMode::On`].
    #[must_use]
    pub fn report_debug(mut self, mode: DebugMode) -> RunContext {
        self.report_debug = mode;
        self
    }

    /// Sets the run's cancellation flag: the synchronous [`CancelHandle`]
    /// the Engine polls between chain steps and from the Lua instruction
    /// hook. A Harness that cancels through an awaitable token bridges it to
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

    /// Sets the run's Host-state snapshot. Section VMs gain a `ui()` global
    /// serving this snapshot (taken by the Host at run start, so a
    /// Host-state change takes effect on the next run), and `models.get`
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
    /// this Engine version produces; a replay hands back the recorded set.
    #[must_use]
    pub fn flags(mut self, flags: Flags) -> RunContext {
        self.flags = flags;
        self
    }

    /// Sets where the root task's provenance sequence starts. The default
    /// (0) is a run recorded on its own. When the Harness records the prompt's
    /// parse events ahead of the run in one stream, it passes their count:
    /// `Prompt::parse` stamps them under task `0` from zero, and this seeds
    /// the run's root counter past them so every `(task, seq)` in the
    /// stream is unique. Spawned tasks are unaffected and count from zero.
    #[must_use]
    pub fn provenance_start(mut self, start: u32) -> RunContext {
        self.provenance_start = start;
        self
    }

    /// Sets the run's current model: the Host's selection (in Workshop,
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

    /// Sets the run's VFS handle, the run's whole filesystem: the real
    /// directories and the declared store every section's `store` table
    /// operates on. The default is a fresh memory store at `/`.
    ///
    /// A handle set here is the Harness's, used as given by
    /// [`Environment::prepare`](super::Environment::prepare) - the
    /// environment never replaces it - so the Harness, when it activates
    /// capabilities, builds the run's handle first, hands it to
    /// activation's services and to this builder, and the capabilities
    /// and the run share one filesystem. The Harness seeds files before the
    /// run and extracts output after it through the prepared handle
    /// ([`vfs_handle`](RunContext::vfs_handle)).
    #[must_use]
    pub fn vfs(mut self, vfs: VfsRef) -> RunContext {
        self.vfs = vfs;
        self
    }

    /// Returns the run's VFS handle: the run's whole filesystem, real
    /// directories and declared store included, through which the Harness
    /// seeds files before the run and extracts output after it.
    ///
    /// Named `vfs_handle` because the builder half already owns
    /// [`vfs`](RunContext::vfs).
    #[must_use]
    pub fn vfs_handle(&self) -> &VfsRef {
        &self.vfs
    }

    /// Returns the run's current model, when the Host selected one.
    #[must_use]
    pub fn current_model(&self) -> Option<&ModelDescriptor> {
        self.model.as_ref()
    }

    /// Returns the run's cancel flag: the handle the Harness hands to the
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

    /// Returns the run's seed, as the Harness drew it.
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

    /// Returns when the run began, as the Harness stamped it.
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
            .field("model", &self.model)
            .field("model_bindings", &self.model_bindings)
            .field("tools", &self.tools)
            .field("tool_bindings", &self.tool_bindings)
            .field(
                "preludes",
                &self
                    .preludes
                    .iter()
                    .map(Prelude::capability)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
