use super::{
    Arc, AtomicU32, AtomicUsize, Error, LUA_LOG_CHARACTER_LIMIT, Lua, MultiValue, Observation,
    Observer, Ordering, Result, Scope, StoreRef, Value, detail,
};

/// Shared body of the `log(message)` host callback, used by both the
/// persistent per-section install and the scope-bound shared-phase install.
fn log_checkpoint(
    execution: &str,
    observer: &dyn Observer,
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
    observer.observe(execution, section, Observation::Lua(message.to_owned()));
    Ok(())
}

/// Installs `log` as a persistent global valid for the section's whole
/// lifecycle. The closure captures owned strings and Arc clones, so it
/// outlives any single chunk without an [`mlua::Scope`].
pub(crate) fn install_log(
    lua: &Lua,
    execution: &str,
    observer: &Arc<dyn Observer>,
    section: &str,
    log_budget: &Arc<AtomicU32>,
    log_byte_budget: &Arc<AtomicUsize>,
) -> Result<()> {
    let execution = execution.to_owned();
    let section = section.to_owned();
    let observer = Arc::clone(observer);
    let log_budget = Arc::clone(log_budget);
    let log_byte_budget = Arc::clone(log_byte_budget);
    let log = lua
        .create_function(move |_, arguments: MultiValue| {
            log_checkpoint(
                &execution,
                observer.as_ref(),
                &section,
                &log_budget,
                &log_byte_budget,
                arguments,
            )
        })
        .map_err(Error::lua)?;
    lua.globals().raw_set("log", log).map_err(Error::lua)
}

/// Scope-bound `log` install for the shared-load phase, which runs before
/// host injection with only a borrowed observer available.
pub(crate) fn install_log_scoped<'scope, 'env: 'scope>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    execution: &'env str,
    observer: &'env dyn Observer,
    section: &'env str,
    log_budget: &'env AtomicU32,
    log_byte_budget: &'env AtomicUsize,
) -> Result<()> {
    let log = scope
        .create_function(move |_, arguments: MultiValue| {
            log_checkpoint(
                execution,
                observer,
                section,
                log_budget,
                log_byte_budget,
                arguments,
            )
        })
        .map_err(Error::lua)?;
    lua.globals().raw_set("log", log).map_err(Error::lua)
}

pub(crate) fn is_log_line_break_or_control(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Installs `untrusted(s)` as a persistent global valid for the section's
/// whole lifecycle. The closure captures nothing and every string input
/// succeeds, so the install needs no observer, no budget, and no
/// [`mlua::Scope`]; a non-string argument fails through mlua's automatic
/// type error.
pub(crate) fn install_untrusted(lua: &Lua) -> Result<()> {
    let untrusted = lua
        .create_function(|_, s: String| Ok(crate::untrusted::wrap(&s)))
        .map_err(Error::lua)?;
    lua.globals()
        .raw_set("untrusted", untrusted)
        .map_err(Error::lua)
}

/// Owned observation context captured by the persistent `store` closures.
struct StoreReporter {
    execution: String,
    observer: Arc<dyn Observer>,
    section: String,
}

impl StoreReporter {
    fn report(&self, succeeded: bool, success: Observation, failure: Observation) {
        observe_store_result(
            &self.execution,
            self.observer.as_ref(),
            &self.section,
            succeeded,
            success,
            failure,
        );
    }
}

pub(crate) fn observe_store_result(
    execution: &str,
    observer: &dyn Observer,
    section: &str,
    succeeded: bool,
    success: Observation,
    failure: Observation,
) {
    observer.observe(
        execution,
        section,
        if succeeded { success } else { failure },
    );
}

/// Expose an always-on `store` table whose methods (`write`, `append`,
/// `read`, `read_lines`, `inject`, `str_replace`, `delete`, `glob`,
/// `exists`) are backed by the run-scoped [`StoreRef`] handle. Installed once
/// per section with [`Lua::create_function`], so the table stays valid across
/// every chunk the VM runs without a live [`mlua::Scope`].
///
/// The table is a deterministic host capability, present regardless of tool
/// scoping. The mutating ops (`write`/`append`/`str_replace`/`delete`) return
/// nil; `read` returns the file verbatim and `read_lines` returns it with
/// line numbers; `glob` returns an array table of matching paths. A
/// [`StoreError`] from any op is mapped into
/// an `mlua` error via [`mlua::Error::external`], so it aborts the chunk and
/// surfaces as [`Error::Lua`].
///
/// The `StoreRef` handle locks a mutex internally per call and is synchronous, so
/// nothing is held across an await.
///
/// [`StoreError`]: crate::store::StoreError
///
/// # Errors
/// Returns [`Error::Lua`] if the `store` table or any of its functions cannot
/// be created or installed into the sandbox globals.
#[expect(
    clippy::too_many_lines,
    reason = "one table installs all store operations beside their matching observation outcomes"
)]
pub(crate) fn install_store_table(
    lua: &Lua,
    globals: &mlua::Table,
    store: &StoreRef,
    execution: &str,
    observer: &Arc<dyn Observer>,
    section: &str,
) -> Result<()> {
    let table = lua.create_table().map_err(Error::lua)?;
    let reporter = Arc::new(StoreReporter {
        execution: execution.to_owned(),
        observer: Arc::clone(observer),
        section: section.to_owned(),
    });

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let write = lua
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.write(&path, &contents);
            report.report(
                result.is_ok(),
                detail::STORE_WRITE_SUCCEEDED,
                detail::STORE_WRITE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("write", write).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let append = lua
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.append(&path, &contents);
            report.report(
                result.is_ok(),
                detail::STORE_APPEND_SUCCEEDED,
                detail::STORE_APPEND_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("append", append).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let read_lines = lua
        .create_function(move |_, path: String| {
            let result = handle.read_lines(&path);
            report.report(
                result.is_ok(),
                detail::STORE_READ_LINES_SUCCEEDED,
                detail::STORE_READ_LINES_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    table.set("read_lines", read_lines).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let read = lua
        .create_function(move |_, path: String| {
            let result = handle.read(&path);
            report.report(
                result.is_ok(),
                detail::STORE_READ_SUCCEEDED,
                detail::STORE_READ_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    table.set("read", read).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let inject = lua
        .create_function(move |_, path: String| {
            let result = handle.inject(&path);
            report.report(
                result.is_ok(),
                detail::STORE_INJECT_SUCCEEDED,
                detail::STORE_INJECT_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    table.set("inject", inject).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let str_replace = lua
        .create_function(move |_, (path, old, new): (String, String, String)| {
            let result = handle.str_replace(&path, &old, &new);
            report.report(
                result.is_ok(),
                detail::STORE_REPLACE_SUCCEEDED,
                detail::STORE_REPLACE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("str_replace", str_replace).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let delete = lua
        .create_function(move |_, path: String| {
            let result = handle.delete(&path);
            report.report(
                result.is_ok(),
                detail::STORE_DELETE_SUCCEEDED,
                detail::STORE_DELETE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("delete", delete).map_err(Error::lua)?;

    let handle = store.clone();
    let report = Arc::clone(&reporter);
    let glob = lua
        .create_function(move |lua, pattern: String| {
            let result = handle.glob(&pattern);
            report.report(
                result.is_ok(),
                detail::STORE_GLOB_SUCCEEDED,
                detail::STORE_GLOB_FAILED,
            );
            let paths = result.map_err(mlua::Error::external)?;
            lua.create_sequence_from(paths)
        })
        .map_err(Error::lua)?;
    table.set("glob", glob).map_err(Error::lua)?;

    let handle = store.clone();
    let exists = lua
        .create_function(move |_, path: String| handle.exists(&path).map_err(mlua::Error::external))
        .map_err(Error::lua)?;
    table.set("exists", exists).map_err(Error::lua)?;

    globals.raw_set("store", table).map_err(Error::lua)?;
    Ok(())
}

/// Scope-bound `store` install retained for the `run_chunk` test harness,
/// which has only a borrowed observer. Production sections use
/// [`install_store_table`].
#[cfg(test)]
#[expect(
    clippy::too_many_lines,
    reason = "one table installs all store operations beside their matching observation outcomes"
)]
pub(crate) fn install_store_table_scoped<'scope, 'env: 'scope>(
    lua: &Lua,
    scope: &'scope Scope<'scope, 'env>,
    globals: &mlua::Table,
    store: &StoreRef,
    execution: &'env str,
    observer: &'env dyn Observer,
    section: &'env str,
) -> Result<()> {
    let table = lua.create_table().map_err(Error::lua)?;

    let handle = store.clone();
    let write = scope
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.write(&path, &contents);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_WRITE_SUCCEEDED,
                detail::STORE_WRITE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("write", write).map_err(Error::lua)?;

    let handle = store.clone();
    let append = scope
        .create_function(move |_, (path, contents): (String, String)| {
            let result = handle.append(&path, &contents);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_APPEND_SUCCEEDED,
                detail::STORE_APPEND_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("append", append).map_err(Error::lua)?;

    let handle = store.clone();
    let read_lines = scope
        .create_function(move |_, path: String| {
            let result = handle.read_lines(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_READ_LINES_SUCCEEDED,
                detail::STORE_READ_LINES_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    table.set("read_lines", read_lines).map_err(Error::lua)?;

    let handle = store.clone();
    let read = scope
        .create_function(move |_, path: String| {
            let result = handle.read(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_READ_SUCCEEDED,
                detail::STORE_READ_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    table.set("read", read).map_err(Error::lua)?;

    let handle = store.clone();
    let inject = scope
        .create_function(move |_, path: String| {
            let result = handle.inject(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_INJECT_SUCCEEDED,
                detail::STORE_INJECT_FAILED,
            );
            result.map_err(mlua::Error::external)
        })
        .map_err(Error::lua)?;
    table.set("inject", inject).map_err(Error::lua)?;

    let handle = store.clone();
    let str_replace = scope
        .create_function(move |_, (path, old, new): (String, String, String)| {
            let result = handle.str_replace(&path, &old, &new);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_REPLACE_SUCCEEDED,
                detail::STORE_REPLACE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("str_replace", str_replace).map_err(Error::lua)?;

    let handle = store.clone();
    let delete = scope
        .create_function(move |_, path: String| {
            let result = handle.delete(&path);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_DELETE_SUCCEEDED,
                detail::STORE_DELETE_FAILED,
            );
            result.map_err(mlua::Error::external)?;
            Ok(())
        })
        .map_err(Error::lua)?;
    table.set("delete", delete).map_err(Error::lua)?;

    let handle = store.clone();
    let glob = scope
        .create_function(move |lua, pattern: String| {
            let result = handle.glob(&pattern);
            observe_store_result(
                execution,
                observer,
                section,
                result.is_ok(),
                detail::STORE_GLOB_SUCCEEDED,
                detail::STORE_GLOB_FAILED,
            );
            let paths = result.map_err(mlua::Error::external)?;
            lua.create_sequence_from(paths)
        })
        .map_err(Error::lua)?;
    table.set("glob", glob).map_err(Error::lua)?;

    let handle = store.clone();
    let exists = scope
        .create_function(move |_, path: String| handle.exists(&path).map_err(mlua::Error::external))
        .map_err(Error::lua)?;
    table.set("exists", exists).map_err(Error::lua)?;

    globals.raw_set("store", table).map_err(Error::lua)?;
    Ok(())
}
