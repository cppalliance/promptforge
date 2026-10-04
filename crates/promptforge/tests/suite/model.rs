//! Describing models and answering their rounds: a catalog looks a model
//! up by id and refuses a repeated id as a whole, and a chat effect's
//! answer becomes the result or, as a failed round, a completion failure.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionResult, ModelCatalog,
    ModelCatalogError, ModelDescriptor, ModelId, ThinkingMode,
};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunErrorKind, RunResult, Step};

/// Declares `writer` and `checker`, and asks `writer` once for a reply to
/// its note.
const GREETER: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note and asks a model to reply to it.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "  checker: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "return models.infer(store.read('note.md'))\n",
    "```\n",
);

/// `gateway/fast`: a 32000-token window on a backend that never thinks.
fn fast() -> Result<ModelDescriptor, Box<dyn Error>> {
    Ok(ModelDescriptor::new(
        ModelId::gateway("fast")?,
        "Quick replies for the greeter",
        NonZeroU32::new(32_000).ok_or("a context window is never zero")?,
        ThinkingMode::Never,
    ))
}

/// Prepares [`GREETER`] with `model` and runs it, answering the store from
/// the run's store and each chat effect with what `chat` returns for the
/// bound model's name.
fn run_greeter(
    model: &ModelDescriptor,
    chat: impl Fn(&str) -> Result<Box<Completion>, CompletionError>,
) -> Result<RunResult, Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(GREETER, "greeter");
    let prompt = parsed?;
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model.clone());
    let (ctx, _requirements) = Environment::new().prepare(&prompt, ctx);
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    loop {
        let effects = match run.step() {
            Step::Pending { effects, .. } => effects,
            Step::Done { result, .. } => return Ok(result),
        };
        for (id, _provenance, effect) in effects {
            let answer = match effect {
                Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
                Effect::Chat { binding, .. } => EffectAnswer::Chat(chat(binding.id().name())),
                Effect::ToolCall { .. } | Effect::Timer { .. } => EffectAnswer::Dropped,
            };
            run.resume(id, answer);
        }
    }
}

#[test]
fn a_catalog_finds_a_model_by_id_and_refuses_a_repeated_id_as_a_whole() -> Result<(), Box<dyn Error>>
{
    let fast = fast()?;
    let deep = ModelDescriptor::new(
        ModelId::new(ModelId::GATEWAY, "deep")?,
        "Careful replies for the greeter",
        NonZeroU32::new(200_000).ok_or("a context window is never zero")?,
        ThinkingMode::Always,
    );

    let catalog = ModelCatalog::new([fast.clone(), deep])?;
    assert_eq!(catalog.models().len(), 2);
    let found = catalog
        .get(&ModelId::gateway("deep")?)
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
fn a_text_completion_served_under_the_bound_models_name_becomes_the_result()
-> Result<(), Box<dyn Error>> {
    let result = run_greeter(&fast()?, |served| {
        let reply = CompletionResult::Text("hello world".to_owned());
        Ok(Box::new(Completion::from_result(reply, served)?))
    })?;
    assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));
    Ok(())
}

#[test]
fn a_failed_model_round_fails_the_run_as_a_completion_failure() -> Result<(), Box<dyn Error>> {
    let result = run_greeter(&fast()?, |_served| {
        let kind = CompletionErrorKind::Overloaded;
        Err(CompletionError::new(kind, kind.phrase()))
    })?;
    let RunResult::Failure(error) = result else {
        return Err("a failed round fails the run".into());
    };
    assert_eq!(error.kind(), RunErrorKind::Completion);
    Ok(())
}
