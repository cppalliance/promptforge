//! Tests for shadow config files, pending saves, and atomic profile state writes.

use super::*;

const CONFIG: &str = r#"
config-version = 0

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

const ACTIVE_PROFILE_REFUSED: &str =
    "active_profile is not a configuration key; select a profile with POST /admin/switch-profile";

fn write_config() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, CONFIG).expect("write config");
    (temp, path)
}

fn document(toml: &str) -> Value {
    toml::from_str(toml).expect("document parses")
}

fn state_shadow(path: &Path) -> PathBuf {
    shadow_path(&crate::profile_state_path(path))
}

#[test]
fn write_atomic_replaces_the_real_file_and_leaves_its_shadow_alone() {
    let (_temp, path) = write_config();
    write_shadow(&path, "config-version = 0\n").expect("stage shadow");

    write_atomic(&path, "config-version = 3\n").expect("atomic write");

    assert_eq!(
        fs::read_to_string(&path).expect("read real"),
        "config-version = 3\n",
        "the real file carries the written contents"
    );
    assert_eq!(
        fs::read_to_string(shadow_path(&path)).expect("read shadow"),
        "config-version = 0\n",
        "the shadow is not consumed by a direct write"
    );
    let leftovers: Vec<_> = fs::read_dir(path.parent().expect("parent"))
        .expect("list dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-") || name.contains(".backup-"))
        .collect();
    assert!(leftovers.is_empty(), "no sidecar survives: {leftovers:?}");
}

#[test]
fn active_profile_in_the_pending_document_is_refused_and_writes_no_shadow() {
    let (_temp, path) = write_config();
    let mut document = document(CONFIG);
    document.as_table_mut().expect("table").insert(
        "active_profile".to_owned(),
        Value::String("work".to_owned()),
    );

    let error = save_config_shadow(&path, document).expect_err("active_profile is refused");

    assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
    assert!(
        error.to_string().contains(ACTIVE_PROFILE_REFUSED),
        "the error names the switch route: {error}"
    );
    assert!(!shadow_path(&path).exists(), "no config shadow is written");
    assert!(!state_shadow(&path).exists(), "no state shadow is written");
}

#[test]
fn pending_save_writes_only_the_config_shadow() {
    let (_temp, path) = write_config();

    let shadows = save_config_shadow(&path, document(CONFIG)).expect("pending save");

    assert_eq!(
        shadows,
        PendingShadows {
            config: shadow_path(&path)
        }
    );
    assert!(shadows.config.is_file());
    assert!(!state_shadow(&path).exists());
}

#[test]
fn pending_save_restores_redacted_secrets() {
    let (_temp, path) = write_config();
    let mut document = document(CONFIG);
    document["server"]["api_key"] = Value::String("***".to_owned());

    let shadows = save_config_shadow(&path, document).expect("pending save");

    assert!(
        fs::read_to_string(shadows.config)
            .expect("read shadow")
            .contains("api_key = \"secret\""),
        "the redacted secret is restored from the real file"
    );
}

#[test]
fn pending_save_rejects_invalid_profiles() {
    let (_temp, path) = write_config();
    let mut document = document(CONFIG);
    document["profile"][1]["models"] = Value::Array(vec![Value::String("ghost".to_owned())]);

    let error = save_config_shadow(&path, document).expect_err("unknown member fails");

    assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
    assert!(!shadow_path(&path).exists());
}

#[test]
fn pending_save_accepts_a_document_that_drops_the_persisted_profile() {
    let (_temp, path) = write_config();
    fs::write(
        crate::profile_state_path(&path),
        "active_profile = \"work\"\n",
    )
    .expect("write state");
    let candidate = CONFIG.replace("[[profile]]\nname = \"work\"\nmodels = [\"local\"]\n\n", "");

    let shadows = save_config_shadow(&path, document(&candidate)).expect("stale state degrades");

    assert!(shadows.config.is_file());
}

#[test]
fn pending_loader_honors_a_supplied_selection() {
    let (_temp, path) = write_config();

    let config = load_pending_config(&path, &ProfileSelection::new(Some("travel"), None))
        .expect("pending loads");

    assert_eq!(config.active_profile().expect("selected").name(), "travel");
    assert!(config.local_models().is_empty());
}

#[test]
fn pending_loader_accepts_no_selection() {
    let (_temp, path) = write_config();

    let config = load_pending_config(&path, &ProfileSelection::default()).expect("pending loads");

    assert!(config.active_profile().is_none());
    assert!(config.local_models().is_empty());
    assert!(config.stale_state_selection().is_none());
}

#[test]
fn pending_loader_reads_the_state_file_and_ignores_a_state_shadow() {
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
    .expect("write leftover state shadow");

    let config = load_pending_config(&path, &ProfileSelection::default()).expect("pending loads");

    assert_eq!(config.active_profile().expect("selected").name(), "work");
    assert_eq!(config.local_models().len(), 1);
}

#[test]
fn pending_loader_degrades_a_stale_state_file_like_load() {
    let (_temp, path) = write_config();
    fs::write(
        crate::profile_state_path(&path),
        "active_profile = \"missing\"\n",
    )
    .expect("write state");

    let config = load_pending_config(&path, &ProfileSelection::default()).expect("pending loads");

    assert!(config.active_profile().is_none());
    assert_eq!(config.stale_state_selection(), Some("missing"));
}

#[test]
fn pending_loader_rejects_an_undefined_ephemeral_selection() {
    let (_temp, path) = write_config();

    let error = load_pending_config(&path, &ProfileSelection::new(Some("missing"), None))
        .expect_err("a typed override must name a defined profile");

    assert_eq!(error.kind(), crate::ConfigErrorKind::Validation);
    assert!(error.to_string().contains("missing"));
}

#[test]
fn pending_report_lists_only_the_config_shadow() {
    let (_temp, path) = write_config();
    let state = crate::profile_state_path(&path);
    fs::write(&state, "active_profile = \"work\"\n").expect("write state");
    write_shadow(
        &path,
        &CONFIG.replace("description = \"local model\"", "description = \"edited\""),
    )
    .expect("write config shadow");
    write_shadow(&state, "active_profile = \"travel\"\n").expect("write leftover state shadow");

    let report = pending_report(&path).expect("report");

    assert_eq!(report.shadowed_files, std::slice::from_ref(&path));
    assert_eq!(report.changed_sections, ["local_model"]);
}

#[test]
fn persist_profile_state_replaces_the_real_state_file() {
    let (_temp, path) = write_config();
    let state = crate::profile_state_path(&path);
    fs::write(&state, "active_profile = \"travel\"\n").expect("write state");
    let selected = ProfileName::parse("work").expect("profile name");

    persist_profile_state(&path, &selected).expect("persist selection");

    assert_eq!(
        fs::read_to_string(&state).expect("read real state"),
        "active_profile = \"work\"\n"
    );
}
