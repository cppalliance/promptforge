//! The module size ratchet: every Rust module under src/ has a ceiling
//! recorded in module-ceilings.toml at the crate root, and may not grow
//! past it plus a small slack. Structure without enforcement regresses in
//! AI-authored code, so the ceilings are load-bearing; the file's header
//! documents the counting rule and how to raise a ceiling legitimately.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Lines a module may run past its recorded ceiling before the ratchet
/// trips. A fixed count rather than a percentage, so the largest modules
/// get no extra headroom; module-ceilings.toml states the full policy.
const SLACK: usize = 30;

/// The shape of module-ceilings.toml.
#[derive(serde::Deserialize)]
struct CeilingsFile {
    /// src/-relative module path with forward slashes, to its ceiling.
    modules: BTreeMap<String, usize>,
}

/// The crate root, resolved from the manifest directory cargo sets for
/// every test invocation.
fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Reads the recorded ceilings out of module-ceilings.toml.
#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
fn recorded_ceilings() -> BTreeMap<String, usize> {
    let path = crate_root().join("module-ceilings.toml");
    let text = fs::read_to_string(&path).expect("module-ceilings.toml exists at the crate root");
    let file: CeilingsFile = toml::from_str(&text).expect("module-ceilings.toml parses as TOML");
    file.modules
}

/// Counts physical lines the way diff tooling does: every newline ends a
/// line, and a final line missing its trailing newline still counts -
/// which is exactly what [`str::lines`] yields.
fn physical_lines(text: &str) -> usize {
    text.lines().count()
}

/// Walks a directory under src/, recording every .rs file's line count
/// keyed by its src/-relative forward-slash path.
#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
fn collect_modules(dir: &Path, src: &Path, out: &mut BTreeMap<String, usize>) {
    for entry in fs::read_dir(dir).expect("directories under src/ are readable") {
        let path = entry
            .expect("directory entries under src/ are readable")
            .path();
        if path.is_dir() {
            collect_modules(&path, src, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = fs::read_to_string(&path).expect("modules under src/ are readable UTF-8");
            let module = path
                .strip_prefix(src)
                .expect("walked paths sit under src/")
                .to_str()
                .expect("module paths are valid UTF-8")
                .replace('\\', "/");
            out.insert(module, physical_lines(&text));
        }
    }
}

/// Measures every module under the crate's src/ tree.
fn measured_modules() -> BTreeMap<String, usize> {
    let src = crate_root().join("src");
    let mut out = BTreeMap::new();
    collect_modules(&src, &src, &mut out);
    out
}

#[test]
fn every_module_stays_at_or_below_its_ceiling_plus_slack() {
    let ceilings = recorded_ceilings();
    let mut overgrown = Vec::new();
    for (module, count) in measured_modules() {
        // A module absent from the ceilings file is reported by the sync
        // test below, with its own message.
        let Some(&ceiling) = ceilings.get(&module) else {
            continue;
        };
        if count > ceiling + SLACK {
            overgrown.push(format!(
                "  {module}: {count} lines, ceiling {ceiling} (slack {SLACK} allows {})",
                ceiling + SLACK
            ));
        }
    }
    assert!(
        overgrown.is_empty(),
        "module size ratchet tripped:\n{}\nsplit the module, or raise its ceiling in \
         module-ceilings.toml in this same commit and state the reason in the commit message",
        overgrown.join("\n")
    );
}

#[test]
fn the_ceilings_file_lists_exactly_the_modules_under_src() {
    let ceilings = recorded_ceilings();
    let measured = measured_modules();
    let mut drift = Vec::new();
    for (module, count) in &measured {
        if !ceilings.contains_key(module) {
            drift.push(format!(
                "  missing entry: add `\"{module}\" = {count}` (a new module ships with a \
                 recorded ceiling)"
            ));
        }
    }
    for module in ceilings.keys() {
        if !measured.contains_key(module) {
            drift.push(format!(
                "  stale entry: \"{module}\" no longer exists under src/; delete its line"
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "module-ceilings.toml is out of step with src/:\n{}",
        drift.join("\n")
    );
}

// The recorded ceilings were produced by this exact rule, so a drift in
// the counter (newline-byte counting, CRLF sensitivity) would silently
// weaken or false-trip the ratchet on this mixed CRLF/LF checkout.
#[test]
fn the_counting_rule_reads_crlf_lf_and_unterminated_files_alike() {
    assert_eq!(physical_lines(""), 0);
    assert_eq!(physical_lines("one line, no trailing newline"), 1);
    assert_eq!(physical_lines("a\nb\n"), 2);
    assert_eq!(
        physical_lines("a\r\nb\r\n"),
        2,
        "a CRLF checkout counts like an LF one"
    );
    assert_eq!(
        physical_lines("a\nb"),
        2,
        "a final line missing its newline still counts"
    );
    assert_eq!(
        physical_lines("a\n\n"),
        2,
        "a trailing blank line ends at its newline and counts"
    );
}
