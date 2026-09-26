//! The workspace-level ui-state API over the backing file: an ephemeral
//! put is a no-op that says so, a file-backed put lands in the file and
//! in `ui_state()`, open reads the file's values into memory, a failed
//! persist leaves the in-memory value standing, and save-as and
//! duplicate start their new backing with the expected values.

use std::collections::BTreeMap;

use serde_json::json;
use workshop_support::StateBucketValue;

use super::*;

use crate::workspace_file::{
    UI_STATE_KEYS, WorkspaceContents, WorkspaceFile, empty_ui_state, open_database,
};

/// A validated put of `value` under `key`, as the route builds one.
fn bucket(key: &str, value: &serde_json::Value) -> StateBucketValue {
    StateBucketValue::new(key, &UI_STATE_KEYS, value.to_string().as_bytes())
        .expect("the fixture is a valid put")
}

/// Opens the file at `path` directly, bypassing any `Workspace`, and
/// returns the ui-state values it holds.
async fn file_ui_state(path: &Path) -> BTreeMap<&'static str, Option<serde_json::Value>> {
    let file = WorkspaceFile::open(path)
        .await
        .expect("the workspace file reopens on its own");
    let contents = file.contents().await.expect("contents read");
    file.close().await;
    contents.ui_state
}

/// How many rows the kv table of the closed file at `path` holds.
async fn kv_row_count(path: &Path) -> i64 {
    let conn = open_database(path).await.expect("the closed file opens");
    let mut rows = conn
        .query("SELECT count(*) FROM kv", ())
        .await
        .expect("kv counts");
    let row = rows
        .next()
        .await
        .expect("the count row reads")
        .expect("count yields one row");
    row.get(0).expect("count is an integer")
}

/// One distinct value per allow-listed key.
fn sample_values() -> [(&'static str, serde_json::Value); 3] {
    [
        ("layout", json!({ "version": 3, "zones": { "tree": 240 } })),
        ("tree", json!({ "expanded": ["C:\\projects\\zebra"] })),
        (
            "closed_editors",
            json!({ "paths": ["C:\\projects\\zebra\\a.md"] }),
        ),
    ]
}

#[tokio::test]
async fn put_while_ephemeral_returns_false_and_writes_nothing() {
    let workspace = Workspace::new();
    assert_eq!(
        workspace.ui_state(),
        empty_ui_state(),
        "an ephemeral workspace has every key unset"
    );

    for (key, value) in sample_values() {
        let saved = workspace.put_ui_state(bucket(key, &value)).await;
        assert!(!saved, "nothing was written, and the caller is told so");
    }

    assert_eq!(
        workspace.ui_state(),
        empty_ui_state(),
        "an ephemeral put leaves no value behind, matching window state"
    );
    assert_eq!(workspace.current().await.path, None);
}

#[tokio::test]
async fn put_with_a_file_open_persists_and_ui_state_reflects_it() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let path = home.path().join("mine.pfwork");
    let workspace = Workspace::new();
    workspace.save_as(&path).await.expect("save as creates");
    assert_eq!(
        workspace.ui_state(),
        empty_ui_state(),
        "a fresh file starts with no values"
    );

    for (key, value) in sample_values() {
        let saved = workspace.put_ui_state(bucket(key, &value)).await;
        assert!(saved, "a file-backed put reports that it wrote");
        assert_eq!(
            workspace.ui_state().get(key).cloned().flatten(),
            Some(value),
            "{key} is visible in memory right after the put"
        );
    }
    // A second put replaces the first in memory and on disk.
    let replaced = json!({ "expanded": [] });
    assert!(
        workspace.put_ui_state(bucket("tree", &replaced)).await,
        "the replace writes"
    );
    assert_eq!(
        workspace.ui_state().get("tree").cloned().flatten(),
        Some(replaced.clone())
    );
    workspace.close_backing_for_test().await;

    let on_disk = file_ui_state(&path).await;
    for (key, value) in sample_values() {
        let expected = if key == "tree" { &replaced } else { &value };
        assert_eq!(
            on_disk.get(key).cloned().flatten().as_ref(),
            Some(expected),
            "{key} reopened from the file matches what was put"
        );
    }
    assert_eq!(kv_row_count(&path).await, 3, "one kv row per key");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlapping_puts_to_one_key_leave_the_file_holding_what_memory_holds() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let path = home.path().join("race.pfwork");
    let workspace = Workspace::new();
    workspace.save_as(&path).await.expect("save as creates");

    // Many puts to the same key started together from several tasks:
    // whichever lands in memory last must also be the row on disk, or
    // the next open restores stale state. Repeated rounds give an
    // unserialized put path many chances to interleave its memory
    // insert and its actor send the other way round.
    for round in 0..8 {
        let puts = (0..16).map(|n| {
            let workspace = workspace.clone();
            tokio::spawn(async move {
                workspace
                    .put_ui_state(bucket("tree", &json!({ "round": round, "n": n })))
                    .await
            })
        });
        for put in puts {
            assert!(put.await.expect("the put task finishes"));
        }
    }

    let in_memory = workspace.ui_state().get("tree").cloned().flatten();
    assert!(
        in_memory.is_some(),
        "one of the puts is the value memory holds"
    );
    workspace.close_backing_for_test().await;
    assert_eq!(
        file_ui_state(&path).await.get("tree").cloned().flatten(),
        in_memory,
        "the file's row is the value memory last accepted"
    );
    assert_eq!(kv_row_count(&path).await, 1, "one row, replaced in place");
}

#[tokio::test]
async fn open_reads_a_files_ui_state_into_memory_and_a_later_open_replaces_it() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let full_path = home.path().join("full.pfwork");
    let empty_path = home.path().join("empty.pfwork");
    // A file written through the storage layer alone, as Step 1 does.
    let file = WorkspaceFile::create(
        &full_path,
        &WorkspaceContents {
            name: "Full".to_string(),
            grants: Vec::new(),
            window_state: None,
            ui_state: empty_ui_state(),
        },
    )
    .await
    .expect("the full file creates");
    for (key, value) in sample_values() {
        file.put_ui_state(&bucket(key, &value))
            .await
            .expect("the storage layer writes");
    }
    file.close().await;
    let empty = WorkspaceFile::create(
        &empty_path,
        &WorkspaceContents {
            name: "Empty".to_string(),
            grants: Vec::new(),
            window_state: None,
            ui_state: empty_ui_state(),
        },
    )
    .await
    .expect("the empty file creates");
    empty.close().await;

    let workspace = Workspace::new();
    workspace
        .open_file(&full_path)
        .await
        .expect("the full file opens");
    let state = workspace.ui_state();
    assert_eq!(state.len(), UI_STATE_KEYS.len(), "every key is present");
    for (key, value) in sample_values() {
        assert_eq!(
            state.get(key).cloned().flatten(),
            Some(value),
            "{key} was read at open"
        );
    }

    workspace
        .open_file(&empty_path)
        .await
        .expect("the empty file opens");
    assert_eq!(
        workspace.ui_state(),
        empty_ui_state(),
        "opening another file replaces the values wholesale"
    );
}

#[tokio::test]
async fn a_failed_persist_leaves_the_in_memory_value_standing() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let path = home.path().join("closed.pfwork");
    let workspace = Workspace::new();
    workspace.save_as(&path).await.expect("save as creates");
    // Stop the actor while leaving the backing in place: every later
    // persist fails with a closed file.
    workspace.close_backing_for_test().await;

    let value = json!({ "version": 3 });
    let saved = workspace.put_ui_state(bucket("layout", &value)).await;
    assert!(
        saved,
        "the workspace is file-backed, so the put is accepted"
    );
    assert_eq!(
        workspace.ui_state().get("layout").cloned().flatten(),
        Some(value),
        "memory is the source of truth and stands"
    );
    assert_eq!(
        file_ui_state(&path).await.get("layout").cloned().flatten(),
        None,
        "the closed file never saw the value"
    );
}

#[tokio::test]
async fn save_as_starts_empty_and_duplicate_keeps_the_live_values() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let first_path = home.path().join("first.pfwork");
    let second_path = home.path().join("second.pfwork");
    let copy_path = home.path().join("copy.pfwork");
    let workspace = Workspace::new();
    workspace
        .save_as(&first_path)
        .await
        .expect("the first save as creates");
    let layout = json!({ "version": 3 });
    assert!(
        workspace.put_ui_state(bucket("layout", &layout)).await,
        "layout writes into the first file"
    );

    workspace
        .save_as(&second_path)
        .await
        .expect("the second save as creates");
    assert_eq!(
        workspace.ui_state(),
        empty_ui_state(),
        "save as starts the new backing with no values; the SPA rewrites them"
    );

    workspace
        .open_file(&first_path)
        .await
        .expect("the first file reopens");
    workspace
        .duplicate(&copy_path)
        .await
        .expect("the duplicate opens");
    assert_eq!(
        workspace.ui_state().get("layout").cloned().flatten(),
        Some(layout.clone()),
        "duplicate keeps the live values, as it keeps the live grants"
    );
    workspace.close_backing_for_test().await;
    assert_eq!(
        file_ui_state(&copy_path)
            .await
            .get("layout")
            .cloned()
            .flatten(),
        Some(layout),
        "the copied file stores the row too"
    );
}
