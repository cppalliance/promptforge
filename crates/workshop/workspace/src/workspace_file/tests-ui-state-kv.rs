//! The three opaque ui-state kv keys: round trip through create, put,
//! close, open; an old file reading all three as absent; and save-as
//! leaving the new file's keys empty.

use serde_json::json;
use workshop_support::{STATE_BUCKET_VALUE_CAP, StateBucketValue};

use super::*;
use crate::Workspace;
use crate::workspace_file::ui_state_kv::UI_STATE_KEYS;

/// A validated put of `text` under `key`, as the route builds one.
fn bucket(key: &str, text: &str) -> StateBucketValue {
    StateBucketValue::new(key, &UI_STATE_KEYS, text.as_bytes()).expect("the fixture is a valid put")
}

/// A contents value with no grants, no window, and no ui state.
fn bare_contents(name: &str) -> WorkspaceContents {
    WorkspaceContents {
        name: name.to_string(),
        grants: Vec::new(),
        window_state: None,
        ui_state: empty_ui_state(),
    }
}

/// Opens `path`, reads its ui state, and closes it again.
async fn reopen_ui_state(path: &Path) -> BTreeMap<&'static str, Option<serde_json::Value>> {
    let file = WorkspaceFile::open(path).await.expect("the file reopens");
    let state = file.read_ui_state().await.expect("ui state reads");
    file.close().await;
    state
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

#[test]
fn the_allow_list_names_exactly_the_three_workspace_keys() {
    assert_eq!(UI_STATE_KEYS, ["layout", "tree", "closed_editors"]);
    assert_eq!(STATE_BUCKET_VALUE_CAP, 1 << 20);
    let empty = empty_ui_state();
    assert_eq!(empty.len(), 3, "the empty map still has every key");
    assert!(empty.values().all(Option::is_none));
}

#[tokio::test]
async fn each_allowed_key_round_trips_through_create_put_close_and_open() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("state.pfwork");
    let file = WorkspaceFile::create(&path, &bare_contents("State"))
        .await
        .expect("creates");
    let values = [
        ("layout", json!({ "version": 3, "zones": { "tree": 240 } })),
        ("tree", json!({ "expanded": ["C:\\projects\\zebra"] })),
        (
            "closed_editors",
            json!({ "paths": ["C:\\projects\\zebra\\a.md"] }),
        ),
    ];

    for (key, value) in &values {
        file.put_ui_state(&bucket(key, &value.to_string()))
            .await
            .expect("an allowed key writes");
    }
    file.close().await;

    let state = reopen_ui_state(&path).await;
    for (key, value) in &values {
        assert_eq!(
            state.get(key).cloned().flatten().as_ref(),
            Some(value),
            "{key} comes back as the JSON that was put"
        );
    }
    assert_eq!(kv_row_count(&path).await, 3, "one kv row per key");
}

#[tokio::test]
async fn a_second_put_replaces_the_first_and_the_text_is_stored_verbatim() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("replace.pfwork");
    let file = WorkspaceFile::create(&path, &bare_contents("Replace"))
        .await
        .expect("creates");

    file.put_ui_state(&bucket("tree", r#"{"expanded":[]}"#))
        .await
        .expect("first put");
    let second = bucket("tree", r#"{"expanded":["C:\\b","C:\\a"]}"#);
    file.put_ui_state(&second).await.expect("second put");
    file.close().await;

    let conn = open_database(&path).await.expect("the closed file opens");
    let mut rows = conn
        .query("SELECT value FROM kv WHERE key = 'tree'", ())
        .await
        .expect("tree row queries");
    let row = rows
        .next()
        .await
        .expect("the row reads")
        .expect("exactly one tree row");
    let stored: String = row.get(0).expect("value is text");
    assert_eq!(
        stored,
        second.text(),
        "the row holds the put's text as it is"
    );
    drop(conn);
    assert_eq!(kv_row_count(&path).await, 1, "a replace adds no row");
}

#[tokio::test]
async fn a_row_that_no_longer_parses_reads_as_none_beside_intact_rows() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("corrupt.pfwork");
    let file = WorkspaceFile::create(&path, &bare_contents("Corrupt"))
        .await
        .expect("creates");
    file.put_ui_state(&bucket("layout", r#"{"version":3}"#))
        .await
        .expect("layout writes");
    file.put_ui_state(&bucket("tree", r#"{"expanded":[]}"#))
        .await
        .expect("tree writes");
    file.put_ui_state(&bucket("closed_editors", r#"{"paths":["C:\\a"]}"#))
        .await
        .expect("closed editors write");
    file.close().await;

    // `put_ui_state` takes only validated JSON, so the only way a row
    // stops parsing is an outside edit of the closed file; seed one
    // directly.
    let conn = open_database(&path).await.expect("the closed file opens");
    conn.execute(
        "UPDATE kv SET value = ?1 WHERE key = 'tree'",
        ("{ \"expanded\": [",),
    )
    .await
    .expect("the tree row is corrupted in place");
    drop(conn);

    let state = reopen_ui_state(&path).await;
    assert_eq!(state.len(), 3, "every key is still present");
    assert_eq!(
        state.get("tree").cloned().flatten(),
        None,
        "the unparseable row reads as absent"
    );
    assert_eq!(
        state.get("layout").cloned().flatten(),
        Some(json!({ "version": 3 })),
        "an intact row beside it still reads"
    );
    assert_eq!(
        state.get("closed_editors").cloned().flatten(),
        Some(json!({ "paths": ["C:\\a"] })),
        "the other intact row still reads"
    );
}

#[tokio::test]
async fn an_old_file_with_no_ui_state_rows_reads_all_three_as_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("old.pfwork");
    // A v1 file as an earlier build wrote it: stamp and window only.
    let conn = open_database(&path).await.expect("seed database opens");
    conn.execute_batch(SCHEMA_V1)
        .await
        .expect("the v1 schema applies");
    for (key, value) in [
        (META_FORMAT, FORMAT_NAME),
        (META_VERSION, SUPPORTED_VERSION),
    ] {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            (key, value),
        )
        .await
        .expect("meta row inserts");
    }
    conn.execute(
        "INSERT INTO kv (key, value) VALUES (?1, ?2)",
        (
            KV_WINDOW,
            r#"{"width":1,"height":2,"x":3,"y":4,"maximized":false}"#,
        ),
    )
    .await
    .expect("window row inserts");
    drop(conn);

    let file = WorkspaceFile::open(&path)
        .await
        .expect("the old file opens");
    let contents = file.contents().await.expect("contents read");
    file.close().await;

    assert_eq!(contents.ui_state, empty_ui_state());
    assert_eq!(
        contents.window_state.map(|state| state.width),
        Some(1),
        "the window row beside them is untouched"
    );
}

#[tokio::test]
async fn save_as_leaves_the_new_files_three_keys_empty() {
    let home = tempfile::TempDir::new().expect("tempdir");
    let first_path = home.path().join("first.pfwork");
    let second_path = home.path().join("second.pfwork");
    let first = WorkspaceFile::create(&first_path, &bare_contents("First"))
        .await
        .expect("the first file creates");
    first
        .put_ui_state(&bucket("layout", r#"{"version":3}"#))
        .await
        .expect("layout writes");
    first
        .put_ui_state(&bucket("tree", r#"{"expanded":["C:\\x"]}"#))
        .await
        .expect("tree writes");
    first
        .put_ui_state(&bucket("closed_editors", r#"{"paths":[]}"#))
        .await
        .expect("closed editors write");
    first.close().await;

    let workspace = Workspace::new();
    workspace
        .open_file(&first_path)
        .await
        .expect("the first file opens as the backing");
    workspace
        .save_as(&second_path)
        .await
        .expect("save as creates the second file");
    workspace.close_backing_for_test().await;

    assert_eq!(
        reopen_ui_state(&second_path).await,
        empty_ui_state(),
        "the SPA is the one writer of these keys after save as"
    );
    assert_eq!(kv_row_count(&second_path).await, 0);
    assert_eq!(
        reopen_ui_state(&first_path)
            .await
            .get("layout")
            .cloned()
            .flatten(),
        Some(json!({ "version": 3 })),
        "the first file keeps what it held"
    );
}
