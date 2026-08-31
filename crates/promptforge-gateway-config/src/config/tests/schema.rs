use std::fs;

use tempfile::TempDir;

use super::super::*;
use crate::{ConfigErrorKind, ProfileSelection, profile_state_path};

const CATALOG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "t"

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
fn version_two_schema_carries_profiles_and_stt_models() {
    let config = Config::from_toml_str(CATALOG).expect("schema parses");

    assert_eq!(config.config_version(), 2);
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
        "/../../gateway.local.example.toml"
    ));
    let catalog = Config::from_toml_str(example).expect("canonical example validates");
    let selected = catalog
        .select_profile(&crate::ProfileName::parse("work").expect("profile name"))
        .expect("work profile selects");

    assert_eq!(selected.local_models()[0].name(), "qwen-local");
    assert_eq!(selected.stt_models().len(), 2);
}

#[test]
fn hard_breaks_name_file_key_line_and_replacement() {
    for (raw, key, line, replacement) in [
        (
            "config-version = 2\ninclude = [\"base.toml\"]\n",
            "include",
            ":2:",
            "[[profile]]",
        ),
        (
            "config-version = 2\nmodels = [\"a\"]\n",
            "models",
            ":2:",
            "[[profile]]",
        ),
        (
            "config-version = 2\n[server]\nbind='127.0.0.1:1'\napi_key='x'\n\
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
config-version = 2
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
            "config-version = 2\nworkshop.voice.final_model = \"small.bin\"\n",
            "workshop.voice.final_model",
            ":2:",
        ),
        (
            "config-version = 2\n[\"workshop\".\"voice\"]\nwindow_seconds = 8\n",
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

#[test]
fn missing_or_wrong_config_version_is_a_located_hard_break() {
    for raw in [
        "[server]\nbind='127.0.0.1:1'\napi_key='x'\n",
        "config-version = 1\n[server]\nbind='127.0.0.1:1'\napi_key='x'\n",
    ] {
        let error = Config::from_toml_str(raw).expect_err("version must be explicit");
        assert_eq!(error.kind(), ConfigErrorKind::HardBreak);
        assert!(error.to_string().contains("config-version"));
        assert!(error.to_string().contains(":1:"));
    }
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
    fs::write(&path, "config-version = 2\ninclude = [\"base.toml\"]\n")
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
fn stale_state_names_value_and_defined_profiles() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"deleted\"\n").expect("write state");

    let error =
        Config::load(&path, &ProfileSelection::default()).expect_err("stale state must fail");
    let message = error.to_string();

    assert_eq!(error.kind(), ConfigErrorKind::Validation);
    assert!(message.contains("deleted"), "stale value named: {message}");
    assert!(
        message.contains("work, travel"),
        "defined profiles named: {message}"
    );
}

#[test]
fn absent_selection_refuses_with_defined_profile_list() {
    let (_temp, path) = file_fixture();

    let error =
        Config::load(&path, &ProfileSelection::default()).expect_err("selection is required");
    let message = error.to_string();

    assert_eq!(error.kind(), ConfigErrorKind::Validation);
    assert!(message.contains("--profile"));
    assert!(message.contains("PROMPTFORGE_PROFILE"));
    assert!(message.contains("work, travel"));
}
