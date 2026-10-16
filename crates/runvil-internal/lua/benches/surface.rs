//! Benchmarks for the active unified-surface leaf paths in the Lua boundary:
//! `messages.new()` builder construction and per-dispatch projection.
//!
//! Run with `cargo bench -p runvil-lua --features test-support`.

// The criterion_group! macro expansion generates an undocumented public
// entry point; bench targets have no docs contract.
#![expect(
    missing_docs,
    reason = "the criterion_group! macro expansion generates an undocumented public entry point; bench targets have no docs contract"
)]
#![expect(
    clippy::expect_used,
    reason = "bench setup panics on construction failure, which is the desired behavior"
)]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use criterion::{Criterion, criterion_group, criterion_main};
use runvil_lua::{
    LuaProgram, MessageContent, MessageRecord, RecordRole, SectionVm, ToolCallRecord, ToolSet,
    project_messages,
};
use runvil_model_client::model::ModelSet;
use runvil_types::emitter::{DebugMode, Emitter, EventSink};
use runvil_types::seed::RunSeed;
use runvil_types::untrusted::GuardNonce;
use serde_json::json;

const SECTION: &str = "Bench";

/// An emitter over a sink nobody drains: the bench measures the VM, not
/// the reports.
fn emitter() -> Emitter {
    Emitter::root(EventSink::default(), "bench", DebugMode::Off)
}

/// A section VM with Engine values injected, so the `messages` namespace is
/// installed as the executor installs it.
fn builder_vm() -> SectionVm {
    SectionVm::new_for_section(
        &GuardNonce::from_seed(&RunSeed::from_bytes([1; 32])),
        &Arc::new(ToolSet::default()),
        &Arc::new(Mutex::new(ModelSet::default())),
        &emitter(),
        SECTION,
    )
    .expect("the bench VM builds")
    .inject_values(
        "",
        &json!({}),
        &std::sync::Arc::new(
            runvil_vfs::VfsRef::default()
                .acquire(runvil_vfs::Origin::new("surface bench"))
                .expect("the stock backend acquires"),
        ),
    )
    .expect("value injection installs the messages namespace")
}

/// Builds one message list through the Rust-backed `messages.new()` list:
/// the chainable builder calls, each validating the record it adds.
fn message_building(c: &mut Criterion) {
    let vm = builder_vm();
    let program = LuaProgram::compile(
        "local msgs = messages.new()\n\
         msgs:system('you are careful')\n\
         msgs:user('summarize this')\n\
         msgs:assistant('a summary')\n\
         msgs:user('now shorter')\n\
         return #msgs",
        "bench",
        NonZeroU32::MIN,
        &emitter(),
        SECTION,
    )
    .expect("the builder chunk compiles");
    let emitter = emitter();
    c.bench_function("message_building", |b| {
        b.iter(|| {
            vm.run_chunk(&program, &emitter, SECTION)
                .expect("the builder chunk runs");
        });
    });
}

/// Projects one conversation covering the active shapes: leading system,
/// alternating user and assistant text, and one complete tool exchange.
fn projection(c: &mut Criterion) {
    let text = |role: RecordRole, body: &str| MessageRecord {
        role,
        content: MessageContent::Text(body.to_owned()),
        tool_calls: Vec::new(),
    };
    let messages = vec![
        text(RecordRole::System, "you are careful"),
        text(RecordRole::User, "summarize this"),
        text(RecordRole::Assistant, "a summary"),
        text(RecordRole::User, "now with tools"),
        MessageRecord {
            tool_calls: vec![ToolCallRecord {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({ "value": "polish" }),
            }],
            ..text(RecordRole::Assistant, "")
        },
        text(
            RecordRole::Tool {
                tool_call_id: "call_1".to_owned(),
            },
            "echoed: polish",
        ),
        text(RecordRole::Assistant, "refined text"),
    ];
    c.bench_function("projection", |b| {
        b.iter(|| {
            project_messages(std::hint::black_box(&messages)).expect("the conversation projects");
        });
    });
}

criterion_group!(benches, message_building, projection);
criterion_main!(benches);
