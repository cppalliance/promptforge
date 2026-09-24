#![doc = include_str!("lib.md")]

pub use promptforge_engine::CapabilityConflict;
pub use promptforge_engine::Environment;
pub use promptforge_engine::RequirementCheck;
pub use promptforge_engine::Requirements;
pub use promptforge_engine::Run;
pub use promptforge_engine::RunContext;
pub use promptforge_engine::RunError;
pub use promptforge_engine::RunErrorKind;
pub use promptforge_engine::RunLimits;
pub use promptforge_engine::RunResult;
pub use promptforge_engine::SourceLocation;
pub use promptforge_engine::Step;
pub use promptforge_engine::UnmetRequirement;
pub use promptforge_parser::ParseError;
pub use promptforge_parser::ParseErrorKind;
pub use promptforge_parser::Prompt;

/// The effects a run issues, the answers a host returns, and their records.
pub mod effect {
    pub use promptforge_engine::AnswerRecord;
    pub use promptforge_engine::ChatAnswerRecord;
    pub use promptforge_engine::Effect;
    pub use promptforge_engine::EffectAnswer;
    pub use promptforge_engine::EffectId;
    pub use promptforge_engine::EffectRecord;
    pub use promptforge_engine::InputAnswerRecord;
    pub use promptforge_engine::StoreAnswerRecord;
    pub use promptforge_engine::ToolAnswerRecord;
}

/// The events a run reports for its host to log.
pub mod event {
    pub use promptforge_types::emitter::DebugMode;
    pub use promptforge_types::event::Event;
    pub use promptforge_types::event::ReplyOrigin;
}

/// The identities of a run's chains and tasks, and the provenance on every report.
pub mod ids {
    pub use promptforge_types::ids::AbandonReason;
    pub use promptforge_types::ids::ChainId;
    pub use promptforge_types::ids::ParseIdError;
    pub use promptforge_types::ids::Provenance;
    pub use promptforge_types::ids::TaskId;
    pub use promptforge_types::ids::TaskOrigin;
}

/// What a model round exchanges, and the catalog and bindings a run resolves.
pub mod model {
    pub use promptforge_engine::ModelBindings;
    pub use promptforge_model_client::client::Completion;
    pub use promptforge_model_client::client::CompletionResult;
    pub use promptforge_model_client::client::Message;
    pub use promptforge_model_client::client::ToolArguments;
    pub use promptforge_model_client::client::ToolCall;
    pub use promptforge_model_client::client::ToolSchema;
    pub use promptforge_model_client::model::CompletionError;
    pub use promptforge_model_client::model::CompletionErrorKind;
    pub use promptforge_model_client::model::CompletionOptions;
    pub use promptforge_model_client::model::ModelBinding;
    pub use promptforge_model_client::model::ModelInvocation;
    pub use promptforge_model_client::model::Temperature;
    pub use promptforge_model_client::model::TemperatureError;
    pub use promptforge_types::models::ModelCatalog;
    pub use promptforge_types::models::ModelCatalogError;
    pub use promptforge_types::models::ModelDescriptor;
    pub use promptforge_types::models::ModelId;
    pub use promptforge_types::models::ModelIdError;
    pub use promptforge_types::models::ThinkingMode;
    pub use promptforge_types::wire::StreamDelta;
}

/// The sans-I/O model-round codec a host's transport runs.
pub mod transport {
    pub use promptforge_model_client::Error as ClientError;
    pub use promptforge_model_client::Timeout as ClientTimeout;
    pub use promptforge_model_client::client::ChunkSource;
    pub use promptforge_model_client::client::build_request_body;
    pub use promptforge_model_client::client::escape_controls;
    pub use promptforge_model_client::client::read_body_capped;
    pub use promptforge_model_client::client::read_completion_stream;
}

/// Tool descriptors, catalogs, identities, output, and errors.
pub mod tools {
    pub use promptforge_engine::ToolBindings;
    pub use promptforge_types::tools::OutputTrust;
    pub use promptforge_types::tools::ToolCatalog;
    pub use promptforge_types::tools::ToolCatalogError;
    pub use promptforge_types::tools::ToolCatalogErrorKind;
    pub use promptforge_types::tools::ToolDescriptor;
    pub use promptforge_types::tools::ToolError;
    pub use promptforge_types::tools::ToolErrorKind;
    pub use promptforge_types::tools::ToolId;
    pub use promptforge_types::tools::ToolIdError;
    pub use promptforge_types::tools::ToolIdErrorKind;
    pub use promptforge_types::tools::ToolOutput;
}

/// Capability identities and the global naming grammar they are built on.
pub mod capabilities {
    pub use promptforge_types::capabilities::CapabilityId;
    pub use promptforge_types::capabilities::CapabilityIdError;
    pub use promptforge_types::capabilities::CapabilityIdErrorKind;
    pub use promptforge_types::names::GlobalName;
    pub use promptforge_types::names::GlobalNameError;
    pub use promptforge_types::names::GlobalNameErrorKind;
}

/// What a parsed prompt declares in its frontmatter, and its sections and blocks.
pub mod prompt {
    pub use promptforge_parser::ArgDecl;
    pub use promptforge_parser::ArgType;
    pub use promptforge_parser::ArgsDecl;
    pub use promptforge_parser::Block;
    pub use promptforge_parser::CapabilityDecl;
    pub use promptforge_parser::FileDecl;
    pub use promptforge_parser::Frontmatter;
    pub use promptforge_parser::ModelKeyword;
    pub use promptforge_parser::ModelRole;
    pub use promptforge_parser::ModelRoles;
    pub use promptforge_parser::Section;
    pub use promptforge_parser::ToolSlot;
    pub use promptforge_parser::ToolSlots;
}

/// The virtual filesystem a run's store lives in, and the host extension point behind it.
pub mod vfs {
    pub use promptforge_engine::perform_store_op;
    pub use promptforge_lua::StoreOp;
    pub use promptforge_lua::StoreOutcome;
    pub use promptforge_store::PathReason;
    pub use promptforge_store::StoreError;
    pub use promptforge_store::StoreErrorKind;
    pub use promptforge_vfs::Access;
    pub use promptforge_vfs::AllowAll;
    pub use promptforge_vfs::Entry;
    pub use promptforge_vfs::ExecId;
    pub use promptforge_vfs::FileType;
    pub use promptforge_vfs::GrepMatch;
    pub use promptforge_vfs::GrepQuery;
    pub use promptforge_vfs::GrepResults;
    pub use promptforge_vfs::HostBackend;
    pub use promptforge_vfs::MemoryBackend;
    pub use promptforge_vfs::Op;
    pub use promptforge_vfs::OpEvent;
    pub use promptforge_vfs::OpSink;
    pub use promptforge_vfs::Origin;
    pub use promptforge_vfs::Policy;
    pub use promptforge_vfs::Stat;
    pub use promptforge_vfs::Verdict;
    pub use promptforge_vfs::Vfs;
    pub use promptforge_vfs::VfsAccess;
    pub use promptforge_vfs::VfsError;
    pub use promptforge_vfs::VfsPath;
    pub use promptforge_vfs::VfsPathBuf;
    pub use promptforge_vfs::VfsRef;
    pub use promptforge_vfs::VfsRefBuilder;
}

/// Cooperative cancellation for a run.
pub mod cancel {
    pub use promptforge_types::cancel::CancelHandle;
    pub use promptforge_types::cancel::Cancelled;
}

/// The UTC instant a run starts from.
pub mod timestamp {
    pub use promptforge_types::timestamp::Timestamp;
}

/// The model-call metrics events carry.
pub mod metrics {
    pub use promptforge_types::metrics::CallMetrics;
    pub use promptforge_types::metrics::ClientTiming;
    pub use promptforge_types::metrics::LlamaTimings;
    pub use promptforge_types::metrics::ToolCallEvent;
    pub use promptforge_types::metrics::Usage;
    pub use promptforge_types::metrics::VllmMetrics;
}

/// What a user-input wait is answered with.
pub mod input {
    pub use promptforge_engine::input::InputError;
    pub use promptforge_engine::input::InputOutcome;
}

/// The behavior flags a run records for replay.
pub mod replay {
    pub use promptforge_types::replay::Flags;
}

/// The engine's test drivers, for companion crates' suites.
#[cfg(feature = "test-support")]
pub mod test_support {
    pub use promptforge_engine::test_support::BoxFuture;
    pub use promptforge_engine::test_support::Performer;
    pub use promptforge_engine::test_support::Performers;
    pub use promptforge_engine::test_support::drive_tokio;
}
