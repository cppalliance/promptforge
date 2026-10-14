//! Describing models and answering their rounds: a catalog looks a model
//! up by id and refuses a repeated id as a whole, and a failed chat round
//! fails the run as a completion failure.

use promptforge::model::{
    CompletionErrorKind, ModelCatalog, ModelCatalogError, ModelId, ThinkingMode,
};
use promptforge::{RunErrorKind, RunResult};

use super::support::{Reply, Scenario, TestResult, fail, model, prompt};

/// Declares `writer` and `checker`, and asks `writer` once for a reply to
/// its note.
fn asking() -> String {
    let lua = "
        store.write('note.md', 'hello')
        models.use('writer')
        return models.infer(store.read('note.md'))
    ";
    prompt("models: { writer: {}, checker: {} }", &[("Greet", lua)])
}

/// Runs [`asking`] with `gateway/fast` as the current model, answering its
/// one chat round with `reply` under the bound model's name.
fn run_asking(reply: Reply) -> TestResult<RunResult> {
    let scenario = Scenario::replies([reply]).model(model("fast", 32_000, ThinkingMode::Never)?);
    Ok(scenario.run(&asking())?.result)
}

#[test]
fn a_catalog_finds_a_model_by_id_and_refuses_a_repeated_id_as_a_whole() -> TestResult {
    let fast = model("fast", 32_000, ThinkingMode::Never)?;
    let deep = model("deep", 200_000, ThinkingMode::Always)?;

    let catalog = ModelCatalog::new([fast.clone(), deep])?;
    assert_eq!(catalog.models().len(), 2);
    let found = catalog
        .get(&ModelId::new(ModelId::GATEWAY, "deep")?)
        .ok_or("gateway/deep is in the catalog")?;
    assert_eq!(found.context().get(), 200_000);
    assert_eq!(found.thinking(), ThinkingMode::Always);

    let Err(ModelCatalogError::DuplicateId { server, name, .. }) =
        ModelCatalog::new([fast.clone(), fast])
    else {
        return Err("a repeated id is refused with DuplicateId".into());
    };
    assert_eq!((server.as_str(), name.as_str()), ("gateway", "fast"));
    Ok(())
}

#[test]
fn a_failed_model_round_fails_the_run_as_a_completion_failure() -> TestResult {
    let RunResult::Failure(error) = run_asking(fail(CompletionErrorKind::Overloaded))? else {
        return Err("a failed round fails the run".into());
    };
    assert_eq!(error.kind(), RunErrorKind::Completion);
    Ok(())
}
