//! Benchmarks for the active unified-surface leaf paths in the Lua boundary:
//! `messages.new()` builder construction and per-dispatch projection.
//!
//! Run with `cargo bench -p promptforge-lua`.

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

use criterion::{Criterion, criterion_group, criterion_main};
use promptforge_core_support::observe::NullObserver;
use promptforge_core_support::untrusted::GuardNonce;
use promptforge_lua::{
    LuaProgram, MessageContent, MessageRecord, MessageRole, SectionVm, ToolCallRecord, ToolSet,
    project_messages,
};
use promptforge_model_client::model::ModelSet;
use serde_json::json;

const EXECUTION: &str = "bench";
const SECTION: &str = "Bench";

/// A section VM with host values injected, so the `messages` namespace is
/// installed exactly as the executor installs it.
fn builder_vm() -> SectionVm {
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        &ToolSet::default(),
        &ModelSet::default(),
        EXECUTION,
        &NullObserver::default(),
        SECTION,
    )
    .expect("the bench VM builds");
    vm.inject_host(
        "",
        &json!({}),
        &std::sync::Arc::new(promptforge_vfs::empty().acquire()),
    )
    .expect("host injection installs the messages namespace");
    vm
}

/// Builds one message list through the pure-Lua `messages.new()` builders:
/// the chainable method calls over a plain numeric table.
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
        EXECUTION,
        &NullObserver::default(),
        SECTION,
    )
    .expect("the builder chunk compiles");
    c.bench_function("message_building", |b| {
        b.iter(|| {
            vm.run_chunk(&program, &NullObserver::default(), SECTION)
                .expect("the builder chunk runs");
        });
    });
}

/// Projects one conversation covering the active shapes: leading system,
/// alternating user and assistant text, and one complete tool exchange.
fn projection(c: &mut Criterion) {
    let text = |role: MessageRole, body: &str| MessageRecord {
        role,
        content: MessageContent::Text(body.to_owned()),
        tool_calls: Vec::new(),
        tool_call_id: None,
    };
    let messages = vec![
        text(MessageRole::System, "you are careful"),
        text(MessageRole::User, "summarize this"),
        text(MessageRole::Assistant, "a summary"),
        text(MessageRole::User, "now with tools"),
        MessageRecord {
            role: MessageRole::Assistant,
            content: MessageContent::Text(String::new()),
            tool_calls: vec![ToolCallRecord {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({ "value": "polish" }),
            }],
            tool_call_id: None,
        },
        MessageRecord {
            role: MessageRole::Tool,
            content: MessageContent::Text("echoed: polish".to_owned()),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_1".to_owned()),
        },
        text(MessageRole::Assistant, "refined text"),
    ];
    c.bench_function("projection", |b| {
        b.iter(|| {
            project_messages(std::hint::black_box(&messages)).expect("the conversation projects");
        });
    });
}

criterion_group!(benches, message_building, projection);
criterion_main!(benches);
