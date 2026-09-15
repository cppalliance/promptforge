//! The per-profile model memory's file: its on-disk shape, the tolerant
//! load, and the off-executor atomic write.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Name of the persisted server-state file, written in the server's
/// state directory.
pub(super) const WORKSHOP_STATE_FILE: &str = "workshop-state.json";

/// One serialized memory snapshot awaiting its write: the bytes and path
/// are captured under the state lock, and the write runs after the guard
/// drops, off the async executor.
#[derive(Debug)]
pub(super) struct PendingWrite {
    /// Where the memory persists.
    pub(super) path: PathBuf,
    /// The serialized [`WORKSHOP_STATE_FILE`] contents.
    pub(super) bytes: Vec<u8>,
}

/// The persisted shape of [`WORKSHOP_STATE_FILE`]. Server state only:
/// the UI's panel layout is view state and stays in webview localStorage.
#[derive(Debug, Default, serde::Deserialize)]
struct StoredState {
    /// Remembered model selection per profile name.
    #[serde(default)]
    last_selected: HashMap<String, String>,
}

/// Loads the per-profile model memory. A missing, unreadable, or corrupt
/// file means "no memory yet": logged and tolerated (zone two).
pub(super) fn load_memory(path: &Path) -> HashMap<String, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "workshop state unreadable; starting with no model memory"
                );
            }
            return HashMap::new();
        }
    };
    match serde_json::from_str::<StoredState>(&raw) {
        Ok(stored) => stored.last_selected,
        Err(error) => {
            tracing::warn!(
                %error,
                path = %path.display(),
                "workshop state corrupt; starting with no model memory"
            );
            HashMap::new()
        }
    }
}

/// Performs a pending memory write off the async executor. On a runtime
/// the file IO moves to the blocking pool and completes in the
/// background - the memory file is a best-effort cache, so no caller
/// awaits it. Outside a runtime (the unit tests drive the mutators
/// synchronously) the write runs inline instead.
pub(super) fn store_pending(pending: Option<PendingWrite>) {
    let Some(pending) = pending else { return };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(move || store_memory(&pending));
        }
        Err(_) => store_memory(&pending),
    }
}

/// Writes one per-profile model-memory snapshot through the shared
/// atomic-write helper, so a crash mid-write cannot leave a truncated
/// [`WORKSHOP_STATE_FILE`]. A failed write costs the memory, not the
/// process (zone two): logged and tolerated.
fn store_memory(pending: &PendingWrite) {
    if let Err(error) = workshop_support::write_atomic(&pending.path, &pending.bytes) {
        tracing::warn!(
            %error,
            path = %pending.path.display(),
            "workshop state write failed; model memory not persisted"
        );
    }
}
