//! The user-state store's file round trip and its tolerated failures:
//! a missing or corrupt file reads as empty, a put writes the whole
//! document atomically, and a refused put touches nothing.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

use super::*;

/// Every file name in `dir`, sorted, so a test can assert exactly what a
/// write left behind.
fn dir_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("readable dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// The map every key present and no value: what an empty store serves.
fn all_null() -> BTreeMap<&'static str, Option<Value>> {
    USER_STATE_KEYS.iter().map(|key| (*key, None)).collect()
}

#[tokio::test]
async fn a_missing_file_yields_all_null_and_creates_nothing() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = UserStateStore::new(dir.path());
    assert_eq!(store.all().await, all_null());
    assert!(
        dir_names(dir.path()).is_empty(),
        "construction alone writes no file"
    );
}

#[tokio::test]
async fn a_put_round_trips_through_a_fresh_store() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    {
        let store = UserStateStore::new(dir.path());
        store
            .put("zoom", json!(1.25))
            .await
            .expect("an allow-listed value under the cap is stored");
        store
            .put("recent_files", json!(["C:/a.md", "C:/b.md"]))
            .await
            .expect("an allow-listed value under the cap is stored");
    }
    let reborn = UserStateStore::new(dir.path());
    let state = reborn.all().await;
    assert_eq!(state["zoom"], Some(json!(1.25)));
    assert_eq!(state["recent_files"], Some(json!(["C:/a.md", "C:/b.md"])));
    assert_eq!(
        state["editor_settings"], None,
        "an unset key reads as absent"
    );
    assert_eq!(
        state["commands_history"], None,
        "an unset key reads as absent"
    );
}

#[tokio::test]
async fn a_corrupt_file_yields_all_null_and_the_next_put_replaces_it() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join(USER_STATE_FILE);
    std::fs::write(&path, "not json {").expect("write fixture");
    let store = UserStateStore::new(dir.path());
    assert_eq!(
        store.all().await,
        all_null(),
        "corrupt state degrades to no state, never to a failure"
    );
    store
        .put("zoom", json!(2))
        .await
        .expect("the put succeeds over a corrupt file");
    let document: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("readable")).expect("valid json");
    assert_eq!(
        document,
        json!({ "zoom": 2 }),
        "the corrupt text is replaced by the whole current document"
    );
}

#[tokio::test]
async fn a_non_object_document_yields_all_null() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join(USER_STATE_FILE), "[1, 2, 3]").expect("write fixture");
    let store = UserStateStore::new(dir.path());
    assert_eq!(
        store.all().await,
        all_null(),
        "valid JSON of the wrong shape is corrupt state"
    );
}

#[tokio::test]
async fn a_write_leaves_no_temp_and_the_file_holds_the_full_document() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = UserStateStore::new(dir.path());
    store
        .put("editor_settings", json!({ "wordWrap": "on" }))
        .await
        .expect("stored");
    store
        .put("commands_history", json!(["workbench.action.files.save"]))
        .await
        .expect("stored");
    assert_eq!(
        dir_names(dir.path()),
        [USER_STATE_FILE],
        "the state file is the only thing left behind; no .pf-tmp remains"
    );
    let document: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(USER_STATE_FILE)).expect("readable"))
            .expect("valid json");
    assert_eq!(
        document,
        json!({
            "editor_settings": { "wordWrap": "on" },
            "commands_history": ["workbench.action.files.save"],
        }),
        "each put rewrites the whole document, so the earlier key survives the later put"
    );
}

#[tokio::test]
async fn a_disallowed_key_is_refused_without_a_write() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = UserStateStore::new(dir.path());
    let error = store
        .put("layout", json!({}))
        .await
        .expect_err("a workspace-bucket key is not a user-bucket key");
    assert!(
        matches!(&error, UserStateError::Key(key) if key == "layout"),
        "the refusal names the key: {error:?}"
    );
    assert!(
        error.to_string().contains("editor_settings"),
        "the message lists the allow-list: {error}"
    );
    assert!(
        dir_names(dir.path()).is_empty(),
        "a refused put creates no file"
    );
    assert_eq!(
        store.all().await,
        all_null(),
        "a refused put stores nothing"
    );
}

#[tokio::test]
async fn an_over_cap_value_is_refused_without_a_write() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = UserStateStore::new(dir.path());
    let oversized = Value::String("x".repeat(USER_STATE_VALUE_CAP));
    let error = store
        .put("zoom", oversized)
        .await
        .expect_err("a value whose JSON text exceeds the cap is refused");
    assert!(
        matches!(
            error,
            UserStateError::TooLarge { actual, cap }
                if actual == USER_STATE_VALUE_CAP + 2 && cap == USER_STATE_VALUE_CAP
        ),
        "the refusal names the actual size and the cap: {error:?}"
    );
    assert!(
        dir_names(dir.path()).is_empty(),
        "a refused put creates no file"
    );
    assert_eq!(
        store.all().await,
        all_null(),
        "a refused put stores nothing"
    );
}

#[tokio::test]
async fn a_value_exactly_at_the_cap_is_accepted() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let store = UserStateStore::new(dir.path());
    // A JSON string's text is its content plus two quotes.
    let at_cap = Value::String("x".repeat(USER_STATE_VALUE_CAP - 2));
    store
        .put("zoom", at_cap.clone())
        .await
        .expect("a value at the cap is under the limit, not over it");
    assert_eq!(store.all().await["zoom"], Some(at_cap));
}

#[tokio::test]
async fn an_unwritable_state_dir_reports_io_and_keeps_the_value_in_memory() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A directory in the file's place: the rename over it fails for a
    // reason other than NotFound.
    std::fs::create_dir(dir.path().join(USER_STATE_FILE)).expect("directory in the file's place");
    let store = UserStateStore::new(dir.path());
    let error = store
        .put("zoom", json!(1))
        .await
        .expect_err("renaming over a directory must fail");
    assert!(
        matches!(error, UserStateError::Io(_)),
        "the write failure is reported as I/O: {error:?}"
    );
    assert_eq!(
        store.all().await["zoom"],
        Some(json!(1)),
        "the in-memory state is the source of truth; a failed persist is degradation"
    );
}

#[tokio::test]
async fn unknown_keys_in_the_file_are_preserved_but_not_served() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join(USER_STATE_FILE);
    std::fs::write(&path, r#"{"zoom": 3, "future_key": true}"#).expect("write fixture");
    let store = UserStateStore::new(dir.path());
    let state = store.all().await;
    assert_eq!(state["zoom"], Some(json!(3)));
    assert_eq!(
        state.len(),
        USER_STATE_KEYS.len(),
        "only allow-listed keys are served"
    );
    store.put("zoom", json!(4)).await.expect("stored");
    let document: Value =
        serde_json::from_slice(&std::fs::read(&path).expect("readable")).expect("valid json");
    assert_eq!(
        document,
        json!({ "zoom": 4, "future_key": true }),
        "a key this build does not know survives a rewrite"
    );
}
