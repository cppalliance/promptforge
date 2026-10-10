//! Prepare-pass integration tests: real-file claims through a shared base
//! VFS, the caller-supplied catalog copied onto the context, and model
//! satisfaction - the trivial fill binding every declared role to
//! the context's current model, and the hard-keyword and context-minimum
//! checks against its descriptor. The store-mount isolation case and the
//! prepare-run refusals drive the run fixture in `promptforge-engine`'s
//! test support, so they sit in that crate's own suite.
//!
//! Plugin install and each run's snapshot - resolving a prompt's
//! declarations against the installed Plugins and catalog assembly - are
//! the Harness's job, not the Engine's; the Engine's prepare only ever
//! sees the catalog its caller hands it.

use std::num::NonZeroU32;

use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::vfs::{Origin, RealBackend, VfsError, VfsRef};
use promptforge::{Environment, Prompt, RequirementCheck};

use super::support::context;

/// A prompt declaring no Plugins at all.
const DECLARES_NOTHING: &str = concat!(
    "---\n",
    "name: declares-nothing\n",
    "description: d\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// Parses a fixture prompt.
fn parse(source: &str, execution: &str) -> Prompt {
    Prompt::parse(source, execution)
        .0
        .expect("the fixture prompt parses")
}

/// A unique temporary directory that removes itself on drop. The suite has
/// no tempfile dependency; this mirrors promptforge-vfs's own test helper.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "promptforge-prepare-{}-{unique}-{name}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("the temp dir creates");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn two_runs_writing_the_same_real_file_through_the_shared_base_conflict() {
    let temp = TempDir::new("shared-base");
    // The caller's handle: a real base at `/` beside a declared store at
    // `/my/store`, shared by every run.
    let vfs = VfsRef::builder()
        .mount(
            "/",
            RealBackend::rooted(&temp.0).expect("the temp dir roots the real backend"),
        )
        .store("/my/store", promptforge::vfs::MemoryBackend::new())
        .build();
    let env = Environment::new();
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let (ctx_a, _) = env.prepare(&prompt, context("run-a").vfs(vfs.clone()));
    let (ctx_b, _) = env.prepare(&prompt, context("run-b").vfs(vfs));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    access_a
        .write("/shared.txt", b"from a")
        .expect("run a writes the real file");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    // The shared base's claims table sees two live identities on one path.
    let error = access_b
        .write("/shared.txt", b"from b")
        .expect_err("run b conflicts with run a's live claim");
    assert!(
        matches!(error, VfsError::Conflict { .. }),
        "a determinism violation, not a backend error: {error}"
    );
    // The conflicting write never partially applied.
    assert_eq!(
        std::fs::read(temp.0.join("shared.txt")).expect("run a's write landed on disk"),
        b"from a"
    );
}

/// A prompt declaring one model role with a hard keyword and a context
/// minimum.
const DECLARES_ANALYST: &str = concat!(
    "---\n",
    "name: declares-analyst\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  analyst:\n",
    "    keywords: [frontier, thinking]\n",
    "    min_context: 200000\n",
    "    description: Deep analysis\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// A prompt declaring one role per soft keyword: documentation of author
/// intent, never a check.
const DECLARES_SOFT_ROLES: &str = concat!(
    "---\n",
    "name: declares-soft-roles\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  scout:\n",
    "    keywords: [frontier, fast]\n",
    "  sprinter:\n",
    "    keywords: [small, creative, chat]\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// A prompt declaring one role with the `no-thinking` hard keyword.
const DECLARES_NO_THINKING: &str = concat!(
    "---\n",
    "name: declares-no-thinking\n",
    "description: d\n",
    "promptforge: 0\n",
    "models:\n",
    "  triage:\n",
    "    keywords: [no-thinking]\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "```lua\n",
    "return 'done'\n",
    "```\n",
);

/// Builds the context's one current model with the given context window
/// and thinking capability.
fn current_model(context: u32, thinking: ThinkingMode) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("current").expect("the id is valid"),
        "The current model",
        NonZeroU32::new(context).expect("the context window is non-zero"),
        thinking,
    )
}

#[test]
fn every_declared_role_resolves_to_the_current_model() {
    let prompt = parse(DECLARES_SOFT_ROLES, "declares-soft-roles");
    let env = Environment::new();
    let model = current_model(32_000, ThinkingMode::Never);
    let (ctx, requirements) = env.prepare(&prompt, context("fill").model(model.clone()));
    // Soft keywords document intent and the roles declare no minimum:
    // nothing is reported.
    assert!(requirements.is_satisfied());
    // The trivial fill binds every declared role to the current model,
    // and handles resolve label -> id -> descriptor.
    let bindings = ctx.model_bindings();
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings.role_id("scout"), Some(model.id()));
    assert_eq!(bindings.resolve("scout"), Some(&model));
    assert_eq!(bindings.resolve("sprinter"), Some(&model));
    assert_eq!(bindings.model(model.id()), Some(&model));
    assert!(bindings.resolve("undeclared").is_none());
}

#[test]
fn a_context_minimum_above_the_current_models_is_reported() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    // min_context 200000 against the model's 32000.
    let (_ctx, requirements) = env.prepare(
        &prompt,
        context("fill").model(current_model(32_000, ThinkingMode::Always)),
    );
    assert!(!requirements.is_satisfied());
    assert_eq!(requirements.missing_required, []);
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        panic!(
            "exactly one requirement is unmet: {:?}",
            requirements.unmet_requirements
        );
    };
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::ContextMinimum);
    assert_eq!(unmet.required, "200000");
    assert_eq!(unmet.actual, "32000");
}

#[test]
fn a_hard_keyword_the_current_model_fails_is_reported() {
    let prompt = parse(DECLARES_ANALYST, "declares-analyst");
    let env = Environment::new();
    // `thinking` against a Never model, with the context minimum met so
    // only the keyword check fires.
    let (_ctx, requirements) = env.prepare(
        &prompt,
        context("fill").model(current_model(200_000, ThinkingMode::Never)),
    );
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        panic!(
            "exactly one requirement is unmet: {:?}",
            requirements.unmet_requirements
        );
    };
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "thinking");
    assert_eq!(unmet.actual, "Never");

    let prompt = parse(DECLARES_NO_THINKING, "declares-no-thinking");
    // `no-thinking` against an Always model.
    let (_ctx, requirements) = env.prepare(
        &prompt,
        context("fill").model(current_model(32_000, ThinkingMode::Always)),
    );
    let [unmet] = requirements.unmet_requirements.as_slice() else {
        panic!(
            "exactly one requirement is unmet: {:?}",
            requirements.unmet_requirements
        );
    };
    assert_eq!(unmet.role, "triage");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "no-thinking");
    assert_eq!(unmet.actual, "Always");
}

// The catalog: prepare copies the caller-supplied catalog onto the context
// verbatim, descriptors and never implementations, and reports nothing
// about tools. The run offers every catalog tool to the prompt's Lua by
// id, and whether a declared Plugin is installed is the Harness's report.

/// A prompt declaring `web`.
const DECLARES_WEB: &str = concat!(
    "---\n",
    "name: declares-web\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - web\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

#[test]
fn prepare_copies_the_caller_supplied_catalog_onto_the_context() {
    let prompt = parse(DECLARES_WEB, "declares-web");
    let descriptor = ToolDescriptor::new(
        ToolId::parse("web/fetch").expect("the id is valid"),
        "Fetch a web page over HTTP",
        serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
    )
    .structured(true);
    let catalog = ToolCatalog::new(std::slice::from_ref(&descriptor)).expect("the catalog builds");
    let (ctx, requirements) = Environment::new()
        .tools(catalog)
        .prepare(&prompt, context("catalog"));
    assert!(
        requirements.is_satisfied(),
        "a catalog reports nothing: {requirements:?}"
    );
    assert_eq!(ctx.tools().tools(), [descriptor]);
}

#[test]
fn prepare_reports_nothing_for_a_declared_plugin_the_catalog_lacks() {
    let prompt = parse(DECLARES_WEB, "declares-web");
    let other = ToolDescriptor::new(
        ToolId::parse("other/search").expect("the id is valid"),
        "Search somewhere else",
        serde_json::json!({"type": "object", "properties": {}}),
    );
    let catalog = ToolCatalog::new(std::slice::from_ref(&other)).expect("the catalog builds");
    let (ctx, requirements) = Environment::new()
        .tools(catalog)
        .prepare(&prompt, context("no-web"));
    assert!(
        requirements.is_satisfied(),
        "the Engine's prepare checks model roles only: {requirements:?}"
    );
    assert_eq!(ctx.tools().tools(), [other]);
}
