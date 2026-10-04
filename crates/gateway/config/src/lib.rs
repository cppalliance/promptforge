//! Configuration for the PromptForge inference gateway.
//!
//! This crate owns everything needed to turn one `gateway.toml` at
//! `config-version = 0` plus its sibling profile state into a validated
//! [`Config`]: TOML parsing, `${VAR}` interpolation, startup profile
//! selection, and validation of every profile before any can run.
//!
//! It is deliberately free of the gateway's HTTP stack: consumers that only
//! need to read a configuration (IDE tooling, config editors, CLIs) depend on
//! this crate alone.
//!
//! Start at [`Config`]: [`Config::load`] reads the single file and resolves
//! command-line, environment, or sibling-state selection, while
//! [`Config::from_toml_str`] validates an unselected in-memory catalog.
//! Removed include chains, profile directories, top-level allowlists, and
//! `[workshop.voice]` fields, and a missing or unsupported `config-version`,
//! produce hard-break diagnostics that name the file, line, problem, and
//! fix. Failures are reported as the opaque [`ConfigError`]; classify them
//! with [`ConfigError::kind`].
//!
//! Pending edits stage as a shadow file beside the real one
//! (`gateway.toml` gains `gateway.toml.next`):
//! [`save_config_shadow`] validates a pending admin document and refuses
//! `active_profile` (selection is not a configuration key), [`write_shadow`]
//! stages arbitrary sibling content, and [`shadow_path`] names a shadow.
//! [`load_pending_config`] reads the shadow with the same selection rules as
//! [`Config::load`], and [`pending_report`] summarizes changed sections.
//! [`persist_profile_state`] atomically updates the real active profile,
//! [`clear_profile_state`] deletes it (the persisted form of "no profile"),
//! and [`write_atomic`] is the bare replace-through-rename primitive that
//! shadow writes, profile state, and the gateway's apply step build on.
//!
//! The crate never mutates the process environment: `${VAR}` interpolation
//! reads it, and loading env files into it is the calling binary's job.

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
    SttModelConfig, SttPipelineConfig, SttRole, ThinkingMode, ToolDialect, ToolsConfig,
    WebSearchConfig, WorkshopConfig,
};
pub use crate::profile::{
    ProfileName, ProfileNameError, ProfileSelection, ProfileState, profile_state_path,
};
pub use crate::shadow::{
    PendingReport, PendingShadows, clear_profile_state, load_pending_config, pending_report,
    pending_var_references, persist_profile_state, save_config_shadow, shadow_path, write_atomic,
    write_shadow,
};
