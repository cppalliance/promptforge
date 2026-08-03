//! The temporary prompts directory both halves of the watcher are tested over.
//!
//! It lives beside the two test modules rather than inside either, because the
//! window's tests and the reload's tests need the same written configuration and
//! neither owns it.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::Config;
use crate::retrieval::Retrieval;
use crate::watch::reload::{Reload, Reloader};
use crate::watch::sessions::ListChanged;

/// A listener that counts announcements instead of sending them.
#[derive(Debug, Default)]
pub(super) struct Recorder {
    /// How many announcements have been made.
    announced: AtomicUsize,
}

impl Recorder {
    /// How many announcements have been made.
    pub(super) fn announced(&self) -> usize {
        self.announced.load(Ordering::Relaxed)
    }
}

impl ListChanged for Recorder {
    fn list_changed(&self) {
        let _previous = self.announced.fetch_add(1, Ordering::Relaxed);
    }
}

/// A prompt whose Lua returns at once, so it needs no gateway.
pub(super) fn prompt(name: &str, description: &str, value: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {description}\nversion: 1\npromptforge: 1\n---\n\n\
         ## Main\n\n```lua\nreturn '{value}'\n```\n"
    )
}

/// A prompt file that declares a `promptforge:` version - so it is a prompt -
/// but whose frontmatter is missing a required field, so it cannot be validated.
pub(super) fn unparsable() -> &'static str {
    "---\npromptforge: 1\n---\n\n## Main\n\nprose\n"
}

/// A `prompts.toml` body over `prompts/`, with `extra` appended verbatim.
pub(super) fn config_source(root: &Path, extra: &str) -> String {
    format!(
        "[server]\ntoken = \"shared\"\n{extra}\n\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\ndefault_expose = \"list\"\n",
        root.join("prompts").display()
    )
}

/// Everything one reload test needs: a written configuration, two prompts, the
/// live catalog, and the recorder the reload announces to.
pub(super) struct Fixture {
    /// The temporary root, held so it outlives the test.
    dir: TempDir,
    /// The configuration boot read, as a live server holds it.
    pub(super) config: Arc<Config>,
    /// The reload under test.
    reloader: Reloader,
    /// The catalog a reload swaps.
    pub(super) catalog: Arc<CatalogHandle>,
    /// What the reload announced to.
    pub(super) recorder: Arc<Recorder>,
}

impl Fixture {
    /// Two listed prompts, `alpha` and `beta`, resolved as boot would, with no
    /// retrieval index behind them.
    pub(super) fn new() -> Fixture {
        Fixture::with_retrieval(Arc::new(Retrieval::idle()))
    }

    /// The same fixture over a retrieval index a reload can rebuild.
    pub(super) fn with_retrieval(retrieval: Arc<Retrieval>) -> Fixture {
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
        let catalog = Arc::new(CatalogHandle::new(catalog));
        let recorder = Arc::new(Recorder::default());
        let listener: Arc<dyn ListChanged> = recorder.clone();
        let config = Arc::new(config);
        let reloader = Reloader::new(
            &source,
            Arc::clone(&config),
            Arc::clone(&catalog),
            listener,
            retrieval,
        );
        Fixture {
            dir,
            config,
            reloader,
            catalog,
            recorder,
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
    pub(super) fn reload(&self) -> Reload {
        self.reloader.reload()
    }

    /// A named entry's description in the live catalog.
    pub(super) fn description(&self, name: &str) -> String {
        self.catalog
            .load()
            .find(name)
            .expect("the entry is in the live catalog")
            .description()
            .to_owned()
    }
}
