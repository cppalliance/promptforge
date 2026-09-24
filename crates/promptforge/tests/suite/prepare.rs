//! Prepare-pass integration tests: host-file claims through a shared base
//! VFS, slot filling by identity against the host-supplied catalog, and
//! model satisfaction - the trivial fill binding every declared role to
//! the context's current model, and the hard-keyword and context-minimum
//! checks against its descriptor. The store-mount isolation case and the
//! prepare-run refusals drive the engine's test host, so they sit in the
//! engine's own suite.
//!
//! Capability activation - resolving a prompt's declarations against a
//! registry, conflict checking, and catalog assembly - is the harness's,
//! and its suite lives with it in `harness-capabilities`; the engine's
//! prepare only ever sees the catalog the host hands it.

use std::num::NonZeroU32;

use promptforge::capabilities::CapabilityId;
use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::vfs::{HostBackend, Origin, VfsError, VfsRef};
use promptforge::{Environment, Prompt, RequirementCheck};

use super::support::context;

/// A prompt declaring no capabilities at all.
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
            "promptforge-api-prepare-{}-{unique}-{name}",
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
fn two_runs_writing_the_same_host_file_through_the_shared_base_conflict() {
    let temp = TempDir::new("shared-base");
    let base = VfsRef::builder()
        .mount(
            "/",
            HostBackend::rooted(&temp.0).expect("the temp dir roots the host backend"),
        )
        .build();
    let env = Environment::new().base_vfs(base);
    let prompt = parse(DECLARES_NOTHING, "declares-nothing");
    let (ctx_a, _) = env.prepare(&prompt, context("run-a"));
    let (ctx_b, _) = env.prepare(&prompt, context("run-b"));
    let access_a = ctx_a
        .vfs_handle()
        .acquire(Origin::new("run-a"))
        .expect("run a acquires");
    access_a
        .write("/shared.txt", b"from a")
        .expect("run a writes the host file");
    let access_b = ctx_b
        .vfs_handle()
        .acquire(Origin::new("run-b"))
        .expect("run b acquires");
    // The shared base's claims table sees two live identities on one path.
    let error = access_b
        .write("/shared.txt", b"from b")
        .expect_err("run b conflicts with run a's live claim");
    assert!(
        matches!(error, VfsError::Conflict(_)),
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

/// Builds the host's one current model with the given context window and
/// thinking capability.
fn current_model(context: u32, thinking: ThinkingMode) -> ModelDescriptor {
    ModelDescriptor::new(
        ModelId::gateway("current").expect("the id is valid"),
        "The host's current model",
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

// ToolBindings and slot filling: exact slots fill by identity against
// the host-supplied catalog (an exact path's first two segments name its
// capability, so a slot whose capability contributed nothing to the
// catalog is reported as missing), with every fill journaled into the
// run's tool bindings as descriptors, never implementations.

/// A prompt declaring `promptforge/web` and one exact tool slot.
const DECLARES_EXACT_SLOT: &str = concat!(
    "---\n",
    "name: declares-exact-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring one tool slot whose capability is not declared at
/// all.
const DECLARES_ORPHAN_SLOT: &str = concat!(
    "---\n",
    "name: declares-orphan-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A host-supplied descriptor for one `promptforge/web` tool.
fn web_descriptor(id: &str, description: &str) -> ToolDescriptor {
    let id = ToolId::parse(id).expect("the fixture tool id is valid");
    ToolDescriptor::new(
        id.clone(),
        id.name(),
        description,
        serde_json::json!({"type": "object", "properties": {}}),
    )
}

/// The step's first test: `prepare` fills a slot by identity against a
/// catalog the host supplied directly - no registry, no activation, no
/// implementation anywhere near the engine - and the binding journals the
/// descriptor's data.
#[test]
fn prepare_fills_a_slot_by_id_against_a_host_supplied_catalog() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    let id = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let descriptor = ToolDescriptor::new(
        id.clone(),
        "fetch",
        "Fetch a web page over HTTP",
        serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
    )
    .structured(true);
    let catalog = ToolCatalog::new(std::slice::from_ref(&descriptor)).expect("the catalog builds");
    let env = Environment::new().tools(catalog);
    let (ctx, requirements) = env.prepare(&prompt, context("fill-by-id"));
    assert!(
        requirements.is_satisfied(),
        "a slot the catalog satisfies reports nothing: {requirements:?}"
    );
    let bindings = ctx.tool_bindings();
    assert_eq!(bindings.len(), 1);
    // Handles resolve alias -> id -> descriptor, and the journaled
    // descriptor is the catalog's entry verbatim.
    assert_eq!(bindings.alias_id("fetch"), Some(&id));
    assert_eq!(bindings.resolve("fetch"), Some(&descriptor));
    assert_eq!(bindings.tool(&id), Some(&descriptor));
    assert!(bindings.resolve("undeclared").is_none());
    // The context's catalog is the environment's, so the host can read
    // back what the run was prepared against.
    assert_eq!(ctx.tools().tools(), [descriptor]);
}

#[test]
fn an_exact_slot_whose_capability_is_inactive_is_reported() {
    let prompt = parse(DECLARES_ORPHAN_SLOT, "declares-orphan-slot");
    // An empty catalog and no declaration: the slot's capability
    // contributed nothing the engine can fill against.
    let (ctx, requirements) = Environment::new().prepare(&prompt, context("fill-orphan"));
    // The exact path's first two segments name its capability.
    assert_eq!(
        requirements.missing_required,
        [CapabilityId::parse("promptforge/web").expect("the id is valid")]
    );
    assert!(!requirements.is_satisfied());
    assert!(ctx.tool_bindings().is_empty());
}

#[test]
fn an_exact_slot_absent_from_an_active_capability_is_not_reported_missing() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    // The capability is present in the catalog but contributed a different
    // tool: the slot's capability is not missing, so the run must not fail
    // unsatisfiably - installing changes nothing.
    let catalog = ToolCatalog::new(&[web_descriptor("promptforge/web/search", "Search the web")])
        .expect("the catalog builds");
    let env = Environment::new().tools(catalog);
    let (ctx, requirements) = env.prepare(&prompt, context("fill-absent-tool"));
    assert!(
        requirements.missing_required.is_empty(),
        "an active capability is never reported missing: {:?}",
        requirements.missing_required
    );
    assert!(requirements.is_satisfied());
    // The alias stays unbound; advertising it fails at run time with the
    // alias named. The engine reaches no logger, so nothing else records it.
    assert!(ctx.tool_bindings().is_empty());
}
