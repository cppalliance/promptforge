//! Payload fidelity: a stored payload reads back as the identical value.
//!
//! The log writes a record's payload as JSON text and parses it back on
//! read; replay compares the parsed value against the one that produced it,
//! so a float whose text does not parse exactly, or an object whose key
//! order changes, would make a replayed run differ from the run that ran.

use harness_log::{Record, RecordFilter, RecordKind, RunLog, RunMeta};
use promptforge::event::{Event, ReplyOrigin};
use promptforge::ids::{ParseIdError, Provenance, TaskId};
use promptforge::metrics::{CallMetrics, ClientTiming, LlamaTimings, Usage, VllmMetrics};
use serde_json::{Map, Value, json};

/// A double whose shortest decimal text a plain parser reads back as the
/// neighbour `3.9078`: the defect exact parsing removes.
const AWKWARD: f64 = 3.907_800_000_000_000_4;

/// Every record of a run, in loop order.
const ALL: RecordFilter = RecordFilter {
    kind: None,
    task: None,
    last: None,
};

/// A run's opening row, as the harness would write it.
fn meta() -> RunMeta {
    RunMeta {
        session_id: "session-1".to_owned(),
        agent: "chat".to_owned(),
        prompt_hash: "sha256:abc".to_owned(),
        seed: 3,
        flags: 0,
        started_at: 1_700_000_000_000,
    }
}

/// The replay key every fidelity event carries.
fn provenance() -> Result<Provenance, ParseIdError> {
    Ok(Provenance {
        task: "0".parse::<TaskId>()?,
        seq: 0,
    })
}

/// Appends `payload` as the one event of a fresh run and asserts the log
/// returns the identical value and the identical text.
async fn round_trips(payload: Value) -> Result<(), Box<dyn std::error::Error>> {
    let mut log = RunLog::in_memory().await?;
    let run = log.begin_run(meta()).await?;
    log.append(
        run,
        Record {
            task_id: "0".to_owned(),
            task_seq: 0,
            kind: RecordKind::Event,
            effect_id: None,
            payload: payload.clone(),
        },
    )
    .await?;

    let stored = log.records(run, ALL).await?;
    assert_eq!(
        stored.len(),
        1,
        "the run holds exactly the one event appended"
    );
    let returned = &stored[0].record.payload;
    assert_eq!(returned, &payload, "the value survives the round trip");
    assert_eq!(
        serde_json::to_string(returned)?,
        serde_json::to_string(&payload)?,
        "the stored text is canonical for the value"
    );
    Ok(())
}

#[tokio::test]
async fn awkward_floats_round_trip_through_the_log() {
    // Each of these is a double whose shortest decimal text a plain
    // parser does not reproduce; without exact parsing the value read
    // back is a neighbour of the one written.
    round_trips(json!({
        "sum": 0.1 + 0.2,
        "noise": AWKWARD,
        "tiny": 1e-7,
        "huge": 1e21,
        "max": f64::MAX,
        "negative_zero": -0.0,
    }))
    .await
    .unwrap();
}

#[tokio::test]
async fn nested_objects_round_trip_with_canonical_key_order() {
    // Insertion order is deliberately not sorted; a `Value` object is a
    // `BTreeMap`, so the stored text orders keys by byte value whatever
    // the insertion order, and that canonical order must survive the read.
    let mut inner = Map::new();
    inner.insert("zulu".to_owned(), json!(1));
    inner.insert("alpha".to_owned(), json!({ "yankee": 2, "bravo": 3 }));
    inner.insert("mike".to_owned(), json!([{ "delta": 4, "charlie": 5 }]));

    let mut outer = Map::new();
    outer.insert("second".to_owned(), Value::Object(inner));
    outer.insert("first".to_owned(), json!(true));

    let payload = Value::Object(outer);
    assert_eq!(
        serde_json::to_string(&payload).unwrap(),
        r#"{"first":true,"second":{"alpha":{"bravo":3,"yankee":2},"mike":[{"charlie":5,"delta":4}],"zulu":1}}"#
    );
    round_trips(payload).await.unwrap();
}

#[tokio::test]
async fn arrays_of_mixed_numbers_round_trip_through_the_log() {
    round_trips(json!({
        "mixed": [0, -1, 1.5, 0.1 + 0.2, AWKWARD, 1e21, i64::MIN, u64::MAX],
        "nested": [[1.25, 2], [3.5]],
    }))
    .await
    .unwrap();
}

#[tokio::test]
async fn a_real_assistant_reply_with_metrics_round_trips_through_the_log() {
    let event = Event::AssistantReply {
        execution: "run-1".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance().unwrap(),
        turn: 2,
        text: "hello".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "llama-3".to_owned(),
        origin: ReplyOrigin::Chat,
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
                prompt_ms: AWKWARD,
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
                ttft_ms: Some(3.907_800_000_000_000_4),
                mean_itl_ms: Some(8.25),
                e2e_ms: 0.1 + 0.2,
            }),
        }),
    };
    round_trips(serde_json::to_value(&event).unwrap())
        .await
        .unwrap();
}
