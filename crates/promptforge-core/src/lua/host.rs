use super::{
    Arc, AtomicU32, AtomicUsize, Error, GuardNonce, LUA_LOG_CHARACTER_LIMIT, Lua, MultiValue,
    Observation, Observer, Ordering, Result, StoreRef, Value, WriteScope, detail,
};

/// Shared body of the persistent per-section `log(message)` host callback.
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

pub(crate) fn is_log_line_break_or_control(character: char) -> bool {
    character.is_control() || matches!(character, '\u{2028}' | '\u{2029}')
}

/// Installs `untrusted(s)` as a persistent global valid for the section's
/// whole lifecycle. The closure captures an owned clone of the run's nonce -
/// mlua's `create_function` requires `Fn + Send + 'static`, so no borrow can
/// cross the install - and every wrap the VM performs shares that one nonce.
/// Every string input succeeds, so the install needs no observer, no budget,
/// and no [`mlua::Scope`]; a non-string argument fails through mlua's
/// automatic type error.
pub(crate) fn install_untrusted(lua: &Lua, nonce: &GuardNonce) -> Result<()> {
    let nonce = nonce.clone();
    let untrusted = lua
        .create_function(move |_, s: String| Ok(crate::untrusted::wrap(&nonce, &s)))
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

/// Shared body of the persistent per-section `store.read` host callback.
///
/// No `start` reads the whole file; a present `start` slices a 1-based
/// inclusive line range. A negative bound converts to 0, which
/// [`StoreRef::read_range`] rejects with the same error a zero bound earns,
/// and an `end` without a `start` is refused rather than silently ignored.
fn read_store_bounded(
    handle: &StoreRef,
    path: &str,
    start: Option<i64>,
    end: Option<i64>,
    numbered: bool,
) -> std::result::Result<String, crate::store::StoreError> {
    match start {
        None if end.is_none() => {
            if numbered {
                handle.read_range_numbered(path, 1, None)
            } else {
                handle.read(path)
            }
        }
        None => Err(crate::store::StoreError::invalid_range(
            path,
            "start is required when end is given",
        )),
        Some(start) => {
            let start = usize::try_from(start).unwrap_or(0);
            let end = end.map(|line| usize::try_from(line).unwrap_or(0));
            if numbered {
                handle.read_range_numbered(path, start, end)
            } else {
                handle.read_range(path, start, end)
            }
        }
    }
}

/// Shared body of the persistent per-section `store.read` host callback.
fn read_store(
    handle: &StoreRef,
    path: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> std::result::Result<String, crate::store::StoreError> {
    read_store_bounded(handle, path, start, end, false)
}

/// Shared body of the persistent per-section `store.read_numbered` callback.
fn read_store_numbered(
    handle: &StoreRef,
    path: &str,
    start: Option<i64>,
    end: Option<i64>,
) -> std::result::Result<String, crate::store::StoreError> {
    read_store_bounded(handle, path, start, end, true)
}

/// Expose an always-on `store` table whose methods (`write`, `append`,
/// `read`, `read_numbered`, `str_replace`, `delete`,
/// `glob`, `exists`) are backed by the run-scoped [`StoreRef`] handle.
/// Installed once per section with [`Lua::create_function`], so the table
/// stays valid across every chunk the VM runs without a live [`mlua::Scope`].
///
/// The table is a deterministic host capability, present regardless of tool
/// scoping. The mutating ops (`write`/`append`/`str_replace`/`delete`) return
/// nil; `read` returns the file verbatim, optionally bounded to a 1-based
/// inclusive line range (`read(path, start)` reads to end of file,
/// `read(path, start, end)` slices); `read_numbered` returns it with
/// absolute line numbers under the same optional bounds; `glob` returns an
/// array table of matching paths. A [`StoreError`] from any op is mapped into
/// an `mlua` error via [`mlua::Error::external`], so it aborts the chunk and
/// surfaces as [`Error::Lua`].
///
/// The `StoreRef` handle locks a mutex internally per call and is synchronous, so
/// nothing is held across an await.
///
/// A fanout arm's table carries its [`WriteScope`]: `store.write` goes
/// through [`StoreRef::write_scoped`], so two arms of one fanout writing the
/// same path fail the second writer with a write-write race error. Every
/// other caller (walk sections, H1) installs with `None` and writes
/// untracked.
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
    write_scope: Option<WriteScope>,
) -> Result<()> {
    let table = lua.create_table().map_err(Error::lua)?;
    let reporter = Arc::new(StoreReporter {
        execution: execution.to_owned(),
        observer: Arc::clone(observer),
        section: section.to_owned(),
    });

    macro_rules! install_reported_store_fn {
        (
            $name:literal,
            $handle:ident,
            $arguments:pat_param,
            $argument_type:ty,
            $success:expr,
            $failure:expr,
            $operation:block
        ) => {{
            let $handle = store.clone();
            let report = Arc::clone(&reporter);
            let function = lua
                .create_function(move |_, $arguments: $argument_type| {
                    let result = $operation;
                    report.report(result.is_ok(), $success, $failure);
                    result.map_err(mlua::Error::external)
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
        detail::STORE_WRITE_SUCCEEDED,
        detail::STORE_WRITE_FAILED,
        {
            match write_scope {
                Some(scope) => handle.write_scoped(&path, &contents, scope),
                None => handle.write(&path, &contents),
            }
        }
    );
    install_reported_store_fn!(
        "append",
        handle,
        (path, contents),
        (String, String),
        detail::STORE_APPEND_SUCCEEDED,
        detail::STORE_APPEND_FAILED,
        { handle.append(&path, &contents) }
    );
    install_reported_store_fn!(
        "read",
        handle,
        (path, start, end),
        (String, Option<i64>, Option<i64>),
        detail::STORE_READ_SUCCEEDED,
        detail::STORE_READ_FAILED,
        { read_store(&handle, &path, start, end) }
    );
    install_reported_store_fn!(
        "read_numbered",
        handle,
        (path, start, end),
        (String, Option<i64>, Option<i64>),
        detail::STORE_READ_NUMBERED_SUCCEEDED,
        detail::STORE_READ_NUMBERED_FAILED,
        { read_store_numbered(&handle, &path, start, end) }
    );
    install_reported_store_fn!(
        "str_replace",
        handle,
        (path, old, new),
        (String, String, String),
        detail::STORE_REPLACE_SUCCEEDED,
        detail::STORE_REPLACE_FAILED,
        { handle.str_replace(&path, &old, &new) }
    );
    install_reported_store_fn!(
        "delete",
        handle,
        path,
        String,
        detail::STORE_DELETE_SUCCEEDED,
        detail::STORE_DELETE_FAILED,
        { handle.delete(&path) }
    );

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
