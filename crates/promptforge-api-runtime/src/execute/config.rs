//! Per-run context and resource limits: [`RunContext`] and [`RunLimits`].

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::cancel::CancelHandle;
use crate::client::{GatewayClient, StreamDelta};
use crate::debug::DebugCapture;
use crate::input::InputBroker;
use crate::model::ModelDescriptor;
use crate::observe::{NullObserver, Observer};
use crate::store::VfsRef;
use crate::tools::ToolCatalog;

use super::bindings::{ModelBindings, ToolBindings};

/// Generates one `nz_*` constructor per `NonZero*` type: a `const fn`
/// building the wrapper from a compile-time-known non-zero value.
macro_rules! nz {
    ($name:ident, $nonzero:ident, $primitive:ty) => {
        /// Builds the non-zero wrapper from a compile-time-known non-zero
        /// value.
        pub(crate) const fn $name(value: $primitive) -> $nonzero {
            match $nonzero::new(value) {
                Some(non_zero) => non_zero,
                None => unreachable!(),
            }
        }
    };
}

nz!(nz_u32, NonZeroU32, u32);
nz!(nz_u64, NonZeroU64, u64);
nz!(nz_usize, NonZeroUsize, usize);

/// Resource ceilings a run honors at its bounded sites: per-section tool
/// iterations, fanout concurrency, model response size, Lua memory, Lua log
/// volume, and the request timeout.
///
/// The defaults are safe, non-environment values so a clean build needs no
/// provisioning. Frontmatter `max_tool_iterations`, when present, still
/// overrides [`RunLimits::max_tool_iterations`] for that prompt.
///
/// # Examples
/// ```
/// use std::num::NonZeroU32;
///
/// use promptforge_api_runtime::execute::RunLimits;
///
/// let eight = NonZeroU32::new(8).ok_or("8 is non-zero")?;
/// let limits = RunLimits::new().max_tool_iterations(eight);
/// assert_eq!(limits.tool_iterations().get(), 8);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RunLimits {
    max_tool_iterations: NonZeroU32,
    fanout_concurrency: NonZeroUsize,
    max_response_bytes: NonZeroU64,
    lua_memory_bytes: NonZeroUsize,
    lua_log_events: NonZeroU32,
    request_timeout: Duration,
}

impl RunLimits {
    /// Builds the default limits (24 tool iterations, 8-way fanout, 16 MiB
    /// response cap, 64 MiB Lua memory, 1024 Lua log events, 120 s timeout).
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_runtime::execute::RunLimits;
    ///
    /// assert_eq!(RunLimits::new().tool_iterations().get(), 24);
    /// ```
    #[must_use]
    pub fn new() -> RunLimits {
        RunLimits {
            max_tool_iterations: nz_u32(24),
            fanout_concurrency: nz_usize(8),
            max_response_bytes: nz_u64(16 * 1024 * 1024),
            lua_memory_bytes: nz_usize(64 * 1024 * 1024),
            lua_log_events: nz_u32(1024),
            request_timeout: Duration::from_secs(120),
        }
    }

    /// Sets the default per-section model round-trip cap.
    #[must_use]
    pub fn max_tool_iterations(mut self, value: NonZeroU32) -> RunLimits {
        self.max_tool_iterations = value;
        self
    }

    /// Sets the maximum number of concurrent fanout arms.
    #[must_use]
    pub fn max_fanout_concurrency(mut self, value: NonZeroUsize) -> RunLimits {
        self.fanout_concurrency = value;
        self
    }

    /// Sets the maximum accepted model response body size, in bytes.
    #[must_use]
    pub fn max_response_bytes(mut self, value: NonZeroU64) -> RunLimits {
        self.max_response_bytes = value;
        self
    }

    /// Sets the per-VM Lua memory ceiling, in bytes.
    #[must_use]
    pub fn lua_memory_bytes(mut self, value: NonZeroUsize) -> RunLimits {
        self.lua_memory_bytes = value;
        self
    }

    /// Sets the maximum number of Lua author `log` checkpoints per VM.
    #[must_use]
    pub fn lua_log_events(mut self, value: NonZeroU32) -> RunLimits {
        self.lua_log_events = value;
        self
    }

    /// Sets the per-request model HTTP timeout.
    #[must_use]
    pub fn request_timeout(mut self, value: Duration) -> RunLimits {
        self.request_timeout = value;
        self
    }

    /// Returns the default per-section model round-trip cap.
    #[must_use]
    pub fn tool_iterations(&self) -> NonZeroU32 {
        self.max_tool_iterations
    }

    /// Returns the maximum number of concurrent fanout arms.
    #[must_use]
    pub fn fanout_concurrency(&self) -> NonZeroUsize {
        self.fanout_concurrency
    }

    /// Returns the maximum accepted model response body size, in bytes.
    #[must_use]
    pub fn response_bytes(&self) -> NonZeroU64 {
        self.max_response_bytes
    }

    /// Returns the per-VM Lua memory ceiling, in bytes.
    #[must_use]
    pub fn lua_memory(&self) -> NonZeroUsize {
        self.lua_memory_bytes
    }

    /// Returns the maximum number of Lua author `log` checkpoints per VM.
    #[must_use]
    pub fn lua_logs(&self) -> NonZeroU32 {
        self.lua_log_events
    }

    /// Returns the per-request model HTTP timeout.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.request_timeout
    }
}

impl Default for RunLimits {
    fn default() -> RunLimits {
        RunLimits::new()
    }
}

/// One run. Created by the host from the
/// [`Environment`](super::Environment) carrying the per-run inputs,
/// enriched at prepare, owned by the executor during
/// [`run`](super::run). Never shared between runs.
///
/// `RunContext` is owned (no borrows), so its observer and debug sinks reach
/// the nested `models.infer` path that a borrowed option could not.
///
/// # Examples
/// ```
/// use promptforge_api_runtime::execute::{RunContext, RunLimits};
///
/// let ctx = RunContext::new("example-run").limits(RunLimits::new());
/// assert_eq!(ctx.name(), "example-run");
/// ```
#[non_exhaustive]
pub struct RunContext {
    /// Run identity, carried on every report and event.
    pub(crate) name: String,
    /// When the context was created.
    pub(crate) start_time: SystemTime,
    /// Model-orchestrated prompt-tool nesting depth: 0 for a root run.
    /// Always 0 today - the sub-run adapter that increments it lands with
    /// the deferred prompt-pack.
    pub(crate) depth: u32,
    pub(crate) observer: Arc<dyn Observer>,
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    pub(crate) client: Option<GatewayClient>,
    pub(crate) cancel: Option<CancelHandle>,
    pub(crate) limits: RunLimits,
    pub(crate) input: Option<Arc<dyn InputBroker>>,
    pub(crate) ui: Option<Arc<dyn Fn() -> serde_json::Value + Send + Sync>>,
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
    /// Builds a context for the run `name` with default observer, no client,
    /// no capture, no cancellation, no input broker, no `ui` provider, no
    /// delta callback, default [`RunLimits`], and the stock store handle
    /// (`promptforge_vfs::empty()`).
    #[must_use]
    pub fn new(name: impl Into<String>) -> RunContext {
        RunContext {
            name: name.into(),
            start_time: SystemTime::now(),
            depth: 0,
            observer: Arc::new(NullObserver::default()),
            debug: None,
            client: None,
            cancel: None,
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

    /// Sets the explicit cancellation handle threaded through the run.
    #[must_use]
    pub fn cancel(mut self, handle: CancelHandle) -> RunContext {
        self.cancel = Some(handle);
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

    /// Sets the run's host-state snapshot provider and, with it, the
    /// Agent-window context: section VMs gain a `ui()` global serving a
    /// fresh snapshot per call, and `models.get` resolves an undeclared
    /// alias as a raw gateway catalog model id, so the Workshop Agent
    /// window can run `models.loop(models.get(ui().selected_model), ...)`
    /// without declaring its model. The default (`None`) installs no `ui`
    /// global and keeps strict declared-alias resolution.
    #[must_use]
    pub fn ui(mut self, provider: Arc<dyn Fn() -> serde_json::Value + Send + Sync>) -> RunContext {
        self.ui = Some(provider);
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

    /// Returns when the context was created.
    #[must_use]
    pub fn start_time(&self) -> SystemTime {
        self.start_time
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
            .field("start_time", &self.start_time)
            .field("depth", &self.depth)
            .field("observer", &"<dyn Observer>")
            .field("client", &self.client)
            .field("debug", &self.debug.as_ref().map(|_| "<dyn DebugCapture>"))
            .field("cancel", &self.cancel.is_some())
            .field("limits", &self.limits)
            .field("input", &self.input.is_some())
            .field("ui", &self.ui.is_some())
            .field("on_delta", &self.on_delta.is_some())
            .field("vfs", &self.vfs)
            .field("model", &self.model)
            .field("model_bindings", &self.model_bindings)
            .field("tools", &self.tools)
            .field("tool_bindings", &self.tool_bindings)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_limits_pins_all_six_defaults_and_the_untested_builders() {
        let defaults = RunLimits::new();
        assert_eq!(defaults.tool_iterations().get(), 24);
        assert_eq!(defaults.fanout_concurrency().get(), 8);
        assert_eq!(defaults.response_bytes().get(), 16 * 1024 * 1024);
        assert_eq!(defaults.lua_memory().get(), 64 * 1024 * 1024);
        assert_eq!(defaults.lua_logs().get(), 1024);
        assert_eq!(defaults.timeout(), Duration::from_secs(120));

        let built = RunLimits::new()
            .max_response_bytes(nz_u64(4 * 1024))
            .lua_log_events(nz_u32(7))
            .request_timeout(Duration::from_secs(5));
        assert_eq!(built.response_bytes().get(), 4 * 1024);
        assert_eq!(built.lua_logs().get(), 7);
        assert_eq!(built.timeout(), Duration::from_secs(5));
    }
}
