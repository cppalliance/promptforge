//! Ceiling wiring: every workspace crate's build script runs the
//! compile-time file-size check.
//!
//! The build script is read as tokens, so a comment or a string that only
//! spells the call is never read as one.

use std::fs;
use std::path::{Component, Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use syn::{Expr, ExprLit, Lit, Meta};

/// The hakari crate: generated, with no build script, so this check holds
/// its files to the limit instead.
const WORKSPACE_HACK: &str = "workspace-hack";
/// The limit `build_ceiling::check()` enforces in every other crate.
const LINE_LIMIT: usize = 500;
/// The ceiling's source, from the workspace root: the file the `#[path]`
/// form includes.
const CEILING_SOURCE: [&str; 4] = ["crates", "build-ceiling", "src", "lib.rs"];

/// Checks that every workspace crate's package-root `build.rs` calls
/// `build_ceiling::check()` and reaches it through a `build-ceiling`
/// build-dependency or a `#[path]` include of the ceiling's source, and
/// that no `workspace-hack` file is over the limit. A crate whose manifest
/// could not be read is held to the `#[path]` form, the one its build
/// script alone can show, and a walk that found no crate fails.
pub(super) fn ceiling_wiring_violations(root: &Path) -> Vec<String> {
    let walk = crate::product::workspace_crates(root);
    if walk.crates.is_empty() && walk.unread.is_empty() {
        return vec![format!(
            "{}: the ceiling wiring check scanned nothing: no workspace crate was found, so \
             none can be shown to run `build_ceiling::check()`",
            root.join("crates").display()
        )];
    }
    let mut violations = Vec::new();
    for krate in &walk.crates {
        let dir = root.join(&krate.dir);
        if krate.package == WORKSPACE_HACK {
            violations.extend(workspace_hack_violations(&dir));
        } else {
            let manifest = fs::read_to_string(dir.join("Cargo.toml"))
                .ok()
                .and_then(|text| toml::from_str::<toml::Value>(&text).ok());
            violations.extend(crate_violation(root, &dir, manifest.as_ref()));
        }
    }
    for dir in &walk.unread {
        violations.extend(crate_violation(root, &root.join(dir), None));
    }
    violations
}

/// Why the crate at `dir` is not wired, or `None` when it is.
fn crate_violation(root: &Path, dir: &Path, manifest: Option<&toml::Value>) -> Option<String> {
    let build = dir.join("build.rs");
    if let Some(target) = manifest
        .and_then(|manifest| manifest.get("package"))
        .and_then(|package| package.get("build"))
        .filter(|target| target.as_str() != Some("build.rs"))
    {
        return Some(format!(
            "{}: sets `package.build = {target}`, so cargo never runs the package-root \
             build.rs; remove the key and call `build_ceiling::check()` from build.rs",
            dir.join("Cargo.toml").display()
        ));
    }
    let tokens: TokenStream = match fs::read_to_string(&build) {
        Ok(text) => match text.parse() {
            Ok(tokens) => tokens,
            Err(error) => return Some(format!("{}: unparseable source: {error}", build.display())),
        },
        Err(error) => {
            return Some(format!(
                "{}: no readable package-root build script ({error}); every crate's build.rs \
                 calls `build_ceiling::check()`, and a `src/build.rs` module is not one",
                build.display()
            ));
        }
    };
    if !calls_check(tokens.clone()) {
        return Some(format!(
            "{}: does not call `build_ceiling::check()`, the compile-time file-size check",
            build.display()
        ));
    }
    if manifest.is_some_and(names_ceiling) || includes_ceiling(root, dir, tokens) {
        return None;
    }
    Some(format!(
        "{}: calls `build_ceiling::check()` but reaches neither a `build-ceiling` entry under \
         [build-dependencies] nor `#[path = \"<relative path>/build-ceiling/src/lib.rs\"] mod \
         build_ceiling;`",
        build.display()
    ))
}

/// Whether `tokens`, at any depth, hold the call `build_ceiling::check()`.
fn calls_check(tokens: TokenStream) -> bool {
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    trees.windows(5).any(|window| {
        matches!(window, [
            TokenTree::Ident(module),
            TokenTree::Punct(first),
            TokenTree::Punct(second),
            TokenTree::Ident(function),
            TokenTree::Group(arguments),
        ] if module == "build_ceiling"
            && first.as_char() == ':'
            && second.as_char() == ':'
            && function == "check"
            && arguments.delimiter() == Delimiter::Parenthesis
            && arguments.stream().is_empty())
    }) || trees
        .iter()
        .any(|tree| matches!(tree, TokenTree::Group(group) if calls_check(group.stream())))
}

/// Whether a `build-ceiling` entry sits in any build-dependency table.
fn names_ceiling(manifest: &toml::Value) -> bool {
    crate::manifest::dependency_tables(manifest, &["build-dependencies"])
        .iter()
        .any(|(_, table)| {
            table.iter().any(|(key, value)| {
                value
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(key)
                    == "build-ceiling"
            })
        })
}

/// Whether the build script of the crate at `dir` declares
/// `mod build_ceiling;` under a `#[path]` that resolves to the ceiling's
/// source.
fn includes_ceiling(root: &Path, dir: &Path, tokens: TokenStream) -> bool {
    let source = lexical(
        &CEILING_SOURCE
            .iter()
            .fold(root.to_path_buf(), |path, part| path.join(part)),
    );
    let mut paths = Vec::new();
    let mut tokens = tokens.into_iter().peekable();
    while let Some(token) = tokens.next() {
        match token {
            TokenTree::Punct(pound) if pound.as_char() == '#' => {
                if matches!(tokens.peek(), Some(TokenTree::Punct(bang)) if bang.as_char() == '!') {
                    tokens.next();
                    tokens.next();
                } else if let Some(TokenTree::Group(attribute)) = tokens.next()
                    && attribute.delimiter() == Delimiter::Bracket
                    && let Ok(Meta::NameValue(pair)) = syn::parse2::<Meta>(attribute.stream())
                    && pair.path.is_ident("path")
                    && let Expr::Lit(ExprLit {
                        lit: Lit::Str(path),
                        ..
                    }) = pair.value
                {
                    paths.push(path.value());
                }
            }
            TokenTree::Ident(keyword) if keyword == "mod" => {
                let named = matches!(tokens.peek(), Some(TokenTree::Ident(name)) if name == "build_ceiling");
                if named && paths.iter().any(|path| lexical(&dir.join(path)) == source) {
                    return true;
                }
                paths.clear();
            }
            _ => paths.clear(),
        }
    }
    false
}

/// `path` with every `.` dropped and every `..` applied, without touching
/// the filesystem.
fn lexical(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    resolved
}

/// The `workspace-hack` files over the limit, and the ones that could not
/// be read.
fn workspace_hack_violations(dir: &Path) -> Vec<String> {
    super::rust_files(dir)
        .into_iter()
        .filter_map(|file| match fs::read_to_string(&file) {
            Ok(text) => {
                let lines = text.lines().count();
                (lines > LINE_LIMIT).then(|| {
                    format!(
                        "{} has {lines} lines, over the limit of {LINE_LIMIT}: workspace-hack \
                         has no build script, so this check holds it to the limit \
                         `build_ceiling::check()` enforces in every other crate",
                        file.display()
                    )
                })
            }
            Err(error) => Some(format!("{}: unreadable source: {error}", file.display())),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const BUILD_DEPENDENCY: &str = "[build-dependencies]\nbuild-ceiling.workspace = true\n";
    const CALLS_CHECK: &str =
        "fn main() -> Result<(), build_ceiling::Violations> { build_ceiling::check() }\n";

    /// A build script in the `#[path]` form, including the ceiling from
    /// `source`.
    fn includes(source: &str) -> String {
        format!(
            "#[expect(unreachable_pub, reason = \"private\")]\n\
             #[path = \"{source}\"]\nmod build_ceiling;\n{CALLS_CHECK}"
        )
    }

    /// Writes a crate under `crates/<dir>/` named `name`, with `tail` inside
    /// and after its `[package]` table and `build` as its package-root
    /// `build.rs` when given. Returns the crate directory.
    fn write_built(root: &Path, dir: &str, name: &str, tail: &str, build: Option<&str>) -> PathBuf {
        let dir = root.join("crates").join(dir);
        std::fs::create_dir_all(dir.join("src")).expect("the crate directory creates");
        let manifest = format!("[package]\nname = \"{name}\"\n{tail}");
        std::fs::write(dir.join("Cargo.toml"), manifest).expect("the manifest writes");
        if let Some(build) = build {
            std::fs::write(dir.join("build.rs"), build).expect("build.rs writes");
        }
        dir
    }

    /// A wired workspace: a crate in each form, the ceiling's own crate,
    /// and a `workspace-hack` with no build script.
    fn write_wired(root: &Path) {
        let local = Some(CALLS_CHECK);
        write_built(
            root,
            "gateway/local",
            "gateway-local",
            BUILD_DEPENDENCY,
            local,
        );
        let web = includes("../build-ceiling/src/lib.rs");
        write_built(root, "plugin-web", "plugin-web", "", Some(&web));
        let runner = includes("../../build-ceiling/src/lib.rs");
        let dir = "harness-internal/runner";
        write_built(root, dir, "harness-runner", "", Some(&runner));
        let own = includes("src/lib.rs");
        write_built(root, "build-ceiling", "build-ceiling", "", Some(&own));
        let hack = write_built(root, "workspace-hack", "workspace-hack", "", None);
        std::fs::write(hack.join("src").join("lib.rs"), "// hakari\n").expect("lib.rs writes");
    }

    /// The wiring violations of a wired workspace plus one crate.
    fn wiring_with(dir: &str, name: &str, tail: &str, build: Option<&str>) -> Vec<String> {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_wired(root.path());
        write_built(root.path(), dir, name, tail, build);
        ceiling_wiring_violations(root.path())
    }

    #[test]
    fn dependency_wired_and_path_wired_crates_pass() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_wired(root.path());
        let violations = ceiling_wiring_violations(root.path());
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn a_build_script_that_names_the_call_only_in_a_comment_fails() {
        let build = Some("// build_ceiling::check()\nfn main() {}\n");
        let violations = wiring_with("gateway/config", "gateway-config", BUILD_DEPENDENCY, build);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("config") && violations[0].contains("does not call"));
    }

    #[test]
    fn a_crate_with_no_package_root_build_script_fails_even_with_a_src_build_module() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_wired(root.path());
        let name = "promptforge-parser";
        let parser = write_built(root.path(), "parser", name, BUILD_DEPENDENCY, None);
        let module = parser.join("src").join("build.rs");
        std::fs::write(module, CALLS_CHECK).expect("the module writes");
        let violations = ceiling_wiring_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("parser") && violations[0].contains("build.rs"));
    }

    #[test]
    fn a_path_include_of_any_file_but_the_ceiling_source_fails() {
        let build = includes("../plugin-web/src/lib.rs");
        let name = "harness-gateway-client";
        let violations = wiring_with(name, name, "", Some(&build));
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains(name));
    }

    #[test]
    fn a_package_build_key_fails_because_cargo_skips_the_wired_build_rs() {
        let tail = format!("build = false\n{BUILD_DEPENDENCY}");
        let build = Some(CALLS_CHECK);
        let violations = wiring_with("gateway/routing", "gateway-routing", &tail, build);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("package.build"));
    }

    #[test]
    fn a_crate_whose_manifest_does_not_parse_is_held_to_the_path_form() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_wired(root.path());
        let broken = root.path().join("crates").join("broken");
        std::fs::create_dir_all(&broken).expect("the crate directory creates");
        std::fs::write(broken.join("Cargo.toml"), "not [valid toml").expect("the manifest writes");
        std::fs::write(broken.join("build.rs"), CALLS_CHECK).expect("build.rs writes");
        let violations = ceiling_wiring_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("broken"));
    }

    #[test]
    fn a_workspace_hack_file_over_the_limit_fails() {
        let root = tempfile::TempDir::new().expect("tempdir");
        write_wired(root.path());
        let lib = root.path().join("crates/workspace-hack/src/lib.rs");
        std::fs::write(lib, "// line\n".repeat(501)).expect("lib.rs writes");
        let violations = ceiling_wiring_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("workspace-hack") && violations[0].contains("501 lines"));
    }

    #[test]
    fn a_wiring_scan_that_finds_no_crate_fails() {
        let root = tempfile::TempDir::new().expect("tempdir");
        std::fs::create_dir_all(root.path().join("crates")).expect("the crates directory creates");
        let violations = ceiling_wiring_violations(root.path());
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("scanned nothing"));
    }
}
