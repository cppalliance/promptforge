//! workshop-support - the workshop server's support vocabulary:
//! crash-safe atomic writes, the gateway reconnect backoff, route
//! deadline tiers, `workshop.toml` configuration, and the generic
//! retained broadcast bus the status, catalog, and menu buses are thin
//! wrappers over.
//!
//! ## Invariants
//!
//! - Tier: vocabulary; may depend on: no internal `workshop-*` crates.
//!   Read `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - A lock poisoned by a panicking peer recovers the value rather than
//!   wedging the process (the zone-two error policy).

mod atomic;
mod backoff;
mod bus;
mod config;
mod deadline;

pub use atomic::{sweep_orphaned_temps, write_atomic};
pub use backoff::{ReconnectBackoff, xorshift};
pub use bus::RetainedBus;
pub use config::{
    AgentsConfig, Config, ConfigError, DEFAULT_ADDR, DEFAULT_CONFIG_PATH, GatewayConfig,
    ServerConfig,
};
pub use deadline::{DEFAULT_DEADLINE, RELAY_DEADLINE, with_deadline};
