//! The prepare pass: real-file claims, the caller's tool catalog, and model
//! roles. The store-mount isolation case and the prepare-run refusals
//! drive the run fixture in `promptforge-engine`'s test support, so they
//! sit in that crate's own suite.

use promptforge::model::ThinkingMode;
use promptforge::tools::{ToolCatalog, ToolDescriptor};
use promptforge::vfs::{MemoryBackend, Origin, RealBackend, VfsError, VfsRef};
use promptforge::{RequirementCheck, Requirements, UnmetRequirement};

use super::support::{
    Prepared, Scenario, TempRoot, TestResult, descriptor, greeter, model, prompt,
};

fn declaring(front: &str) -> String {
    prompt(front, &[("Only", "return 'done'")])
}

fn report(front: &str, context: u32, thinking: ThinkingMode) -> TestResult<Requirements> {
    let prepared = Scenario::default()
        .model(model("current", context, thinking)?)
        .prepare(&declaring(front))?;
    Ok(prepared.requirements)
}

fn only_unmet(requirements: &Requirements) -> TestResult<&UnmetRequirement> {
    match requirements.unmet_requirements.as_slice() {
        [unmet] => Ok(unmet),
        unmet => Err(format!("exactly one requirement is unmet: {unmet:?}").into()),
    }
}

#[test]
fn two_runs_writing_the_same_real_file_through_the_shared_base_conflict() -> TestResult {
    let temp =
        TempRoot::new("two_runs_writing_the_same_real_file_through_the_shared_base_conflict");
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
    let error = access_b
        .write("/shared.txt", b"from b")
        .expect_err("run b conflicts with run a's live claim");
    assert!(
        matches!(error, VfsError::Conflict { .. }),
        "a determinism violation, not a backend error: {error}"
    );
    assert_eq!(std::fs::read(temp.path().join("shared.txt"))?, b"from a");
    Ok(())
}

const ANALYST: &str = "models: { analyst: { keywords: [frontier, thinking], min_context: 200000, description: Deep analysis } }";

#[test]
fn every_declared_role_resolves_to_the_current_model() -> TestResult {
    // Soft keywords document author intent, and are never checked.
    let source = declaring(
        "models: { scout: { keywords: [frontier, fast] }, sprinter: { keywords: [small, creative, chat] } }",
    );
    let model = model("current", 32_000, ThinkingMode::Never)?;
    let Prepared {
        ctx, requirements, ..
    } = Scenario::default().model(model.clone()).prepare(&source)?;
    assert!(requirements.is_satisfied());
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
    // The context minimum is met, so only the keyword check fires.
    let requirements = report(ANALYST, 200_000, ThinkingMode::Never)?;
    let unmet = only_unmet(&requirements)?;
    assert_eq!(unmet.role, "analyst");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "thinking");
    assert_eq!(unmet.actual, "Never");

    let triage = "models: { triage: { keywords: [no-thinking] } }";
    let requirements = report(triage, 32_000, ThinkingMode::Always)?;
    let unmet = only_unmet(&requirements)?;
    assert_eq!(unmet.role, "triage");
    assert_eq!(unmet.check, RequirementCheck::HardKeyword);
    assert_eq!(unmet.required, "no-thinking");
    assert_eq!(unmet.actual, "Always");
    Ok(())
}

fn prepared_over(descriptor: &ToolDescriptor) -> TestResult<Prepared> {
    let catalog = ToolCatalog::new(std::slice::from_ref(descriptor))?;
    Scenario::default()
        .context(move |ctx| ctx.tools(catalog.clone()))
        .prepare(&declaring("plugins: [web]"))
}

#[test]
fn a_prepared_context_keeps_the_caller_supplied_catalog() -> TestResult {
    let descriptor = descriptor(
        "web/fetch",
        serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
    )?
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
    let other = descriptor(
        "other/search",
        serde_json::json!({"type": "object", "properties": {}}),
    )?;
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
