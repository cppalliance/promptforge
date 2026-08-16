//! The temporary prompts directory both halves of the watcher are tested over.
//!
//! It lives beside the two test modules rather than inside either, because the
//! window's tests and the reload's tests need the same written configuration and
//! neither owns it.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::Config;
use crate::retrieval::Retrieval;
use crate::watch::reload::{Reload, ReloadError, Reloader};

/// A prompt whose Lua returns at once, so it needs no gateway.
///
/// `name` and `description` land in YAML frontmatter and `value` in a Lua string
/// literal, so each is serialized rather than interpolated raw: a quote, a
/// newline, or a YAML/Lua metacharacter in any of them would otherwise terminate
/// the scalar or spill onto the next line and produce malformed fixture content.
pub(super) fn prompt(name: &str, description: &str, value: &str) -> String {
    let name = yaml_scalar(name);
    let description = yaml_scalar(description);
    let value = lua_string(value);
    format!(
        "---\nname: {name}\ndescription: {description}\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn {value}\n```\n"
    )
}

/// A YAML double-quoted scalar that safely carries arbitrary content.
///
/// A JSON string is a valid YAML double-quoted scalar and escapes every
/// character - quotes, backslashes, newlines, control characters - that would
/// otherwise break the frontmatter, so serializing through JSON is a real
/// quoted-and-escaped scalar rather than a raw plain one.
fn yaml_scalar(value: &str) -> String {
    serde_json::to_string(value).expect("a string serializes as JSON")
}

/// A Lua double-quoted string literal that safely carries arbitrary content, so
/// a value with a quote, a backslash, or a newline stays one string rather than
/// terminating the literal or running past the end of the line.
fn lua_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// A prompt file that declares a `promptforge:` version - so it is a prompt -
/// but whose frontmatter is missing a required field, so it cannot be validated.
pub(super) fn unparsable() -> &'static str {
    "---\npromptforge: 1\n---\n\n# Test prompt\n\n## Main\n\nprose\n"
}

/// A `prompts.toml` body over `prompts/`, with `extra` appended verbatim.
pub(super) fn config_source(root: &Path, extra: &str) -> String {
    // Encode the prompts directory as a TOML string value rather than
    // interpolating its raw display form into a literal string: a Windows path's
    // backslashes, or an apostrophe anywhere in the temporary root, would
    // otherwise corrupt the document.
    let prompts = toml::Value::from(root.join("prompts").display().to_string());
    format!(
        "[server]\napi_key = \"shared\"\n{extra}\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"gw\"\n\n\
         [paths]\nprompts = {prompts}\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
    )
}

/// Everything one reload test needs: a written configuration, two prompts, and
/// the live catalog.
pub(super) struct Fixture {
    /// The temporary root, held so it outlives the test.
    dir: TempDir,
    /// The configuration boot read, as a live server holds it.
    pub(super) config: Arc<Config>,
    /// The reload under test.
    reloader: Reloader,
    /// The catalog a reload swaps.
    pub(super) catalog: Arc<CatalogHandle>,
}

impl Fixture {
    /// Two listed prompts, `alpha` and `beta`, resolved as boot would, with no
    /// retrieval index behind them.
    pub(super) fn new() -> Fixture {
        Fixture::with_retrieval(|_catalog| Retrieval::idle())
    }

    /// The same fixture over a retrieval index built from the resolved catalog,
    /// so the generation the handle publishes carries both.
    pub(super) fn with_retrieval(build: impl FnOnce(&Catalog) -> Retrieval) -> Fixture {
        let dir = tempfile::tempdir().expect("create a temporary root");
        let root = dir.path();
        fs::create_dir_all(root.join("prompts")).expect("create the prompts directory");
        Fixture::write_prompt(root, "alpha", "Do the alpha thing", "alpha v1");
        Fixture::write_prompt(root, "beta", "Do the beta thing", "beta v1");
        fs::write(root.join("prompts.toml"), config_source(root, ""))
            .expect("write the configuration");

        let source = root.join("prompts.toml");
        let config = Config::load(&source).expect("the fixture configuration loads");
        let catalog = Catalog::resolve(&config, OnBroken::Reject).expect("boot resolves");
        let retrieval = build(&catalog);
        let catalog = Arc::new(CatalogHandle::with_retrieval(catalog, retrieval));
        let config = Arc::new(config);
        let reloader = Reloader::new(&source, Arc::clone(&config), Arc::clone(&catalog));
        Fixture {
            dir,
            config,
            reloader,
            catalog,
        }
    }

    /// Writes a prompt file named after the prompt.
    pub(super) fn write_prompt(root: &Path, name: &str, description: &str, value: &str) {
        fs::write(
            root.join("prompts").join(format!("{name}.md")),
            prompt(name, description, value),
        )
        .expect("write the fixture prompt");
    }

    /// The temporary root.
    pub(super) fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Replaces one prompt's file.
    pub(super) fn rewrite(&self, name: &str, description: &str, value: &str) {
        Fixture::write_prompt(self.root(), name, description, value);
    }

    /// Replaces one prompt's file with one that cannot be validated.
    pub(super) fn break_prompt(&self, name: &str) {
        fs::write(
            self.root().join("prompts").join(format!("{name}.md")),
            unparsable(),
        )
        .expect("break the fixture prompt");
    }

    /// One reload, as the settled window runs it.
    pub(super) fn reload(&self) -> Result<Reload, ReloadError> {
        self.reloader.reload()
    }

    /// The reload under test, so a concurrency test can drive its build and
    /// commit steps apart and prove a stale build is dropped.
    pub(super) fn reloader(&self) -> &Reloader {
        &self.reloader
    }

    /// A named entry's description in the live catalog.
    pub(super) fn description(&self, name: &str) -> String {
        self.catalog
            .load()
            .catalog()
            .find(name)
            .expect("the entry is in the live catalog")
            .description()
            .to_owned()
    }
}
