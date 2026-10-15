//! Benchmarks for the active executor paths: `models.loop` (its rules
//! run in Rust behind the trampoline in `__impl_coro.lua`, which yields
//! one `chat` request per round to the scheduler) over one scripted
//! terminal turn, which is the round-overhead gate the plan's checkpoints
//! compare against, and the `compactors.fail` invocation on a precheck
//! overflow, which runs zero rounds and measures the overflow failure
//! path.
//!
//! Run with `cargo bench -p promptforge-engine`.

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
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use promptforge_engine::test_support::scripted_chat::{ScriptedChat, ScriptedReply};
use promptforge_engine::test_support::{drive, perform_locally};
use promptforge_engine::{Effect, EffectAnswer, Run, RunContext, RunLimits, RunResult};
use promptforge_parser::Prompt;
use promptforge_types::models::{ModelCatalog, ModelDescriptor, ModelId, ThinkingMode};
use promptforge_types::seed::RunSeed;
use promptforge_types::timestamp::Timestamp;

const EXECUTION: &str = "bench";

/// The fixed seed every bench run starts from.
const SEED: RunSeed = RunSeed::from_bytes([1; 32]);

/// A scripted model whose every round gets the same terminal-text reply,
/// so a `models.loop` bench measures exactly one round per iteration.
fn bench_chat() -> ScriptedChat {
    ScriptedChat::new(vec![ScriptedReply::Text {
        model: "bench-model".to_owned(),
        content: "bench reply".to_owned(),
        finish_reason: Some("stop".to_owned()),
        reasoning: None,
        metrics: None,
    }])
}

/// The model catalog the bench prompts resolve against; `context` sizes the
/// one model's window.
fn bench_catalog(context: u32) -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("bench-model").expect("the bench model id is valid"),
        "A general model for benches",
        NonZeroU32::new(context).expect("the bench context is non-zero"),
        ThinkingMode::Switchable,
    )])
    .expect("the bench catalog has a single unique model")
}

/// One section driving `models.loop` over a builder-made list.
const LOOP_PROMPT: &str = "---\nname: bench_loop\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
    # Bench\n\n\
    ```lua\n\
    models.default('writer')\n\
    ```\n\n\
    ## Only\n\n\
    ```lua\n\
    local msgs = messages.new()\n\
    msgs:user('hi')\n\
    models.loop(msgs)\n\
    return msgs[#msgs].content\n\
    ```\n";

/// Parses the loop prompt once for the whole benchmark.
fn parse_loop_prompt() -> Prompt {
    Prompt::parse(LOOP_PROMPT, EXECUTION)
        .0
        .expect("the bench prompt parses")
}

/// Prepares and runs `prompt` on a `context`-token window, on the serial
/// driver: every effect is performed in process, and every round is
/// answered by `chat` under the run's timeout. A prompt the prepare
/// refuses ends with the refusal.
fn run_once(prompt: &Prompt, chat: &ScriptedChat, context: u32) -> RunResult {
    let limits = RunLimits::new();
    let ctx = RunContext::new(EXECUTION, SEED, Timestamp::UNIX_EPOCH)
        .model(bench_catalog(context).models()[0].clone())
        .limits(limits);
    let (ctx, requirements) = ctx.prepare(prompt);
    if let Some(refusal) = requirements.refusal() {
        return RunResult::Failure(refusal);
    }
    let run = Run::new(Arc::new(prompt.clone()), "", ctx);
    let (result, _) = drive(run, |_, effect| {
        perform_locally(effect, &mut |round| {
            let Effect::Chat {
                messages,
                tools,
                options,
                ..
            } = round
            else {
                unreachable!("perform_locally hands the model only chat effects");
            };
            let (_, answer) = chat.complete_now(messages, tools, options, limits.timeout());
            EffectAnswer::Chat(answer.map(Box::new))
        })
    });
    result
}

/// One `models.loop` turn end to end: parse is excluded, so the measurement
/// covers VM setup, projection, the round, and the terminal-record append.
fn models_loop(c: &mut Criterion) {
    let chat = bench_chat();
    let prompt = parse_loop_prompt();
    c.bench_function("models_loop", |b| {
        b.iter(|| {
            let result = run_once(&prompt, &chat, 131_072);
            assert!(
                matches!(result, RunResult::Ok(_)),
                "the loop bench run succeeds: {result:?}"
            );
        });
    });
    assert!(
        chat.call_count() > 0,
        "every loop iteration is exactly one request"
    );
}

/// The `compactors.fail` path: a one-token context window overflows the
/// request precheck before any dispatch, so the default compactor raises
/// typed context exhaustion without a single request.
fn compactors_fail(c: &mut Criterion) {
    let chat = bench_chat();
    let prompt = parse_loop_prompt();
    c.bench_function("compactors_fail", |b| {
        b.iter(|| {
            let result = run_once(&prompt, &chat, 1);
            let RunResult::Failure(error) = result else {
                panic!("a one-token window must exhaust at the precheck");
            };
            assert_eq!(
                error.kind(),
                promptforge_engine::RunErrorKind::ContextExhausted,
                "the default compactor is compactors.fail: {error:?}"
            );
        });
    });
    assert_eq!(
        chat.call_count(),
        0,
        "the precheck overflow never reaches the wire"
    );
}

criterion_group!(benches, models_loop, compactors_fail);
criterion_main!(benches);
