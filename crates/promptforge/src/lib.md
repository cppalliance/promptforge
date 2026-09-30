This crate parses PromptForge prompt files and runs them from your Rust program, which does their outside work.

You hand it a prompt file, and it hands you back the prompt's result. On the way, the prompt stops each time it needs something from outside, such as a model reply, a tool's output, or a file. The core idea is one loop: the prompt asks for outside work, and your program does the work and answers. By the end of this page, you will have built `greeter`, a small program that runs one prompt file and checks its result. Every model and tool answer in it is canned, so each example runs offline.

# Before you start

A PromptForge prompt is a Markdown file that is also a program. It opens with YAML frontmatter, has exactly one `#` title, and holds `##` headings under the title. Each `##` heading, with the text and Lua under it, is a *section*, and sections run in file order. The `lua` blocks hold the logic, and the prose holds text for a model. The [PromptForge user guide](https://cppalliance.github.io/promptforge/) teaches the prompt language in full.

A prompt does no I/O of its own, so its model calls, tool calls, and file reads and writes all come to your program as work to do. Each piece of that outside work is an *effect*, and the program that runs the prompt and does that work is the *host*. One execution of one prompt, from its start to its result, is a *run*, and a record of something that happened during it is an *event*. The files a prompt reads and writes live in its [store](vfs), a set of virtual files that every section shares.

Here is the smallest prompt that runs, the one every tour builds on.

````
use promptforge::Prompt;

let source = concat!(
    // 1. The frontmatter names the prompt, describes it, and declares format version 0.
    "---\n",
    "name: greeter\n",
    "description: Writes a note to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    // 2. The one `#` heading is the prompt's title.
    "# Greeter\n\n",
    // 3. The `##` section's Lua writes a note to the store, reads it back, and returns it.
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "return store.read('note.md')\n",
    "```\n",
);

// 4. The text parses as a prompt, and its title is the `#` heading's text.
let (parsed, _events) = Prompt::parse(source, "greeter");
assert!(parsed.is_ok_and(|prompt| prompt.title() == "Greeter"));
````

1. The frontmatter holds the three keys a runnable prompt carries: `name`, `description`, and `promptforge: 0`. The version line marks the file as a PromptForge prompt.
2. The single `#` line is the title, `Greeter`. A prompt has exactly one title.
3. The `##` section's `lua` block writes the note `hello` to the store, reads it back, and returns it. The logic is Lua, and each store call is outside work your program will do.
4. [`Prompt::parse`] accepts the text and reports the title `Greeter`. That proves the file is a valid prompt before anything runs.

# Run a prompt

You have a prompt file, and you want its result from your Rust program.

First, one idea: a run never reaches outside itself, and it does no I/O and reads no clock. Whenever it needs outside work, even a store read, it stops and hands you an effect, so your program decides how every model call, tool call, and file access happens. Because the run also has no clock or randomness of its own, a run given the same seed, start time, and answers repeats exactly. That is why the greeter can run offline with canned answers.

Stepping a run feels like polling a future: [`Run::step`] returns [`Step::Pending`] until it returns [`Step::Done`] with the result. Unlike a future, a pending run hands you the work it is waiting on, and nothing moves until you do that work and answer.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_store_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "return store.read('note.md')\n",
#     "```\n",
# );
// 1. Parse the greeter from Before you start once, naming this execution for its events.
let (parsed, _parse_events) = Prompt::parse(source, "greeter");
let prompt = Arc::new(parsed?);

// 2. Build the context with a fixed seed and start time, and create the run.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
let mut run = Run::new(prompt, "", ctx);

// 3. Answer each store effect from the run's in-memory store.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
        _ => EffectAnswer::Dropped,
    }
}

// 4. Step the run, and answer each effect it hands you, until it is done.
let result = loop {
    match run.step() {
        Step::Pending { effects, .. } => {
            for (id, _provenance, effect) in effects {
                run.resume(id, answer(effect));
            }
        }
        Step::Done { result, .. } => break result,
    }
};
assert!(matches!(result, RunResult::Ok(text) if text == "hello"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. Pass [`Prompt::parse`] the file's text and an identifier for this execution, such as a session id or the file's name. It stamps that identifier on every parse event and never opens it as a path, so parsing opens no file. It returns the parse result beside the parse events, which this example sets aside.
2. Step 2 builds the context and then the run.
   - [`RunContext::new`] takes the run's name, which the run stamps on every event, then the seed and the start time. The fixed seed `7` and [`Timestamp::UNIX_EPOCH`](timestamp::Timestamp::UNIX_EPOCH) make this test repeat exactly. A live program passes a seed from a secure random source and the current time instead, because a secret seed keeps the wrapping around [untrusted output](tools) unguessable.
   - [`Run::new`] takes the prompt in an [`Arc`](std::sync::Arc), the argument string that reaches Lua as `args`, here empty, and the context. Creating a run cannot fail.
3. `answer` performs each [`Effect::Store`](effect::Effect::Store) with [`perform_store_op`](vfs::perform_store_op) against the context's default in-memory store, and drops any other kind. Your program, not the run, does the store work. The greeter issues only store effects, so the last arm never runs here. A host answers every kind of effect its prompts use, and keeps [`EffectAnswer::Dropped`](effect::EffectAnswer::Dropped) for work it gives up on. [Stop a run](#stop-a-run) shows what a drop does.
4. The loop calls `step`, hands each answer to [`Run::resume`] under its effect's id, and stops only at `Step::Done`. Each effect also comes with its *provenance*: the [task](ids) that issued it and that task's position in its own order. The greeter ignores it, and [Group a log by task](ids#group-a-log-by-task) explains provenance in full. The result `hello` proves the note went out and came back through two store effects.

When the file has a mistake, such as a missing title or a Lua syntax error, `Prompt::parse` returns a [`ParseError`] of kind [`ParseErrorKind::Structure`] or [`ParseErrorKind::Lua`], with no line or column. Show the user the error's message, because it tells the author what is wrong and, for Lua, which block it is in.

Check the first step's result as closely as the parse result. A file with no `promptforge:` version still parses. Its first step returns `Step::Done` with a [`RunResult::Failure`] of kind [`RunErrorKind::Parse`], or [`RunErrorKind::Version`] for an unsupported version. `Run::new` never fails, so the run reports these problems from its first step.

You might expect `Run::step` to run the prompt to its end, the way a function call would. Instead, it returns at the first piece of outside work, even a store read, and the run waits until you answer.

Parse once, then step and answer until `Done`. Next, [Answer a model](#answer-a-model) adds a model call to the same prompt.

# Answer a model

Your prompt asks a model for a reply, and your program must supply that model and answer the call.

A prompt never names a concrete model. It names what it needs under a label of its own, such as `writer`, and that label is a *model role*. Before the run starts, one step matches your model to each role the prompt declares and writes the match into the context. That step is *prepare*, and the run can use only what the context holds.

A model role works like a generic parameter with a trait bound: the prompt names what it needs, such as a minimum context size, and you supply a concrete model. Unlike the compiler, prepare checks the bound at run time and reports a mismatch instead of refusing to build.

````
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_store_op;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::Environment;

// 1. The greeter declares the role `writer`, selects it, and returns its reply to the note.
let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note and asks a model to reply to it.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "return models.infer(store.read('note.md'))\n",
    "```\n",
);
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;

// 2. Describe one canned model, and set it on the context before you prepare.
let id = ModelId::gateway("canned")?;
let window = std::num::NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
let model = ModelDescriptor::new(id, "Always replies hi there", window, ThinkingMode::Never);
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);

// 3. Prepare, confirm there is no refusal, and run the context that prepare returned.
let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
assert!(requirements.refusal().is_none());
let mut run = Run::new(Arc::new(prompt), "", ctx);

// 4. Add a chat arm that answers with the canned reply, and drive the run as before.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
        Effect::Chat { .. } => {
            let reply = CompletionResult::Text("hi there".to_owned());
            EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new).map_err(Into::into))
        }
        _ => EffectAnswer::Dropped,
    }
}
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 run.resume(id, answer(effect));
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };
assert!(matches!(result, RunResult::Ok(text) if text == "hi there"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The frontmatter declares the role `writer` under `models:`. The Lua selects it with `models.use` and returns the reply `models.infer` gets for the note. The prompt names a role, never a concrete model.
2. A [`ModelDescriptor`](model::ModelDescriptor) takes a gateway id, a description, a context window of 8192 tokens, and [`ThinkingMode::Never`](model::ThinkingMode::Never). [`RunContext::model`] puts it on the context before you call [`Environment::prepare`]. Prepare reads the model once and fills every role the prompt declares with it, so a model set afterwards fills nothing. That also means two roles can't get different models: prepare fills every role with the context's one model, so all of a prompt's roles share it.
3. Prepare returns a new context and a [`Requirements`] report. Before you create the run, call [`Requirements::refusal`] on the report. When it returns an error, show that error and do not run the prompt. Prepare itself never fails, and the error's text lists each gap in the report, one line per gap. That text is the *requirements notice*. It also lists gaps your own program merged into the report, such as missing services and conflicts between [capabilities](capabilities), which prepare never finds itself. Then pass the context that prepare returned to [`Run::new`], not the one you built, because a context that skipped prepare runs with no models and no tools.
4. `answer` gains an arm for [`Effect::Chat`](effect::Effect::Chat) that answers with a [`Completion`](model::Completion) built from the text `hi there`, and the loop from [Run a prompt](#run-a-prompt) drives the run. [`Completion::from_result`](model::Completion::from_result) returns a `Result`, because it rejects an empty tool-call batch, or one where two calls share an id, but never text. `.map(Box::new)` is there because [`EffectAnswer::Chat`](effect::EffectAnswer::Chat) holds a boxed completion. A completion carries the request and response bodies, so the box keeps it from setting the size of every other answer. `.map_err(Into::into)` turns the constructor's error into the [`CompletionError`](model::CompletionError) that the answer expects. The result `hi there` proves the canned reply to the chat effect became the run's result.

When a role's `min_context`, its minimum context size, is larger than your model's context window, the report holds an [`UnmetRequirement`] for that role with the check [`RequirementCheck::ContextMinimum`]. A window exactly equal to `min_context` already passes. Set a model with a larger window and prepare again, because prepare never looks for another model on its own.

Set a model whenever the prompt declares a role. With none, prepare fills and checks nothing, reports success, and the run fails later, when a section selects the role. A clean report does not prove your roles are filled.

You might expect `Run::new` to match the prompt's roles to your model on its own. Instead, only `Environment::prepare` does that, and a run built from a context that skipped prepare has no models at all.

Set the model, prepare, check the refusal, then run. Next, [Call a tool](#call-a-tool) gives the greeter a tool to call.

# Call a tool

Your prompt needs a tool, and your program must offer that tool and run it when the prompt calls.

The run sees only a description of each tool. When the prompt calls one, the run hands the call to your program, which runs the tool and answers. A run does no I/O, so it cannot run your tool's code. The prompt names each tool it needs under a label of its own, such as `shout`, and that label is a *tool slot*. Prepare fills each slot from your tool catalog.

Offering a tool is like publishing a remote API: the caller sees the tool's name and description, and the work runs on your side. Unlike a server, you never listen for calls. Each call comes back from `step` as an effect.

````
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_store_op;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};

// 1. The greeter fills the tool slot `shout`, and its Lua calls the tool with the model's reply.
let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note, asks a model to reply, and shouts the reply.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "tools:\n",
    "  shout: example/text/shout\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "local reply = models.infer(store.read('note.md'))\n",
    "return tools.call('shout', { text = reply })\n",
    "```\n",
);
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;
# let id = ModelId::gateway("canned")?;
# let window = std::num::NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
# let model = ModelDescriptor::new(id, "Always replies hi there", window, ThinkingMode::Never);
# let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);

// 2. Describe the tool, give the environment the whole catalog at once, and prepare with it.
let id = ToolId::parse("example/text/shout")?;
let schema = serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}});
let shout = ToolDescriptor::new(id, "shout", "Returns the text in capital letters.", schema);
let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
let (ctx, requirements) = environment.prepare(&prompt, ctx);
assert!(requirements.refusal().is_none());
# let mut run = Run::new(Arc::new(prompt), "", ctx);

// 3. Add a tool call arm that answers with the canned output, and drive the run as before.
fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
        Effect::Chat { .. } => {
            let reply = CompletionResult::Text("hi there".to_owned());
            EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new).map_err(Into::into))
        }
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        _ => EffectAnswer::Dropped,
    }
}
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 run.resume(id, answer(effect));
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };
assert!(matches!(result, RunResult::Ok(text) if text == "HI THERE"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The frontmatter fills the tool slot `shout` with the tool path `example/text/shout` under `tools:`. The Lua passes the model's reply to `tools.call` and returns what comes back. The prompt names a tool but never holds its code.
2. A [`ToolDescriptor`](tools::ToolDescriptor) holds the tool's id, its wire name, a description, and a JSON schema for its input. It goes into a [`ToolCatalog`](tools::ToolCatalog), and [`Environment::tools`] takes the whole catalog in one call before you call [`Environment::prepare`]. Build the whole catalog first, because each call replaces the catalog rather than adding to it. Prepare matches the slot's tool path against each descriptor's id, so the description alone fills the slot.
3. `answer` gains an arm for [`Effect::ToolCall`](effect::Effect::ToolCall) that answers with the trusted output `HI THERE`, and the same loop drives the run. [`ToolOutput::trusted`](tools::ToolOutput::trusted) is for output your own code produced. The run passes it on unchanged to whoever asked for it: a model that called the tool, or, as in the greeter, the Lua that called `tools.call`. That is why the result is exactly `HI THERE`.

Output you mark as [untrusted](tools) is different. The run wraps untrusted output with a nonce before any model or any calling Lua sees it, so a model reads it as data and not as instructions. The greeter's Lua would get the wrapped text too, so its result would no longer be `HI THERE`.

A tool slot's tool belongs to a [capability](capabilities), a named pack of tools. When a slot's capability has no tool in your catalog at all, prepare reports that capability as missing, and [`Requirements::refusal`] returns an error that names it. The user learns exactly which capability to supply. When the slot's capability is in the catalog but that exact tool is not, prepare reports nothing, and the run fails only when a section offers the tool. Make sure your catalog holds the exact tool each slot names, because a clean report does not prove every slot is filled.

A host that serves many prompts usually builds its catalog in a step of its own before prepare, its *capability activation*, and records what it could not provide in a [`Requirements`] of its own. This crate does not do that step, and the greeter, which builds its catalog by hand, has nothing to merge. Prepare never checks the `capabilities:` list itself. Only your own capability activation reports a declared capability you lack.

You might expect to register a closure or a trait object that the run calls when it needs the tool. Instead, the run holds only the tool's description, and each call comes back to you as an effect to run and answer.

The run asks for the tool; your program runs it. Next, [Stop a run](#stop-a-run) ends the greeter early from another thread.

# Stop a run

A run is waiting on a slow model, and another thread in your program decides to stop it.

A cancel asks the run to stop. Once you give up on the work still out, the run reports cancelled, because stopping was your choice and not a failure.

A cancel handle works like a shared [`AtomicBool`](std::sync::atomic::AtomicBool) stop flag: any thread can set it, and the run notices at its next step. Unlike a worker thread that exits, the run then waits for you to give up on each piece of work it asked for.

````
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
# use promptforge::vfs::perform_store_op;
# use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note, asks a model to reply, and shouts the reply.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "tools:\n",
#     "  shout: example/text/shout\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "models.use('writer')\n",
#     "local reply = models.infer(store.read('note.md'))\n",
#     "return tools.call('shout', { text = reply })\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;
# let id = ModelId::gateway("canned")?;
# let window = std::num::NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
# let model = ModelDescriptor::new(id, "Always replies hi there", window, ThinkingMode::Never);
# let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
# let id = ToolId::parse("example/text/shout")?;
# let schema = serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}});
# let shout = ToolDescriptor::new(id, "shout", "Returns the text in capital letters.", schema);
# let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
# let (ctx, requirements) = environment.prepare(&prompt, ctx);
# assert!(requirements.refusal().is_none());
# let mut run = Run::new(Arc::new(prompt), "", ctx);
# fn answer(effect: Effect) -> EffectAnswer {
#     match effect {
#         Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
#         Effect::Chat { .. } => {
#             let reply = CompletionResult::Text("hi there".to_owned());
#             EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new).map_err(Into::into))
#         }
#         Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
#         _ => EffectAnswer::Dropped,
#     }
# }
// 1. Answer the store effects as before, but hold the chat effect instead of answering it.
let mut held = Vec::new();
while held.is_empty() {
    let Step::Pending { effects, .. } = run.step() else {
        panic!("the greeter waits on its model before it can finish");
    };
    for (id, _provenance, effect) in effects {
        match effect {
            Effect::Chat { .. } => held.push(id),
            other => run.resume(id, answer(other)),
        }
    }
}

// 2. Move the run's cancel handle to a second thread, and cancel the run from there.
let handle = run.cancel_handle();
std::thread::spawn(move || handle.cancel()).join().map_err(|_| "the cancelling thread panicked")?;

// 3. Keep stepping, and once the run has decided, drop every effect you still hold.
let Step::Pending { effects, .. } = run.step() else {
    panic!("the held chat effect still needs its answer");
};
assert!(effects.is_empty() && run.decided());
for id in held {
    run.resume(id, EffectAnswer::Dropped);
}

// 4. The next step ends the run as cancelled, not as a failure.
assert!(matches!(run.step(), Step::Done { result: RunResult::Cancelled, .. }));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The loop answers the two store effects as before, but keeps the chat effect's id unanswered in `held`. The model call is still out when the cancel lands.
2. [`Run::cancel_handle`] gives you a clone of the run's cancel handle, which moves into a second thread that cancels and is joined. Every handle, including one you gave the context with [`RunContext::cancel`], reaches the run's one cancel flag. Cancel whenever you need to, even while Lua is busy in a loop or waiting on a model reply, because your next call to `step` tears the run down wherever it is.
3. The next step is [`Step::Pending`] with no new effects, and [`Run::decided`] returns true. The host then answers each held id with [`EffectAnswer::Dropped`](effect::EffectAnswer::Dropped). [`Step::Done`] arrives only after every issued effect has an answer, so a run with an unanswered effect never ends.
4. The step after that is `Step::Done` with [`RunResult::Cancelled`]. A cancel ends the run as its own outcome and never as a [`RunResult::Failure`], so your failure handling never has to recognize it.

`EffectAnswer::Dropped` is the answer for work you give up on, and it works even when nobody has cancelled. Dropping even one effect when nothing has cancelled the run resumes the Lua waiting on it with the same cancelled error a cancel raises. If the prompt does not catch that error, the run ends as `RunResult::Cancelled`, not as a failure, because giving the work up was your choice, just like a cancel. So if you dropped the greeter's chat effect, its run would end as cancelled even though nothing called cancel.

Still match every outcome after you cancel. The first outcome that ends a run is the one it reports, so a run that finished or failed before your cancel landed keeps that result.

You might expect a cancel to work like dropping a future, where the work just stops and you walk away. Instead, you keep stepping and answer each effect still out with `EffectAnswer::Dropped`, and only then does the run end as cancelled.

Cancel, drop what you hold, and step until `Done`. Next, [The complete program](#the-complete-program) puts every piece into one host.

# The complete program

Here is the whole greeter host, every line visible, with one addition: it writes every parse and step event to a log.

````
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::event::Event;
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId, ToolOutput};
use promptforge::vfs::perform_store_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

const GREETER: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note, asks a model to reply, and shouts the reply.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "tools:\n",
    "  shout: example/text/shout\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "local reply = models.infer(store.read('note.md'))\n",
    "return tools.call('shout', { text = reply })\n",
    "```\n",
);

fn answer(effect: Effect) -> EffectAnswer {
    match effect {
        Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
        Effect::Chat { .. } => {
            let reply = CompletionResult::Text("hi there".to_owned());
            EffectAnswer::Chat(Completion::from_result(reply, "canned").map(Box::new).map_err(Into::into))
        }
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("HI THERE"))),
        _ => EffectAnswer::Dropped,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parse the greeter, and start the log with its parse events before checking the result.
    let (parsed, parse_events) = Prompt::parse(GREETER, "greeter");
    let mut log: Vec<Event> = parse_events.clone();
    let prompt = parsed?;

    // 2. Build the context: run events number on from the parse events, and the model is set.
    let model = ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Always replies hi there",
        NonZeroU32::new(8_192).ok_or("a context window is never zero")?,
        ThinkingMode::Never,
    );
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH)
        .provenance_start(u32::try_from(parse_events.len())?)
        .model(model);

    // 3. Offer the shout tool, prepare, and refuse to run when prepare reports a gap.
    let shout = ToolDescriptor::new(
        ToolId::parse("example/text/shout")?,
        "shout",
        "Returns the text in capital letters.",
        serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}}),
    );
    let environment = Environment::new().tools(ToolCatalog::new(&[shout])?);
    let (ctx, requirements) = environment.prepare(&prompt, ctx);
    if let Some(refusal) = requirements.refusal() {
        return Err(refusal.into());
    }

    // 4. Create the run, and keep a cancel handle for anything that may need to stop it.
    let mut run = Run::new(Arc::new(prompt), "", ctx);
    let cancel = run.cancel_handle();

    // 5. Step until Done, logging each step's events and answering each effect exactly once.
    let result = loop {
        match run.step() {
            Step::Pending { effects, events } => {
                log.extend(events);
                for (id, _provenance, effect) in effects {
                    if run.decided() {
                        run.resume(id, EffectAnswer::Dropped);
                    } else {
                        run.resume(id, answer(effect));
                    }
                }
            }
            Step::Done { result, events } => {
                log.extend(events);
                break result;
            }
        }
    };

    // 6. The tool's output is the result, and the log opens with the parse events.
    assert!(matches!(result, RunResult::Ok(text) if text == "HI THERE"));
    assert!(log.starts_with(&parse_events));
    assert!(!cancel.is_cancelled());
    Ok(())
}
````

1. [`Prompt::parse`] runs once, and its events start the log before the parse result is checked. Write the parse events first, whether or not the file parsed, because they end with a record of whether it did.
2. The context gets [`RunContext::provenance_start`] with the number of parse events, then the canned model. Every event and effect carries its [provenance](ids): its task and its position in that task's order. Parse events take the first positions, and the run's own events number on from them, so no two records in the one log share a task and position. Pass that count whenever parse events and run events share one log.
3. The environment offers the `shout` tool, prepare fills the role and the slot, and any refusal becomes the program's error before a run exists. Build every host in this order: parse the prompt, build the context, prepare it with your environment, check the refusal, create the run, and step it. Each call takes what the one before it returns.
4. [`Run::new`] takes the prepared context, and the program keeps a cancel handle that a signal handler or another thread could use. Stopping a run needs no change to the loop.
5. The loop appends each step's events to the log in the order they come, and each step's events cover exactly what happened since the step before. It answers each effect exactly once, with the answer that matches its kind, and drops effects once [`Run::decided`] returns true. It ends only at [`Step::Done`]. The [`Step`] variant and `Run::decided`, never the events, decide what the host does next.
6. The result is `HI THERE`, the log starts with the parse events, and nothing cancelled the run. The pieces from every tour fit in one host.

Answer each effect with care, because [`Run::resume`] reports nothing back. A wrong kind, an unknown id, or a second answer ends the run as a failure of kind [`RunErrorKind::Internal`].

Only the run says when it is over. `Step::Done` ends the loop, and the events that come with each step are a record for your log, never a signal.

A run's events are like log records: you write them out, and your control flow never branches on them. Unlike a logger, nothing is emitted behind your back. Each `step` hands you its events, and your host writes them. A run that fails to start returns `Step::Done` with no events at all.

This is the loop every host runs:

````text
        ┌────────────────────────────────────────────┐
        │                                            │
        v                                            │
  ┌────────────┐   Step::Pending, with effects   ┌───┴───────────────────────┐
  │ run.step() │ ──────────────────────────────> │ perform each effect, then │
  └─────┬──────┘                                 │ run.resume(id, answer)    │
        │                                        └───────────────────────────┘
        │ Step::Done, with the result
        v
  ┌────────────┐
  │ RunResult  │
  └────────────┘
````

You might expect to watch the events for a finish record and stop there. Instead, the run is over only when `step` returns `Step::Done`, which comes only after every effect has its answer.

Parsing and the loop come from [Run a prompt](#run-a-prompt). The model and prepare come from [Answer a model](#answer-a-model), and the tool and its catalog come from [Call a tool](#call-a-tool). The cancel handle and the drop after a decision come from [Stop a run](#stop-a-run). The log and the provenance start are new here.

Step, answer, log, and stop only at `Done`. Next, the [effect](effect) page shows how to answer every kind of outside work a run can ask for.

# Reference

## CapabilityConflict

[`CapabilityConflict`] names two capabilities a prompt declared that cannot be active in one run, in the order the prompt declared them. You build one when your own capability activation finds such a pair, because [`Environment::prepare`] never reports one. Push it onto [`Requirements.conflicts`](Requirements::conflicts) and merge that report in. A conflict leaves the report unsatisfied, so neither capability activates and [`Requirements::refusal`] refuses the run. Fix it by declaring only one of the two in the prompt.

- [`CapabilityConflict::new`]: the only way to build one, because the struct is non-exhaustive even though its fields are public.
- [`first`](CapabilityConflict::first): the capability the prompt declared earlier, no matter which of the two lists the other as a conflict.
- [`second`](CapabilityConflict::second): the capability the prompt declared later.

## Environment

[`Environment`] describes what every run in one deployment may use: tool descriptions, not the tools themselves, and your capabilities' Lua [preludes](capabilities). Call [`Environment::prepare`] after you set the context's model and before [`Run::new`], because it binds every role to that model. Prepare never fails, and it returns the gaps in a [`Requirements`], but never missing services or conflicts. Merge your own capability activation's report in, and refuse the run when [`Requirements::refusal`] returns an error. [Answer a model](#answer-a-model) teaches it.

- [`Environment::new`]: starts with no tools and no preludes, so `prepare` reports the capability behind every exact tool slot as missing.
- [`Environment::tools`]: replaces the whole catalog. A slot whose capability is in it without that tool goes unreported, and offering the tool fails at run time.
- [`Environment::preludes`]: replaces the list, installed in order. One that fails to load or reuses a taken name fails the run as [`RunErrorKind::Lua`] before any effect.
- `prepare`: checks only `min_context` and the `thinking` and `no-thinking` keywords. It keeps the context's store, so two contexts on one store share files.

## MissingService

[`MissingService`] names a host service that a required capability needs and your program lacks, such as a way to ask the operator on an unattended batch host. Your own capability activation finds that gap and pushes it onto [`Requirements.missing_services`](Requirements::missing_services), since [`Environment::prepare`] never reports one. [`Requirements::refusal`] then refuses the run and names the service, because [`Requirements::merge`] drops any missing-capability entry for that capability. Provide the service, or declare the capability optional in the prompt.

- [`MissingService::new`]: the only way to build one, because the struct is non-exhaustive.
- [`service`](MissingService::service): free text in your host's own words that a model will read, such as "an input broker".

## ParseError

[`ParseError`] explains why [`Prompt::parse`] rejected a prompt file: a stable kind to match on and, when known, where in the file. Match on [`ParseError::kind`], not on the message. Many failures have no line or column, among them `#` title failures, rule failures in otherwise valid frontmatter, and Lua compile failures. Show the line and column when present, or else the message, where a Lua compile failure names its block, such as "section `Transform` epilog". [Run a prompt](#run-a-prompt) teaches it.

- [`ParseError::span`]: offsets into the body after the frontmatter and any leading BOM, with CRLF read as LF, so they do not index the original text.
- [`ParseError::name`]: the prompt's frontmatter name, and `None` for any frontmatter failure, even one found after the YAML decoded.
- [`ParseError::line`]: the line in the file as written. Use it with the column, not `span`, to point at the mistake.

## Prompt

[`Prompt`] holds a parsed prompt file that many runs can share, so parse it once and hand it to each [`Run::new`] in an [`Arc`](std::sync::Arc). The `lua` blocks between the `#` title and the first `##` heading run once before any section. A prompt with no `##` sections is those blocks alone, and their return is the run's result. Parsing never checks the `promptforge:` version, so a missing or unsupported one fails only at the run's first step. [Run a prompt](#run-a-prompt) teaches it.

- [`Prompt::parse`]: returns any [`ParseError`] beside the events, never instead of them, so log the events either way. `execution` only labels them.
- [`Prompt::title`]: the `#` title's text. A file without exactly one non-empty `#` title fails as [`ParseErrorKind::Structure`].
- [`Prompt::strip_h1_prose`]: removes the prose and description under the `#` title but keeps its Lua. Call it before wrapping the prompt in an `Arc`.

## Requirements

[`Requirements`] reports what must change before a prompt can run: missing capabilities, missing services, capability conflicts, and model shortfalls. [`Environment::prepare`] returns one, and you decide from it whether to call [`Run::new`]. Prepare never fills `missing_services` or `conflicts`, so merge your own capability activation's report in first. When any list is non-empty, [`Requirements::refusal`] returns a [`RunError`] of kind [`RunErrorKind::RequirementsUnmet`] whose message is the notice text. Report that error instead of creating the run, as [Answer a model](#answer-a-model) teaches.

- [`Requirements::merge`]: skips capabilities and services already listed, drops a missing capability once a service names it, and appends conflicts and shortfalls, duplicates included.
- [`Requirements::notice`]: the refusal text, one line per gap. It omits roles unfilled for lack of a model, and slots whose cataloged capability lacks that tool.
- [`unmet_requirements`](Requirements::unmet_requirements): filled only by `prepare`, one entry per role check the context's current model fails.
- [`missing_required`](Requirements::missing_required): filled by your activation for absent or failed required capabilities, and by `prepare` for a slot whose capability has no catalog tools.

## Run

[`Run`] drives one run: you call `step` and answer its effects with `resume`. Create it from a [`Prompt`] in an `Arc` and a prepared [`RunContext`], because one that skipped [`Environment::prepare`] has no tools and no model roles. [`Run::new`] never fails, so a missing or unsupported `promptforge:` version, or an unusable store, comes back from the first step as [`Step::Done`] with [`RunResult::Failure`]. Fix the version line or the store, and start a new run. [Run a prompt](#run-a-prompt) teaches it.

- [`Run::step`]: never panics or returns an error. Stepping after `Step::Done` reports an internal failure instead.
- [`Run::resume`]: never errors. A wrong-kind, unknown-id, or repeated answer ends the run as an internal failure, and an answer no [chain](ids) waits for is discarded.
- [`Run::cancel`]: a request, not a stop. The run ends cancelled only after you answer each outstanding effect with [`EffectAnswer::Dropped`](effect::EffectAnswer::Dropped).
- [`Run::cancel_handle`]: a clone of the context's cancel flag, for cancelling this run from another thread.
- [`Run::decided`]: true once the outcome is fixed, even for a run that never started. Then answer held effects `EffectAnswer::Dropped`, since `Step::Done` waits for every answer.

## RunContext

[`RunContext`] holds what one run gets from your program: name, seed, start time, limits, cancel flag, files, and current model. Build one per run, and set its model before [`Environment::prepare`], which binds every role to it once. With no model, selecting a role fails at run time. A store handle without a working store ends the first step with [`RunErrorKind::Store`], so pass a working store or keep the default in-memory one. [Run a prompt](#run-a-prompt) teaches it.

- [`RunContext::new`]: a live host passes the current time and a cryptographically random seed, which feeds a security nonce.
- [`RunContext::report_debug`]: turning debug mode on adds `Request` and `Response` events with the raw model bodies, which the `Chat` effect and its answer already hold.
- [`RunContext::cancel`]: replaces the flag `new` made, so your handle, [`RunContext::cancel_handle`], and [`Run::cancel_handle`] all reach one flag.
- [`RunContext::ui`]: installs a `ui()` global, and lets `models.get` resolve an undeclared alias as a raw model id. Without it, resolution stays strict.
- [`RunContext::provenance_start`]: pass the number of parse events you logged, so parse and run events keep unique `(task, seq)` pairs. Only the root task's counter moves.

## RunError

[`RunError`] explains why a run failed: a stable kind to match on, whether a retry may help, and the underlying cause. You get one from [`RunResult::Failure`] in [`Step::Done`], or as the refusal from [`Requirements::refusal`], whose message is exactly the notice text. A run you cancel is not a failure, since it ends as [`RunResult::Cancelled`]. Match [`RunError::kind`] in code, show the message to people, and retry only a retryable error. [Answer a model](#answer-a-model) teaches it.

- [`RunError::is_cancelled`]: true only for a host interrupt. An uncaught Lua task cancellation is [`RunErrorKind::Lua`] and returns false.
- [`RunError::is_retryable`]: true for transport failures, unreadable or malformed replies, and backend statuses of 500 and up. Every lower status, 429 included, is not.
- [`RunError::location`]: `Some` only for parse and internal failures. An internal fault points at a Rust source line, not at the prompt.

## RunLimits

[`RunLimits`] sets the ceilings one run honors, from tool rounds per section to the model receive timeout. The defaults are 24 tool rounds, 8 concurrent [tasks](ids), 16 MiB replies, 64 MiB of Lua memory, 1024 log events, and 120 s per receive. To change them, install your own with [`RunContext::limits`]. A tool loop out of rounds ends with [`RunErrorKind::Tool`], and exhausted Lua log events end with [`RunErrorKind::Quota`]. Raise the matching ceiling, or change the prompt so it needs less.

- [`RunLimits::max_tool_iterations`]: caps model rounds in one section's tool loop. The prompt's frontmatter `max_tool_iterations` overrides it for that prompt.
- [`RunLimits::max_concurrency`]: nests, so each spawned task counts against its owner's limit and every ancestor's, and a fan-out runs within its parent's remaining share.
- [`RunLimits::lua_memory_bytes`] and [`RunLimits::lua_log_events`]: apply to each Lua VM, not to the whole run.
- [`RunLimits::request_timeout`]: bounds each wait for the next part of a reply, the headers and then each body chunk, so a steady stream never times out.

## SourceLocation

[`SourceLocation`] says where a failure happened: a position in the prompt or, for an internal fault, a line of Rust source. Read it from [`RunError::location`] when you point a person at the failing spot. Match on [`RunError::kind`], not on this, to decide what to do.

- [`path`](SourceLocation::path): the prompt's frontmatter name, or `<prompt>` for you to replace with your own label, or a Rust source path for an internal fault.
- [`line`](SourceLocation::line): 1-based when known.
- [`column`](SourceLocation::column): 1-based when known, and always `None` for an internal fault.
- [`span`](SourceLocation::span): set only for structural parse failures, as offsets into the body after the frontmatter and any BOM, with CRLF read as LF.

## UnmetRequirement

[`UnmetRequirement`] describes one way the current model falls short of a model role: the role, the failed check, and what was required against what the model has. You read these in [`Requirements.unmet_requirements`](Requirements::unmet_requirements) after [`Environment::prepare`], which alone creates them, and you can read one but not build one. Any entry makes [`Requirements::refusal`] refuse the run. Give the context a model that meets the role, and prepare again. [Answer a model](#answer-a-model) teaches it.

- [`required`](UnmetRequirement::required): what the role asks for, as text: a token count such as "200000", or a keyword such as "thinking".
- [`actual`](UnmetRequirement::actual): what the current model has, as text: its context size such as "32000", or its thinking mode such as "Never".

## ParseErrorKind

[`ParseErrorKind`] classifies a [`ParseError`] by what in the file is wrong, so your program can match on it rather than on the message. Use it when you handle a failed [`Prompt::parse`] and want to send the author to the right part of the file. [Run a prompt](#run-a-prompt) teaches it.

| Kind | What is wrong in the file |
|---|---|
| [`Frontmatter`](ParseErrorKind::Frontmatter) | Missing, unclosed, or invalid YAML; a reserved or doubly used tool alias or role label; a repeated capability; or a slot naming an optional capability. |
| [`Structure`](ParseErrorKind::Structure) | A missing, duplicate, or empty `#` title. It is also the fallback for internal parse faults and for Lua errors other than compile errors. |
| [`Fence`](ParseErrorKind::Fence) | The removed `lua prompt` fence, an unclosed exact fence, a second `lua shared` fence, or a `lua shared` fence outside the `#` title's body. |
| [`List`](ParseErrorKind::List) | A list-only section holds something other than list items, or an empty item. |
| [`Lua`](ParseErrorKind::Lua) | The shared library, or a Lua block under the `#` title or in a section, does not compile. The message names the block. |

## RequirementCheck

[`RequirementCheck`] says which model check a role failed: its context minimum or a hard thinking keyword. Read it from [`UnmetRequirement.check`](UnmetRequirement::check) to tell a context shortfall from a thinking mismatch. Soft keywords such as `frontier`, `fast`, `small`, `creative`, and `chat` are never checked, so they never appear. [Answer a model](#answer-a-model) teaches it.

- [`RequirementCheck::ContextMinimum`]: the role's `min_context` is larger than the current model's context window.
- [`RequirementCheck::HardKeyword`]: the role asks for `thinking` or `no-thinking`, and the model's thinking mode does not match.

## RunErrorKind

[`RunErrorKind`] classifies a [`RunError`] by the phase that failed, so your program can match on it when it decides how to react to a failed or refused run. `Internal` also covers host mistakes: an answer of the wrong kind, an answer for an id never issued, or a second answer to one effect. Answer each issued effect exactly once, with the answer its kind expects. [The complete program](#the-complete-program) teaches it.

| Kind | What failed |
|---|---|
| [`Parse`](RunErrorKind::Parse) | A frontmatter or structure failure, including a prompt with no `promptforge:` version line. An unsupported version is `Version` instead. |
| [`Version`](RunErrorKind::Version) | The prompt declares a `promptforge:` major version that this build does not support. |
| [`Binding`](RunErrorKind::Binding) | Only a failed schema binding or a missing required model. Absent or clashing capabilities arrive as `RequirementsUnmet` instead. |
| [`Completion`](RunErrorKind::Completion) | A model call: transport, backend, undecodable or empty replies, missing or invalid client configuration, or a disabled model gateway. |
| [`Tool`](RunErrorKind::Tool) | A tool loop ran out of rounds, a call reached a tool outside the section's scope or one that is not bound, or a tool failed. |
| [`Store`](RunErrorKind::Store) | A store operation failed, or, when the run starts, its files have no declared store or the store fails its probe. |
| [`Determinism`](RunErrorKind::Determinism) | Two tasks that run at the same time claimed the same store path. The run ends at once, and the prompt's Lua cannot catch it. |
| [`Lua`](RunErrorKind::Lua) | Lua compile and runtime failures, and task misuse, such as a leaked task, a result awaited twice, or an uncaught task cancellation. |
| [`Quota`](RunErrorKind::Quota) | The run used up its Lua log events, its log bytes, or its instructions. |
| [`ContextExhausted`](RunErrorKind::ContextExhausted) | The compactor could not fit the conversation into the model's context window. |
| [`Substitution`](RunErrorKind::Substitution) | A `{{ }}` placeholder in the prompt's prose could not be filled. |
| [`Cancelled`](RunErrorKind::Cancelled) | A host interrupt, seen only inside a run. A run you cancel ends as [`RunResult::Cancelled`] instead of a failure of this kind. |
| [`Internal`](RunErrorKind::Internal) | A broken invariant inside the run, including the host mistakes the paragraph above names. |
| [`RequirementsUnmet`](RunErrorKind::RequirementsUnmet) | The refusal from [`Requirements::refusal`], for missing capabilities, missing services, conflicts, or model shortfalls. |

## RunResult

[`RunResult`] reports what a finished run produced when [`Run::step`] returns [`Step::Done`]: its final text, a cancellation, or the error it failed with. [`RunResult::Ok`] holds the final text; write it in full in patterns, because the prelude's `Ok` is also in scope. A failed run holds its [`RunError`] in [`RunResult::Failure`], so match the error's kind to decide what to fix. A run you cancelled ends as [`RunResult::Cancelled`] instead. [Run a prompt](#run-a-prompt) teaches it.

## Step

[`Step`] reports what one [`Run::step`] call produced: effects for you to perform and events to log, or the run's result. Perform each effect in [`Step::Pending`], answer it with [`Run::resume`], and stop at [`Step::Done`], deciding from the variant and [`Run::decided`], never from the events. A `Step::Pending` with no new effects while you hold no unanswered effect means the run has stalled. Stop driving it and report the stall as an error. [Run a prompt](#run-a-prompt) teaches it.

- `Step::Pending` `effects`: in issue order, each with its task's provenance. Empty means every [chain](ids) waits on an effect already issued.
- `Step::Pending` `events`: the reports made since the previous step, in order, for you to log.
- `Step::Done` `events`: the reports since the previous step, and empty for a run that failed to start.

# Where to go next

- [effect](effect): answer every kind of outside work a run can ask for, and log what you did.
- [event](event): log, show, and debug what happens during a run.
- [ids](ids): group a run's log by task, and follow each task from start to end.
- [model](model): describe your models, see which model each prompt role got, and answer model rounds.
- [transport](transport): build request bodies, read streamed replies, and report failed rounds when you write your own model connection.
- [tools](tools): offer tools to a run, and answer the tool calls it makes.
- [capabilities](capabilities): name capabilities, check which tools belong to each, and give a run the Lua that capabilities add.
- [prompt](prompt): read what a prompt declares before you run it, and pass it arguments.
- [vfs](vfs): give a run its files, the store every section shares, host folders beside it, and rules about what the run may change.
- [cancel](cancel): stop runs and tasks from any thread, one at a time or all together.
- [timestamp](timestamp): give each run its start time, and keep that time with the run's record.
- [metrics](metrics): read the token counts and timings of each model call.
- [replay](replay): store a run's behavior flags with its record, and hand them back unchanged.
