//! Host callbacks installed into every section VM: `log`, `untrusted`, `ui`, and the `store` table.

use std::fmt::Write as _;

use promptforge_types::event::lifecycle::Lifecycle;

use crate::protocol::StoreOp;

use super::{
    Access, Arc, AtomicU32, AtomicUsize, Emitter, Error, GuardNonce, LUA_LOG_CHARACTER_LIMIT, Lua,
    LuaSerdeExt, MultiValue, Mutex, Ordering, Result, Value, lifecycle,
};

/// Shared body of the persistent per-section `log(message)` host callback.
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
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
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
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
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
/// to - an unset host field must simply be absent.
const UI_SNAPSHOT_OPTIONS: mlua::serde::SerializeOptions = mlua::serde::SerializeOptions::new()
    .serialize_none_to_null(false)
    .serialize_unit_to_null(false);

/// Installs `ui()` as a persistent global valid for the section's whole
/// lifecycle: each call converts the host's `snapshot` afresh into a new
/// table, JSON nulls reading as nil, so author code that mutates one
/// result never sees the mutation on the next call. The snapshot is the
/// host state as the host captured it at run start; a change on the host
/// takes effect on the next run. A run whose host supplies no snapshot
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

/// Owned reporting context captured by the persistent `store` closures.
struct StoreReporter {
    emitter: Emitter,
    section: String,
}

impl StoreReporter {
    fn report(&self, succeeded: bool, success: Lifecycle, failure: Lifecycle) {
        self.emitter
            .report(&self.section, if succeeded { success } else { failure });
    }
}

/// The model-facing message for a failed store operation: what failed and,
/// when there is a fix, how to make it, so the text a model reads names
/// both. The operation supplies the wording its call surface promises (a
/// glob reports "invalid glob pattern", a path reports "invalid path"), and
/// the structured [`VfsError`](promptforge_vfs::VfsError) supplies the
/// path, rule, anchor, and count.
#[must_use]
pub fn store_error_message(
    op: &crate::protocol::StoreOp,
    error: &promptforge_vfs::VfsError,
) -> String {
    use promptforge_vfs::VfsError;
    match error {
        VfsError::NotFound { path } => format!("file not found in store: {path}"),
        VfsError::AlreadyExists { path } => format!("file already exists in store: {path}"),
        VfsError::NotADirectory { path } => format!("not a directory in store: {path}"),
        VfsError::IsADirectory { path } => format!("is a directory in store: {path}"),
        VfsError::DirectoryNotEmpty { path } => format!("directory not empty in store: {path}"),
        VfsError::NotUtf8 { path } => format!("file in store is not UTF-8: {path}"),
        VfsError::InvalidPath { path, reason } => {
            if matches!(op, crate::protocol::StoreOp::Glob { .. }) {
                format!("invalid glob pattern {path:?}: {reason}")
            } else {
                format!("invalid path {path:?}: {reason}")
            }
        }
        VfsError::InvalidRange { path, reason } => {
            format!("invalid line range for {path}: {reason}")
        }
        VfsError::Anchor {
            path,
            anchor,
            count,
        } => {
            if anchor.is_empty() {
                format!("str_replace requires a non-empty anchor: {path}")
            } else if *count == 0 {
                format!("anchor {anchor:?} was not found in {path}, expected exactly one")
            } else {
                format!(
                    "anchor {anchor:?} occurs {count} times in {path}, expected exactly one; \
                     include more surrounding text so it matches once"
                )
            }
        }
        VfsError::PermissionDenied { path, reason } => {
            format!("permission denied for store path {path}: {reason}")
        }
        VfsError::Unsupported { path, detail } => {
            format!("unsupported store operation on {path}: {detail}")
        }
        VfsError::Conflict { path, detail } => format!("store conflict on {path}: {detail}"),
        VfsError::Backend { message } => format!("store backend failure: {message}"),
        _ => "store operation failed".to_owned(),
    }
}

/// Records a store conflict in the shared slot the engine reads when a
/// shared library load returns: a conflict the author caught with `pcall`
/// must still end the run with a determinism violation, so the direct
/// closures write it here rather than only raising it.
fn record_store_conflict<T>(
    conflicts: &Mutex<Option<String>>,
    result: &std::result::Result<T, promptforge_vfs::VfsError>,
) {
    if let Err(promptforge_vfs::VfsError::Conflict { detail, .. }) = result
        && let Ok(mut slot) = conflicts.lock()
    {
        *slot = Some(detail.clone());
    }
}

/// Shared body of the persistent per-section `store.read` host callback.
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

/// Shared body of the persistent per-section `store.read` host callback.
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

/// Exposes an always-on `store` table whose methods (`write`, `append`,
/// `read`, `read_numbered`, `str_replace`, `delete`,
/// `glob`, `exists`) are backed by the store view derived from the
/// caller's VFS access capability, so each operation runs under the
/// caller's own identity and the store's strict path rules.
/// Installed once per section with [`Lua::create_function`], so the table
/// stays valid across every chunk the VM runs without a live [`mlua::Scope`].
///
/// The table is a deterministic host capability, present regardless of tool
/// scoping. The mutating ops (`write`/`append`/`str_replace`/`delete`) return
/// nil; `read` returns the file verbatim, optionally bounded to a 1-based
/// inclusive line range (`read(path, start)` reads to end of file,
/// `read(path, start, end)` slices); `read_numbered` returns it with
/// absolute line numbers under the same optional bounds; `glob` returns an
/// array table of matching paths. A [`VfsError`] from any op is raised as
/// a structured error value of kind `store` via
/// [`Error::store`], so it aborts the chunk and a `pcall` caller reads
/// `err.kind`, `err.reason`, and the variant's fields.
///
/// Every closure captures an `Arc` clone of the section's store view.
/// The view locks the backend per call and is synchronous, so nothing is
/// held across an await. The view carries the caller's identity, which is
/// what the claims model attributes operations to: a fanout arm's view is
/// spawned from the caller's, so two unordered arms touching one path
/// surface the conflict as [`VfsError::Conflict`]. A conflict these direct
/// closures see - during a shared library load, before the yield shims
/// install - is recorded in `conflicts`, the shared slot the engine reads
/// when the load returns, so the run ends with a determinism violation
/// even if author code caught the error.
///
/// [`VfsError`]: promptforge_vfs::VfsError
///
/// # Errors
/// Returns [`Error::Lua`] if the `store` table or any of its functions cannot
/// be created or installed into the sandbox globals, or
/// [`Error::Store`] if the handle the access came from declares no store.
#[expect(
    clippy::too_many_lines,
    reason = "one table installs all store operations beside their matching observation outcomes"
)]
pub(crate) fn install_store_table(
    lua: &Lua,
    globals: &mlua::Table,
    access: &Arc<Access>,
    emitter: &Emitter,
    section: &str,
    conflicts: &Arc<Mutex<Option<String>>>,
) -> Result<()> {
    let table = lua.create_table().map_err(Error::lua)?;
    let reporter = Arc::new(StoreReporter {
        emitter: emitter.clone(),
        section: section.to_owned(),
    });
    // One store view for the section's whole life: derived once from the
    // chain's access, so the closures share it and the strict path rules
    // run on every operation through it.
    let view = Arc::new(
        promptforge_vfs::detail::store_view(access).map_err(|source| Error::Store {
            message: "store operation failed".to_owned(),
            source,
        })?,
    );

    macro_rules! install_reported_store_fn {
        (
            $name:literal,
            $handle:ident,
            $arguments:pat_param,
            $argument_type:ty,
            $success:expr,
            $failure:expr,
            $op:expr,
            $operation:block
        ) => {{
            let $handle = Arc::clone(&view);
            let report = Arc::clone(&reporter);
            let conflicts = Arc::clone(conflicts);
            let function = lua
                .create_function(move |_, $arguments: $argument_type| {
                    let result = $operation;
                    record_store_conflict(&conflicts, &result);
                    report.report(result.is_ok(), $success, $failure);
                    result.map_err(|source| mlua::Error::external(Error::store(&$op, source)))
                })
                .map_err(Error::lua)?;
            table.set($name, function).map_err(Error::lua)?;
        }};
    }

    install_reported_store_fn!(
        "write",
        handle,
        (path, contents),
        (String, String),
        lifecycle::STORE_WRITE_SUCCEEDED,
        lifecycle::STORE_WRITE_FAILED,
        StoreOp::Write {
            path: path.clone(),
            contents: contents.clone(),
        },
        { handle.write(&path, contents.as_bytes()) }
    );
    install_reported_store_fn!(
        "append",
        handle,
        (path, contents),
        (String, String),
        lifecycle::STORE_APPEND_SUCCEEDED,
        lifecycle::STORE_APPEND_FAILED,
        StoreOp::Append {
            path: path.clone(),
            contents: contents.clone(),
        },
        { handle.append(&path, contents.as_bytes()) }
    );
    install_reported_store_fn!(
        "read",
        handle,
        (path, start, end),
        (String, Option<i64>, Option<i64>),
        lifecycle::STORE_READ_SUCCEEDED,
        lifecycle::STORE_READ_FAILED,
        StoreOp::Read {
            path: path.clone(),
            start,
            end,
        },
        { read_store(&handle, &path, start, end) }
    );
    install_reported_store_fn!(
        "read_numbered",
        handle,
        (path, start, end),
        (String, Option<i64>, Option<i64>),
        lifecycle::STORE_READ_NUMBERED_SUCCEEDED,
        lifecycle::STORE_READ_NUMBERED_FAILED,
        StoreOp::ReadNumbered {
            path: path.clone(),
            start,
            end,
        },
        { read_store_numbered(&handle, &path, start, end) }
    );
    install_reported_store_fn!(
        "str_replace",
        handle,
        (path, old, new),
        (String, String, String),
        lifecycle::STORE_REPLACE_SUCCEEDED,
        lifecycle::STORE_REPLACE_FAILED,
        StoreOp::StrReplace {
            path: path.clone(),
            old: old.clone(),
            new: new.clone(),
        },
        { handle.str_replace(&path, &old, &new) }
    );
    install_reported_store_fn!(
        "delete",
        handle,
        path,
        String,
        lifecycle::STORE_DELETE_SUCCEEDED,
        lifecycle::STORE_DELETE_FAILED,
        StoreOp::Delete { path: path.clone() },
        { handle.remove(&path, false).map(|_| ()) }
    );

    let handle = Arc::clone(&view);
    let report = Arc::clone(&reporter);
    let glob_conflicts = Arc::clone(conflicts);
    let glob = lua
        .create_function(move |lua, pattern: String| {
            let result = handle.glob(&pattern);
            record_store_conflict(&glob_conflicts, &result);
            report.report(
                result.is_ok(),
                lifecycle::STORE_GLOB_SUCCEEDED,
                lifecycle::STORE_GLOB_FAILED,
            );
            let paths = result.map_err(|source| {
                mlua::Error::external(Error::store(
                    &StoreOp::Glob {
                        pattern: pattern.clone(),
                    },
                    source,
                ))
            })?;
            lua.create_sequence_from(paths)
        })
        .map_err(Error::lua)?;
    table.set("glob", glob).map_err(Error::lua)?;

    let handle = Arc::clone(&view);
    let exists_conflicts = Arc::clone(conflicts);
    let exists = lua
        .create_function(move |_, path: String| {
            let result = handle.exists(&path);
            record_store_conflict(&exists_conflicts, &result);
            result.map_err(|source| {
                mlua::Error::external(Error::store(
                    &StoreOp::Exists { path: path.clone() },
                    source,
                ))
            })
        })
        .map_err(Error::lua)?;
    table.set("exists", exists).map_err(Error::lua)?;

    globals.raw_set("store", table).map_err(Error::lua)?;
    Ok(())
}

/// Executes one validated store operation against the store view: the
/// single implementation behind both the direct closures and the
/// executor's leaf-yield dispatch, so the two paths cannot drift. The
/// bounded-read argument rules (a negative bound converts to 0, an `end`
/// without a `start` is refused) sit in the shared `read_store_bounded`
/// helper above; the read ops route through their named wrappers as the
/// closures do. `view` is the store view the caller derived; every
/// operation maps onto one [`Access`] call over it.
///
/// # Errors
/// Returns the [`VfsError`](promptforge_vfs::VfsError) the operation
/// produces: path or pattern validation, not-found, anchor, range,
/// conflict, or backend failure.
pub fn run_store_op(
    view: &Access,
    op: crate::protocol::StoreOp,
) -> std::result::Result<crate::protocol::StoreOutcome, promptforge_vfs::VfsError> {
    use crate::protocol::{StoreOp, StoreOutcome};
    match op {
        StoreOp::Write { path, contents } => view
            .write(&path, contents.as_bytes())
            .map(|()| StoreOutcome::Unit),
        StoreOp::Append { path, contents } => view
            .append(&path, contents.as_bytes())
            .map(|()| StoreOutcome::Unit),
        StoreOp::Read { path, start, end } => {
            read_store(view, &path, start, end).map(StoreOutcome::Text)
        }
        StoreOp::ReadNumbered { path, start, end } => {
            read_store_numbered(view, &path, start, end).map(StoreOutcome::Text)
        }
        StoreOp::StrReplace { path, old, new } => view
            .str_replace(&path, &old, &new)
            .map(|()| StoreOutcome::Unit),
        StoreOp::Delete { path } => view.remove(&path, false).map(|_| StoreOutcome::Unit),
        StoreOp::Glob { pattern } => view.glob(&pattern).map(StoreOutcome::Paths),
        StoreOp::Exists { path } => view.exists(&path).map(StoreOutcome::Bool),
    }
}
