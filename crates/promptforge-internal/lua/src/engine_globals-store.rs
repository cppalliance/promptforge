//! The `store` table: Lua dispatchers over the direct store closures,
//! and the switch that routes them to the yield shims once the shared
//! library has loaded.

use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use mlua::{FromLuaMulti, Function, IntoLuaMulti, Lua, Table};
use promptforge_types::emitter::Emitter;
use promptforge_types::event::lifecycle::{self, Lifecycle};
use promptforge_vfs::{Access, VfsError};

use super::{read_store, read_store_numbered};
use crate::SharedSource;
use crate::error::{Error, Result};
use crate::program::LuaProgram;
use crate::protocol::VfsOp;

/// The store dispatcher chunk's name, `@`-prefixed as the shim chunks'
/// names are, so its frames render as verbatim `file:line:` references.
const STORE_CHUNK_NAME: &str = "@crates/promptforge-internal/lua/src/__impl_store.lua";

/// The store dispatcher source, embedded verbatim so chunk line 1 is file
/// line 1.
const STORE_SOURCE: &str = include_str!("__impl_store.lua");

/// The registry key of the store dispatchers' phase table, kept by
/// [`install_store_table`] so [`route_store_to_shims`] can switch every
/// dispatcher to the yield shims once the Plugin preludes have installed.
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
    /// Reports `success` for an `Ok` result and `failure` for an `Err`.
    fn report<T>(
        &self,
        result: &std::result::Result<T, VfsError>,
        success: Lifecycle,
        failure: Lifecycle,
    ) {
        self.emitter.report(
            &self.section,
            if result.is_ok() { success } else { failure },
        );
    }
}

/// The Lua arguments of `read` and `read_numbered`: the path and the
/// optional 1-based inclusive line bounds.
type Bounds = (String, Option<i64>, Option<i64>);

/// What every direct store closure shares: the section's one store view,
/// the reporter, and the conflict slot.
struct StoreShared {
    view: Arc<Access>,
    reporter: Arc<StoreReporter>,
    conflicts: Arc<Mutex<Option<String>>>,
}

impl StoreShared {
    /// Builds one direct store closure taking the Lua arguments `A`: it runs
    /// `operation` over the store view, records a conflict, reports
    /// `outcomes` (success first), and raises a failure as
    /// [`Error::store`] under the operation `op` rebuilds from the
    /// arguments.
    fn closure<A, T>(
        &self,
        lua: &Lua,
        outcomes: (Lifecycle, Lifecycle),
        operation: impl Fn(&Access, &A) -> std::result::Result<T, VfsError> + Send + 'static,
        op: impl Fn(A) -> VfsOp + Send + 'static,
    ) -> Result<Function>
    where
        A: FromLuaMulti + 'static,
        T: IntoLuaMulti + 'static,
    {
        let view = Arc::clone(&self.view);
        let reporter = Arc::clone(&self.reporter);
        let conflicts = Arc::clone(&self.conflicts);
        let (success, failure) = outcomes;
        lua.create_function(move |_, arguments: A| {
            let result = operation(&view, &arguments);
            record_store_conflict(&conflicts, &result);
            reporter.report(&result, success, failure);
            result.map_err(|source| mlua::Error::external(Error::store(&op(arguments), source)))
        })
        .map_err(Error::lua)
    }
}

/// The model-facing message for a failed store operation: what failed and,
/// when there is a fix, how to make it, so the text a model reads names
/// both. The operation supplies the wording its call surface promises (a
/// glob reports "invalid glob pattern", a path reports "invalid path"), and
/// the structured [`VfsError`] supplies the path, rule, anchor, and count.
#[must_use]
pub fn store_error_message(op: &VfsOp, error: &VfsError) -> String {
    match error {
        VfsError::NotFound { path } => format!("file not found in store: {path}"),
        VfsError::AlreadyExists { path } => format!("file already exists in store: {path}"),
        VfsError::NotADirectory { path } => format!("not a directory in store: {path}"),
        VfsError::IsADirectory { path } => format!("is a directory in store: {path}"),
        VfsError::DirectoryNotEmpty { path } => format!("directory not empty in store: {path}"),
        VfsError::NotUtf8 { path, valid_up_to } => {
            format!("file in store is not UTF-8 after byte {valid_up_to}: {path}")
        }
        VfsError::InvalidPath { path, reason } => {
            if matches!(op, VfsOp::Glob { .. }) {
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
        VfsError::ApprovalRequired { path, reason } => {
            format!("store path {path} needs approval, which this run cannot give: {reason}")
        }
        VfsError::ScopeEnded { path } => format!(
            "store access ended: the run or tool call that owned it has ended, so {path} \
             can no longer be touched through it"
        ),
        VfsError::ReadOnly { path } => format!("store path is read-only: {path}"),
        VfsError::OutsideRoot { path, detail } => {
            format!("store path {path} is outside the mounted root: {detail}")
        }
        VfsError::RootProtected { path } => {
            format!("the store's mounted root cannot be removed, renamed, or replaced: {path}")
        }
        VfsError::NoStore => "no store is declared, so no store operation can run".to_owned(),
        VfsError::CrossMount { path, detail } => {
            format!("store operation on {path} spans two mounts: {detail}")
        }
        VfsError::Io { path, message, .. } => format!("store I/O failure on {path}: {message}"),
        VfsError::Unsupported { path, detail } => {
            format!("unsupported store operation on {path}: {detail}")
        }
        VfsError::Conflict { path, detail } => format!("store conflict on {path}: {detail}"),
        VfsError::Backend { message } => format!("store backend failure: {message}"),
        _ => "store operation failed".to_owned(),
    }
}

/// Records a store conflict in the shared slot the Engine reads when the
/// Plugin preludes return: a conflict a prelude caught with `pcall` must
/// still end the run with a determinism violation, so the direct closures
/// write it here rather than only raising it.
fn record_store_conflict<T>(
    conflicts: &Mutex<Option<String>>,
    result: &std::result::Result<T, VfsError>,
) {
    if let Err(VfsError::Conflict { detail, .. }) = result {
        // A panic elsewhere cannot leave the slot half-written, so a
        // poisoned lock still takes the conflict.
        *conflicts.lock().unwrap_or_else(PoisonError::into_inner) = Some(detail.clone());
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
/// closures see - while the Plugin preludes install, before the yield
/// shims install - is recorded in `conflicts`, the shared slot the Engine
/// reads when the preludes return, so the run ends with a determinism
/// violation even if the prelude caught the error.
///
/// The table's function values are Lua dispatchers over those closures,
/// not the closures themselves: each runs its closure until
/// [`route_store_to_shims`] switches it to the matching yield shim. So a
/// function a prelude captured while it installed runs directly during
/// the install and yields like `store.*` afterward.
///
/// # Errors
/// Returns [`Error::LuaRuntime`] if the `store` table or any of its
/// functions cannot be created or installed into the sandbox globals, or
/// [`Error::Store`] if the handle the access came from declares no store.
pub(crate) fn install_store_table(
    lua: &Lua,
    globals: &mlua::Table,
    access: &Arc<Access>,
    emitter: &Emitter,
    section: &str,
    conflicts: &Arc<Mutex<Option<String>>>,
) -> Result<()> {
    let table = lua.create_table().map_err(Error::lua)?;
    // One store view for the section's whole life: derived once from the
    // chain's access, so the closures share it and the strict path rules
    // run on every operation through it.
    let shared = StoreShared {
        view: Arc::new(
            promptforge_vfs::detail::store_view(access).map_err(|cause| Error::Store {
                message: format!("store operation failed: {cause}"),
                cause,
            })?,
        ),
        reporter: Arc::new(StoreReporter {
            emitter: emitter.clone(),
            section: section.to_owned(),
        }),
        conflicts: Arc::clone(conflicts),
    };
    let program = STORE_PROGRAM.as_ref().map_err(Error::shared)?;
    let (dispatcher, phase): (Function, Table) = program.load(lua)?.call(()).map_err(Error::lua)?;
    lua.set_named_registry_value(STORE_PHASE_REGISTRY, phase)
        .map_err(Error::lua)?;
    let install = |name: &str, direct: Function| -> Result<()> {
        let function: Function = dispatcher.call((name, direct)).map_err(Error::lua)?;
        table.raw_set(name, function).map_err(Error::lua)
    };

    let write = shared.closure(
        lua,
        (lifecycle::VFS_WRITE_SUCCEEDED, lifecycle::VFS_WRITE_FAILED),
        |view, (path, contents): &(String, String)| view.write(path, contents.as_bytes()),
        |(path, contents)| VfsOp::Write { path, contents },
    )?;
    install("write", write)?;
    let append = shared.closure(
        lua,
        (
            lifecycle::VFS_APPEND_SUCCEEDED,
            lifecycle::VFS_APPEND_FAILED,
        ),
        |view, (path, contents): &(String, String)| view.append(path, contents.as_bytes()),
        |(path, contents)| VfsOp::Append { path, contents },
    )?;
    install("append", append)?;
    let read = shared.closure(
        lua,
        (lifecycle::VFS_READ_SUCCEEDED, lifecycle::VFS_READ_FAILED),
        |view, (path, start, end): &Bounds| read_store(view, path, *start, *end),
        |(path, start, end)| VfsOp::Read { path, start, end },
    )?;
    install("read", read)?;
    let read_numbered = shared.closure(
        lua,
        (
            lifecycle::VFS_READ_NUMBERED_SUCCEEDED,
            lifecycle::VFS_READ_NUMBERED_FAILED,
        ),
        |view, (path, start, end): &Bounds| read_store_numbered(view, path, *start, *end),
        |(path, start, end)| VfsOp::ReadNumbered { path, start, end },
    )?;
    install("read_numbered", read_numbered)?;
    let str_replace = shared.closure(
        lua,
        (
            lifecycle::VFS_REPLACE_SUCCEEDED,
            lifecycle::VFS_REPLACE_FAILED,
        ),
        |view, (path, old, new): &(String, String, String)| view.str_replace(path, old, new),
        |(path, old, new)| VfsOp::StrReplace { path, old, new },
    )?;
    install("str_replace", str_replace)?;
    let delete = shared.closure(
        lua,
        (
            lifecycle::VFS_DELETE_SUCCEEDED,
            lifecycle::VFS_DELETE_FAILED,
        ),
        |view, path: &String| view.remove(path, false).map(|_| ()),
        |path| VfsOp::Delete { path },
    )?;
    install("delete", delete)?;
    let glob = shared.closure(
        lua,
        (lifecycle::VFS_GLOB_SUCCEEDED, lifecycle::VFS_GLOB_FAILED),
        |view, pattern: &String| view.glob(pattern),
        |pattern| VfsOp::Glob { pattern },
    )?;
    install("glob", glob)?;
    let exists = shared.closure(
        lua,
        (
            lifecycle::VFS_EXISTS_SUCCEEDED,
            lifecycle::VFS_EXISTS_FAILED,
        ),
        |view, path: &String| view.exists(path),
        |path| VfsOp::Exists { path },
    )?;
    install("exists", exists)?;

    globals.raw_set("store", table).map_err(Error::lua)?;
    Ok(())
}

/// Switches every store dispatcher [`install_store_table`] built on this
/// VM to the yield shims in `shims`, whichever reference the prompt
/// holds: a function a Plugin prelude captured while it installed yields
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::{VfsError, record_store_conflict};

    #[test]
    fn a_conflict_reaches_a_poisoned_slot() {
        let slot = Mutex::new(None::<String>);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = slot.lock().expect("the first lock is not poisoned");
            panic!("poison the store-conflict slot");
        }));
        assert!(poisoned.is_err(), "the panic must poison the slot");
        let conflict: std::result::Result<(), VfsError> = Err(VfsError::Conflict {
            path: "a.txt".to_owned(),
            detail: "a.txt claimed by two identities".to_owned(),
        });
        record_store_conflict(&slot, &conflict);
        let held = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            held.as_deref(),
            Some("a.txt claimed by two identities"),
            "a poisoned slot must still record the conflict"
        );
    }
}
