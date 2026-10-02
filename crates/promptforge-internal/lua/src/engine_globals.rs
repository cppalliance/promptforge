//! Engine globals installed into every section VM: `log`, `untrusted`, `ui`, and the `store` table.

#[path = "engine_globals-store.rs"]
mod store;

use std::fmt::Write as _;

use super::{
    Access, Arc, AtomicU32, AtomicUsize, Emitter, Error, GuardNonce, LUA_LOG_CHARACTER_LIMIT, Lua,
    LuaSerdeExt, MultiValue, Ordering, Result, Value,
};

pub use store::store_error_message;
pub(crate) use store::{install_store_table, route_store_to_shims};

/// Shared body of the persistent per-section `log(message)` Engine function.
fn log_checkpoint(
    emitter: &Emitter,
    section: &str,
    log_budget: &AtomicU32,
    log_byte_budget: &AtomicUsize,
    arguments: MultiValue,
) -> mlua::Result<()> {
    if arguments.len() != 1 {
        return Err(mlua::Error::external("log expects exactly one argument"));
    }
    // Spend one unit of the per-VM log budget before doing any work; an
    // exhausted budget refuses further checkpoints (lua 002).
    if log_budget
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
        .is_err()
    {
        return Err(mlua::Error::external(crate::error::lua_quota::LOG_EVENT));
    }
    let Some(Value::String(message)) = arguments.into_iter().next() else {
        return Err(mlua::Error::external("log message must be a UTF-8 string"));
    };
    let message = message
        .to_str()
        .map_err(|_| mlua::Error::external("log message must be a UTF-8 string"))?;
    if message.chars().count() > LUA_LOG_CHARACTER_LIMIT {
        return Err(mlua::Error::external(
            "log message must be at most 256 characters",
        ));
    }
    if message.chars().any(is_log_line_break_or_control) {
        return Err(mlua::Error::external(
            "log message must not contain newline or control characters",
        ));
    }
    // Enforce a cumulative byte ceiling in addition to the event count,
    // so many small events cannot emit unbounded total log volume
    // (lua 002).
    if log_byte_budget
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
            remaining.checked_sub(message.len())
        })
        .is_err()
    {
        return Err(mlua::Error::external(crate::error::lua_quota::LOG_BYTE));
    }
    emitter.lua(section, &message);
    Ok(())
}

/// Installs `log` as a persistent global valid for the section's whole
/// lifecycle. The closure captures owned strings and Arc clones, so it
/// outlives any single chunk without an [`mlua::Scope`].
pub(crate) fn install_log(
    lua: &Lua,
    emitter: &Emitter,
    section: &str,
    log_budget: &Arc<AtomicU32>,
    log_byte_budget: &Arc<AtomicUsize>,
) -> Result<()> {
    let section = section.to_owned();
    let emitter = emitter.clone();
    let log_budget = Arc::clone(log_budget);
    let log_byte_budget = Arc::clone(log_byte_budget);
    let log = lua
        .create_function(move |_, arguments: MultiValue| {
            log_checkpoint(&emitter, &section, &log_budget, &log_byte_budget, arguments)
        })
        .map_err(Error::lua)?;
    lua.globals().raw_set("log", log).map_err(Error::lua)
}

pub(crate) fn is_log_line_break_or_control(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Installs `untrusted(s)` as a persistent global valid for the section's
/// whole lifecycle. The closure captures an owned clone of the run's nonce -
/// mlua's `create_function` requires `Fn + Send + 'static`, so no borrow can
/// cross the install - and every wrap the VM performs shares that one nonce.
/// Every string input succeeds, so the install needs no emitter, budget, or
/// [`mlua::Scope`]; a non-string argument fails through mlua's automatic
/// type error.
pub(crate) fn install_untrusted(lua: &Lua, nonce: &GuardNonce) -> Result<()> {
    let nonce = nonce.clone();
    let untrusted = lua
        .create_function(move |_, s: String| Ok(nonce.wrap(&s)))
        .map_err(Error::lua)?;
    lua.globals()
        .raw_set("untrusted", untrusted)
        .map_err(Error::lua)
}

/// `ui()` snapshot conversion: a JSON null field reads as nil in author
/// code, never as the userdata NULL sentinel the serde bridge defaults
/// to - an unset Host field must simply be absent.
const UI_SNAPSHOT_OPTIONS: mlua::serde::SerializeOptions = mlua::serde::SerializeOptions::new()
    .serialize_none_to_null(false)
    .serialize_unit_to_null(false);

/// Installs `ui()` as a persistent global valid for the section's whole
/// lifecycle: each call converts the Host's `snapshot` afresh into a new
/// table, JSON nulls reading as nil, so author code that mutates one
/// result never sees the mutation on the next call. The snapshot is the
/// Host state as the Host captured it at run start; a change on the Host
/// takes effect on the next run. A run whose Host supplies no snapshot
/// never installs the global, so `ui` is absent there - not stubbed.
///
/// The snapshot arrives shared: one run installs it into every section VM
/// it starts, and the closure serializes through the `Arc`, so no VM holds
/// its own copy of the JSON tree.
///
/// # Errors
/// Returns [`Error::Lua`] if the function or the global cannot be created.
pub fn install_ui(lua: &Lua, snapshot: Arc<serde_json::Value>) -> Result<()> {
    let snapshot = lua
        .create_function(move |lua, ()| lua.to_value_with(snapshot.as_ref(), UI_SNAPSHOT_OPTIONS))
        .map_err(Error::lua)?;
    lua.globals().raw_set("ui", snapshot).map_err(Error::lua)
}

/// Shared body of the persistent per-section `store.read` Engine function.
///
/// No `start` reads the whole file; a present `start` slices a 1-based
/// inclusive line range. A negative bound converts to 0, which the range
/// validation rejects with the same error a zero bound produces, and an
/// `end` without a `start` is refused rather than silently ignored.
fn read_store_bounded(
    view: &Access,
    path: &str,
    start: Option<i64>,
    end: Option<i64>,
    numbered: bool,
) -> std::result::Result<String, promptforge_vfs::VfsError> {
    match start {
        None if end.is_none() => {
            if numbered {
                read_range_numbered(view, path, 1, None)
            } else {
                view.read_string(path)
            }
        }
        None => Err(promptforge_vfs::VfsError::InvalidRange {
            path: path.to_owned(),
            reason: "start is required when end is given",
        }),
        Some(start) => {
            let start = usize::try_from(start).unwrap_or(0);
            let end = end.map(|line| usize::try_from(line).unwrap_or(0));
            if numbered {
                read_range_numbered(view, path, start, end)
            } else {
                read_range(view, path, start, end)
            }
        }
    }
}

/// Shared body of the persistent per-section `store.read` Engine function.
fn read_store(
    view: &Access,
    path: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> std::result::Result<String, promptforge_vfs::VfsError> {
    read_store_bounded(view, path, start, end, false)
}

/// Shared body of the persistent per-section `store.read_numbered` callback.
fn read_store_numbered(
    view: &Access,
    path: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> std::result::Result<String, promptforge_vfs::VfsError> {
    read_store_bounded(view, path, start, end, true)
}

/// Reads lines `start..=end` of the file at `path`, 1-based and inclusive,
/// joined with `"\n"` and no trailing newline.
fn read_range(
    view: &Access,
    path: &str,
    start: usize,
    end: Option<usize>,
) -> std::result::Result<String, promptforge_vfs::VfsError> {
    with_read_range(view, path, start, end, |lines, _| lines.join("\n"))
}

/// Reads lines `start..=end` of the file at `path` numbered absolutely
/// from `start`, each number right-aligned to the width of the largest
/// emitted number, followed by `"| "`.
fn read_range_numbered(
    view: &Access,
    path: &str,
    start: usize,
    end: Option<usize>,
) -> std::result::Result<String, promptforge_vfs::VfsError> {
    with_read_range(view, path, start, end, number_lines_from)
}

/// Reads and resolves one line range while its owned contents remain live.
fn with_read_range(
    view: &Access,
    path: &str,
    start: usize,
    end: Option<usize>,
    render: impl FnOnce(&[&str], usize) -> String,
) -> std::result::Result<String, promptforge_vfs::VfsError> {
    let contents = view.read_string(path)?;
    let lines: Vec<&str> = contents.lines().collect();
    let Some((start, end)) = resolve_line_range(path, lines.len(), start, end)? else {
        return Ok(String::new());
    };
    Ok(render(&lines[start - 1..end], start))
}

/// Resolves 1-based inclusive bounds against `line_count` into the
/// effective `(start, end)`, or `None` when the range falls entirely past
/// the last line. Evaluation order is fixed: a `start` below 1 is an
/// error; a `start` past the last line reads as empty; an omitted `end`
/// means the last line, and a given `end` clamps down to it; an `end`
/// before `start` at that point is an error.
fn resolve_line_range(
    path: &str,
    line_count: usize,
    start: usize,
    end: Option<usize>,
) -> std::result::Result<Option<(usize, usize)>, promptforge_vfs::VfsError> {
    if start == 0 {
        return Err(promptforge_vfs::VfsError::InvalidRange {
            path: path.to_owned(),
            reason: "start must be at least 1",
        });
    }
    if start > line_count {
        return Ok(None);
    }
    let end = end.unwrap_or(line_count).min(line_count);
    if end < start {
        return Err(promptforge_vfs::VfsError::InvalidRange {
            path: path.to_owned(),
            reason: "end must not be before start",
        });
    }
    Ok(Some((start, end)))
}

/// Renders `lines` numbered absolutely from `start`, each number
/// right-aligned to the width of the largest emitted number, followed by
/// `"| "`; lines are joined with `"\n"` and there is no trailing newline.
fn number_lines_from(lines: &[&str], start: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let last = start + lines.len() - 1;
    let width = last.to_string().len();
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let number = start + index;
        // Writing to a String is infallible, so the result is discarded.
        let _ = write!(out, "{number:>width$}| {line}");
    }
    out
}

/// Executes one validated store operation against the store view: the
/// executor's leaf-yield dispatch for a `store.*` call inside a block.
///
/// The direct closures `install_store_table` builds are a second path.
/// They run only while the shared library loads, before
/// `route_store_to_shims` switches the table to the yield shims. They share
/// this function's operation bodies (the same [`Access`] calls and read
/// helpers) and add lifecycle events, store-conflict recording, and
/// `Error::store` wrapping. The bounded-read argument rules (a negative
/// bound converts to 0, an `end` without a `start` is refused) sit in the
/// shared `read_store_bounded` helper above. `view` is the store view the
/// caller derived; every operation maps onto one [`Access`] call over it.
///
/// # Errors
/// Returns the [`VfsError`](promptforge_vfs::VfsError) the operation
/// produces: path or pattern validation, not-found, anchor, range,
/// conflict, or backend failure.
pub fn run_store_op(
    view: &Access,
    op: crate::protocol::VfsOp,
) -> std::result::Result<crate::protocol::VfsOutcome, promptforge_vfs::VfsError> {
    use crate::protocol::{VfsOp, VfsOutcome};
    match op {
        VfsOp::Write { path, contents } => view
            .write(&path, contents.as_bytes())
            .map(|()| VfsOutcome::Unit),
        VfsOp::Append { path, contents } => view
            .append(&path, contents.as_bytes())
            .map(|()| VfsOutcome::Unit),
        VfsOp::Read { path, start, end } => {
            read_store(view, &path, start, end).map(VfsOutcome::Text)
        }
        VfsOp::ReadNumbered { path, start, end } => {
            read_store_numbered(view, &path, start, end).map(VfsOutcome::Text)
        }
        VfsOp::StrReplace { path, old, new } => view
            .str_replace(&path, &old, &new)
            .map(|()| VfsOutcome::Unit),
        VfsOp::Delete { path } => view.remove(&path, false).map(|_| VfsOutcome::Unit),
        VfsOp::Glob { pattern } => view.glob(&pattern).map(VfsOutcome::Paths),
        VfsOp::Exists { path } => view.exists(&path).map(VfsOutcome::Bool),
    }
}
