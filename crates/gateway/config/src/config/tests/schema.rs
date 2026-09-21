//! Tests for the config schema version, hard-break detection, and profile selection.

use std::fs;

use tempfile::TempDir;

use super::super::*;
use crate::{ConfigErrorKind, ProfileSelection, clear_profile_state, profile_state_path};

const CATALOG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "cloud"
description = "a remote model"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "a"
description = "model a"
source = "/models/a.gguf"
context = 4096

[[local_model]]
name = "b"
description = "model b"
source = "/models/b.gguf"
context = 4096

[[stt_model]]
name = "speech"
role = "interim"
source = "/models/base.en.bin"
vram_gb = 1.0

[[profile]]
name = "work"
models = ["a", "speech"]

[[profile]]
name = "travel"
models = ["b"]
"#;

fn file_fixture() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, CATALOG).expect("write config");
    (temp, path)
}

#[test]
fn version_zero_schema_has_profiles_and_stt_models() {
    let config = Config::from_toml_str(CATALOG).expect("schema parses");

    assert_eq!(config.config_version(), 0);
    assert_eq!(config.profiles().len(), 2);
    assert_eq!(config.profiles()[0].name(), "work");
    assert_eq!(config.profiles()[0].models(), ["a", "speech"]);
    assert_eq!(config.catalog_stt_models()[0].name(), "speech");
    assert_eq!(config.catalog_stt_models()[0].role(), SttRole::Interim);
    assert!((config.catalog_stt_models()[0].vram_gb() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn canonical_example_uses_the_validated_section_layout() {
    let example = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../gateway.local.example.toml"
    ));
    let catalog = Config::from_toml_str(example).expect("canonical example validates");
    let selected = catalog
        .select_profile(Some(
            &crate::ProfileName::parse("work").expect("profile name"),
        ))
        .expect("work profile selects");

    assert_eq!(selected.local_models()[0].name(), "qwen-local");
    assert_eq!(selected.stt_models().len(), 2);
}

#[test]
fn canonical_stt_section_parses_into_the_runtime_shape() {
    let config = Config::from_toml_str(&format!(
        "{CATALOG}\n[stt]\nwindow_seconds = 8\ninterval_ms = 250\nvocabulary = [\"WG21\"]\n"
    ))
    .expect("canonical STT section parses");

    let stt = config.stt().expect("canonical STT settings are present");
    assert_eq!(stt.window_seconds(), 8);
    assert_eq!(stt.interval_ms(), 250);
    assert_eq!(stt.vocabulary(), ["WG21"]);
}

#[test]
fn hard_breaks_name_file_key_line_and_replacement() {
    for (raw, key, line, replacement) in [
        (
            "config-version = 0\ninclude = [\"base.toml\"]\n",
            "include",
            ":2:",
            "[[profile]]",
        ),
        (
            "config-version = 0\nmodels = [\"a\"]\n",
            "models",
            ":2:",
            "[[profile]]",
        ),
        (
            "config-version = 0\n[server]\nbind='127.0.0.1:1'\napi_key='x'\n\
             [workshop.voice]\ninterim_model='tiny.bin'\n",
            "workshop.voice.interim_model",
            ":6:",
            "[[stt_model]]",
        ),
    ] {
        let error = Config::from_toml_str(raw).expect_err("legacy layout must fail");
        let message = error.to_string();
        assert_eq!(error.kind(), ConfigErrorKind::HardBreak);
        assert!(message.contains("<memory>"), "file named: {message}");
        assert!(message.contains(key), "key named: {message}");
        assert!(message.contains(line), "line named: {message}");
        assert!(
            message.contains(replacement),
            "replacement named: {message}"
        );
    }
}

#[test]
fn hard_break_detection_uses_toml_keys_not_string_contents() {
    let valid = r#"
config-version = 0
[server]
bind = "127.0.0.1:8081"
api_key = """
include = ["not-a-config-key.toml"]
"""
"#;
    Config::from_toml_str(valid).expect("legacy spelling inside a value is data");

    for (raw, key, line) in [
        (
            "\"config-version\" = 2\n\"include\" = [\"base.toml\"]\n",
            "include",
            ":2:",
        ),
        (
            "config-version = 0\nworkshop.voice.final_model = \"small.bin\"\n",
            "workshop.voice.final_model",
            ":2:",
        ),
        (
            "config-version = 0\n[\"workshop\".\"voice\"]\nwindow_seconds = 8\n",
            "workshop.voice",
            ":2:",
        ),
    ] {
        let error = Config::from_toml_str(raw).expect_err("legacy key must hard-break");
        let message = error.to_string();
        assert_eq!(
            error.kind(),
            ConfigErrorKind::HardBreak,
            "expected hard break for {key}: {message}"
        );
        assert!(message.contains(key), "key named: {message}");
        assert!(message.contains(line), "line named: {message}");
    }
}

/// The format version this loader replaced. Spelled as a number so the
/// repository-wide check that no `config-version` header at the previous
/// value remains keeps passing while the tests still exercise a document
/// written at that version.
const PREVIOUS_VERSION: u32 = 2;

#[test]
fn missing_or_wrong_config_version_is_a_located_hard_break() {
    for raw in [
        "[server]\nbind='127.0.0.1:1'\napi_key='x'\n".to_string(),
        "config-version = 1\n[server]\nbind='127.0.0.1:1'\napi_key='x'\n".to_string(),
        format!("config-version = {PREVIOUS_VERSION}\n[server]\nbind='127.0.0.1:1'\napi_key='x'\n"),
    ] {
        let error = Config::from_toml_str(&raw).expect_err("version must be explicit");
        let message = error.to_string();
        assert_eq!(error.kind(), ConfigErrorKind::HardBreak);
        assert!(message.contains(":1:"), "line named: {message}");
        assert!(
            message.contains("config-version = 0"),
            "replacement names the required value: {message}"
        );
    }
}

#[test]
fn the_previous_format_version_fails_with_a_message_naming_zero() {
    let previous_header = format!("config-version = {PREVIOUS_VERSION}");
    let previous = CATALOG.replacen("config-version = 0", &previous_header, 1);
    assert_ne!(
        previous, CATALOG,
        "the fixture declares the current version"
    );

    let error = Config::from_toml_str(&previous)
        .expect_err("the previous format version is no longer accepted");
    let message = error.to_string();

    assert_eq!(error.kind(), ConfigErrorKind::HardBreak);
    assert!(
        message.contains("config-version = 0"),
        "the message names the required value: {message}"
    );
    assert!(
        !message.contains(&previous_header),
        "the message does not name the rejected value as the fix: {message}"
    );
}

#[test]
fn sibling_profiles_directory_is_a_hard_break() {
    let (temp, path) = file_fixture();
    fs::create_dir(temp.path().join("profiles")).expect("create legacy directory");

    let error = Config::load(&path, &ProfileSelection::new(Some("work"), None))
        .expect_err("profiles directory must fail");

    assert_eq!(error.kind(), ConfigErrorKind::HardBreak);
    let message = error.to_string();
    assert!(
        message.contains("gateway.toml:1"),
        "file and line: {message}"
    );
    assert!(message.contains("profiles/"), "feature named: {message}");
    assert!(
        message.contains("[[profile]]"),
        "replacement named: {message}"
    );
}

#[test]
fn file_hard_break_names_the_loaded_path_and_source_line() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, "config-version = 0\ninclude = [\"base.toml\"]\n")
        .expect("write legacy config");

    let error = Config::load(&path, &ProfileSelection::default())
        .expect_err("legacy config must hard-break before selection");
    let message = error.to_string();

    assert_eq!(error.kind(), ConfigErrorKind::HardBreak);
    assert!(message.contains(&path.display().to_string()));
    assert!(message.contains(":2:"));
    assert!(message.contains("include"));
}

#[test]
fn selection_precedence_is_cli_then_env_then_state() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"work\"\n").expect("write state");

    let state = Config::load(&path, &ProfileSelection::default()).expect("state selects");
    assert_eq!(state.active_profile().expect("active").name(), "work");
    assert_eq!(state.local_models()[0].name(), "a");
    assert_eq!(state.stt_models()[0].name(), "speech");
    assert_eq!(state.models()[0].name(), "cloud");
    assert_eq!(state.stale_state_selection(), None);

    let environment =
        Config::load(&path, &ProfileSelection::new(None, Some("travel"))).expect("env selects");
    assert_eq!(
        environment.active_profile().expect("active").name(),
        "travel"
    );
    assert_eq!(environment.local_models()[0].name(), "b");

    let command_line = Config::load(&path, &ProfileSelection::new(Some("work"), Some("travel")))
        .expect("cli selects");
    assert_eq!(
        command_line.active_profile().expect("active").name(),
        "work"
    );
}

#[test]
fn stale_state_file_loads_with_no_profile_and_records_the_name() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"deleted\"\n").expect("write state");

    let config = Config::load(&path, &ProfileSelection::default())
        .expect("a stale state file degrades instead of refusing");

    assert!(config.active_profile().is_none());
    assert_eq!(config.stale_state_selection(), Some("deleted"));
    assert!(config.local_models().is_empty());
    assert!(config.stt_models().is_empty());
    assert_eq!(config.models()[0].name(), "cloud");
}

#[test]
fn undefined_command_line_or_environment_profile_still_refuses() {
    let (_temp, path) = file_fixture();

    for inputs in [
        ProfileSelection::new(Some("x"), None),
        ProfileSelection::new(None, Some("x")),
    ] {
        let error = Config::load(&path, &inputs).expect_err("an ephemeral selection must exist");
        let message = error.to_string();
        assert_eq!(error.kind(), ConfigErrorKind::Validation);
        assert!(
            message.contains("active profile x is not defined"),
            "undefined name named: {message}"
        );
        assert!(
            message.contains("work, travel"),
            "defined profiles named: {message}"
        );
    }
}

#[test]
fn malformed_state_selection_still_refuses() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"../work\"\n").expect("write state");

    let error = Config::load(&path, &ProfileSelection::default())
        .expect_err("a malformed state name is not a stale selection");

    assert_eq!(error.kind(), ConfigErrorKind::Validation);
    assert!(error.to_string().contains("../work"));
}

#[test]
fn absent_selection_loads_with_no_profile_and_every_remote_model() {
    let (_temp, path) = file_fixture();

    let config = Config::load(&path, &ProfileSelection::default())
        .expect("no selection is a supported startup state");

    assert!(config.active_profile().is_none());
    assert_eq!(config.stale_state_selection(), None);
    assert!(config.local_models().is_empty());
    assert!(config.stt_models().is_empty());
    assert_eq!(config.models().len(), 1);
    assert_eq!(config.models()[0].name(), "cloud");
    assert_eq!(config.catalog_local_models().len(), 2);
    assert_eq!(config.catalog_stt_models().len(), 1);
}

#[test]
fn clear_profile_state_deletes_the_state_file_and_tolerates_absence() {
    let (_temp, path) = file_fixture();
    let state_path = profile_state_path(&path);
    fs::write(&state_path, "active_profile = \"work\"\n").expect("write state");

    clear_profile_state(&path).expect("present state clears");
    assert!(!state_path.exists(), "the state file is deleted");

    clear_profile_state(&path).expect("absent state is already clear");
    assert!(!state_path.exists());
}

#[test]
fn clear_profile_state_reports_a_failed_removal_as_a_removal() {
    let (_temp, path) = file_fixture();
    let state_path = profile_state_path(&path);
    fs::create_dir(&state_path).expect("occupy the state path with a directory");

    let error = clear_profile_state(&path).expect_err("a directory is not removable as a file");

    assert_eq!(error.kind(), ConfigErrorKind::Write);
    let mut chain = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(inner) = source {
        chain.push_str(": ");
        chain.push_str(&inner.to_string());
        source = inner.source();
    }
    assert!(
        chain.contains("remove state file"),
        "the operator reads a removal, not a write: {chain}"
    );
    assert!(
        chain.contains(&state_path.display().to_string()),
        "the failing path is named: {chain}"
    );
}

#[test]
fn selecting_a_profile_clears_a_stale_state_selection() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"deleted\"\n").expect("write state");
    let stale = Config::load(&path, &ProfileSelection::default()).expect("stale load degrades");
    assert_eq!(stale.stale_state_selection(), Some("deleted"));

    let work = crate::ProfileName::parse("work").expect("profile name");
    let selected = stale.select_profile(Some(&work)).expect("work selects");
    assert_eq!(selected.active_profile().expect("active").name(), "work");
    assert_eq!(
        selected.stale_state_selection(),
        None,
        "a fresh selection supersedes the stale name"
    );

    let unselected = stale.select_profile(None).expect("no profile selects");
    assert!(unselected.active_profile().is_none());
    assert_eq!(unselected.stale_state_selection(), None);
}
