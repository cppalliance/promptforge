//! Per-VM tool runtime state: call counts, the task allowlist, and the scoped tool set a section sees.

use super::{Arc, BTreeMap, Error, Mutex, Result};

/// Shared per-VM tool-call counts, seeded at 0 for every key the installer
/// was given: a catalog tool's id, or a local tool's alias. A dispatch
/// seeds a missing key on demand through [`ensure`](Self::ensure).
///
/// The executor increments a count when dispatch is attempted (even if the tool
/// later errors). Lua reads the snapshot through the `tools.calls` table.
#[derive(Debug, Clone, Default)]
pub struct ToolCallCounts {
    inner: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl ToolCallCounts {
    /// Creates a counts map pre-seeded with 0 for every name.
    #[must_use]
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        let map: BTreeMap<String, u64> = names.into_iter().map(|name| (name, 0)).collect();
        Self {
            inner: Arc::new(Mutex::new(map)),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, u64>>> {
        self.inner
            .lock()
            .map_err(|_| Error::Lua("tool call counts mutex was poisoned".to_owned()))
    }

    /// Ensures `name` is present in the map, seeding it at 0 when new.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub fn ensure(&self, name: &str) -> Result<()> {
        let mut map = self.lock()?;
        map.entry(name.to_owned()).or_insert(0);
        Ok(())
    }

    /// Increments the count for `name`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned or `name` was never
    /// seeded.
    pub fn increment(&self, name: &str) -> Result<()> {
        let mut map = self.lock()?;
        let count = map.get_mut(name).ok_or_else(|| {
            Error::Lua(format!(
                "tool call counts: name {name:?} was not pre-seeded"
            ))
        })?;
        *count += 1;
        Ok(())
    }

    /// Returns the current count for `name`, or `None` when `name` was
    /// never seeded.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub fn get(&self, name: &str) -> Result<Option<u64>> {
        Ok(self.lock()?.get(name).copied())
    }

    /// Returns a snapshot of every seeded name.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub fn names(&self) -> Result<Vec<String>> {
        Ok(self.lock()?.keys().cloned().collect())
    }
}

/// Which targets the model may start a task over in one section, once
/// the author has opted in through `tools.allow_tasks`.
///
/// The allowlist is the section's fact, recorded on its tool runtime
/// beside the scope: the `chat` arm advertises the task built-ins to the
/// model while it is set, and the `tool_call` arm checks a `task` call's
/// target against it. Targets are compared as the author wrote them (a
/// heading such as `## Research`), whitespace-trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskAllowlist {
    /// Any section the owner's chain can resolve.
    Any,
    /// Only the named targets.
    Only(Vec<String>),
}

impl TaskAllowlist {
    /// Whether `target` may be started under this allowlist.
    #[must_use]
    pub fn permits(&self, target: &str) -> bool {
        match self {
            TaskAllowlist::Any => true,
            TaskAllowlist::Only(targets) => {
                let target = target.trim();
                targets.iter().any(|allowed| allowed.trim() == target)
            }
        }
    }
}

/// Tracks tools one section VM offered and their description overrides.
#[derive(Debug)]
pub struct ToolRuntime {
    /// The wire names `tools.offer` put in the section's scope, in
    /// first-offer order.
    pub added: Vec<String>,
    /// The section's author overrides for model-facing schema
    /// descriptions, keyed by wire name.
    pub description_overrides: BTreeMap<String, String>,
    /// The model's task allowlist, once `tools.allow_tasks` has run in the
    /// section; `None` leaves the task built-ins off the model's tool
    /// surface.
    pub allowed_tasks: Option<TaskAllowlist>,
}
