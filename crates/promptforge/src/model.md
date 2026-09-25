Model identities and descriptors, role bindings, and the messages, completions, and errors of a model round.

This module holds every type involved in a model round. You describe your deployment's model, set it on the run's context so the prompt's model roles bind to it, and then answer each model round with a completion or an error. All of it is plain data. Nothing here opens a connection, so the same host code can answer a round from a real gateway, from a canned script in a test, or from a recorded log.

# Where this fits

Models enter a run at two points.

**Before the run.** The host sets the current model with [`RunContext::model`](crate::RunContext::model), then calls [`Environment::prepare`](crate::Environment::prepare), which binds every model role the prompt declares to that model.

**During the run.** Each model round reaches the host as an [`Effect::Chat`](crate::effect::Effect::Chat) inside the [`Step::Pending`](crate::Step::Pending) that [`Run::step`](crate::Run::step) returns. The effect has five fields:

- [`Effect::Chat::binding`](crate::effect::Effect#variant.Chat.field.binding) is the [`ModelBinding`] that the round runs under.
- [`Effect::Chat::messages`](crate::effect::Effect#variant.Chat.field.messages) is the conversation, a [`Vec`] of [`Message`] values in wire order.
- [`Effect::Chat::tools`](crate::effect::Effect#variant.Chat.field.tools) is a [`Vec`] of the [`ToolSchema`] values advertised to the model. It is empty when the round advertises no tools.
- [`Effect::Chat::options`](crate::effect::Effect#variant.Chat.field.options) is the round's [`CompletionOptions`], derived from the binding.
- [`Effect::Chat::stream`](crate::effect::Effect#variant.Chat.field.stream) is a [`bool`] that says whether the host forwards live [`StreamDelta`] values while the reply arrives.

A host that runs its own transport passes the messages, tools, and options to [`build_request_body`](crate::transport::build_request_body), reads the reply through [`read_completion_stream`](crate::transport::read_completion_stream), and gets back a [`Completion`] or a [`CompletionError`]. The [`transport`](crate::transport) module page covers that codec. A host with no transport builds a scripted [`Completion`] instead. Either way, the host answers through [`Run::resume`](crate::Run::resume) with [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat), holding `Ok(Box::new(completion))` or `Err(error)`. [`EffectAnswer::Dropped`](crate::effect::EffectAnswer::Dropped) gives up on the round instead.

When a completion asks for tool calls, the run issues one [`Effect::ToolCall`](crate::effect::Effect::ToolCall) per call on a later step. After a served round, the run reports its model events, including [`Event::ModelTurnTruncated`](crate::event::Event::ModelTurnTruncated) when a text reply finished with the reason `"length"`.

# A first model round

This program declares one model role, sets the current model, prepares the context, and answers the run's one model round with scripted text.

````
use std::num::NonZeroU32;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::model::{Completion, CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

let source = concat!(
    "---\n",
    "name: pinger\n",
    "description: asks the model once\n",
    "promptforge: 0\n",
    "models:\n",
    "  writer: {}\n",
    "---\n",
    "\n",
    "# Pinger\n",
    "\n",
    "## Ask\n",
    "\n",
    "```lua\n",
    "models.use('writer', { max_tokens = 256 })\n",
    "return models.infer('ping')\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "pinger");
let prompt = parsed?;

let model = ModelDescriptor::new(
    ModelId::gateway("house-model")?,
    "The host's current model",
    NonZeroU32::new(131_072).ok_or("context is non-zero")?,
    ThinkingMode::Switchable,
);
let ctx = RunContext::new("pinger", 7, Timestamp::UNIX_EPOCH).model(model.clone());
let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);
assert!(requirements.is_satisfied());
assert_eq!(ctx.model_bindings().resolve("writer"), Some(&model));

let mut run = Run::new(Arc::new(prompt), "", ctx);
let result = loop {
    match run.step() {
        Step::Pending { effects, .. } => {
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Chat { binding, messages, .. } => {
                        assert_eq!(binding.alias(), "writer");
                        assert_eq!(binding.id().name(), "house-model");
                        assert_eq!(binding.invocation().max_tokens.map(NonZeroU32::get), Some(256));
                        assert_eq!(messages[0].role(), "user");
                        assert_eq!(messages[0].content(), "ping");
                        let reply = CompletionResult::Text("pong".to_owned());
                        let completion = Completion::from_result(reply, binding.id().name());
                        EffectAnswer::Chat(Ok(Box::new(completion)))
                    }
                    _ => EffectAnswer::Dropped,
                };
                run.resume(id, answer);
            }
        }
        Step::Done { result, .. } => break result,
    }
};

match result {
    RunResult::Ok(text) => assert_eq!(text, "pong"),
    other => panic!("the run should succeed: {other:?}"),
}
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Declare a role.** A prompt never names a concrete model. Its `models:` frontmatter declares roles, here one role called `writer` with no keywords. In the section's Lua, `models.use('writer', { max_tokens = 256 })` selects that role for the section with a generation cap, and `models.infer('ping')` runs one tool-free model round and returns the reply text.
2. **Describe the model.** A [`ModelDescriptor`] names the model with a [`ModelId`] and records a description, the context window in tokens as a [`NonZeroU32`](std::num::NonZeroU32), and a [`ThinkingMode`].
3. **Bind the roles.** [`RunContext::model`](crate::RunContext::model) sets the descriptor as the run's current model. [`Environment::prepare`](crate::Environment::prepare) binds every declared role to it and reports no unmet requirements, because the role declares neither keywords nor a `min_context`. [`RunContext::model_bindings`](crate::RunContext::model_bindings) returns the resulting [`ModelBindings`], where the role label `writer` resolves to the descriptor.
4. **Read the round.** The binding carries the role's alias, the model's id, and the cap from `models.use` in its [`ModelInvocation`]. The conversation is one user [`Message`] holding `ping`.
5. **Answer the round.** [`Completion::from_result`] builds a completion from a [`CompletionResult::Text`] and a model name. The section returns the reply, so the run ends with [`RunResult::Ok`](crate::RunResult::Ok) holding `"pong"`.

# Model identity

A model is named by a [`ModelId`], a two-part identity made of a server namespace and the caller-facing model name. The name is what goes on the wire as the request's model. Gateway models use the namespace `"gateway"`, which is the value of [`ModelId::GATEWAY`], and [`ModelId::gateway`] builds an id in that namespace from the name alone.

Both constructors validate their input. An empty part or a part that holds any Unicode control character fails with a [`ModelIdError`], so an unusable identity never exists. Other non-ASCII text is fine.

````
use promptforge::model::ModelId;

let id = ModelId::new(ModelId::GATEWAY, "claude-sonnet-4-6")?;
assert_eq!(id, ModelId::gateway("claude-sonnet-4-6")?);
assert_eq!(id.server(), "gateway");
assert_eq!(id.name(), "claude-sonnet-4-6");
assert!(ModelId::gateway("café-模型").is_ok());

let error = ModelId::gateway("").err().ok_or("an empty name is rejected")?;
assert_eq!(error.to_string(), "invalid model id: name must not be empty");
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Descriptors and the catalog

A [`ModelDescriptor`] describes one model that the host can serve: its [`ModelId`], a prose description, its context window in tokens, and its [`ThinkingMode`]. The thinking mode says whether the model never, always, or switchably emits thinking tokens. A gateway's model list spells it in lowercase, and [`ThinkingMode`] deserializes from that form.

A host that offers several models collects their descriptors into a [`ModelCatalog`], which keeps them in the host's order and rejects a repeated id with [`ModelCatalogError::DuplicateId`].

````
use std::num::NonZeroU32;

use promptforge::model::{ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ThinkingMode};

let small = ModelDescriptor::new(
    ModelId::gateway("small")?,
    "A tiny model",
    NonZeroU32::new(8_192).ok_or("context is non-zero")?,
    ThinkingMode::Never,
);
let mode: ThinkingMode = serde_json::from_str("\"switchable\"")?;
let analyst = ModelDescriptor::new(
    ModelId::gateway("analyst")?,
    "A careful analysis model",
    NonZeroU32::new(131_072).ok_or("context is non-zero")?,
    mode,
);
assert_eq!(analyst.thinking(), ThinkingMode::Switchable);

let catalog = ModelCatalog::new([small.clone(), analyst])?;
assert_eq!(catalog.models().len(), 2);
assert_eq!(catalog.get(small.id()), Some(&small));
assert!(catalog.contains(&ModelId::gateway("analyst")?));

let Err(error) = ModelCatalog::new([small.clone(), small]) else {
    panic!("a repeated id is rejected");
};
assert!(matches!(error, ModelCatalogError::DuplicateId { .. }));
assert_eq!(error.to_string(), "duplicate model identity in catalog: gateway/small");
# Ok::<(), Box<dyn std::error::Error>>(())
````

In this version no function in the crate takes a [`ModelCatalog`]. A run is given exactly one model, the descriptor passed to [`RunContext::model`](crate::RunContext::model), and every declared role binds to it. The catalog is a host-side collection for choosing that descriptor. The event module declares [`Event::ModelCatalogValidationStarted`](crate::event::Event::ModelCatalogValidationStarted), [`Event::ModelCatalogValidationSucceeded`](crate::event::Event::ModelCatalogValidationSucceeded), and [`Event::ModelCatalogValidationFailed`](crate::event::Event::ModelCatalogValidationFailed), but they are not currently emitted. Building a catalog or preparing a run reports none of them.

# Roles and bindings

A prompt declares the model roles it needs under `models:` in its frontmatter. Each role can list keywords, a `min_context`, and a description, and the [`prompt`](crate::prompt) module page lists what a role declares. At run time, Lua picks a role with `models.default(label)` for the whole prompt, called from the H1, or with `models.use(label, opts?)` for one section, for example `models.use('analyst', { temperature = 0, max_tokens = 1024 })`. A model round with neither fails the run with [`RunErrorKind::Binding`](crate::RunErrorKind::Binding).

On the host side, [`Environment::prepare`](crate::Environment::prepare) binds every declared role to the current model and checks each role against it:

- A role's `min_context` above the model's context window fails with [`RequirementCheck::ContextMinimum`](crate::RequirementCheck::ContextMinimum).
- The hard keyword `thinking` fails against [`ThinkingMode::Never`], and the hard keyword `no-thinking` fails against [`ThinkingMode::Always`]. Both fail with [`RequirementCheck::HardKeyword`](crate::RequirementCheck::HardKeyword). [`ThinkingMode::Switchable`] satisfies both.
- Soft keywords are never checked.

Each failure becomes an [`UnmetRequirement`](crate::UnmetRequirement) in [`Requirements::unmet_requirements`](crate::Requirements::unmet_requirements), with the required and actual values side by side. The role is bound either way, so the host decides whether to refuse the run. With no current model, prepare binds nothing and checks nothing, and a round that selects a role fails at run time.

This prompt declares two roles, and the current model's context window is too small for one of them:

````
use std::num::NonZeroU32;

use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, RequirementCheck, RunContext};

let source = concat!(
    "---\n",
    "name: analysis\n",
    "description: runs a deep analysis\n",
    "promptforge: 0\n",
    "models:\n",
    "  analyst:\n",
    "    keywords: [frontier, thinking]\n",
    "    min_context: 200000\n",
    "    description: Deep analysis\n",
    "  scout:\n",
    "    keywords: [fast]\n",
    "---\n",
    "\n",
    "# Analysis\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "analysis");
let prompt = parsed?;
let model = ModelDescriptor::new(
    ModelId::gateway("house-model")?,
    "The host's current model",
    NonZeroU32::new(32_000).ok_or("context is non-zero")?,
    ThinkingMode::Always,
);
let ctx = RunContext::new("analysis", 7, Timestamp::UNIX_EPOCH).model(model.clone());
let (ctx, requirements) = Environment::new().prepare(&prompt, ctx);

let bindings = ctx.model_bindings();
assert_eq!(bindings.len(), 2);
assert_eq!(bindings.role_id("analyst"), Some(model.id()));
assert_eq!(bindings.resolve("scout"), Some(&model));
assert_eq!(bindings.model(model.id()), Some(&model));
assert!(bindings.resolve("undeclared").is_none());

let [unmet] = requirements.unmet_requirements.as_slice() else {
    panic!("only the context minimum fails");
};
assert_eq!(unmet.role, "analyst");
assert_eq!(unmet.check, RequirementCheck::ContextMinimum);
assert_eq!(unmet.required, "200000");
assert_eq!(unmet.actual, "32000");
# Ok::<(), Box<dyn std::error::Error>>(())
````

The `thinking` keyword passes here because the model's mode is [`ThinkingMode::Always`], and `frontier` and `fast` are soft keywords. Both roles are bound to the one model, so [`ModelBindings::len`] counts two roles while the bindings hold one descriptor.

# Bindings and request options

Every model round runs under a [`ModelBinding`]: a prompt-local alias bound to a model id, together with the frozen invocation parameters that every round under it uses. The run builds one binding per bound role and hands it to the host in [`Effect::Chat::binding`](crate::effect::Effect#variant.Chat.field.binding).

The frozen parameters live in a [`ModelInvocation`], a plain struct with three public optional fields: the sampling [`Temperature`], the generation cap, and the thinking switch.

A binding the run builds carries three things from the role:

- The role's description, or the model descriptor's description when the role declares none.
- The role's keywords, recorded in kebab-case as the binding's capabilities.
- The thinking switch. A `thinking` keyword freezes [`ModelInvocation::thinking`] to `Some(true)`, and a `no-thinking` keyword freezes it to `Some(false)`.

[`ModelBinding::completion_options`] turns a binding into the wire request options in one call. The run builds every [`Effect::Chat::options`](crate::effect::Effect#variant.Chat.field.options) this way, so the options always match the binding.

A host that drives the transport outside a run, or a test that needs a binding, builds one with [`ModelBinding::new`]. A host can also build [`CompletionOptions`] by hand with a builder chain. Every temperature goes through [`Temperature`], which accepts only finite values in the inclusive range `[0.0, 2.0]`, so a bad value never reaches a request.

````
use std::num::NonZeroU32;

use promptforge::model::{
    CompletionOptions, ModelBinding, ModelId, ModelInvocation, Temperature, TemperatureError,
};

let invocation = ModelInvocation {
    temperature: Some(Temperature::new(0.7)?),
    max_tokens: None,
    thinking: Some(true),
};
let binding = ModelBinding::new(
    "analyst",
    "Deep analysis",
    ModelId::gateway("house-model")?,
    invocation,
    NonZeroU32::new(131_072).ok_or("context is non-zero")?,
)
.with_capabilities(vec!["frontier".to_owned(), "thinking".to_owned()]);
assert_eq!(binding.alias(), "analyst");
assert_eq!(binding.capabilities().len(), 2);
assert_eq!(binding.invocation().thinking, Some(true));
assert_eq!(binding.invocation().temperature.map(Temperature::get), Some(0.7));
let _from_binding = binding.completion_options();

let cap = NonZeroU32::new(256).ok_or("max tokens is non-zero")?;
let _by_hand = CompletionOptions::new("house-model")
    .with_temperature(0.2)?
    .with_max_tokens(cap)
    .with_thinking(false);

assert!(matches!(Temperature::new(f64::NAN), Err(TemperatureError::NotFinite)));
let error = Temperature::try_from(2.5_f64).err().ok_or("2.5 is out of range")?;
assert_eq!(error.to_string(), "temperature 2.5 is outside the supported range [0.0, 2.0]");
# Ok::<(), Box<dyn std::error::Error>>(())
````

The generation cap is a [`NonZeroU32`](std::num::NonZeroU32), so a zero cap, which would forbid all output, cannot be expressed. The thinking switch only matters for a [`ThinkingMode::Switchable`] model. When it is set, the request body carries `chat_template_kwargs.enable_thinking`.

# Messages

A [`Message`] is one entry in a round's conversation. The run hands the host the whole conversation in [`Effect::Chat::messages`](crate::effect::Effect#variant.Chat.field.messages), and [`Message::role`] and [`Message::content`] read any entry, including messages built by the run. A host builds its own messages with [`Message::user`], [`Message::assistant`], and [`Message::tool`]. A tool result's first argument is the id of the [`ToolCall`] it answers, as returned by [`ToolCall::id`]. The constructor does not check that match, so the host keeps the ids straight.

````
use promptforge::model::Message;

let question = Message::user("What changed?");
assert_eq!(question.role(), "user");
assert_eq!(question.content(), "What changed?");
assert_eq!(
    serde_json::to_value(&question)?,
    serde_json::json!({ "role": "user", "content": "What changed?" }),
);

let turn = Message::assistant("Two files.");
assert_eq!(turn.role(), "assistant");

let result = Message::tool("call_1", "src/lib.rs, src/model.rs");
assert_eq!(result.role(), "tool");
assert_eq!(result.content(), "src/lib.rs, src/model.rs");
# Ok::<(), Box<dyn std::error::Error>>(())
````

[`Message`] and [`ToolSchema`] serialize to the OpenAI chat-completions wire shape, which is how [`build_request_body`](crate::transport::build_request_body) puts them into a request. Only the run builds a `system` message, a multimodal message, or a [`ToolSchema`]. A host passes the schemas from [`Effect::Chat::tools`](crate::effect::Effect#variant.Chat.field.tools) through to the request unchanged.

# Answering with a completion

A [`Completion`] is one finished model round. Its [`Completion::result`] is a [`CompletionResult`], which is either [`CompletionResult::Text`] for a final text reply or [`CompletionResult::ToolCalls`] for a batch of requested tool calls. Each [`ToolCall`] exposes its id, its tool name, and a typed [`ToolArguments`] view of its arguments, so the host never handles raw JSON. Beside the result, a completion carries the round's metadata: the serving model, the finish reason, the reasoning text, token usage, and timings.

A host with a transport gets its completion from [`read_completion_stream`](crate::transport::read_completion_stream). A host without one, such as a test or a replay, builds it with [`Completion::from_result`], and builds scripted tool calls with [`ToolCall::from_parts`]. A completion built this way reports the model name it was given, and the rest of its metadata is absent.

````
use promptforge::effect::EffectAnswer;
use promptforge::model::{Completion, CompletionResult, Message, ToolCall};

let call = ToolCall::from_parts(
    "call_1",
    "fetch",
    serde_json::json!({ "url": "https://example.com" }),
);
let completion = Completion::from_result(CompletionResult::ToolCalls(vec![call]), "house-model");
assert_eq!(completion.model(), "house-model");
assert_eq!(completion.finish_reason(), None);
assert!(completion.usage().is_none());

let CompletionResult::ToolCalls(calls) = completion.result() else {
    panic!("the round asked for tools");
};
let call = &calls[0];
assert_eq!(call.id(), "call_1");
assert_eq!(call.name(), "fetch");
let arguments = call.arguments();
assert!(arguments.contains("url"));
assert_eq!(arguments.names().collect::<Vec<_>>(), ["url"]);
assert_eq!(arguments.to_json_string(), r#"{"url":"https://example.com"}"#);
let tool_result = Message::tool(call.id(), "<html>example</html>");
assert_eq!(tool_result.role(), "tool");

let answer = EffectAnswer::Chat(Ok(Box::new(completion)));
assert!(matches!(answer, EffectAnswer::Chat(Ok(_))));
````

When a run receives [`CompletionResult::ToolCalls`], it issues one [`Effect::ToolCall`](crate::effect::Effect::ToolCall) per call. A call whose name is outside the tool scope advertised for that round fails as out of scope. [`CompletionResult`] is `#[non_exhaustive]`, and the run fails with an internal error on any variant it does not recognize, so a host answers only with [`CompletionResult::Text`] or [`CompletionResult::ToolCalls`].

# Streaming deltas

While a round streams, [`read_completion_stream`](crate::transport::read_completion_stream) calls the host's `on_delta` callback with each [`StreamDelta`]. [`StreamDelta::Text`] is a fragment of the reply, and [`StreamDelta::Reasoning`] is a fragment of the reasoning side channel. A host forwards them to a live viewer only when [`Effect::Chat::stream`](crate::effect::Effect#variant.Chat.field.stream) is `true`. Tool-call fragments never arrive as deltas, and the final [`Completion`] holds the whole turn either way.

The callback is an [`Fn`], so a callback that builds up text needs interior mutability:

````
use std::cell::RefCell;

use promptforge::model::StreamDelta;

let visible = RefCell::new(String::new());
let on_delta = |delta: StreamDelta| match delta {
    StreamDelta::Text(fragment) => visible.borrow_mut().push_str(&fragment),
    StreamDelta::Reasoning(_) => {}
    _ => {}
};
on_delta(StreamDelta::Reasoning("checking the diff".to_owned()));
on_delta(StreamDelta::Text("hel".to_owned()));
on_delta(StreamDelta::Text("lo".to_owned()));
assert_eq!(visible.into_inner(), "hello");
````

# Failed rounds

A [`CompletionError`] is a failed model round. The host gets one from its transport and answers the round with an [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) holding `Err(error)`. [`CompletionError::kind`] classifies the failure into a [`CompletionErrorKind`], which is stable and matchable. [`CompletionError::is_retryable`] says whether a retry may succeed, and [`CompletionError::is_timeout`] says whether the failure was a timeout. For a backend failure, [`CompletionError::status`] gives the HTTP status and [`CompletionError::backend_body`] gives the backend's error body, which never appears in the [`Display`](std::fmt::Display) text.

````
use promptforge::model::{CompletionError, CompletionErrorKind};

fn report(error: &CompletionError) -> String {
    match error.kind() {
        CompletionErrorKind::Backend => {
            let status = error.status().map_or_else(|| "unknown".to_owned(), |s| s.to_string());
            format!("backend status {status}: {}", error.backend_body().unwrap_or(""))
        }
        _ if error.is_timeout() => "timed out, safe to retry".to_owned(),
        _ if error.is_retryable() => "transient, safe to retry".to_owned(),
        _ => error.to_string(),
    }
}
# let _ = report;
````

The run treats a [`CompletionErrorKind::EmptyReply`] failure as a completed round with no reply, and it reads [`CompletionError::finish_reason`] to tell a clean empty exit from a truncated one. An empty turn with the reason `"stop"` after successful tool calls is a clean exit. A missing reason or `"length"` stays a hard failure.

# Reference

This part covers every item in the module, from identities through bindings and requests to completions and errors.

Three conventions hold across the module. Every struct except [`ModelInvocation`] has private fields, so the host builds one through its constructor or receives it from the run. Builder methods take `self` and return the updated value, so calls chain. [`CompletionErrorKind`], [`CompletionResult`], [`ModelCatalogError`], [`StreamDelta`], [`TemperatureError`], and [`ThinkingMode`] are `#[non_exhaustive]`, so a `match` on them needs a wildcard arm.

## ModelId

[`ModelId`] is the stable identity of one model: a server namespace plus the caller-facing model name. Build one with [`ModelId::new`] or [`ModelId::gateway`].

- [`ModelId::GATEWAY`] is the gateway namespace, the [`&str`](str) value `"gateway"`. Pass it as the `server` argument of [`ModelId::new`].

[`ModelId::new`] takes two arguments and returns a [`Result`] of a [`ModelId`] or a [`ModelIdError`].

- `server`, anything that converts [`Into`] a [`String`], is the identity namespace. Use [`ModelId::GATEWAY`] for gateway models.
- `name`, anything that converts [`Into`] a [`String`], is the caller-facing model name, the one sent on the wire. For a gateway model it is the gateway's model name.

Each part must be non-empty and free of Unicode control characters, which covers C0 controls, DEL, NUL, and C1 controls such as U+0085. Other non-ASCII text such as `"café-模型"` is accepted. `server` is checked before `name`, so when both are bad the error names `server`.

[`ModelId::gateway`] takes only `name`, with the same rules, and returns the same thing as [`ModelId::new`] with [`ModelId::GATEWAY`] as the server.

- [`ModelId::server`] returns the namespace as a [`&str`](str).
- [`ModelId::name`] returns the model name as a [`&str`](str). [`ModelBinding::completion_options`] sends it as the request's model.

[`ModelId`] implements [`Eq`], [`Hash`](std::hash::Hash), and [`Ord`], ordered by server and then by name, so it works as a map key. It does not implement [`Display`](std::fmt::Display), [`FromStr`](std::str::FromStr), or any serde trait.

## ModelIdError

[`ModelIdError`] says why a [`ModelId`] could not be built. [`ModelId::new`] and [`ModelId::gateway`] return it, and hosts never build one. It has no accessors. Read it through its [`Display`](std::fmt::Display) text, `invalid model id: {field} {reason}`, where the field is `server` or `name` and the reason is `must not be empty` or `must not contain a control character`. For example, an empty name gives `invalid model id: name must not be empty`. It implements [`std::error::Error`].

## ThinkingMode

[`ThinkingMode`] says whether a model emits thinking tokens. It is recorded on a [`ModelDescriptor`], and [`Environment::prepare`](crate::Environment::prepare) checks it against a role's hard thinking keywords. Name a variant directly, or deserialize one from the gateway's lowercase form `"never"`, `"always"`, or `"switchable"`.

- [`ThinkingMode::Never`]: the model never emits thinking tokens. Use it for a model without a reasoning channel. A role with the hard `thinking` keyword fails against it, reported with the actual value `"Never"`.
- [`ThinkingMode::Always`]: the model always emits thinking tokens. A role with the hard `no-thinking` keyword fails against it, reported with the actual value `"Always"`.
- [`ThinkingMode::Switchable`]: the client turns thinking on or off per request. Use it for a model that honors `chat_template_kwargs.enable_thinking`. It satisfies both hard keywords. Per-request control goes through [`CompletionOptions::with_thinking`] or [`ModelInvocation::thinking`].

[`ThinkingMode`] implements serde's [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html) only. It does not implement [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html), [`Display`](std::fmt::Display), or [`FromStr`](std::str::FromStr).

## ModelDescriptor

[`ModelDescriptor`] describes one model that the host can serve. The host passes one to [`RunContext::model`](crate::RunContext::model) and receives descriptors back from [`ModelBindings::resolve`], [`ModelBindings::model`], [`ModelCatalog::get`], and [`RunContext::current_model`](crate::RunContext::current_model).

[`ModelDescriptor::new`] takes four arguments and cannot fail.

- `id`, a [`ModelId`], is the model's stable identity.
- `description`, anything that converts [`Into`] a [`String`], is a prose description of the model. It becomes a binding's description when the role declares none.
- `context`, a [`NonZeroU32`](std::num::NonZeroU32), is the context window in tokens. A zero-token window cannot be expressed. Prepare compares it against each role's `min_context`.
- `thinking`, a [`ThinkingMode`], says whether the model emits thinking tokens. Prepare checks it against each role's hard thinking keywords.

Each accessor takes `&self` and returns one field: [`ModelDescriptor::id`] returns a reference to the [`ModelId`], [`ModelDescriptor::description`] returns a [`&str`](str), [`ModelDescriptor::context`] returns the [`NonZeroU32`](std::num::NonZeroU32), and [`ModelDescriptor::thinking`] returns the [`ThinkingMode`]. [`ModelDescriptor`] has no serde support.

## ModelCatalog

[`ModelCatalog`] is the set of models that a host can serve, kept in host order with no repeated ids. A host typically builds it from a gateway's model list or from a pinned offline entry, then picks the run's current model from it.

[`ModelCatalog::new`] takes one argument, `models`, anything that implements [`IntoIterator`] of [`ModelDescriptor`], such as an array or a [`Vec`]. The catalog keeps the iteration order. It returns a [`Result`], and fails with [`ModelCatalogError::DuplicateId`] when two descriptors share an id. The error names the later of the two occurrences. [`ModelCatalog::empty`] returns an empty catalog, and the [`Default`] value is the same empty catalog.

- [`ModelCatalog::models`] returns every descriptor as a slice, in host order.
- [`ModelCatalog::is_empty`] returns `true` when the catalog holds no descriptors.
- [`ModelCatalog::get`] takes a reference to a [`ModelId`] and returns an [`Option`] of a reference to the matching [`ModelDescriptor`], or [`None`]. It is a linear search.
- [`ModelCatalog::contains`] takes a reference to a [`ModelId`] and returns `true` when a descriptor with that id is present.

## ModelCatalogError

[`ModelCatalogError`] says why a [`ModelCatalog`] could not be built. [`ModelCatalog::new`] returns it, and hosts never build one. It implements [`std::error::Error`].

- [`ModelCatalogError::DuplicateId`]: two descriptors share one [`ModelId`], which would make lookups ambiguous. Drop or rename the duplicate and build the catalog again. The variant is itself `#[non_exhaustive]`, so match it as `DuplicateId { server, name, .. }`. Its [`Display`](std::fmt::Display) text is `duplicate model identity in catalog: {server}/{name}`.
  - [`ModelCatalogError::DuplicateId::server`](ModelCatalogError#variant.DuplicateId.field.server), a [`String`], is the repeated id's namespace, which is `"gateway"` for gateway models.
  - [`ModelCatalogError::DuplicateId::name`](ModelCatalogError#variant.DuplicateId.field.name), a [`String`], is the repeated id's model name.

## ModelBindings

[`ModelBindings`] records which model each declared role is bound to, and the descriptor of every model that the run may use. Lookups go from role label to id to descriptor. The host reads them from [`RunContext::model_bindings`](crate::RunContext::model_bindings) after [`Environment::prepare`](crate::Environment::prepare), which is their only writer. In this version every declared role binds to the context's current model. With no current model the bindings stay empty and every lookup returns [`None`]. The [`Default`] value is empty.

- [`ModelBindings::role_id`] takes `label`, a [`&str`](str) naming a role declared under `models:`, and returns an [`Option`] of a reference to the bound [`ModelId`]. It is [`None`] when the role is undeclared or no current model was set.
- [`ModelBindings::resolve`] takes the same `label` and returns an [`Option`] of a reference to the bound model's [`ModelDescriptor`], or [`None`] when the role is unbound.
- [`ModelBindings::model`] takes `id`, a reference to a [`ModelId`], and returns an [`Option`] of a reference to that model's [`ModelDescriptor`] when the run may use it, or [`None`].
- [`ModelBindings::len`] returns the number of bound roles as a [`usize`]. Two roles bound to one model count as two, while the descriptor table holds that model once.
- [`ModelBindings::is_empty`] returns `true` when no roles are bound.

## ModelBinding

[`ModelBinding`] is one prompt-local alias bound to a model id and its frozen invocation parameters. The host receives it in [`Effect::Chat::binding`](crate::effect::Effect#variant.Chat.field.binding), or builds one with [`ModelBinding::new`].

[`ModelBinding::new`] takes five arguments and cannot fail. It returns a binding with an empty capabilities list.

- `alias`, anything that converts [`Into`] a [`String`], is the exact prompt-local alias. It is not validated.
- `description`, anything that converts [`Into`] a [`String`], is the role's description.
- `id`, a [`ModelId`], is the bound model's identity.
- `invocation`, a [`ModelInvocation`], is the frozen per-request fields. Every field is optional, so `ModelInvocation { temperature: None, max_tokens: None, thinking: None }` is valid.
- `context`, a [`NonZeroU32`](std::num::NonZeroU32), is the model's context window in tokens. It is required at construction, so a binding never exists half-built.

Two builder methods adjust a binding. Each takes it by value and cannot fail.

- [`ModelBinding::with_capabilities`] takes a [`Vec`] of [`String`], the role's full keyword set in kebab-case, and replaces any previous set.
- [`ModelBinding::with_invocation`] takes a [`ModelInvocation`] and replaces the binding's invocation.

The accessors take `&self` and cannot fail.

- [`ModelBinding::alias`] returns the alias as a [`&str`](str).
- [`ModelBinding::description`] returns the description as a [`&str`](str). On a binding the run built, it is the role's description, or the model descriptor's description when the role declares none.
- [`ModelBinding::id`] returns a reference to the bound [`ModelId`].
- [`ModelBinding::invocation`] returns a reference to the [`ModelInvocation`].
- [`ModelBinding::context`] returns the context window as a [`NonZeroU32`](std::num::NonZeroU32).
- [`ModelBinding::capabilities`] returns the keyword set as a slice of [`String`]. It is empty on a binding built with [`ModelBinding::new`] alone.
- [`ModelBinding::completion_options`] returns the [`CompletionOptions`] for a request under this binding. The wire model is the [`ModelId::name`] of [`ModelBinding::id`], and the temperature, generation cap, and thinking switch are copied from the invocation. Pass the result to [`build_request_body`](crate::transport::build_request_body).

## ModelInvocation

[`ModelInvocation`] holds the frozen per-request fields that every round under a binding uses. It is the one struct in this module built with a struct literal. The host reads it from [`ModelBinding::invocation`]. It has no [`Default`].

- [`ModelInvocation::temperature`], an [`Option`] of [`Temperature`], is the sampling temperature, when one was set. A [`Temperature`] is always valid.
- [`ModelInvocation::max_tokens`], an [`Option`] of [`NonZeroU32`](std::num::NonZeroU32), is the generation cap, when one was set. A zero cap cannot be expressed.
- [`ModelInvocation::thinking`], an [`Option`] of [`bool`], is the thinking switch sent as `chat_template_kwargs.enable_thinking`, when set. The run sets `Some(true)` for a role with the `thinking` keyword and `Some(false)` for one with `no-thinking`.

## Temperature

[`Temperature`] is a validated sampling temperature, finite and within `[0.0, 2.0]` inclusive. Building one is the only way to place a temperature into a request.

[`Temperature::new`] takes `value`, an [`f64`], and returns a [`Result`] of a [`Temperature`] or a [`TemperatureError`]. `0.0`, `0.7`, and `2.0` are accepted. NaN and infinities fail with [`TemperatureError::NotFinite`], which is checked first, and a finite value below `0.0` or above `2.0` fails with [`TemperatureError::OutOfRange`]. [`Temperature`] also implements [`TryFrom`] of [`f64`] with the same rules and [`TemperatureError`] as its error type.

[`Temperature::get`] takes the temperature by value and returns the [`f64`].

## TemperatureError

[`TemperatureError`] says why a temperature was rejected. [`Temperature::new`], [`Temperature`]'s [`TryFrom`] conversion, and [`CompletionOptions::with_temperature`] return it, and hosts never build one. It implements [`std::error::Error`].

- [`TemperatureError::NotFinite`]: the value was NaN or an infinity. Supply a finite value in `[0.0, 2.0]`. Its [`Display`](std::fmt::Display) text is `temperature must be finite`.
- [`TemperatureError::OutOfRange`]: the value was finite but outside `[0.0, 2.0]`, such as `-0.1` or `2.5`. Clamp or correct it. The variant is itself `#[non_exhaustive]`, so match it as `OutOfRange { value, .. }`. Its [`Display`](std::fmt::Display) text is `temperature {value} is outside the supported range [0.0, 2.0]`.
  - [`TemperatureError::OutOfRange::value`](TemperatureError#variant.OutOfRange.field.value), an [`f64`], is the rejected value.

## CompletionOptions

[`CompletionOptions`] holds the per-call fields merged into a chat-completions request body: the model name sent on the wire and the optional temperature, generation cap, and thinking switch. The host receives them in [`Effect::Chat::options`](crate::effect::Effect#variant.Chat.field.options), derives them with [`ModelBinding::completion_options`], or builds them by hand. They go to [`build_request_body`](crate::transport::build_request_body). There is no [`Default`].

[`CompletionOptions::new`] takes `model`, anything that converts [`Into`] a [`String`], which is the model name sent on the wire. Normally that is the [`ModelId::name`] of the bound model. It is not validated. The new options leave the temperature, generation cap, and thinking switch unset, and three builder methods set them. Each takes the options by value.

- [`CompletionOptions::with_temperature`] takes `temperature`, an [`f64`], and returns a [`Result`] of the updated options or a [`TemperatureError`], under the same rules as [`Temperature::new`]. The options are consumed on failure.
- [`CompletionOptions::with_max_tokens`] takes `max_tokens`, a [`NonZeroU32`](std::num::NonZeroU32), the most tokens to generate, and cannot fail.
- [`CompletionOptions::with_thinking`] takes `thinking`, a [`bool`], and cannot fail. Once set, the request body carries `chat_template_kwargs.enable_thinking` with that value.

## Message

[`Message`] is one chat message in a round's conversation. The host receives the conversation in [`Effect::Chat::messages`](crate::effect::Effect#variant.Chat.field.messages), in wire order, or builds messages with three constructors. None of them can fail.

- [`Message::user`] takes `content`, anything that converts [`Into`] a [`String`], and returns a message with the role `user`.
- [`Message::assistant`] takes `content` the same way and returns a plain `assistant` turn with no tool calls.
- [`Message::tool`] takes `tool_call_id` and `content`, each anything that converts [`Into`] a [`String`], and returns a message with the role `tool`. `tool_call_id` must be the [`ToolCall::id`] of the call this result answers. The constructor does not check it.

There is no constructor for a `system` message or a multimodal message. Only the run builds those.

- [`Message::role`] returns the role as a [`&str`](str): `system`, `user`, `assistant`, or `tool`.
- [`Message::content`] returns the message text as a [`&str`](str). For a multimodal message, whose content is a list of parts, it returns `""`.

[`Message`] implements serde's [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html) in the OpenAI chat-completions wire shape. A plain message serializes to `{"role":..,"content":..}`, and the `tool_call_id` and `tool_calls` keys appear only when set.

## ToolSchema

[`ToolSchema`] is one tool advertised to the model in the OpenAI function-calling shape: its wire name, a one-sentence description, and the JSON Schema of its parameters. The host receives the round's schemas in [`Effect::Chat::tools`](crate::effect::Effect#variant.Chat.field.tools), and an empty list advertises none. Only the run builds a [`ToolSchema`], and it has no public accessors. The host passes the schemas through to [`build_request_body`](crate::transport::build_request_body). It implements serde's [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html), and inside a request body each schema is wrapped as `{"type":"function","function":{"name":..,"description":..,"parameters":..}}`.

## Completion

[`Completion`] is one finished model round: the text or tool-call outcome plus the round's metadata. The host answers an [`Effect::Chat`](crate::effect::Effect::Chat) with an [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) holding `Ok(Box::new(completion))`. A host with a transport gets one from [`read_completion_stream`](crate::transport::read_completion_stream).

[`Completion::from_result`] builds a completion without a transport, for a test or a replay. It takes two arguments and cannot fail.

- `result`, a [`CompletionResult`], is the round's outcome: [`CompletionResult::Text`] for a reply or [`CompletionResult::ToolCalls`] for a tool batch.
- `model`, anything that converts [`Into`] a [`String`], is the model name that [`Completion::model`] reports. It is not validated.

On a completion built this way, every optional metadata accessor below returns [`None`], and both the request and response bodies are JSON `null`.

Each accessor takes `&self` and cannot fail.

- [`Completion::result`] returns a reference to the [`CompletionResult`]. Match on it with a wildcard arm.
- [`Completion::model`] returns the serving model as a [`&str`](str), as the backend named it in the response body. It is empty when the body named none.
- [`Completion::finish_reason`] returns the finish reason as an [`Option`] of [`&str`](str), such as `"stop"` or `"length"`, or [`None`] when the backend supplied none.
- [`Completion::reasoning_content`] returns the reasoning side channel as an [`Option`] of [`&str`](str). It is never promoted into the answer.
- [`Completion::usage`] returns an [`Option`] of a reference to the backend's token accounting, a [`Usage`](crate::metrics::Usage).
- [`Completion::llama_timings`] returns an [`Option`] of a reference to the llama.cpp timings, a [`LlamaTimings`](crate::metrics::LlamaTimings), when that backend served the round.
- [`Completion::client_timing`] returns an [`Option`] of a reference to the timing measured on the client's own clock, a [`ClientTiming`](crate::metrics::ClientTiming), when the transport measured one.

The [`metrics`](crate::metrics) module page covers the three metric types. A host that logs answers converts a reference to a [`Completion`] into a [`ChatAnswerRecord`](crate::effect::ChatAnswerRecord) through [`From`]. [`Completion`] is not [`Clone`].

## CompletionResult

[`CompletionResult`] is the outcome of a round. The host reads it from [`Completion::result`], or builds a variant to pass to [`Completion::from_result`].

- [`CompletionResult::Text`] holds a [`String`], the model's final text reply. Display or record the text. The run resumes the section with it.
- [`CompletionResult::ToolCalls`] holds a [`Vec`] of [`ToolCall`], the requested tool calls. Read each call's id, name, and arguments. The run issues one [`Effect::ToolCall`](crate::effect::Effect::ToolCall) per call.

The run fails with an internal error on any variant it does not recognize, so answer only with these two.

## ToolCall

[`ToolCall`] is one requested tool call: its id, the tool's name, and its arguments. The host receives calls inside [`CompletionResult::ToolCalls`]. The model sends the arguments as a JSON-encoded string, and the call holds them parsed, or as a JSON string when they are not valid JSON.

[`ToolCall::from_parts`] builds a call for a scripted or replayed round. It takes three arguments and cannot fail.

- `id`, anything that converts [`Into`] a [`String`], is the id of the call, which the tool result echoes back through [`Message::tool`]. It is not validated.
- `name`, anything that converts [`Into`] a [`String`], is the tool to invoke, named by its prompt-local alias as advertised to the model. The run fails the call as out of scope when the name is outside the round's advertised tool scope.
- `arguments`, a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the argument payload, normally a JSON object.

The accessors take `&self` and cannot fail.

- [`ToolCall::id`] returns the call's id as a [`&str`](str). Pass it as the `tool_call_id` of [`Message::tool`].
- [`ToolCall::name`] returns the tool name as a [`&str`](str).
- [`ToolCall::arguments`] returns a [`ToolArguments`] view that borrows from the call.

## ToolArguments

[`ToolArguments`] is a read-only view of one [`ToolCall`]'s arguments. The host gets one from [`ToolCall::arguments`] and never builds one. It borrows from its call.

- [`ToolArguments::to_json_string`] returns the arguments as canonical JSON text in a [`String`]. When the wire arguments were not valid JSON, the call holds them as a JSON string, so this returns that string JSON-quoted.
- [`ToolArguments::is_empty`] returns `true` for a JSON `null` payload or an empty object, and `false` for anything else.
- [`ToolArguments::contains`] takes `key`, a [`&str`](str), and returns `true` when the arguments are a JSON object with that top-level key. It returns `false` when the key is absent or the arguments are not an object.
- [`ToolArguments::names`] returns an [`Iterator`] over the top-level argument names as [`&str`](str) values when the arguments are an object, or an empty iterator otherwise.

## StreamDelta

[`StreamDelta`] is one live increment of a streaming round. Reply text and reasoning stay separate so a viewer can render them differently. The host receives deltas in the `on_delta` callback it passes to [`read_completion_stream`](crate::transport::read_completion_stream), and forwards them only when [`Effect::Chat::stream`](crate::effect::Effect#variant.Chat.field.stream) is `true`. Tool-call fragments never arrive as deltas. They are held back until the batch is complete and validated.

- [`StreamDelta::Text`] holds a [`String`], a fragment of the reply text. Append it to the visible reply.
- [`StreamDelta::Reasoning`] holds a [`String`], a fragment of the reasoning side channel, never part of the answer. Render it apart from the reply, or ignore it.

## CompletionError

[`CompletionError`] describes why a model round failed. The host gets one from [`read_completion_stream`](crate::transport::read_completion_stream) or [`read_body_capped`](crate::transport::read_body_capped), or converts a [`ClientError`](crate::transport::ClientError) into one through [`From`]. The host answers the round with an [`EffectAnswer::Chat`](crate::effect::EffectAnswer::Chat) holding `Err(error)`. Each method takes `&self`, has no arguments, and cannot fail.

- [`CompletionError::kind`] returns the stable [`CompletionErrorKind`]. Branch on it instead of matching message text.
- [`CompletionError::is_retryable`] returns `true` for transport failures, malformed responses, failures reading a backend error body, and backend statuses of 500 or above. It returns `false` for everything else, including a backend status below 500, an empty reply, disabled access, and configuration failures.
- [`CompletionError::is_timeout`] returns `true` when the failure was a transport timeout. The check looks for a [`ClientTimeout`](crate::transport::ClientTimeout) in the error's source, so a transport must wrap its own timeout error in [`ClientTimeout`](crate::transport::ClientTimeout) before boxing it, or this returns `false`.
- [`CompletionError::status`] returns the HTTP status as an [`Option`] of [`u16`] for a backend failure or a failure reading the backend error body. It is [`None`] otherwise.
- [`CompletionError::backend_body`] returns the backend's error body as an [`Option`] of [`&str`](str), bounded in size and with control characters escaped. It is [`Some`] only for a backend failure. The body never appears in the [`Display`](std::fmt::Display) text, so reading it is an opt-in diagnostic.
- [`CompletionError::finish_reason`] returns the finish reason as an [`Option`] of [`&str`](str) for an empty-reply failure whose backend supplied one. It is [`None`] for every other failure.

Its [`Display`](std::fmt::Display) text is the underlying message, such as `http transport failure` or `non-success backend status 503`. It implements [`std::error::Error`], and [`source`](std::error::Error::source) reaches the underlying transport cause. It also converts back into a [`ClientError`](crate::transport::ClientError) through [`From`]. [`CompletionError`] is not [`Clone`].

## CompletionErrorKind

[`CompletionErrorKind`] is the matchable classification of a [`CompletionError`], returned by [`CompletionError::kind`]. Hosts never build one, but they name its variants to compare against or match on. It does not implement [`Display`](std::fmt::Display).

- [`CompletionErrorKind::Transport`]: the HTTP request failed at the transport layer, such as a lost connection or a timeout, or the body of a non-success response could not be read. A retry may succeed. Check [`CompletionError::is_timeout`] to spot a timeout, and [`CompletionError::status`] for a body-read failure, which still carries its status.
- [`CompletionErrorKind::Backend`]: the backend returned a non-success HTTP status. Read [`CompletionError::status`] and, when needed, [`CompletionError::backend_body`]. It is retryable only for a status of 500 or above.
- [`CompletionErrorKind::MalformedResponse`]: the response could not be decoded or was structurally invalid. That includes a stream that passed its byte cap, ended without the `[DONE]` sentinel, or cut a tool-call batch short. A retry may succeed.
- [`CompletionErrorKind::EmptyReply`]: the model returned neither tool calls nor text. Read [`CompletionError::finish_reason`]. It is not retryable. The run treats it as a completed round with no reply.
- [`CompletionErrorKind::Disabled`]: the host disabled gateway access. It is not retryable until the host enables access again.
- [`CompletionErrorKind::Config`]: the client could not be configured, because of a missing or non-Unicode environment variable, a bad endpoint, or an invalid configuration, or the shared model set's lock was poisoned. It is not retryable. Fix the configuration.

