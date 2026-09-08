//! Module size ratchet for the Workshop binary and its tests.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Fixed growth allowance above each recorded physical-line count.
const SLACK: usize = 30;

#[derive(serde::Deserialize)]
struct CeilingsFile {
    source_modules: BTreeMap<String, usize>,
    test_modules: BTreeMap<String, usize>,
}

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
fn recorded_ceilings() -> CeilingsFile {
    let path = crate_root().join("module-ceilings.toml");
    let text = fs::read_to_string(&path).expect("module-ceilings.toml exists at the crate root");
    toml::from_str(&text).expect("module-ceilings.toml parses as TOML")
}

fn physical_lines(text: &str) -> usize {
    text.lines().count()
}

#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
fn collect_rust_modules(root: &Path, directory: &Path, out: &mut BTreeMap<String, usize>) {
    let mut entries = fs::read_dir(directory)
        .expect("read a Workshop module directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read every Workshop module entry");
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_modules(root, &path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let relative = path
                .strip_prefix(root)
                .expect("a measured module stays below its root");
            let name = relative.to_string_lossy().replace('\\', "/");
            let text = fs::read_to_string(&path).expect("read a Workshop Rust module");
            out.insert(name, physical_lines(&text));
        }
    }
}

fn measured_modules(directory: &str) -> BTreeMap<String, usize> {
    let root = crate_root().join(directory);
    let mut modules = BTreeMap::new();
    collect_rust_modules(&root, &root, &mut modules);
    modules
}

fn check_ceiling(
    group: &str,
    measured: &BTreeMap<String, usize>,
    ceilings: &BTreeMap<String, usize>,
) -> Vec<String> {
    let mut overgrown = Vec::new();
    for (module, count) in measured {
        let Some(&ceiling) = ceilings.get(module) else {
            continue;
        };
        if *count > ceiling + SLACK {
            overgrown.push(format!(
                "  {group}/{module}: {count} lines, ceiling {ceiling} (slack {SLACK} allows {})",
                ceiling + SLACK
            ));
        }
    }
    overgrown
}

fn check_sync(
    group: &str,
    measured: &BTreeMap<String, usize>,
    ceilings: &BTreeMap<String, usize>,
) -> Vec<String> {
    let mut drift = Vec::new();
    for (module, count) in measured {
        if !ceilings.contains_key(module) {
            drift.push(format!(
                "  missing entry: add `\"{module}\" = {count}` to [{group}_modules]"
            ));
        }
    }
    for module in ceilings.keys() {
        if !measured.contains_key(module) {
            drift.push(format!(
                "  stale entry: remove `\"{module}\"` from [{group}_modules]"
            ));
        }
    }
    drift
}

#[test]
fn every_workshop_module_stays_at_or_below_its_ceiling_plus_slack() {
    let recorded = recorded_ceilings();
    let mut overgrown = check_ceiling("source", &measured_modules("src"), &recorded.source_modules);
    overgrown.extend(check_ceiling(
        "test",
        &measured_modules("tests"),
        &recorded.test_modules,
    ));
    assert!(
        overgrown.is_empty(),
        "Workshop module size ratchet tripped:\n{}",
        overgrown.join("\n")
    );
}

#[test]
fn the_ceiling_file_lists_every_source_and_test_module_exactly_once() {
    let recorded = recorded_ceilings();
    let mut drift = check_sync("source", &measured_modules("src"), &recorded.source_modules);
    drift.extend(check_sync(
        "test",
        &measured_modules("tests"),
        &recorded.test_modules,
    ));
    assert!(
        drift.is_empty(),
        "module-ceilings.toml is out of step with Workshop modules:\n{}",
        drift.join("\n")
    );
}

#[test]
fn the_counting_rule_handles_crlf_lf_and_unterminated_files() {
    assert_eq!(physical_lines(""), 0);
    assert_eq!(physical_lines("one line, no trailing newline"), 1);
    assert_eq!(physical_lines("one line\n"), 1);
    assert_eq!(physical_lines("one\r\ntwo\r\n"), 2);
}
