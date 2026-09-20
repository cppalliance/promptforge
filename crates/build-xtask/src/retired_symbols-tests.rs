//! Fixture tests for the retired-symbol scan: one source tree per case,
//! written into a temporary directory and scanned in isolation.

use std::path::Path;

use super::*;

const SEEDS: [&str; 2] = ["Observer", "GatewaySource"];

/// Writes a source tree of `(relative path, contents)` pairs into a fresh
/// temporary directory.
fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    for (rel, text) in files {
        let path = dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory creates");
        std::fs::write(&path, text).expect("the source writes");
    }
    dir
}

fn scan(root: &Path) -> Vec<Hit> {
    retired_symbols(root, &SEEDS)
}

#[test]
fn a_seed_in_live_code_is_reported_with_its_file_line_and_symbol() {
    let dir = tree(&[("src/lib.rs", "use std::fmt;\n\npub trait Observer {}\n")]);
    let hits = scan(dir.path());
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].symbol, "Observer");
    assert_eq!(hits[0].line, 3);
    assert!(hits[0].file.ends_with("lib.rs"), "{:?}", hits[0].file);
    let rendered = hits[0].to_string();
    assert!(
        rendered.contains("lib.rs") && rendered.contains(":3") && rendered.contains("Observer"),
        "the hit renders file, line, and symbol: {rendered}"
    );
}

#[test]
fn a_seed_only_in_comments_passes() {
    let dir = tree(&[(
        "src/lib.rs",
        "// Observer was retired\n\
         /// The old GatewaySource is gone.\n\
         /* a block mentioning Observer /* nested GatewaySource */ still Observer */\n\
         //! crate docs name Observer too\n\
         pub struct Live;\n",
    )]);
    let hits = scan(dir.path());
    assert!(hits.is_empty(), "comments are stripped: {hits:?}");
}

#[test]
fn a_seed_only_in_string_literals_passes() {
    let dir = tree(&[(
        "src/lib.rs",
        "const A: &str = \"Observer\";\n\
         const B: &str = r#\"a \"quoted\" GatewaySource\"#;\n\
         const C: &[u8] = b\"Observer\";\n\
         const D: &str = \"escaped \\\" then Observer\";\n\
         const E: &str = \"// not a comment: GatewaySource\";\n\
         const F: &str = r\"raw GatewaySource\";\n",
    )]);
    let hits = scan(dir.path());
    assert!(hits.is_empty(), "string literals are stripped: {hits:?}");
}

#[test]
fn char_literals_and_lifetimes_do_not_swallow_live_code() {
    // A `'"'` char literal must not open a string, and a lifetime `'a` must
    // not open a char literal; both would hide the seed that follows.
    let dir = tree(&[(
        "src/lib.rs",
        "const Q: char = '\"';\npub struct Observer;\n\
         fn f<'a>(x: &'a str) -> GatewaySource { todo!() }\n\
         const E: char = '\\'';\nconst N: char = '\\n';\n",
    )]);
    let hits = scan(dir.path());
    let symbols: Vec<&str> = hits.iter().map(|hit| hit.symbol.as_str()).collect();
    assert_eq!(symbols, ["Observer", "GatewaySource"], "{hits:?}");
    assert_eq!(hits[0].line, 2);
    assert_eq!(hits[1].line, 3);
}

#[test]
fn a_seed_only_in_an_inline_cfg_test_module_passes() {
    let dir = tree(&[(
        "src/lib.rs",
        "pub struct Live;\n\n\
         #[cfg(test)]\nmod tests {\n    use super::Observer;\n    fn g() -> GatewaySource {}\n}\n",
    )]);
    let hits = scan(dir.path());
    assert!(hits.is_empty(), "cfg(test) modules are skipped: {hits:?}");
}

#[test]
fn a_seed_only_in_cfg_test_module_files_passes() {
    // Both the `#[path]` sibling form and the plain `mod name;` form
    // resolve to files the scan must skip.
    let dir = tree(&[
        (
            "src/lib.rs",
            "pub struct Live;\nmod engine;\n\
             #[cfg(test)]\n#[path = \"lib-tests.rs\"]\nmod tests;\n\
             #[cfg(test)]\nmod more_tests;\n",
        ),
        ("src/lib-tests.rs", "use Observer;\n"),
        ("src/more_tests.rs", "use GatewaySource;\n"),
        (
            "src/engine.rs",
            "pub struct Engine;\n#[cfg(test)]\nmod tests;\n",
        ),
        ("src/engine/tests.rs", "use Observer;\n"),
    ]);
    let hits = scan(dir.path());
    assert!(
        hits.is_empty(),
        "cfg(test) module files are skipped: {hits:?}"
    );
}

#[test]
fn a_seed_only_in_visibility_qualified_cfg_test_module_files_passes() {
    // `pub(crate) mod fixtures;` is the shape the repository writes for
    // shared test helpers; every visibility form must still resolve to a
    // module file the scan skips, and a live `pub` item after them is
    // still scanned.
    let dir = tree(&[
        (
            "src/lib.rs",
            "pub struct Live;\n\
             #[cfg(test)]\npub(crate) mod fixtures;\n\
             #[cfg(test)]\npub mod helpers;\n\
             #[cfg(test)]\n#[path = \"lib-cases.rs\"]\npub(in crate) mod cases;\n\
             #[cfg(test)]\npub (crate) mod spaced;\n\
             pub struct GatewaySource;\n",
        ),
        ("src/fixtures.rs", "pub struct Observer;\n"),
        ("src/helpers.rs", "use GatewaySource;\n"),
        ("src/lib-cases.rs", "use Observer;\n"),
        ("src/spaced.rs", "use Observer;\n"),
    ]);
    let hits = scan(dir.path());
    let symbols: Vec<&str> = hits.iter().map(|hit| hit.symbol.as_str()).collect();
    assert_eq!(
        symbols,
        ["GatewaySource"],
        "qualified cfg(test) module files are skipped: {hits:?}"
    );
    assert!(hits[0].file.ends_with("lib.rs"), "{:?}", hits[0].file);
    assert_eq!(hits[0].line, 11);
}

#[test]
fn a_seed_only_in_a_cfg_test_item_passes_and_later_live_code_is_still_scanned() {
    let dir = tree(&[(
        "src/lib.rs",
        "#[cfg(test)]\nuse crate::Observer;\n\
         #[cfg(test)]\nfn helper() -> Observer { Observer }\n\
         #[cfg(test)]\n#[allow(dead_code)]\nstruct Unit;\n\
         pub struct GatewaySource;\n",
    )]);
    let hits = scan(dir.path());
    let symbols: Vec<&str> = hits.iter().map(|hit| hit.symbol.as_str()).collect();
    assert_eq!(
        symbols,
        ["GatewaySource"],
        "the cfg(test) items are skipped and the live item after them is not: {hits:?}"
    );
    assert_eq!(hits[0].line, 8);
}

#[test]
fn tests_directories_and_test_support_paths_are_skipped() {
    let dir = tree(&[
        ("src/lib.rs", "pub struct Live;\n"),
        ("tests/it/main.rs", "use Observer;\n"),
        ("src/test_support.rs", "pub struct Observer;\n"),
        ("src/test_support/driver.rs", "pub struct GatewaySource;\n"),
        ("src/execute/tests/mod.rs", "use Observer;\n"),
        ("target/debug/build/generated.rs", "use Observer;\n"),
    ]);
    let hits = scan(dir.path());
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn a_seed_embedded_in_a_longer_identifier_passes() {
    let dir = tree(&[(
        "src/lib.rs",
        "pub struct ObserverAdapter;\npub fn my_Observer_x() {}\npub struct observer;\n\
         pub struct GatewaySources;\n",
    )]);
    let hits = scan(dir.path());
    assert!(
        hits.is_empty(),
        "identifier matches are whole-token: {hits:?}"
    );
}

#[test]
fn hits_are_ordered_by_file_then_line_and_cover_every_occurrence() {
    let dir = tree(&[
        ("src/b.rs", "use GatewaySource;\n\nimpl Observer for X {}\n"),
        ("src/a.rs", "fn f(o: &dyn Observer) {}\n"),
    ]);
    let hits = scan(dir.path());
    let summary: Vec<(String, usize, &str)> = hits
        .iter()
        .map(|hit| {
            let name = hit
                .file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned();
            (name, hit.line, hit.symbol.as_str())
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("a.rs".to_owned(), 1, "Observer"),
            ("b.rs".to_owned(), 1, "GatewaySource"),
            ("b.rs".to_owned(), 3, "Observer"),
        ],
        "{hits:?}"
    );
}

#[test]
fn an_absent_source_root_yields_no_hits() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let hits = scan(&dir.path().join("absent"));
    assert!(hits.is_empty(), "{hits:?}");
}
