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

/// Resource limits for one run.
///
/// The limits cover tool iterations per section, task concurrency, model
/// response size, Lua memory, Lua log volume, and how long a model request
/// waits for its response.
///
/// The defaults are safe to use as they are and do not come from
/// environment variables. A prompt's frontmatter `max_tool_iterations`
/// field, when present, overrides [`RunLimits::max_tool_iterations`] for
/// that prompt.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct RunLimits {
    max_tool_iterations: NonZeroU32,
    concurrency: NonZeroUsize,
    max_response_bytes: NonZeroU64,
    lua_memory_bytes: NonZeroUsize,
    lua_log_events: NonZeroU32,
    request_timeout: Duration,
}

impl RunLimits {
    /// Creates limits with the default values.
    ///
    /// The defaults are 24 tool iterations per section, 8 concurrent tasks,
    /// a 16 MiB model response cap, 64 MiB of Lua memory, 1024 Lua log
    /// events, and a 120 second wait for each part of a model response.
    #[must_use]
    pub fn new() -> RunLimits {
        RunLimits {
            max_tool_iterations: const { nz_u32(24) },
            concurrency: const { nz_usize(8) },
            max_response_bytes: const { nz_u64(16 * 1024 * 1024) },
            lua_memory_bytes: const { nz_usize(64 * 1024 * 1024) },
            lua_log_events: const { nz_u32(1024) },
            request_timeout: Duration::from_secs(120),
        }
    }

    /// Sets the default maximum number of tool iterations per section.
    ///
    /// A tool iteration is one model round trip. A prompt's frontmatter
    /// `max_tool_iterations` field overrides this default for that prompt.
    #[must_use]
    pub fn max_tool_iterations(mut self, value: NonZeroU32) -> RunLimits {
        self.max_tool_iterations = value;
        self
    }

    /// Sets the run's concurrency ceiling: the most tasks the scheduler
    /// admits at once across the whole run.
    ///
    /// Each spawned task counts against its owner's limit and against every
    /// ancestor's limit, so limits nest. A fanout started inside one arm of
    /// another fanout runs within that arm's remaining share.
    #[must_use]
    pub fn max_concurrency(mut self, value: NonZeroUsize) -> RunLimits {
        self.concurrency = value;
        self
    }

    /// Sets the maximum accepted model response body size, in bytes.
    #[must_use]
    pub fn max_response_bytes(mut self, value: NonZeroU64) -> RunLimits {
        self.max_response_bytes = value;
        self
    }

    /// Sets the memory ceiling for each Lua virtual machine, in bytes.
    #[must_use]
    pub fn lua_memory_bytes(mut self, value: NonZeroUsize) -> RunLimits {
        self.lua_memory_bytes = value;
        self
    }

    /// Sets the maximum number of `log` events that a prompt's Lua code can
    /// record in each Lua virtual machine.
    #[must_use]
    pub fn lua_log_events(mut self, value: NonZeroU32) -> RunLimits {
        self.lua_log_events = value;
        self
    }

    /// Sets the longest time a model request waits for the next part of its
    /// response.
    ///
    /// The request waits first for the response headers, then for each body
    /// chunk. Each arrival restarts the wait, so a long stream that keeps
    /// arriving is never cut off.
    #[must_use]
    pub fn request_timeout(mut self, value: Duration) -> RunLimits {
        self.request_timeout = value;
        self
    }

    /// Returns the default maximum number of tool iterations per section.
    #[must_use]
    pub fn tool_iterations(&self) -> NonZeroU32 {
        self.max_tool_iterations
    }

    /// Returns the run's concurrency ceiling: the most tasks the
    /// scheduler admits at once across the whole run.
    #[must_use]
    pub fn concurrency(&self) -> NonZeroUsize {
        self.concurrency
    }

    /// Returns the maximum accepted model response body size, in bytes.
    #[must_use]
    pub fn response_bytes(&self) -> NonZeroU64 {
        self.max_response_bytes
    }

    /// Returns the memory ceiling for each Lua virtual machine, in bytes.
    #[must_use]
    pub fn lua_memory(&self) -> NonZeroUsize {
        self.lua_memory_bytes
    }

    /// Returns the maximum number of `log` events that a prompt's Lua code
    /// can record in each Lua virtual machine.
    #[must_use]
    pub fn lua_logs(&self) -> NonZeroU32 {
        self.lua_log_events
    }

    /// Returns the longest time a model request waits for the next part of
    /// its response.
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
        assert_eq!(defaults.concurrency().get(), 8);
        assert_eq!(defaults.response_bytes().get(), 16 * 1024 * 1024);
        assert_eq!(defaults.lua_memory().get(), 64 * 1024 * 1024);
        assert_eq!(defaults.lua_logs().get(), 1024);
        assert_eq!(defaults.timeout(), Duration::from_secs(120));

        let built = RunLimits::new()
            .max_response_bytes(const { nz_u64(4 * 1024) })
            .lua_log_events(const { nz_u32(7) })
            .request_timeout(Duration::from_secs(5));
        assert_eq!(built.response_bytes().get(), 4 * 1024);
        assert_eq!(built.lua_logs().get(), 7);
        assert_eq!(built.timeout(), Duration::from_secs(5));
    }

    #[test]
    fn the_tool_iteration_builder_replaces_the_default_limit() {
        let limits = RunLimits::new().max_tool_iterations(const { nz_u32(8) });
        assert_eq!(limits.tool_iterations().get(), 8);
    }
}
