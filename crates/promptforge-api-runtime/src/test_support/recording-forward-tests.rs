use std::sync::Mutex;

use promptforge_api_types::ids::{ChainId, Provenance, TaskId};

use super::*;

fn provenance() -> Provenance {
    Provenance {
        task: TaskId::from(ChainId::root()),
        seq: 0,
    }
}

#[derive(Default)]
struct Recorder {
    observed: Mutex<Vec<(String, String)>>,
    content: Mutex<Vec<String>>,
    captured: Mutex<Vec<(u32, String)>>,
}

impl Observer for Recorder {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        self.observed
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push((section.to_owned(), event.to_string()));
    }

    fn on_assistant_reply(
        &self,
        _execution: &str,
        section: &str,
        chain_id: u32,
        depth: u32,
        turn: u32,
        text: &str,
        finish_reason: Option<&str>,
        model: &str,
        _metrics: Option<&promptforge_api_types::metrics::CallMetrics>,
    ) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!(
                "{section}: reply chain={chain_id} depth={depth} turn={turn} text={text} finish={finish_reason:?} model={model}"
            ));
    }

    fn on_user_input(&self, _execution: &str, section: &str, text: &str) {
        self.content
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push(format!("{section}: input {text}"));
    }
}

impl DebugCapture for Recorder {
    fn on_event(&self, _execution: &str, _section: &str, turn_index: u32, event: DebugEvent) {
        let kind = match event {
            DebugEvent::Request { .. } => "request",
            DebugEvent::Response { .. } => "response",
        };
        self.captured
            .lock()
            .expect("the recorder mutex is not poisoned")
            .push((turn_index, kind.to_owned()));
    }
}

#[test]
fn each_event_group_reaches_its_seam_in_batch_order() {
    let recorder = Recorder::default();
    let events = vec![
        Event::SectionStarted {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
        },
        Event::Request {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            body: serde_json::json!({}),
        },
        Event::Response {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            body: serde_json::json!({}),
            finish_reason: None,
            reasoning_content: None,
        },
        Event::AssistantReply {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            text: "hi".to_owned(),
            finish_reason: Some("stop".to_owned()),
            model: "m".to_owned(),
            metrics: None,
        },
        Event::UserInput {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            text: "typed".to_owned(),
        },
        Event::Lua {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            message: "note".to_owned(),
        },
        Event::TaskSucceeded {
            execution: "run".to_owned(),
            section: "W".to_owned(),
            provenance: provenance(),
            task: "0.1".parse().expect("a task id parses"),
        },
    ];
    forward(events, &recorder, Some(&recorder));
    assert_eq!(
        *recorder.observed.lock().expect("not poisoned"),
        vec![
            ("A".to_owned(), Observation::SectionStarted.to_string()),
            ("A".to_owned(), "Lua: note".to_owned()),
            ("W".to_owned(), "Task succeeded".to_owned()),
        ]
    );
    assert_eq!(
        *recorder.content.lock().expect("not poisoned"),
        vec![
            "A: reply chain=0 depth=0 turn=1 text=hi finish=Some(\"stop\") model=m".to_owned(),
            "A: input typed".to_owned(),
        ]
    );
    assert_eq!(
        *recorder.captured.lock().expect("not poisoned"),
        vec![(1, "request".to_owned()), (1, "response".to_owned())]
    );
}

#[test]
fn debug_events_are_dropped_without_a_capture() {
    let recorder = Recorder::default();
    forward(
        vec![Event::Request {
            execution: "run".to_owned(),
            section: "A".to_owned(),
            provenance: provenance(),
            turn: 1,
            body: serde_json::json!({}),
        }],
        &recorder,
        None,
    );
    assert!(recorder.observed.lock().expect("not poisoned").is_empty());
    assert!(recorder.captured.lock().expect("not poisoned").is_empty());
}
