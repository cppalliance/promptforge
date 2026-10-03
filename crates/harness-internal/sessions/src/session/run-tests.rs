//! Tests that run failures pushed to the client keep their cause chain,
//! that the session's broker hands a delta callback only to a section's
//! own round, that each delta is published with the reply stamp of the
//! event that will supersede it, and that a turn-cancel wins over an
//! answer that landed before the run was polled again.

use std::future::poll_fn;
use std::num::NonZeroU32;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use harness_capabilities::{CapabilityRegistry, HostServices};
use harness_runner::performers::{BoxFuture, InferenceBroker, OnDelta};
use harness_runner::recorder::{MemoryRecorder, RecorderError, RunId, RunOutcome};
use harness_runner::{HarnessError, display_chain};
use promptforge::effect::Round;
use promptforge::event::{Event, ReplyOrigin};
use promptforge::ids::{Provenance, RoundId};
use promptforge::model::{
    Completion, CompletionError, CompletionErrorKind, CompletionOptions, CompletionResult, Message,
    ModelBinding, ModelCatalog, ModelDescriptor, ModelId, ModelInvocation, StreamDelta,
    ThinkingMode, ToolSchema,
};
use tokio::sync::{Semaphore, broadcast, mpsc};

use super::{SessionBroker, delta_callback, run_once};
use crate::discovery::AgentSource;
use crate::environment::Bindings;
use crate::lifecycle::{CANCELLATION_CAPACITY, RunLifecycle};
use crate::protocol::{Delta, DeltaKind, SessionId};
use crate::session::files::SessionFiles;
use crate::session::{Session, SessionCore, SessionSeed};

/// Keeps whether each round it was handed came with a delta callback,
/// and refuses the round.
#[derive(Default)]
struct Watching {
    callbacks: Mutex<Vec<(ReplyOrigin, bool)>>,
}

impl InferenceBroker for Watching {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        Box::pin(async { Ok(ModelCatalog::empty()) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        round: Round,
        on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.callbacks
            .lock()
            .unwrap()
            .push((round.origin, on_delta.is_some()));
        let kind = CompletionErrorKind::Unavailable;
        Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
    }
}

/// A prompt whose one section returns its one model round's reply.
const INFERS: &str = "---\nname: infers\ndescription: infers once\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Infers\n\n```lua\nmodels.default('writer')\n```\n\n\
    ## Only\n\n```lua\nreturn models.infer('prose')\n```\n";

/// Lists one model and holds every round until `gate` hands it a permit,
/// counting the rounds it was handed.
struct Gated {
    gate: Arc<Semaphore>,
    rounds: AtomicUsize,
}

impl InferenceBroker for Gated {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        let model = ModelDescriptor::new(
            ModelId::gateway("m").expect("a literal model name is valid"),
            "m",
            NonZeroU32::new(4096).expect("4096 is non-zero"),
            ThinkingMode::Switchable,
        );
        let catalog = ModelCatalog::new([model]).expect("one model is distinct");
        Box::pin(async move { Ok(catalog) })
    }

    fn chat(
        &self,
        _binding: ModelBinding,
        _messages: Vec<Message>,
        _tools: Vec<ToolSchema>,
        _options: CompletionOptions,
        _round: Round,
        _on_delta: Option<OnDelta>,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        self.rounds.fetch_add(1, Ordering::SeqCst);
        let gate = Arc::clone(&self.gate);
        Box::pin(async move {
            let _permit = gate.acquire_owned().await.expect("the gate stays open");
            Completion::from_result(CompletionResult::Text("landed".to_owned()), "m")
                .map(|completion| Box::new(completion.with_finish_reason("stop")))
        })
    }
}

/// A session core over `broker`, with nothing launched behind it.
fn core_over(broker: Arc<dyn InferenceBroker>) -> Arc<SessionCore> {
    core_running(String::new(), broker)
}

/// A session core whose program is `source`, over `broker`.
fn core_running(source: String, broker: Arc<dyn InferenceBroker>) -> Arc<SessionCore> {
    let (events, _events) = mpsc::unbounded_channel();
    let (cancellations, _cancellations) = mpsc::channel(CANCELLATION_CAPACITY);
    SessionCore::new(SessionSeed {
        id: SessionId::fresh(),
        agent: "run-tests".to_owned(),
        source: AgentSource::Markdown(source),
        args: String::new(),
        files: SessionFiles::new(None, None),
        lifecycle: Arc::new(RunLifecycle::new(events, cancellations)),
        recorder: Arc::new(MemoryRecorder::new()),
        broker,
        capabilities: Arc::new(CapabilityRegistry::new()),
        services: HostServices::new(),
    })
}

/// A round's binding; the session's broker passes it through untouched.
fn binding() -> ModelBinding {
    ModelBinding::new(
        "writer",
        "the round's model",
        ModelId::gateway("m").expect("a literal model name is valid"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

/// The reply that settles a round.
fn reply() -> Event {
    Event::AssistantReply {
        execution: "run-tests".to_owned(),
        section: "Only".to_owned(),
        provenance: Provenance {
            task: "0".parse().unwrap(),
            seq: 0,
        },
        turn: 1,
        round: RoundId::new(0),
        text: "settled".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "m".to_owned(),
        metrics: None,
        origin: ReplyOrigin::Chat,
    }
}

/// The next delta on `deltas`, which must already be there.
fn next_delta(deltas: &mut broadcast::Receiver<Delta>) -> Delta {
    deltas.try_recv().expect("the delta was published at once")
}

#[test]
fn a_recorder_failure_pushed_to_the_client_keeps_its_cause_chain() {
    let failure = HarnessError::Recorder {
        run: Some(RunId::from_raw(1)),
        source: RecorderError::new("disk gone"),
    };
    let rendered = display_chain(&failure);
    assert!(
        rendered.contains("could not be recorded"),
        "the outer frame names what happened: {rendered}"
    );
    assert!(
        rendered.contains("the run recorder failed"),
        "the recorder's own frame follows: {rendered}"
    );
    assert!(
        rendered.contains("disk gone"),
        "the innermost cause reaches the client: {rendered}"
    );
}

#[tokio::test]
async fn the_sessions_broker_hands_a_callback_only_to_a_sections_own_round() {
    let watching = Arc::new(Watching::default());
    let broker = SessionBroker {
        inner: Arc::clone(&watching) as Arc<dyn InferenceBroker>,
        core: core_over(Arc::clone(&watching) as Arc<dyn InferenceBroker>),
        listed: Mutex::new(None),
    };
    for (id, origin) in [(0, ReplyOrigin::Infer), (1, ReplyOrigin::Chat)] {
        let round = Round {
            id: RoundId::new(id),
            origin,
        };
        let _refused = broker
            .chat(
                binding(),
                Vec::new(),
                Vec::new(),
                CompletionOptions::new("m"),
                round,
                None,
            )
            .await;
    }
    assert_eq!(
        *watching.callbacks.lock().unwrap(),
        [(ReplyOrigin::Infer, false), (ReplyOrigin::Chat, true)],
        "a nested infer's round is read whole, and a section's round streams"
    );
}

#[tokio::test]
async fn a_turn_cancel_raised_as_an_answer_lands_ends_the_run_cancelled() {
    let gate = Arc::new(Semaphore::new(0));
    let broker = Arc::new(Gated {
        gate: Arc::clone(&gate),
        rounds: AtomicUsize::new(0),
    });
    let core = core_running(
        INFERS.to_owned(),
        Arc::clone(&broker) as Arc<dyn InferenceBroker>,
    );
    let mut run = pin!(run_once(Arc::clone(&core), Arc::new(Bindings::new())));
    for _ in 0..16 {
        if broker.rounds.load(Ordering::SeqCst) > 0 {
            break;
        }
        let polled = poll_fn(|cx| Poll::Ready(run.as_mut().poll(cx))).await;
        assert!(polled.is_pending(), "the run waits on its round");
    }
    assert_eq!(
        broker.rounds.load(Ordering::SeqCst),
        1,
        "the round is in flight"
    );

    // The supervisor cancels the turn, and the round's answer lands,
    // before the session's task polls the run again.
    gate.add_permits(1);
    core.cancel_current_run();

    let outcome = run.await.expect("a cancelled run is an outcome");
    assert_eq!(
        outcome,
        RunOutcome::Cancelled,
        "the cancel is seen ahead of the landed answer"
    );
}

#[test]
fn a_delta_carries_the_stamp_of_the_reply_that_supersedes_it() {
    let core = core_over(Arc::new(Watching::default()));
    let session = Session::new(Arc::clone(&core));
    let mut deltas = session.subscribe_deltas();
    let on_delta = delta_callback(Arc::clone(&core));

    on_delta(StreamDelta::Text("set".to_owned()));
    core.observe(&reply());
    on_delta(StreamDelta::Reasoning("next".to_owned()));

    let settled = session.transcript(0);
    assert_eq!(settled.len(), 1, "the reply reached the transcript");
    assert_eq!(
        next_delta(&mut deltas),
        Delta {
            kind: DeltaKind::Text,
            content: "set".to_owned(),
            reply: settled[0].reply.expect("a reply is stamped"),
        },
        "the delta published before the reply carries the reply's stamp"
    );
    assert_eq!(
        next_delta(&mut deltas).reply,
        settled[0].reply.expect("a reply is stamped") + 1,
        "a delta after the reply belongs to the next round"
    );
}

#[test]
fn a_session_with_no_client_listening_drops_deltas_without_failing_the_round() {
    let core = core_over(Arc::new(Watching::default()));
    let on_delta = delta_callback(core);

    // A panic here would abort the round's future mid-stream; returning
    // is the callback dropping the delta.
    on_delta(StreamDelta::Text("unheard".to_owned()));
    on_delta(StreamDelta::Text("still unheard".to_owned()));
}
