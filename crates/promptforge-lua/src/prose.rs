//! The read-only lazy `prose` global machinery.
//!
//! Before each Lua coroutine starts, the executor installs the pending
//! Markdown buffer as a fresh lazy `prose` template through
//! [`SectionVm::install_lazy_prose`](crate::SectionVm::install_lazy_prose).
//! The global stays unresolved until its first runtime read, which
//! snapshots the section state (the `var` clipboard, the live `sys`, and
//! the bare globals), renders every `{{ }}` substitution once through the
//! host's callback, and memoizes the string for later reads. Assigning to
//! `prose` raises, and `{{ prose }}` inside the template is rejected as
//! recursive.
//!
//! The guard rides on the `_G` metatable: `__index` renders and memoizes
//! the `prose` key, `__newindex` rejects writes to it, and every other key
//! delegates to whatever metatable author code (say, the shared library)
//! installed first. Each install replaces the previous pair's handler, so
//! a later fence's buffer evaluates fresh while an already rendered string
//! the author kept in a local or `var` survives untouched.

use super::{Arc, Error, Json, Lua, LuaSerdeExt, MultiValue, Mutex, Result, Value, var_to_json};

/// Marker field on a metatable this module installed: a re-install reuses
/// the recorded delegates instead of chaining a new handler over its own.
const GUARD_MARKER: &str = "__promptforge_prose_guard";
/// The metatable field recording the `__index` the guard shadows.
const DELEGATE_INDEX: &str = "__promptforge_prose_delegate_index";
/// The metatable field recording the `__newindex` the guard shadows.
const DELEGATE_NEWINDEX: &str = "__promptforge_prose_delegate_newindex";

/// The section state a `prose` render snapshots at the first read.
///
/// The host's render callback receives the `var` clipboard and the live
/// `sys` JSON as read at render time, plus a bare-global lookup for
/// `{{ name }}` resolution. The lookup reads the section VM's globals:
/// `Ok(None)` when unset, the JSON form when set, and an error for a
/// function, userdata, or thread - or for `prose` itself, which is
/// rejected as recursive.
pub struct ProseState<'a> {
    /// The `var` clipboard read back at the first read.
    pub var: Json,
    /// The live `sys` JSON at the first read.
    pub sys: Json,
    /// The bare-global lookup for `{{ name }}` resolution.
    pub globals: &'a dyn Fn(&str) -> Result<Option<Json>>,
}

impl std::fmt::Debug for ProseState<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProseState")
            .field("var", &self.var)
            .field("sys", &self.sys)
            .finish_non_exhaustive()
    }
}

/// Installs `prose` as a fresh read-only lazy global on this VM.
///
/// `render` runs at most once, on the first runtime read of `prose`, with
/// the section state snapshot; its result is memoized for later reads.
/// `sys_live` is the VM's live `sys` mirror, so a `sys` enrichment between
/// install and first read is visible to the render.
///
/// # Errors
/// Returns [`Error::Lua`] if the guard metatable cannot be built or
/// installed.
pub(crate) fn install<F>(lua: &Lua, sys_live: &Arc<Mutex<Option<Json>>>, render: F) -> Result<()>
where
    F: Fn(ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    let globals = lua.globals();
    let old = globals.metatable();
    // The delegates the new guard shadows: a metatable of our own already
    // recorded its delegates, so a re-install reuses them rather than
    // chaining over the previous handler; any other metatable (a shared
    // library's) contributes its own index pair.
    let (delegate_index, delegate_newindex) = match &old {
        Some(old) if matches!(old.raw_get::<Value>(GUARD_MARKER), Ok(Value::Boolean(true))) => (
            old.raw_get::<Value>(DELEGATE_INDEX).map_err(Error::lua)?,
            old.raw_get::<Value>(DELEGATE_NEWINDEX)
                .map_err(Error::lua)?,
        ),
        Some(old) => (
            old.raw_get::<Value>("__index").map_err(Error::lua)?,
            old.raw_get::<Value>("__newindex").map_err(Error::lua)?,
        ),
        None => (Value::Nil, Value::Nil),
    };
    let metatable = lua.create_table().map_err(Error::lua)?;
    // Carry every other field the previous metatable installed (a shared
    // library's `_G` metatable keeps working), then shadow the index pair
    // with the prose guard.
    if let Some(old) = &old {
        for pair in old.clone().pairs::<Value, Value>() {
            let (key, value) = pair.map_err(Error::lua)?;
            let shadowed =
                matches!(&key, Value::String(name) if name == "__index" || name == "__newindex");
            if !shadowed {
                metatable.raw_set(key, value).map_err(Error::lua)?;
            }
        }
    }
    metatable.raw_set(GUARD_MARKER, true).map_err(Error::lua)?;
    metatable
        .raw_set(DELEGATE_INDEX, delegate_index.clone())
        .map_err(Error::lua)?;
    metatable
        .raw_set(DELEGATE_NEWINDEX, delegate_newindex.clone())
        .map_err(Error::lua)?;

    // The memo slot is per install: the raw global stays unset, so every
    // read and every write of `prose` crosses the guard, the read-only
    // rejection cannot be escaped by an assignment after the first read,
    // and the next fence's install starts unresolved.
    let memo = Arc::new(Mutex::new(None::<String>));
    let index = guard_index(lua, sys_live, &memo, &delegate_index, render)?;
    let newindex = guard_newindex(lua, &delegate_newindex)?;
    metatable.raw_set("__index", index).map_err(Error::lua)?;
    metatable
        .raw_set("__newindex", newindex)
        .map_err(Error::lua)?;
    globals.set_metatable(Some(metatable)).map_err(Error::lua)
}

/// Builds the guard's `__index`: a `prose` read renders once through the
/// host callback and memoizes; every other key delegates to the shadowed
/// `__index`.
///
/// # Errors
/// Returns [`Error::Lua`] if the closure cannot be created.
fn guard_index<F>(
    lua: &Lua,
    sys_live: &Arc<Mutex<Option<Json>>>,
    memo: &Arc<Mutex<Option<String>>>,
    delegate_index: &Value,
    render: F,
) -> Result<mlua::Function>
where
    F: Fn(ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    let memo = Arc::clone(memo);
    let sys_live = Arc::clone(sys_live);
    let delegate_index = delegate_index.clone();
    lua.create_function(move |lua, (target, key): (mlua::Table, Value)| {
        if matches!(&key, Value::String(name) if name == "prose") {
            {
                let guard = memo.lock().map_err(|_| {
                    mlua::Error::external(Error::Lua("prose memo slot was poisoned".to_owned()))
                })?;
                if let Some(rendered) = guard.as_ref() {
                    return lua.create_string(rendered).map(Value::String);
                }
            }
            let var = var_to_json(lua).map_err(mlua::Error::external)?;
            let sys = {
                let guard = sys_live.lock().map_err(|_| {
                    mlua::Error::external(Error::Lua("sys live slot was poisoned".to_owned()))
                })?;
                guard.clone().ok_or_else(|| {
                    mlua::Error::external(Error::Lua(
                        "section VM host values have not been injected".to_owned(),
                    ))
                })?
            };
            let globals_lookup = |name: &str| -> Result<Option<Json>> {
                if name == "prose" {
                    return Err(Error::Lua(
                        "recursive {{ prose }}: prose cannot reference itself".to_owned(),
                    ));
                }
                let value: Value = lua.globals().get(name).map_err(Error::lua)?;
                match value {
                    Value::Nil => Ok(None),
                    Value::Function(_) | Value::UserData(_) | Value::Thread(_) => {
                        Err(Error::Lua(format!(
                            "global `{name}` is a {}; bare globals in prose must be JSON data",
                            value.type_name()
                        )))
                    }
                    other => Ok(Some(lua.from_value(other).map_err(Error::lua)?)),
                }
            };
            let rendered = render(ProseState {
                var,
                sys,
                globals: &globals_lookup,
            })?;
            let mut guard = memo.lock().map_err(|_| {
                mlua::Error::external(Error::Lua("prose memo slot was poisoned".to_owned()))
            })?;
            *guard = Some(rendered.clone());
            drop(guard);
            return lua.create_string(&rendered).map(Value::String);
        }
        match &delegate_index {
            Value::Function(function) => Ok(function
                .call::<MultiValue>((target, key))?
                .into_iter()
                .next()
                .unwrap_or(Value::Nil)),
            Value::Table(table) => table.get(key),
            _ => Ok(Value::Nil),
        }
    })
    .map_err(Error::lua)
}

/// Builds the guard's `__newindex`: a `prose` write raises the read-only
/// error; every other write delegates to the shadowed `__newindex`.
///
/// # Errors
/// Returns [`Error::Lua`] if the closure cannot be created.
fn guard_newindex(lua: &Lua, delegate_newindex: &Value) -> Result<mlua::Function> {
    let delegate_newindex = delegate_newindex.clone();
    lua.create_function(
        move |_lua, (target, key, value): (mlua::Table, Value, Value)| -> mlua::Result<()> {
            if matches!(&key, Value::String(name) if name == "prose") {
                return Err(mlua::Error::runtime(
                    "prose is read-only: assign to `var` or a section global instead",
                ));
            }
            match &delegate_newindex {
                Value::Function(function) => {
                    function.call::<MultiValue>((target, key, value))?;
                    Ok(())
                }
                Value::Table(table) => table.set(key, value),
                _ => target.raw_set(key, value),
            }
        },
    )
    .map_err(Error::lua)
}
