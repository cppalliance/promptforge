//! That the `prompts.toml` the repository ships resolves the prompts it ships.
//!
//! The two are one artifact: a configuration that parses on its own says nothing
//! about whether the catalog it names is servable, and boot refuses an
//! incomplete catalog. This test is the only thing that keeps the shipped pair
//! honest, since every other catalog test writes its own prompts into a
//! temporary directory.
//!
//! Two things are supplied here that the process supplies at runtime. The
//! `${VAR}` values become literals, because setting an environment variable is
//! `unsafe` under edition 2024 and this workspace forbids unsafe. And the
//! prompts directory becomes absolute, because the shipped path is relative to
//! the working directory the server is started from - the repository root -
//! while a test runs from its own crate.

#![expect(
    clippy::expect_used,
    reason = "test setup panics on failure, which is the desired behavior"
)]

use std::path::{Path, PathBuf};

use promptforge_mcp::{Catalog, Config, Expose, OnBroken};

/// The shipped configuration, read at compile time so a rename breaks the build
/// rather than the test.
const SHIPPED: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../prompts.toml"));

/// The repository root, two levels above this crate.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root exists")
}

/// Replaces every `${VAR}` with a literal, leaving `$$` alone.
///
/// The values themselves are not what is under test - only `[server].token`
/// being non-blank is checked at load - so one placeholder serves for all of
/// them.
fn without_variables(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find("${") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after.find('}').expect("every ${ has its }");
        out.push_str("supplied-by-the-environment");
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// The shipped configuration, pointed at the repository's own prompts.
fn shipped_config() -> Config {
    let mut config = Config::from_toml_str(&without_variables(SHIPPED))
        .expect("the shipped prompts.toml parses");
    let shipped_relative = config.paths.prompts.clone();
    config.paths.prompts = workspace_root().join(&shipped_relative);
    assert!(
        config.paths.prompts.is_dir(),
        "[paths].prompts names the repository's own prompts directory: {}",
        shipped_relative.display()
    );
    config
}

#[test]
fn the_shipped_configuration_resolves_the_shipped_prompts() {
    let config = shipped_config();
    // Boot's rule: any prompt that fails validation is a fault here, so this
    // passing is the same statement as `serve prompts.toml` reaching its
    // transport.
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the shipped catalog resolves");

    let names: Vec<&str> = catalog
        .entries()
        .iter()
        .map(promptforge_mcp::Entry::name)
        .collect();
    assert_eq!(names, ["echo", "greet", "hello", "research_person"]);
    for entry in catalog.entries() {
        assert!(
            entry.problem().is_none() && entry.prompt().is_some(),
            "every shipped prompt is healthy: {} {:?}",
            entry.name(),
            entry.problem()
        );
    }
}

#[test]
fn the_shipped_configuration_promotes_one_prompt_and_lists_the_rest() {
    let config = shipped_config();
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the shipped catalog resolves");

    // The named block is an exception to `default_expose`, which is what makes
    // the file a demonstration of the promotion rather than a description of it.
    let promoted = catalog
        .find("research_person")
        .expect("the promoted prompt resolved");
    assert_eq!(promoted.expose(), Expose::Tool);
    for entry in catalog
        .entries()
        .iter()
        .filter(|e| e.name() != "research_person")
    {
        assert_eq!(
            entry.expose(),
            Expose::List,
            "{} takes default_expose",
            entry.name()
        );
    }
}
