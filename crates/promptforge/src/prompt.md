The read-only view of a prompt's frontmatter: its identity, version, store files, capabilities, tool slots, args, and model roles.

A prompt's frontmatter is its contract with the host. This module lets your program read that contract from a parsed [`Prompt`](crate::Prompt) before any run exists. From it you learn what to call the prompt, which store file to stage and which to collect, which capabilities to activate, which tools and model roles to supply, and what kind of argument string to pass. Every type here is reached from one [`Frontmatter`], which [`Prompt::frontmatter`](crate::Prompt::frontmatter) returns. The parser validates every key strictly, so each declaration you read is already well-formed.

# Where this fits

A host reads the frontmatter between parsing and preparing. [`Prompt::parse`](crate::Prompt::parse) returns the parse outcome beside a [`Vec`] of [`Event`](crate::event::Event) values, and [`Prompt::frontmatter`](crate::Prompt::frontmatter) works on the parsed [`Prompt`](crate::Prompt) right away. Nothing has to run first. The host then uses the declarations in this order.

1. Activate the capabilities listed by [`Frontmatter::capabilities`], and stage the file named by [`Frontmatter::input`] in the store.
2. Call [`Environment::prepare`](crate::Environment::prepare). It fills each tool slot from [`Frontmatter::tools`] against the host's catalog and binds each role from [`Frontmatter::models`] to the run's current model. It returns the prepared [`RunContext`](crate::RunContext) beside a [`Requirements`](crate::Requirements) report.
3. Build the run with [`Run::new`](crate::Run::new). Its argument string is either plain prose or JSON, and [`ArgsDecl::is_default`] on [`Frontmatter::args`] says which.
4. During the run, [`Frontmatter::max_tool_iterations`] bounds each section's tool loop.
5. After [`Step::Done`](crate::Step::Done), collect the file named by [`Frontmatter::output`] from the store.

The version from [`Frontmatter::promptforge`] does not stop [`Run::new`](crate::Run::new). An unsupported or missing version arrives as the first [`Step::Done`](crate::Step::Done) from [`Run::step`](crate::Run::step). A host that wants to refuse such a prompt earlier checks the version here.

# Reading a prompt's contract

This program parses a prompt that uses every frontmatter key, then reads each declaration back.

````
use std::num::NonZeroU32;

use promptforge::Prompt;
use promptforge::prompt::{ArgType, ModelKeyword, ToolSlot};

let source = concat!(
    "---\n",
    "name: researcher\n",
    "description: researches a topic\n",
    "promptforge: 0\n",
    "max_tool_iterations: 20\n",
    "input:\n",
    "  path: paper.md\n",
    "  description: The input paper\n",
    "output:\n",
    "  path: report.md\n",
    "  description: The output report\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "  - ref: io.github.corp/mcp\n",
    "    optional: true\n",
    "    config:\n",
    "      servers: [alpha]\n",
    "tools:\n",
    "  search: promptforge/web/search\n",
    "  fetch: promptforge/web/fetch\n",
    "args:\n",
    "  query:\n",
    "    type: string\n",
    "  limit:\n",
    "    type: integer\n",
    "    optional: true\n",
    "models:\n",
    "  analyst:\n",
    "    keywords: [frontier, thinking]\n",
    "    min_context: 200000\n",
    "    description: deep reasoning\n",
    "---\n",
    "\n",
    "# Researcher\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "researcher");
let prompt = parsed?;
let frontmatter = prompt.frontmatter();

assert_eq!(frontmatter.name(), "researcher");
assert_eq!(frontmatter.description(), "researches a topic");
assert_eq!(frontmatter.promptforge(), Some(0));
assert_eq!(frontmatter.max_tool_iterations(), NonZeroU32::new(20));

let input = frontmatter.input().ok_or("input is declared")?;
assert_eq!(input.path(), "paper.md");
assert_eq!(input.description(), "The input paper");
let output = frontmatter.output().ok_or("output is declared")?;
assert_eq!(output.path(), "report.md");

let capabilities = frontmatter.capabilities();
assert_eq!(capabilities.len(), 2);
assert_eq!(capabilities[0].id().to_string(), "promptforge/web");
assert!(!capabilities[0].is_optional());
assert!(capabilities[0].config().is_none());
assert_eq!(capabilities[1].id().to_string(), "io.github.corp/mcp");
assert!(capabilities[1].is_optional());
assert!(capabilities[1].config().is_some());

match frontmatter.tools().get("search") {
    Some(ToolSlot::Exact(id)) => assert_eq!(id.to_string(), "promptforge/web/search"),
    other => panic!("expected an exact slot, got {other:?}"),
}

let args = frontmatter.args();
assert!(!args.is_default());
let limit = args.get("limit").ok_or("limit is declared")?;
assert_eq!(limit.kind(), ArgType::Integer);
assert!(limit.is_optional());

let analyst = frontmatter.models().get("analyst").ok_or("analyst is declared")?;
assert_eq!(analyst.keywords(), &[ModelKeyword::Frontier, ModelKeyword::Thinking]);
assert_eq!(analyst.min_context(), NonZeroU32::new(200_000));
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Identity and version.** [`Frontmatter::name`] and [`Frontmatter::description`] identify the prompt, so a host can list prompts and label runs. [`Frontmatter::promptforge`] is the targeted engine version, and [`Frontmatter::max_tool_iterations`] is the prompt's own cap on each section's tool loop.
2. **Store files.** [`Frontmatter::input`] and [`Frontmatter::output`] each return an [`Option`] of a [`FileDecl`]. Its [`FileDecl::path`] is where the host stages the input before the run and reads the output after it.
3. **Capabilities.** [`Frontmatter::capabilities`] keeps declaration order. The first entry is a bare id string, which is always required. The second is a map that marks the capability optional and carries configuration for the host to hand to it at activation.
4. **Tool slots.** [`ToolSlots::get`] looks a slot up by its alias. Every slot today is a [`ToolSlot::Exact`] that holds a [`ToolId`](crate::tools::ToolId).
5. **Args.** An explicit `args:` key makes a structured declaration, so [`ArgsDecl::is_default`] is `false` and the host passes a JSON argument string. [Passing the argument string](#passing-the-argument-string) explains the choice.
6. **Model roles.** [`ModelRoles::get`] looks a role up by its label. The role lists its keywords in the order written, and a minimum context window in tokens.

The [Reference](#reference) gives the exact YAML form of every key.

# Strict validation

The frontmatter parser rejects anything it does not recognize. An unknown or misspelled top-level key fails the parse, so a typo such as `desciption:` fails loudly instead of being skipped. The same rule holds inside every entry of `capabilities:`, `tools:`, `args:`, and `models:`, where misspelled sub-keys such as `optionl`, `wants`, `tipe`, and `keyword` fail too. Malformed YAML fails the parse as well. A leading UTF-8 byte order mark is dropped before parsing.

Every frontmatter failure has the kind [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter). A bad value is reported at its exact line and column. [`ParseError::line`](crate::ParseError::line) and [`ParseError::column`](crate::ParseError::column) count from the top of the file, so the opening `---` line is line 1. [`ParseError::name`](crate::ParseError::name) returns [`None`], because a frontmatter failure happens before the prompt's name is known. Report the position to the prompt author under your own label for the source.

````
use promptforge::{ParseErrorKind, Prompt};

let typo = concat!(
    "---\n",
    "name: notes\n",
    "desciption: keeps a note\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Notes\n",
);
let (failed, _parse_events) = Prompt::parse(typo, "notes");
let error = failed.err().ok_or("the misspelled key fails the parse")?;
assert_eq!(error.kind(), ParseErrorKind::Frontmatter);

let bad_id = concat!(
    "---\n",
    "name: fetcher\n",
    "description: fetches a page\n",
    "capabilities:\n",
    "  - web\n",
    "---\n",
    "\n",
    "# Fetcher\n",
);
let (failed, _parse_events) = Prompt::parse(bad_id, "fetcher");
let error = failed.err().ok_or("the one-segment capability id fails the parse")?;
assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
assert_eq!(error.line(), Some(5));
assert_eq!(error.column(), Some(5));
assert_eq!(error.name(), None);
# Ok::<(), Box<dyn std::error::Error>>(())
````

The bad id `web` sits on file line 5, and the value starts in column 5, after the `  - ` list marker.

# Prompt-local names

Three kinds of name are local to a prompt: tool aliases under `tools:`, model role labels under `models:`, and arg names under `args:`. The model only ever sees these local names, never a global path. All three share one grammar, `[A-Za-z][A-Za-z0-9_-]{0,63}`, which is a letter followed by up to 63 letters, digits, underscores, or hyphens. So `1search`, `has space`, `has/slash`, and `has.dot` are rejected. A 64-character name passes, and a 65-character name fails. A name that appears twice in one map fails with "duplicate {what} `{key}`: contract map keys must be unique", where `{what}` is `tool alias`, `model role label`, or `arg name`.

The lookup methods [`ToolSlots::get`], [`ArgsDecl::get`], and [`ModelRoles::get`] take the name exactly as written, and the match is case-sensitive. A string outside the grammar can never be present, so looking one up returns [`None`].

# Passing the argument string

[`Frontmatter::args`] tells the host how to fill the argument string for [`Run::new`](crate::Run::new). There are no freeform prompts: every prompt has an args declaration. A prompt with no `args:` key gets an implicit one, a single optional string arg named `prose` with the description "Freeform input for this prompt". [`ArgsDecl::is_default`] returns `true` only for that implicit declaration.

- For a default declaration, pass plain prose. The run wraps the string as `{"prose": args}`. An empty string counts as present, not absent.
- For a structured declaration, pass a JSON object string such as `{"query": "papers", "limit": 5}`. The run parses it as JSON. A parse failure or a JSON `null` gives the prompt a nil `argv`.

An explicit `args:` block is always structured, even one with exactly the same shape as the implicit declaration. The declaration advertises the prompt's arguments, documents them, and is the source for a tool or MCP input schema for calling the prompt. It enforces nothing. Enforcement belongs to the prompt's H1.

````
use promptforge::Prompt;
use promptforge::prompt::{ArgType, ArgsDecl};

let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: says hi\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Greeter\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greeter");
let prompt = parsed?;
let args = prompt.frontmatter().args();
assert!(args.is_default());
assert_eq!(args.len(), 1);

let prose = args.get("prose").ok_or("the implicit arg is declared")?;
assert_eq!(prose.kind(), ArgType::String);
assert!(prose.is_optional());
assert!(prose.default().is_none());
assert_eq!(prose.description(), Some("Freeform input for this prompt"));

assert!(ArgsDecl::default().is_default());
assert_eq!(ArgType::String.to_string(), "string");
# Ok::<(), Box<dyn std::error::Error>>(())
````

# What prepare checks

Parsing checks only the shape of the `tools:` and `models:` declarations. [`Environment::prepare`](crate::Environment::prepare) checks them against the deployment.

**Tool slots** are filled by identity against the host's tool catalog. The first two segments of a slot's tool id name the capability that must contribute the tool. A slot whose tool is in the catalog is bound. When a slot's capability contributed nothing, that capability lands in [`Requirements::missing_required`](crate::Requirements::missing_required). The report is then unsatisfied, so [`Requirements::refusal`](crate::Requirements::refusal) returns the error the host fails the run with before building it. When the capability contributed other tools but not the named one, the slot is not reported and stays unbound, and advertising that alias fails at run time.

**Model roles** are bound to the run's current model, set with [`RunContext::model`](crate::RunContext::model). With no current model, roles stay unbound, and selecting one at run time fails. With a current model, prepare checks two things for each role and reports each failure as an [`UnmetRequirement`](crate::UnmetRequirement) in [`Requirements::unmet_requirements`](crate::Requirements::unmet_requirements).

- The hard keywords. `thinking` fails when the model's [`ThinkingMode`](crate::model::ThinkingMode) is [`ThinkingMode::Never`](crate::model::ThinkingMode::Never), and `no-thinking` fails when it is [`ThinkingMode::Always`](crate::model::ThinkingMode::Always). The failure's [`UnmetRequirement::check`](crate::UnmetRequirement::check) is [`RequirementCheck::HardKeyword`](crate::RequirementCheck::HardKeyword).
- The context minimum. A model whose context window is below the role's `min_context:` fails, with [`RequirementCheck::ContextMinimum`](crate::RequirementCheck::ContextMinimum).

A hard keyword also freezes the bound invocation's thinking switch, on for `thinking` and off for `no-thinking`. The soft keywords `frontier`, `fast`, `small`, `creative`, and `chat` record the author's intent and are never checked. The [`model`](crate::model) module page covers binding, and the [`tools`](crate::tools) module page covers the catalog.

# Reference

This part covers every item in the module, starting from [`Frontmatter`] and following its accessors. Four conventions hold across the module.

- Every struct is `#[non_exhaustive]` with private fields, so a host cannot write a struct literal. It receives each value from [`Prompt::frontmatter`](crate::Prompt::frontmatter) and the accessors below.
- Every accessor takes `&self` and cannot fail. The lookup methods take one extra argument, described with each.
- The enums [`ToolSlot`], [`ArgType`], and [`ModelKeyword`] are `#[non_exhaustive]`, so every `match` on them needs a wildcard arm.
- [`ArgDecl::default`] and [`CapabilityDecl::config`] return a [`serde_yaml_ng::Value`](https://docs.rs/serde_yaml_ng/latest/serde_yaml_ng/enum.Value.html). A host that names that type needs the `serde_yaml_ng` crate as a dependency.

Besides the accessors, there are two ways to get a value. All eleven types implement serde [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html), so a host can decode a [`Frontmatter`] or any declaration type directly from YAML with a serde YAML deserializer and get the same validation that [`Prompt::parse`](crate::Prompt::parse) applies. [`Prompt::parse`](crate::Prompt::parse) decodes the frontmatter this way itself. [`ArgsDecl`], [`ModelRoles`], and [`ToolSlots`] also implement [`Default`], for building declarations in tests.

## Frontmatter

[`Frontmatter`] is the parsed YAML frontmatter of a prompt file. The host gets a reference to it from [`Prompt::frontmatter`](crate::Prompt::frontmatter). It accepts exactly the ten top-level keys below, and any other key fails the parse with [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter). Only `name:` and `description:` are required.

````yaml
name: researcher
description: researches a topic
promptforge: 0
max_tool_iterations: 20
input:
  path: paper.md
  description: The input paper
output:
  path: report.md
  description: The output report
capabilities:
  - promptforge/web
tools:
  fetch: promptforge/web/fetch
args:
  query:
    type: string
models:
  analyst:
    keywords: [thinking]
````

- [`Frontmatter::name`] returns the prompt's identifier as a [`&str`](str), from the required key `name:`, for example `name: greeter`. Parse errors that happen after the frontmatter decodes are stamped with it. Use it to list prompts or label runs.
- [`Frontmatter::description`] returns the one-line description as a [`&str`](str), from the required key `description:`, for example `description: says hi`. Prompt listings and name retrieval show it. Neither key has a default, so a frontmatter without `name:` or `description:` fails the parse with [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter).
- [`Frontmatter::promptforge`] returns the engine major version as an [`Option`] of [`u32`], from the key `promptforge:`, written `promptforge: 0`. It returns [`None`] when the key is absent. The key's presence marks the file as a PromptForge prompt. Parsing accepts a missing key, but a run does not. The run accepts only version `0`. Any other version fails the run with [`RunErrorKind::Version`](crate::RunErrorKind::Version) and the message "unsupported promptforge version: {0} (this build supports major 0)". A missing version fails the run with [`RunErrorKind::Parse`](crate::RunErrorKind::Parse) and the message "not a promptforge prompt: no promptforge version".
- [`Frontmatter::max_tool_iterations`] returns an [`Option`] of a [`NonZeroU32`](std::num::NonZeroU32), from the key `max_tool_iterations:`, for example `max_tool_iterations: 20`. It caps the model round trips of each section's `models.loop`, and reaching the cap fails the section with a tool-loop-exhausted error. When present, it overrides [`RunLimits::max_tool_iterations`](crate::RunLimits::max_tool_iterations) for this prompt. When absent, it returns [`None`], and the run uses [`RunLimits::tool_iterations`](crate::RunLimits::tool_iterations), which defaults to 24. Valid values are 1 through 1000, checked at parse. Values such as `0`, `-1`, `1001`, and `100000000000` fail with "max_tool_iterations must be a positive integer (>= 1), got {raw}" or "max_tool_iterations must be <= 1000, got {raw}".
- [`Frontmatter::input`] returns an [`Option`] of a reference to a [`FileDecl`], from the key `input:`. The prompt expects this file in the store when it starts, so stage it before the run. It returns [`None`] when the key is absent.
- [`Frontmatter::output`] returns an [`Option`] of a reference to a [`FileDecl`], from the key `output:`. The prompt leaves this file in the store when it finishes, so collect it after [`Step::Done`](crate::Step::Done). It returns [`None`] when the key is absent.
- [`Frontmatter::capabilities`] returns a slice of [`CapabilityDecl`] in declaration order, from the sequence key `capabilities:`. It is empty when the key is absent. Activate these capabilities before [`Environment::prepare`](crate::Environment::prepare).
- [`Frontmatter::tools`] returns a reference to the [`ToolSlots`], from the map key `tools:`. It is empty when the key is absent. Read it to know which tools your catalog must contain.
- [`Frontmatter::args`] returns a reference to the [`ArgsDecl`], from the map key `args:`. When the key is absent, it is the implicit declaration described in [Passing the argument string](#passing-the-argument-string).
- [`Frontmatter::models`] returns a reference to the [`ModelRoles`], from the map key `models:`. It is empty when the key is absent.

## FileDecl

[`FileDecl`] is one declared store file, the value of the `input:` or `output:` key. The host gets it from [`Frontmatter::input`] or [`Frontmatter::output`]. Each key is a map with exactly two required sub-keys, and any other sub-key fails the parse.

````yaml
input:
  path: paper.md
  description: The input paper
output:
  path: report.md
  description: The output report
````

- [`FileDecl::path`] returns the store-internal path as a [`&str`](str), from `path:`, for example `"paper.md"`. Stage an input file at this path before the run, or read an output file from it after the run.
- [`FileDecl::description`] returns the file's purpose as a [`&str`](str), from `description:`. It documents the file and also feeds MCP schema generation for the prompt.

## CapabilityDecl

[`CapabilityDecl`] is one entry of the `capabilities:` sequence, naming a capability for the host to activate. The host gets a slice of them from [`Frontmatter::capabilities`] and activates them before [`Environment::prepare`](crate::Environment::prepare). An entry takes one of two forms.

````yaml
capabilities:
  - promptforge/web
  - ref: io.github.corp/mcp
    optional: true
    config:
      servers: [alpha]
````

A bare id string declares a required capability with no configuration. A map declares the id under `ref:`, which is required, plus `optional:`, a boolean that defaults to `false`, and `config:`, any YAML value, which may be omitted. A map without `ref:` fails with a missing-field error, and a repeated key fails with a duplicate-field error. Any other key, such as `optionl`, fails with an unknown-field error that lists the three valid keys. Any other shape, such as `- 42`, fails with an error that asks for a capability id string or a map. Every one of these failures has the kind [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter).

- [`CapabilityDecl::id`] returns a reference to the capability's [`GlobalName`](crate::capabilities::GlobalName), which has exactly two segments, `namespace/pack`, such as `promptforge/web` or `io.github.corp/mcp`. Its [`Display`](std::fmt::Display) form is the id text. Use it to find and activate the capability. Each segment is non-empty lowercase ASCII letters and digits plus `-`, `_`, and `.`, and ids carry no version. The parse rejects `web`, `promptforge/web/fetch`, `promptforge//web`, `promptforge/web@1`, and `Promptforge/web`. A wrong segment count fails with "invalid capability id `{text}`: a capability id has exactly 2 segments (namespace/pack)", and any other grammar failure with "invalid capability id `{text}`: {error}".
- [`CapabilityDecl::is_optional`] returns a [`bool`]. When it is `true` and the capability is absent, skip it with a log line instead of failing preparation. A bare string entry is always `false`. A map entry reads `optional:`.
- [`CapabilityDecl::config`] returns an [`Option`] of a reference to a [`serde_yaml_ng::Value`](https://docs.rs/serde_yaml_ng/latest/serde_yaml_ng/enum.Value.html), from the `config:` key of a map entry, in any YAML shape. It is [`None`] when the key is absent, and always for a bare string entry. Hand it to the capability at activation. It carries prompt-side data only. User-specific configuration, such as credentials and server lists, comes from the host through the run services and is never named in the prompt.

## ToolSlots

[`ToolSlots`] maps each prompt-local tool alias to a [`ToolSlot`], from the `tools:` map. The host gets it from [`Frontmatter::tools`]. [`ToolSlots::default`] returns an empty map. Each value is an exact tool path string.

````yaml
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
````

Each alias follows the grammar in [Prompt-local names](#prompt-local-names). A bad alias fails with "invalid tool alias `{key}`: expected \[A-Za-z\]\[A-Za-z0-9_-\]{0,63}". The alias `open` is reserved, and a prompt that uses it fails the parse with "the `open` key is reserved for the deferred open toolset posture; it is not a usable tool alias".

- [`ToolSlots::get`] takes `alias`, a [`&str`](str), the alias exactly as written under `tools:`. It returns an [`Option`] of a reference to the [`ToolSlot`], or [`None`] when no slot has that alias.
- [`ToolSlots::iter`] returns an iterator of `(alias, slot)` pairs, a [`&str`](str) and a reference to a [`ToolSlot`], over every declared slot. Use it to check that your catalog covers each named tool. The pairs come in ascending alias order, not declaration order, because the slots are kept in a sorted map.
- [`ToolSlots::len`] returns the number of declared slots as a [`usize`].
- [`ToolSlots::is_empty`] returns `true` when no slots are declared, including when the `tools:` key is absent.

This prompt declares `search` before `fetch`, and iteration yields them sorted:

````
use promptforge::prompt::ToolSlots;
use promptforge::{ParseErrorKind, Prompt};

let source = concat!(
    "---\n",
    "name: fetcher\n",
    "description: fetches a page\n",
    "promptforge: 0\n",
    "tools:\n",
    "  search: promptforge/web/search\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n",
    "\n",
    "# Fetcher\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "fetcher");
let prompt = parsed?;
let tools = prompt.frontmatter().tools();
let aliases: Vec<&str> = tools.iter().map(|(alias, _slot)| alias).collect();
assert_eq!(aliases, ["fetch", "search"]);
assert_eq!(tools.len(), 2);
assert!(ToolSlots::default().is_empty());

let reserved = concat!(
    "---\n",
    "name: fetcher\n",
    "description: fetches a page\n",
    "tools:\n",
    "  open: promptforge/web/fetch\n",
    "---\n",
    "\n",
    "# Fetcher\n",
);
let (failed, _parse_events) = Prompt::parse(reserved, "fetcher");
let error = failed.err().ok_or("the reserved alias fails the parse")?;
assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
# Ok::<(), Box<dyn std::error::Error>>(())
````

## ToolSlot

[`ToolSlot`] says how one tool slot is filled. The host gets it from [`ToolSlots::get`] or [`ToolSlots::iter`]. It decodes only from a bare YAML string, for example `search: promptforge/web/search`.

- [`ToolSlot::Exact`] holds a [`ToolId`](crate::tools::ToolId), an exact global tool path with three segments, `namespace/pack/name`. The first two segments name the capability that must contribute the tool, so `promptforge/web/fetch` belongs to `promptforge/web`. Every slot declared today uses this variant. Match it to get the [`ToolId`](crate::tools::ToolId), and make sure that capability is activated and offers the tool. A malformed path, such as `promptforge/web`, `web`, `promptforge/Web/fetch`, or `promptforge/web/`, fails the parse with "invalid exact tool path `{text}`: {error}". A value that is not a string, such as a map or a number, fails with "an exact tool path string".

[`ToolSlot`] is `#[non_exhaustive]` because an open, host-offered posture is deferred and will join it later. That is why the alias `open` is reserved. Include a wildcard arm in every `match`, and treat an unknown posture as unbound. [`Environment::prepare`](crate::Environment::prepare) skips such slots too.

## ArgsDecl

[`ArgsDecl`] is the prompt's typed args declaration, mapping each arg name to an [`ArgDecl`]. The host gets it from [`Frontmatter::args`]. [`ArgsDecl::default`] returns the implicit declaration, one optional string arg named `prose`, for which [`ArgsDecl::is_default`] is `true`. An explicit declaration is a map of arg name to arg map.

````yaml
args:
  query:
    type: string
  limit:
    type: integer
    optional: true
````

Each arg name follows the grammar in [Prompt-local names](#prompt-local-names). A bad name fails with "invalid arg name `{key}`: expected \[A-Za-z\]\[A-Za-z0-9_-\]{0,63}".

- [`ArgsDecl::is_default`] returns `true` only when the `args:` key was absent. [Passing the argument string](#passing-the-argument-string) explains how its answer decides the argument string. An explicit declaration with the implicit shape is not equal to [`ArgsDecl::default`].
- [`ArgsDecl::get`] takes `name`, a [`&str`](str), the arg name exactly as written under `args:`. It returns an [`Option`] of a reference to the [`ArgDecl`], or [`None`] when no arg has that name.
- [`ArgsDecl::iter`] returns an iterator of `(name, declaration)` pairs, a [`&str`](str) and a reference to an [`ArgDecl`], over every declared arg. Use it to build a schema or list the arguments. The pairs come in ascending name order, not declaration order, because the args are kept in a sorted map.
- [`ArgsDecl::len`] returns the number of declared args as a [`usize`]. The implicit declaration has length 1.
- [`ArgsDecl::is_empty`] returns `true` when no args are declared. Only an explicit empty map, `args: {}`, produces that. The implicit declaration is not empty.

## ArgDecl

[`ArgDecl`] is one declared arg. The host gets it from [`ArgsDecl::get`] or [`ArgsDecl::iter`]. Its map takes exactly four keys: `type:`, which is required, `optional:`, `default:`, and `description:`. Any other key, such as `tipe`, fails the parse.

````yaml
args:
  use_mcp:
    type: boolean
    default: true
    description: Search MCP-connected private sources
````

This arg reads back as [`ArgType::Boolean`], not optional, with a default of `true` and the description above.

- [`ArgDecl::kind`] returns the declared [`ArgType`], from `type:`. The method does not share the key's name because `type` is a Rust keyword.
- [`ArgDecl::is_optional`] returns a [`bool`], from `optional:`, which defaults to `false`. When it is `true`, a call may omit the field entirely. Absent is not the same as the empty string.
- [`ArgDecl::default`] returns an [`Option`] of a reference to a [`serde_yaml_ng::Value`](https://docs.rs/serde_yaml_ng/latest/serde_yaml_ng/enum.Value.html), from `default:`, or [`None`] when no default is declared. The parse checks the default against `type:`. A `string` needs a YAML string, a `boolean` a YAML bool, a `number` any YAML number, and an `integer` a whole number that fits an [`i64`] or a [`u64`]. A mismatch fails with "the default does not match the declared type `{type}`". For example, `type: boolean` with `default: 'true'`, `type: integer` with `default: 1.5`, and `type: string` with `default: 42` all fail.
- [`ArgDecl::description`] returns the human-readable description as an [`Option`] of [`&str`](str), from `description:`, or [`None`].

## ArgType

[`ArgType`] is the closed set of arg types, one for each word allowed in an arg's `type:` key. The host gets it from [`ArgDecl::kind`], and can also compare it against a named variant such as [`ArgType::String`]. [`ArgType`] implements [`Display`](std::fmt::Display), which prints the YAML spelling of each type: `string`, `boolean`, `integer`, or `number`. It decodes from exactly those four words, and any other word, such as `type: text`, fails the parse with [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter). It has no [`FromStr`](std::str::FromStr) impl.

- [`ArgType::String`]: `type: string`. Supply a JSON string. A declared default must be a YAML string.
- [`ArgType::Boolean`]: `type: boolean`. Supply a JSON `true` or `false`. A declared default must be a YAML bool, so a quoted `'true'` is rejected.
- [`ArgType::Integer`]: `type: integer`. Supply a whole JSON number. A declared default must be a whole number that fits an [`i64`] or a [`u64`], so `1.5` is rejected.
- [`ArgType::Number`]: `type: number`. Supply any JSON number. A declared default may be any YAML number, integer or float.

## ModelRoles

[`ModelRoles`] maps each prompt-local role label to a [`ModelRole`], from the `models:` map. The host gets it from [`Frontmatter::models`]. [`ModelRoles::default`] returns an empty map. The declaration never names a concrete model id. [What prepare checks](#what-prepare-checks) describes how each role is bound and checked.

````yaml
models:
  analyst:
    keywords: [frontier, thinking]
    min_context: 200000
    description: deep reasoning
  spare: {}
````

Each label follows the grammar in [Prompt-local names](#prompt-local-names). A bad label fails with "invalid model role label `{key}`: expected \[A-Za-z\]\[A-Za-z0-9_-\]{0,63}".

- [`ModelRoles::get`] takes `label`, a [`&str`](str), the role label exactly as written under `models:`. It returns an [`Option`] of a reference to the [`ModelRole`], or [`None`] when no role has that label.
- [`ModelRoles::iter`] returns an iterator of `(label, role)` pairs, a [`&str`](str) and a reference to a [`ModelRole`], over every declared role. Use it to check a candidate model against each role before a run. The pairs come in ascending label order, because the roles are kept in a sorted map.
- [`ModelRoles::len`] returns the number of declared roles as a [`usize`].
- [`ModelRoles::is_empty`] returns `true` when no roles are declared, including when the `models:` key is absent.

````
use std::num::NonZeroU32;

use promptforge::Prompt;
use promptforge::prompt::{ModelKeyword, ModelRoles};

let source = concat!(
    "---\n",
    "name: writer\n",
    "description: writes a draft\n",
    "promptforge: 0\n",
    "models:\n",
    "  spare: {}\n",
    "  drafter:\n",
    "    keywords: [no-thinking, creative, chat]\n",
    "    min_context: 32000\n",
    "    description: quick drafts\n",
    "---\n",
    "\n",
    "# Writer\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "writer");
let prompt = parsed?;
let roles = prompt.frontmatter().models();
let labels: Vec<&str> = roles.iter().map(|(label, _role)| label).collect();
assert_eq!(labels, ["drafter", "spare"]);

let drafter = roles.get("drafter").ok_or("drafter is declared")?;
assert_eq!(
    drafter.keywords(),
    &[ModelKeyword::NoThinking, ModelKeyword::Creative, ModelKeyword::Chat],
);
assert_eq!(drafter.min_context(), NonZeroU32::new(32_000));
assert_eq!(drafter.description(), Some("quick drafts"));

let spare = roles.get("spare").ok_or("spare is declared")?;
assert!(spare.keywords().is_empty());
assert_eq!(spare.min_context(), None);
assert_eq!(spare.description(), None);
assert!(ModelRoles::default().is_empty());
# Ok::<(), Box<dyn std::error::Error>>(())
````

## ModelRole

[`ModelRole`] is one declared model role. The host gets it from [`ModelRoles::get`] or [`ModelRoles::iter`]. Its map takes exactly three keys, all optional: `keywords:`, `min_context:`, and `description:`. Any other key, such as `keyword`, fails the parse. An empty map, `{}`, is valid. Parsing checks only this shape.

- [`ModelRole::keywords`] returns a slice of [`ModelKeyword`], from the sequence `keywords:`, in the order written. It is empty when the key is absent. At run time the keywords also appear, in kebab-case, as the capability list of the bound model handle that the prompt sees.
- [`ModelRole::min_context`] returns the minimum context window in tokens as an [`Option`] of a [`NonZeroU32`](std::num::NonZeroU32), from `min_context:`, for example `min_context: 200000`. It returns [`None`] when the key is absent, and `min_context: 0` fails the parse. At prepare, a model whose context window is below the minimum yields an [`UnmetRequirement`](crate::UnmetRequirement) whose [`UnmetRequirement::check`](crate::UnmetRequirement::check) is [`RequirementCheck::ContextMinimum`](crate::RequirementCheck::ContextMinimum), with the minimum in [`UnmetRequirement::required`](crate::UnmetRequirement::required) and the model's context window in [`UnmetRequirement::actual`](crate::UnmetRequirement::actual).
- [`ModelRole::description`] returns the role's description as an [`Option`] of [`&str`](str), from `description:`, or [`None`]. When present, it replaces the model descriptor's own description on the bound model handle that the prompt sees.

## ModelKeyword

[`ModelKeyword`] is the closed vocabulary for a role's `keywords:` list. The host gets it from [`ModelRole::keywords`], and can also name a variant to compare against. It decodes from exactly seven kebab-case words, and any other word, such as `multimodal`, fails the parse with [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter). Adding a keyword is a language change. It has neither a [`Display`](std::fmt::Display) nor a [`FromStr`](std::str::FromStr) impl. It implements [`Ord`], ordered as the variants are listed below.

The two hard keywords are checked against the bound model at prepare, as [What prepare checks](#what-prepare-checks) describes. A failure is an [`UnmetRequirement`](crate::UnmetRequirement) whose [`UnmetRequirement::check`](crate::UnmetRequirement::check) is [`RequirementCheck::HardKeyword`](crate::RequirementCheck::HardKeyword), with the keyword in [`UnmetRequirement::required`](crate::UnmetRequirement::required) and the model's thinking mode name in [`UnmetRequirement::actual`](crate::UnmetRequirement::actual). That name is `"Never"` for a failed `thinking` and `"Always"` for a failed `no-thinking`.

- [`ModelKeyword::Thinking`]: YAML `thinking`. The role needs a model that supports extended thinking. Prepare reports a failure when the model's thinking mode is [`ThinkingMode::Never`](crate::model::ThinkingMode::Never), so pick a model whose mode is [`ThinkingMode::Always`](crate::model::ThinkingMode::Always) or [`ThinkingMode::Switchable`](crate::model::ThinkingMode::Switchable). The bound invocation has thinking switched on.
- [`ModelKeyword::NoThinking`]: YAML `no-thinking`. The role needs a model that does not think. Prepare reports a failure when the model's thinking mode is [`ThinkingMode::Always`](crate::model::ThinkingMode::Always), so pick a model whose mode is [`ThinkingMode::Never`](crate::model::ThinkingMode::Never) or [`ThinkingMode::Switchable`](crate::model::ThinkingMode::Switchable). The bound invocation has thinking switched off.

The five soft keywords record the author's intent and are never checked. A host may use them as hints when it chooses a model.

- [`ModelKeyword::Frontier`]: YAML `frontier`. The author wants a frontier-capability model.
- [`ModelKeyword::Fast`]: YAML `fast`. The author wants a fast model.
- [`ModelKeyword::Small`]: YAML `small`. The author wants a small model.
- [`ModelKeyword::Creative`]: YAML `creative`. The author wants a creative model.
- [`ModelKeyword::Chat`]: YAML `chat`. The author wants a chat-tuned model.
