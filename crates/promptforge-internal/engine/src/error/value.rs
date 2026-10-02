//! The internal error as a Lua error value: the kind an author branches
//! on and the kind's fields.

use super::{Error, join_task_ids};

/// The internal type's rendering into the Lua error table: the kind an author
/// branches on and the kind's fields. Failures outside the prompt
/// (transport, backend, configuration, input) render as
/// `internal`; every Lua-phase failure renders as `lua`; a store
/// operation's own failure renders as `store` with its `reason` and the
/// structured variant's fields.
impl promptforge_lua::ErrorValue for Error {
    fn kind(&self) -> promptforge_lua::ErrorKind {
        use promptforge_lua::ErrorKind;
        match self {
            Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::LuaQuota { .. }
            | Error::Substitution(_) => ErrorKind::Lua,
            Error::ContextExhausted { .. } => ErrorKind::ContextExhausted,
            Error::EmptyModelReply { .. } => ErrorKind::EmptyModelReply,
            Error::Interrupted | Error::TaskCancelled { .. } => ErrorKind::Cancelled,
            Error::ToolLoopExhausted => ErrorKind::ToolLoopExhausted,
            Error::TasksLive { .. } => ErrorKind::TasksLive,
            Error::TaskNotOwned { .. } => ErrorKind::TaskNotOwned,
            Error::TaskConsumed { .. } => ErrorKind::TaskConsumed,
            Error::OutOfScopeToolCall { .. } => ErrorKind::OutOfScopeTool,
            Error::UnboundToolCall { .. } => ErrorKind::UnboundTool,
            Error::Tool { .. } => ErrorKind::Tool,
            Error::Store { .. } => ErrorKind::Store,
            Error::ParseFrontmatter { .. }
            | Error::ParseStructured { .. }
            | Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::Config { .. }
            | Error::GatewayDisabled
            | Error::Http(_)
            | Error::Backend { .. }
            | Error::MalformedResponse(_)
            | Error::MalformedResponseSource { .. }
            | Error::BackendBodyRead { .. }
            | Error::BindSchema { .. }
            | Error::ModelRequired { .. }
            | Error::UnsupportedVersion(_)
            | Error::RequirementsUnmet { .. }
            | Error::Internal { .. }
            | Error::Determinism(_) => ErrorKind::Internal,
        }
    }

    fn fields(&self) -> Vec<(String, promptforge_lua::ErrorField)> {
        use promptforge_lua::ErrorField;
        match self {
            Error::ContextExhausted { reason } => {
                vec![(
                    "reason".to_owned(),
                    ErrorField::String(reason.tag().to_owned()),
                )]
            }
            Error::EmptyModelReply {
                finish_reason: Some(finish_reason),
                ..
            } => vec![(
                "finish_reason".to_owned(),
                ErrorField::String(finish_reason.clone()),
            )],
            Error::OutOfScopeToolCall { name, .. } | Error::UnboundToolCall { name, .. } => {
                vec![("name".to_owned(), ErrorField::String(name.clone()))]
            }
            Error::TasksLive { tasks } => {
                vec![("tasks".to_owned(), ErrorField::String(join_task_ids(tasks)))]
            }
            Error::TaskNotOwned { task }
            | Error::TaskConsumed { task }
            | Error::TaskCancelled { task } => {
                vec![("task".to_owned(), ErrorField::String(task.to_string()))]
            }
            Error::Store { source, .. } => promptforge_lua::store_error_value_fields(source),
            _ => Vec::new(),
        }
    }
}
