//! Parses Runvil prompt files and runs them, leaving their outside work
//! to the caller.
//!
//! A [`Prompt`] is a parsed prompt file. A [`Run`] executes one prompt.
//! [`Run::step`] returns [`Step::Pending`] with the effects the run waits on.
//! The caller performs each effect and hands its answer to [`Run::resume`].
//! The run ends at [`Step::Done`] with its [`RunResult`].
//! Before the run starts, the caller puts the tools a run may offer on its
//! [`RunContext`], and [`RunContext::prepare`] binds the prompt's model roles.
//!
//! The crate's `greeter` example is that whole loop in one program.

pub use runvil_engine::MissingService;
pub use runvil_engine::RequirementCheck;
pub use runvil_engine::Requirements;
pub use runvil_engine::Run;
pub use runvil_engine::RunContext;
pub use runvil_engine::RunError;
pub use runvil_engine::RunErrorKind;
pub use runvil_engine::RunLimits;
pub use runvil_engine::RunResult;
pub use runvil_engine::SourceLocation;
pub use runvil_engine::Step;
pub use runvil_engine::UnavailablePlugin;
pub use runvil_engine::UnmetRequirement;
pub use runvil_parser::ParseError;
pub use runvil_parser::ParseErrorKind;
pub use runvil_parser::Prompt;
pub use runvil_types::seed::RunSeed;
pub use runvil_types::seed::RunUid;

pub mod effect {
    //! The outside work a run asks for, the answers it takes back, and the
    //! records a run log stores for both.

    pub use runvil_engine::AnswerRecord;
    pub use runvil_engine::ChatAnswerRecord;
    pub use runvil_engine::Effect;
    pub use runvil_engine::EffectAnswer;
    pub use runvil_engine::EffectId;
    pub use runvil_engine::EffectRecord;
    pub use runvil_engine::FailureRecord;
    pub use runvil_engine::Round;
    pub use runvil_engine::ToolAnswerRecord;
    pub use runvil_engine::ToolCallOrigin;
    pub use runvil_engine::ToolCaller;
}

pub mod event {
    //! The events a run reports for the caller to log.

    pub use runvil_types::emitter::DebugMode;
    pub use runvil_types::event::Event;
    pub use runvil_types::event::ReplyOrigin;
}

pub mod ids {
    //! Task, chain, and round ids, and the provenance that orders a run's
    //! records.

    pub use runvil_types::ids::AbandonReason;
    pub use runvil_types::ids::ChainId;
    pub use runvil_types::ids::ParseIdError;
    pub use runvil_types::ids::ParseTaskOriginError;
    pub use runvil_types::ids::Provenance;
    pub use runvil_types::ids::RoundId;
    pub use runvil_types::ids::TaskId;
    pub use runvil_types::ids::TaskOrigin;
}

pub mod model {
    //! Model descriptions and catalogs, the models a prompt's roles are
    //! bound to, and the chat vocabulary of a model round.

    pub use runvil_engine::ModelBindings;
    pub use runvil_model_client::client::ArgumentNames;
    pub use runvil_model_client::client::Completion;
    pub use runvil_model_client::client::CompletionResult;
    pub use runvil_model_client::client::Message;
    pub use runvil_model_client::client::ParseRoleError;
    pub use runvil_model_client::client::RawExchange;
    pub use runvil_model_client::client::Role;
    pub use runvil_model_client::client::ToolArguments;
    pub use runvil_model_client::client::ToolCall;
    pub use runvil_model_client::client::ToolSchema;
    pub use runvil_model_client::model::CompletionError;
    pub use runvil_model_client::model::CompletionErrorKind;
    pub use runvil_model_client::model::CompletionOptions;
    pub use runvil_model_client::model::ModelBinding;
    pub use runvil_model_client::model::ModelInvocation;
    pub use runvil_model_client::model::Temperature;
    pub use runvil_model_client::model::TemperatureError;
    pub use runvil_types::models::ModelCatalog;
    pub use runvil_types::models::ModelCatalogError;
    pub use runvil_types::models::ModelDescriptor;
    pub use runvil_types::models::ModelId;
    pub use runvil_types::models::ModelIdError;
    pub use runvil_types::models::ThinkingMode;
}

pub mod tools {
    //! Tool descriptions and catalogs, and the output or error that answers
    //! a tool call.

    pub use runvil_types::tools::OutputTrust;
    pub use runvil_types::tools::ToolCatalog;
    pub use runvil_types::tools::ToolCatalogError;
    pub use runvil_types::tools::ToolCatalogErrorKind;
    pub use runvil_types::tools::ToolDescriptor;
    pub use runvil_types::tools::ToolError;
    pub use runvil_types::tools::ToolErrorKind;
    pub use runvil_types::tools::ToolId;
    pub use runvil_types::tools::ToolIdError;
    pub use runvil_types::tools::ToolIdErrorKind;
    pub use runvil_types::tools::ToolOutput;
}

pub mod plugins {
    //! Plugin ids, the Lua preludes Plugins add to a run, and the
    //! global names those preludes may define.

    pub use runvil_types::names::GlobalName;
    pub use runvil_types::names::GlobalNameError;
    pub use runvil_types::names::GlobalNameErrorKind;
    pub use runvil_types::plugins::PluginId;
    pub use runvil_types::plugins::PluginIdError;
    pub use runvil_types::plugins::PluginIdErrorKind;
    pub use runvil_types::plugins::Prelude;
}

pub mod prompt {
    //! What a prompt's frontmatter declares: its files, Plugins,
    //! arguments, and model roles.

    pub use runvil_parser::ArgDecl;
    pub use runvil_parser::ArgType;
    pub use runvil_parser::ArgsDecl;
    pub use runvil_parser::ArgsDeclIter;
    pub use runvil_parser::FileDecl;
    pub use runvil_parser::Frontmatter;
    pub use runvil_parser::ModelKeyword;
    pub use runvil_parser::ModelRole;
    pub use runvil_parser::ModelRoles;
    pub use runvil_parser::ModelRolesIter;
}

pub mod vfs {
    //! A run's files: the store every section shares, real directories
    //! beside it, and the policy that decides what the run may change.

    pub use runvil_engine::perform_vfs_op;
    pub use runvil_lua::VfsOp;
    pub use runvil_lua::VfsOutcome;
    pub use runvil_vfs::Access;
    pub use runvil_vfs::AcquireContext;
    pub use runvil_vfs::AllowAll;
    pub use runvil_vfs::Depth;
    pub use runvil_vfs::Entry;
    pub use runvil_vfs::ExecId;
    pub use runvil_vfs::FileType;
    pub use runvil_vfs::GlobKind;
    pub use runvil_vfs::MemoryBackend;
    pub use runvil_vfs::Mode;
    pub use runvil_vfs::ModeHandle;
    pub use runvil_vfs::ModePolicy;
    pub use runvil_vfs::Op;
    pub use runvil_vfs::OpEvent;
    pub use runvil_vfs::OpSink;
    pub use runvil_vfs::Origin;
    pub use runvil_vfs::ParsePathReasonError;
    pub use runvil_vfs::PathReason;
    pub use runvil_vfs::Policy;
    pub use runvil_vfs::RealBackend;
    pub use runvil_vfs::Stat;
    pub use runvil_vfs::Verdict;
    pub use runvil_vfs::Vfs;
    pub use runvil_vfs::VfsAccess;
    pub use runvil_vfs::VfsError;
    pub use runvil_vfs::VfsPath;
    pub use runvil_vfs::VfsPathBuf;
    pub use runvil_vfs::VfsRef;
    pub use runvil_vfs::VfsRefBuilder;
    pub use runvil_vfs::Writability;
}

pub mod cancel {
    //! Cancel handles that stop runs and tasks from any thread.

    pub use runvil_types::cancel::CancelHandle;
    pub use runvil_types::cancel::Cancelled;
}

pub mod timestamp {
    //! The start time a run's context carries, which a prompt reads as
    //! `sys.when`.

    pub use runvil_types::timestamp::Timestamp;
}

pub mod metrics {
    //! The token counts and timings of a model call.

    pub use runvil_types::metrics::CallMetrics;
    pub use runvil_types::metrics::ClientTiming;
    pub use runvil_types::metrics::LlamaTimings;
    pub use runvil_types::metrics::ToolCallEvent;
    pub use runvil_types::metrics::Usage;
    pub use runvil_types::metrics::VllmMetrics;
}

pub mod replay {
    //! The behavior flags a run's record keeps and hands back bit for bit.

    pub use runvil_types::replay::Flags;
}
