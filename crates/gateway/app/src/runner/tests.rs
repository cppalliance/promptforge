//! Tests for startup profile selection, catalog loading, and the boot load.

use std::error::Error as _;

use super::*;

const CATALOG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha"
context = 1024
upstream = "alpha"
endpoints = ["fake"]

[[model]]
name = "beta-model"
description = "beta"
context = 1024
upstream = "beta"
endpoints = ["fake"]

[[profile]]
name = "alpha"
models = []

[[profile]]
name = "beta"
models = []
"#;

fn fixture(state: &str) -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    std::fs::write(&path, CATALOG).expect("write config");
    std::fs::write(
        gateway_config::profile_state_path(&path),
        format!("active_profile = \"{state}\"\n"),
    )
    .expect("write state");
    (temp, path)
}

fn error_text(error: &StartupError) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str("; ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[test]
fn command_line_profile_overrides_environment_and_state() {
    let (_temp, path) = fixture("alpha");
    let options = ServeOptions::new(
        Some(path.clone()),
        ProfileName::parse("beta").expect("name"),
    );

    let (config, context) =
        load_startup_with_environment(&path, &options, Some("alpha")).expect("startup loads");

    assert_eq!(
        config
            .active_profile()
            .map(gateway_config::ProfileConfig::name),
        Some("beta")
    );
    assert_eq!(
        context.active.as_ref().map(ProfileName::as_str),
        Some("beta")
    );
}

#[test]
fn environment_profile_overrides_state_without_a_cli_value() {
    let (_temp, path) = fixture("alpha");
    let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);

    let (config, _) =
        load_startup_with_environment(&path, &options, Some("beta")).expect("startup loads");

    assert_eq!(
        config
            .active_profile()
            .map(gateway_config::ProfileConfig::name),
        Some("beta")
    );
}

#[test]
fn startup_uses_the_sibling_state_without_overrides() {
    let (_temp, path) = fixture("alpha");
    let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);

    let (config, context) =
        load_startup_with_environment(&path, &options, None).expect("startup loads");

    assert_eq!(config.models()[0].name(), "alpha-model");
    assert_eq!(context.config_path, Some(path));
}

#[test]
fn unknown_override_lists_the_loaded_catalog_profiles() {
    let (_temp, path) = fixture("alpha");
    let options = ServeOptions::new(
        Some(path.clone()),
        ProfileName::parse("ghost").expect("name"),
    );

    let error =
        load_startup_with_environment(&path, &options, None).expect_err("unknown profile fails");
    let text = error_text(&error);

    assert!(text.contains("ghost"), "{text}");
    assert!(text.contains("alpha") && text.contains("beta"), "{text}");
}

#[test]
fn a_workshop_section_still_loads_and_earns_the_deprecation_warning() {
    // A config that includes `[workshop]` must parse, with the warning
    // discharging the no-silent-ignore rule for the section's inert
    // serving fields.
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    std::fs::write(
        &path,
        format!("{CATALOG}\n[workshop]\nbind = \"127.0.0.1:7910\"\nopen_browser = true\n"),
    )
    .expect("write config");
    std::fs::write(
        gateway_config::profile_state_path(&path),
        "active_profile = \"alpha\"\n",
    )
    .expect("write state");
    let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);

    let (config, _) = load_startup_with_environment(&path, &options, None)
        .expect("a config that includes [workshop] still loads");

    let warning = workshop_section_deprecation(&config)
        .expect("a [workshop] section earns the deprecation warning");
    assert!(
        warning.contains("[workshop]"),
        "the warning names the section: {warning}"
    );
    assert!(
        !warning.contains("[workshop.stt]"),
        "the warning must not advertise the legacy STT section: {warning}"
    );
}

#[test]
fn no_workshop_section_earns_no_deprecation_warning() {
    let (_temp, path) = fixture("alpha");
    let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);
    let (config, _) = load_startup_with_environment(&path, &options, None).expect("startup loads");
    assert!(workshop_section_deprecation(&config).is_none());
}

/// A selected profile produces no notice; a stale state file produces
/// the warning naming the missing and the defined profiles; no
/// selection at all produces the plain no-profile line.
#[test]
fn the_boot_selection_notice_names_stale_and_absent_selections() {
    let (_temp, path) = fixture("alpha");
    let options = ServeOptions::new(Some(path.clone()), None::<ProfileName>);
    let (selected, _) =
        load_startup_with_environment(&path, &options, None).expect("startup loads");
    assert_eq!(boot_selection_notice(&selected), None);

    std::fs::write(
        gateway_config::profile_state_path(&path),
        "active_profile = \"ghost\"\n",
    )
    .expect("write stale state");
    let (stale, context) =
        load_startup_with_environment(&path, &options, None).expect("a stale state degrades");
    assert_eq!(context.active, None, "no boot command is enqueued");
    assert_eq!(
        boot_selection_notice(&stale),
        Some(BootSelectionNotice::Stale(
            "state file selects profile \"ghost\", which is not defined (defined profiles: alpha, beta); booting with no profile"
                .to_owned()
        ))
    );

    std::fs::remove_file(gateway_config::profile_state_path(&path)).expect("remove state");
    let (none, _) = load_startup_with_environment(&path, &options, None).expect("no state loads");
    assert_eq!(
        boot_selection_notice(&none),
        Some(BootSelectionNotice::None(
            "no profile selected; serving remote models only".to_owned()
        ))
    );
}

/// With no profile the runner enqueues no boot command: the local
/// runtime stays empty, every remote model routes from assembly, and
/// the status reports `profile: null`.
#[tokio::test]
async fn no_profile_enqueues_no_boot_load_and_serves_the_remote_table() {
    use tower::ServiceExt as _;

    let config = Config::from_toml_str(CATALOG)
        .expect("catalog parses")
        .select_profile(None)
        .expect("no selection");
    let gateway = Gateway::new(&config, ProfilesContext::default()).expect("assembles");

    assert!(!gateway.enqueue_boot_load(None), "nothing to load");
    assert!(gateway.state.commands.pending_commands().is_empty());
    {
        let live = gateway.state.live.read().await;
        #[cfg(feature = "local")]
        assert!(live.local.models().is_empty(), "the local runtime is empty");
        assert!(live.routing.model("alpha-model").is_ok());
        assert!(live.routing.model("beta-model").is_ok());
    }
    let response = gateway
        .router()
        .oneshot(
            axum::http::Request::builder()
                .uri("/admin/status")
                .header("authorization", "Bearer test-token")
                .body(axum::body::Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router answers");
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let status: serde_json::Value = serde_json::from_slice(&body).expect("status is JSON");
    assert!(status["profile"].is_null(), "no profile runs: {status}");
    assert_eq!(
        status["models"],
        serde_json::json!(["alpha-model", "beta-model"])
    );
}

/// With a selected profile the runner enqueues exactly one boot load,
/// labelled for the status readout.
#[test]
fn a_selected_profile_enqueues_one_boot_load() {
    let config = Config::from_toml_str(CATALOG)
        .expect("catalog parses")
        .select_profile(Some(&ProfileName::parse("alpha").expect("name")))
        .expect("alpha selects");
    let gateway = Gateway::new(&config, ProfilesContext::default()).expect("assembles");

    assert!(gateway.enqueue_boot_load(Some(ProfileName::parse("alpha").expect("name"))));
    let pending: Vec<String> = gateway
        .state
        .commands
        .pending_commands()
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    assert_eq!(pending, ["load-profile: alpha"]);
}
