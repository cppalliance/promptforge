//! The user-state store: the in-memory document, its tolerant load, and
//! the off-executor atomic rewrite on every put.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::error::UserStateError;

/// The keys the user bucket accepts.
pub const USER_STATE_KEYS: [&str; 4] = [
    "editor_settings",
    "zoom",
    "recent_files",
    "commands_history",
];

/// The largest user-state value the store accepts, in bytes of JSON text.
pub const USER_STATE_VALUE_CAP: usize = 1 << 20;

/// Name of the persisted user-state file, written in the server's state
/// directory.
pub(crate) const USER_STATE_FILE: &str = "ui-state.json";

/// The account-scoped UI state document: one JSON object in the state
/// directory, held in memory and rewritten whole on every put.
///
/// The in-memory map is the source of truth. A key this build does not
/// know is kept in the map and rewritten with the rest, so a newer
/// build's value survives a round trip through this one, but only the
/// allow-listed keys are served.
#[derive(Debug)]
pub struct UserStateStore {
    /// Where the document persists: `state_dir/ui-state.json`.
    path: PathBuf,
    /// The current document, under the single writer's lock. The lock is
    /// held across the write so two puts cannot land their rewrites out
    /// of order.
    state: Mutex<Map<String, Value>>,
}

impl UserStateStore {
    /// Opens the store over `state_dir/ui-state.json`, reading the file
    /// tolerantly: a missing, unreadable, or corrupt file means "no state
    /// yet" - logged and tolerated (zone two). Nothing is created until
    /// the first put.
    #[must_use]
    pub fn new(state_dir: &Path) -> Self {
        let path = state_dir.join(USER_STATE_FILE);
        let state = load_document(&path);
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    /// Every allow-listed key with its stored value, `None` when never
    /// saved.
    pub async fn get_all(&self) -> BTreeMap<&'static str, Option<Value>> {
        let state = self.state.lock().await;
        USER_STATE_KEYS
            .iter()
            .map(|key| (*key, state.get(*key).cloned()))
            .collect()
    }

    /// Stores `value` under the allow-listed `key`, replacing any earlier
    /// value, and rewrites the whole document atomically. Validation runs
    /// before the lock is taken, so a refused put writes nothing. A
    /// failed write leaves the new value in memory: the map is the source
    /// of truth and the file is its mirror.
    ///
    /// # Errors
    /// Returns [`UserStateError::Key`] for a key outside
    /// [`USER_STATE_KEYS`], [`UserStateError::TooLarge`] when the value's
    /// JSON text exceeds [`USER_STATE_VALUE_CAP`], and
    /// [`UserStateError::Io`] when the write fails.
    pub async fn put(&self, key: &str, value: Value) -> Result<(), UserStateError> {
        let key = user_state_key(key)?;
        check_value_cap(&value)?;
        let mut state = self.state.lock().await;
        state.insert(key.to_owned(), value);
        // Serializing a map of already-parsed values cannot fail; a
        // failure here is a serde_json invariant break, reported as I/O
        // rather than panicking the server.
        let bytes =
            serde_json::to_vec(&*state).map_err(|error| UserStateError::Io(error.into()))?;
        let path = self.path.clone();
        let written =
            tokio::task::spawn_blocking(move || workshop_support::write_atomic(&path, &bytes))
                .await
                .unwrap_or_else(|join| Err(io::Error::other(join)));
        drop(state);
        written.map_err(|error| {
            tracing::warn!(
                %error,
                path = %self.path.display(),
                "user state write failed; value kept in memory, not persisted"
            );
            UserStateError::Io(error)
        })
    }
}

/// Resolves `key` to its allow-list entry.
///
/// # Errors
/// Returns [`UserStateError::Key`] when `key` is not one of
/// [`USER_STATE_KEYS`].
fn user_state_key(key: &str) -> Result<&'static str, UserStateError> {
    USER_STATE_KEYS
        .iter()
        .copied()
        .find(|allowed| *allowed == key)
        .ok_or_else(|| UserStateError::Key(key.to_owned()))
}

/// Checks that `value`'s JSON text fits under the cap.
///
/// # Errors
/// Returns [`UserStateError::TooLarge`] past the cap.
fn check_value_cap(value: &Value) -> Result<(), UserStateError> {
    // The compact serialization is what the document holds, so its
    // length is the size the cap governs.
    let actual = value.to_string().len();
    if actual > USER_STATE_VALUE_CAP {
        return Err(UserStateError::TooLarge {
            actual,
            cap: USER_STATE_VALUE_CAP,
        });
    }
    Ok(())
}

/// Loads the document at `path`. A missing file is the ordinary "no
/// state yet" and is silent; an unreadable file, one that does not parse,
/// or one whose top level is not an object is corrupt state: logged once
/// and read as empty, to be replaced whole by the next put.
fn load_document(path: &Path) -> Map<String, Value> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "user state unreadable; starting with no state"
                );
            }
            return Map::new();
        }
    };
    match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(document)) => document,
        Ok(other) => {
            tracing::warn!(
                path = %path.display(),
                found = other_kind(&other),
                "user state is not a JSON object; starting with no state"
            );
            Map::new()
        }
        Err(error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "user state corrupt; starting with no state"
            );
            Map::new()
        }
    }
}

/// The JSON kind of a value that is not the object the document requires,
/// for the corrupt-shape warning.
fn other_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[path = "store-tests.rs"]
mod tests;
