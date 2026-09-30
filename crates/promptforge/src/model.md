Describe the models your program serves, see which model each prompt role got, and answer model rounds.

You need this page when your prompts call a model. A model round is one request to a model and its reply; the run hands your program each round as a `Chat` effect, and your answer is the reply. This page shows how your program describes the models it serves, sees which model each of a prompt's roles got, and answers the model rounds a run asks for.

# Where this fits

The crate overview's [Run a prompt](crate#run-a-prompt) shows how a run hands your program each piece of outside work as an effect, and [Answer a model](crate#answer-a-model) adds a model call to the same prompt. The [effects page](crate::effect) shows a host that answers `Chat` effects only in passing. This page covers the models behind those rounds, and what a chat answer holds.

# Describe your models

Your program serves one or more models, and you want to tell PromptForge what each one is. For each model you write down a small record: which model it is, a line about what it is for, how many tokens its context window holds, and whether it thinks before it answers. That record is a [`ModelDescriptor`]. A [`ModelCatalog`] is the checked list of them, kept in your order.

Describing models feels like filling a registry of backends keyed by name. Unlike a `HashMap`, the catalog refuses a repeated key instead of replacing the earlier entry.

````
use std::num::NonZeroU32;

use promptforge::model::{ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ThinkingMode};

// 1. Describe gateway/fast: a 32000-token window on a backend that never thinks.
let fast = ModelDescriptor::new(
    ModelId::gateway("fast")?,
    "Quick replies for the greeter",
    NonZeroU32::new(32_000).ok_or("a context window is never zero")?,
    ThinkingMode::Never,
);

// 2. Describe gateway/deep: a 200000-token window on a backend that always thinks.
let deep = ModelDescriptor::new(
    ModelId::new(ModelId::GATEWAY, "deep")?,
    "Careful replies for the greeter",
    NonZeroU32::new(200_000).ok_or("a context window is never zero")?,
    ThinkingMode::Always,
);

// 3. Collect both in a catalog, and look gateway/deep up by its id.
let catalog = ModelCatalog::new([fast.clone(), deep])?;
assert_eq!(catalog.models().len(), 2);
let found = catalog.get(&ModelId::gateway("deep")?).ok_or("gateway/deep is in the catalog")?;
assert_eq!(found.context().get(), 200_000);
assert_eq!(found.thinking(), ThinkingMode::Always);

// 4. A list that names gateway/fast twice is refused as a whole, naming the repeated id.
let Err(ModelCatalogError::DuplicateId { server, name, .. }) = ModelCatalog::new([fast.clone(), fast]) else {
    panic!("a repeated id is refused with DuplicateId");
};
assert_eq!((server.as_str(), name.as_str()), ("gateway", "fast"));
Ok::<(), Box<dyn std::error::Error>>(())
````

1. Step 1 builds the id with [`ModelId::gateway`], which returns a `Result`. `gateway/fast` gets a 32000-token window as a `NonZeroU32`, and [`ThinkingMode::Never`]. Once the id is valid, the descriptor cannot fail.
2. Step 2 builds the same kind of id the long way, with [`ModelId::new`] and the [`ModelId::GATEWAY`] namespace. `new` accepts any server namespace, so you can name models from more than one server. When both parts are bad, the [`ModelIdError`] names the server, because the server is checked first. Fix the server part before you trust the error about the name. `gateway/deep` always thinks, so it gets `Always`. Give each descriptor the [`ThinkingMode`] its backend really has: `Never`, `Always`, or `Switchable`. Prepare checks the mode against what a prompt requires of each model it asks for, so a wrong mode makes it report the wrong requirements as unmet; the next section shows those checks.
3. Step 3 builds the catalog with [`ModelCatalog::new`], which keeps your order. The catalog has no `len` of its own, so count its entries with [`models()`](ModelCatalog::models)`.len()`. [`get`](ModelCatalog::get) finds `gateway/deep` by its id, with its 200000-token window and its `Always` mode intact. [`contains`](ModelCatalog::contains) asks the same question when you only need a yes or no. Prepare and the run never read a `ModelCatalog`; your list of models stays with your program. Use it to offer a choice of models, for example a model picker, then look the chosen model up with `get` and give that descriptor to the run context, as the next section shows.
4. Step 4 passes a list that names `gateway/fast` twice. `ModelCatalog::new` refuses the whole list with [`ModelCatalogError::DuplicateId`], which names the repeated `server` and `name`. Match it as `DuplicateId { server, name, .. }`. The catalog stops at the first repeat it finds, so fix repeats one at a time.

An id part is refused when it is empty or holds any control character, including newline, NUL, DEL, and U+0085. Non-ASCII names such as `café-模型` pass, and so do ordinary spaces. So handle the `Result` for names read from a config file, and trim names yourself, because ` m` and `m` are two ids.

You might expect `ModelCatalog::new` to keep the last descriptor for a repeated id, as inserting into a `HashMap` does. Instead, it refuses the whole list with `DuplicateId`, naming the id that repeated.

One descriptor per id, with the thinking mode the backend really has. Next, [See which model each role got](#see-which-model-each-role-got) hands one of these descriptors to a run.

# See which model each role got

Your prompt names more than one model it needs, each under a name of its own, a *model role*. Before the run starts, a step called *prepare* fills each role with a real model. A [`RunContext`](crate::RunContext) is what one run starts from; you build it with `RunContext::new(name, seed, started_at)`, and its current model is the one descriptor you set with [`RunContext::model`](crate::RunContext::model). Prepare records, for every declared role, the model that will serve it: the run context's [`ModelBindings`], where every role points at that current model.

Reading bindings feels like reading a resolved dependency lock file: each declared name maps to one concrete entry. Unlike a lock file, you never write it; prepare fills it and you only read.

````
# use std::num::NonZeroU32;
# use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
# let fast = ModelDescriptor::new(
#     ModelId::gateway("fast")?,
#     "Quick replies for the greeter",
#     NonZeroU32::new(32_000).ok_or("a context window is never zero")?,
#     ThinkingMode::Never,
# );
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, RunContext};

// 1. The greeter declares two roles, `writer` and `checker`, and its section asks `writer` once.
let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note and asks a model to reply to it.\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "  checker: {}\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "models.use('writer')\n",
    "return models.infer(store.read('note.md'))\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greeter");
let prompt = parsed?;

// 2. Set gateway/fast as the current model, and prepare.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(fast.clone());
let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
assert!(requirements.refusal().is_none());

// 3. Read the bindings: both roles resolve to gateway/fast.
let bindings = ctx.model_bindings();
assert_eq!(bindings.resolve("writer"), Some(&fast));
assert_eq!(bindings.resolve("checker"), Some(&fast));
assert_eq!(bindings.role_id("checker"), Some(fast.id()));

// 4. `len` counts two roles on one shared model, and an undeclared role resolves to nothing.
assert_eq!(bindings.len(), 2);
assert_eq!(bindings.model(fast.id()), Some(&fast));
assert!(bindings.resolve("editor").is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. Step 1 declares the roles `writer` and `checker` under `models:`, with no keywords. A role can also declare `min_context: N` under its name, the fewest context tokens it needs. The section selects `writer` with `models.use` and asks it once with `models.infer`.
2. Step 2 sets `gateway/fast` with `RunContext::model`, then calls [`Environment::prepare`](crate::Environment::prepare), which binds every declared role to that one model and returns the context and a [`Requirements`](crate::Requirements) report. The report lists what your program could not provide, and [`notice()`](crate::Requirements::notice) renders it as the [requirements notice](crate#answer-a-model). [`refusal()`](crate::Requirements::refusal) is `Some` whenever anything is listed, including an unmet role requirement, holding a [`RunError`](crate::RunError) of kind `RequirementsUnmet` with that notice; it is `None` when nothing blocks the run. Check `refusal()` after prepare and before you build the run. Here it is `None`, because neither role declares a hard keyword or a `min_context`.
3. Step 3 reads the context's [`model_bindings()`](crate::RunContext::model_bindings). [`resolve`](ModelBindings::resolve) returns a role's [`ModelDescriptor`], here `gateway/fast` for both `writer` and `checker`, and [`role_id`](ModelBindings::role_id) returns its [`ModelId`]. This is how you log or check which model serves each role.
4. Step 4 checks that [`len()`](ModelBindings::len) is 2, while [`model(fast.id())`](ModelBindings::model) returns the one shared descriptor. `len()` counts bound roles, not distinct models. The role `editor`, which the prompt never declared, resolves to `None`.

With no current model, prepare binds no role and checks nothing. `model_bindings()` is empty, `resolve` gives `None` for every declared role, and the report says nothing about roles, so `refusal()` can be `None` even for a role with a `min_context`. The run then fails when a section calls `models.use` on one of those roles, because the role is not bound. So set the current model before prepare, and check `resolve` if you are unsure it was set.

When the current model misses a role's hard need, prepare reports an unmet requirement naming the role, not an error, and that is what makes `refusal()` return `Some`. A role goes unmet when its `min_context` is above the model's window, when it declares `thinking` and the model is `Never`, or when it declares `no-thinking` and the model is `Always`. Soft role keywords are never checked. Read the requirements notice after prepare, and decide whether to run or pick another model.

You might expect prepare to pick the best catalog entry for each role from its description. Instead, every role binds to the one current model, because the choice of model is your program's. The current model is your host's selection, such as the model a user picks from a list, and prepare never sees your catalog, so it can give each role only that model. To serve a role with another model, set that model as current and prepare again. A role that model cannot serve shows up as an unmet requirement.

Every role gets the current model; read the bindings to confirm it. Next, [Answer a model round](#answer-a-model-round) answers the round that `writer` asks for.

# Answer a model round

Your run hands you a [`Chat`](crate::effect::Effect::Chat) effect, and you want to send it to your model and give the reply back. Your answer is the whole outcome of that round: the model's reply together with the model that served it, or the reason the round failed. The reply is a [`Completion`], and the failure is a [`CompletionError`].

Answering a round feels like proxying an HTTP call: forward the request, wrap the response. Unlike a plain proxy, you wrap the reply in a `Completion` that also says which model served it.

````
# use std::num::NonZeroU32;
# use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
# use promptforge::timestamp::Timestamp;
# use promptforge::{Environment, Prompt, RunContext};
# let fast = ModelDescriptor::new(
#     ModelId::gateway("fast")?,
#     "Quick replies for the greeter",
#     NonZeroU32::new(32_000).ok_or("a context window is never zero")?,
#     ThinkingMode::Never,
# );
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note and asks a model to reply to it.\n",
#     "promptforge: 0\n",
#     "models:\n",
#     "  writer: {}\n",
#     "  checker: {}\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "models.use('writer')\n",
#     "return models.infer(store.read('note.md'))\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");
# let prompt = parsed?;
use std::sync::Arc;
use promptforge::effect::{Effect, EffectAnswer};
use promptforge::model::{Completion, CompletionError, CompletionResult};
use promptforge::{transport::ClientError, vfs::perform_store_op};
use promptforge::{Run, RunErrorKind, RunResult, Step};

// 1. Each run prepares the greeter from the last tour with `model`, and its section asks `writer` once.
fn run_greeter(
    prompt: &Prompt,
    model: &ModelDescriptor,
    chat: impl Fn(&str) -> Result<Box<Completion>, CompletionError>,
) -> RunResult {
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model.clone());
    let (ctx, _requirements) = Environment::new().prepare(prompt, ctx);
    let mut run = Run::new(Arc::new(prompt.clone()), "", ctx);
    // 2. Answer the store as before, and hand each chat effect's model name to `chat` for its answer.
    loop {
        let effects = match run.step() {
            Step::Pending { effects, .. } => effects,
            Step::Done { result, .. } => return result,
        };
        for (id, _provenance, effect) in effects {
            let answer = match effect {
                Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
                Effect::Chat { binding, .. } => EffectAnswer::Chat(chat(binding.id().name())),
                _ => EffectAnswer::Dropped,
            };
            run.resume(id, answer);
        }
    }
}

// 3. Answer with a whole text completion, reported as served by gateway/fast's name.
let result = run_greeter(&prompt, &fast, |served| {
    let reply = CompletionResult::Text("hello world".to_owned());
    Ok(Box::new(Completion::from_result(reply, served)?))
});
assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));

// 4. Run it again, and answer with a failed round: the backend returned status 503.
let result = run_greeter(&prompt, &fast, |_served| {
    Err(ClientError::Backend { status: 503, body: "overloaded".to_owned() }.into())
});
let RunResult::Failure(error) = result else { panic!("a failed round fails the run") };
assert_eq!(error.kind(), RunErrorKind::Completion);
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. Step 1 defines `run_greeter`. It gives the context the model, prepares the two-role greeter from the previous section, and creates a fresh run each time you call it. The `""` passed to [`Run::new`](crate::Run::new) is the run's arguments string, empty because the greeter takes none. One prompt serves both answers below, because your program chooses each answer per round.
2. Step 2 answers each store effect with [`perform_store_op`](crate::vfs::perform_store_op), as on the [crate page](crate#run-a-prompt). It answers each chat effect with [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat), holding whatever `chat` returns. [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) answers any other effect without performing it, and a chain waiting on it resumes with a cancelled error; that arm never runs for the greeter. A live host sends the effect's `messages`, `tools`, and `options` to its backend, but not its `stream` flag. The options name the model by its id's [`name()`](ModelId::name), not the role name or the server part, so the `writer` role bound to `gateway/fast` reaches your backend as model `fast`.
3. Step 3 builds the reply with [`Completion::from_result`] and [`CompletionResult::Text`], then boxes it, and the run ends with [`RunResult::Ok`](crate::RunResult::Ok) holding `hello world`. `from_result` is the only way to build a completion without a transport; a live host gets its completion from [`read_completion_stream`](crate::transport::read_completion_stream). Pass the model name your backend reported as the second argument, because [`Completion::model`] records the model that served the round, which can differ from the one requested. This canned host has no backend to report one, so step 2 passes the requested name, `binding.id().name()`; a live host passes the name from its backend's response. The `?` turns the constructor's error into a `CompletionError` through `From`.
4. Step 4 converts [`ClientError::Backend`](crate::transport::ClientError::Backend) with status 503 into a `CompletionError` with `into()`, and answers the round with `Err`. The run ends with [`RunResult::Failure`](crate::RunResult::Failure) of kind [`RunErrorKind::Completion`](crate::RunErrorKind::Completion). The run needs the failure itself, not a missing answer, so answer a failed round with `EffectAnswer::Chat(Err(error))`.

When the model asks for tools instead, build each call with [`ToolCall::from_parts`]. Pass its arguments as a parsed JSON object, not the encoded string the wire carries, and wrap the calls in [`CompletionResult::ToolCalls`]. `from_result` refuses an empty batch and two calls that share an id, but not an empty text reply. A live reply whose text is empty or only whitespace is an `EmptyReply` failure, and `from_result` accepts both, so check `text.trim().is_empty()` yourself when your backend can return one.

To decide on a retry, read [`is_retryable()`](CompletionError::is_retryable). It is true for transport failures, 5xx statuses, and malformed responses, and false for every status below 500, including 429, so add your own backoff for rate limits. A `CompletionError`'s `Display` never shows the backend's error body. Read [`backend_body()`](CompletionError::backend_body) when you want it, so you choose whether that body reaches your logs.

You might expect to answer a chat effect with the reply text. Instead, the answer is a boxed `Completion` built with `Completion::from_result`, which carries whether the model replied or asked for tools, and which model served the round.

Answer each round with a whole completion or its error. The [Reference](#reference) below covers every type this page names.

# Reference

## Completion

[`Completion`] holds the parsed outcome of one model round, plus the metadata the backend reported. You build one to answer a chat effect, or read one to see what a model returned. [`Completion::from_result`] builds one without a transport. It accepts an empty text reply, but fails for an empty tool-call batch, and when two calls share an id; give the batch at least one call, each with its own id. [Answer a model round](#answer-a-model-round) teaches it.

- [`model`](Completion::model): the name the backend reported serving, which can differ from the request; empty when the response named no model.
- [`reasoning_content`](Completion::reasoning_content): never folded into the answer text, so show or log it yourself.
- [`finish_reason`](Completion::finish_reason): the choice's `finish_reason`, when the backend supplied one.
- [`usage`](Completion::usage), [`llama_timings`](Completion::llama_timings), [`client_timing`](Completion::client_timing): token accounting, llama.cpp's `timings`, and client-clock timings; always `None` after `from_result`.

## CompletionError

[`CompletionError`] reports why a model round or a catalog fetch failed. Answer a chat effect with one to fail the round, and match [`kind()`](CompletionError::kind) for the cause. Retry only when [`is_retryable()`](CompletionError::is_retryable) is true: for transport failures, 5xx statuses, and malformed responses, but never for a status below 500, including 429, so rate limits need your own backoff. A transport builds one only by converting a [`ClientError`](crate::transport::ClientError) through `From`, which is what step 4's `into()` does. [Answer a model round](#answer-a-model-round) teaches it.

- [`backend_body`](CompletionError::backend_body): the backend's error body for a non-success status; the error's `Display` never includes it.
- [`status`](CompletionError::status): the HTTP status from a status failure or a failed body read, which is still `Transport`; do not infer the kind from it.
- [`is_timeout`](CompletionError::is_timeout): true only when the transport wrapped its error in the crate's `Timeout` marker; a custom transport must mark its timeouts.
- [`finish_reason`](CompletionError::finish_reason): `Some` only for an empty reply; after successful tool calls, `Some("stop")` exits cleanly, and a missing or `"length"` reason fails hard.

## CompletionOptions

[`CompletionOptions`] holds the per-call fields merged into a chat-completions request body. Use it to build request options with a validated temperature, a token cap, or a thinking switch. [`CompletionOptions::new`] takes the caller-facing model name sent on the wire, and sets no temperature, no token cap, and no thinking switch, so the backend's defaults apply. There are no public getters, so you cannot read a value back out.

- [`with_temperature`](CompletionOptions::with_temperature): the only fallible setter; it returns [`TemperatureError::NotFinite`] for NaN or an infinity and `OutOfRange` outside `[0.0, 2.0]`.
- [`with_thinking`](CompletionOptions::with_thinking): sends `chat_template_kwargs.enable_thinking` without checking the model's mode, so check [`ModelDescriptor::thinking`] yourself first.

## Message

[`Message`] is one chat message in a request's message list. Read them from a chat effect, or build them for a scripted round. There is no public `system` constructor, though a message you read can be a system message. A plain message serializes to just `role` and `content`. An assistant turn that requested tools may re-render each call, so its key order and whitespace can differ from the provider's raw `tool_calls`.

- [`role`](Message::role): returns `system`, `user`, `assistant`, or `tool`.
- [`content`](Message::content): returns `""` for a multimodal message whose content is a list of parts, so an empty string does not mean an empty message.
- [`tool`](Message::tool): builds a `tool` result; its id should match the [`ToolCall`] it answers, and nothing checks that it does.
- [`assistant`](Message::assistant): builds a plain text turn with no `tool_calls` field.

## ModelBinding

[`ModelBinding`] ties one prompt-local role alias to a model identity and the frozen request fields for that role. It tells you the model and settings a chat round runs under. [`completion_options()`](ModelBinding::completion_options) sends the id's [`ModelId::name`] as the wire model, not the alias or the server namespace, so a `writer` role bound to `gateway/m` sends `"model": "m"`. [Answer a model round](#answer-a-model-round) reads the binding a chat effect carries.

- [`with_invocation`](ModelBinding::with_invocation): replaces all three invocation fields at once rather than merging them, so carry over any field you want to keep.
- [`new`](ModelBinding::new): starts with an empty keyword list; add keywords with [`with_capabilities`](ModelBinding::with_capabilities).
- [`capabilities`](ModelBinding::capabilities): the bound role's keywords, from the closed kebab-case frontmatter vocabulary; Lua sees them on the handle as `capabilities`.
- [`description`](ModelBinding::description): in a binding prepare builds, the role's `description` from the front matter, or the model descriptor's description when the role declares none.
- [`context`](ModelBinding::context): the context window in tokens.

## ModelBindings

[`ModelBindings`] records which model each declared role was bound to during prepare. Read it after prepare through [`RunContext::model_bindings`](crate::RunContext::model_bindings). Every declared role binds to the context's current model, and a hard keyword or context minimum that model misses shows up as an unmet requirement naming the role. You cannot write bindings; change the current model and prepare again. [See which model each role got](#see-which-model-each-role-got) teaches it.

- [`resolve`](ModelBindings::resolve): the role's descriptor; an undeclared role gives `None`.
- [`model`](ModelBindings::model): the descriptor under an identity, when this run may use it.
- [`len`](ModelBindings::len) and [`is_empty`](ModelBindings::is_empty): count bound roles, not distinct models, so two roles on one model give `len() == 2`.

## ModelCatalog

[`ModelCatalog`] collects the models your program serves, with no two sharing an id. Build one from a gateway `GET /v1/models` listing or a pinned offline entry. [`ModelCatalog::new`] keeps your order, and returns [`ModelCatalogError::DuplicateId`] when two descriptors share one [`ModelId`]. It stops at the first repeat, so after you fix one there may be another. [Describe your models](#describe-your-models) teaches it.

- [`empty`](ModelCatalog::empty): an empty catalog, as `Default` also gives, listing no models.
- [`models`](ModelCatalog::models): the descriptors in your order; the catalog has no length method of its own, so count this slice.

## ModelDescriptor

[`ModelDescriptor`] describes one model you serve: its id, description, context window, and thinking mode. Build one for a catalog, or to hand the run context its current model. The description is not checked, so nothing warns you about a blank one. Prepare reports a role's `min_context` above the context window as an unmet [`ContextMinimum`](crate::RequirementCheck::ContextMinimum), with both numbers as strings. [Describe your models](#describe-your-models) teaches it.

- [`thinking`](ModelDescriptor::thinking): the mode; prepare reports a hard `thinking` role on a `Never` model, or `no-thinking` on `Always`, as unmet, showing that value.

## ModelId

[`ModelId`] names one model by a server namespace plus the caller-facing model name. Its constructors return [`ModelIdError`] when a part is empty or holds any Unicode control character, including NUL, newline, DEL, and U+0085. Non-ASCII names such as `café-模型` pass, and so does other whitespace, so trim names yourself, or ` m` and `m` are two ids. When both parts are bad, the error names `server`. [Describe your models](#describe-your-models) teaches it.

- [`new`](ModelId::new): accepts any server namespace, not only `gateway`, so you can name models from more than one server.
- [`gateway`](ModelId::gateway): builds an id in the `gateway` namespace, whose name is the gateway's model name, the OpenAI model `id`.
- [`name`](ModelId::name): the caller-facing model name, which is what a request sends as its model.

## ModelIdError

[`ModelIdError`] says why a [`ModelId`] could not be built. Its message names the rejected part, `server` or `name`, and why: the part was empty or held a control character. That message alone tells whoever supplied the id what to fix, so report it back to them. There are no accessors for the part or the reason, so you cannot branch on the cause. Fix the named part and build the id again.

## ModelInvocation

[`ModelInvocation`] holds the frozen per-request fields for one binding: temperature, token cap, and thinking switch. You build one for a [`ModelBinding`], or to replace its settings with [`ModelBinding::with_invocation`]. The fields are public and there is no `Default`, so build it with a struct literal that names all three. In each binding prepare builds, `temperature` and `max_tokens` are `None`; leave a field `None` to send nothing for it.

- [`thinking`](ModelInvocation::thinking): sets `chat_template_kwargs.enable_thinking`; prepare sets it to `Some(true)` for a `thinking` role, `Some(false)` for a `no-thinking` role, and `None` otherwise.

## Temperature

[`Temperature`] holds a sampling temperature that is finite and within `[0.0, 2.0]`, for [`ModelInvocation::temperature`]. [`Temperature::new`] returns [`TemperatureError::NotFinite`] for NaN or an infinity, and `OutOfRange` outside the range; `TryFrom<f64>` runs the same check. Both endpoints are inclusive, and `-0.0` passes because it compares equal to `0.0`. It is `Copy` and `PartialEq`, but not `Eq` or `PartialOrd`, so compare temperatures through [`get()`](Temperature::get).

## ToolArguments

[`ToolArguments`] gives a borrowed, typed view of one tool call's arguments object. Use it when you run a tool the model asked for. The arguments are always a JSON object, because the decoder and [`ToolCall::from_parts`] both refuse any other value, so you never handle a bare string or array.

- [`names`](ToolArguments::names) and [`contains`](ToolArguments::contains): see only top-level keys; check nested keys by parsing [`to_json_string()`](ToolArguments::to_json_string), which serializes the object as JSON text.
- [`is_empty`](ToolArguments::is_empty): true for an object with no keys.

## ToolCall

[`ToolCall`] holds one tool invocation the model asked for: its id, tool name, and arguments. You read them from a [`CompletionResult::ToolCalls`] reply, or script one with [`ToolCall::from_parts`]. `from_parts` takes a parsed JSON value, not the encoded string the wire carries, and fails when `id` or `name` is blank, meaning empty or only whitespace, or `arguments` is not a JSON object. It does not check for duplicate ids; [`Completion::from_result`] checks them across the batch.

- [`id`](ToolCall::id): the id the model assigned; answer with [`Message::tool`] using the same id.
- [`arguments`](ToolCall::arguments): always an object, because a model call with missing or invalid arguments fails the round, not your tool.

## ToolSchema

[`ToolSchema`] advertises one tool to the model in the OpenAI function-calling shape. You pass a chat effect's tool list to your backend; you cannot build or inspect one, because the run builds each from the prompt's tool contract. Its wire name is never empty and uses only `[A-Za-z0-9_.-]`, and its parameters schema is always a JSON object. It serializes as a flat `{name, description, parameters}` object, without the `{"type":"function","function":{..}}` wrapper a request uses.

## CompletionErrorKind

[`CompletionErrorKind`] classifies a [`CompletionError`] into a stable kind you can match. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm. A failed read of a non-success body counts as a transport failure, so a transport failure can still carry a status. [Answer a model round](#answer-a-model-round) teaches the errors it classifies.

| Variant | Meaning |
|---|---|
| [`Transport`](CompletionErrorKind::Transport) | The request failed at the transport layer, such as a connection failure or a timeout, including a failed read of a non-success body. |
| [`Backend`](CompletionErrorKind::Backend) | The backend returned a non-success status. |
| [`MalformedResponse`](CompletionErrorKind::MalformedResponse) | The response could not be decoded or was structurally invalid, including an oversized or undecodable catalog body. |
| [`EmptyReply`](CompletionErrorKind::EmptyReply) | The model returned no tool calls and no text other than whitespace. |
| [`Disabled`](CompletionErrorKind::Disabled) | The host explicitly disabled gateway access. |
| [`Config`](CompletionErrorKind::Config) | The client could not be configured, from missing environment, invalid environment, or invalid config. |

## CompletionResult

[`CompletionResult`] holds the outcome of a model round: [`Text`](CompletionResult::Text), a final text reply, or [`ToolCalls`](CompletionResult::ToolCalls), a batch of tool calls the model asked for. You build one to answer a chat effect, or read one from a [`Completion`]. It is `#[non_exhaustive]`, so a `match` needs a `_` arm. [`Completion::from_result`] rejects an empty `ToolCalls` batch but accepts an empty `Text`. [Answer a model round](#answer-a-model-round) teaches it.

## ModelCatalogError

[`ModelCatalogError`] says why [`ModelCatalog::new`] refused its descriptors. Its variant [`DuplicateId`](ModelCatalogError::DuplicateId) means two descriptors shared one [`ModelId`]; its `server` and `name` are the repeated id's parts, from the second occurrence. The enum and the variant are both `#[non_exhaustive]`, so match `DuplicateId { .. }`, and you cannot build one. Remove or rename the repeated model, and build the catalog again. [Describe your models](#describe-your-models) teaches it.

## StreamDelta

[`StreamDelta`] carries one live piece of a streaming reply to the delta callback. [`Text`](StreamDelta::Text) is a fragment of the answer text, and [`Reasoning`](StreamDelta::Reasoning) a fragment of reasoning, never part of the answer. Tool calls arrive only in the finished completion. Forward deltas only for a `Chat` effect whose `stream` flag is true: a section's chat round, not a nested `models.infer` round. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm.

## TemperatureError

[`TemperatureError`] says why [`Temperature::new`] or [`CompletionOptions::with_temperature`] rejected a temperature. [`NotFinite`](TemperatureError::NotFinite) means NaN or an infinity, and [`OutOfRange`](TemperatureError::OutOfRange) means a finite value outside `[0.0, 2.0]`, held in its `value`. NaN reports `NotFinite`, because finiteness is checked first. The enum and `OutOfRange` are both `#[non_exhaustive]`, so match `OutOfRange { value, .. }`. Pass a finite value from `0.0` to `2.0`.

## ThinkingMode

[`ThinkingMode`] says whether a model you describe can emit thinking tokens. Pass it as the last argument of [`ModelDescriptor::new`]. When a role's hard keyword needs thinking the model never does, or forbids thinking the model always does, prepare reports one unmet requirement; bind a matching model and prepare again, or refuse the run. Deserializing accepts only `"never"`, `"always"`, and `"switchable"`; any other string, a capitalized name included, is a serde error. [Describe your models](#describe-your-models) teaches it.

- [`Never`](ThinkingMode::Never): the backend never emits thinking tokens, so a `thinking` role is unmet.
- [`Always`](ThinkingMode::Always): the backend always emits thinking tokens; it satisfies a `thinking` role, and a `no-thinking` role is unmet.
- [`Switchable`](ThinkingMode::Switchable): the client may turn thinking on or off per request.
