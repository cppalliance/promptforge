//! The agent-only `runtime.events()` read view over the host's [`EventLog`].
//!
//! `runtime.events()` returns lazy userdata, never a copy: `__len` reads the
//! snapshot's length bound and `__index` converts exactly one entry per
//! access through the serde boundary, so the log is never copied in bulk.
//! The bound is the determinism rule made mechanical - the resume-refresh
//! rule: the driver republishes it through [`EventsSnapshot::refresh`] at
//! every host-call resume, and an agent program is one long-running chunk,
//! so appends - landing while the program is suspended, or synchronously
//! from a host callback while it runs - become visible exactly at the next
//! resume, never mid-chunk. A view is read-only: assignment raises, and a
//! converted entry is a fresh table whose mutation cannot reach the log.
//!
//! Installed by the agent executor alone; a section VM never has a
//! `runtime` global.

use promptforge_core_support::events::EventLog;

use super::{
    Arc, AtomicU64, Error, Lua, LuaSerdeExt, MetaMethod, Ordering, Result, UserData,
    UserDataMethods, Value,
};

/// The driver-held refresh handle for one VM's `runtime.events()` views.
///
/// [`refresh`](Self::refresh) re-reads the log's length into the bound
/// shared with every view the VM's `runtime.events()` has returned or will
/// return. The agent driver calls it at every host-call resume.
pub struct EventsSnapshot {
    /// The host's log, re-measured on refresh.
    log: Arc<dyn EventLog>,
    /// The length bound every view of this VM reads.
    bound: Arc<AtomicU64>,
}

impl EventsSnapshot {
    /// Refreshes the snapshot's length bound to the log's current length.
    pub fn refresh(&self) {
        // Relaxed suffices: the bound is written and read on the driver's
        // own task, and cross-thread appends are ordered by the log itself.
        self.bound.store(self.log.len(), Ordering::Relaxed);
    }
}

/// Shows the bound; the log trait object has no useful rendering.
impl std::fmt::Debug for EventsSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventsSnapshot")
            .field("bound", &self.bound.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// One lazy view over the log: the userdata `runtime.events()` returns.
struct EventsView {
    /// The host's log, read one entry at a time on `__index`.
    log: Arc<dyn EventLog>,
    /// The snapshot bound shared with the driver's [`EventsSnapshot`].
    bound: Arc<AtomicU64>,
}

impl UserData for EventsView {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Len, |_, this, ()| {
            // Lua integers are 64-bit signed; no real log outgrows them, so
            // the cap can never truncate in practice.
            Ok(i64::try_from(this.bound.load(Ordering::Relaxed)).unwrap_or(i64::MAX))
        });
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: Value| {
            let bound = this.bound.load(Ordering::Relaxed);
            let Some(index) = entry_index(&key, bound) else {
                return Ok(Value::Nil);
            };
            match this.log.get(index) {
                // The one conversion per access: exactly this entry crosses
                // the serde boundary as a fresh table.
                Some(event) => lua.to_value(&event),
                // Only a log that shrank - violating the append-only
                // contract - lands here; absence reads as nil rather than
                // failing the chunk.
                None => Ok(Value::Nil),
            }
        });
        methods.add_meta_method(
            MetaMethod::NewIndex,
            |_, _, (_, _): (Value, Value)| -> mlua::Result<()> {
                Err(mlua::Error::external("runtime.events() is read-only"))
            },
        );
    }
}

/// Maps one Lua key to the 0-based log index it addresses: a 1-based
/// integer position within `bound`. A float key holding an exact integer
/// addresses like that integer, mirroring Lua's own table indexing; every
/// other key addresses nothing.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "the round-trip comparison passes only exact integers; the one saturation collision (2^63 reads as i64::MAX) still lands past any real bound and reads nil"
)]
fn entry_index(key: &Value, bound: u64) -> Option<u64> {
    let position = match key {
        Value::Integer(position) => *position,
        Value::Number(position) => {
            let truncated = *position as i64;
            if ((truncated as f64) - *position).abs() > 0.0 {
                return None;
            }
            truncated
        }
        _ => return None,
    };
    if position < 1 {
        return None;
    }
    // `position >= 1`, so the conversion cannot fail; the fallback keeps
    // the arm expression-shaped without an expect.
    let index = u64::try_from(position - 1).unwrap_or(u64::MAX);
    (index < bound).then_some(index)
}

/// Installs the agent-only `runtime.events()` host call on `lua`.
///
/// With a log, `runtime.events()` returns a fresh lazy view (userdata) over
/// it, and the returned [`EventsSnapshot`] is the driver's refresh handle.
/// The bound starts at the log's length at install, so a relaunched agent
/// sees its whole persisted history from its first instruction. With no
/// log there is no history and nothing to refresh: `runtime.events()`
/// returns a fresh empty table and the handle is `None`.
///
/// # Errors
/// Returns [`Error::Lua`] if the `runtime` table or its `events` function
/// cannot be created or installed.
pub fn install_runtime_events(
    lua: &Lua,
    log: Option<Arc<dyn EventLog>>,
) -> Result<Option<EventsSnapshot>> {
    let runtime = lua.create_table().map_err(Error::lua)?;
    let snapshot = if let Some(log) = log {
        let bound = Arc::new(AtomicU64::new(0));
        let view_log = Arc::clone(&log);
        let view_bound = Arc::clone(&bound);
        let events = lua
            .create_function(move |_, ()| {
                Ok(EventsView {
                    log: Arc::clone(&view_log),
                    bound: Arc::clone(&view_bound),
                })
            })
            .map_err(Error::lua)?;
        runtime.raw_set("events", events).map_err(Error::lua)?;
        let snapshot = EventsSnapshot { log, bound };
        snapshot.refresh();
        Some(snapshot)
    } else {
        let events = lua
            .create_function(|lua, ()| lua.create_table())
            .map_err(Error::lua)?;
        runtime.raw_set("events", events).map_err(Error::lua)?;
        None
    };
    lua.globals()
        .raw_set("runtime", runtime)
        .map_err(Error::lua)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use promptforge_core_support::events::{RuntimeEvent, RuntimeEventKind};

    use super::*;
    use crate::AtomicUsize;

    /// An instrumented log: counts `get` calls, so a test can prove access
    /// converts one entry at a time and never copies the log in bulk.
    #[derive(Default)]
    struct CountingLog {
        events: crate::Mutex<Vec<RuntimeEvent>>,
        gets: AtomicUsize,
    }

    impl CountingLog {
        fn push(&self, kind: RuntimeEventKind, content: &str) {
            self.events
                .lock()
                .expect("the test log must not be poisoned")
                .push(RuntimeEvent {
                    kind,
                    section: "agent".to_owned(),
                    chain_id: 0,
                    depth: 0,
                    turn: 0,
                    content: content.to_owned(),
                    model: None,
                    tool_call_id: None,
                    finish_reason: None,
                    metrics: None,
                });
        }

        fn get_calls(&self) -> usize {
            self.gets.load(Ordering::Relaxed)
        }

        fn pop(&self) {
            self.events
                .lock()
                .expect("the test log must not be poisoned")
                .pop();
        }
    }

    impl EventLog for CountingLog {
        fn len(&self) -> u64 {
            u64::try_from(
                self.events
                    .lock()
                    .expect("the test log must not be poisoned")
                    .len(),
            )
            .expect("the test log length fits in u64")
        }

        fn get(&self, index: u64) -> Option<RuntimeEvent> {
            self.gets.fetch_add(1, Ordering::Relaxed);
            let events = self
                .events
                .lock()
                .expect("the test log must not be poisoned");
            usize::try_from(index)
                .ok()
                .and_then(|index| events.get(index).cloned())
        }
    }

    fn view_over(log: &Arc<CountingLog>) -> (Lua, EventsSnapshot) {
        let lua = Lua::new();
        let snapshot = install_runtime_events(&lua, Some(Arc::clone(log) as Arc<dyn EventLog>))
            .expect("the installer succeeds")
            .expect("a supplied log yields a refresh handle");
        (lua, snapshot)
    }

    #[test]
    fn the_view_serves_indexed_reads_and_rejects_writes() {
        let log = Arc::new(CountingLog::default());
        log.push(RuntimeEventKind::UserInput, "hello");
        log.push(RuntimeEventKind::AssistantReply, "world");
        let (lua, _snapshot) = view_over(&log);
        let (len, first, second_kind, float_kind, past, zero, negative, named, write_ok): (
            i64,
            String,
            String,
            String,
            bool,
            bool,
            bool,
            bool,
            bool,
        ) = lua
            .load(
                r#"
                local events = runtime.events()
                local write_ok = pcall(function() events[1] = "x" end)
                return #events, events[1].content, events[2].kind, events[2.0].kind,
                    events[3] == nil, events[0] == nil, events[-1] == nil,
                    events.latest == nil, write_ok
                "#,
            )
            .eval()
            .expect("the chunk runs");
        assert_eq!(len, 2, "__len is the snapshot bound");
        assert_eq!(first, "hello", "1-based access converts the first entry");
        assert_eq!(
            second_kind, "agent_message",
            "kinds convert to their pinned serialized labels"
        );
        assert_eq!(
            float_kind, "agent_message",
            "a float key holding an exact integer addresses like that integer"
        );
        assert!(past, "a past-bound index reads nil");
        assert!(zero, "index 0 reads nil: the view is 1-based");
        assert!(negative, "a negative index reads nil");
        assert!(named, "a non-numeric key reads nil");
        assert!(!write_ok, "assignment must raise: the view is read-only");
    }

    #[test]
    fn per_index_access_converts_exactly_one_entry() {
        let log = Arc::new(CountingLog::default());
        log.push(RuntimeEventKind::UserInput, "one");
        log.push(RuntimeEventKind::UserInput, "two");
        log.push(RuntimeEventKind::UserInput, "three");
        let (lua, _snapshot) = view_over(&log);
        let content: String = lua
            .load(
                r"
                local events = runtime.events()
                local _ = #events
                return events[2].content
                ",
            )
            .eval()
            .expect("the chunk runs");
        assert_eq!(content, "two");
        assert_eq!(
            log.get_calls(),
            1,
            "one indexed access converts one entry; a bulk copy or a len-driven scan would read more"
        );
    }

    #[test]
    fn appends_become_visible_at_refresh_never_between() {
        let log = Arc::new(CountingLog::default());
        log.push(RuntimeEventKind::UserInput, "one");
        let (lua, snapshot) = view_over(&log);
        log.push(RuntimeEventKind::UserInput, "two");
        log.push(RuntimeEventKind::UserInput, "three");
        // The view is deliberately a global, so the second chunk reads the
        // same view the first created: the refresh must reach it.
        let (len, second_nil): (i64, bool) = lua
            .load(
                r"
                events = runtime.events()
                return #events, events[2] == nil
                ",
            )
            .eval()
            .expect("the first chunk runs");
        assert_eq!(
            len, 1,
            "the bound stays at the install-time length until a refresh"
        );
        assert!(
            second_nil,
            "an appended entry past the bound reads nil even though the log holds it"
        );
        assert_eq!(
            log.get_calls(),
            0,
            "a past-bound read never touches the log"
        );
        snapshot.refresh();
        let (len, third): (i64, String) = lua
            .load("return #events, events[3].content")
            .eval()
            .expect("the second chunk runs");
        assert_eq!(len, 3, "one refresh publishes every append at once");
        assert_eq!(third, "three");
    }

    #[test]
    fn an_entry_the_log_no_longer_holds_reads_nil() {
        let log = Arc::new(CountingLog::default());
        log.push(RuntimeEventKind::UserInput, "one");
        log.push(RuntimeEventKind::UserInput, "two");
        let (lua, _snapshot) = view_over(&log);
        // Shrink the log behind the installed bound of 2 - the append-only
        // contract violated - so an in-bound `get` returns None.
        log.pop();
        let (first, second_nil): (String, bool) = lua
            .load(
                r"
                local events = runtime.events()
                return events[1].content, events[2] == nil
                ",
            )
            .eval()
            .expect("the chunk survives a shrunk log");
        assert_eq!(first, "one", "an entry the log still holds converts");
        assert!(
            second_nil,
            "an in-bound entry the log no longer holds must read nil, never fail the chunk"
        );
    }

    #[test]
    fn an_absent_log_yields_a_fresh_empty_table() {
        let lua = Lua::new();
        let snapshot = install_runtime_events(&lua, None).expect("the installer succeeds");
        assert!(snapshot.is_none(), "no log means nothing to refresh");
        let (kind, len, first_nil): (String, i64, bool) = lua
            .load(
                r"
                local events = runtime.events()
                return type(events), #events, events[1] == nil
                ",
            )
            .eval()
            .expect("the chunk runs");
        assert_eq!(kind, "table");
        assert_eq!(len, 0);
        assert!(first_nil);
    }
}
