//! The per-profile model memory's file: its on-disk shape, the tolerant
//! load, and the off-executor atomic write.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// Name of the persisted server-state file, written in the server's
/// state directory.
pub(super) const WORKSHOP_STATE_FILE: &str = "workshop-state.json";

/// The memory file's writer: where the memory persists, the sequence the
/// next snapshot takes, and the write gate its pending writes share. It
/// lives in the menu state, so snapshots take their sequences under the
/// state lock, in the order their contents were captured.
#[derive(Debug)]
pub(super) struct MemoryWriter {
    /// Where the memory persists.
    path: PathBuf,
    /// The sequence of the last snapshot taken; 0 before the first.
    last_taken: u64,
    /// Shared with every pending write; see [`PendingWrite`].
    gate: Arc<Mutex<u64>>,
}

impl MemoryWriter {
    /// A writer for the memory file at `path`, with nothing taken or
    /// written yet.
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_taken: 0,
            gate: Arc::default(),
        }
    }

    /// Takes `bytes` as the newest snapshot awaiting its write.
    pub(super) fn pending(&mut self, bytes: Vec<u8>) -> PendingWrite {
        self.last_taken += 1;
        PendingWrite {
            path: self.path.clone(),
            bytes,
            sequence: self.last_taken,
            gate: Arc::clone(&self.gate),
        }
    }
}

/// One serialized memory snapshot awaiting its write: the bytes, path,
/// and sequence are captured under the state lock, and the write runs
/// after the guard drops, off the async executor.
#[derive(Debug)]
pub(super) struct PendingWrite {
    /// Where the memory persists.
    path: PathBuf,
    /// The serialized [`WORKSHOP_STATE_FILE`] contents.
    bytes: Vec<u8>,
    /// The snapshot's place in capture order, starting at 1.
    sequence: u64,
    /// The memory file's write gate: held across each write so writes
    /// never interleave, it holds the sequence of the last snapshot
    /// written, 0 before the first.
    gate: Arc<Mutex<u64>>,
}

/// The persisted shape of [`WORKSHOP_STATE_FILE`]. Server state only:
/// the UI's panel layout is view state and persists through the UI-state
/// buckets (`workshop-user-state` and the workspace file), never here.
#[derive(Debug, Default, serde::Deserialize)]
struct StoredState {
    /// Remembered model selection per profile name.
    #[serde(default)]
    last_selected: HashMap<String, String>,
}

/// Loads the per-profile model memory. A missing, unreadable, or corrupt
/// file means "no memory yet": logged and tolerated.
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
/// [`WORKSHOP_STATE_FILE`]. Pending writes reach the blocking pool in
/// no guaranteed order, so the gate serializes them and skips a snapshot
/// no newer than the last one written: the newest snapshot wins. A
/// failed write costs the memory, not the process: logged and tolerated.
pub(super) fn store_memory(pending: &PendingWrite) {
    let mut last_written = pending.gate.lock().unwrap_or_else(PoisonError::into_inner);
    if pending.sequence <= *last_written {
        return;
    }
    match workshop_support::write_atomic(&pending.path, &pending.bytes) {
        Ok(()) => *last_written = pending.sequence,
        Err(error) => tracing::warn!(
            %error,
            path = %pending.path.display(),
            "workshop state write failed; model memory not persisted"
        ),
    }
}
