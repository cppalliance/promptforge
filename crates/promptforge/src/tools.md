Tool identities, descriptors, and catalogs, plus the output and error types for answering a tool call.

A prompt calls tools by prompt-local aliases, but the tools themselves belong to the host. This module is how the host describes its tools to a run and answers their calls. The host describes each tool as plain data, collects the descriptions into a catalog, and lets [`Environment::prepare`](crate::Environment::prepare) bind the prompt's aliases against it. The run never holds an implementation. Each call reaches the host as an effect that names the tool's stable id, and the host runs its own code and answers with output marked trusted or untrusted. That puts every tool call under the host's control, and it lets the engine guard model input against text the host does not vouch for.

# Where this fits

The host builds a [`ToolCatalog`] from the tools of its activated capabilities and installs it with [`Environment::tools`](crate::Environment::tools). [`Environment::prepare`](crate::Environment::prepare) then fills the prompt's tool slots into the context, where [`RunContext::tool_bindings`](crate::RunContext::tool_bindings) reads them back. When a slot's capability contributed nothing to the catalog, prepare adds that capability to [`Requirements::missing_required`](crate::Requirements::missing_required).

Once [`Run::new`](crate::Run::new) has consumed the context, any [`Step::Pending`](crate::Step::Pending) from [`Run::step`](crate::Run::step) can hold an [`Effect::ToolCall`](crate::effect::Effect::ToolCall). The run issues one when a section's script calls a bound tool, or when a model round requests one. The effect has three fields:

- [`Effect::ToolCall::tool`](crate::effect::Effect#variant.ToolCall.field.tool), a [`ToolId`], is the stable identity of the tool to run.
- [`Effect::ToolCall::alias`](crate::effect::Effect#variant.ToolCall.field.alias), a [`String`], is the prompt-local alias used by the call.
- [`Effect::ToolCall::args`](crate::effect::Effect#variant.ToolCall.field.args), a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), holds the call's arguments.

The host looks up the id in its own implementation table, never the alias. It runs the tool with the arguments and calls [`Run::resume`](crate::Run::resume) with an [`EffectAnswer::ToolCall`](crate::effect::EffectAnswer::ToolCall), which holds a [`Result`] of a [`ToolOutput`] or a [`ToolError`].

The engine then applies its trust rule and reports [`Event::ToolCallSucceeded`](crate::event::Event::ToolCallSucceeded) or [`Event::ToolCallFailed`](crate::event::Event::ToolCallFailed), followed by [`Event::ToolResult`](crate::event::Event::ToolResult). The [`Event::ToolResult::trusted`](crate::event::Event#variant.ToolResult.field.trusted) field records the trust marking. [`Event::ToolResult`](crate::event::Event::ToolResult) is always reported for a model-issued call, and for a script call only on success. When a model requests a batch of calls, the batch is first reported unexecuted as [`Event::AssistantToolCalls`](crate::event::Event::AssistantToolCalls). After the answer, the model round continues or the script resumes.

For a run log, [`EffectAnswer::record`](crate::effect::EffectAnswer::record) turns the answer into an [`AnswerRecord::ToolCall`](crate::effect::AnswerRecord::ToolCall). It holds either a [`ToolAnswerRecord`](crate::effect::ToolAnswerRecord), with the output text and whether it was trusted, or the error's display text.

# A tool call from start to finish

This program describes one tool, installs it, prepares a prompt that binds it, and answers the tool call when the prompt's script makes it.

````
use std::collections::HashMap;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolError, ToolId, ToolOutput};
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};
use serde_json::Value;

fn echo(args: &Value) -> Result<ToolOutput, ToolError> {
    let value = args
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::message("echo needs a string `value` argument"))?;
    Ok(ToolOutput::trusted(value))
}

let source = concat!(
    "---\n",
    "name: echoer\n",
    "description: echoes a value\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - example/tools\n",
    "tools:\n",
    "  echo: example/tools/echo\n",
    "---\n",
    "\n",
    "# Echoer\n",
    "\n",
    "## Only\n",
    "\n",
    "```lua\n",
    "return tools.call('echo', { value = 'hi' })\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "echoer");
let prompt = Arc::new(parsed?);

let id = ToolId::parse("example/tools/echo")?;
let descriptor = ToolDescriptor::new(
    id.clone(),
    "echo",
    "Echo the value argument.",
    serde_json::json!({"type": "object", "properties": {"value": {"type": "string"}}}),
);
let catalog = ToolCatalog::new(&[descriptor])?;

let mut table: HashMap<ToolId, fn(&Value) -> Result<ToolOutput, ToolError>> = HashMap::new();
table.insert(id.clone(), echo);

let env = Environment::new().tools(catalog);
let ctx = RunContext::new("echoer", 7, Timestamp::UNIX_EPOCH);
let (ctx, requirements) = env.prepare(&prompt, ctx);
assert!(requirements.is_satisfied());
assert_eq!(ctx.tool_bindings().alias_id("echo"), Some(&id));

let mut run = Run::new(Arc::clone(&prompt), "", ctx);
let result = loop {
    match run.step() {
        Step::Pending { effects, .. } => {
            for (effect_id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::ToolCall { tool, alias, args } => {
                        assert_eq!(alias, "echo");
                        let output = match table.get(&tool) {
                            Some(implementation) => implementation(&args),
                            None => Err(ToolError::message(
                                "the tool the call names has no implementation in the host's table",
                            )),
                        };
                        EffectAnswer::ToolCall(output)
                    }
                    _ => EffectAnswer::Dropped,
                };
                run.resume(effect_id, answer);
            }
        }
        Step::Done { result, .. } => break result,
    }
};

match result {
    RunResult::Ok(text) => assert_eq!(text, "hi"),
    other => panic!("the run should succeed: {other:?}"),
}
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Describe the tool.** [`ToolId::parse`] turns the text `example/tools/echo` into the tool's [`ToolId`]. [`ToolDescriptor::new`] pairs the id with a wire name, a one-sentence description for the model, and a JSON Schema for the arguments. The descriptor is data only.
2. **Build the catalog.** [`ToolCatalog::new`] validates the descriptors and returns the catalog. [`Environment::tools`](crate::Environment::tools) installs it on the deployment's [`Environment`](crate::Environment).
3. **Keep the implementations.** The host's own [`HashMap`](std::collections::HashMap) maps each [`ToolId`] to a function. Any table keyed by [`ToolId`] works, because [`ToolId`] is hashable and ordered.
4. **Prepare.** The prompt declares the capability `example/tools` and binds the alias `echo` to the tool `example/tools/echo`. [`Environment::prepare`](crate::Environment::prepare) fills that slot from the catalog by identity, so [`ToolBindings::alias_id`] returns the id.
5. **Answer the call.** The section's Lua calls `tools.call('echo', { value = 'hi' })`, which reaches the host as an [`Effect::ToolCall`](crate::effect::Effect::ToolCall). The host looks up [`Effect::ToolCall::tool`](crate::effect::Effect#variant.ToolCall.field.tool) in its table, runs the function with [`Effect::ToolCall::args`](crate::effect::Effect#variant.ToolCall.field.args), and answers with [`EffectAnswer::ToolCall`](crate::effect::EffectAnswer::ToolCall). An id missing from the table gets a [`ToolError`] instead, built with [`ToolError::message`].
6. **Read the result.** The echo function answers with [`ToolOutput::trusted`], so the script receives the text unchanged and returns it. The run ends with [`RunResult::Ok`](crate::RunResult::Ok) holding `"hi"`.

# How prepare binds tool slots

A prompt declares its tool slots under the `tools:` frontmatter key. Each slot binds a prompt-local alias to an exact tool path. The first two segments of a tool path name the capability that contributes the tool, so `example/web/fetch` belongs to `example/web`. [`Environment::prepare`](crate::Environment::prepare) fills each slot by identity with [`ToolCatalog::get`], and each slot ends one of three ways.

- **The catalog holds the tool.** The slot is bound. The [`ToolBindings`] hold the catalog's descriptor exactly as the host supplied it.
- **The tool's capability contributed nothing to the catalog.** The slot stays unbound, the capability is added to [`Requirements::missing_required`](crate::Requirements::missing_required), and the requirements are not satisfied.
- **The capability contributed other tools, but not this one.** The slot stays unbound and prepare reports nothing. The failure happens at run time, with the alias named, when the prompt advertises the alias to a model.

So an empty [`ToolBindings`] after prepare can mean three things: the prompt declared no tool slots, a slot's capability was missing and was reported, or a slot's capability was present without that tool and nothing was reported.

Prepare's slot fill is the only writer of the bindings. A host cannot add bindings, and a context that was never prepared holds empty bindings and an empty catalog. Two aliases can bind the same tool. The bindings then hold that tool's descriptor once, [`ToolBindings::len`] counts both aliases, and both aliases resolve to the same id.

When the run advertises a bound tool to a model, it uses the prompt-local alias and the descriptor held by the binding. The model never sees the tool's global id.

This prompt declares two slots in one capability, and the catalog holds only one of the two tools:

````
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::{Environment, Prompt, RunContext};

let source = concat!(
    "---\n",
    "name: reader\n",
    "description: reads a page\n",
    "promptforge: 0\n",
    "tools:\n",
    "  fetch: example/web/fetch\n",
    "  search: example/web/search\n",
    "---\n",
    "\n",
    "# Reader\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "reader");
let prompt = parsed?;

let id = ToolId::parse("example/web/fetch")?;
let fetch = ToolDescriptor::new(
    id.clone(),
    "fetch",
    "Fetch a web page over HTTP.",
    serde_json::json!({"type": "object", "properties": {"url": {"type": "string"}}}),
);
let env = Environment::new().tools(ToolCatalog::new(&[fetch.clone()])?);
let (ctx, requirements) = env.prepare(&prompt, RunContext::new("reader", 7, Timestamp::UNIX_EPOCH));

assert!(requirements.is_satisfied());
let bindings = ctx.tool_bindings();
assert_eq!(bindings.len(), 1);
assert_eq!(bindings.alias_id("fetch"), Some(&id));
assert_eq!(bindings.resolve("fetch"), Some(&fetch));
assert_eq!(bindings.tool(&id), Some(&fetch));
assert!(bindings.alias_id("search").is_none());
assert_eq!(ctx.tools().tools(), [fetch]);
# Ok::<(), Box<dyn std::error::Error>>(())
````

The `search` slot's capability `example/web` is in the catalog, so prepare reports nothing and the requirements are satisfied. The `search` alias stays unbound, and advertising it fails the run.

# What a prompt does with its tools

A host rarely writes prompts, but it helps to know which Lua calls turn into tool effects. Binding happens at prepare. At run time, Lua only chooses which bound aliases the model sees. `tools.always(alias)` advertises an alias to every section and is usually called from the H1. `tools.add(alias)` advertises it to the current section. Using an alias that the prompt's `tools:` frontmatter does not declare fails the run.

The model tool loop lives inside `models.loop`. The run dispatches each tool call requested by the model, appends the results to the message list, and repeats until the model returns terminal text. Each of those calls reaches the host as an [`Effect::ToolCall`](crate::effect::Effect::ToolCall). Inside that loop, untrusted tool output reaches the model wrapped in nonce-tagged `<untrusted_input_...>` markers.

Three more Lua calls work with tools.

- `tools.call(alias, args)` calls any bound tool directly without advertising it. The worked example above uses it. It also becomes an [`Effect::ToolCall`](crate::effect::Effect::ToolCall).
- `tools['add_local'](name, description, params, fn)` defines a local tool backed by a Lua function. The engine answers a local tool itself, so it never becomes an [`Effect::ToolCall`](crate::effect::Effect::ToolCall).
- `tools.calls[alias]` reads the section's call count for an alias. Counts are taken at dispatch, before the host runs the tool.

# Trusted and untrusted output

Every successful answer is a [`ToolOutput`], and the host must mark it as trusted or untrusted when it builds one. [`ToolOutput`] has exactly two constructors, so the marking cannot be forgotten.

- [`ToolOutput::trusted`] is for text the host vouches for, produced by its own first-party code. The engine appends it to the model verbatim.
- [`ToolOutput::untrusted`] is for external data that an attacker can influence, such as web pages and third-party API responses. The engine wraps it in a nonce-guarded envelope before any model or the calling script sees it.

The answer record keeps the marking, so a run log shows which outputs the host vouched for:

````
use promptforge::effect::{AnswerRecord, EffectAnswer};
use promptforge::tools::{OutputTrust, ToolOutput};

let page = ToolOutput::untrusted("<html>");
assert_eq!(page.trust(), OutputTrust::Untrusted);
assert_eq!(page.text(), "<html>");

let AnswerRecord::ToolCall(Ok(record)) = EffectAnswer::ToolCall(Ok(page)).record() else {
    panic!("a successful call records its output");
};
assert!(!record.trusted);
````

**Structured output needs trusted output.** A descriptor built with [`ToolDescriptor::structured`] marks the tool's output as one JSON value. A script-initiated call then resumes the output into the script as a Lua table instead of a string, and output text that is not valid JSON becomes the tool's error. The model tool loop ignores the marking and always adds tool results to the conversation as text. The nonce wrap for untrusted output runs before the JSON parse. So an untrusted output from a structured tool fails a script call with a "returned invalid JSON" tool error, even when its raw text is valid JSON. Answer a structured tool with [`ToolOutput::trusted`].

# Reporting a failure

A tool that fails answers with a [`ToolError`] instead of a [`ToolOutput`]. The error's message is shown to the model, so it must hold no secrets or internal detail. Put the underlying cause behind [`ToolError::with_source`] instead. The cause stays available to host code through [`source`](std::error::Error::source), and the error's [`Display`](std::fmt::Display) output is the message alone.

What happens next depends on who made the call.

- **A model-issued call.** The engine turns the [`ToolError`] into the call's result, with its message nonce-wrapped as untrusted. The model reads the failure and its round continues, so the run does not fail. The failure is reported as an [`Event::ToolResult`](crate::event::Event::ToolResult).
- **A script-issued call.** The error propagates to the Lua caller, and no [`Event::ToolResult`](crate::event::Event::ToolResult) is reported.

[`ToolErrorKind`] classifies a failure for host code, set with [`ToolError::with_kind`] and read with [`ToolError::kind`]. The engine does not read the kind when it dispatches. [`ToolError::is_retryable`] says whether retrying the same call could succeed, and [`ToolError::is_cancelled`] says whether the call was cancelled.

The answer record of a failed call holds only the error's display text. The boxed cause is not recorded.

````
use promptforge::effect::{AnswerRecord, EffectAnswer};
use promptforge::tools::{ToolError, ToolErrorKind};

let io = std::io::Error::other("connection refused by 10.0.0.7:8443");
let error = ToolError::with_source("the search backend is unavailable", io)
    .with_kind(ToolErrorKind::Transport);
assert_eq!(error.to_string(), "the search backend is unavailable");
assert!(std::error::Error::source(&error).is_some());
assert!(error.is_retryable());
assert!(!error.is_cancelled());

let AnswerRecord::ToolCall(Err(text)) = EffectAnswer::ToolCall(Err(error)).record() else {
    panic!("a failed call records its message");
};
assert_eq!(text, "the search backend is unavailable");
````

# Reference

This part covers every item in the module, in the order a host meets them: identities, descriptors, the catalog, the bindings, and then the answer types.

Two conventions hold across the module. Every struct is `#[non_exhaustive]`, so a host cannot build one with a struct literal. Every enum is `#[non_exhaustive]`, so a `match` on one needs a wildcard arm.

## ToolId

[`ToolId`] is the stable identity of a tool: a three-segment `namespace/pack/name` name. It is the catalog key, and [`Effect::ToolCall::tool`](crate::effect::Effect#variant.ToolCall.field.tool) holds one. The wire name advertised to a model is deliberately not identity. A host gets a [`ToolId`] from [`ToolId::parse`] or by deserializing its string form.

[`ToolId::parse`] takes one argument.

- `id`, a [`&str`](str), is the full tool id, for example `"promptforge/web/fetch"` or `"org.rustalliance/core/search"`. It must have exactly three segments separated by `/`. Each segment must be non-empty and use only lowercase ASCII letters, digits, `-`, `_`, and `.`. The namespace is a reverse-DNS name or the reserved first-party prefix `promptforge`. Comparison is case-sensitive, and an `@` version pin is rejected because v1 names are unversioned.

It returns the validated [`ToolId`], or a [`ToolIdError`] whose [`ToolIdError::field`] is `"id"`. The kind is [`ToolIdErrorKind::SegmentCount`] for `"promptforge/web"`, [`ToolIdErrorKind::Empty`] for `"promptforge//fetch"`, and [`ToolIdErrorKind::Control`] for `"Promptforge/web/fetch"`.

The other methods take `&self`, have no arguments, and cannot fail.

- [`ToolId::name`] returns the short name, the last of the three segments, as a [`&str`](str). For `promptforge/web/fetch` it is `"fetch"`.
- [`ToolId::capability`] returns the [`CapabilityId`](crate::capabilities::CapabilityId) of the capability that contributed the tool, built from the first two segments with no lookup and no re-parse. For `promptforge/web/fetch` it is `promptforge/web`. This holds for every tool id.

[`ToolId`] implements [`Display`](std::fmt::Display) with its canonical `namespace/pack/name` string. It serializes through serde as that one string, for example the JSON string `"promptforge/web/fetch"`. Deserializing runs [`ToolId::parse`], so an invalid string such as `"promptforge/web_fetch"` is a deserialization error. [`ToolId`] is ordered and hashable, so it works as a map key.

````
use promptforge::capabilities::CapabilityId;
use promptforge::tools::{ToolId, ToolIdErrorKind};

let id = ToolId::parse("promptforge/web/fetch")?;
assert_eq!(id.name(), "fetch");
assert_eq!(id.capability(), CapabilityId::parse("promptforge/web")?);
assert_eq!(id.to_string(), "promptforge/web/fetch");
assert_eq!(serde_json::to_string(&id)?, "\"promptforge/web/fetch\"");
assert!(serde_json::from_str::<ToolId>("\"promptforge/web_fetch\"").is_err());

let error = ToolId::parse("promptforge/web").err().ok_or("two segments fail")?;
assert_eq!(error.kind(), ToolIdErrorKind::SegmentCount);
assert_eq!(error.field(), "id");

let error = ToolId::parse("promptforge//fetch").err().ok_or("an empty segment fails")?;
assert_eq!(error.kind(), ToolIdErrorKind::Empty);

let error = ToolId::parse("Promptforge/web/fetch").err().ok_or("uppercase fails")?;
assert_eq!(error.kind(), ToolIdErrorKind::Control);
# Ok::<(), Box<dyn std::error::Error>>(())
````

## ToolIdError

[`ToolIdError`] explains why text could not be accepted as a [`ToolId`]. [`ToolId::parse`] and [`ToolId`] deserialization return it, and hosts never build one. Each method takes `&self`, has no arguments, and cannot fail.

- [`ToolIdError::kind`] returns the stable [`ToolIdErrorKind`]. Branch on it instead of matching message text.
- [`ToolIdError::field`] returns a [`&str`](str) naming what was rejected. Every error from [`ToolId::parse`] returns `"id"`. The other possible value is `"wire name"`.

[`ToolIdError`] implements [`std::error::Error`]. Its [`Display`](std::fmt::Display) format is `invalid tool {field}: {reason}`. The reasons from [`ToolId::parse`] are `a tool id must have exactly 3 segments (namespace/pack/name)`, `segments must not be empty`, and `segments may contain only lowercase ASCII letters, digits, '-', '_', '.'`.

## ToolIdErrorKind

[`ToolIdErrorKind`] is the matchable classification of a [`ToolIdError`], returned by [`ToolIdError::kind`].

- [`ToolIdErrorKind::SegmentCount`]: the id did not have exactly three segments. [`ToolId::parse`] returns it for one, two, or four or more segments. Two segments usually means a capability id such as `promptforge/web` was passed. Supply the full three-segment tool id.
- [`ToolIdErrorKind::Empty`]: a segment was empty, as in `promptforge//fetch`. Fill in the missing segment.
- [`ToolIdErrorKind::Separator`]: a wire name contained the `/` separator. [`ToolId::parse`] never returns it, because parse splits on `/`. A wire name with a `/` reaches the host as [`ToolCatalogError::InvalidWireName`] instead, with the reason `must not contain the '/' separator`.
- [`ToolIdErrorKind::Control`]: a segment contained a character outside the allowed set. Despite the name, this covers every disallowed character: control characters, uppercase letters, `@`, non-ASCII, and anything other than lowercase ASCII letters, digits, `-`, `_`, and `.`. Rewrite the id with allowed characters.

## ToolDescriptor

[`ToolDescriptor`] is one tool as data: its stable identity, its wire name, the description shown to the model, the JSON Schema for its arguments, its output kind, and the co-activation conflicts of the capability that contributed it. It never holds an implementation. The host keeps implementations in its own table, keyed by [`ToolId`]. A host builds a descriptor with [`ToolDescriptor::new`] and the two builder methods, or deserializes one.

[`ToolDescriptor::new`] takes four arguments and cannot fail. It checks nothing.

- `id`, a [`ToolId`], is the tool's stable identity and catalog key. Build it with [`ToolId::parse`].
- `wire_name`, anything that converts [`Into`] a [`String`], is the transport name of the tool. It must be non-empty and contain no `/` and no control character, but only [`ToolCatalog::new`] checks that. The worked example uses the tool's short name, `"echo"`.
- `description`, anything that converts [`Into`] a [`String`], is the one-sentence description shown to the model, for example `"Fetch a web page over HTTP."`.
- `parameters_schema`, a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the JSON Schema `object` that the tool's arguments must match, for example `{"type": "object", "properties": {"url": {"type": "string"}}}`. Nothing in this module validates it.

It returns a descriptor with plain text output and no conflicts. Two builder methods adjust it. Each takes the descriptor by value plus one argument, returns the updated descriptor, and cannot fail.

- [`ToolDescriptor::structured`] takes a [`bool`] and sets [`ToolDescriptor::structured_output`]. `true` marks the output as one JSON value, and `false`, the default, marks it as plain text. [Trusted and untrusted output](#trusted-and-untrusted-output) explains what the marking changes and why it needs trusted output.
- [`ToolDescriptor::with_conflicts`] takes a [`Vec`] of [`CapabilityId`](crate::capabilities::CapabilityId) and sets [`ToolDescriptor::conflicts`], replacing any previous value.

All six fields are public, so a host can read them and assign them after construction.

- [`ToolDescriptor::id`], a [`ToolId`], is the stable identity and the catalog key. [`Effect::ToolCall::tool`](crate::effect::Effect#variant.ToolCall.field.tool) holds this id. It must be unique within a catalog.
- [`ToolDescriptor::wire_name`], a [`String`], is the transport wire name. It is not identity, and [`ToolCatalog::get`] never matches on it.
- [`ToolDescriptor::description`], a [`String`], is the one-sentence description shown to the model. It becomes the bound slot's description.
- [`ToolDescriptor::parameters_schema`], a [`serde_json::Value`](https://docs.rs/serde_json/latest/serde_json/enum.Value.html), is the JSON Schema `object` for the arguments. The run advertises it under the prompt-local alias.
- [`ToolDescriptor::structured_output`], a [`bool`], says whether the output text is one JSON value. The default is `false`.
- [`ToolDescriptor::conflicts`], a [`Vec`] of [`CapabilityId`](crate::capabilities::CapabilityId), lists capabilities that cannot be activated together with the capability that contributed the tool. The default is empty. It is stored for the record. The host checks it before activation, and the catalog does not check it.

[`ToolDescriptor`] supports serde serialization and deserialization with no renamed fields. It is a JSON object with the keys `id`, `wire_name`, `description`, `parameters_schema`, `structured_output`, and `conflicts`, where `id` is the `namespace/pack/name` string. A whole descriptor round-trips, so a host can ship a catalog's descriptors between processes or persist them.

````
use promptforge::capabilities::CapabilityId;
use promptforge::tools::{ToolDescriptor, ToolId};

let lookup = ToolDescriptor::new(
    ToolId::parse("example/data/lookup")?,
    "lookup",
    "Look up a record by key.",
    serde_json::json!({"type": "object", "properties": {"key": {"type": "string"}}}),
)
.structured(true)
.with_conflicts(vec![CapabilityId::parse("example/legacy")?]);
assert_eq!(lookup.wire_name, "lookup");
assert!(lookup.structured_output);
assert_eq!(lookup.conflicts.len(), 1);

let json = serde_json::to_string(&lookup)?;
let back: ToolDescriptor = serde_json::from_str(&json)?;
assert_eq!(back, lookup);
# Ok::<(), Box<dyn std::error::Error>>(())
````

## ToolCatalog

[`ToolCatalog`] is a validated set of tool descriptors. Runs bind their tool slots against it. The host builds it from the tools of its activated capabilities and keeps the implementations in its own table. Build one with [`ToolCatalog::new`], or use [`ToolCatalog::default`] for an empty one. Install it with [`Environment::tools`](crate::Environment::tools), and read it back from a prepared context with [`RunContext::tools`](crate::RunContext::tools).

[`ToolCatalog::new`] takes one argument.

- `tools`, a slice of [`ToolDescriptor`], holds the descriptors to validate, usually the tools contributed by the capabilities the host activated. Each [`ToolDescriptor::id`] must be unique. Each [`ToolDescriptor::wire_name`] must be non-empty and contain no `/` and no control character, which is a byte below 0x20 or the byte 0x7f. Uppercase and other printable characters are accepted in a wire name. An empty slice is valid and gives an empty catalog.

It returns the catalog, with each descriptor cloned in the order supplied. It fails with [`ToolCatalogError::InvalidWireName`] for a bad wire name, or [`ToolCatalogError::DuplicateId`] when two descriptors share a [`ToolId`]. It checks the descriptors in the order supplied, the wire name before the id for each one, and returns the first failure. It does not check that each tool belongs to an activated capability, and it does not check [`ToolDescriptor::conflicts`]. Containment and co-activation conflicts are the host's job before it builds the catalog.

The other methods take `&self` and cannot fail.

- [`ToolCatalog::get`] takes a [`&ToolId`](ToolId) and returns the descriptor with that id as an [`Option`] of a reference, or [`None`]. The lookup is by identity only, and a wire name never matches. It is a linear scan, meant for the cold bind-time path that runs once per declared slot.
- [`ToolCatalog::tools`] returns every descriptor as a slice, in the order supplied to [`ToolCatalog::new`].

A catalog is cheap to share across tasks and threads. Cloning it copies one reference-counted slice of descriptors, and it is [`Send`] and [`Sync`]. It has no serde support, so ship the descriptors instead and rebuild the catalog. Its [`Debug`](std::fmt::Debug) output prints only the list of ids.

````
use promptforge::tools::{ToolCatalog, ToolCatalogError, ToolCatalogErrorKind, ToolDescriptor, ToolId};

let schema = serde_json::json!({"type": "object", "properties": {}});
let id = ToolId::parse("example/web/fetch")?;
let fetch = ToolDescriptor::new(id.clone(), "fetch", "Fetch a web page.", schema.clone());

let catalog = ToolCatalog::new(&[fetch.clone()])?;
assert_eq!(catalog.get(&id), Some(&fetch));
assert!(catalog.get(&ToolId::parse("example/web/search")?).is_none());
assert!(ToolCatalog::new(&[])?.tools().is_empty());

let error = ToolCatalog::new(&[fetch.clone(), fetch]).err().ok_or("a repeated id fails")?;
assert_eq!(error.kind(), ToolCatalogErrorKind::DuplicateId);
assert_eq!(error.duplicate_id(), Some(&id));

let slashed = ToolDescriptor::new(ToolId::parse("example/web/get")?, "web/get", "Get a page.", schema);
let error = ToolCatalog::new(&[slashed]).err().ok_or("a slash in a wire name fails")?;
assert_eq!(error.kind(), ToolCatalogErrorKind::InvalidWireName);
assert_eq!(error.duplicate_id(), None);
let ToolCatalogError::InvalidWireName { wire_name, reason, .. } = error else {
    panic!("the wire name is rejected");
};
assert_eq!(wire_name, "web/get");
assert_eq!(reason, "must not contain the '/' separator");
# Ok::<(), Box<dyn std::error::Error>>(())
````

## ToolCatalogError

[`ToolCatalogError`] explains why [`ToolCatalog::new`] could not build a catalog: a repeated identity, or a wire name that a transport would reject. [`ToolCatalog::new`] returns it, and hosts never build one. Both variants are `#[non_exhaustive]`, so their patterns need `..`.

- [`ToolCatalogError::DuplicateId`]: more than one descriptor has the same [`ToolId`]. The host sees it when two activated capabilities, or one capability twice, contribute the same id. Remove or rename the duplicate.
  - [`ToolCatalogError::DuplicateId::id`](ToolCatalogError#variant.DuplicateId.field.id), a [`ToolId`], is the identity supplied more than once.
- [`ToolCatalogError::InvalidWireName`]: a descriptor's wire name is empty, contains `/`, or contains a control character. Fix that descriptor's wire name.
  - [`ToolCatalogError::InvalidWireName::wire_name`](ToolCatalogError#variant.InvalidWireName.field.wire_name), a [`String`], is the rejected wire name as supplied.
  - [`ToolCatalogError::InvalidWireName::reason`](ToolCatalogError#variant.InvalidWireName.field.reason), a [`&'static str`](str), says why it was rejected. It is one of `must not be empty`, `must not contain the '/' separator`, and `must not contain a control character`.

Two methods read the error. Each takes `&self`, has no arguments, and cannot fail.

- [`ToolCatalogError::kind`] returns the stable [`ToolCatalogErrorKind`] that matches the variant. Branch on it.
- [`ToolCatalogError::duplicate_id`] returns the duplicated id as [`Some`] for [`ToolCatalogError::DuplicateId`], and [`None`] for [`ToolCatalogError::InvalidWireName`].

[`ToolCatalogError`] implements [`std::error::Error`]. Its [`Display`](std::fmt::Display) text is `duplicate tool identity {id:?} in the tool catalog` or `invalid tool wire name {wire_name:?}: {reason}`. The duplicate message formats the id with [`Debug`](std::fmt::Debug), not in its `namespace/pack/name` form.

## ToolCatalogErrorKind

[`ToolCatalogErrorKind`] is the matchable classification of a [`ToolCatalogError`], returned by [`ToolCatalogError::kind`].

- [`ToolCatalogErrorKind::DuplicateId`]: two supplied tools shared a [`ToolId`]. Call [`ToolCatalogError::duplicate_id`] on the error to get it.
- [`ToolCatalogErrorKind::InvalidWireName`]: a supplied descriptor's wire name was not legal for a transport. Fix that descriptor.

## ToolBindings

[`ToolBindings`] records which tool each alias in the prompt's `tools:` frontmatter was bound to, plus the descriptor of each tool available to the run. It holds descriptors only, never implementations, and resolves an alias to an id to a descriptor. The host receives it from [`RunContext::tool_bindings`](crate::RunContext::tool_bindings) on the context returned by [`Environment::prepare`](crate::Environment::prepare). [`ToolBindings::default`] gives an empty set, which is also what an unprepared context holds. [How prepare binds tool slots](#how-prepare-binds-tool-slots) describes how the bindings are filled.

Each method takes `&self` and cannot fail.

- [`ToolBindings::alias_id`] takes `alias`, a [`&str`](str) holding the prompt-local alias as written in the prompt's `tools:` frontmatter, for example `"fetch"`. The match is exact and case-sensitive. It returns the bound [`ToolId`] as an [`Option`] of a reference, or [`None`] when the slot was not filled or the alias was never declared.
- [`ToolBindings::resolve`] takes the same `alias` argument and returns the bound tool's [`ToolDescriptor`], exactly as the catalog holds it, or [`None`] when the alias is unbound.
- [`ToolBindings::tool`] takes `id`, a [`&ToolId`](ToolId), such as the id named by an [`Effect::ToolCall`](crate::effect::Effect::ToolCall). It returns the descriptor bound under that id when this run may call the tool, or [`None`].
- [`ToolBindings::len`] returns the number of bound aliases as a [`usize`]. It counts aliases, not distinct tools, so two aliases bound to one tool count as 2.
- [`ToolBindings::is_empty`] returns `true` when no aliases are bound.

## ToolOutput

[`ToolOutput`] is the result of a successful tool call. The host returns it as the success value inside an [`EffectAnswer::ToolCall`](crate::effect::EffectAnswer::ToolCall). It holds the output text and its required trust marking. [Trusted and untrusted output](#trusted-and-untrusted-output) explains what the engine does with each marking.

A host builds one with either of two constructors, and there is no other way to get one. Each takes `text`, anything that converts [`Into`] a [`String`], and cannot fail.

- [`ToolOutput::trusted`] marks the text as produced by the host's own first-party code. Choose it only for text the host vouches for. When the descriptor has [`ToolDescriptor::structured_output`] set, the text should be one JSON value.
- [`ToolOutput::untrusted`] marks the text as external data that an attacker can influence, such as web pages and third-party API responses.

Two methods read it back. Each takes `&self` and cannot fail.

- [`ToolOutput::text`] returns the output text as supplied, without any wrapping, as a [`&str`](str).
- [`ToolOutput::trust`] returns the [`OutputTrust`] marking.

[`ToolOutput`] has no serde support. A run log records it through [`EffectAnswer::record`](crate::effect::EffectAnswer::record) instead, which keeps the text and whether it was trusted.

## OutputTrust

[`OutputTrust`] says whether a tool's output is trusted or must be treated as untrusted data. It is stored inside every [`ToolOutput`]. The host sets it by choosing [`ToolOutput::trusted`] or [`ToolOutput::untrusted`], reads it with [`ToolOutput::trust`], and never passes it anywhere directly.

- [`OutputTrust::Trusted`]: the output was produced by trusted first-party code. The engine appends the text to model input verbatim.
- [`OutputTrust::Untrusted`]: the output contains external data that an attacker can influence. The engine wraps the text in a nonce-guarded envelope before it can reach the next model turn or the calling script.

The engine treats any future variant as untrusted and wraps it. [`OutputTrust`] has no serde support.

## ToolError

[`ToolError`] is a failure of one tool call, with a message that is safe to show a model. The host returns it as the error value inside an [`EffectAnswer::ToolCall`](crate::effect::EffectAnswer::ToolCall). [Reporting a failure](#reporting-a-failure) explains what the engine does with it.

A host builds one with either of two constructors. Neither can fail.

- [`ToolError::message`] takes `text`, anything that converts [`Into`] a [`String`]. The text is the error's whole [`Display`](std::fmt::Display) output and is shown to the model, so write it for the model, with no secrets or internal detail. The error has kind [`ToolErrorKind::Other`] and no source. A host that cannot resolve a call's [`ToolId`] refuses the call this way, for example with `"the tool the call names has no implementation in the host's table"`.
- [`ToolError::with_source`] takes `text`, the same model-safe message, and `src`, the underlying cause. The cause is any [`std::error::Error`] that is also [`Send`], [`Sync`], and `'static`, such as a [`std::io::Error`]. It is boxed. The error has kind [`ToolErrorKind::Backend`], and its [`source`](std::error::Error::source) returns the cause.

[`ToolError::with_kind`] takes the error by value and a [`ToolErrorKind`], and returns the error with that kind in place of the current one. Use it when the failure belongs to a class other than the constructor's default. It cannot fail.

The remaining methods take `&self`, have no arguments, and cannot fail.

- [`ToolError::kind`] returns the [`ToolErrorKind`]. Match on it instead of on the message.
- [`ToolError::is_cancelled`] returns `true` only when the kind is [`ToolErrorKind::Cancelled`].
- [`ToolError::is_retryable`] returns `true` only when the kind is [`ToolErrorKind::Transport`], the one kind where retrying the same call could plausibly succeed.

[`ToolError`] implements [`Display`](std::fmt::Display) with the message alone, never the cause, and it implements [`std::error::Error`]. It is [`Send`], [`Sync`], and `'static`. It is not [`Clone`], and it is neither [`UnwindSafe`](std::panic::UnwindSafe) nor [`RefUnwindSafe`](std::panic::RefUnwindSafe).

````
use promptforge::tools::{ToolError, ToolErrorKind};

let refused = ToolError::message("the tool the call names has no implementation in the host's table");
assert_eq!(refused.kind(), ToolErrorKind::Other);
assert!(std::error::Error::source(&refused).is_none());

let bad_args = ToolError::message("`url` must be a string").with_kind(ToolErrorKind::InvalidArguments);
assert_eq!(bad_args.kind(), ToolErrorKind::InvalidArguments);
assert!(!bad_args.is_retryable());

let backend = ToolError::with_source("backend failed", std::io::Error::other("boom"));
assert_eq!(backend.kind(), ToolErrorKind::Backend);
assert_eq!(backend.to_string(), "backend failed");
````

## ToolErrorKind

[`ToolErrorKind`] is the matchable classification of a [`ToolError`]. The host sets it when it builds the error, by naming a variant and passing it to [`ToolError::with_kind`], and reads it back with [`ToolError::kind`]. Choose the variant that helps your own code branch.

- [`ToolErrorKind::InvalidArguments`]: the model supplied arguments that the tool could not accept. Set it when argument validation fails.
- [`ToolErrorKind::Backend`]: the tool's backend refused or failed the request. It is the default kind from [`ToolError::with_source`].
- [`ToolErrorKind::Transport`]: the request failed at the transport layer, such as a network failure or a timeout. It is the only kind for which [`ToolError::is_retryable`] returns `true`.
- [`ToolErrorKind::Cancelled`]: the run was cancelled before or during the call. It is the only kind for which [`ToolError::is_cancelled`] returns `true`.
- [`ToolErrorKind::Other`]: any other tool failure. It is the default kind from [`ToolError::message`].
