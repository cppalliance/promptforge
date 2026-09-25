//! workshop-support - the workshop server's support vocabulary:
//! crash-safe atomic writes, the gateway reconnect backoff, route
//! deadline tiers, `workshop.toml` configuration, the generic retained
//! broadcast bus the status, catalog, and menu buses are thin wrappers
//! over, the shared error-message rendering, and the JSON state-bucket
//! validator the user-state and workspace buckets both use.
//!
//! ## Invariants
//!
//! - Tier: vocabulary; may depend on: no internal `workshop-*` crates.
//!   Read `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - A lock poisoned by a panicking peer recovers the value rather than
//!   wedging the process.

mod atomic;
mod backoff;
mod bus;
mod config;
mod deadline;
mod error_message;
#[cfg(feature = "test-fixtures")]
pub mod fixtures;
mod state_bucket;

pub use atomic::{sweep_orphaned_temps, write_atomic};
pub use backoff::{ReconnectBackoff, xorshift};
pub use bus::RetainedBus;
pub use config::{
    AgentsConfig, Config, ConfigError, DEFAULT_ADDR, DEFAULT_CONFIG_PATH, GatewayConfig,
    ServerConfig,
};
pub use deadline::{
    DEADLINE_ELAPSED_CODE, DEFAULT_DEADLINE, RELAY_DEADLINE, deadline_elapsed_message,
    with_deadline,
};
pub use error_message::{LEAK_DETAIL, render_message};
pub use state_bucket::{
    StateBucketError, check_bucket_cap, check_bucket_text, resolve_bucket_key, validate_bucket_body,
};
