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

pub mod effect {
    #![doc = include_str!("effect.md")]

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

pub mod event {
    #![doc = include_str!("event.md")]

    pub use promptforge_types::emitter::DebugMode;
    pub use promptforge_types::event::Event;
    pub use promptforge_types::event::ReplyOrigin;
}

pub mod ids {
    #![doc = include_str!("ids.md")]

    pub use promptforge_types::ids::AbandonReason;
    pub use promptforge_types::ids::ChainId;
    pub use promptforge_types::ids::ParseIdError;
    pub use promptforge_types::ids::Provenance;
    pub use promptforge_types::ids::TaskId;
    pub use promptforge_types::ids::TaskOrigin;
}

pub mod model {
    #![doc = include_str!("model.md")]

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

pub mod transport {
    #![doc = include_str!("transport.md")]

    pub use promptforge_model_client::Error as ClientError;
    pub use promptforge_model_client::Timeout as ClientTimeout;
    pub use promptforge_model_client::client::ChunkSource;
    pub use promptforge_model_client::client::build_request_body;
    pub use promptforge_model_client::client::escape_controls;
    pub use promptforge_model_client::client::read_body_capped;
    pub use promptforge_model_client::client::read_completion_stream;
}

pub mod tools {
    #![doc = include_str!("tools.md")]

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

pub mod capabilities {
    #![doc = include_str!("capabilities.md")]

    pub use promptforge_types::capabilities::CapabilityId;
    pub use promptforge_types::capabilities::CapabilityIdError;
    pub use promptforge_types::capabilities::CapabilityIdErrorKind;
    pub use promptforge_types::names::GlobalName;
    pub use promptforge_types::names::GlobalNameError;
    pub use promptforge_types::names::GlobalNameErrorKind;
}

pub mod prompt {
    #![doc = include_str!("prompt.md")]

    pub use promptforge_parser::ArgDecl;
    pub use promptforge_parser::ArgType;
    pub use promptforge_parser::ArgsDecl;
    pub use promptforge_parser::CapabilityDecl;
    pub use promptforge_parser::FileDecl;
    pub use promptforge_parser::Frontmatter;
    pub use promptforge_parser::ModelKeyword;
    pub use promptforge_parser::ModelRole;
    pub use promptforge_parser::ModelRoles;
    pub use promptforge_parser::ToolSlot;
    pub use promptforge_parser::ToolSlots;
}

pub mod vfs {
    #![doc = include_str!("vfs.md")]

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
    pub use promptforge_vfs::Mode;
    pub use promptforge_vfs::ModeHandle;
    pub use promptforge_vfs::ModePolicy;
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

pub mod cancel {
    #![doc = include_str!("cancel.md")]

    pub use promptforge_types::cancel::CancelHandle;
    pub use promptforge_types::cancel::Cancelled;
}

pub mod timestamp {
    #![doc = include_str!("timestamp.md")]

    pub use promptforge_types::timestamp::Timestamp;
}

pub mod metrics {
    #![doc = include_str!("metrics.md")]

    pub use promptforge_types::metrics::CallMetrics;
    pub use promptforge_types::metrics::ClientTiming;
    pub use promptforge_types::metrics::LlamaTimings;
    pub use promptforge_types::metrics::ToolCallEvent;
    pub use promptforge_types::metrics::Usage;
    pub use promptforge_types::metrics::VllmMetrics;
}

pub mod input {
    #![doc = include_str!("input.md")]

    pub use promptforge_engine::input::InputError;
    pub use promptforge_engine::input::InputOutcome;
}

pub mod replay {
    #![doc = include_str!("replay.md")]

    pub use promptforge_types::replay::Flags;
}
