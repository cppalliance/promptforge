Answer every kind of outside work a run asks for, and log what your program did.

You need this when your prompts call models or tools or wait on a timer, or when you want a log of every piece of outside work your program did for a run.

# Where this fits

[Run a prompt](crate#run-a-prompt) taught the loop every program writes: step the run, answer each *effect*, a piece of outside work the run asks for, and resume. That program answers only store effects and drops the rest. This page adds model rounds, tool calls, and timers. Then it shows how to log each effect beside its answer.

# Answer every kind of effect

Your prompt calls a model, a tool, and a timed wait, but your program answers only store operations.

A run asks for four kinds of outside work: a model round, a tool call, an operation on its store of virtual files, and a timer. Each is an [`Effect`] that takes one [`EffectAnswer`] of its kind.

The greeter prompt asks for all four. Its text sits in the example's collapsed setup lines. `Quick`, the task behind the timed wait, does no outside work, so it finishes while the run steps, before any timer answer can arrive, and the greeter's return value never reads what `join_any` returns. The order rule below covers when order does matter.

````
# use std::num::NonZeroU32;
# use std::sync::Arc;
# use std::time::Duration;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::model::{Completion, CompletionError, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# const GREETER: &str = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Asks a model and a tool at once, and waits.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "tools:\n",
#     "  shout: example/text/shout\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "local ask = tasks.spawn('## Ask')\n",
#     "local shout = tasks.spawn('## Shout')\n",
#     "store.write('note.md', 'hello')\n",
#     "local results = tasks.join({ ask, shout })\n",
#     "tasks.join_any({ tasks.spawn('## Quick') }, { timeout = 0.01 })\n",
#     "return results[1].result .. ' / ' .. results[2].result\n",
#     "```\n\n",
#     "## Ask\n\n",
#     "```lua\n",
#     "models.use('writer')\n",
#     "return models.infer('hello')\n",
#     "```\n\n",
#     "## Shout\n\n",
#     "```lua\n",
#     "return tools.call('shout', { text = 'hello' })\n",
#     "```\n\n",
#     "## Quick\n\n",
#     "```lua\n",
#     "return 'quick'\n",
#     "```\n",
# );
# fn canned_completion(text: &str) -> Result<Box<Completion>, CompletionError> {
#     let reply = CompletionResult::Text(text.to_owned());
#     Completion::from_result(reply, "canned").map(Box::new).map_err(Into::into)
# }
# fn prepared_run() -> Result<Run, Box<dyn std::error::Error>> {
#     let (parsed, _parse_events) = Prompt::parse(GREETER, "greeter");
#     let prompt = parsed?;
#     let model = ModelDescriptor::new(
#         ModelId::gateway("canned")?,
#         "Always replies hi there",
#         NonZeroU32::new(8_192).ok_or("a context window is never zero")?,
#         ThinkingMode::Never,
#     );
#     let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
#     let shout = ToolDescriptor::new(
#         ToolId::parse("example/text/shout")?,
#         "shout",
#         "Returns the text in capital letters.",
#         serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
#     );
#     let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
#     let (ctx, requirements) = environment.prepare(&prompt, ctx);
#     if let Some(refusal) = requirements.refusal() {
#         return Err(refusal.into());
#     }
#     Ok(Run::new(Arc::new(prompt), "", ctx))
# }
// 1. Answer each effect with the answer of its own kind.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Chat { .. } => EffectAnswer::Chat(canned_completion("hi there")),
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
        Effect::Timer { seconds } => {
            std::thread::sleep(Duration::try_from_secs_f64(seconds).unwrap_or_default());
            EffectAnswer::Timer
        }
    }
}

// 2. Drive one run, answering its effects in issue order or reversed.
fn drive(mut run: Run, reverse: bool) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        match run.step() {
            Step::Pending { mut effects, .. } => {
                if reverse {
                    effects.reverse();
                }
                for (id, _provenance, effect) in effects {
                    run.resume(id, answer(effect));
                }
            }
            Step::Done { result: RunResult::Ok(text), .. } => return Ok(text),
            Step::Done { result, .. } => return Err(format!("the greeter did not succeed: {result:?}").into()),
        }
    }
}

// 3. Answering in reverse gives the same result as answering in issue order.
let text = drive(prepared_run()?, true)?;
assert_eq!(text, drive(prepared_run()?, false)?);
assert_eq!(text, "hi there / HI THERE");
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `answer` matches each effect with its answer.
   - For [`Effect::Chat`], forward live pieces to your streaming callback only when `stream` is `true`, as in rounds of `models.loop`, the section's chat loop. A `models.infer` round, a nested one-shot call over one user message with no tools, sets it `false`, so no live pieces arrive and a callback waiting on them never hears anything.
   - [`Effect::ToolCall`]'s `origin`, a [`ToolCallOrigin`], says whether Lua ([`ToolCaller::Script`]) or a model round ([`ToolCaller::Model`]) asked, so you can let a script call a tool freely and still check the call when a model asks for it.
   - [`Effect::Vfs`] holds `access`, an [`Arc`](std::sync::Arc) around an [`Access`](crate::vfs::Access), the chain's permission to the store, where a *chain* is one walk over sibling sections. Pass it to [`perform_vfs_op`](crate::vfs::perform_vfs_op) as given, and never build a second access from it, widen it to more of the store, or use it for any store work beyond this one operation. Holding the `Arc` afterward is harmless, because the access refuses every operation once the run reaches `Done` or is dropped.
   - Sleep for [`Effect::Timer`]'s `seconds`, then answer [`EffectAnswer::Timer`].
2. `drive` resumes each effect from [`Step::Pending`](crate::Step::Pending) with [`Run::resume`](crate::Run::resume), backward when `reverse` is set.
3. Both runs return the same text.

Every store operation leaves a claim on its path, and a claim clashes with one left by a chain that is not ordered with it. A store answer reporting such a clash ends the run at once, with a failure the prompt cannot catch; [vfs](crate::vfs) explains claims.

````text
  the run asks for          your Harness answers with
  ───────────────────       ──────────────────────────────────────────────
  Effect::Chat         ──>  EffectAnswer::Chat        completion or error
  Effect::ToolCall     ──>  EffectAnswer::ToolCall    tool output or error
  Effect::Vfs          ──>  EffectAnswer::Vfs         store outcome or error
  Effect::Timer        ──>  EffectAnswer::Timer       after the sleep ends

  any of the four      ──>  EffectAnswer::Dropped     given up, still its one answer
````

Give up on an effect by answering [`EffectAnswer::Dropped`]; a chain still waiting on it resumes with a cancelled error.

A chain stops waiting when the run's outcome is decided, or when its task is torn down because its owner, the chain that spawned it, ended. After a `Pending` step, [`Run::decided`](crate::Run::decided) returning `true` means nobody waits on any effect still out. Still answer each one, because [`Step::Done`](crate::Step::Done) waits for every issued effect, and the run discards an answer nobody waits on, so `Dropped` and the real answer both work.

You might expect `resume` to need answers in issue order. Instead, it matches each answer to its effect by the [`EffectId`] you pass, so resume each as it finishes.

However you pace or order your answers, each task's effects, events, and provenance come out the same, and so does the run's returned text; only the interleaving of events across tasks can differ. Answer order does decide which task finishes first, so a result that depends on that, such as what `tasks.join_any` or a timed wait returns, or whether a spawner is still running when its task ends, can change with order.

Next, [Log effects and answers](#log-effects-and-answers).

# Log effects and answers

You want a log of every piece of outside work a run asked for, and of how your program answered it.

Logging effects is like logging requests and responses in web middleware, except that an effect can hold a live store handle. So you log a *record*: the same content minus live handles and full error values, ready to serialize. An effect's record is an [`EffectRecord`], and an answer's is an [`AnswerRecord`].

````
# use std::num::NonZeroU32;
# use std::sync::Arc;
# use std::time::Duration;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
# use promptforge::vfs::perform_vfs_op;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# const GREETER: &str = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Asks a model and a tool at once, and waits.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "tools:\n",
#     "  shout: example/text/shout\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "local ask = tasks.spawn('## Ask')\n",
#     "local shout = tasks.spawn('## Shout')\n",
#     "store.write('note.md', 'hello')\n",
#     "local results = tasks.join({ ask, shout })\n",
#     "tasks.join_any({ tasks.spawn('## Quick') }, { timeout = 0.01 })\n",
#     "return results[1].result .. ' / ' .. results[2].result\n",
#     "```\n\n",
#     "## Ask\n\n",
#     "```lua\n",
#     "models.use('writer')\n",
#     "return models.infer('hello')\n",
#     "```\n\n",
#     "## Shout\n\n",
#     "```lua\n",
#     "return tools.call('shout', { text = 'hello' })\n",
#     "```\n\n",
#     "## Quick\n\n",
#     "```lua\n",
#     "return 'quick'\n",
#     "```\n",
# );
# fn answer(effect: Effect) -> EffectAnswer {
#     match effect {
#         Effect::Chat { .. } => {
#             let reply = CompletionResult::Text("hi there".to_owned());
#             EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new).map_err(Into::into))
#         }
#         Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
#         Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#         Effect::Timer { seconds } => {
#             std::thread::sleep(Duration::try_from_secs_f64(seconds).unwrap_or_default());
#             EffectAnswer::Timer
#         }
#     }
# }
# fn prepared_run() -> Result<Run, Box<dyn std::error::Error>> {
#     let (parsed, _parse_events) = Prompt::parse(GREETER, "greeter");
#     let prompt = parsed?;
#     let model = ModelDescriptor::new(
#         ModelId::gateway("canned")?,
#         "Always replies hi there",
#         NonZeroU32::new(8_192).ok_or("a context window is never zero")?,
#         ThinkingMode::Never,
#     );
#     let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
#     let shout = ToolDescriptor::new(
#         ToolId::parse("example/text/shout")?,
#         "shout",
#         "Returns the text in capital letters.",
#         serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
#     );
#     let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
#     let (ctx, requirements) = environment.prepare(&prompt, ctx);
#     if let Some(refusal) = requirements.refusal() {
#         return Err(refusal.into());
#     }
#     Ok(Run::new(Arc::new(prompt), "", ctx))
# }
use promptforge::effect::{AnswerRecord, EffectRecord};
use serde_json::{json, Value};

// 1. Drive the greeter from the tour above.
let mut run = prepared_run()?;
let mut log: Vec<String> = Vec::new();
let mut issued = 0;
let result = loop {
    match run.step() {
        Step::Pending { effects, .. } => {
            for (id, provenance, effect) in effects {
                issued += 1;
                // 2. Take the effect's record before performing it, and the answer's before resuming it.
                let effect_record = effect.record();
                let reply = answer(effect);
                let answer_record = reply.record();
                run.resume(id, reply);
                // 3. Write one JSON line per effect, keyed by provenance, with the answer after the effect.
                let line = json!({ "provenance": provenance, "effect": effect_record, "answer": answer_record });
                log.push(line.to_string());
            }
        }
        Step::Done { result, .. } => break result,
    }
};
assert!(matches!(result, RunResult::Ok(_)));

// 4. The log holds one line per effect, and each line parses back into both record types.
let lines: Vec<Value> = log.iter().map(|line| serde_json::from_str(line)).collect::<Result<_, _>>()?;
assert_eq!(lines.len(), issued);
for line in &lines {
    let _: EffectRecord = serde_json::from_value(line["effect"].clone())?;
    let _: AnswerRecord = serde_json::from_value(line["answer"].clone())?;
}

// 5. The note's write logs as an externally tagged store operation, answered with a unit outcome.
let write = lines.iter().find(|line| line["effect"]["Vfs"]["op"].get("Write").is_some()).ok_or("the greeter writes its note")?;
assert_eq!(write["effect"], json!({ "Vfs": { "op": { "Write": { "path": "note.md", "contents": "hello" } } } }));
assert_eq!(write["answer"], json!({ "Vfs": { "Ok": "Unit" } }));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The loop reuses the greeter and `answer` from the first tour, so logging changes nothing about how you answer.
2. It calls [`Effect::record`] before `answer` consumes the effect, and [`EffectAnswer::record`] before [`Run::resume`](crate::Run::resume) consumes the answer. Both methods borrow, so take each record while you hold the live value.
3. It writes one JSON line per effect: the provenance, the effect record, then its one answer record.
   - Provenance serializes as its task and `seq`, and two runs of the same prompt with the same inputs and answers stamp the same provenance on the same effects, so the key matches across runs.
   - A *replay* re-runs the prompt and compares each new effect's record with the logged one.
   - Records hold no id, so use the number from [`EffectId::get`] only to pair lines within one run.
4. It parses every line back into both record types, and the line count equals the number of effects issued, so the log holds exactly one answer per effect.
5. The note's write logs as `Vfs`, then `op`, then `Write`, answered by `Vfs` with `Ok` holding `Unit`. Both records use serde's external tagging, so you can match on variant names; only [`ToolCaller`] uses snake case, `"script"` and `"model"`.

The records keep what identifies the work:

- [`EffectRecord::Chat`]'s `model` is the bound model's name, and [`ChatAnswerRecord`]'s `model` is the one that served the round; log both to audit which model answered.
- An `AnswerRecord` stores failures as display text, so you cannot recover a [`CompletionError`](crate::model::CompletionError), [`ToolError`](crate::tools::ToolError), or [`VfsError`](crate::vfs::VfsError); branch on the live answer before logging it.
- A chat answer record keeps the reply text or the tool names, never both, and no arguments, ids, bodies, or metrics.
- A text round's metrics arrive in the `metrics` field of [`Event::AssistantReply`](crate::event::Event::AssistantReply), and its raw bodies only as debug events, which are off by default; [Capture raw model traffic](crate::event#capture-raw-model-traffic) shows how to log them.
- [`ToolAnswerRecord`]'s `text` is the tool's raw output before trust handling, and its `trusted` is `true` only when the tool declared its output [`OutputTrust::Trusted`](crate::tools::OutputTrust::Trusted); every other trust level records `false`.
- An `EffectRecord::Chat` leaves out `stream`, so rounds that differ only in streaming log the same.

You might expect to serialize each [`Effect`] straight into your log. Instead, `Effect` is not `Serialize`, `Clone`, or `PartialEq`, because it may hold a live store handle, so you log its record, which drops the handle and keeps the operation.

Log the records, keyed by provenance. Next, [ids](crate::ids) explains the tasks and provenance your log keys on.

# Reference

## ChatAnswerRecord

[`ChatAnswerRecord`] records a completed model round in your log, keeping what identifies the answer and leaving out the request and response bodies. It is the success payload of [`AnswerRecord::Chat`], which [`EffectAnswer::record`] builds from a [`Completion`](crate::model::Completion). It serializes as `{ "model": ..., "finish_reason": "stop", "reply": ..., "tool_calls": [] }`. [Log effects and answers](#log-effects-and-answers) shows what it keeps.

- `model`: the model that served the round, as the response named it, which can differ from the bound name in [`EffectRecord::Chat`].
- `reply`: the reply text for a text reply, and `None` for a tool-call reply, so `reply` and `tool_calls` are never both filled.
- `tool_calls`: only the requested tools' names, in call order, without arguments or ids; empty for a text reply.

## EffectId

[`EffectId`] pairs one in-flight effect with its answer. Take it from an `(id, provenance, effect)` triple in [`Step::Pending`](crate::Step::Pending), and hand it back with the answer through [`Run::resume`](crate::Run::resume). An id means nothing outside the run that issued it. [Answer every kind of effect](#answer-every-kind-of-effect) shows the pairing, and [Log effects and answers](#log-effects-and-answers) shows why a log keys by provenance instead.

- [`get`](EffectId::get): the raw number, for keying your log or task table within one run; a replay matches effects by their records, not by id.

## ToolAnswerRecord

[`ToolAnswerRecord`] records a tool's successful output in your log, as the success payload of [`AnswerRecord::ToolCall`]. [Log effects and answers](#log-effects-and-answers) shows what it keeps.

- `text`: the tool's raw output, before the run's trust rules apply, not what the model or script saw.
- `trusted`: `true` only when the tool declared its output [`OutputTrust::Trusted`](crate::tools::OutputTrust::Trusted); every other trust level records `false`.

## ToolCallOrigin

[`ToolCallOrigin`] says who made one tool call: the run's execution, the section whose Lua was running, and whether its script or a model round asked. Read it from the `origin` of [`Effect::ToolCall`] to attribute a call in your log, or to apply a different policy to the same tool by caller. [`EffectRecord::ToolCall`] copies it unchanged. [Answer every kind of effect](#answer-every-kind-of-effect) introduces it.

- `execution`: the run's execution identifier, a plain string the type does not check.
- `section`: the section that made the call, a plain string the type does not check.

## AnswerRecord

[`AnswerRecord`] stores one [`EffectAnswer`] in your log, one variant per answer kind, with every failure turned into its display text. Get one from [`EffectAnswer::record`]. A stored failure cannot be turned back into a [`CompletionError`](crate::model::CompletionError), [`ToolError`](crate::tools::ToolError), or [`VfsError`](crate::vfs::VfsError), so act on the live answer when you need the error's kind. [Log effects and answers](#log-effects-and-answers) teaches it.

| Variant | What it holds |
|---|---|
| `Chat` | the round's [`ChatAnswerRecord`] or its failure text, recorded as `{ "Chat": { "Ok": { ... } } }` |
| [`ToolCall`](AnswerRecord::ToolCall) | the tool's [`ToolAnswerRecord`] or its failure text |
| `Vfs` | the [`VfsOutcome`](crate::vfs::VfsOutcome) or the failure text; a successful write records as `{ "Vfs": { "Ok": "Unit" } }` |
| `Timer` | nothing; the timer fired |
| `Dropped` | nothing; you dropped the effect without performing it |

## Effect

An [`Effect`] is one piece of outside work a run asks your program to perform; the run never performs it itself. When [`Run::step`](crate::Run::step) returns [`Step::Pending`](crate::Step::Pending), perform each `(EffectId, Provenance, Effect)` and answer it through [`Run::resume`](crate::Run::resume). An `Effect` cannot be cloned, compared, or serialized, because it may hold a live store handle, so log [`Effect::record`] instead. [Answer every kind of effect](#answer-every-kind-of-effect) teaches it.

| Variant | What it asks for |
|---|---|
| `Chat` | one model round over `messages` with `tools` advertised, under the binding's frozen `options` |
| [`ToolCall`](Effect::ToolCall) | one bound tool call; `tool` is the identity you resolve against your activated capabilities, and `alias` the name the prompt used |
| `Vfs` | one operation on the run's store view, one of the eight `store.*` calls; other code that touches the VFS does not appear as this effect |
| `Timer` | one sleep of `seconds`, the timeout behind a timed wait |

- `record`: the request minus its live handles; it keeps only tool names, and leaves out `stream`, `options`, and the store access.
- `Chat.stream`: when `true`, forward the round's live pieces to your streaming callback; a `models.infer` round sets `false` and sends none.
- `Vfs.access`: the chain's permission to the store; use it exactly as given, and it refuses every operation once the run reaches `Done` or is dropped.
- `Timer.seconds`: the sleep length, documented as non-negative and finite.

## EffectAnswer

An [`EffectAnswer`] answers one [`Effect`] with the variant of its own kind, or with `Dropped`; every effect takes exactly one answer. Pass it to [`Run::resume`](crate::Run::resume) for each issued effect. Answer an effect you abandon, as for a cancelled run or a task that ended first, with `Dropped`, because a drop counts as its one answer. A chain still waiting on a dropped effect resumes with a cancelled error. [Answer every kind of effect](#answer-every-kind-of-effect) teaches it.

| Variant | What it answers with |
|---|---|
| `Chat` | the round's boxed [`Completion`](crate::model::Completion) or its [`CompletionError`](crate::model::CompletionError) |
| [`ToolCall`](EffectAnswer::ToolCall) | the tool's own output or failure, before the run's trust and count rules apply |
| `Vfs` | the operation's [`VfsOutcome`](crate::vfs::VfsOutcome) or the store's structured failure |
| `Timer` | nothing; the timer fired |
| `Dropped` | nothing; you dropped the effect without performing it |

- `record`: returns the [`AnswerRecord`] your log keeps, with each failure as display text, and a completion without its bodies or metrics.

## EffectRecord

An [`EffectRecord`] is an [`Effect`] minus its live handles, in the form a run log stores and a replay compares. Get one from [`Effect::record`]. The record holds no id, so keep the [`EffectId`] beside it when you need one. [Log effects and answers](#log-effects-and-answers) teaches it.

| Variant | What it records |
|---|---|
| `Chat` | the round's model name, alias, wire-form messages, tool names, and frozen invocation settings |
| [`ToolCall`](EffectRecord::ToolCall) | the effect's tool identity, alias, arguments, and origin, copied unchanged |
| `Vfs` | the validated operation, such as `{ "Vfs": { "op": { "Write": { "path": "notes.md", "contents": "kept" } } } }` |
| `Timer` | the sleep length in seconds |

- `Chat.model`: the bound model's name, which can differ from the served model that [`ChatAnswerRecord`] records.
- `Chat.messages`: one wire-form JSON value per request message.
- `Chat.tools`: the advertised tool names only, in schema order; a round with no tools records `[]`.
- `Chat.temperature`: the frozen sampling temperature, or `None` when the binding declared none; `max_tokens` and `thinking` work the same way.

## ToolCaller

[`ToolCaller`] says which kind of code asked for one tool call: `Script` when the section's own Lua called the tool through `tools.call`, and `Model` when a model round requested it. Read it from the `caller` of a [`ToolCallOrigin`] to attribute a call or choose a policy. It serializes in snake case, as `"script"` or `"model"`. The enum is non-exhaustive, so your `match` needs a wildcard arm. [Answer every kind of effect](#answer-every-kind-of-effect) introduces it.
