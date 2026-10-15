//! Apply keeps rollback copies: the first Apply since the last start copies
//! each real file it replaces to `<file>.prev`, and later Applies keep that
//! copy; an env-only change that breaks a `${VAR}` is refused.

use gateway_config::write_shadow;

use super::{CONFIG, GAMMA_MODEL, fixture, post, serve_fixture, stage_gamma};
use crate::commands::apply::capture_apply;
use crate::config_shadow::rollback_path;

#[tokio::test]
async fn apply_keeps_the_booted_config_as_the_rollback_copy() {
    let (_temp, config, paths) = fixture();
    let config_path = paths.config_path.clone();
    stage_gamma(&config_path);
    let (addr, _state) = serve_fixture(config, paths).await;

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(rollback_path(&config_path))
            .expect("should be able to read the rollback copy"),
        CONFIG,
        "the rollback config should have booted"
    );
}

#[tokio::test]
async fn a_second_apply_keeps_the_first_rollback_copy() {
    let (_temp, config, paths) = fixture();

    let config_path = paths.config_path.clone();
    stage_gamma(&config_path);

    let (addr, _state) = serve_fixture(config, paths).await;
    post(addr, "admin/config-apply").await;

    let edited = format!("{CONFIG}{GAMMA_MODEL}")
        .replace("description = \"gamma\"", "description = \"gamma, edited\"");

    write_shadow(&config_path, &edited).expect("the second config change should succeed");

    let response = post(addr, "admin/config-apply").await;

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        std::fs::read_to_string(rollback_path(&config_path))
            .expect("the rollback copy should be successfully read"),
        CONFIG,
        "a second call to Apply should not change the contents of the rollback file"
    );
}

#[test]
fn an_env_change_that_breaks_a_reference_is_refused() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let config_path = temp.path().join("gateway.toml");

    std::fs::write(
        &config_path,
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\n\
         api_key = \"${PF_TEST_ROLLBACK_KEY}\"\n",
    )
    .expect("write config");

    let env_path = config_path.with_extension("env");

    std::fs::write(&env_path, "PF_TEST_ROLLBACK_KEY=booted\n")
        .expect("writing env file should succeed");

    write_shadow(&env_path, "OTHER=value\n").expect("writing to shadow should succeed");

    let result = capture_apply(&config_path);

    assert!(
        result.is_err(),
        "an env-only change must resolve every reference the config makes"
    );
}
