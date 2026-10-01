Read everything a prompt declares, its files, capabilities, tools, arguments, and model roles, before you run it.

You need this page when you list a prompt's options, build its arguments, or check a model before a run.

# Where this fits

[Run a prompt](crate#run-a-prompt) parses a prompt and runs it. This page reads what parsing hands back: the YAML block at the top of the file, called the *frontmatter*. The frontmatter is the prompt's contract with your program, and all of it is readable right after parsing. [The models page](crate::model) covers what [prepare](crate#answer-a-model) binds to it.

# Read a prompt's contract

You have a parsed prompt, and you want to list what it needs from your program before you run it. [`Frontmatter`] is a read-only view of the prompt's frontmatter.

Reading the frontmatter feels like reading a `Cargo.toml`: a name, a description, and dependencies, all declared up front. Unlike Cargo, parsing never resolves them against your program; prepare does.

````
use promptforge::prompt::ToolSlot;
use promptforge::{ParseErrorKind, Prompt};

// 1. The greeter declares two files, one capability, one tool slot, `times` then `name`, and one model role.
let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Greets someone by name.\n",
#     "promptforge: 0\n",
    "input: { path: names.md, description: The names to greet }\n",
    "output: { path: note.md, description: The note the greeter leaves }\n",
    "capabilities: [example/text]\n",
    "tools: { shout: example/text/shout }\n",
    "args:\n",
    "  times: { type: integer, optional: true, default: 1 }\n",
    "  name: { type: string }\n",
    "models:\n",
    "  writer: { keywords: [fast] }\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello ' .. argv.name)\n",
#     "return store.read('note.md')\n",
#     "```\n",
);

// 2. Parse the prompt, and print its identity from the frontmatter.
let prompt = Prompt::parse(source, "greeter").0?;
let frontmatter = prompt.frontmatter();
println!("{} (version {:?}): {}", frontmatter.name(), frontmatter.promptforge(), frontmatter.description());

// 3. Print every declaration: files, capabilities, tool slots, arguments, and model roles.
for file in [frontmatter.input(), frontmatter.output()].into_iter().flatten() {
    println!("file {}: {}", file.path(), file.description());
}
for capability in frontmatter.capabilities() {
    println!("capability {} (optional: {})", capability.id(), capability.is_optional());
}
for (alias, slot) in frontmatter.tools().iter() {
    let ToolSlot::Exact(id) = slot else { continue };
    println!("tool {alias}: {id}");
}
for (name, arg) in frontmatter.args().iter() {
    println!("arg {name}: {} (optional: {})", arg.kind(), arg.is_optional());
}
for (label, role) in frontmatter.models().iter() {
    println!("role {label}: {:?}, min context {:?}", role.keywords(), role.min_context());
}

// 4. The arguments print sorted by name, not in the file's order.
assert!(frontmatter.args().iter().map(|(name, _)| name).eq(["name", "times"]));

// 5. A copy with `capabilities:` misspelled fails to parse.
let parsed = Prompt::parse(&source.replace("capabilities:", "capabilites:"), "greeter").0;
assert_eq!(parsed.err().map(|error| error.kind()), Some(ParseErrorKind::Frontmatter));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `input` and `output` each name a file in the [store](crate::vfs). `example/text` names a [capability](crate::capabilities), and a bare string like this is required and has no config. `shout` is a [tool slot](crate#call-a-tool), and the first two segments of its path name its capability.
2. [`Prompt::parse`](crate::Prompt::parse) returns the parse result beside the parse events, and `.0?` keeps the result. [`Prompt::frontmatter`](crate::Prompt::frontmatter) gives you the whole contract. `name` and `description` are the only required keys. [`Frontmatter::promptforge`] returns `None` when the file does not mark itself as a PromptForge prompt, so use it to skip ordinary Markdown files.
3. No accessor can fail. An absent key gives `None` for `input`, `output`, `promptforge`, and `max_tool_iterations`, an empty list for `capabilities`, `tools`, and `models`, and one default argument for `args`. `max_tool_iterations` caps one section's tool rounds, and falls back to your run's [`RunLimits`](crate::RunLimits) cap, default 24. So you can list any prompt without guarding for missing sections. The `let ... else` skips any slot form other than `Exact`, because [`ToolSlot`] is non-exhaustive.
4. [`ToolSlots::iter`], [`ArgsDecl::iter`], and [`ModelRoles::iter`] yield entries sorted by alias, name, or label, while [`Frontmatter::capabilities`] keeps the file's order. Their file order cannot be recovered, so do not promise it.
5. The misspelled copy fails with [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter). Every unknown key is a parse error, at the top level or inside an argument, a file declaration, a capability map, or a model role, and so is an unknown model keyword. Only a capability's `config` takes any YAML value. Show the parse error to the prompt's author, instead of running a prompt that lost a setting.

Two `---` lines wrap the frontmatter. A leading byte-order mark is dropped, and the opening `---` may carry surrounding whitespace. The closing `---` must start at column 0; an indented one never closes the block. A file without both lines fails with `ParseErrorKind::Frontmatter`.

A capability can also be a map, with `ref`, `optional`, and `config`. `ref` holds the capability id, the same `namespace/pack` string a bare entry is, and is required in the map form; `optional` and `config` are what the map adds. [`CapabilityDecl::config`] returns any YAML value, unchecked at parse, so validate it before your capability reads it.

Parsing checks only what the file's text shows: keys, names, duplicates, ranges, and default types. Your tool catalog arrives with the [`Environment`](crate::Environment) and your model with the [`RunContext`](crate::RunContext), and both reach the prompt only at prepare. So parse a prompt once, and prepare it against each program or model you want to try. A prompt that parses may still need what your program lacks, and the [requirements notice](crate#answer-a-model) tells you what.

A slot whose tool is in your catalog binds. A slot whose capability has no tools at all in the catalog is reported in `missing_required`, whether or not `capabilities:` declares it. A slot whose capability has other tools in the catalog but not this one is reported nothing, stays unbound, and fails when the prompt uses the alias. Check each slot with [`ToolCatalog::get`](crate::tools::ToolCatalog::get) before the run.

You might expect a misspelled key such as `capabilites:` to be ignored, as a plain serde struct ignores unknown fields. Instead, parsing refuses the whole file, so the typo shows up before any run.

Parsing proves the shape; prepare proves the fit. Next, [Pass arguments to a run](#pass-arguments-to-a-run) builds the values a prompt declares.

# Pass arguments to a run

You want to hand a prompt the values it declares, from a command line or a request. A prompt can list the arguments it takes, each with a type, whether a call may leave it out, a default, and a description. [`ArgsDecl`] holds that list. It describes the fields of the prompt's `argv`, and the prompt itself decides what to do with values that do not fit.

Declared arguments feel like a `clap` argument definition: names, types, optional flags, defaults, and help text. Unlike `clap`, the declaration never checks what you pass. It only describes it.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_store_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

// 1. The greeter declares `times` and `name`; `plain` has no `args:` key and greets `argv.prose`.
let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Greets someone by name.\n",
#     "promptforge: 0\n",
    "args:\n",
    "  times: { type: integer, optional: true, default: 1 }\n",
    "  name: { type: string }\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
    "store.write('note.md', 'hello ' .. argv.name)\n",
#     "return store.read('note.md')\n",
#     "```\n",
);
let plain = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Greets someone by name.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
    "store.write('note.md', 'hello ' .. argv.prose)\n",
#     "return store.read('note.md')\n",
#     "```\n",
);

// 2. Parse both prompts, and print each declared argument with its type.
let prompt = Prompt::parse(source, "greeter").0?;
let plain = Prompt::parse(plain, "greeter").0?;
prompt.frontmatter().args().iter().for_each(|(name, arg)| println!("{name}: {}", arg.kind()));

// 3. Build JSON for declared arguments, and keep plain text for the default declaration.
assert!(!prompt.frontmatter().args().is_default() && plain.frontmatter().args().is_default());
let json = serde_json::json!({ "name": "world" }).to_string();

// 4. Run each prompt with its argument text, answering its store effects, and read the result.
fn greet(prompt: Prompt, text: &str) -> RunResult {
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let mut run = Run::new(Arc::new(prompt), text, ctx);
    loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    let Effect::Store { access, op } = effect else { panic!("the greeter only uses the store") };
                    run.resume(id, EffectAnswer::Store(perform_store_op(&access, op)));
                }
            }
            Step::Done { result, .. } => return result,
        }
    }
}
assert!(matches!(greet(prompt, &json), RunResult::Ok(text) if text == "hello world"));
assert!(matches!(greet(plain, "world"), RunResult::Ok(text) if text == "hello world"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. `source` declares a required string `name` and an optional integer `times` with default 1, and its Lua greets `argv.name`. `plain` is the same greeter with no `args:` key, and its Lua greets `argv.prose`.
2. [`ArgDecl::kind`] returns an [`ArgType`], whose `Display` prints the YAML spelling: `string`, `boolean`, `integer`, or `number`. Those are also JSON Schema type names. So you can derive a schema field or a CLI help line from `kind` with no mapping table of your own.
3. Check [`ArgsDecl::is_default`] first. It is true only when the prompt has no `args:` key. Such a prompt takes one optional string argument named `prose`, and plain text you pass wraps into `argv = { prose = "<text>" }`. The greeter declares its arguments, so the example builds JSON text with [`serde_json`](https://docs.rs/serde_json).
4. `greet` creates and steps each run as [Run a prompt](crate#run-a-prompt) does. [`RunContext::new`](crate::RunContext::new) takes the run's name, a seed, and the start time every section reads as `sys.when`. Fixed values make the example repeat exactly; a live program draws the seed from a secure random source, because it seeds the untrusted-envelope nonce. What this step adds is the second argument of [`Run::new`](crate::Run::new): the argument text. Lua reads the exact text as `args`, and reads `argv` as the text wrapped into `prose` or parsed as JSON. The JSON arrives as `argv.name`, the plain text wraps into `argv.prose`, and both runs give "hello world".

An explicit `args:` key turns wrapping off, even one that declares only an optional string `prose`. An empty `args:` map declares no arguments at all. So decide whether to wrap by `is_default`, never by the argument names.

For a prompt that declares `args:`, text that is not valid JSON, or the JSON `null`, makes `argv` nil, and the run goes on; nothing refuses it. So a prompt checks `if argv then`, and your program validates the text before the run when it needs that guarantee.

[`ArgDecl::is_optional`] means a call may leave the field out entirely. Absent is not the same as an empty string, so omit an optional field instead of sending `""`.

[`ArgDecl::default`] returns a raw [`serde_yaml_ng::Value`](https://docs.rs/serde_yaml_ng/latest/serde_yaml_ng/enum.Value.html), not a typed value. An `integer` default is always a whole number that fits `i64` or `u64`, while a `number` default may be any number. Convert the default yourself before you show it or send it. The run never fills in a default, so send it yourself when the prompt needs the value.

The declaration enforces nothing on the values you pass. Checking them belongs to the Lua under the prompt's `#` title, which runs before the first `##` section and is the only place a prompt can change `argv`. See [Before you start](crate#before-you-start) for the `#` title.

You might expect a run to refuse an `integer` argument sent as `"five"`, as serde refuses a mistyped field. Instead, nothing checks your values against the declaration, and the prompt decides what a bad value means.

The declaration describes the arguments; your program builds them, and the prompt judges them. Next, [Check a prompt's model roles](#check-a-prompts-model-roles) reads what a prompt needs from a model.

# Check a prompt's model roles

Before prepare, you want to see what models a prompt needs, and whether your model can serve them. [`ModelRole`] is your view of one [model role](crate#answer-a-model). Prepare checks only its hard keywords, `thinking` and `no-thinking`, and its context minimum. The other needs are never checked.

````
use std::num::NonZeroU32;

use promptforge::model::{ModelDescriptor, ModelId, ThinkingMode};
use promptforge::prompt::ModelKeyword;
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, RequirementCheck, RunContext};

// 1. The greeter declares a `fast` writer, and a checker that must not think and needs 100000 tokens.
let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Greets someone by name.\n",
#     "promptforge: 0\n",
    "models:\n",
    "  writer: { keywords: [fast] }\n",
    "  checker: { keywords: [no-thinking], min_context: 100000 }\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello ' .. argv.name)\n",
#     "return store.read('note.md')\n",
#     "```\n",
);
let prompt = Prompt::parse(source, "greeter").0?;

// 2. Describe one model with a 200000-token window that always thinks.
let window = NonZeroU32::new(200_000).ok_or("a context window is never zero")?;
let id = ModelId::gateway("canned")?;
let model = ModelDescriptor::new(id, "Thinks on every call", window, ThinkingMode::Always);

// 3. Compare each role's hard keywords and context minimum with the model, and record each miss.
let mut misses = Vec::new();
for (label, role) in prompt.frontmatter().models().iter() {
    let keyword_miss = role.keywords().iter().any(|keyword| match keyword {
        ModelKeyword::Thinking => model.thinking() == ThinkingMode::Never,
        ModelKeyword::NoThinking => model.thinking() == ThinkingMode::Always,
        _ => false,
    });
    let context_miss = role.min_context().is_some_and(|min| min > model.context());
    if keyword_miss || context_miss {
        misses.push((label, keyword_miss, context_miss));
    }
}
assert_eq!(misses, [("checker", true, false)]);

// 4. Prepare with the same model, and read the one unmet requirement it reports.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).model(model);
let (_ctx, requirements) = Environment::new().prepare(&prompt, ctx);
let [unmet] = requirements.unmet_requirements.as_slice() else { panic!("only the checker falls short") };
assert_eq!(unmet.role, "checker");
assert_eq!(unmet.check, RequirementCheck::HardKeyword);
assert_eq!(unmet.required, "no-thinking");
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The role `writer` has the keyword `fast`. The role `checker` has the keyword `no-thinking` and a `min_context` of 100000 tokens. Every key in a role is optional, so a role with no keywords and no minimum accepts any model. YAML spells keywords in kebab-case, so [`ModelKeyword::NoThinking`] is written `no-thinking`. An unknown keyword is a parse error.
2. A [`ModelDescriptor`](crate::model::ModelDescriptor) describes your model: a 200000-token window that always thinks, [`ThinkingMode::Always`](crate::model::ThinkingMode::Always). It names no role, because role labels belong to the prompt.
3. [`ModelRoles::iter`] yields `(label, role)` pairs sorted by label. Only `thinking` and `no-thinking` are hard keywords, so the match tests only `Thinking` and `NoThinking`. The soft keywords, `frontier`, `fast`, `small`, `creative`, and `chat`, only record what the author intended, so the wildcard arm skips them. [`ModelRole::min_context`] is in tokens, and you compare it with your model's window. The assert shows `writer` passes and `checker` misses on its keyword alone, since 100000 fits in 200000.
4. [`RunContext::model`](crate::RunContext::model) sets the context's one current model, and [`Environment::prepare`](crate::Environment::prepare) binds every role to it. When the context has a current model, prepare always binds that way; with none, every role stays unbound. There is no other fill to choose, and you cannot write the bindings yourself. Prepare then reports each hard miss as an unmet requirement, not as an error. Here it reports `checker` as a `no-thinking` miss, and ignores `fast`.

Prepare reports [`RequirementCheck::HardKeyword`](crate::RequirementCheck::HardKeyword) for a `thinking` role on a model that never thinks, or a `no-thinking` role on one that always thinks. It reports [`RequirementCheck::ContextMinimum`](crate::RequirementCheck::ContextMinimum) when a role's minimum is above your model's window. Read the requirements notice after prepare, and choose whether to run anyway or pick another model.

You might expect a `fast` or `small` keyword to steer prepare toward a matching model. Instead, soft keywords are never checked, and every role binds to the one current model. A `ModelDescriptor` records only a context window and a thinking mode, so there is nothing for `fast` or `small` to be checked against, and with one current model there is nothing to choose between. When a soft keyword matters to you, choose the model yourself before prepare.

Only thinking and the context minimum are checked; the other keywords are notes from the author. Next, [the models page](crate::model) shows how to describe the models that prepare binds.

# Reference

## ArgDecl

[`ArgDecl`] describes one argument under a prompt's `args:` key: its type, whether a call may omit it, its default, and its description. Use it to document an argument or derive a schema field; it enforces nothing on calls. Parsing fails when `type` is missing, when `default` does not match `type`, or on any key besides `type`, `optional`, `default`, and `description`. An `integer` default must be whole, so `1.5` is refused. See [Pass arguments to a run](#pass-arguments-to-a-run).

- [`ArgDecl::is_optional`]: a call may leave the field out entirely; absent is not the empty string.
- [`ArgDecl::default`]: the raw YAML value as written, not a typed Rust value.

## ArgsDecl

[`ArgsDecl`] maps each argument name to its [`ArgDecl`]. A prompt with no `args:` key gets one optional `string` argument named `prose`, and plain text wraps into `argv = { prose = "<text>" }`. Use it to list arguments, or to choose between wrapping plain text and building JSON. Parsing fails when a name is outside `[A-Za-z][A-Za-z0-9_-]{0,63}`, or when two arguments share a name; rename one or drop the duplicate. See [Pass arguments to a run](#pass-arguments-to-a-run).

- [`ArgsDecl::is_default`]: true only when the prompt has no `args:` key; an explicit `args:`, even the same `prose` shape, gives false and never wraps.
- [`ArgsDecl::iter`]: yields arguments sorted by name, not in the file's order.
- [`ArgsDecl::is_empty`]: true for an explicit empty `args:` map; the implicit default always holds `prose`.

## CapabilityDecl

[`CapabilityDecl`] names one [capability](crate::capabilities) a prompt activates, by its `namespace/pack` id. Write it as a bare id, which is required, or as a map with `ref`, `optional`, and `config`. Credentials come from your run services, never from the prompt. Parsing fails on an id without exactly two segments or with an `@` version pin, on a repeated id, and on an optional capability that backs a tool slot. See [Read a prompt's contract](#read-a-prompts-contract).

- [`CapabilityDecl::is_optional`]: whether your capability activation may skip it. Prepare never checks the `capabilities:` list, so it never reports a missing optional capability.
- [`CapabilityDecl::config`]: any YAML value, unchecked at parse; validate it before your capability reads it.

## FileDecl

[`FileDecl`] names a [store](crate::vfs) file the prompt expects when it starts, under `input`, or leaves when it finishes, under `output`. Use it to stage the input file before a run, or collect the output file after. Parsing fails when `path` or `description` is missing, or when the declaration has any other key; add the missing key or remove the extra one. See [Read a prompt's contract](#read-a-prompts-contract).

- [`FileDecl::path`]: the filename inside the store, such as `"paper.md"`.
- [`FileDecl::description`]: the file's purpose, which also feeds MCP schema generation.

## Frontmatter

[`Frontmatter`] holds a parsed prompt's YAML block, its whole contract. Use it to list a prompt or read its declarations before a run. Parsing fails on any unknown key, a missing `name` or `description`, a `max_tool_iterations` outside 1 to 1000, and one name used as both a tool alias and a role label. A block that never opens or closes fails with [`ParseErrorKind::Frontmatter`](crate::ParseErrorKind::Frontmatter); start the closing `---` at column 0. See [Read a prompt's contract](#read-a-prompts-contract).

- [`Frontmatter::promptforge`]: the major version of this crate the file targets; `None` means the file is not a PromptForge prompt.
- [`Frontmatter::max_tool_iterations`]: caps model round trips in one section's tool loop; `None` leaves the run's own cap in force.
- [`Frontmatter::capabilities`]: in declaration order; empty when `capabilities:` is absent.
- [`Frontmatter::args`]: the default `prose` declaration when `args:` is absent.

## ModelRole

[`ModelRole`] is your view of one [model role](crate#answer-a-model): its keywords, a context minimum in tokens, and a description. Use it to check what a role requires before you choose a model. An unknown key or keyword fails parsing, and prepare reports a hard keyword miss or a too-small window as an unmet requirement. Then choose a model with the matching thinking mode or a larger window, or refuse the run. See [Check a prompt's model roles](#check-a-prompts-model-roles).

- [`ModelRole::keywords`]: the keywords as written.
- [`ModelRole::min_context`]: in tokens; `min_context: 0` cannot be written.

## ModelRoles

[`ModelRoles`] maps each role label to its [`ModelRole`], and labels are local to the prompt. Prepare binds every role to the context's current model, or leaves all unbound without one; see [Answer a model](crate#answer-a-model). Parsing fails when a label breaks the alias pattern, repeats, is also a tool alias, or is a reserved name: an Engine global, a kept Lua standard-library global, or a Lua keyword. Each label becomes a global in the section, so rename the label. See [Check a prompt's model roles](#check-a-prompts-model-roles).

- [`ModelRoles::iter`]: yields roles sorted by label, not in declaration order.

## ToolSlots

[`ToolSlots`] maps each alias to its [`ToolSlot`]. Use it to see which tools a prompt binds, and under which aliases, which are all the model ever sees. Parsing fails on the alias `open`, an alias outside `[A-Za-z][A-Za-z0-9_-]{0,63}`, a duplicate, a reserved name, a role label, or a slot whose capability is declared optional. Rename the alias, and declare the capability as required. See [Read a prompt's contract](#read-a-prompts-contract).

- [`ToolSlots::iter`]: yields slots sorted by alias, not in declaration order.

## ArgType

[`ArgType`] names the four argument types. Match on [`ArgDecl::kind`] to build a schema field for each argument; `Display` prints the same lowercase spelling as the YAML. Any other `type:` value fails parsing, so use one of the four spellings. See [Pass arguments to a run](#pass-arguments-to-a-run).

| Variant | YAML | The default must be |
|---|---|---|
| `String` | `string` | a YAML string |
| `Boolean` | `boolean` | a YAML bool |
| `Integer` | `integer` | a whole number that fits in 64 bits, so `1.5` is refused |
| `Number` | `number` | any YAML number, integers included |

## ModelKeyword

[`ModelKeyword`] names the closed set of model keywords. Prepare checks the hard keywords against the model, and soft keywords only record what the author intended. Match on it when you read [`ModelRole::keywords`]. YAML spells each keyword in kebab-case, and an unknown keyword fails parsing, so use a listed spelling. See [Check a prompt's model roles](#check-a-prompts-model-roles).

| Variant | YAML | Kind | Checked at prepare |
|---|---|---|---|
| `Thinking` | `thinking` | hard | unmet against a model that never thinks |
| `NoThinking` | `no-thinking` | hard | unmet against a model that always thinks |
| `Frontier` | `frontier` | soft | never |
| `Fast` | `fast` | soft | never |
| `Small` | `small` | soft | never |
| `Creative` | `creative` | soft | never |
| `Chat` | `chat` | soft | never |

## ToolSlot

[`ToolSlot`] says how one tool slot is filled, and is `#[non_exhaustive]`, so your match needs a wildcard arm. Parsing fails when a slot value is not a string or not a valid tool path. When the slot's capability has no tools in your catalog, prepare lists it in [`Requirements::missing_required`](crate::Requirements::missing_required). When the capability has tools but not this one, prepare reports nothing and leaves the alias unbound, so check each slot with [`ToolCatalog::get`](crate::tools::ToolCatalog::get). See [Read a prompt's contract](#read-a-prompts-contract).

- `Exact`: holds a [`ToolId`](crate::tools::ToolId), the exact global tool path; its first two segments name the tool's capability.

