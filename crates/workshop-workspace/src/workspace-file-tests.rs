use std::fs;
use std::path::PathBuf;

use super::actor::SCHEMA_V1;
use super::*;

#[path = "workspace-file-tests-mutations.rs"]
mod mutations;

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

/// A fully populated contents value: two grants in a deliberate order
/// and a saved window.
fn sample_contents() -> WorkspaceContents {
    WorkspaceContents {
        name: "Sample".to_string(),
        grants: vec![
            GrantRow {
                path: PathBuf::from("C:\\projects\\zebra"),
                position: 0,
                added_at: "2026-09-16T10:00:00Z".to_string(),
            },
            GrantRow {
                path: PathBuf::from("C:\\projects\\apple"),
                position: 1,
                added_at: "2026-09-16T10:00:01Z".to_string(),
            },
        ],
        window_state: Some(WindowState {
            width: 1280,
            height: 800,
            x: 40,
            y: 60,
            maximized: false,
        }),
    }
}

/// Builds a v1-shaped database at `path` with the given meta rows and
/// nothing else, so tests can shape the meta table by hand.
async fn seed_schema_with_meta(path: &Path, meta: &[(&str, &str)]) {
    let conn = open_database(path).await.expect("seed database opens");
    conn.execute_batch(SCHEMA_V1)
        .await
        .expect("the v1 schema applies");
    for (key, value) in meta {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            (*key, *value),
        )
        .await
        .expect("meta row inserts");
    }
}

#[tokio::test]
async fn create_then_open_round_trips_name_grants_and_window_state() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("round-trip.pfwork");
    let expected = sample_contents();

    let created = WorkspaceFile::create(&path, &expected)
        .await
        .expect("a fresh workspace file creates");
    created.close().await;

    let opened = WorkspaceFile::open(&path)
        .await
        .expect("the created workspace file opens");
    let contents = opened.contents().await.expect("contents read");
    opened.close().await;

    assert_eq!(contents.name, expected.name);
    assert_eq!(
        contents.grants, expected.grants,
        "grants keep their positions and order"
    );
    let window = contents.window_state.expect("the saved window comes back");
    let saved = expected.window_state.expect("the sample has a window");
    assert_eq!(
        (
            window.width,
            window.height,
            window.x,
            window.y,
            window.maximized
        ),
        (saved.width, saved.height, saved.x, saved.y, saved.maximized)
    );
}

#[tokio::test]
async fn name_defaults_to_the_file_stem_when_meta_name_is_absent() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("Quarterly Report.pfwork");
    seed_schema_with_meta(
        &path,
        &[
            (META_FORMAT, FORMAT_NAME),
            (META_VERSION, SUPPORTED_VERSION),
        ],
    )
    .await;

    let opened = WorkspaceFile::open(&path)
        .await
        .expect("a v1 file without a name opens");
    let contents = opened.contents().await.expect("contents read");
    opened.close().await;

    assert_eq!(contents.name, "Quarterly Report");
    assert!(contents.grants.is_empty());
    assert!(contents.window_state.is_none());
}

#[tokio::test]
async fn a_database_without_a_meta_table_is_refused_and_left_byte_identical() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("foreign.pfwork");
    {
        let conn = open_database(&path).await.expect("foreign database opens");
        conn.execute_batch("CREATE TABLE notes (body TEXT NOT NULL);")
            .await
            .expect("a foreign table creates");
    }
    let before = fs::read(&path).expect("the foreign file reads");

    let error = WorkspaceFile::open(&path)
        .await
        .expect_err("a database with no meta table is not a workspace");

    assert!(
        matches!(&error, WorkspaceFileError::NotAWorkspace { path: refused } if refused == &path),
        "expected NotAWorkspace naming the path, got {error:?}"
    );
    let after = fs::read(&path).expect("the foreign file still reads");
    assert_eq!(before, after, "a refused open writes nothing to the file");
}

#[tokio::test]
async fn a_random_bytes_file_is_refused() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("noise.pfwork");
    // A deterministic byte soup that is neither empty nor a SQLite header.
    let noise: Vec<u8> = (0..4096u32)
        .map(|i| u8::try_from((i.wrapping_mul(2_654_435_761) >> 13) & 0xff).unwrap_or(0))
        .collect();
    fs::write(&path, &noise).expect("noise writes");

    let error = WorkspaceFile::open(&path)
        .await
        .expect_err("random bytes are not a workspace");

    assert!(
        matches!(error, WorkspaceFileError::NotAWorkspace { .. }),
        "expected NotAWorkspace, got {error:?}"
    );
    assert_eq!(
        fs::read(&path).expect("noise still reads"),
        noise,
        "a refused open leaves the bytes alone"
    );
}

#[tokio::test]
async fn meta_version_two_is_refused_as_unsupported_version() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("future.pfwork");
    seed_schema_with_meta(&path, &[(META_FORMAT, FORMAT_NAME), (META_VERSION, "2")]).await;

    let error = WorkspaceFile::open(&path)
        .await
        .expect_err("a newer schema version is refused");

    assert!(
        matches!(
            &error,
            WorkspaceFileError::UnsupportedVersion { found, supported }
                if found == "2" && *supported == SUPPORTED_VERSION
        ),
        "expected UnsupportedVersion {{ found: \"2\", supported: \"1\" }}, got {error:?}"
    );
}

#[tokio::test]
async fn create_leaves_only_the_file_in_the_directory() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("solo.pfwork");

    let created = WorkspaceFile::create(&path, &sample_contents())
        .await
        .expect("a fresh workspace file creates");
    created.close().await;

    // No directory ceremony: the workspace is the file. Close checkpoints
    // and removes the engine's `-wal` sidecar, so nothing else remains.
    let mut names = Vec::new();
    for entry in fs::read_dir(dir.path()).expect("the tempdir lists") {
        let entry = entry.expect("entry reads");
        assert!(
            entry.file_type().expect("entry type reads").is_file(),
            "create made a directory: {}",
            entry.path().display()
        );
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    assert_eq!(names, vec!["solo.pfwork".to_string()]);
}

#[tokio::test]
async fn a_failed_create_removes_the_half_written_file_so_a_retry_succeeds() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("retry.pfwork");
    // Two grants under one canonical path collide on the grants primary
    // key, so the write fails after the file and schema already exist.
    let mut colliding = sample_contents();
    colliding.grants[1].path = colliding.grants[0].path.clone();

    let error = WorkspaceFile::create(&path, &colliding)
        .await
        .expect_err("a primary-key collision fails the create");

    assert!(
        matches!(error, WorkspaceFileError::Database { .. }),
        "expected the engine failure, got {error:?}"
    );
    assert!(!path.exists(), "a failed create leaves no file behind");
    assert!(
        fs::read_dir(dir.path())
            .expect("the tempdir lists")
            .next()
            .is_none(),
        "a failed create leaves no sidecar behind"
    );

    let retried = WorkspaceFile::create(&path, &sample_contents())
        .await
        .expect("the same path is free again after the failure");
    retried.close().await;
}

#[tokio::test]
async fn create_refuses_an_existing_path() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("taken.pfwork");
    fs::write(&path, b"already here").expect("placeholder writes");

    let error = WorkspaceFile::create(&path, &sample_contents())
        .await
        .expect_err("create never overwrites");

    assert!(
        matches!(&error, WorkspaceFileError::Io { source } if source.kind() == io::ErrorKind::AlreadyExists),
        "expected an AlreadyExists I/O failure, got {error:?}"
    );
    assert_eq!(fs::read(&path).expect("placeholder reads"), b"already here");
}
