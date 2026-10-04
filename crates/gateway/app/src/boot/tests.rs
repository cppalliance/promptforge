//! Tests for config discovery, first-run config generation, and the installer's STT choice.

use super::*;

/// Hex characters [`generate_api_key`] produces (two u64s, 128 bits).
const API_KEY_LENGTH: usize = 32;

/// Fixed locations rooted at a tempdir, for [`resolve_in`].
fn locations(temp: &tempfile::TempDir) -> Locations {
    Locations {
        exe_dir: temp.path().join("exe"),
        cwd: temp.path().join("cwd"),
        home: temp.path().join("home"),
    }
}

#[test]
fn candidates_are_ordered_exe_then_cwd_then_profile() {
    let candidates = candidates_from(
        Path::new("exe-dir"),
        Path::new("cwd-dir"),
        Path::new("home-dir"),
    );
    assert_eq!(
        candidates,
        vec![
            PathBuf::from("exe-dir/gateway.toml"),
            PathBuf::from("cwd-dir/gateway.toml"),
            PathBuf::from("home-dir/.promptforge/gateway.toml"),
        ]
    );
}

#[test]
fn the_first_existing_candidate_wins() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dirs = locations(&temp);
    for dir in [&dirs.exe_dir, &dirs.cwd] {
        std::fs::create_dir_all(dir).expect("create fixture dir");
    }
    let promptforge = dirs.home.join(".promptforge");
    std::fs::create_dir_all(&promptforge).expect("create profile dir");
    let in_cwd = dirs.cwd.join(CONFIG_FILE_NAME);
    let in_home = promptforge.join(CONFIG_FILE_NAME);
    std::fs::write(&in_cwd, "").expect("write fixture");
    std::fs::write(&in_home, "").expect("write fixture");

    let candidates = candidates_from(&dirs.exe_dir, &dirs.cwd, &dirs.home);
    assert_eq!(
        first_existing(&candidates).as_deref(),
        Some(in_cwd.as_path()),
        "the current directory beats the profile"
    );

    let in_exe = dirs.exe_dir.join(CONFIG_FILE_NAME);
    std::fs::write(&in_exe, "").expect("write fixture");
    assert_eq!(
        first_existing(&candidates).as_deref(),
        Some(in_exe.as_path()),
        "beside the executable beats everything"
    );
}

#[test]
fn no_config_returns_none() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dirs = locations(&temp);
    let candidates = candidates_from(&dirs.exe_dir, &dirs.cwd, &dirs.home);
    assert_eq!(first_existing(&candidates), None);
}

#[test]
fn an_explicit_path_wins_without_touching_the_disk() {
    let resolved = resolve_in(
        Some(PathBuf::from("explicit/gateway.toml")),
        || panic!("an explicit path skips the location lookup"),
        InstallerStt::Included,
    )
    .expect("the explicit path resolves");
    assert_eq!(resolved, PathBuf::from("explicit/gateway.toml"));
}

#[test]
fn discovery_finds_an_existing_config_without_generating() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dirs = locations(&temp);
    std::fs::create_dir_all(&dirs.cwd).expect("create cwd");
    let in_cwd = dirs.cwd.join(CONFIG_FILE_NAME);
    std::fs::write(&in_cwd, "").expect("write fixture");

    let resolved = resolve_in(None, || Ok(locations(&temp)), InstallerStt::Included)
        .expect("discovery resolves");

    assert_eq!(resolved, in_cwd);
    assert!(
        !dirs.home.join(".promptforge").exists(),
        "a discovered config means no first-run generation"
    );
}

#[test]
fn the_report_discovery_returns_an_explicit_path_without_a_lookup() {
    let discovered = discover_in(Some(PathBuf::from("explicit/gateway.toml")), || {
        panic!("an explicit path skips the location lookup")
    });
    assert_eq!(discovered, Some(PathBuf::from("explicit/gateway.toml")));
}

#[test]
fn the_report_discovery_names_an_existing_candidate() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dirs = locations(&temp);
    std::fs::create_dir_all(&dirs.cwd).expect("create cwd");
    let in_cwd = dirs.cwd.join(CONFIG_FILE_NAME);
    std::fs::write(&in_cwd, "").expect("write fixture");

    let discovered = discover_in(None, || Ok(locations(&temp)));

    assert_eq!(discovered, Some(in_cwd));
}

#[test]
fn the_report_discovery_falls_back_to_the_profile_without_generating() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dirs = locations(&temp);

    let discovered = discover_in(None, || Ok(locations(&temp)));

    assert_eq!(discovered, Some(profile_config_path(&dirs.home)));
    assert!(
        !dirs.home.join(".promptforge").exists(),
        "the report names the profile location but never writes it"
    );
}

#[test]
fn the_report_discovery_reads_an_unlocatable_process_as_none() {
    let discovered = discover_in(None, || Err(BootError::NoHome));
    assert_eq!(discovered, None);
}

#[test]
fn first_run_generates_a_bootable_config_into_the_profile() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dirs = locations(&temp);

    let resolved = resolve_in(None, || Ok(locations(&temp)), InstallerStt::Included)
        .expect("first run generates");

    assert_eq!(resolved, profile_config_path(&dirs.home));
    // The boot path itself: the generated file loads with no CLI or
    // environment profile, because generation wrote the sibling state
    // file selecting `default`.
    let config = gateway_config::Config::load(
        &resolved,
        &gateway_config::ProfileSelection::new(None, None),
    )
    .expect("the generated config boots with no profile flags");
    assert_eq!(
        config.config_version(),
        0,
        "the first-run default declares the pre-release format version"
    );
    assert_eq!(
        config
            .active_profile()
            .map(gateway_config::ProfileConfig::name),
        Some(DEFAULT_PROFILE),
        "the state file selects the generated profile on every boot"
    );
}

#[test]
fn the_generated_config_binds_loopback_on_an_os_assigned_port() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Included)
        .expect("generates");
    let config = gateway_config::Config::from_toml_str(
        &std::fs::read_to_string(&path).expect("generated config reads"),
    )
    .expect("the generated config parses");

    assert_eq!(
        config.server().bind().to_string(),
        "127.0.0.1:0",
        "the sidecar bind is loopback with an OS-assigned port"
    );
    assert!(
        config.workshop().is_none(),
        "the generated config omits the [workshop] section"
    );
    let key = config.server().api_key().expose();
    assert_eq!(key.len(), API_KEY_LENGTH);
    assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn the_generated_config_includes_the_recommended_stt_pair_by_default() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Included)
        .expect("generates");
    let raw = std::fs::read_to_string(&path).expect("read back");
    let config = gateway_config::Config::from_toml_str(&raw).expect("the config parses");

    let stt = config.catalog_stt_models();
    assert_eq!(stt.len(), 2);
    assert_eq!(stt[0].name(), "whisper-base-en");
    assert_eq!(stt[1].name(), "whisper-small-en");
    assert!(raw.contains("sha256 = "), "the pair stays digest-pinned");
    assert!(
        raw.contains("models = [\"whisper-base-en\", \"whisper-small-en\"]"),
        "the default profile selects the pair"
    );
}

#[test]
fn the_generated_config_writes_the_diagnostics_hint_as_a_comment() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Included)
        .expect("generates");
    let raw = std::fs::read_to_string(&path).expect("read back");

    assert!(
        raw.contains("# Diagnostics: promptforge-gateway diagnostics\n"),
        "the hint is a comment, never a config field: {raw}"
    );
    gateway_config::Config::from_toml_str(&raw)
        .expect("a commented hint leaves the config parseable");
}

#[test]
fn the_generated_config_omits_stt_when_the_installer_declined_it() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Omitted)
        .expect("generates");
    let raw = std::fs::read_to_string(&path).expect("read back");
    let config = gateway_config::Config::from_toml_str(&raw).expect("the config parses");

    assert!(
        config.catalog_stt_models().is_empty(),
        "no [[stt_model]] entries when the installer declined STT"
    );
    assert!(
        !raw.contains("stt_model"),
        "the file omits STT text entirely: {raw}"
    );
    assert!(raw.contains("models = []"), "the profile selects nothing");
    assert_eq!(config.server().bind().to_string(), "127.0.0.1:0");
    assert_eq!(config.server().api_key().expose().len(), API_KEY_LENGTH);
}

#[test]
fn generation_never_overwrites_an_existing_config() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = temp.path().join(CONFIG_FILE_NAME);
    std::fs::write(&path, "sentinel").expect("write fixture");

    let written = generate_default(&path, InstallerStt::Included).expect("generation succeeds");

    assert_eq!(written, path);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "sentinel",
        "an existing config is never overwritten, key included"
    );
}

#[test]
fn the_generated_config_without_stt_assembles_a_gateway() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = generate_default(&temp.path().join(CONFIG_FILE_NAME), InstallerStt::Omitted)
        .expect("generates");
    let config =
        gateway_config::Config::load(&path, &gateway_config::ProfileSelection::new(None, None))
            .expect("the generated config loads");
    crate::Gateway::from_config(&config, crate::ProfilesContext::default())
        .expect("the generated config assembles a gateway");
}

#[test]
fn generated_api_keys_are_random_hex() {
    let first = generate_api_key();
    let second = generate_api_key();
    assert_eq!(first.len(), API_KEY_LENGTH);
    assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(first, second, "two first runs must not share a bearer key");
}

#[test]
fn the_installer_dword_maps_only_zero_to_omitted() {
    assert_eq!(InstallerStt::from_dword(None), InstallerStt::Included);
    assert_eq!(InstallerStt::from_dword(Some(0)), InstallerStt::Omitted);
    assert_eq!(InstallerStt::from_dword(Some(1)), InstallerStt::Included);
}
