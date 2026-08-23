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
//! and [`Config::from_toml_str`] parses a TOML string. Failures are reported
//! as the opaque [`ConfigError`]; classify them with [`ConfigError::kind`].
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
mod paths;
mod profile;
mod queue;

pub use crate::api_error::{ConfigError, ConfigErrorKind};
pub use crate::config::{
    Config, DeviceConfig, DeviceKind, EndpointConfig, LaneConfig, LocalConfig, LocalModelConfig,
    ModelConfig, Protocol, SearchProvider, Secret, ServerConfig, ThinkingMode, ToolsConfig,
    WebSearchConfig,
};
pub use crate::profile::{ProfileName, ProfileNameError, default_profiles_dir, list_profiles};
pub use crate::queue::QueueConfig;
