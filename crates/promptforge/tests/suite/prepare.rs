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
use promptforge::vfs::{MemoryBackend, Origin, RealBackend, VfsError, VfsRef};
use promptforge::{RequirementCheck, Requirements, UnmetRequirement};

use super::support::{Prepared, Scenario, TestResult, greeter, prompt};

/// A prompt with the frontmatter line `front` and one section that
/// returns at once.
fn declaring(front: &str) -> String {
    prompt(front, &[("Only", "return 'done'")])
}

/// Builds the context's one current model with the given context window
/// and thinking capability.
fn current_model(context: u32, thinking: ThinkingMode) -> TestResult<ModelDescriptor> {
    Ok(ModelDescriptor::new(
        ModelId::gateway("current")?,
        "The current model",
        NonZeroU32::new(context).ok_or("a context window is never zero")?,
        thinking,
    ))
}

/// What prepare reports for a prompt declaring `front`, under a current
/// model with the given context window and thinking capability.
fn report(front: &str, context: u32, thinking: ThinkingMode) -> TestResult<Requirements> {
    let model = current_model(context, thinking)?;
    let prepared = Scenario::default()
        .model(model)
        .prepare(&declaring(front))?;
    Ok(prepared.requirements)
}

/// The one requirement `requirements` reports as unmet.
fn only_unmet(requirements: &Requirements) -> TestResult<&UnmetRequirement> {
    match requirements.unmet_requirements.as_slice() {
        [unmet] => Ok(unmet),
        unmet => Err(format!("exactly one requirement is unmet: {unmet:?}").into()),
    }
}

#[test]
fn two_runs_writing_the_same_real_file_through_the_shared_base_conflict() -> TestResult {
    let temp = tempfile::tempdir()?;
    // The caller's handle: a real base at `/` beside a declared store at
    // `/my/store`, shared by every run.
    let vfs = VfsRef::builder()
        .mount("/", RealBackend::rooted(temp.path())?)
        .store("/my/store", MemoryBackend::new())
        .build();
    let scenario = Scenario::default().context(move |ctx| ctx.vfs(vfs.clone()));
    let source = greeter("return 'done'");
    let (run_a, run_b) = (scenario.prepare(&source)?, scenario.prepare(&source)?);
    let access_a = run_a.ctx.vfs_handle().acquire(Origin::new("run-a"))?;
    access_a.write("/shared.txt", b"from a")?;
    let access_b = run_b.ctx.vfs_handle().acquire(Origin::new("run-b"))?;
    // The shared base's claims table sees two live identities on one path.
    let error = access_b
        .write("/shared.txt", b"from b")
        .expect_err("run b conflicts with run a's live claim");
    assert!(
        matches!(error, VfsError::Conflict { .. }),
        "a determinism violation, not a backend error: {error}"
    );
    // The conflicting write never partially applied.
    assert_eq!(std::fs::read(temp.path().join("shared.txt"))?, b"from a");
    Ok(())
}

/// One model role with a hard keyword and a context minimum.
const ANALYST: &str = "models: { analyst: { keywords: [frontier, thinking], min_context: 200000, description: Deep analysis } }";

#[test]
fn every_declared_role_resolves_to_the_current_model() -> TestResult {
    // One role per soft keyword: documentation of author intent, never a
    // check.
    let source = declaring(
        "models: { scout: { keywords: [frontier, fast] }, sprinter: { keywords: [small, creative, chat] } }",
    );
    let model = current_model(32_000, ThinkingMode::Never)?;
    let Prepared {
        ctx, requirements, ..
    } = Scenario::default().model(model.clone()).prepare(&source)?;
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
    Ok(())
}

#[test]
fn a_context_minimum_above_the_current_models_is_reported() -> TestResult {
    // min_context 200000 against the model's 32000.
    let requirements = report(ANALYST, 32_000, ThinkingMode::Always)?;
    assert!(!requirements.is_satisfied());
    assert_eq!(requirements.missing_required, []);
    let unmet = only_unmet(&requirements)?;
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::ContextMinimum);
    assert_eq!(unmet.required, "200000");
    assert_eq!(unmet.actual, "32000");
    Ok(())
}

#[test]
fn a_hard_keyword_the_current_model_fails_is_reported() -> TestResult {
    // `thinking` against a Never model, with the context minimum met so
    // only the keyword check fires.
    let requirements = report(ANALYST, 200_000, ThinkingMode::Never)?;
    let unmet = only_unmet(&requirements)?;
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "thinking");
    assert_eq!(unmet.actual, "Never");

    // `no-thinking` against an Always model.
    let triage = "models: { triage: { keywords: [no-thinking] } }";
    let requirements = report(triage, 32_000, ThinkingMode::Always)?;
    let unmet = only_unmet(&requirements)?;
    assert_eq!(unmet.role, "triage");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "no-thinking");
    assert_eq!(unmet.actual, "Always");
    Ok(())
}

// The catalog: a prepared context keeps the caller-supplied catalog
// verbatim, descriptors and never implementations, and prepare reports
// nothing about tools. The catalog lists every tool a run can use: Lua calls
// one by id, and a section offers one to its model with `tools.offer`.
// Whether a declared Plugin is installed is the Harness's report.

/// Prepares a prompt declaring `web` over a catalog of `descriptor` alone.
fn prepared_over(descriptor: &ToolDescriptor) -> TestResult<Prepared> {
    let catalog = ToolCatalog::new(std::slice::from_ref(descriptor))?;
    Scenario::default()
        .context(move |ctx| ctx.tools(catalog.clone()))
        .prepare(&declaring("plugins: [web]"))
}

#[test]
fn a_prepared_context_keeps_the_caller_supplied_catalog() -> TestResult {
    let descriptor = ToolDescriptor::new(
        ToolId::parse("web/fetch")?,
        "Fetch a web page over HTTP",
        serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
    )
    .structured(true);
    let Prepared {
        ctx, requirements, ..
    } = prepared_over(&descriptor)?;
    assert!(
        requirements.is_satisfied(),
        "a catalog reports nothing: {requirements:?}"
    );
    assert_eq!(ctx.catalog().tools(), [descriptor]);
    Ok(())
}

#[test]
fn prepare_reports_nothing_for_a_declared_plugin_the_catalog_lacks() -> TestResult {
    let other = ToolDescriptor::new(
        ToolId::parse("other/search")?,
        "Search somewhere else",
        serde_json::json!({"type": "object", "properties": {}}),
    );
    let Prepared {
        ctx, requirements, ..
    } = prepared_over(&other)?;
    assert!(
        requirements.is_satisfied(),
        "the Engine's prepare checks model roles only: {requirements:?}"
    );
    assert_eq!(ctx.catalog().tools(), [other]);
    Ok(())
}
