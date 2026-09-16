use super::*;

#[tokio::test]
async fn turso_opens_a_tempdir_database_and_round_trips_user_version() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("spike.pfwork");

    let conn = open_database(&path)
        .await
        .expect("a fresh database opens at a path that does not exist yet");
    conn.execute("PRAGMA user_version = 7", ())
        .await
        .expect("user_version is writable");
    drop(conn);

    // A second open through a fresh connection proves the write reached
    // the file rather than living in the first connection's state.
    let reopened = open_database(&path)
        .await
        .expect("the written database reopens");
    let mut rows = reopened
        .query("PRAGMA user_version", ())
        .await
        .expect("user_version is readable");
    let row = rows
        .next()
        .await
        .expect("the pragma row reads")
        .expect("the pragma yields exactly one row");
    let version: i64 = row.get(0).expect("user_version is an integer");
    assert_eq!(version, 7);

    assert!(
        path.is_file(),
        "the database is a regular file at the chosen path"
    );
}

#[tokio::test]
async fn open_database_refuses_a_path_whose_parent_is_missing() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("missing").join("spike.pfwork");

    let error = open_database(&path)
        .await
        .expect_err("a database cannot be created under a directory that does not exist");
    // The path is UTF-8, so the only failure left is the engine refusing
    // to create the file under a directory that is not there.
    assert!(
        matches!(error, WorkspaceFileError::Database { .. }),
        "a missing parent surfaces as an engine failure, got {error:?}"
    );
    assert!(!path.exists(), "a refused open creates nothing");
}
