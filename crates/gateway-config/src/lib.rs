//! Configuration for the PromptForge inference gateway.
//!
//! This crate owns everything needed to turn one version-2 `gateway.toml`
//! plus its sibling profile state into a validated [`Config`]: TOML parsing,
//! `${VAR}` interpolation, startup profile selection, and validation of every
//! profile before any can run.
//!
//! It is deliberately free of the gateway's HTTP stack: consumers that only
//! need to read a configuration (IDE tooling, config editors, CLIs) depend on
//! this crate alone.
//!
//! Start at [`Config`]: [`Config::load`] reads the single file and resolves
//! command-line, environment, or sibling-state selection, while
//! [`Config::from_toml_str`] validates an unselected in-memory catalog.
//! Removed include chains, profile directories, top-level allowlists, and
//! `[workshop.voice]` fields produce hard-break diagnostics with file, key,
//! and line. Failures are reported as the opaque
//! [`ConfigError`]; classify them with [`ConfigError::kind`].
//!
//! Pending edits stage as shadow files beside the real ones
//! (`gateway.toml` gains `gateway.toml.next`):
//! [`save_config_shadow`] validates a pending admin document and splits its
//! `active_profile` into the sibling state shadow, [`write_shadow`] stages
//! arbitrary sibling content, and [`shadow_path`] names either shadow.
//! [`load_pending_config`] reads both shadows with the same selection rules,
//! and [`pending_report`] summarizes changed sections.
//! [`promote_shadow`] is the explicit apply step. It uses atomic replacement
//! where the platform supports it and a failure-safe backup fallback
//! elsewhere. [`persist_profile_state`] atomically updates the real active
//! profile without consuming an unapplied state shadow.
//!
//! The crate never mutates the process environment: `${VAR}` interpolation
//! reads it, and loading env files into it is the calling binary's job.
//!
//! # Examples
//!
//! ```no_run
//! use gateway_config::Config;
//! use std::path::Path;
//!
//! let inputs = gateway_config::ProfileSelection::new(Some("work"), None);
//! let config = Config::load(Path::new("gateway.toml"), &inputs)?;
//! println!("{} models configured", config.models().len());
//! # Ok::<(), gateway_config::ConfigError>(())
//! ```

mod api_error;
mod config;
mod error;
mod profile;
mod shadow;

pub use crate::api_error::{ConfigError, ConfigErrorKind};
pub use crate::config::{
    Capabilities, Config, DominionConfig, DominionKind, DraftTokenMax, DraftTokenMaxError,
    EndpointConfig, LlamaBackend, LocalConfig, LocalModelConfig, ModelConfig, ModelKind,
    MultimodalProjectorConfig, ProfileConfig, Protocol, QueuePolicy, RECOMMENDED_STT_MODELS,
    RecommendedSttModel, SearchProvider, Secret, ServerConfig, SpeculationType, SpeculativeConfig,
    SttModelConfig, SttRole, ThinkingMode, ToolDialect, ToolsConfig, WebSearchConfig,
    WorkshopConfig, WorkshopSttConfig, WorkshopTapeConfig,
};
pub use crate::profile::{
    ProfileName, ProfileNameError, ProfileSelection, ProfileState, profile_state_path,
};
pub use crate::shadow::{
    PendingReport, PendingShadows, load_pending_config, pending_report, pending_var_references,
    persist_profile_state, promote_shadow, save_config_shadow, shadow_path, write_shadow,
};
