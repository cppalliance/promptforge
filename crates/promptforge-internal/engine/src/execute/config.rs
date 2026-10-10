//! Per-run context and resource limits: [`RunContext`] and [`RunLimits`].

use std::fmt;

#[path = "config-limits.rs"]
mod limits;

use promptforge_types::emitter::DebugMode;
use promptforge_types::plugins::Prelude;
use promptforge_types::replay::Flags;
use promptforge_types::timestamp::Timestamp;

pub use limits::RunLimits;

use crate::cancel::CancelHandle;
use crate::model::ModelDescriptor;
use crate::tools::ToolCatalog;
use promptforge_vfs::VfsRef;

use super::bindings::ModelBindings;

/// The inputs for one run of a prompt.
///
/// The caller builds a context with `new` and the builder methods. The
/// prepare step of an [`Environment`](super::Environment) then adds the
/// tool catalog, the Plugin preludes, and the model bindings. The Engine
/// owns the context for the run. Each context belongs to exactly one run.
///
/// The context holds only the Engine's input. The observer, client, tool
/// implementations, broker, and capture belong to the caller. The Engine
/// reports events and issues effects as values, and returns each one to
/// the caller from `step`.
///
/// The caller supplies the run's clock and randomness: it passes the
/// run's `seed` and `started_at` to [`new`](RunContext::new), and both
/// are required. For a live run, the caller must draw the seed from a
/// CSPRNG and read the start time from its own clock. The caller records both
/// values, and a replay passes back the recorded ones. Given the same
/// inputs and the same answers, a run reproduces its nonces, `sys.when`,
/// effects, and events.
#[non_exhaustive]
pub struct RunContext {
    /// Run identity, stamped on every report and event.
    pub(super) name: String,
    /// The run's seed, as the caller supplied it: the source of the
    /// untrusted-envelope nonce (and of every other in-run random choice).
    pub(super) seed: u64,
    /// The behavior flags the run records; empty by default.
    flags: Flags,
    /// When the run began, as the caller stamped it: rendered as `sys.when`
    /// in every section, the H1 pass included.
    pub(super) started_at: Timestamp,
    /// Where the root task's provenance sequence starts: 0 by default. When
    /// the caller puts the prompt's parse events (stamped under task `0`
    /// from zero) in the same stream ahead of the run, it passes their
    /// count, so the run's root task continues the sequence and
    /// `(task, seq)` is unique across the parse/run boundary.
    pub(super) provenance_start: u32,
    /// Model-orchestrated prompt-tool nesting depth: 0 for a root run.
    /// Nothing increments it, so it is always 0.
    depth: u32,
    /// Whether the run reports each model round's raw request and response
    /// bodies as `Request` and `Response` events. Off by default: the
    /// bodies already travel in the `Chat` effect and its answer, and the
    /// events serve a caller that wants the pair in the event stream too.
    pub(super) report_debug: DebugMode,
    /// The run's cancel flag: minted once at construction, replaced by
    /// [`cancel`](RunContext::cancel), and shared from here by every
    /// section VM's instruction hook and the run's own `cancel`, so one
    /// flag reaches them all; the caller watches the same flag to drop the
    /// effects it has in flight.
    pub(super) cancel: CancelHandle,
    pub(crate) limits: RunLimits,
    /// The application-state snapshot the `ui()` global serves, taken by
    /// the caller at run start; its presence also turns on the raw-model-id
    /// `models.get` fallback.
    pub(super) ui: Option<serde_json::Value>,
    /// The run's whole filesystem: real directories and the declared
    /// store. The default is a fresh memory store at `/`; the caller
    /// replaces it with [`vfs`](RunContext::vfs).
    pub(super) vfs: VfsRef,
    /// The run's current model, which the caller chooses, set before
    /// prepare. Input to prepare's fill function, which binds every
    /// declared role to it.
    pub(super) model: Option<ModelDescriptor>,
    /// The run's model satisfaction, written by
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function: which concrete model each declared role is bound to.
    pub(super) model_bindings: ModelBindings,
    /// The run's tool catalog: every tool of every Plugin the caller can
    /// serve, declared by the prompt or not, with each tool under its
    /// Plugin's name. Written by
    /// [`Environment::prepare`](super::Environment::prepare); the run's
    /// offering draws every tool from it.
    pub(super) tools: ToolCatalog,
    /// The run's Plugin preludes, in declaration order. Written by
    /// [`Environment::prepare`](super::Environment::prepare) from the
    /// environment's list; every section VM installs each one before the
    /// shared library replays. Empty on a caller-built context that was
    /// never prepared.
    pub(super) preludes: Vec<Prelude>,
}

impl RunContext {
    /// Builds a context for a run named `name`, with the given `seed` and
    /// `started_at`.
    ///
    /// The new context has a fresh cancel flag, a `ui` snapshot of `None`,
    /// debug reporting set to `DebugMode::Off`, default [`RunLimits`], empty
    /// [`Flags`], and the default filesystem, which is a fresh memory store
    /// at `/`.
    ///
    /// `seed` is the source of the untrusted-envelope nonce. For a live
    /// run, the caller must draw it from a CSPRNG, because a predictable
    /// seed makes a guessable nonce. `started_at` is the instant every
    /// section reads as `sys.when`. The caller records both values, and a
    /// replay passes them back verbatim.
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
            preludes: Vec::new(),
        }
    }

    /// Sets whether the run reports each model round's raw request and
    /// response bodies as `Request` and `Response` events.
    ///
    /// The default, [`DebugMode::Off`], turns both events off. A caller that
    /// wants the pair in the event stream, such as for a debug capture,
    /// passes [`DebugMode::On`].
    #[must_use]
    pub fn report_debug(mut self, mode: DebugMode) -> RunContext {
        self.report_debug = mode;
        self
    }

    /// Sets the run's cancel flag, replacing the one that `new` created.
    ///
    /// The flag is a synchronous [`CancelHandle`] that the Engine polls
    /// between chain steps and from the Lua instruction hook. A caller that
    /// cancels through an awaitable token must bridge the token to this
    /// flag by setting the flag when the token fires.
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

    /// Sets the snapshot of application state that prompts read through the
    /// `ui()` global.
    ///
    /// With a snapshot set, every section VM gets a `ui()` global that
    /// returns it. The caller takes the snapshot at run start, so a change
    /// to application state takes effect on the next run. A snapshot also
    /// makes `models.get` resolve an alias outside the prompt's declarations
    /// as a raw model id. So a prompt can run
    /// `models.get(ui().selected_model):loop(...)` and skip
    /// declaring its model. The default is `None`, which leaves the `ui`
    /// global absent and limits `models.get` to declared aliases.
    #[must_use]
    pub fn ui(mut self, snapshot: serde_json::Value) -> RunContext {
        self.ui = Some(snapshot);
        self
    }

    /// Sets the behavior flags the run records.
    ///
    /// A live run keeps the default, the empty set. A replay passes back the
    /// set the original run recorded.
    #[must_use]
    pub fn flags(mut self, flags: Flags) -> RunContext {
        self.flags = flags;
        self
    }

    /// Sets where the root task's provenance sequence starts.
    ///
    /// The default, 0, fits a run recorded on its own. When the caller
    /// records the prompt's parse events ahead of the run in one stream, it
    /// passes their count. `Prompt::parse` stamps those events under task
    /// `0`, counting from zero, so this start moves the run's root counter
    /// past them and every `(task, seq)` in the stream stays unique.
    /// Spawned tasks count from zero.
    #[must_use]
    pub fn provenance_start(mut self, start: u32) -> RunContext {
        self.provenance_start = start;
        self
    }

    /// Sets the run's current model, which the caller chooses.
    ///
    /// [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function binds every declared role to this model. It also checks
    /// each role's hard keywords and context minimum against the model's
    /// descriptor. With the default, `None`, the model bindings stay empty,
    /// and selecting a declared role at run time fails.
    #[must_use]
    pub fn model(mut self, model: ModelDescriptor) -> RunContext {
        self.model = Some(model);
        self
    }

    /// Sets the run's VFS handle, which is the run's whole filesystem.
    ///
    /// The filesystem holds the real directories and the declared store
    /// that every section's `store` table operates on. The default is a
    /// fresh memory store at `/`.
    ///
    /// [`Environment::prepare`](super::Environment::prepare) keeps a handle
    /// set here as given. Every tool call effect carries an access to this
    /// same filesystem, so the tools and the run share one set of files.
    /// The caller seeds files before the run and extracts output after it
    /// through the prepared handle, returned by
    /// [`vfs_handle`](RunContext::vfs_handle).
    #[must_use]
    pub fn vfs(mut self, vfs: VfsRef) -> RunContext {
        self.vfs = vfs;
        self
    }

    /// Returns the run's VFS handle, which is the run's whole filesystem.
    ///
    /// The filesystem includes the real directories and the declared
    /// store. The caller seeds files through this handle before the run and
    /// extracts output through it after the run. Set the handle with
    /// [`vfs`](RunContext::vfs).
    #[must_use]
    pub fn vfs_handle(&self) -> &VfsRef {
        &self.vfs
    }

    /// Returns the run's current model, or `None` by default.
    #[must_use]
    pub fn current_model(&self) -> Option<&ModelDescriptor> {
        self.model.as_ref()
    }

    /// Returns the run's cancel flag.
    ///
    /// The caller watches this flag, so one cancel reaches the run and the
    /// effects the caller has in flight. Replace the flag with
    /// [`cancel`](RunContext::cancel).
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.clone()
    }

    /// Returns the run's model bindings, which map each declared role to a
    /// concrete model.
    ///
    /// The bindings also hold the descriptors of the models this run may
    /// use. [`Environment::prepare`](super::Environment::prepare)'s fill
    /// function writes them. A lookup goes from a role's label to a model
    /// id, and from the id to the model's descriptor.
    #[must_use]
    pub fn model_bindings(&self) -> &ModelBindings {
        &self.model_bindings
    }

    /// Returns the run's tool catalog.
    ///
    /// [`Environment::prepare`](super::Environment::prepare) copies the
    /// environment's catalog here: every tool of every Plugin the caller
    /// can serve, declared by the prompt or not. On a caller-built context,
    /// the catalog is empty until prepare runs.
    #[must_use]
    pub fn tools(&self) -> &ToolCatalog {
        &self.tools
    }

    /// Returns the run's name, which identifies the run on every report and
    /// event.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the run's seed, as the caller supplied it.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Returns the behavior flags the run records. Set them with
    /// [`flags`](RunContext::flags).
    #[must_use]
    pub fn run_flags(&self) -> Flags {
        self.flags
    }

    /// Returns when the run began, as the caller stamped it.
    #[must_use]
    pub fn started_at(&self) -> Timestamp {
        self.started_at
    }

    /// Returns how many model-orchestrated prompt tools this run is nested
    /// inside.
    ///
    /// The depth is 0 for a root run.
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
            .field(
                "preludes",
                &self
                    .preludes
                    .iter()
                    .map(Prelude::plugin)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
