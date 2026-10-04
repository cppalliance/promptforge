//! Guard helper tests: capture, child `PATH`, readiness, and the production command.

use super::{
    capture_reader, child_path_with_prefix, new_capture, production_command, readiness_lists_model,
};
use crate::server::SpawnRequest;
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

struct ErroringReader;

impl Read for ErroringReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("capture stream boom"))
    }
}

struct EofReader;

impl Read for EofReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }
}

#[test]
fn capture_reader_surfaces_read_errors_on_join() {
    // SERVER-005: a genuine capture read failure is returned on join, not
    // silently swallowed by ending the loop.
    let handle = capture_reader("test-stream", ErroringReader, new_capture()).expect("spawn");
    assert!(handle.join().expect("thread joined").is_err());
}

#[test]
fn capture_reader_reports_clean_eof_as_ok() {
    // Normal completion (child pipe closed) is EOF, an `Ok(())` result.
    let handle = capture_reader("test-stream", EofReader, new_capture()).expect("spawn");
    assert!(handle.join().expect("thread joined").is_ok());
}

#[test]
fn readiness_lists_model_matches_alias_and_tolerates_junk() {
    let body = br#"{"object":"list","data":[{"id":"promptforge-local-abc"}]}"#;
    assert!(readiness_lists_model(body, "promptforge-local-abc"));
    assert!(!readiness_lists_model(body, "some-other-alias"));
    // Missing `data`, empty body, and truncated JSON all read as not-ready.
    assert!(!readiness_lists_model(br#"{"object":"list"}"#, "x"));
    assert!(!readiness_lists_model(b"", "x"));
    assert!(!readiness_lists_model(
        br#"{"data":[{"id":"promptforge"#,
        "x"
    ));
}

#[test]
fn child_path_with_prefix_orders_prefix_before_inherited() {
    let prefix = vec![PathBuf::from("staged"), PathBuf::from("toolkit-bin")];
    let inherited =
        std::env::join_paths([PathBuf::from("c"), PathBuf::from("d")]).expect("join inherited");
    let joined = child_path_with_prefix(&prefix, Some(inherited)).expect("join child path");
    let entries: Vec<PathBuf> = std::env::split_paths(&joined).collect();
    assert_eq!(
        entries,
        vec![
            PathBuf::from("staged"),
            PathBuf::from("toolkit-bin"),
            PathBuf::from("c"),
            PathBuf::from("d"),
        ]
    );
}

#[test]
fn child_path_with_prefix_without_inherited_is_just_the_prefix() {
    let prefix = vec![PathBuf::from("staged")];
    let joined = child_path_with_prefix(&prefix, None).expect("join child path");
    let entries: Vec<PathBuf> = std::env::split_paths(&joined).collect();
    assert_eq!(entries, vec![PathBuf::from("staged")]);
}

#[test]
fn production_command_prepends_path_to_child_env_only() {
    let before = std::env::var_os("PATH");
    let args = [OsString::from("--version")];
    let prefix = [PathBuf::from("staged-dir"), PathBuf::from("toolkit-bin")];
    let request = SpawnRequest {
        executable: Path::new("llama-server"),
        args: &args,
        path_prefix: &prefix,
        port: 0,
        model_alias: "env-test",
        api_key: "env-test",
    };
    let command = production_command(&request).expect("build child command");

    // The process-global environment is never mutated.
    assert_eq!(std::env::var_os("PATH"), before);

    let child_path = command
        .get_envs()
        .find(|(key, _)| *key == OsStr::new("PATH"))
        .and_then(|(_, value)| value)
        .expect("child PATH is set")
        .to_owned();
    let entries: Vec<PathBuf> = std::env::split_paths(&child_path).collect();
    assert_eq!(entries[..2], prefix[..]);
    if let Some(inherited) = before {
        let inherited_entries: Vec<PathBuf> = std::env::split_paths(&inherited).collect();
        assert!(entries.ends_with(&inherited_entries));
    }
}

#[test]
fn production_command_with_empty_prefix_leaves_child_path_inherited() {
    let args = [OsString::from("--version")];
    let request = SpawnRequest {
        executable: Path::new("llama-server"),
        args: &args,
        path_prefix: &[],
        port: 0,
        model_alias: "env-test",
        api_key: "env-test",
    };
    let command = production_command(&request).expect("build child command");
    assert!(
        command.get_envs().all(|(key, _)| key != OsStr::new("PATH")),
        "an empty prefix must not override the child's inherited PATH"
    );
}
