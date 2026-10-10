//! What a Host does around a run: it answers `input.ask()` through its
//! input broker, stops an in-flight round or cancels a run through the
//! Harness's control, and streams a section round from its own broker
//! under the round id the run records.

use std::error::Error;
use std::num::NonZeroU32;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Poll, Waker};

use async_trait::async_trait;
use harness::plugin::{HostContext, HostServices};
use harness::record::{MemoryRecorder, RecordKind, RunOutcome};
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, RunRequest};
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, CompletionResult, Message, ModelBinding,
    ModelCatalog, ModelDescriptor, ModelId, ThinkingMode, ToolSchema,
};
use promptforge_plugin::ToolError;

use crate::asker::{AskBroker, asker_host, broker_services};
use crate::support::{Clock, Offline};

/// Asks the operator once and returns the answer.
const ASKS: &str = concat!(
    "---\nname: asks\ndescription: Asks the operator once\npromptforge: 0\n",
    "plugins:\n  - user-input\n",
    "---\n\n# Asks\n\n## Only\n\n```lua\n",
    "return input.ask()\n",
    "```\n",
);

/// Waits on one model round under a `pcall`, and returns the error kind
/// when the round fails.
const PATIENT: &str = concat!(
    "---\nname: patient\ndescription: Waits on the model\npromptforge: 0\n",
    "models: { writer: {} }\n",
    "---\n\n# Patient\n\n## Only\n\n```lua\n",
    "local ok, err = pcall(writer.infer, writer, 'Take your time.')\n",
    "return ok and 'answered' or err.kind\n",
    "```\n",
);

/// Runs one section round and returns the model's reply.
const CHATS: &str = concat!(
    "---\nname: chats\ndescription: Says hello to the model\npromptforge: 0\n",
    "models: { writer: {} }\n",
    "---\n\n# Chats\n\n## Only\n\n```lua\n",
    "local msgs = messages.new()\n",
    "msgs:user('Hello, desk.')\n",
    "writer:loop(msgs)\n",
    "return msgs[#msgs].content\n",
    "```\n",
);

/// A request for the run `name` over `source`, with no arguments, no
/// input text, and a fresh store.
fn request(name: &str, source: &str) -> RunRequest {
    RunRequest {
        name: name.to_owned(),
        source: source.to_owned(),
        args: String::new(),
        input_text: None,
        vfs: VfsRef::default(),
        host: HostSnapshot::default(),
    }
}

/// The one stub model the stuck and streaming brokers list.
fn stub_catalog() -> Result<ModelCatalog, Box<dyn Error>> {
    let window = NonZeroU32::new(131_072).ok_or("131072 is non-zero")?;
    let model = ModelDescriptor::new(
        ModelId::gateway("stub-model")?,
        "stub",
        window,
        ThinkingMode::Never,
    );
    Ok(ModelCatalog::new([model])?)
}

/// The Host's input broker: the operator always types the same text.
struct Operator(&'static str);

#[async_trait]
impl AskBroker for Operator {
    async fn wait(&self) -> Result<String, ToolError> {
        Ok(self.0.to_owned())
    }
}

/// Raised once a round reaches the stuck broker, waking whoever waits.
struct Started(Mutex<(bool, Option<Waker>)>);

impl Started {
    fn raise(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 = true;
        if let Some(waker) = state.1.take() {
            waker.wake();
        }
    }

    async fn wait(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if state.0 {
                return Poll::Ready(());
            }
            state.1 = Some(cx.waker().clone());
            Poll::Pending
        })
        .await;
    }
}

/// Lists the stub model and never answers a round.
struct Stuck {
    catalog: ModelCatalog,
    started: Arc<Started>,
}

impl InferenceBroker for Stuck {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { Ok(catalog) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.started.raise();
        Box::pin(std::future::pending())
    }
}

/// A Harness over a stuck broker listing `catalog`, beside the flag its
/// first round raises.
fn stuck_harness(catalog: ModelCatalog) -> (Harness, Arc<Started>) {
    let started = Arc::new(Started(Mutex::new((false, None))));
    let broker = Stuck {
        catalog,
        started: Arc::clone(&started),
    };
    let harness = Harness::new(
        Arc::new(MemoryRecorder::new()),
        Arc::new(broker),
        Arc::new(Clock),
        Arc::new(HostContext::new(HostServices::new())),
        HostServices::new(),
    );
    (harness, started)
}

/// Lists the stub model, and replies `You said: <last message>` in two
/// pieces, sending each piece of a section's own round to `window` under
/// its round id.
struct Streaming {
    catalog: ModelCatalog,
    window: Sender<(u64, String)>,
}

impl InferenceBroker for Streaming {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { Ok(catalog) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        let window = (round.origin == ReplyOrigin::Chat).then(|| self.window.clone());
        let said = messages
            .last()
            .map(|message| message.content().to_owned())
            .unwrap_or_default();
        Box::pin(async move {
            let mut reply = String::new();
            for piece in ["You said: ".to_owned(), said] {
                if let Some(window) = &window {
                    // The test holds the receiver until the run ends, so a
                    // send fails only once no one is left to read the piece.
                    let _ = window.send((round.id.get(), piece.clone()));
                }
                reply.push_str(&piece);
            }
            Completion::from_result(CompletionResult::Text(reply), "stub-model").map(Box::new)
        })
    }
}

#[tokio::test]
async fn input_ask_returns_what_the_hosts_input_broker_answers_byte_for_byte() {
    let harness = Harness::new(
        Arc::new(MemoryRecorder::new()),
        Arc::new(Offline),
        Arc::new(Clock),
        asker_host(),
        broker_services(Arc::new(Operator("Hello, desk."))),
    );

    let report = harness
        .run(request("desk-asks-1", ASKS))
        .await
        .expect("the run reaches an outcome");
    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "Hello, desk.".to_owned()
        }
    );
}

#[tokio::test]
async fn stopping_an_in_flight_round_drops_the_call_and_the_prompts_pcall_catches_it() {
    let (harness, round_started) = stuck_harness(stub_catalog().expect("the stub catalog builds"));
    let control = harness.control();
    let (report, ()) = tokio::join!(harness.run(request("desk-patient", PATIENT)), async {
        round_started.wait().await;
        control.stop_round();
    });
    assert_eq!(
        report.expect("the run reaches an outcome").outcome,
        RunOutcome::Completed {
            final_text: "cancelled".to_owned()
        }
    );
}

#[tokio::test]
async fn a_run_cancelled_before_it_begins_ends_cancelled_and_is_never_recorded() {
    let (harness, _round_started) = stuck_harness(stub_catalog().expect("the stub catalog builds"));
    harness.control().cancel();
    let report = harness
        .run(request("desk-patient", PATIENT))
        .await
        .expect("the run reaches an outcome");
    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert_eq!(report.run_id, None);
}

#[tokio::test]
async fn a_hosts_own_broker_streams_a_section_round_under_the_round_id_the_run_records() {
    let (window, shown) = channel();
    let recorder = Arc::new(MemoryRecorder::new());
    let broker = Streaming {
        catalog: stub_catalog().expect("the stub catalog builds"),
        window,
    };
    let harness = Harness::new(
        recorder.clone(),
        Arc::new(broker),
        Arc::new(Clock),
        Arc::new(HostContext::new(HostServices::new())),
        HostServices::new(),
    );
    let report = harness
        .run(request("desk-chats-1", CHATS))
        .await
        .expect("the run reaches an outcome");

    let records = recorder.records(report.run_id.expect("the run began"));
    let reply = records
        .iter()
        .filter(|record| record.kind == RecordKind::Event)
        .find(|record| record.payload["kind"] == "assistant_reply")
        .expect("the run recorded its reply");
    let round = reply.payload["round"].as_u64().expect("a round id");
    let shown: Vec<(u64, String)> = shown.try_iter().collect();
    assert_eq!(
        shown,
        [
            (round, "You said: ".to_owned()),
            (round, "Hello, desk.".to_owned())
        ]
    );
}
