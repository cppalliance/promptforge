//! Run configuration and resource limits: [`RunConfig`] and [`RunLimits`].

use std::fmt;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::Arc;
use std::time::Duration;

use crate::cancel::CancelHandle;
use crate::client::GatewayClient;
use crate::debug::DebugCapture;
use crate::observe::{NullObserver, Observer};

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
/// use promptforge_core::execute::RunLimits;
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
    /// use promptforge_core::execute::RunLimits;
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

/// Everything a run needs beyond the prompt, its input, its tools, and its
/// store: the execution id, where progress is reported, the raw-capture seam,
/// the gateway client, an explicit cancellation handle, and resource limits.
///
/// `RunConfig` is owned (no borrows), so its observer and debug sinks reach the
/// nested `model:infer` hook that a borrowed option could not.
///
/// # Examples
/// ```
/// use promptforge_core::execute::{RunConfig, RunLimits};
///
/// let config = RunConfig::new("example-run").limits(RunLimits::new());
/// assert_eq!(config.execution(), "example-run");
/// ```
#[non_exhaustive]
pub struct RunConfig {
    pub(crate) execution: String,
    pub(crate) observer: Arc<dyn Observer>,
    pub(crate) debug: Option<Arc<dyn DebugCapture>>,
    pub(crate) client: Option<GatewayClient>,
    pub(crate) cancel: Option<CancelHandle>,
    pub(crate) limits: RunLimits,
}

impl RunConfig {
    /// Builds a config for `execution` with default observer, no client, no
    /// capture, no cancellation, and default [`RunLimits`].
    #[must_use]
    pub fn new(execution: impl Into<String>) -> RunConfig {
        RunConfig {
            execution: execution.into(),
            observer: Arc::new(NullObserver),
            debug: None,
            client: None,
            cancel: None,
            limits: RunLimits::new(),
        }
    }

    /// Sets the progress observer, retained for the whole run and its infer hook.
    #[must_use]
    pub fn observer(mut self, observer: Arc<dyn Observer>) -> RunConfig {
        self.observer = observer;
        self
    }

    /// Sets the opt-in raw request/response capture sink.
    #[must_use]
    pub fn debug(mut self, debug: Arc<dyn DebugCapture>) -> RunConfig {
        self.debug = Some(debug);
        self
    }

    /// Sets the gateway client; `None` builds one from the environment on first
    /// use.
    #[must_use]
    pub fn client(mut self, client: GatewayClient) -> RunConfig {
        self.client = Some(client);
        self
    }

    /// Sets the explicit cancellation handle threaded through the run.
    #[must_use]
    pub fn cancel(mut self, handle: CancelHandle) -> RunConfig {
        self.cancel = Some(handle);
        self
    }

    /// Sets the resource limits honored across the run.
    #[must_use]
    pub fn limits(mut self, limits: RunLimits) -> RunConfig {
        self.limits = limits;
        self
    }

    /// Returns the execution identifier shared by every report.
    #[must_use]
    pub fn execution(&self) -> &str {
        &self.execution
    }
}

impl fmt::Debug for RunConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunConfig")
            .field("execution", &self.execution)
            .field("observer", &"<dyn Observer>")
            .field("client", &self.client)
            .field("debug", &self.debug.as_ref().map(|_| "<dyn DebugCapture>"))
            .field("cancel", &self.cancel.is_some())
            .field("limits", &self.limits)
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
