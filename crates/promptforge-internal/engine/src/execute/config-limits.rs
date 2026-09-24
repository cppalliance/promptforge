//! Per-run resource ceilings: [`RunLimits`].

use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::time::Duration;

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
/// The defaults are safe, non-environment values that a clean build can use
/// as they are. Frontmatter `max_tool_iterations`, when present, still
/// overrides [`RunLimits::max_tool_iterations`] for that prompt.
///
/// # Examples
/// ```
/// use std::num::NonZeroU32;
///
/// use promptforge_engine::RunLimits;
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
    /// use promptforge_engine::RunLimits;
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
