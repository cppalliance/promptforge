//! The `store` table: Lua dispatchers over the direct store closures,
//! and the switch that routes them to the yield shims once the shared
//! library has loaded.

use std::sync::LazyLock;

use mlua::Table;
use promptforge_types::event::lifecycle::Lifecycle;

use crate::protocol::StoreOp;

use super::{read_store, read_store_numbered};
use crate::{
    Access, Arc, Emitter, Error, Function, Lua, LuaProgram, Mutex, Result, SharedSource, lifecycle,
};

/// The store dispatcher chunk's name, `@`-prefixed as the shim chunks'
/// names are, so its frames render as verbatim `file:line:` references.
const STORE_CHUNK_NAME: &str = "@crates/promptforge-internal/lua/src/__impl_store.lua";

/// The store dispatcher source, embedded verbatim so chunk line 1 is file
/// line 1.
const STORE_SOURCE: &str = include_str!("__impl_store.lua");

/// The registry key of the store dispatchers' phase table, kept by
/// [`install_store_table`] so [`route_store_to_shims`] can switch every
/// dispatcher to the yield shims once the shared library has loaded.
const STORE_PHASE_REGISTRY: &str = "promptforge.engine.store_phase";

/// The store dispatcher program, compiled once and loaded per VM under the
/// shim programs' failure contract: compiling the bundled source fails
/// only on a crate bug.
static STORE_PROGRAM: LazyLock<std::result::Result<LuaProgram, SharedSource>> =
    LazyLock::new(|| {
        LuaProgram::compile_internal(STORE_SOURCE, STORE_CHUNK_NAME)
            .map_err(crate::detail::shared_source_new)
    });

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

/// Records a store conflict in the shared slot the Engine reads when a
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

/// Exposes an always-on `store` table whose methods (`write`, `append`,
/// `read`, `read_numbered`, `str_replace`, `delete`,
/// `glob`, `exists`) are backed by the store view derived from the
/// caller's VFS access capability, so each operation runs under the
/// caller's own identity and the store's strict path rules.
/// Installed once per section with [`Lua::create_function`], so the table
/// stays valid across every chunk the VM runs without a live [`mlua::Scope`].
///
/// The table is a deterministic Engine global, present regardless of tool
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
/// install - is recorded in `conflicts`, the shared slot the Engine reads
/// when the load returns, so the run ends with a determinism violation
/// even if author code caught the error.
///
/// The table's function values are Lua dispatchers over those closures,
/// not the closures themselves: each runs its closure until
/// [`route_store_to_shims`] switches it to the matching yield shim. So a
/// function the shared library captured at load time runs directly during
/// the load and yields like `store.*` afterward.
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
    let program = STORE_PROGRAM.as_ref().map_err(Error::shared)?;
    let (dispatcher, phase): (Function, Table) = program.load(lua)?.call(()).map_err(Error::lua)?;
    lua.set_named_registry_value(STORE_PHASE_REGISTRY, phase)
        .map_err(Error::lua)?;
    let install = |name: &str, direct: Function| -> Result<()> {
        let function: Function = dispatcher.call((name, direct)).map_err(Error::lua)?;
        table.raw_set(name, function).map_err(Error::lua)
    };

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
            install($name, function)?;
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
    install("glob", glob)?;

    let handle = Arc::clone(&view);
    let exists_conflicts = Arc::clone(conflicts);
    let report = Arc::clone(&reporter);
    let exists = lua
        .create_function(move |_, path: String| {
            let result = handle.exists(&path);
            record_store_conflict(&exists_conflicts, &result);
            report.report(
                result.is_ok(),
                lifecycle::STORE_EXISTS_SUCCEEDED,
                lifecycle::STORE_EXISTS_FAILED,
            );
            result.map_err(|source| {
                mlua::Error::external(Error::store(
                    &StoreOp::Exists { path: path.clone() },
                    source,
                ))
            })
        })
        .map_err(Error::lua)?;
    install("exists", exists)?;

    globals.raw_set("store", table).map_err(Error::lua)?;
    Ok(())
}

/// Switches every store dispatcher [`install_store_table`] built on this
/// VM to the yield shims in `shims`, whichever reference the prompt
/// holds: a function the shared library captured at load time yields
/// from here on exactly as `store.*` does.
///
/// # Errors
/// Returns [`Error::Lua`] if the VM's store table was never installed or
/// the switch fails.
pub(crate) fn route_store_to_shims(lua: &Lua, shims: &Table) -> Result<()> {
    let phase: Table = lua
        .named_registry_value(STORE_PHASE_REGISTRY)
        .map_err(Error::lua)?;
    phase.raw_set("shims", shims.clone()).map_err(Error::lua)
}
