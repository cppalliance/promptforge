Name packs of tools as capabilities, check which tools belong to each, and give every section a capability's Lua.

You need this when you group tools into packs, or add Lua that every section of a run loads.

# Where this fits

From [the crate page](crate), you know how your program, the Harness, prepares a run of a prompt and steps through its sections. From the [tools page](crate::tools), you know that a tool name has three parts, `namespace/pack/name`. This page adds the pack: several tools offered together under the name made of the first two parts. That pack is a *capability*, and a capability can also add Lua, its *prelude*, to every section of a run.

# Name a capability

Your program offers several tools as one pack, and you need a name for the pack and a way to check that each tool belongs to it.

A pack name works like a Rust module path, where a module holds every item whose path starts with its own. Unlike a module path, the parts are separated by `/`, and two parts name a pack while three name a tool. That two-part name is a *capability*, and a [`CapabilityId`] holds one. The number of parts marks what a name is, so anyone reading a name can tell a capability from a tool by counting. A tool's capability is always its first two parts: [`ToolId::capability`](crate::tools::ToolId::capability) returns it straight from the tool name, with no lookup.

````
use promptforge::capabilities::{CapabilityId, CapabilityIdErrorKind};
use promptforge::tools::ToolId;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Name the pack that holds the greeter's web tools.
    let web = CapabilityId::parse("promptforge/web")?;

    // 2. Check which tools belong to the pack: its own fetch does, another pack's fetch does not.
    let fetch = ToolId::parse("promptforge/web/fetch")?;
    let other = ToolId::parse("promptforge/other/fetch")?;
    assert!(web.contains(&fetch));
    assert!(!web.contains(&other));

    // 3. A one-part name, such as the program's own name, is not a capability.
    let error = CapabilityId::parse("greeter")
        .err()
        .ok_or("a one-part name is not a capability id")?;
    assert_eq!(error.kind(), CapabilityIdErrorKind::SegmentCount);
    Ok(())
}
````

1. [`CapabilityId::parse`] accepts exactly two parts, `namespace/pack`, such as `"promptforge/web"`, so the `?` succeeds. `CapabilityId::parse` is the only constructor, but `ToolId::capability` also hands you a `CapabilityId`, the first two parts of a tool id that already parsed, and deserializing goes through `parse`. Every one of these ways yields a valid id, so every id you hold is already valid. Each part may use only lowercase ASCII letters, digits, `-`, `_`, and `.`.
2. [`ToolId::parse`](crate::tools::ToolId::parse) makes two three-part tool names, and [`CapabilityId::contains`] checks each one. It is true exactly when the tool name minus its last part equals the capability id. So `promptforge/web/fetch` is in the pack, and `promptforge/other/fetch`, which shares only the namespace, is out.
3. The one-part name `greeter` fails, and [`CapabilityIdError::kind`] says why: [`CapabilityIdErrorKind::SegmentCount`], the wrong number of parts. Match on the kind, because the reason text is only for display.

The kind has two more cases. `Empty` means an empty part, such as `"a//b"`. `Control` means a character outside the allowed set, so uppercase, spaces, a version such as `@1`, and non-ASCII characters all land there. An empty string fails with `SegmentCount`, not `Empty`.

A three-part tool name such as `"promptforge/web/fetch"` also fails `CapabilityId::parse` with `SegmentCount`, because three parts name a tool. When a string may be either a capability or a tool, use [`GlobalName::parse`], which accepts both two and three parts.

A `CapabilityId` serializes as its `namespace/pack` string and deserializes through `CapabilityId::parse`, so a bad name in a config file is a deserialization error.

This crate knows capabilities only by name, and never checks which tools they offer. Check containment yourself with `CapabilityId::contains` when you assemble the run's tool catalog, so a tool outside its capability's name never reaches a run.

*Capability activation* is the step your program runs before prepare that turns each capability the prompt declares into tools for the catalog and preludes for the environment, and records in a [`Requirements`](crate::Requirements) of its own what it could not supply. This crate never runs that step.

A prompt declares its capabilities in a `capabilities:` frontmatter list, one `namespace/pack` per entry, and binds each tool in a `tools:` slot whose path starts with a capability's two parts, as [Call a tool](crate#call-a-tool) shows. [Prepare](crate::Environment::prepare) never reads the `capabilities:` list. It lists a capability in [`Requirements::missing_required`](crate::Requirements::missing_required) only when an exact tool slot's path names it and your [`Environment::tools`](crate::Environment::tools) catalog has no tool for it at all. Only your activation reports a declared capability that no slot names, so combine both reports with [`Requirements::merge`](crate::Requirements::merge) before you tell the user what to install.

You might expect `CapabilityId::parse("PromptForge/web")` to lowercase the name. Instead, it fails with `Control`, because uppercase is never allowed.

Count the parts: two is a capability, three is a tool inside it. Next, [Add a prelude to every section](#add-a-prelude-to-every-section) gives every section the Lua a capability adds.

# Add a prelude to every section

Your capability's tools are clumsy to call directly, and you want every [section](crate) of a run to have a short Lua helper for them.

This works like Rust's prelude, names in scope everywhere without an import, except that you write it in Lua. Lua text that a capability adds to every section of a run is a *prelude*, and a [`Prelude`] holds one.

````
use std::sync::Arc;

use promptforge::capabilities::{CapabilityId, Prelude};
use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Environment, Prompt, Run, RunContext, RunResult, Step};

// 1. The greeter's section calls `greet`, which the prompt never defines, and stores its result.
let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: Greets through a prelude helper and reads the greeting back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n## Greet\n\n",
    "```lua\n",
    "store.write('note.md', greet('world'))\n",
    "return store.read('note.md')\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greeter");
let prompt = Arc::new(parsed?);

// 2. The `promptforge/web` capability adds a prelude that defines `greet`.
let web = CapabilityId::parse("promptforge/web")?;
let prelude = Prelude::new(web, "function greet(name) return 'hello ' .. name end");

// 3. Put the prelude on the environment, and prepare the run with it.
let env = Environment::new().preludes(vec![prelude]);
let (ctx, _requirements) = env.prepare(&prompt, RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH));
let mut run = Run::new(prompt, "", ctx);

// 4. Answer each store effect from the in-memory store, and step until the run is done.
let result = loop {
    let effects = match run.step() {
        Step::Pending { effects, .. } => effects,
        Step::Done { result, .. } => break result,
    };
    for (id, _provenance, effect) in effects {
        let Effect::Vfs { access, op } = effect else { panic!("the greeter issues only store effects") };
        run.resume(id, EffectAnswer::Vfs(perform_vfs_op(&access, op)));
    }
};
assert!(matches!(result, RunResult::Ok(text) if text == "hello world"));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The greeter's only section writes `greet('world')` to `note.md` and returns it read back, though the prompt never defines `greet`.
2. [`Prelude::new`] pairs the `promptforge/web` id, which names the prelude in tracebacks and error messages, with one line of Lua. A real prelude defines helpers, such as `sh.run(script)`, that reach the capability's tools through `tools.call`, the Lua call that asks your program to run a tool. The run hands you an [`Effect::ToolCall`](crate::effect::Effect::ToolCall) to answer, as [Call a tool](crate#call-a-tool) shows. Name each tool by its full `namespace/pack/name` path, and call tools only inside a prelude's functions, because a prelude that calls a tool while it loads fails the run.
3. [`Environment::preludes`](crate::Environment::preludes) puts the preludes on the environment, and [`Environment::prepare`](crate::Environment::prepare) prepares the run with them. As in [Run a prompt](crate#run-a-prompt), [`RunContext::new`](crate::RunContext::new) takes the run's name, a seed, and a start time. The fixed seed `7` and [`Timestamp::UNIX_EPOCH`](crate::timestamp::Timestamp::UNIX_EPOCH) make the test repeat exactly, and a live program passes a secure random seed and the current time. Call [`Requirements::refusal`](crate::Requirements::refusal) on the report prepare returns, and when it returns an error, show it and do not call [`Run::new`](crate::Run::new). The greeter declares no tools or models, so its report has no gaps. The empty string passed to `Run::new` is the run's argument string, which reaches Lua as `args`.
4. The loop answers the store effects with [`perform_vfs_op`](crate::vfs::perform_vfs_op) and steps until [`Step::Done`](crate::Step::Done). A context from `RunContext::new` starts with a fresh in-memory store at `/`, and `perform_vfs_op` reads and writes it through each effect's `access`, so the example needs no store of its own. The section calls `greet` without loading it, and the store reads back `hello world`.

Every section, fanout arm, and spawned task installs the preludes afresh, so nothing one section does reaches another's copy. A table a prelude defines is read-only at its top level, so a section that assigns a new field gets an error naming the capability. Each prelude sees only the base Lua functions, `string`, `table`, `math`, `tools`, `store`, `untrusted`, and a read-only `var`, so it cannot call another prelude's helpers.

Each section installs the preludes after the `tools` and `store` Engine globals and before the prompt's shared library, the `lua shared` block under the `#` title, so even the shared library's top-level code can call prelude helpers. The prompt's frontmatter tool and model aliases install last, which is why a prelude may not use their names.

A prelude that fails to load fails the run with [`RunErrorKind::Lua`](crate::RunErrorKind::Lua) before the run issues any effect. That covers a prelude that does not compile, raises an error, or calls a tool while loading, and the error and its traceback name the prelude's capability. A prelude also fails the run when it defines a global that another prelude, an Engine global such as `tools` or `store`, or a reserved name such as `ui` or `item` already holds. So does a global named like one of the prompt's frontmatter tool or model aliases.

You might expect `Prelude::new` to reject Lua that does not compile. Instead, it accepts any text and any id and compiles nothing, so test each prelude with a run.

A prelude is Lua every section gets for free, and it is checked only when a section loads it. Next, [Reference](#reference) covers every item this page teaches.

# Reference

## CapabilityId

[`CapabilityId`] names a capability as `namespace/pack`, the prefix of every tool id it contributes. Use it to key a capability or compare with the missing capabilities prepare reports. [`CapabilityId::parse`] is the only public constructor, so every id you hold is valid, and [`ToolId::capability`](crate::tools::ToolId::capability) also returns one, built from a tool id that already parsed. Parsing and deserializing reject text that is not exactly two valid segments. Fix the name, or branch on [`CapabilityIdError::kind`], as [Name a capability](#name-a-capability) teaches.

- `parse` needs lowercase ASCII letters, digits, `-`, `_`, and `.`; uppercase is rejected, not folded, and `@` fails because names are unversioned.
- `contains` is true exactly when dropping the tool id's last segment yields this id.
- `Display` writes the canonical `namespace/pack` form, which is also the single string it serializes as.
- `Ord` compares the namespace first, then the pack, segment by segment rather than as one string.

## CapabilityIdError

[`CapabilityIdError`] says why a capability id did not parse. Use it when you report a bad capability name, or branch on which rule it broke. Match on [`CapabilityIdError::kind`], the stable classification, instead of the display text. The human-readable reason is reachable only through `Display`, because there is no accessor for it. [Name a capability](#name-a-capability) matches on its kind.

## GlobalName

[`GlobalName`] holds a name in the one naming grammar, where two segments name a capability, `namespace/pack`, and three name a tool, `namespace/pack/name`. Use it to validate a name before you know which of the two it names. [`GlobalName::parse`] rejects one segment, four or more, an empty segment, and any byte outside `a-z`, `0-9`, `-`, `_`, and `.`, so every non-ASCII character fails. Fix the name, or branch on [`GlobalNameError::kind`]. [Name a capability](#name-a-capability) introduces it.

- Unlike [`CapabilityId`], it does not serialize or deserialize, so store its `Display` string instead.
- [`GlobalName::pack`] returns the second segment for both two- and three-segment names.
- `Display` joins the segments with `/`, reproducing the parsed text, and is the only way to read a tool name's third segment.

## GlobalNameError

[`GlobalNameError`] says why a global name did not parse. Use it when you report a bad name, or branch on which rule it broke. Match on [`GlobalNameError::kind`], the stable classification, rather than the display text.

## Prelude

[`Prelude`] carries the Lua source that one activated capability adds to every section of a run, named by that capability's id. Use it to give every section helpers, such as `sh.run(script)`, that reach the capability's tools through `tools.call`. [`Prelude::new`] never fails: it accepts any source, even empty or non-Lua text, and any id. The id only names the prelude in tracebacks and error messages, so read those to find which prelude broke. [Add a prelude to every section](#add-a-prelude-to-every-section) teaches it.

## CapabilityIdErrorKind

[`CapabilityIdErrorKind`] classifies why a capability id failed to parse, in a form stable enough to match on. You reach it through [`CapabilityIdError::kind`]. Add a wildcard arm to every match, because the enum is `#[non_exhaustive]`. [Name a capability](#name-a-capability) matches on it.

- `SegmentCount`: the id did not have exactly two segments, including the empty string and a valid three-segment tool name.
- `Empty`: a segment was empty, as in `"a//b"` or `"/web"`.
- `Control`: despite the name, any disallowed byte, so uppercase letters, `@`, spaces, and non-ASCII characters land here with control characters.

## GlobalNameErrorKind

[`GlobalNameErrorKind`] classifies why a global name failed to parse, in a form stable enough to match on. You reach it through [`GlobalNameError::kind`]. Add a wildcard arm to every match, because the enum is `#[non_exhaustive]`.

- `SegmentCount`: the name did not have two or three segments; the empty string lands here.
- `Empty`: a segment was empty.
- `Control`: a segment held a control byte or any other disallowed byte, such as uppercase, `@`, a space, or a non-ASCII character.
