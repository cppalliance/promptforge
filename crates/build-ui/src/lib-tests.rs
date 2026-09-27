//! The esbuild lookup, the nearest lockfile, the npm workspace members,
//! and the watch list, over scratch package trees.

use std::fs;
use std::path::{Path, PathBuf};

use super::{UiBuild, find_esbuild, nearest_lockfile, watched_paths, workspace_members};

const ESBUILD_SHIM: &str = if cfg!(windows) {
    "esbuild.cmd"
} else {
    "esbuild"
};

const CONFIG: UiBuild = UiBuild {
    static_files: &["index.html"],
    define_app_version: false,
    splitting: false,
};

fn install_esbuild(dir: &Path) -> PathBuf {
    let shim = dir.join("node_modules").join(".bin").join(ESBUILD_SHIM);
    write(&shim, "");
    shim
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn a_ui_local_install_wins_over_an_ancestor_install() {
    let temp = tempfile::TempDir::new().unwrap();
    let ui = temp.path().join("ui");
    write(&temp.path().join("package-lock.json"), "{}");
    install_esbuild(temp.path());
    let local = install_esbuild(&ui);
    assert_eq!(find_esbuild(&ui), Some(local));
}

#[test]
fn an_ancestor_install_is_found() {
    let temp = tempfile::TempDir::new().unwrap();
    let ui = temp.path().join("ui");
    fs::create_dir_all(&ui).unwrap();
    write(&temp.path().join("package-lock.json"), "{}");
    let hoisted = install_esbuild(temp.path());
    assert_eq!(find_esbuild(&ui), Some(hoisted));
}

#[test]
fn no_install_in_the_tree_finds_nothing_in_it() {
    let temp = tempfile::TempDir::new().unwrap();
    let ui = temp.path().join("ui");
    fs::create_dir_all(ui.join("node_modules").join(".bin")).unwrap();
    assert!(find_esbuild(&ui).is_none());
}

#[test]
fn an_install_above_the_install_root_is_not_used() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path().join("root");
    let ui = root.join("ui");
    fs::create_dir_all(&ui).unwrap();
    write(&root.join("package-lock.json"), "{}");
    install_esbuild(temp.path());
    assert_eq!(find_esbuild(&ui), None);
}

#[test]
fn the_nearest_lockfile_is_the_workspace_root_one() {
    let temp = tempfile::TempDir::new().unwrap();
    let ui = temp.path().join("ui");
    write(&ui.join("package.json"), "{}");
    let root_lockfile = temp.path().join("package-lock.json");
    write(&root_lockfile, "{}");
    assert_eq!(nearest_lockfile(&ui), Some(root_lockfile));
}

#[test]
fn a_ui_local_lockfile_is_nearer_than_the_root_one() {
    let temp = tempfile::TempDir::new().unwrap();
    let ui = temp.path().join("ui");
    write(&temp.path().join("package-lock.json"), "{}");
    let local_lockfile = ui.join("package-lock.json");
    write(&local_lockfile, "{}");
    assert_eq!(nearest_lockfile(&ui), Some(local_lockfile));
}

#[test]
fn the_other_listed_members_are_returned() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("package.json"),
        r#"{ "private": true, "workspaces": ["ui", "look"] }"#,
    );
    write(&root.join("package-lock.json"), "{}");
    write(&root.join("ui").join("package.json"), r#"{ "name": "ui" }"#);
    fs::create_dir_all(root.join("look")).unwrap();
    let workspace = workspace_members(&root.join("ui")).unwrap().unwrap();
    assert_eq!(workspace.root, root);
    assert_eq!(workspace.members, vec![root.join("look")]);
}

#[test]
fn a_package_json_without_workspaces_is_not_the_root() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("package.json"),
        r#"{ "workspaces": ["inner/ui", "look"] }"#,
    );
    write(&root.join("package-lock.json"), "{}");
    write(
        &root.join("inner").join("package.json"),
        r#"{ "name": "inner" }"#,
    );
    let ui = root.join("inner").join("ui");
    write(&ui.join("package.json"), r#"{ "name": "ui" }"#);
    let workspace = workspace_members(&ui).unwrap().unwrap();
    assert_eq!(workspace.root, root);
    assert_eq!(workspace.members, vec![root.join("look")]);
}

#[test]
fn a_workspace_root_above_the_install_root_is_not_adopted() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("package.json"),
        r#"{ "workspaces": ["ui", "look"] }"#,
    );
    fs::create_dir_all(root.join("look")).unwrap();
    let ui = root.join("ui");
    write(&ui.join("package.json"), r#"{ "name": "ui" }"#);
    write(&ui.join("package-lock.json"), "{}");
    let workspace = workspace_members(&ui).unwrap();
    assert!(workspace.is_none(), "adopted {workspace:?}");
}

#[test]
fn a_malformed_package_json_is_an_error_naming_it() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    let broken = root.join("package.json");
    write(&broken, r#"{ "workspaces": ["ui", "look"] "#);
    write(&root.join("package-lock.json"), "{}");
    let ui = root.join("ui");
    write(&ui.join("package.json"), r#"{ "name": "ui" }"#);
    let error = workspace_members(&ui).unwrap_err().to_string();
    assert!(
        error.contains(&broken.display().to_string()),
        "the error does not name {}: {error}",
        broken.display()
    );
    assert!(watched_paths(&ui, &CONFIG).is_err());
}

#[test]
fn a_malformed_package_json_above_the_install_root_is_not_read() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("package.json"),
        r#"{ "workspaces": ["ui", "look"] "#,
    );
    let ui = root.join("ui");
    write(&ui.join("package.json"), r#"{ "name": "ui" }"#);
    write(&ui.join("package-lock.json"), "{}");
    let workspace = workspace_members(&ui).unwrap();
    assert!(workspace.is_none(), "adopted {workspace:?}");
    assert!(watched_paths(&ui, &CONFIG).is_ok());
}

#[test]
fn the_ui_dir_is_left_out_when_reached_through_a_parent_hop() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("package.json"),
        r#"{ "workspaces": ["ui", "look"] }"#,
    );
    write(&root.join("package-lock.json"), "{}");
    for member in ["ui", "look", "server"] {
        fs::create_dir_all(root.join(member)).unwrap();
    }
    let workspace = workspace_members(&root.join("server").join("..").join("ui"))
        .unwrap()
        .unwrap();
    assert_eq!(workspace.root, root);
    assert_eq!(workspace.members, vec![root.join("look")]);
}

#[test]
fn watch_covers_the_workspace_root_and_the_other_members() {
    let temp = tempfile::TempDir::new().unwrap();
    let root = temp.path();
    write(
        &root.join("package.json"),
        r#"{ "workspaces": ["ui", "look"] }"#,
    );
    write(&root.join("package-lock.json"), "{}");
    write(&root.join("look").join("tokens.css"), "");
    let ui = root.join("ui");
    write(&ui.join("package.json"), r#"{ "name": "ui" }"#);
    write(&ui.join("index.html"), "");
    write(&ui.join("src").join("main.ts"), "");
    let paths = watched_paths(&ui, &CONFIG).unwrap();
    for expected in [
        root.join("package.json"),
        root.join("package-lock.json"),
        root.join("look"),
        ui.join("src"),
        ui.join("index.html"),
        ui.join("package.json"),
    ] {
        assert!(
            paths.contains(&expected),
            "{} is not watched",
            expected.display()
        );
    }
    assert!(
        !paths.contains(&ui),
        "the UI directory is watched recursively"
    );
}

#[test]
fn watch_lists_only_paths_that_exist() {
    let temp = tempfile::TempDir::new().unwrap();
    let ui = temp.path().join("ui");
    write(&ui.join("src").join("main.ts"), "");
    let paths = watched_paths(&ui, &CONFIG).unwrap();
    assert!(paths.contains(&ui.join("src")));
    for path in &paths {
        assert!(path.exists(), "{} is watched but missing", path.display());
    }
}
