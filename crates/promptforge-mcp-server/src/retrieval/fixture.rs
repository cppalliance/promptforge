//! One embedding model for the whole test binary, and the catalog it ranks.
//!
//! Loading the model is seconds of CPU and every retrieval test needs one, so it
//! is loaded once behind a [`LazyLock`] and shared. That is sound rather than
//! merely cheap: the engine's own contract is that two indexes over one encoder
//! embed identical text to identical vectors, so sharing cannot change an
//! answer. A test that wants its own model would be measuring the loader, which
//! the engine's own suite already does.

use std::sync::{Arc, LazyLock};

use promptforge_tool_picker::Model;
use tempfile::TempDir;

use crate::catalog::{Catalog, OnBroken};
use crate::config::Config;
use crate::retrieval::Retrieval;

/// The test binary's one loaded model.
static MODEL: LazyLock<Model> =
    LazyLock::new(|| Model::load().expect("the compiled-in retrieval model loads"));

/// The shared model, for a test that indexes something itself.
pub(crate) fn model() -> Model {
    MODEL.clone()
}

/// Retrieval over `catalog`, over the shared model.
pub(crate) fn retrieval(catalog: &Catalog) -> Retrieval {
    let retrieval = Retrieval::idle();
    retrieval.install_with(&model(), catalog);
    retrieval
}

/// A prompt whose Lua returns at once, so it needs no gateway.
pub(crate) fn prompt(name: &str, description: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\npromptforge: 1\n---\n\n\
         # Test prompt\n\n## Main\n\n```lua\nreturn '{name}'\n```\n"
    )
}

/// A resolved catalog and everything a server over it needs.
///
/// The temporary directory is held here because dropping it would remove the
/// files the entries name.
pub(crate) struct Prompts {
    /// The prompts directory, held so it outlives the catalog.
    _dir: TempDir,
    /// The configuration the catalog was resolved from.
    pub(crate) config: Arc<Config>,
    /// The catalog resolved from `config`, every prompt healthy.
    pub(crate) catalog: Catalog,
}

/// A catalog over the given `(name, description)` pairs, resolved as boot would.
pub(crate) fn catalog(prompts: &[(&str, &str)]) -> Prompts {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    for (name, description) in prompts {
        std::fs::write(
            dir.path().join(format!("{name}.md")),
            prompt(name, description),
        )
        .expect("write the fixture prompt");
    }
    let config = Config::from_toml_str(&format!(
        "[server]\ntoken = \"t\"\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\nkey = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    Prompts {
        _dir: dir,
        config: Arc::new(config),
        catalog,
    }
}

/// The six prompts every ranking test is asked to choose between.
///
/// Six rather than three, so "the right prompt came back inside the shortlist" is
/// a claim about ranking and not about the shortlist being as long as the
/// catalog. Each description is written the way a tool author would write one,
/// which is the register the tool's own documentation asks a caller for.
pub(crate) const PROMPTS: &[(&str, &str)] = &[
    (
        "stakeholder_position",
        "Build a stakeholder position report for one entity.",
    ),
    (
        "paper_summary",
        "Summarize a standards paper and list the open questions it leaves.",
    ),
    (
        "meeting_minutes",
        "Extract decisions and action items from a meeting transcript.",
    ),
    (
        "code_review",
        "Review a source file and report each defect with its severity.",
    ),
    (
        "release_notes",
        "Draft release notes from a range of commits.",
    ),
    (
        "translate_text",
        "Translate a document into another language.",
    ),
];
