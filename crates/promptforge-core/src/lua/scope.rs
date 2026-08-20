use super::{Arc, BTreeMap, Error, Mutex, Result};

/// Shared per-VM tool-call counts, pre-seeded at 0 for every in-scope alias.
///
/// The executor increments a count when dispatch is attempted (even if the tool
/// later errors). Lua reads the snapshot through the `tools.calls` table.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolCallCounts {
    inner: Arc<Mutex<BTreeMap<String, u64>>>,
}

impl ToolCallCounts {
    /// Creates a counts map pre-seeded with 0 for every alias.
    #[must_use]
    pub(crate) fn new(aliases: impl IntoIterator<Item = String>) -> Self {
        let map: BTreeMap<String, u64> = aliases.into_iter().map(|a| (a, 0)).collect();
        Self {
            inner: Arc::new(Mutex::new(map)),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, u64>>> {
        self.inner
            .lock()
            .map_err(|_| Error::Lua("tool call counts mutex was poisoned".to_owned()))
    }

    /// Ensures `alias` is present in the map, seeding it at 0 when new.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub(crate) fn ensure(&self, alias: &str) -> Result<()> {
        let mut map = self.lock()?;
        map.entry(alias.to_owned()).or_insert(0);
        Ok(())
    }

    /// Increments the count for `alias`.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned or alias is not in scope.
    pub(crate) fn increment(&self, alias: &str) -> Result<()> {
        let mut map = self.lock()?;
        let count = map.get_mut(alias).ok_or_else(|| {
            Error::Lua(format!(
                "tool call counts: alias {alias:?} was not pre-seeded"
            ))
        })?;
        *count += 1;
        Ok(())
    }

    /// Returns the current count for `alias`, or `None` if not in scope.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub(crate) fn get(&self, alias: &str) -> Result<Option<u64>> {
        Ok(self.lock()?.get(alias).copied())
    }

    /// Returns a snapshot of all in-scope aliases.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] if the mutex is poisoned.
    pub(crate) fn aliases(&self) -> Result<Vec<String>> {
        Ok(self.lock()?.keys().cloned().collect())
    }
}

/// Tracks tools added to one section VM and their description overrides.
#[derive(Debug)]
pub(crate) struct ToolRuntime {
    /// Prompt-local aliases currently in the section's tool scope.
    pub(crate) added: Vec<String>,
    /// Per-alias author overrides for model-facing schema descriptions.
    pub(crate) description_overrides: BTreeMap<String, String>,
}
