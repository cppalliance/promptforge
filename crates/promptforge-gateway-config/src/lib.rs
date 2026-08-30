//! Configuration for the PromptForge inference gateway.
//!
//! This crate owns everything needed to turn a `gateway.toml` (or a named
//! profile with recursive `include` resolution) into a validated [`Config`]:
//! TOML parsing, `${VAR}` environment interpolation over string leaves, and
//! semantic validation. A [`Config`] cannot be constructed without validation,
//! so downstream code never re-checks operator input.
//!
//! It is deliberately free of the gateway's HTTP stack: consumers that only
//! need to read a configuration (IDE tooling, config editors, CLIs) depend on
//! this crate alone.
//!
//! Start at [`Config`]: [`Config::load`] reads a file with include resolution,
//! [`Config::load_profile`] loads a named profile from a profiles directory,
//! and [`Config::from_toml_str`] parses a TOML string.
//! [`Config::load_profile_with_chain`] additionally returns the resolved
//! include chain, and [`load_server`] and [`load_workshop`] read only a boot
//! file's `[server]` and `[workshop]` sections (includes and interpolation,
//! without full validation); [`load_boot_sections`] reads both in one pass.
//! Failures are reported as the opaque
//! [`ConfigError`]; classify them with [`ConfigError::kind`].
//!
//! Pending edits stage as shadow files beside the real ones
//! (`default.toml` gains `default.toml.next`): [`save_profile_shadow`],
//! [`save_include_shadow`], and [`save_boot_shadow`] validate the merged
//! pending state before writing, [`write_shadow`] stages arbitrary sibling
//! content (env files), and [`shadow_path`] names the shadow for any
//! managed file. No save ever touches a real file. Reading the pending
//! state back, [`load_pending_profile`] loads the chain with shadows
//! preferred (provenance names the `.next` files), and [`pending_report`]
//! summarizes which files carry shadows and which sections they change.
//!
//! The crate never mutates the process environment: `${VAR}` interpolation
//! reads it, and loading env files into it is the calling binary's job.
//!
//! # Examples
//!
//! ```no_run
//! use promptforge_gateway_config::Config;
//! use std::path::Path;
//!
//! let config = Config::load(Path::new("gateway.toml"))?;
//! println!("{} models configured", config.models().len());
//! # Ok::<(), promptforge_gateway_config::ConfigError>(())
//! ```

mod api_error;
mod config;
mod error;
mod profile;

pub use crate::api_error::{ConfigError, ConfigErrorKind};
pub use crate::config::{
    Capabilities, Config, DominionConfig, DominionKind, DraftTokenMax, DraftTokenMaxError,
    EndpointConfig, LocalConfig, LocalModelConfig, ModelConfig, ModelKind,
    MultimodalProjectorConfig, Protocol, QueuePolicy, SearchProvider, Secret, ServerConfig,
    SpeculationType, SpeculativeConfig, ThinkingMode, ToolDialect, ToolsConfig, WebSearchConfig,
    WorkshopConfig, WorkshopTapeConfig, WorkshopVoiceConfig,
};
pub use crate::profile::{
    PendingReport, ProfileName, ProfileNameError, list_profiles, load_boot_sections,
    load_pending_profile, load_server, load_workshop, pending_report, save_boot_shadow,
    save_include_shadow, save_profile_shadow, shadow_path, write_shadow,
};
