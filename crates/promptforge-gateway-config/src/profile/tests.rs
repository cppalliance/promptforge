use super::*;
use tempfile::TempDir;

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}

/// Minimal valid gateway TOML that needs no `${VAR}` interpolation.
const MINIMAL_CONFIG: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

#[test]
fn include_merges_and_child_overrides_by_name() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://base"
api_key = ""
concurrency = 2

[[model]]
name = "m1"
description = "from base"
context = 1000
upstream = "u-base"
endpoints = ["anthropic"]
"#,
    );
    write(
        tmp.path(),
        "child.toml",
        r#"
include = ["base.toml"]

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://child"
api_key = ""
concurrency = 9

[[model]]
name = "m1"
description = "from child"
context = 2000
upstream = "u-child"
endpoints = ["anthropic"]

[[model]]
name = "m2"
description = "extra"
context = 3000
upstream = "u2"
endpoints = ["anthropic"]
"#,
    );

    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.endpoints().len(), 1);
    assert_eq!(config.endpoints()[0].base_url(), "http://child");
    assert_eq!(config.endpoints()[0].concurrency(), Some(9));
    assert_eq!(config.models().len(), 2);
    assert_eq!(config.models()[0].description(), "from child");
    assert_eq!(config.models()[0].context(), 2000);
    assert_eq!(config.models()[1].name(), "m2");
}

#[test]
fn include_paths_are_relative_to_including_file() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    write(
        &nested,
        "base.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#,
    );
    write(
        tmp.path(),
        "root.toml",
        r#"
include = ["nested/base.toml"]
"#,
    );
    let config = load_path(&tmp.path().join("root.toml")).unwrap();
    assert_eq!(config.models()[0].name(), "m");
}

#[test]
fn detects_include_cycles() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "a.toml", r#"include = ["b.toml"]"#);
    write(tmp.path(), "b.toml", r#"include = ["a.toml"]"#);
    let err = load_path(&tmp.path().join("a.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::IncludeCycle { .. }));
}

#[test]
fn rejects_runaway_include_depth() {
    let tmp = TempDir::new().unwrap();
    // Root at depth 0 plus MAX_INCLUDE_DEPTH+1 nested includes exceeds the cap.
    let last = MAX_INCLUDE_DEPTH + 1;
    for i in 0..=last {
        let body = if i == last {
            r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"
"#
            .to_owned()
        } else {
            format!("include = [\"n{}.toml\"]", i + 1)
        };
        write(tmp.path(), &format!("n{i}.toml"), &body);
    }
    let err = load_path(&tmp.path().join("n0.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::IncludeDepth { .. }));
}

#[test]
fn later_scalar_wins() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "base-token"

[queue]
max_depth = 10
"#,
    );
    write(
        tmp.path(),
        "child.toml",
        r#"
include = ["base.toml"]

[server]
api_key = "child-token"

[queue]
max_depth = 50
"#,
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.server().api_key().expose(), "child-token");
    assert_eq!(config.queue().max_depth(), 50);
    assert_eq!(config.server().bind().to_string(), "127.0.0.1:8081");
}

#[test]
fn list_profiles_returns_sorted_stems() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "threat.toml",
        "[server]\nbind=\"127.0.0.1:1\"\ntoken=\"t\"\n",
    );
    write(
        tmp.path(),
        "analytical.toml",
        "[server]\nbind=\"127.0.0.1:1\"\ntoken=\"t\"\n",
    );
    write(tmp.path(), "notes.txt", "ignore");
    // A directory whose name ends in `.toml` is not a profile.
    std::fs::create_dir(tmp.path().join("bogus.toml")).unwrap();
    let names = list_profiles(tmp.path()).unwrap();
    assert_eq!(names, vec!["analytical", "threat"]);
}

#[test]
fn load_named_reads_from_profiles_dir() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "alpha.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#,
    );
    let config = load_named(tmp.path(), &ProfileName::parse("alpha").unwrap()).unwrap();
    assert_eq!(config.models()[0].name(), "m");
}

#[test]
fn load_named_accepts_single_component_name_containing_dot_dot() {
    // PROFILE-010: `analysis..v2` is one normal path component (it merely
    // contains `..`), so it is a valid ProfileName and must load. The old
    // substring `contains("..")` check wrongly rejected it.
    let tmp = TempDir::new().unwrap();
    let name = ProfileName::parse("analysis..v2").expect("single component is valid");
    write(
        tmp.path(),
        "analysis..v2.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#,
    );
    let config = load_named(tmp.path(), &name).unwrap();
    assert_eq!(config.models()[0].name(), "m");
}

#[test]
fn local_model_override_by_name() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[local_model]]
name = "q"
description = "base"
source = "https://example.com/a.gguf"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
context = 1024
"#,
    );
    write(
        tmp.path(),
        "child.toml",
        r#"
include = ["base.toml"]

[[local_model]]
name = "q"
description = "child"
source = "https://example.com/b.gguf"
sha256 = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
context = 2048
"#,
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.local_models().len(), 1);
    assert_eq!(config.local_models()[0].description(), "child");
    assert_eq!(config.local_models()[0].context(), 2048);
}

#[test]
fn include_merges_devices_and_nested_lanes() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[device]]
id = "anthropic"
type = "remote"
concurrency = 10

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "http://a"
api_key = ""
device = "anthropic"

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["anthropic"]
"#,
    );
    write(
        tmp.path(),
        "child.toml",
        r#"
include = ["base.toml"]

[[device]]
id = "local-gpu"
type = "local"

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 1
"#,
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.devices().len(), 2);
    let local = config
        .devices()
        .iter()
        .find(|d| d.id() == "local-gpu")
        .unwrap();
    assert_eq!(local.lanes().len(), 1);
    assert_eq!(local.lanes()[0].id(), "generative");
}

#[test]
fn include_attaches_orphan_device_lanes_from_child() {
    // Leaf-only [[device.lane]] parses as a table; attach by lane.device.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "common.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[device]]
id = "local-gpu"
type = "local"
"#,
    );
    write(
        tmp.path(),
        "gemma.toml",
        r#"
include = ["common.toml"]

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 3

[[local_model]]
name = "gemma"
description = "prose"
source = "https://example.com/a.gguf"
sha256 = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
context = 1024
device = "local-gpu"
lane = "generative"
"#,
    );
    let config = load_path(&tmp.path().join("gemma.toml")).unwrap();
    assert_eq!(config.devices().len(), 1);
    assert_eq!(config.devices()[0].lanes().len(), 1);
    assert_eq!(config.devices()[0].lanes()[0].id(), "generative");
    assert_eq!(config.devices()[0].lanes()[0].concurrency(), 3);
    assert_eq!(
        config
            .local_model_concurrency(&config.local_models()[0])
            .unwrap(),
        3
    );
}

#[test]
fn non_array_inherited_collection_is_a_located_error() {
    // PROFILE-001: a keyed collection (`endpoint`) provided as a table, not an
    // array of tables, is rejected with a path:line diagnostic.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "broken.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[endpoint]
id = "not-an-array"
"#,
    );
    let err = load_path(&tmp.path().join("broken.toml")).unwrap_err();
    let msg = err.to_string();
    assert!(matches!(err, ConfigError::Validation(_)));
    assert!(msg.contains("broken.toml:"), "expected path:line in {msg}");
    assert!(msg.contains("endpoint"), "expected key in {msg}");
}

#[test]
fn orphan_device_lane_without_device_field_is_rejected() {
    // PROFILE-002: a leaf-only [[device.lane]] must name its parent device.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "common.toml",
        "[server]\nbind = \"127.0.0.1:8081\"\nkey = \"t\"\n\n[[device]]\nid = \"gpu\"\ntype = \"local\"\n",
    );
    write(
        tmp.path(),
        "leaf.toml",
        r#"
include = ["common.toml"]

[[device.lane]]
id = "generative"
concurrency = 1
"#,
    );
    let err = load_path(&tmp.path().join("leaf.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Validation(_)));
    assert!(err.to_string().contains("device"));
}

#[test]
fn merge_type_error_includes_path_and_line() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "broken.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[device]
id = "not-an-array"
"#,
    );
    let err = load_path(&tmp.path().join("broken.toml")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("broken.toml:"), "expected path:line in {msg}");
    assert!(msg.contains("expected"), "expected type hint in {msg}");
}

// ---- include-chain tests ----

#[test]
fn config_chain_ordering_is_root_first_depth_first() {
    let tmp = TempDir::new().unwrap();
    let nested = tmp.path().join("nested");
    fs::create_dir(&nested).unwrap();

    write(&nested, "grandchild.toml", MINIMAL_CONFIG);
    write(&nested, "child.toml", "include = [\"grandchild.toml\"]\n");
    write(
        tmp.path(),
        "root.toml",
        "include = [\"nested/child.toml\"]\n",
    );

    let (_value, chain) = collect_config_chain(&tmp.path().join("root.toml")).unwrap();
    assert_eq!(chain.len(), 3);
    assert!(
        chain[0].ends_with("root.toml"),
        "first should be root, got {:?}",
        chain[0]
    );
    assert!(
        chain[1].ends_with("child.toml"),
        "second should be child, got {:?}",
        chain[1]
    );
    assert!(
        chain[2].ends_with("grandchild.toml"),
        "third should be grandchild, got {:?}",
        chain[2]
    );
}

#[test]
fn load_succeeds_with_no_env_file() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "bare.toml", MINIMAL_CONFIG);
    let config = load_path(&tmp.path().join("bare.toml")).unwrap();
    assert_eq!(config.models()[0].name(), "m");
}

// ---- profile models allowlist ----

/// A two-model catalog that needs no `${VAR}` interpolation.
const TWO_MODEL_CATALOG: &str = r#"
[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://a"
api_key = ""

[[model]]
name = "m1"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]

[[model]]
name = "m2"
description = "prose"
context = 1
upstream = "u"
endpoints = ["e"]
"#;

#[test]
fn profile_allowlist_filters_the_included_catalog() {
    // The allowlist applies after include-merge: the profile selects a subset
    // of the catalog the include chain produced.
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "base.toml", TWO_MODEL_CATALOG);
    write(
        tmp.path(),
        "child.toml",
        "include = [\"base.toml\"]\nmodels = [\"m2\"]\n",
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.models().len(), 1);
    assert_eq!(config.models()[0].name(), "m2");
    assert_eq!(config.model_allowlist(), Some(&["m2".to_string()][..]));
}

#[test]
fn profile_allowlist_overrides_an_inherited_allowlist() {
    // `models` merges like a scalar: the later file's list replaces the
    // earlier one outright rather than unioning with it.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        &format!("models = [\"m1\"]\n{TWO_MODEL_CATALOG}"),
    );
    write(
        tmp.path(),
        "child.toml",
        "include = [\"base.toml\"]\nmodels = [\"m2\"]\n",
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    let names: Vec<&str> = config
        .models()
        .iter()
        .map(crate::config::ModelConfig::name)
        .collect();
    assert_eq!(names, ["m2"]);
}

#[test]
fn inherited_allowlist_applies_when_the_profile_declares_none() {
    // A profile without a `models` key inherits the list its include chain
    // established.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        &format!("models = [\"m1\"]\n{TWO_MODEL_CATALOG}"),
    );
    write(tmp.path(), "child.toml", "include = [\"base.toml\"]\n");
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    let names: Vec<&str> = config
        .models()
        .iter()
        .map(crate::config::ModelConfig::name)
        .collect();
    assert_eq!(names, ["m1"]);
}

// ---- chain-aware profile load and boot [server] extraction ----

#[test]
fn load_named_with_chain_returns_config_and_chain() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path(), "base.toml", MINIMAL_CONFIG);
    write(tmp.path(), "alpha.toml", "include = [\"base.toml\"]\n");

    let (config, chain) =
        load_named_with_chain(tmp.path(), &ProfileName::parse("alpha").unwrap()).unwrap();
    assert_eq!(config.models()[0].name(), "m");
    assert_eq!(chain.len(), 2);
    assert!(chain[0].ends_with("alpha.toml"), "profile first: {chain:?}");
    assert!(chain[1].ends_with("base.toml"), "include second: {chain:?}");
}

#[test]
fn load_server_reads_server_section_without_full_validation() {
    // The bare catalog may fail checks that apply to a loaded profile (here a
    // model naming an undefined endpoint); load_server still extracts [server].
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "gateway.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
api_key = "boot-key"

[[model]]
name = "m"
description = "prose"
context = 1
upstream = "u"
endpoints = ["ghost"]
"#,
    );

    let server = load_server(&tmp.path().join("gateway.toml")).unwrap();
    assert_eq!(server.bind().to_string(), "127.0.0.1:8081");
    assert_eq!(server.api_key().expose(), "boot-key");
    assert!(
        load_path(&tmp.path().join("gateway.toml")).is_err(),
        "full validation must reject the undefined endpoint"
    );
}

#[test]
fn load_server_resolves_the_boot_files_own_include_chain() {
    // The boot file's [server] may itself come from an include.
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        "[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"from-base\"\n",
    );
    write(tmp.path(), "gateway.toml", "include = [\"base.toml\"]\n");

    let server = load_server(&tmp.path().join("gateway.toml")).unwrap();
    assert_eq!(server.api_key().expose(), "from-base");
}

#[test]
fn load_server_interpolates_from_the_process_environment() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "gateway.toml",
        "[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"${PFG_DEFINITELY_UNSET_BOOT_KEY}\"\n",
    );

    let err = load_server(&tmp.path().join("gateway.toml")).unwrap_err();
    assert_eq!(err.kind(), crate::ConfigErrorKind::UnresolvedVar);
}

#[test]
fn load_server_requires_a_server_section() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "gateway.toml",
        "[local]\ncache_dir = \"/tmp/x\"\n",
    );

    let err = load_server(&tmp.path().join("gateway.toml")).unwrap_err();
    assert_eq!(err.kind(), crate::ConfigErrorKind::Validation);
    assert!(err.to_string().contains("[server]"), "got: {err}");
}
