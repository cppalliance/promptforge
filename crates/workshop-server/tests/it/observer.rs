//! The persisted event-log schema canary. `observer/version1.jsonl` was
//! written by the first shipped version of the log format and is committed
//! verbatim: it must load in every future build, because every session log
//! already on disk has its shape. A change that fails this test breaks
//! those logs silently - the fix is a new format version with a migration,
//! never an edit to the fixture.

use std::path::Path;

use promptforge_core_support::events::{
    CallMetrics, ClientTiming, EventLog, LlamaTimings, RuntimeEvent, RuntimeEventKind, Usage,
    VllmMetrics,
};
use workshop_server::WorkshopObserver;

#[test]
fn the_committed_version_1_log_loads_forever_after() {
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/observer/version1.jsonl");
    // Load a byte-for-byte copy: load_from reopens its file for append,
    // and the committed fixture must never carry a write handle.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let fixture = dir.path().join("version1.jsonl");
    std::fs::copy(&committed, &fixture).expect("stage a copy of the committed fixture");
    let log = WorkshopObserver::load_from(&fixture)
        .expect("the committed version-1 fixture must load in every future build");

    let bare = |kind: RuntimeEventKind, turn: u32, content: &str| RuntimeEvent {
        kind,
        section: "chat".to_owned(),
        chain_id: 0,
        depth: 0,
        turn,
        content: content.to_owned(),
        model: None,
        tool_call_id: None,
        finish_reason: None,
        metrics: None,
    };
    let expected = [
        bare(RuntimeEventKind::UserInput, 0, "hi"),
        RuntimeEvent {
            model: Some("llama-3".to_owned()),
            ..bare(RuntimeEventKind::Thinking, 1, "pondering")
        },
        RuntimeEvent {
            model: Some("llama-3".to_owned()),
            ..bare(
                RuntimeEventKind::AssistantToolCalls,
                1,
                r#"[{"id":"call_1","name":"read_file","arguments":{"path":"notes.txt"}}]"#,
            )
        },
        RuntimeEvent {
            tool_call_id: Some("call_1".to_owned()),
            ..bare(RuntimeEventKind::ToolResult, 1, "file contents")
        },
        RuntimeEvent {
            chain_id: 1,
            model: Some("llama-3".to_owned()),
            finish_reason: Some("stop".to_owned()),
            metrics: Some(CallMetrics {
                usage: Some(Usage {
                    prompt_tokens: 7,
                    completion_tokens: 3,
                    total_tokens: 10,
                    cached_tokens: Some(2),
                    reasoning_tokens: Some(1),
                }),
                llama: Some(LlamaTimings {
                    prompt_n: 7,
                    prompt_ms: 12.5,
                    prompt_per_second: 560.0,
                    predicted_n: 3,
                    predicted_ms: 30.5,
                    predicted_per_second: 98.5,
                    draft_n: 4,
                    draft_n_accepted: 2,
                }),
                vllm: Some(VllmMetrics {
                    time_to_first_token_ms: Some(8.5),
                    generation_time_ms: Some(22.5),
                    queue_time_ms: Some(1.5),
                    mean_itl_ms: Some(7.5),
                    tokens_per_second: Some(133.5),
                }),
                client: Some(ClientTiming {
                    ttft_ms: Some(9.5),
                    mean_itl_ms: Some(8.25),
                    e2e_ms: 41.5,
                }),
            }),
            ..bare(RuntimeEventKind::AssistantReply, 2, "hello")
        },
    ];

    assert_eq!(
        log.len(),
        expected.len() as u64,
        "the fixture holds one event of every persisted kind"
    );
    for (index, expected_event) in expected.iter().enumerate() {
        assert_eq!(
            log.get(index as u64).as_ref(),
            Some(expected_event),
            "entry {index} of the committed fixture must replay unchanged"
        );
    }
}
