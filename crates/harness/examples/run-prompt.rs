//! A Host runs one prompt through a Harness: it supplies a recorder, an
//! offline inference broker, a tokio timer, and an empty Plugin
//! registry, hands the run the operator's name, and prints the greeting
//! the run leaves at its output file.

use std::sync::Arc;
use std::time::Duration;

use harness::plugin::{HostServices, PluginRegistry};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, RunRequest, Timer};
use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding,
    ModelCatalog, ToolSchema,
};

/// Reads the line in `name.txt` and writes its greeting to `reply.txt`.
const GREET: &str = concat!(
    "---\nname: greet\ndescription: Greets the operator by name\npromptforge: 0\n",
    "input: { path: name.txt, description: The operator's name }\n",
    "output: { path: reply.txt, description: The greeting }\n",
    "---\n\n# Greet\n\n## Answer\n\n```lua\n",
    "store.write('reply.txt', 'Hello, ' .. store.read('name.txt') .. '.')\n",
    "```\n",
);

/// Lists no model and refuses every round, since the prompt asks no model.
struct Offline;

impl InferenceBroker for Offline {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// Sleeps on tokio's timer.
struct Clock;

impl Timer for Clock {
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        // The Engine refuses a timeout that `Duration` can't hold before it
        // issues the effect, so the zero fallback never fires; if it did,
        // waking at once beats never waking.
        let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO);
        Box::pin(tokio::time::sleep(duration))
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Build one Harness for this run from the Host's recorder, broker,
    //    timer, Plugins, and services.
    let recorder = Arc::new(MemoryRecorder::new());
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(Offline),
        Arc::new(Clock),
        PluginRegistry::new(),
        HostServices::new(),
    );

    // 2. Name the run, hand it the prompt's text and the operator's name,
    //    and give it a fresh store.
    let request = RunRequest {
        name: "desk-greet-1".to_owned(),
        source: GREET.to_owned(),
        args: String::new(),
        input_text: Some("desk".to_owned()),
        vfs: VfsRef::default(),
        host: HostSnapshot::default(),
    };

    // 3. Run it to its end; the Harness is spent.
    let report = harness.run(request).await?;

    // 4. The report says how the run ended and what it left at its output
    //    file, and the recorder holds the run's outcome.
    assert!(matches!(report.outcome, RunOutcome::Completed { .. }));
    let run_id = report.run_id.ok_or("the run began")?;
    assert!(recorder.outcome(run_id).is_some());
    let greeting = report.output?;
    assert_eq!(greeting, "Hello, desk.");
    println!("{greeting}");
    Ok(())
}
