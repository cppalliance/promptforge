use gateway_config::shadow_path;

use super::*;

/// A config path in a fresh directory; nothing is written.
fn config_path() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    (temp, path)
}

#[test]
fn when_reading_the_pending_env_the_shadow_wins_over_the_real_env_file() {
    let (_temp, config_path) = config_path();
    let env_path = config_path.with_extension("env");

    let var_name = "PF_TEST_KEY";
    let value = "sk-A";
    let shadow_value = "sk-B";

    std::fs::write(&env_path, format!("{var_name}={value}\n")).expect("error writing env file");
    std::fs::write(
        shadow_path(&env_path),
        format!("{var_name}={shadow_value}\n"),
    )
    .expect("error writing env shadow");

    let pending = PendingEnv::new(&config_path).expect("pending env reads");

    let actual = pending
        .resolve_var(var_name)
        .expect("Could not resolve test var");
    let expected = shadow_value;

    assert_eq!(actual, expected, "Got `{actual}` but expected `{expected}`");
}

#[test]
fn no_shadow_env_file_is_an_empty_pending_env() {
    let (_temp, config_path) = config_path();

    let pending = PendingEnv::new(&config_path).expect("Error building pending env");

    assert!(pending.data.is_empty());
    assert_eq!(
        pending.resolve_var("PF_TEST_UNSET_EVERYWHERE"),
        Err(VarError::NotPresent)
    );
}

#[test]
fn when_reading_the_pending_env_the_shadow_file_wins_over_the_process_environment() {
    // PATH is set in every test process, so it stands in for a variable
    // both the file and the process define.

    let (_temp, config_path) = config_path();
    std::fs::write(config_path.with_extension("env.next"), "PATH=from-file\n")
        .expect("Error writing env file");

    let pending = PendingEnv::new(&config_path).expect("Error building pending env");

    assert_eq!(pending.resolve_var("PATH"), Ok("from-file".to_owned()));
}

#[test]
fn a_variable_only_in_the_process_environment_resolves_from_it() {
    let (_temp, config_path) = config_path();

    let pending = PendingEnv::new(&config_path).expect("Error building pending env");

    assert_eq!(pending.resolve_var("PATH"), std::env::var("PATH"));
}

#[test]
fn a_malformed_env_file_is_an_error() {
    let (_temp, config_path) = config_path();
    std::fs::write(
        config_path.with_extension("env.next"),
        r#"="I'm only a value with no name!"#,
    )
    .expect("error writing env file");

    assert!(PendingEnv::new(&config_path).is_err());
}
