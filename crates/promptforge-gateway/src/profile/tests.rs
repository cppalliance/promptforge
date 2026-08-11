use super::*;
use tempfile::TempDir;

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}

#[test]
fn include_merges_and_child_overrides_by_name() {
    let tmp = TempDir::new().unwrap();
    write(
        tmp.path(),
        "base.toml",
        r#"
[server]
bind = "127.0.0.1:8081"
key = "t"

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
    assert_eq!(config.endpoints.len(), 1);
    assert_eq!(config.endpoints[0].base_url, "http://child");
    assert_eq!(config.endpoints[0].concurrency, Some(9));
    assert_eq!(config.models.len(), 2);
    assert_eq!(config.models[0].description, "from child");
    assert_eq!(config.models[0].context, 2000);
    assert_eq!(config.models[1].name, "m2");
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
key = "t"

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
    assert_eq!(config.models[0].name, "m");
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
key = "t"
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
key = "base-token"

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
key = "child-token"

[queue]
max_depth = 50
"#,
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.server.key.expose(), "child-token");
    assert_eq!(config.queue.max_depth, 50);
    assert_eq!(config.server.bind.to_string(), "127.0.0.1:8081");
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
key = "t"

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
    let config = load_named(tmp.path(), "alpha").unwrap();
    assert_eq!(config.models[0].name, "m");
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
key = "t"

[[local_model]]
name = "q"
description = "base"
source = "https://example.com/a.gguf"
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
context = 2048
"#,
    );
    let config = load_path(&tmp.path().join("child.toml")).unwrap();
    assert_eq!(config.local_models.len(), 1);
    assert_eq!(config.local_models[0].description, "child");
    assert_eq!(config.local_models[0].context, 2048);
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
key = "t"

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
    assert_eq!(config.devices.len(), 2);
    let local = config.devices.iter().find(|d| d.id == "local-gpu").unwrap();
    assert_eq!(local.lanes.len(), 1);
    assert_eq!(local.lanes[0].id, "generative");
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
key = "t"

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
context = 1024
device = "local-gpu"
lane = "generative"
"#,
    );
    let config = load_path(&tmp.path().join("gemma.toml")).unwrap();
    assert_eq!(config.devices.len(), 1);
    assert_eq!(config.devices[0].lanes.len(), 1);
    assert_eq!(config.devices[0].lanes[0].id, "generative");
    assert_eq!(config.devices[0].lanes[0].concurrency, 3);
    assert_eq!(
        config
            .local_model_concurrency(&config.local_models[0])
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
key = "t"

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
key = "t"

[device]
id = "not-an-array"
"#,
    );
    let err = load_path(&tmp.path().join("broken.toml")).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("broken.toml:"), "expected path:line in {msg}");
    assert!(msg.contains("expected"), "expected type hint in {msg}");
}
