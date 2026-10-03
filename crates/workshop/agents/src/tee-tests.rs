//! Tests for the recorder tee: an event reaches the conversation only
//! after the inner recorder accepts it, a refusal is the inner recorder's
//! own, and the recorded failure events become failure reports.

use std::sync::{Arc, Mutex};

use harness::record::{
    MemoryRecorder, Record, RecordKind, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome,
    RunRecorder,
};
use promptforge::event::Event;
use promptforge::ids::Provenance;

use crate::state::FailureKind;
use crate::table::Conversations;

use super::*;

fn meta() -> RunMeta {
    RunMeta {
        session_id: "conversation-1".to_owned(),
        agent: String::new(),
        prompt_hash: "sha256:00".to_owned(),
        seed: 7,
        flags: 0,
        started_at: 0,
    }
}

fn provenance() -> Provenance {
    Provenance {
        task: "0".parse().unwrap(),
        seq: 0,
    }
}

fn section_started() -> Event {
    Event::SectionStarted {
        execution: "conversation-1".to_owned(),
        section: "Conversation".to_owned(),
        provenance: provenance(),
    }
}

fn record(kind: RecordKind, event: &Event) -> Record {
    Record {
        task_id: "0".to_owned(),
        task_seq: 0,
        kind,
        effect_id: None,
        payload: serde_json::to_value(event).unwrap(),
    }
}

/// An inner recorder that notes, at each append, how many entries the
/// conversation's transcript already held, and refuses appends on demand.
struct Watching {
    inner: MemoryRecorder,
    conversation: Conversation,
    held_at_append: Mutex<Vec<usize>>,
    refuse: bool,
}

impl RunRecorder for Watching {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        self.inner.begin_run(meta)
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        self.held_at_append
            .lock()
            .unwrap()
            .push(self.conversation.transcript(0).len());
        if self.refuse {
            return Box::pin(async { Err(RecorderError::new("the log is full")) });
        }
        self.inner.append(run, record)
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        self.inner.end_run(run, outcome)
    }
}

fn watched(refuse: bool) -> (Conversation, Arc<Watching>, Arc<dyn RunRecorder>) {
    let conversation = Conversations::new().open("echo");
    let inner = Arc::new(Watching {
        inner: MemoryRecorder::new(),
        conversation: conversation.clone(),
        held_at_append: Mutex::new(Vec::new()),
        refuse,
    });
    let tee = conversation.recorder(inner.clone());
    (conversation, inner, tee)
}

#[tokio::test]
async fn the_tee_adds_an_event_to_the_transcript_after_the_inner_recorder_accepts_it() {
    let (conversation, inner, tee) = watched(false);
    let mut live = conversation.subscribe_events();
    let run = tee.begin_run(meta()).await.unwrap();
    tee.append(run, record(RecordKind::Event, &section_started()))
        .await
        .unwrap();
    assert_eq!(
        *inner.held_at_append.lock().unwrap(),
        [0],
        "the transcript was empty while the inner recorder took the event"
    );
    let entries = conversation.transcript(0);
    assert_eq!(entries.len(), 1, "the accepted event is in the transcript");
    assert_eq!(
        live.try_recv().unwrap(),
        entries[0],
        "the live broadcast carries the entry the transcript holds"
    );
    assert_eq!(
        inner.inner.records(run).len(),
        1,
        "the inner recorder holds the record"
    );
}

#[tokio::test]
async fn the_tee_returns_the_inner_recorders_refusal_and_keeps_the_event_out() {
    let (conversation, _inner, tee) = watched(true);
    let mut live = conversation.subscribe_events();
    let run = tee.begin_run(meta()).await.unwrap();
    let refusal = tee
        .append(run, record(RecordKind::Event, &section_started()))
        .await
        .expect_err("the inner recorder's refusal is the tee's");
    assert_eq!(
        harness::display_chain(&refusal),
        "the run recorder failed: the log is full",
        "the refusal is the inner recorder's own"
    );
    assert!(
        conversation.transcript(0).is_empty(),
        "a refused event never reaches the transcript"
    );
    assert!(live.try_recv().is_err(), "nor the live broadcast");
}

#[tokio::test]
async fn effects_and_answers_reach_the_inner_recorder_and_not_the_transcript() {
    let (conversation, inner, tee) = watched(false);
    let run = tee.begin_run(meta()).await.unwrap();
    for kind in [RecordKind::Effect, RecordKind::Answer] {
        tee.append(run, record(kind, &section_started()))
            .await
            .unwrap();
    }
    assert_eq!(inner.inner.records(run).len(), 2);
    assert!(conversation.transcript(0).is_empty());
}

#[tokio::test]
async fn begin_run_names_the_conversations_agent_and_notes_the_run() {
    let (conversation, inner, tee) = watched(false);
    assert_eq!(conversation.run_id(), None, "no run has begun");
    let run = tee.begin_run(meta()).await.unwrap();
    assert_eq!(
        inner.inner.meta(run).map(|meta| meta.agent),
        Some("echo".to_owned()),
        "the run's metadata names the agent the conversation runs"
    );
    assert_eq!(conversation.run_id(), Some(run));
}

#[tokio::test]
async fn recorded_turn_failures_report_their_kind_and_section() {
    let (conversation, _inner, tee) = watched(false);
    let mut errors = conversation.subscribe_errors();
    let run = tee.begin_run(meta()).await.unwrap();
    let model = Event::ModelTurnFailed {
        execution: "conversation-1".to_owned(),
        section: "Conversation".to_owned(),
        provenance: provenance(),
    };
    let tool = Event::ToolCallFailed {
        execution: "conversation-1".to_owned(),
        section: "Lookup".to_owned(),
        provenance: provenance(),
    };
    for event in [&model, &tool] {
        tee.append(run, record(RecordKind::Event, event))
            .await
            .unwrap();
    }
    let first = errors.try_recv().unwrap();
    assert_eq!(first.kind, FailureKind::ModelTurnFailed);
    assert_eq!(first.message, "Model turn failed in agent `Conversation`");
    let second = errors.try_recv().unwrap();
    assert_eq!(second.kind, FailureKind::ToolCallFailed);
    assert_eq!(second.message, "Tool call failed in agent `Lookup`");
    assert_eq!(
        conversation.transcript(0).len(),
        2,
        "a failure event is still a transcript entry"
    );
}
