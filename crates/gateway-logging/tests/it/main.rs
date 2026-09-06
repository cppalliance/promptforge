//! The dependency boundary: `gateway-logging` links only the standard
//! library, `tracing`, and `tracing-subscriber`, so the log pipeline can
//! never grow a dependency on the gateway, sidecar state, or STT types.
//! This test reads the crate's own manifest and fails when any other
//! dependency appears.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

/// The allowlist the crate's AGENTS.md grants.
const ALLOWED: [&str; 2] = ["tracing", "tracing-subscriber"];

#[test]
fn the_manifest_declares_only_the_tracing_dependencies() {
    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path).expect("the crate manifest must be readable");

    let mut section = String::new();
    let mut declared = BTreeSet::new();
    let mut forbidden_tables = Vec::new();
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).trim().to_string();
            // No build, dev, or target-specific dependency tables: the
            // boundary covers every way a crate can enter the build.
            if section != "dependencies"
                && (section.ends_with("dependencies") || section.starts_with("target."))
            {
                forbidden_tables.push(section.clone());
            }
            // A `[dependencies.<name>]` sub-table declares the dependency
            // `<name>` without a `key = value` line under
            // `[dependencies]`, so count it against the same allowlist.
            if let Some(rest) = section.strip_prefix("dependencies.")
                && let Some(name) = rest.split('.').next()
            {
                declared.insert(name.to_string());
            }
            continue;
        }
        if section == "dependencies"
            && !line.starts_with('#')
            && let Some((key, _)) = line.split_once('=')
        {
            // `tracing.workspace = true` names the `tracing` crate: the
            // dotted suffix is workspace inheritance, not part of the
            // dependency name.
            let key = key.trim();
            let name = key.split_once('.').map_or(key, |(name, _)| name);
            declared.insert(name.to_string());
        }
    }

    let expected: BTreeSet<&str> = ALLOWED.into_iter().collect();
    let declared: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    assert_eq!(
        declared, expected,
        "gateway-logging may depend only on tracing and tracing-subscriber"
    );
    assert!(
        forbidden_tables.is_empty(),
        "gateway-logging declares no build, dev, or target-specific dependencies: {forbidden_tables:?}"
    );
}
