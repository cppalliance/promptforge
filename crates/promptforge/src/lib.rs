//! Parses PromptForge prompt files and runs them, leaving their outside work
//! to the caller.
//!
//! A [`Prompt`] is a parsed prompt file. A [`Run`] executes one prompt.
//! [`Run::step`] returns [`Step::Pending`] with the effects the run waits on.
//! The caller performs each effect and hands its answer to [`Run::resume`].
//! The run ends at [`Step::Done`] with its [`RunResult`].
//! Before the run starts, [`Environment::prepare`] binds the prompt's model
//! roles and tool slots into the run's [`RunContext`].
//!
//! The crate's `greeter` example is that whole loop in one program.

pub use promptforge_engine::Environment;
pub use promptforge_engine::MissingService;
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
pub use promptforge_engine::UnavailablePlugin;
pub use promptforge_engine::UnmetRequirement;
pub use promptforge_parser::ParseError;
pub use promptforge_parser::ParseErrorKind;
pub use promptforge_parser::Prompt;

pub mod effect {
    //! The outside work a run asks for, the answers it takes back, and the
    //! records a run log stores for both.

    pub use promptforge_engine::AnswerRecord;
    pub use promptforge_engine::ChatAnswerRecord;
    pub use promptforge_engine::Effect;
    pub use promptforge_engine::EffectAnswer;
    pub use promptforge_engine::EffectId;
    pub use promptforge_engine::EffectRecord;
    pub use promptforge_engine::Round;
    pub use promptforge_engine::ToolAnswerRecord;
    pub use promptforge_engine::ToolCallOrigin;
    pub use promptforge_engine::ToolCaller;
}

pub mod event {
    //! The events a run reports for the caller to log.

    pub use promptforge_types::emitter::DebugMode;
    pub use promptforge_types::event::Event;
    pub use promptforge_types::event::ReplyOrigin;
}

pub mod ids {
    //! Task, chain, and round ids, and the provenance that orders a run's
    //! records.

    pub use promptforge_types::ids::AbandonReason;
    pub use promptforge_types::ids::ChainId;
    pub use promptforge_types::ids::ParseIdError;
    pub use promptforge_types::ids::Provenance;
    pub use promptforge_types::ids::RoundId;
    pub use promptforge_types::ids::TaskId;
    pub use promptforge_types::ids::TaskOrigin;
}

pub mod model {
    //! Model descriptions and catalogs, the models a prompt's roles are
    //! bound to, and the chat vocabulary of a model round.

    pub use promptforge_engine::ModelBindings;
    pub use promptforge_model_client::client::Completion;
    pub use promptforge_model_client::client::CompletionResult;
    pub use promptforge_model_client::client::Message;
    pub use promptforge_model_client::client::RawExchange;
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
}

pub mod tools {
    //! Tool descriptions and catalogs, the tools a prompt's slots are bound
    //! to, and the output or error that answers a tool call.

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

pub mod plugins {
    //! Plugin ids, the Lua preludes Plugins add to a run, and the
    //! global names those preludes may define.

    pub use promptforge_types::names::GlobalName;
    pub use promptforge_types::names::GlobalNameError;
    pub use promptforge_types::names::GlobalNameErrorKind;
    pub use promptforge_types::plugins::PluginId;
    pub use promptforge_types::plugins::PluginIdError;
    pub use promptforge_types::plugins::PluginIdErrorKind;
    pub use promptforge_types::plugins::Prelude;
}

pub mod prompt {
    //! What a prompt's frontmatter declares: its files, Plugins, tool
    //! slots, arguments, and model roles.

    pub use promptforge_parser::ArgDecl;
    pub use promptforge_parser::ArgType;
    pub use promptforge_parser::ArgsDecl;
    pub use promptforge_parser::FileDecl;
    pub use promptforge_parser::Frontmatter;
    pub use promptforge_parser::ModelKeyword;
    pub use promptforge_parser::ModelRole;
    pub use promptforge_parser::ModelRoles;
    pub use promptforge_parser::ToolSlot;
    pub use promptforge_parser::ToolSlots;
}

pub mod vfs {
    //! A run's files: the store every section shares, real directories
    //! beside it, and the policy that decides what the run may change.

    pub use promptforge_engine::perform_vfs_op;
    pub use promptforge_lua::VfsOp;
    pub use promptforge_lua::VfsOutcome;
    pub use promptforge_vfs::Access;
    pub use promptforge_vfs::AcquireContext;
    pub use promptforge_vfs::AllowAll;
    pub use promptforge_vfs::Entry;
    pub use promptforge_vfs::ExecId;
    pub use promptforge_vfs::FileType;
    pub use promptforge_vfs::MemoryBackend;
    pub use promptforge_vfs::Mode;
    pub use promptforge_vfs::ModeHandle;
    pub use promptforge_vfs::ModePolicy;
    pub use promptforge_vfs::Op;
    pub use promptforge_vfs::OpEvent;
    pub use promptforge_vfs::OpSink;
    pub use promptforge_vfs::Origin;
    pub use promptforge_vfs::PathReason;
    pub use promptforge_vfs::Policy;
    pub use promptforge_vfs::RealBackend;
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
    //! Cancel handles that stop runs and tasks from any thread.

    pub use promptforge_types::cancel::CancelHandle;
    pub use promptforge_types::cancel::Cancelled;
}

pub mod timestamp {
    //! The start time a run's context carries, which a prompt reads as
    //! `sys.when`.

    pub use promptforge_types::timestamp::Timestamp;
}

pub mod metrics {
    //! The token counts and timings of a model call.

    pub use promptforge_types::metrics::CallMetrics;
    pub use promptforge_types::metrics::ClientTiming;
    pub use promptforge_types::metrics::LlamaTimings;
    pub use promptforge_types::metrics::ToolCallEvent;
    pub use promptforge_types::metrics::Usage;
    pub use promptforge_types::metrics::VllmMetrics;
}

pub mod replay {
    //! The behavior flags a run's record keeps and hands back bit for bit.

    pub use promptforge_types::replay::Flags;
}
