//! The block guard's failure stash: the capture that records a guarded
//! block's raised value and raise-point traceback, and the take that reads
//! them back when the failure reaches the Engine.

use mlua::{Function, Lua, Table, Value};

use crate::error::{Error, Result};
use crate::error_value::{Raised, raised_from};

/// The registry key of the last value a guarded block raised, written by
/// the guard's `stash_failure` capture from the message handler at the
/// raise point and taken by [`take_failure`] when the failure reaches the
/// Engine.
const FAILURE_REGISTRY: &str = "runvil.impl_coro.failure";

/// The registry key of the traceback recorded beside the stashed failure:
/// the coroutine's stack at the raise point, before the guard's `xpcall`
/// unwinds the block's frames. The guard's re-raise happens after that
/// unwinding, so the traceback mlua appends to the re-raised error shows
/// only the guard's own frame; this one holds the author's.
const FAILURE_TRACEBACK_REGISTRY: &str = "runvil.impl_coro.failure_traceback";

/// Builds the prelude's `stash_failure(failure)` capture: records the
/// block's raised value and its raise-point traceback for [`take_failure`].
pub(super) fn stash_failure_capture(lua: &Lua) -> Result<Function> {
    lua.create_function(|lua, failure: Value| {
        // Called from the guard's message handler, so the failing
        // frames are still on this coroutine's stack: level 1 starts
        // the traceback at the handler, above this capture's own frame.
        let traceback = lua.traceback(None, 1)?;
        lua.set_named_registry_value(FAILURE_TRACEBACK_REGISTRY, traceback)?;
        lua.set_named_registry_value(FAILURE_REGISTRY, failure)
    })
    .map_err(Error::lua)
}

/// What the guard stashed for the last failure of a guarded block.
#[derive(Debug, Default)]
pub(crate) struct StashedFailure {
    /// The raised value read back as a [`Raised`] when it is a structured
    /// error table built on this VM; `None` when it was a plain string, an
    /// author's own table, or a Rust callback's wrapped failure.
    pub(crate) raised: Option<Raised>,
    /// The raised value itself when it was a table, which identifies the
    /// raise of one particular table.
    table: Option<Table>,
    /// The coroutine's traceback at the raise point, `stack traceback:`
    /// heading included, when the handler recorded one.
    traceback: Option<String>,
}

impl StashedFailure {
    /// Whether the block raised `table` itself, not merely a table with
    /// the same contents.
    pub(crate) fn raised_table_is(&self, table: &Table) -> bool {
        self.table
            .as_ref()
            .is_some_and(|raised| raised.to_pointer() == table.to_pointer())
    }

    /// Restores the raise-point traceback onto a Lua-raised failure.
    ///
    /// mlua appends the coroutine's traceback to a runtime error when the
    /// coroutine dies, but the guard's re-raise is what kills it, so that
    /// traceback shows the guard's frame and nothing of the block's. When
    /// this stash recorded the real one, the appended tail is replaced with
    /// it, so the line mapper sees the author's frames. A Rust callback's
    /// wrapped failure has its own traceback and is left untouched.
    pub(crate) fn restore_traceback<'e>(
        &self,
        error: &'e mlua::Error,
    ) -> std::borrow::Cow<'e, mlua::Error> {
        let (mlua::Error::RuntimeError(message), Some(traceback)) = (error, &self.traceback) else {
            return std::borrow::Cow::Borrowed(error);
        };
        let head = message
            .rfind("\nstack traceback:")
            .map_or(message.as_str(), |at| &message[..at]);
        std::borrow::Cow::Owned(mlua::Error::RuntimeError(format!("{head}\n{traceback}")))
    }
}

/// Takes what the guard stashed for the last failure of a guarded block.
/// Both slots are cleared on every call, so a later failure never sees a
/// stale value; the default (nothing raised, no traceback) when nothing
/// was stashed.
///
/// # Errors
/// Returns [`Error::LuaRuntime`] if a registry slot cannot be read or
/// cleared.
pub(crate) fn take_failure(lua: &Lua) -> Result<StashedFailure> {
    let failure: Value = lua
        .named_registry_value(FAILURE_REGISTRY)
        .map_err(Error::lua)?;
    let traceback: Value = lua
        .named_registry_value(FAILURE_TRACEBACK_REGISTRY)
        .map_err(Error::lua)?;
    if matches!(failure, Value::Nil) {
        return Ok(StashedFailure::default());
    }
    lua.set_named_registry_value(FAILURE_REGISTRY, Value::Nil)
        .map_err(Error::lua)?;
    lua.set_named_registry_value(FAILURE_TRACEBACK_REGISTRY, Value::Nil)
        .map_err(Error::lua)?;
    let traceback = match traceback {
        Value::String(text) => Some(text.to_str().map_err(Error::lua)?.to_owned()),
        _ => None,
    };
    let raised = raised_from(lua, &failure).map_err(Error::lua)?;
    let table = match failure {
        Value::Table(table) => Some(table),
        _ => None,
    };
    Ok(StashedFailure {
        raised,
        table,
        traceback,
    })
}
