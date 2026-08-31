use super::*;

const CONFIG: &str = r#"
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "secret"

[[local_model]]
name = "local"
description = "local model"
source = "/models/local.gguf"
context = 4096

[[profile]]
name = "work"
models = ["local"]

[[profile]]
name = "travel"
models = []
"#;

fn write_config() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, CONFIG).expect("write config");
    (temp, path)
}

#[test]
fn pending_document_splits_active_profile_into_state_shadow() {
    let (_temp, path) = write_config();
    let mut document: Value = toml::from_str(CONFIG).expect("fixture parses");
    document.as_table_mut().expect("table").insert(
        "active_profile".to_owned(),
        Value::String("work".to_owned()),
    );

    let shadows = save_config_shadow(&path, document).expect("pending save");

    assert_eq!(shadows.config, shadow_path(&path));
    let state = shadows.state.expect("state shadow");
    assert_eq!(state, shadow_path(&crate::profile_state_path(&path)));
    assert_eq!(
        fs::read_to_string(state).expect("read state"),
        "active_profile = \"work\"\n"
    );
    assert!(
        !fs::read_to_string(shadows.config)
            .expect("read config")
            .contains("active_profile")
    );
}

#[test]
fn pending_loader_prefers_pending_profile_state() {
    let (_temp, path) = write_config();
    fs::write(
        crate::profile_state_path(&path),
        "active_profile = \"work\"\n",
    )
    .expect("write state");
    write_shadow(
        &crate::profile_state_path(&path),
        "active_profile = \"travel\"\n",
    )
    .expect("write state shadow");

    let config = load_pending_config(&path, &ProfileSelection::default()).expect("pending loads");

    assert_eq!(config.active_profile().expect("selected").name(), "travel");
    assert!(config.local_models().is_empty());
}

#[test]
fn pending_save_restores_redacted_secrets_and_rejects_invalid_profiles() {
    let (_temp, path) = write_config();
    let mut document: Value = toml::from_str(CONFIG).expect("config parses");
    document["server"]["api_key"] = Value::String("***".to_owned());
    document.as_table_mut().expect("table").insert(
        "active_profile".to_owned(),
        Value::String("missing".to_owned()),
    );

    let error = save_config_shadow(&path, document).expect_err("unknown profile fails");

    assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
    assert!(!shadow_path(&path).exists());
}

#[test]
fn pending_save_validates_retained_profile_state() {
    let (_temp, path) = write_config();
    fs::write(
        crate::profile_state_path(&path),
        "active_profile = \"work\"\n",
    )
    .expect("write state");
    let candidate = CONFIG.replace("[[profile]]\nname = \"work\"\nmodels = [\"local\"]\n\n", "");
    let document: Value = toml::from_str(&candidate).expect("candidate parses");

    let error = save_config_shadow(&path, document).expect_err("stale state must fail");

    assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
    assert!(error.to_string().contains("work"));
    assert!(!shadow_path(&path).exists());
}

#[test]
fn failed_state_write_rolls_back_config_shadow() {
    let (_temp, path) = write_config();
    let previous = CONFIG.replace(
        "description = \"local model\"",
        "description = \"previous\"",
    );
    write_shadow(&path, &previous).expect("write previous config shadow");
    let mut document: Value = toml::from_str(CONFIG).expect("config parses");
    document.as_table_mut().expect("table").insert(
        "active_profile".to_owned(),
        Value::String("work".to_owned()),
    );
    let state_shadow = shadow_path(&crate::profile_state_path(&path));
    fs::create_dir(&state_shadow).expect("block state shadow with directory");

    let error = save_config_shadow(&path, document).expect_err("state write must fail");

    assert_eq!(error.kind(), crate::ConfigErrorKind::Write);
    assert_eq!(
        fs::read_to_string(shadow_path(&path)).expect("previous shadow remains"),
        previous
    );
    assert!(state_shadow.is_dir());
}

#[test]
fn pending_report_includes_config_and_active_profile_changes() {
    let (_temp, path) = write_config();
    let state = crate::profile_state_path(&path);
    fs::write(&state, "active_profile = \"work\"\n").expect("write state");
    write_shadow(
        &path,
        &CONFIG.replace("description = \"local model\"", "description = \"edited\""),
    )
    .expect("write config shadow");
    write_shadow(&state, "active_profile = \"travel\"\n").expect("write state shadow");

    let report = pending_report(&path).expect("report");

    assert_eq!(report.shadowed_files, [path.clone(), state]);
    assert_eq!(report.changed_sections, ["active_profile", "local_model"]);
}

#[test]
fn immediate_persistence_preserves_an_unapplied_state_shadow() {
    let (_temp, path) = write_config();
    let state = crate::profile_state_path(&path);
    fs::write(&state, "active_profile = \"work\"\n").expect("write state");
    write_shadow(&state, "active_profile = \"travel\"\n").expect("write pending state");
    let selected = ProfileName::parse("work").expect("profile name");

    persist_profile_state(&path, &selected).expect("persist immediate selection");

    assert_eq!(
        fs::read_to_string(&state).expect("read real state"),
        "active_profile = \"work\"\n"
    );
    assert_eq!(
        fs::read_to_string(shadow_path(&state)).expect("read pending state"),
        "active_profile = \"travel\"\n"
    );
}
