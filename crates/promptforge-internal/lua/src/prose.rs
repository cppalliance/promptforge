//! The read-only lazy `prose` global machinery.
//!
//! Before each Lua coroutine starts, the executor installs the pending
//! Markdown buffer as a fresh lazy `prose` template through
//! [`SectionVm::install_lazy_prose`](crate::SectionVm::install_lazy_prose).
//! The global stays unresolved until its first runtime read, which
//! snapshots the section state (the section's `var` table, the live
//! `sys`, and the bare globals), renders every `{{ }}` substitution once
//! through the Engine's callback, and memoizes the string for later reads.
//! Assigning to `prose` raises, and `{{ prose }}` inside the template is
//! rejected as recursive.
//!
//! The guard is the `_G` guard ([`crate::globals`]): it serves `prose`
//! through the current install's render and refuses every write, before
//! any metatable author code set on `_G` sees the key. Before the first
//! install, `prose` reads nil and still refuses writes. Each install
//! replaces the previous render, so a later fence's buffer evaluates fresh
//! while an already rendered string the author kept in a local or `var`
//! survives untouched.

use super::{Arc, Error, Json, Lua, LuaSerdeExt, Mutex, Result, Value, var_to_json};

/// The refusal an assignment of the `prose` global raises.
pub(crate) const ASSIGNMENT_REFUSAL: &str =
    "prose is read-only: assign to `var` or a section global instead";

/// The section state a `prose` render snapshots at the first read.
///
/// The Engine's render callback receives the section's `var` table and the
/// live `sys` JSON as read at render time, plus a bare-global lookup for
/// `{{ name }}` resolution. The lookup reads the section VM's globals:
/// `Ok(None)` when unset, the JSON form when set, and an error for a
/// function, userdata, or thread - or for `prose` itself, which is
/// rejected as recursive.
pub struct ProseState<'a> {
    /// The section's `var` table read back at the first read.
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
/// Returns [`Error::Lua`] if the read cannot be built or the guard cannot
/// record it.
pub(crate) fn install<F>(lua: &Lua, sys_live: &Arc<Mutex<Option<Json>>>, render: F) -> Result<()>
where
    F: Fn(ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    let read = lazy_read(lua, sys_live, render)?;
    crate::globals::install_prose(lua, read)
}

/// Builds one install's `prose` read: the first call renders once through
/// the Engine callback and memoizes, and every later call returns the memo.
///
/// # Errors
/// Returns [`Error::Lua`] if the closure cannot be created.
fn lazy_read<F>(lua: &Lua, sys_live: &Arc<Mutex<Option<Json>>>, render: F) -> Result<mlua::Function>
where
    F: Fn(ProseState) -> mlua::Result<String> + Send + Sync + 'static,
{
    // The memo slot is per install, so the next fence's install starts
    // unresolved.
    let memo = Mutex::new(None::<String>);
    let sys_live = Arc::clone(sys_live);
    lua.create_function(move |lua, ()| {
        {
            let guard = memo.lock().map_err(|_| {
                mlua::Error::external(Error::Lua("prose memo slot was poisoned".to_owned()))
            })?;
            if let Some(rendered) = guard.as_ref() {
                return lua.create_string(rendered);
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
        lua.create_string(&rendered)
    })
    .map_err(Error::lua)
}
