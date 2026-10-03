This crate lets your program run PromptForge prompts, one [`Harness`] per run.

Your program builds a Harness for each run, hands it the recorder, the broker that reaches its models, the timer, and the capabilities and services the run may use, then awaits [`Harness::run`]. That is the whole job of a Host. The Harness drives the run, and your program owns everything around it: the models, the clock, the person at the screen, and when to stop.

By the end of this page you will have built `desk`, a Host that runs prompts for one person. Each tour adds one idea: run a prompt and read its output, answer the operator by supplying an input broker, stop a round and cancel a run, and stream from your own broker. A real `desk` passes `harness_gateway_client::GatewayBroker`, which reaches the PromptForge Gateway. The examples on this page compile against this crate alone, so their hidden lines define `Offline`, a broker that lists no model and refuses every round, and `Clock`, a timer that sleeps on tokio. Where a tour needs a model, its hidden lines list `stub-model`, and a model that answers writes `You said: ` and the last message.

# Before you start

A PromptForge prompt is a Markdown file. Its Lua code holds the logic, and its prose holds text for a model. The [PromptForge language guide](https://cppalliance.github.io/promptforge/language/) teaches how to write one. This page uses a few words for the things a Host deals with:

- One execution of a prompt, from its start to its end, is a *run*. One Harness drives one run.
- The object your program hands the Harness to answer every model call the run makes, and to list the models the run can bind, is the *broker*. One model call is a *round*.
- The object that takes a record of every step the run makes is the *recorder*.
- The object the run sleeps on, for a timed wait, is the *timer*.
- The person your program puts in front of a run to answer its questions is the *operator*.
- One small part of a reply, sent while the model is still writing, is a *piece*.

A run never reaches the outside world by itself. When it needs outside work done, it asks the Harness, and the Harness does that work through what your program handed it: a model round through the broker, a tool call through the capabilities, a question to the operator through your input broker, a sleep through the timer, and a read or write of a file in the run's [store](vfs) inline. Every record the run makes goes to the recorder.

# Run a prompt

You have a prompt and you want your program to run it and read its answer. Your program builds a Harness, hands it the prompt's text in a [`RunRequest`], and awaits the [`RunReport`].

Running a prompt feels like awaiting an async function: you build the call, await it, and read what it returns. Unlike a plain function, every outside step the run takes goes through the objects you handed the Harness, and nothing else.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{Harness, HostSnapshot, RunRequest};
use std::sync::Arc;
# use harness::{BoxFuture, InferenceBroker, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
# struct Offline;
# impl InferenceBroker for Offline {
#     fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#         Box::pin(async { Ok(ModelCatalog::empty()) })
#     }
#     fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: promptforge::effect::Round) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#         let kind = CompletionErrorKind::Unavailable;
#         Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
#     }
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {

// 1. desk's `greet` prompt: it reads the line in `name.txt` and writes its greeting to `reply.txt`.
let greet = concat!(
    "---\nname: greet\ndescription: Greets the operator by name\npromptforge: 0\n",
    "input: { path: name.txt, description: The operator's name }\n",
    "output: { path: reply.txt, description: The greeting }\n",
    "---\n\n# Greet\n\n## Answer\n\n```lua\n",
    "store.write('reply.txt', 'Hello, ' .. store.read('name.txt') .. '.')\n",
    "```\n",
);

// 2. Build one Harness for this run from desk's recorder, broker, timer, capabilities, and services.
let recorder = Arc::new(MemoryRecorder::new());
let harness = Harness::new(recorder.clone(), Arc::new(Offline), Arc::new(Clock), CapabilityRegistry::new(), HostServices::new());

// 3. Name the run, hand it the prompt's text and the operator's name, and give it a fresh store.
let request = RunRequest {
    name: "desk-greet-1".into(),
    source: greet.into(),
    args: String::new(),
    input_text: Some("desk".into()),
    vfs: VfsRef::default(),
    host: HostSnapshot::default(),
};

// 4. Run it to its end; the Harness is spent.
let report = harness.run(request).await?;

// 5. The report says how the run ended and what it left at its output file.
assert!(matches!(report.outcome, RunOutcome::Completed { .. }));
assert_eq!(report.output?, "Hello, desk.");
assert!(recorder.outcome(report.run_id.ok_or("the run began")?).is_some());
# Ok(())
# }
````

1. Step 1 writes `greet` as a string. Its frontmatter holds the three keys every prompt needs, `name`, `description`, and `promptforge: 0`, plus the files it reads and writes: `input:` names the file it reads, and `output:` the file it writes. The source is built with `concat!` so that rustdoc keeps its `# Greet` line. The source text is the Harness's one prompt input: your program reads it from wherever it keeps prompts.
2. Step 2 builds a Harness with [`Harness::new`]. It takes the recorder as an `Arc<dyn RunRecorder>`, here a [`MemoryRecorder`](record::MemoryRecorder) that keeps the run's records in memory, the broker as an [`InferenceBroker`], the timer as a [`Timer`], a [`CapabilityRegistry`](capability::CapabilityRegistry) of the capabilities the prompt may declare, and the [`HostServices`](capability::HostServices) those capabilities read. `greet` makes no model round, so the hidden `Offline` broker is enough, and it declares no capability. Building a Harness touches nothing and calls no broker.
3. Step 3 builds the [`RunRequest`]. `name` names the run in every record and event, `source` is the prompt's text, `args` is the run's argument text, and `input_text` is written at the prompt's declared input file before the run starts. `vfs` is the run's whole filesystem, here a fresh memory store; [Files](vfs) shows how to give a run files of your own. `host` is the [`HostSnapshot`]: the operator's selected model and granted workspace roots, both empty here.
4. Step 4 awaits [`Harness::run`], which consumes the Harness. The future needs no runtime of its own and starts no task, so any executor can drive it. Build a new Harness for each run; the objects you hand it are shared behind `Arc`s and clone cheaply.
5. Step 5 reads the [`RunReport`]. `outcome` is how the run ended, `output` is what a completed run left at its declared output file, and `run_id` is the id the recorder issued. The run's model comes from the broker: as it starts, the run asks the broker for its model list and binds the selected model from it, or the first model listed when nothing is selected.

A prompt that does not parse, an input file that cannot be put in place, and a capability the registry lacks each end the run as failed, reported in `outcome`. [`Harness::run`] returns an error only when the run cannot be driven at all: [`HarnessError::Model`] when the broker cannot list its models or lacks the selected one, [`HarnessError::Recorder`] when the recorder refuses a write, and [`HarnessError::Stalled`] when the run waits with nothing in flight.

You might expect one Harness to serve every run your program makes, like a client you build once. Instead, each run gets its own, and owns its own state, so runs never share anything you did not share yourself.

Build a Harness, hand it a request, await the report. Next, [Answer the operator](#answer-the-operator) lets a run ask a person a question.

# Answer the operator

The prompt asks the operator a question, and your program must carry it to a person and bring back what they type. The Harness waits on an *input broker* your program supplies, and the run pauses that part of the prompt until the broker answers.

An input broker feels like an async function your program implements: the Harness calls it, awaits it, and hands the text to the prompt. Unlike a callback you register, it travels with the run's services, so each run reaches the operator your program bound it to.

````
use async_trait::async_trait;
use harness::capability::{CapabilityRegistry, HostServices, INPUT_BROKER, InputBroker, InputError, UserInput};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{Harness, HostSnapshot, RunRequest};
use std::sync::Arc;
# use harness::{BoxFuture, InferenceBroker, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
# struct Offline;
# impl InferenceBroker for Offline {
#     fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#         Box::pin(async { Ok(ModelCatalog::empty()) })
#     }
#     fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: promptforge::effect::Round) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#         let kind = CompletionErrorKind::Unavailable;
#         Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
#     }
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {

// 1. desk's input broker: here the operator always types the same text.
struct Operator(&'static str);

#[async_trait]
impl InputBroker for Operator {
    async fn wait(&self) -> Result<String, InputError> {
        Ok(self.0.to_owned())
    }
}

// 2. Register the capability that gives a prompt `input.ask()`, and supply the broker as a service.
let mut capabilities = CapabilityRegistry::new();
capabilities.register(Arc::new(UserInput::new()))?;
let mut services = HostServices::new();
let operator: Arc<dyn InputBroker> = Arc::new(Operator("Hello, desk."));
services.provide(&INPUT_BROKER, operator)?;

// 3. A prompt that asks once and returns the answer.
let asks = concat!(
    "---\nname: asks\ndescription: Asks the operator once\npromptforge: 0\n",
    "capabilities:\n  - promptforge/user-input\n",
    "---\n\n# Asks\n\n## Only\n\n```lua\n",
    "return input.ask()\n",
    "```\n",
);

// 4. Run it: the answer comes back byte for byte.
let harness = Harness::new(Arc::new(MemoryRecorder::new()), Arc::new(Offline), Arc::new(Clock), capabilities, services);
let request = RunRequest {
    name: "desk-asks-1".into(),
    source: asks.into(),
    args: String::new(),
    input_text: None,
    vfs: VfsRef::default(),
    host: HostSnapshot::default(),
};
let report = harness.run(request).await?;
assert_eq!(report.outcome, RunOutcome::Completed { final_text: "Hello, desk.".into() });
# Ok(())
# }
````

1. Step 1 implements [`InputBroker`](capability::InputBroker) for `Operator`. Its one method, `wait`, returns the operator's next message byte for byte, or an [`InputError`](capability::InputError) when nobody will answer; the prompt sees that error at its `input.ask()` call. A real `desk` shows the question in its window and resolves `wait` when the operator presses Enter.
2. Step 2 registers [`UserInput`](capability::UserInput), the `promptforge/user-input` capability, and provides the broker under [`INPUT_BROKER`](capability::INPUT_BROKER). A Host supplies a fresh input broker for each run by cloning its base services and providing the broker on the clone; the base services leave `INPUT_BROKER` empty, because [`HostServices::provide`](capability::HostServices::provide) refuses a second provider for the same id.
3. Step 3 writes `asks`, which declares `promptforge/user-input` under `capabilities:` and calls `input.ask()`.
4. Step 4 runs `asks` and asserts that the operator's text is the run's final text.

The model can ask the operator too. The prompt's `tools:` frontmatter maps an alias to [`USER_INPUT_ASK_TOOL`], as in `ask: promptforge/user-input/ask`, and its Lua offers the alias with `tools.add` or `tools.always`. Both paths wait on the same input broker.

A question to the operator is the one effect a stop leaves alone, as the next tour shows, so a person mid-answer never loses the prompt they are typing into. A cancel drops it: the Harness drops the future `wait` returned, so a broker that shows a prompt clears it when its future is dropped.

You might expect the Harness to bring its own way to reach a person. Instead, it has none: a prompt that requires `promptforge/user-input` on a Host with no input broker is refused as it prepares, and an optional one reads `input.connected()` as `false`.

Supply the broker, register the capability, and the run asks through you. Next, [Stop a round and cancel a run](#stop-a-round-and-cancel-a-run) cuts a run's work short.

# Stop a round and cancel a run

The model is stuck or heading the wrong way, and the operator wants to stop this answer but keep the run. Or the operator is done, and wants the run gone. A [`RunControl`] does both: [`RunControl::stop_round`] drops the work in flight and keeps the run going, and [`RunControl::cancel`] ends the run.

A `RunControl` feels like a pair of [`AbortHandle`](https://docs.rs/futures/latest/futures/future/struct.AbortHandle.html)s for the run's future. Unlike aborting a future, the run hears about it: each dropped step is answered `Dropped`, and the prompt decides what that means.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{MemoryRecorder, RunOutcome};
use harness::vfs::VfsRef;
use harness::{Harness, HostSnapshot, RunRequest};
use std::sync::Arc;
# use harness::{BoxFuture, InferenceBroker, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog, ModelDescriptor, ModelId, ThinkingMode, ToolSchema};
# struct Stuck;
# impl InferenceBroker for Stuck {
#     fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#         let model = ModelId::gateway("stub-model").map(|id| ModelDescriptor::new(id, "stub", std::num::NonZeroU32::MIN.saturating_add(131_071), ThinkingMode::Never));
#         Box::pin(async move { Ok(ModelCatalog::new(model.into_iter()).unwrap_or_else(|_| ModelCatalog::empty())) })
#     }
#     fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: promptforge::effect::Round) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#         Box::pin(std::future::pending())
#     }
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }
# fn desk() -> Harness {
#     Harness::new(Arc::new(MemoryRecorder::new()), Arc::new(Stuck), Arc::new(Clock), CapabilityRegistry::new(), HostServices::new())
# }
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {

// 1. A prompt whose one model round sits under a `pcall`; the hidden broker never answers it.
let patient = concat!(
    "---\nname: patient\ndescription: Waits on the model\npromptforge: 0\n",
    "models: { writer: {} }\n",
    "---\n\n# Patient\n\n## Only\n\n```lua\n",
    "local ok, err = pcall(models.infer, writer, 'Take your time.')\n",
    "return ok and 'answered' or err.kind\n",
    "```\n",
);
let request = || RunRequest {
    name: "desk-patient".into(),
    source: patient.into(),
    args: String::new(),
    input_text: None,
    vfs: VfsRef::default(),
    host: HostSnapshot::default(),
};

// 2. Take the control before the run, then stop the round: the pcall catches the dropped call.
let harness = desk();
let control = harness.control();
control.stop_round();
let report = harness.run(request()).await?;
assert_eq!(report.outcome, RunOutcome::Completed { final_text: "cancelled".into() });

// 3. Cancel a run before it begins: it ends cancelled, and the recorder never hears of it.
let harness = desk();
harness.control().cancel();
let report = harness.run(request()).await?;
assert_eq!(report.outcome, RunOutcome::Cancelled);
assert_eq!(report.run_id, None);
# Ok(())
# }
````

1. Step 1 writes `patient`, whose one round runs under a `pcall`, Lua's protected call, so the prompt sees a failed call as a value instead of an error. The hidden `Stuck` broker lists `stub-model` and never answers a round, and the hidden `desk` builds a Harness over it.
2. Step 2 takes the run's control with [`Harness::control`] before it calls `run`, which consumes the Harness, and raises a stop. A stop raised while nothing is in flight drops what is in flight when the run next waits, here the stuck round, so the example raises it before the run starts. A real `desk` keeps the control in its window and raises the stop from there while the round runs. The dropped round reaches the prompt as an error whose `kind` is `cancelled`, the `pcall` catches it, and the run goes on to its end. An uncaught one would end the run with the outcome `Cancelled`.
3. Step 3 cancels before the run begins. The run ends `Cancelled` with no `run_id`, because the recorder never began it. A cancel while the run is going answers every effect in flight `Dropped`, a question to the operator included, and ends the run `Cancelled`.

A stop drops every effect in flight except a question to the operator: model rounds, tool calls, and timers. An open question stays open, so the operator can still answer it. The run's cancel flag stays clear, and the next round starts fresh.

`RunControl` is cheap to clone, every clone steers the same run, and both calls take effect from any thread. Calling `cancel` again does nothing.

You might expect a stop to end the turn and start the prompt over, the way aborting a task ends it. Instead, the same run goes on, with every value its Lua held, and the prompt's own `pcall` decides whether a dropped step matters.

Stop a round to keep the run; cancel to end it. Next, [Stream from your own broker](#stream-from-your-own-broker) shows a reply while the model writes it.

# Stream from your own broker

You want the operator to watch a reply appear as the model writes it. The Harness never streams: it hands every round to your broker and takes the finished reply. Your broker streams the pieces wherever your program shows them, and the round's id pairs the pieces with the finished reply the run records.

Streaming from your broker feels like teeing a stream: the pieces go to your window as they arrive, and the whole reply goes back to the run. Unlike a tee the Harness sets up, your broker decides which rounds stream, because only your program knows which ones the operator is watching.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{MemoryRecorder, RecordKind};
use harness::vfs::VfsRef;
use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, RunRequest};
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::model::{Completion, CompletionError, CompletionOptions, CompletionResult, Message, ModelBinding, ModelCatalog, ToolSchema};
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
# use harness::Timer;
# use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
# fn stub_models() -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#     let model = ModelId::gateway("stub-model").map(|id| ModelDescriptor::new(id, "stub", std::num::NonZeroU32::MIN.saturating_add(131_071), ThinkingMode::Never));
#     Box::pin(async move { Ok(ModelCatalog::new(model.into_iter()).unwrap_or_else(|_| ModelCatalog::empty())) })
# }
# fn model_writes(messages: &[Message]) -> Vec<String> {
#     let said = messages.last().map(|message| message.content().to_owned()).unwrap_or_default();
#     vec!["You said: ".to_owned(), said]
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {

// 1. desk's broker talks to its model and holds its own sender to desk's window.
struct Streaming {
    window: Sender<(u64, String)>,
}

impl InferenceBroker for Streaming {
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
        stub_models()
    }

    fn chat(&self, _: ModelBinding, messages: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, round: Round) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
        // 2. Stream only a section's own rounds, each piece under its round's id.
        let window = (round.origin == ReplyOrigin::Chat).then(|| self.window.clone());
        Box::pin(async move {
            let mut reply = String::new();
            for piece in model_writes(&messages) {
                if let Some(window) = &window {
                    let _ = window.send((round.id.get(), piece.clone()));
                }
                reply.push_str(&piece);
            }
            Completion::from_result(CompletionResult::Text(reply), "stub-model").map(Box::new)
        })
    }
}

// 3. A prompt with one section round.
let chats = concat!(
    "---\nname: chats\ndescription: Says hello to the model\npromptforge: 0\n",
    "models: { writer: {} }\n",
    "---\n\n# Chats\n\n## Only\n\n```lua\n",
    "local msgs = messages.new()\n",
    "msgs:user('Hello, desk.')\n",
    "models.loop(writer, msgs)\n",
    "return msgs[#msgs].content\n",
    "```\n",
);

// 4. Run it over the streaming broker.
let (window, shown) = channel();
let recorder = Arc::new(MemoryRecorder::new());
let harness = Harness::new(recorder.clone(), Arc::new(Streaming { window }), Arc::new(Clock), CapabilityRegistry::new(), HostServices::new());
let request = RunRequest {
    name: "desk-chats-1".into(),
    source: chats.into(),
    args: String::new(),
    input_text: None,
    vfs: VfsRef::default(),
    host: HostSnapshot::default(),
};
let report = harness.run(request).await?;

// 5. The finished reply the run recorded carries the round id its pieces carried.
let records = recorder.records(report.run_id.ok_or("the run began")?);
let reply = records
    .iter()
    .filter(|record| record.kind == RecordKind::Event)
    .find(|record| record.payload["kind"] == "assistant_reply")
    .ok_or("the run recorded its reply")?;
let round = reply.payload["round"].as_u64().ok_or("a round id")?;
let shown: Vec<(u64, String)> = shown.try_iter().collect();
assert_eq!(shown, [(round, "You said: ".to_owned()), (round, "Hello, desk.".to_owned())]);
# Ok(())
# }
````

1. Step 1 defines `Streaming`, the broker that talks to `desk`'s model, with its own `window` sender standing in for `desk`'s window. Here the hidden `stub_models` lists `stub-model`, and the hidden `model_writes` stands in for the model, writing `You said: ` and the last message in two pieces. A real `desk` serves its rounds through `harness_gateway_client::GatewayBroker`, whose `chat_streaming` hands each piece to a callback of `desk`'s.
2. Step 2 takes a sender only for a round whose [`Round`](promptforge::effect::Round) has the origin [`ReplyOrigin::Chat`](promptforge::event::ReplyOrigin::Chat): a section's own round, the one the operator watches. A nested `models.infer` round has the origin `Infer`, and `desk` reads it whole from its answer. Each piece goes out under `round.id`, the run-wide number of the round, and the whole reply goes back to the run.
3. Step 3 writes `chats`, whose one section sends one round through `models.loop`.
4. Step 4 runs `chats` over `Streaming`, recording into a memory recorder.
5. Step 5 finds the `assistant_reply` event in the run's records and asserts that the pieces `desk` showed carry the round id that event holds. The run's thinking, reply, and tool-call events each carry their round's id as `round`, so `desk` swaps its streamed preview for the finished text by that number.

Send each piece as it arrives, without blocking, because the Harness polls the round inside the run's future. A missed piece costs the operator a moment of streaming, never text, because the finished reply holds it all.

You might expect the Harness to hand you a stream of pieces. Instead, your broker sees every piece first, because it is the one talking to the model, and the run sees only the finished reply.

Stream where the operator watches, and let the recorded reply have the last word. [Where to go next](#where-to-go-next) lists the module pages.

# Reference

## BoxFuture

[`BoxFuture`] is the boxed, sendable, `'static` future every [`InferenceBroker`] and [`Timer`] method returns. Build one with `Box::pin(async move { ... })`, and move what the future needs into it, because the Harness polls it inside the run's future after the call returns. [Run a prompt](#run-a-prompt) shows one in the hidden `Offline` broker.

## CurrentModelError

[`CurrentModelError`] says why a run could not bind its model as it starts, inside [`HarnessError::Model`]. `CatalogFetchFailed` holds the broker's failure to list its models, and `SelectionAbsent` names a selected model the list lacks. The Harness never binds a model it made up instead.

## Harness

[`Harness`] drives one run for your program. Build it with [`Harness::new`] from an `Arc<dyn RunRecorder>`, an `Arc<dyn InferenceBroker>`, an `Arc<dyn Timer>`, a [`CapabilityRegistry`](capability::CapabilityRegistry), and [`HostServices`](capability::HostServices); building one touches nothing. Take its [`RunControl`] with [`Harness::control`], then await [`Harness::run`], which consumes it. [Run a prompt](#run-a-prompt) teaches this.

- [`Harness::run`]: resolves the run's model through the broker, prepares the source, drives the run, and reads the declared output file of a completed run, all inside one `Send` future that starts no task.
- A cancel before `run`, or while the broker lists its models, reports `Cancelled` with no `run_id`.

## HarnessError

[`HarnessError`] says why [`Harness::run`] could not drive a run to an outcome of its own. Show it through [`display_chain`], so that its cause is in the text. Match it with a wildcard arm, because it may gain variants.

| Variant | Meaning |
|---|---|
| `Model` | The broker could not list its models, or lacks the selected one; the run never began. |
| `Recorder` | The recorder refused a write; `run` is the run it issued, `None` when it refused to begin one. |
| `Stalled` | The run waited with nothing in flight, which means the Harness lost an effect. |

## HostSnapshot

[`HostSnapshot`] carries your program's selected model and granted workspace roots into a run, in its [`RunRequest`]. The run reads it once, as it starts, so a new selection reaches the next run, never a run in progress. With no selection, a run binds the first model its broker lists. [Run a prompt](#run-a-prompt) teaches this.

- `selected_model`: never swapped for another; a model the broker's list lacks fails the run with [`HarnessError::Model`].
- `workspace_roots`: only the first root reaches the prompt's `ui()` global.
- [`HostSnapshot::ui`]: returns `{ "selected_model", "workspace_root" }`, each `null` when absent.

## InferenceBroker

[`InferenceBroker`] is the trait your program implements, or takes from a crate, to give a run its models. Pass it to [`Harness::new`] as an `Arc<dyn InferenceBroker>`. The Harness polls each call inside the run's future, so a broker must not block while polled; blocking or CPU-heavy work goes to your own runtime.

- `models`: lists the models the broker serves. A run calls it once as it starts.
- `chat`: performs one round over the messages with the tools advertised, under the round's options. An error it returns fails that round, and the prompt receives it.
- `round`: `chat`'s [`Round`](promptforge::effect::Round), the round's run-wide id and the path that dispatched it; the round's events carry the same id. The Harness takes only the finished reply; [Stream from your own broker](#stream-from-your-own-broker) shows a broker that streams a round's pieces on its own.

## OutputError

[`OutputError`] says why a [`RunReport`]'s `output` holds no text. A missing output never fails the run, so check it apart from the outcome.

| Variant | Meaning |
|---|---|
| `NotCompleted` | The run did not complete, so its output file was not read. |
| `Undeclared` | The prompt declares no `output:` file. |
| `Missing` | The run completed without writing its declared output file; `path` holds that path. |
| `Vfs` | The store refused the read of the output file at `path`; `source` holds the store's failure. |

## RunControl

[`RunControl`] steers one run from outside its future. Take it with [`Harness::control`] before [`Harness::run`]. Every clone steers the same run. [Stop a round and cancel a run](#stop-a-round-and-cancel-a-run) teaches this.

- [`RunControl::stop_round`]: drops every effect in flight except a question to the operator, and answers each `Dropped`; the run goes on.
- [`RunControl::cancel`]: drops every effect in flight and ends the run `Cancelled`. Idempotent.

## RunReport

[`RunReport`] is how a run ended. [Run a prompt](#run-a-prompt) reads one.

- `run_id`: the run's id at the recorder; `None` when a cancel ended the run before the recorder began it.
- `outcome`: how the run ended, as the recorder was told.
- `output`: the text a completed run left at its declared output file, or an [`OutputError`].

## RunRequest

[`RunRequest`] is what to run. Build it as a struct literal and pass it to [`Harness::run`]. [Run a prompt](#run-a-prompt) teaches this.

- `name`: the run's name, every event's `execution` and the run metadata's `name`.
- `source`: the prompt's Markdown text.
- `args`: the run's argument text, handed to the prompt as `args`.
- `input_text`: written at the prompt's declared `input:` file before the run; with `None`, the store must already hold that file.
- `vfs`: the run's whole filesystem; [Files](vfs) teaches it.
- `host`: the [`HostSnapshot`] the run reads as it starts.

## Timer

[`Timer`] is the trait your program implements to give a run its clock. A run sleeps on it for a timed wait. Return a future that resolves once the seconds have passed, on your own runtime; dropping it must tear the sleep down.

## USER_INPUT_ASK_TOOL

[`USER_INPUT_ASK_TOOL`] is the id of the tool that asks the operator. Bind it under an alias in `tools:`, such as `ask: promptforge/user-input/ask`, and offer the alias with `tools.add` or `tools.always`. A script's `input.ask()` calls the same tool. [Answer the operator](#answer-the-operator) teaches this.

## display_chain

[`display_chain`] renders an error and every cause in its `source()` chain as one line, joined with `: `. Use it to show a Harness error to a person or a model, because an error's `Display` holds only its own message.

# Where to go next

- [`capability`]: install the capabilities your prompts declare, and provide the services they read.
- [`record`]: record every run to a store of your own, and read a run back.
- [`vfs`]: give a run files of your own instead of a fresh empty store.
- [`cancel`]: stop your program's own async work at its next safe point.
