Capability ids, the naming grammar shared by capability and tool names, the errors that name each broken rule, and the Lua preludes capabilities hand a run.

A capability is host code that runs at run setup and makes services, such as tools, available to a run. PromptForge knows a capability only by its identity, a short `namespace/pack` name such as `promptforge/web`. This module turns that text into a validated [`CapabilityId`], checks whether a tool belongs to a capability, and validates any capability or tool name against the one grammar they share. When a name is wrong, the error carries a kind for the host to branch on and a message that states the rule the name broke. It also holds [`Prelude`], the Lua source a capability can contribute so that prompt authors call friendly functions instead of raw tool calls.

# Where this fits

Capability ids never travel through an [`Effect`](crate::effect::Effect), an [`EffectAnswer`](crate::effect::EffectAnswer), or an [`Event`](crate::event::Event). They matter in the preflight before [`Run::new`](crate::Run::new), where the host works through these steps.

1. **Read the declarations.** [`Frontmatter::capabilities`](crate::prompt::Frontmatter::capabilities) returns the prompt's [`CapabilityDecl`](crate::prompt::CapabilityDecl) values. Each one's [`CapabilityDecl::id`](crate::prompt::CapabilityDecl::id) is a [`GlobalName`], this module's type for a validated name that may be either a capability or a tool, and here it always has two segments. To get a [`CapabilityId`] from it, re-parse its [`Display`](std::fmt::Display) text with [`CapabilityId::parse`].
2. **Activate.** The host activates those capabilities from its own registry and gathers each tool's [`ToolDescriptor`](crate::tools::ToolDescriptor). A descriptor's [`ToolDescriptor::conflicts`](crate::tools::ToolDescriptor::conflicts) lists [`CapabilityId`] values, which the host checks before activation. The host can confirm that each tool belongs to its capability with [`CapabilityId::contains`]. The host also gathers each activated capability's [`Prelude`], if it has one, in the order the prompt declares the capabilities, and installs the list with [`Environment::preludes`](crate::Environment::preludes).
3. **Report.** The host records what it could not satisfy in a [`Requirements`](crate::Requirements) value. An absent capability goes onto [`Requirements::missing_required`](crate::Requirements::missing_required). A present, required capability that needs a host service the host does not provide becomes a [`MissingService`](crate::MissingService), built with [`MissingService::new`](crate::MissingService::new) and pushed onto [`Requirements::missing_services`](crate::Requirements::missing_services). A declared clash between two capabilities becomes a [`CapabilityConflict`](crate::CapabilityConflict), built with [`CapabilityConflict::new`](crate::CapabilityConflict::new) and pushed onto [`Requirements::conflicts`](crate::Requirements::conflicts).
4. **Prepare and merge.** [`Environment::prepare`](crate::Environment::prepare) adds its own [`Requirements::missing_required`](crate::Requirements::missing_required) entries for tool slots whose capability contributed nothing to the catalog. The host folds its report in with [`Requirements::merge`](crate::Requirements::merge).
5. **Refuse or run.** When [`Requirements::refusal`](crate::Requirements::refusal) returns a [`RunError`](crate::RunError), the host fails the run with it instead of calling [`Run::new`](crate::Run::new). [`Requirements::notice`](crate::Requirements::notice) renders every id through its [`Display`](std::fmt::Display) form, for example `missing required capability: promptforge/web`.

During the run, the capability behind any [`Effect::ToolCall`](crate::effect::Effect::ToolCall) is available from its [`tool`](crate::effect::Effect#variant.ToolCall.field.tool) field through [`ToolId::capability`](crate::tools::ToolId::capability). Every section's Lua machine installs the run's preludes before any of the prompt's own Lua runs, and [Handing preludes to a run](#handing-preludes-to-a-run) describes what that means for the host.

# Parsing a capability id

This example parses a capability id, reads it back, checks a tool against it, and shows what happens when a tool id is passed where a capability id belongs.

````
use promptforge::capabilities::{CapabilityId, CapabilityIdErrorKind};
use promptforge::tools::ToolId;

let web = CapabilityId::parse("promptforge/web")?;
assert_eq!(web.namespace(), "promptforge");
assert_eq!(web.pack(), "web");
assert_eq!(web.to_string(), "promptforge/web");

let fetch = ToolId::parse("promptforge/web/fetch")?;
assert!(web.contains(&fetch));
assert_eq!(fetch.capability(), web);

let error = CapabilityId::parse("promptforge/web/fetch")
    .err()
    .ok_or("a tool id is not a capability id")?;
assert!(matches!(error.kind(), CapabilityIdErrorKind::SegmentCount));
assert_eq!(
    error.to_string(),
    "invalid capability id: a capability id must have exactly 2 segments (namespace/pack)",
);
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Parse.** [`CapabilityId::parse`] takes the exact `namespace/pack` text and returns a [`Result`] of a [`CapabilityId`] or a [`CapabilityIdError`]. The parsed id can be declared, compared, or reported wherever a capability identity is expected.
2. **Read it back.** [`CapabilityId::namespace`] and [`CapabilityId::pack`] return the two segments as borrowed [`&str`](str) values. The [`Display`](std::fmt::Display) form is the canonical `namespace/pack` text.
3. **Check a tool.** A tool id has three segments, `namespace/pack/name`, and its first two name the capability that contributes it. [`CapabilityId::contains`] returns `true` for a [`ToolId`](crate::tools::ToolId) that belongs to the capability. [`ToolId::capability`](crate::tools::ToolId::capability) goes the other way and returns the tool's [`CapabilityId`] without re-parsing.
4. **Handle a rejection.** A three-segment tool id is not a capability id, so the parse fails. [`CapabilityIdError::kind`] returns a [`CapabilityIdErrorKind`] to branch on, and the [`Display`](std::fmt::Display) text names the rule that was broken.

# The naming grammar

Capability ids and tool ids follow one grammar. Here it is in full:

````text
capability id  =  segment "/" segment
tool id        =  segment "/" segment "/" segment
segment        =  one or more bytes, each one of  a-z  0-9  -  _  .
````

- **Separator.** Only `/` separates segments. The text is split on every `/` with no trimming, so a leading, trailing, or doubled slash produces an empty segment.
- **Segment count.** A [`CapabilityId`] has exactly 2 segments, `namespace/pack`. A [`ToolId`](crate::tools::ToolId) has exactly 3, `namespace/pack/name`. A [`GlobalName`] accepts either 2 or 3.
- **Characters.** Each byte of a segment is a lowercase ASCII letter `a` to `z`, a digit `0` to `9`, `-`, `_`, or `.`. Every other byte is rejected. That includes uppercase letters, spaces, `@`, `:`, other punctuation, control bytes, and every non-ASCII byte.
- **Case.** Nothing is lowercased on the way in. `Promptforge/web` and `promptforge/Web` are parse errors, not aliases of `promptforge/web`, and comparison is case-sensitive.
- **Versions.** Names are unversioned. A pin such as `promptforge/web@2` is a parse error.
- **Length.** Each segment needs at least one byte. There is no maximum length for a segment or for the whole name.
- **Position.** No positional rules are checked. A segment may start or end with `-`, `_`, `.`, or a digit, and a segment made only of those characters, such as `..`, passes.

**Naming your own capabilities.** Put your own capabilities under a reverse-DNS namespace such as `org.rustalliance`, and leave the `promptforge` namespace for first-party packs. These are conventions, and the parser checks neither of them. The `.` in a reverse-DNS namespace is simply an allowed character.

````
use promptforge::capabilities::{CapabilityId, GlobalName};
use promptforge::tools::ToolId;

let own = CapabilityId::parse("org.rustalliance/core")?;
assert_eq!(own.namespace(), "org.rustalliance");
assert_eq!(own.pack(), "core");
assert!(ToolId::parse("org.rustalliance/core/search").is_ok());
assert!(GlobalName::parse("org.rustalliance/my-pack/v1_2.tool").is_ok());
assert!(CapabilityId::parse("../-_").is_ok());

for rejected in ["Promptforge/web", "promptforge/Web", "promptforge/web@2", "promptforge /web", "promptforge/wéb"] {
    assert!(CapabilityId::parse(rejected).is_err());
}
# Ok::<(), Box<dyn std::error::Error>>(())
````

# How a rejection is classified

Every rejection carries one of three kinds. [`CapabilityId::parse`] reports them as a [`CapabilityIdErrorKind`], and [`GlobalName::parse`] reports them as a [`GlobalNameErrorKind`]. Both enums have the same three variants, one per rule of the grammar.

- **Segment count.** The text does not split on `/` into an allowed number of segments. This is [`CapabilityIdErrorKind::SegmentCount`] or [`GlobalNameErrorKind::SegmentCount`].
- **Empty segment.** The count is allowed, but one segment has zero length. This is [`CapabilityIdErrorKind::Empty`] or [`GlobalNameErrorKind::Empty`].
- **Disallowed byte.** A segment holds a byte outside `a` to `z`, `0` to `9`, `-`, `_`, and `.`. This is [`CapabilityIdErrorKind::Control`] or [`GlobalNameErrorKind::Control`]. Despite the name, this kind covers every disallowed byte, not only control characters.

The checks run in a fixed order, and the first failure wins.

1. **Count first.** The segments are counted before any segment is examined. The empty string splits into one empty segment, so it is a segment count error, not an empty segment error. A four-segment input that also has an empty segment is still a segment count error.
2. **Segments left to right.** The segments are then checked in order. Each one is checked for emptiness first and then byte by byte, from left to right.
3. **Exactly 2 for a capability id.** [`CapabilityId::parse`] runs the first two checks with the shared 2-or-3 count, and only text that passes them is checked for exactly 2 segments. So a three-segment input is a segment count error only when all three segments are valid. `promptforge//web` is an empty segment error, and `Promptforge/web/fetch` is a disallowed byte error. Both count checks give the same message.

So `/Web` is an empty segment error, because its empty first segment is checked before the uppercase `W`. `Promptforge/` is a disallowed byte error, because the uppercase `P` in the first segment is found before the empty second segment.

````
use promptforge::capabilities::{CapabilityId, CapabilityIdErrorKind, GlobalName, GlobalNameErrorKind};

let kind = |text: &str| CapabilityId::parse(text).err().map(|error| error.kind());
assert!(kind("promptforge/web").is_none());
assert!(matches!(kind(""), Some(CapabilityIdErrorKind::SegmentCount)));
assert!(matches!(kind("promptforge"), Some(CapabilityIdErrorKind::SegmentCount)));
assert!(matches!(kind("promptforge/web/fetch"), Some(CapabilityIdErrorKind::SegmentCount)));
assert!(matches!(kind("a//b/c"), Some(CapabilityIdErrorKind::SegmentCount)));
assert!(matches!(kind("promptforge/"), Some(CapabilityIdErrorKind::Empty)));
assert!(matches!(kind("/Web"), Some(CapabilityIdErrorKind::Empty)));
assert!(matches!(kind("promptforge//web"), Some(CapabilityIdErrorKind::Empty)));
assert!(matches!(kind("Promptforge/"), Some(CapabilityIdErrorKind::Control)));
assert!(matches!(kind("promptforge/web@2"), Some(CapabilityIdErrorKind::Control)));

let name_kind = |text: &str| GlobalName::parse(text).err().map(|error| error.kind());
assert!(name_kind("promptforge/web/fetch").is_none());
assert!(matches!(name_kind("promptforge/web/fetch/extra"), Some(GlobalNameErrorKind::SegmentCount)));
assert!(matches!(name_kind("promptforge//web"), Some(GlobalNameErrorKind::Empty)));
assert!(matches!(name_kind("promptforge/w\tb"), Some(GlobalNameErrorKind::Control)));

let tab = GlobalName::parse("promptforge/w\tb").err().ok_or("a tab is rejected")?;
assert_eq!(tab.to_string(), "invalid global name: segments must not contain a control character");
let tab = CapabilityId::parse("promptforge/w\tb").err().ok_or("a tab is rejected")?;
assert_eq!(
    tab.to_string(),
    "invalid capability id: segments may contain only lowercase ASCII letters, digits, '-', '_', '.'",
);
# Ok::<(), Box<dyn std::error::Error>>(())
````

The last two checks show the one difference between the two error types' messages. A [`GlobalNameError`] gives a control byte its own reason, and a [`CapabilityIdError`] gives it the same reason as any other disallowed byte. The kind is the same either way. The Reference lists every message under [`CapabilityIdError`] and [`GlobalNameError`].

# Checking tool membership

Before a tool goes into a run's catalog, the host can check that it belongs to the capability that offered it. The host enforces containment when it assembles the run's catalog. [`ToolCatalog::new`](crate::tools::ToolCatalog::new) does not check it, so a host that wants every catalog entry to belong to an activated capability calls [`CapabilityId::contains`] itself before building the catalog.

[`CapabilityId::contains`] drops the tool id's last segment and compares the rest with the capability id as a whole. It compares identities, not text prefixes. So `promptforge/web` contains `promptforge/web/fetch`, but not `promptforge/web2/fetch`, whose pack merely starts with the same text, and not `promptforge/other/fetch`.

````
use promptforge::capabilities::CapabilityId;
use promptforge::tools::ToolId;

let web = CapabilityId::parse("promptforge/web")?;
let offered = [
    ToolId::parse("promptforge/web/fetch")?,
    ToolId::parse("promptforge/web2/fetch")?,
    ToolId::parse("promptforge/other/fetch")?,
];
let admitted: Vec<&ToolId> = offered.iter().filter(|tool| web.contains(tool)).collect();
assert_eq!(admitted.len(), 1);
assert_eq!(admitted[0].to_string(), "promptforge/web/fetch");
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Names of either kind

[`GlobalName::parse`] validates text against the shared grammar without deciding whether it names a capability or a tool. It accepts 2 segments, a capability's `namespace/pack`, or 3 segments, a tool's `namespace/pack/name`. Anything else fails with [`GlobalNameErrorKind::SegmentCount`]. The host also receives one from [`CapabilityDecl::id`](crate::prompt::CapabilityDecl::id), and a declared capability's name always has 2 segments.

A [`GlobalName`] does not record its kind. The caller tells the kind by counting the segments of the text. [`GlobalName::namespace`] and [`GlobalName::pack`] return the first two segments, so on a tool name [`GlobalName::pack`] returns the middle segment. No accessor returns the third segment. There is also no conversion from a [`GlobalName`] to a [`CapabilityId`] or a [`ToolId`](crate::tools::ToolId). To get one, re-parse the name's [`Display`](std::fmt::Display) text with [`CapabilityId::parse`] or [`ToolId::parse`](crate::tools::ToolId::parse). A [`ToolId`](crate::tools::ToolId) then gives the tool name through [`ToolId::name`](crate::tools::ToolId::name).

````
use promptforge::capabilities::{CapabilityId, GlobalName};
use promptforge::tools::ToolId;

let text = "promptforge/web/fetch";
let name = GlobalName::parse(text)?;
assert_eq!(name.namespace(), "promptforge");
assert_eq!(name.pack(), "web");
assert_eq!(text.split('/').count(), 3);

let tool = ToolId::parse(&name.to_string())?;
assert_eq!(tool.name(), "fetch");
assert_eq!(tool.capability(), CapabilityId::parse("promptforge/web")?);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Printing, storing, and collecting

**Printing.** A [`CapabilityId`] or a [`GlobalName`] prints through [`Display`](std::fmt::Display) in its canonical slash-joined form, and that text re-parses to an equal value.

**Storing.** Through serde, a [`CapabilityId`] serializes as one plain string, such as the JSON string `"promptforge/web"`. Deserializing runs [`CapabilityId::parse`] on the string. An invalid string, including a three-segment tool id, is a deserialization error that carries the [`CapabilityIdError`] message. A [`GlobalName`] has no serde support, so store its [`Display`](std::fmt::Display) text instead.

**Collecting.** Both types implement [`Hash`](std::hash::Hash), [`PartialOrd`](std::cmp::PartialOrd), and [`Ord`](std::cmp::Ord) alongside [`Eq`], so they work as keys in a [`HashMap`](std::collections::HashMap), a [`HashSet`](std::collections::HashSet), a [`BTreeMap`](std::collections::BTreeMap), or a [`BTreeSet`](std::collections::BTreeSet). The order compares the segments in turn, and it is case-sensitive.

````
use std::collections::BTreeSet;

use promptforge::capabilities::{CapabilityId, GlobalName};

let web = CapabilityId::parse("promptforge/web")?;
assert_eq!(CapabilityId::parse(&web.to_string())?, web);
let name = GlobalName::parse("promptforge/web/fetch")?;
assert_eq!(GlobalName::parse(&name.to_string())?, name);

let json = serde_json::to_string(&web)?;
assert_eq!(json, "\"promptforge/web\"");
assert_eq!(serde_json::from_str::<CapabilityId>(&json)?, web);
assert!(serde_json::from_str::<CapabilityId>("\"promptforge/web/fetch\"").is_err());

let mut active = BTreeSet::new();
active.insert(web.clone());
active.insert(CapabilityId::parse("org.rustalliance/core")?);
active.insert(CapabilityId::parse("promptforge/web")?);
assert_eq!(active.len(), 2);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Handing preludes to a run

A prelude is a string of Lua source that a capability contributes. It defines tables and functions for prompt authors, such as `greeter.greet(name)`, and those functions reach the capability's own tools through `tools.call`. The engine never learns what a capability does. It knows only the prelude's source and the capability's id, which names the prelude in tracebacks and error messages.

This example hands one prelude to a run whose only section calls it, then shows a second prelude that defines the same global and fails the run.

````
use std::sync::Arc;

use promptforge::capabilities::{CapabilityId, Prelude};
use promptforge::timestamp::Timestamp;
use promptforge::{Environment, Prompt, Run, RunContext, RunErrorKind, RunResult, Step};

let source = concat!(
    "---\n",
    "name: greet\n",
    "description: greets through a prelude\n",
    "promptforge: 0\n",
    "---\n",
    "\n",
    "# Greet\n",
    "\n",
    "## Only\n",
    "\n",
    "```lua\n",
    "return greeter.greet('world')\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greet");
let prompt = Arc::new(parsed?);
let started_at = Timestamp::from_unix_millis(951_782_400_000);

let greeter = Prelude::new(
    CapabilityId::parse("example/greeter")?,
    "greeter = {}\nfunction greeter.greet(name) return 'hello ' .. name end",
);
let env = Environment::new().preludes(vec![greeter.clone()]);
let (ctx, requirements) = env.prepare(&prompt, RunContext::new("greet", 7, started_at));
assert!(requirements.is_satisfied());
let mut run = Run::new(Arc::clone(&prompt), "", ctx);
let Step::Done { result, .. } = run.step() else {
    panic!("the section issues no effect, so the first step ends the run");
};
match result {
    RunResult::Ok(text) => assert_eq!(text, "hello world"),
    other => panic!("the run should succeed: {other:?}"),
}

let clash = Prelude::new(CapabilityId::parse("example/clash")?, "greeter = {}");
let env = Environment::new().preludes(vec![greeter, clash]);
let (ctx, _requirements) = env.prepare(&prompt, RunContext::new("clash", 7, started_at));
let mut run = Run::new(Arc::clone(&prompt), "", ctx);
let Step::Done { result: RunResult::Failure(error), .. } = run.step() else {
    panic!("the collision fails the run on its first step");
};
assert_eq!(error.kind(), RunErrorKind::Lua);
assert!(error.to_string().contains(
    "capability `example/clash`: its prelude defines the global `greeter`, \
     which capability `example/greeter`'s prelude already defines"
));
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what the engine does with the list.

1. **Copy it onto the run.** [`Environment::preludes`](crate::Environment::preludes) stores the list, and [`Environment::prepare`](crate::Environment::prepare) copies it onto the context, the same way it copies the tool catalog.
2. **Install it in every section.** Each section's Lua machine, the fanout arms and spawned task chains included, installs every prelude in list order. That happens after the host globals such as `tools` and `store` exist, and before the prompt's shared library runs, so the shared library can call what the preludes define.
3. **Keep each prelude to itself.** A prelude runs in an environment of its own. It sees the base functions that survive the sandbox, the `string`, `table`, and `math` libraries, `tools`, `store`, `untrusted`, and a read-only view of `var`. It does not see the other preludes. Each global it assigns becomes a global of every section. A table global is sealed at its top level: author code can read the table's own fields but cannot add or replace them, a table stored in one of those fields can still be changed, and `pairs` over the sealed table sees nothing.
4. **Fail early.** A prelude only defines functions. It runs as a plain chunk, not inside a section's coroutine, so a prelude that calls a tool while loading fails. A prelude also fails when one of its globals takes a name already in use: a reserved name (every host global, `ui`, `item`, `argv`, and `prose` included even where a section lacks them, every Lua standard-library global the sandbox keeps, and every Lua keyword, the same list no frontmatter alias may take), any other global the section already holds, a tool or model alias from the prompt's frontmatter, or a global of an earlier prelude. Either failure ends the run with [`RunErrorKind::Lua`](crate::RunErrorKind::Lua) when the first section's Lua machine is set up, before the run issues any effect, and the message names the capability.

# Reference

This part covers every item in the module. [`CapabilityId`] and its error types come first, then [`GlobalName`] and its error types, and last [`Prelude`]. The error types and kind enums are `#[non_exhaustive]`, so a `match` on a kind needs a wildcard arm. Every accessor on this page is `#[must_use]`, including [`CapabilityId::contains`] and both error types' kind methods.

## CapabilityId

[`CapabilityId`] is the stable identity of an installed capability, a validated two-segment `namespace/pack` name. It is also the prefix of every tool id the capability contributes, so `promptforge/web` contributes `promptforge/web/fetch`.

The host gets one in three ways: [`CapabilityId::parse`] on text, [`ToolId::capability`](crate::tools::ToolId::capability) on a parsed [`ToolId`](crate::tools::ToolId), or serde deserialization from a string. It also receives them from the API in [`Requirements::missing_required`](crate::Requirements::missing_required), in [`CapabilityConflict::first`](crate::CapabilityConflict::first) and [`CapabilityConflict::second`](crate::CapabilityConflict::second), and in [`ToolDescriptor::conflicts`](crate::tools::ToolDescriptor::conflicts). A host that describes its own tools passes a [`Vec`] of them to [`ToolDescriptor::with_conflicts`](crate::tools::ToolDescriptor::with_conflicts). The struct is `#[non_exhaustive]` with a private field, so it cannot be built with a struct literal. It has no [`Default`], [`FromStr`](std::str::FromStr), or [`From`] implementation.

[`CapabilityId::parse`] takes one argument.

- `id`, a [`&str`](str), is the capability id text. Pass the exact `namespace/pack` string with no surrounding whitespace and no version pin. It must have exactly 2 `/`-separated, non-empty segments that follow [the naming grammar](#the-naming-grammar).

It returns a [`Result`]. On success it holds the validated [`CapabilityId`], which the host stores, compares, or passes wherever a capability identity is expected. On failure it holds a [`CapabilityIdError`], whose kind is described under [`CapabilityIdErrorKind`]. The text is validated against the shared grammar first, and the exactly-2 check comes last, as [the check order](#how-a-rejection-is-classified) describes.

The other methods take `&self` and cannot fail.

- [`CapabilityId::namespace`] returns the first segment as a [`&str`](str) borrowed from the id, such as `promptforge` or a reverse-DNS name like `org.rustalliance`.
- [`CapabilityId::pack`] returns the second segment as a [`&str`](str) borrowed from the id, such as `web` in `promptforge/web`.
- [`CapabilityId::contains`] takes `tool`, a reference to any parsed [`ToolId`](crate::tools::ToolId), and returns a [`bool`]. It is `true` when dropping the tool id's last segment yields exactly this capability id, and `false` otherwise. [Checking tool membership](#checking-tool-membership) shows it in use.

Caller-relevant traits:

- [`Display`](std::fmt::Display) writes the canonical `namespace/pack` string, which re-parses to an equal id.
- serde serializes the id as a single string in `namespace/pack` form, such as the JSON `"promptforge/web"`. Deserializing validates the string with [`CapabilityId::parse`], and an invalid string is a deserialization error carrying the [`CapabilityIdError`] message.
- [`Hash`](std::hash::Hash), [`PartialOrd`](std::cmp::PartialOrd), and [`Ord`](std::cmp::Ord) are derived, so the id works as a collection key.

## CapabilityIdError

[`CapabilityIdError`] is the reason a string could not be parsed as a [`CapabilityId`]. [`CapabilityId::parse`] returns it, and its message also becomes the message of the serde error when deserializing a [`CapabilityId`] fails. Hosts never build one. It is `#[non_exhaustive]` with private fields and has no public constructor.

- [`CapabilityIdError::kind`] takes `&self`, cannot fail, and returns the [`CapabilityIdErrorKind`] to branch on.

[`CapabilityIdError`] implements [`Display`](std::fmt::Display) as `invalid capability id: ` followed by one fixed reason per kind. The reason text is reachable only through [`Display`](std::fmt::Display).

- For [`CapabilityIdErrorKind::SegmentCount`]: `a capability id must have exactly 2 segments (namespace/pack)`.
- For [`CapabilityIdErrorKind::Empty`]: `segments must not be empty`.
- For [`CapabilityIdErrorKind::Control`]: `segments may contain only lowercase ASCII letters, digits, '-', '_', '.'`. Control bytes get this reason too.

It implements [`std::error::Error`], so `?` converts it into a [`Box`] of `dyn Error`.

## CapabilityIdErrorKind

[`CapabilityIdErrorKind`] is the matchable classification of a [`CapabilityIdError`], returned by [`CapabilityIdError::kind`]. Branch on it instead of the message text. It is [`Copy`] and `#[non_exhaustive]`, and hosts never build one.

- [`CapabilityIdErrorKind::SegmentCount`]: the text does not split on `/` into exactly 2 segments. The host sees it for 1 segment such as `promptforge`, for the empty string, for 3 valid segments such as `promptforge/web/fetch`, and for 4 or more, even when one of them is empty. A 3-segment input is usually a tool id passed where a capability id belongs. Pass a two-segment id. For a tool id, take its capability with [`ToolId::capability`](crate::tools::ToolId::capability) instead of re-parsing.
- [`CapabilityIdErrorKind::Empty`]: the text has 2 or 3 segments and one has zero length, from a leading, trailing, or doubled `/` such as `/web`, `promptforge/`, or `promptforge//web`. Supply both the namespace and the pack, with a single `/` between them.
- [`CapabilityIdErrorKind::Control`]: a segment contains a byte outside `a` to `z`, `0` to `9`, `-`, `_`, and `.`. The host sees it for uppercase letters such as `Promptforge/web`, an `@` pin such as `promptforge/web@2`, whitespace, other punctuation, a control byte, or a non-ASCII character. Lowercase the name, drop any `@` suffix, and remove the other disallowed characters.

## GlobalName

[`GlobalName`] is a validated name in the naming grammar shared by capability and tool ids. Two segments name a capability, `namespace/pack`, and three name a tool, `namespace/pack/name`. It validates text as either kind without deciding which.

The host gets one from [`GlobalName::parse`], or receives one from [`CapabilityDecl::id`](crate::prompt::CapabilityDecl::id) for a capability a prompt declares, which always has 2 segments. The segment list is private, so [`GlobalName::parse`] is the only constructor. There is no [`Default`], [`FromStr`](std::str::FromStr), [`From`], or serde implementation, and no conversion to [`CapabilityId`] or [`ToolId`](crate::tools::ToolId). [Names of either kind](#names-of-either-kind) shows how to re-parse one into those types.

[`GlobalName::parse`] takes one argument.

- `s`, a [`&str`](str), is the name text. Pass `namespace/pack` or `namespace/pack/name` exactly, with no whitespace and no version pin. It must have 2 or 3 `/`-separated, non-empty segments that follow [the naming grammar](#the-naming-grammar).

It returns a [`Result`]. On success it holds the validated [`GlobalName`]. The host tells its kind by counting the segments of the original text. On failure it holds a [`GlobalNameError`], whose kind is described under [`GlobalNameErrorKind`]. The checks run in [the fixed order](#how-a-rejection-is-classified), and the first failure is returned.

The other methods take `&self` and cannot fail, because every [`GlobalName`] has at least 2 segments.

- [`GlobalName::namespace`] returns the first segment as a [`&str`](str), such as `promptforge` or a reverse-DNS name.
- [`GlobalName::pack`] returns the second segment as a [`&str`](str). For a three-segment tool name that is the middle segment, `web` in `promptforge/web/fetch`. No method returns the third segment. [`ToolId::name`](crate::tools::ToolId::name) provides it for tool ids.

Caller-relevant traits:

- [`Display`](std::fmt::Display) writes the segments joined with `/`, such as `promptforge/web/fetch`, and the text re-parses to an equal [`GlobalName`].
- [`Hash`](std::hash::Hash), [`PartialOrd`](std::cmp::PartialOrd), and [`Ord`](std::cmp::Ord) are derived over the segment list, so the name works as a collection key. Comparison is case-sensitive.

## GlobalNameError

[`GlobalNameError`] is the reason a string could not be parsed as a [`GlobalName`]. [`GlobalName::parse`] returns it, and hosts never build one. It is `#[non_exhaustive]` with private fields and has no public constructor.

- [`GlobalNameError::kind`] takes `&self`, cannot fail, and returns the [`GlobalNameErrorKind`] to branch on.

[`GlobalNameError`] implements [`Display`](std::fmt::Display) as `invalid global name: ` followed by one of four fixed reasons. The reason text is reachable only through [`Display`](std::fmt::Display).

- For [`GlobalNameErrorKind::SegmentCount`]: `must have exactly 2 segments (namespace/pack) or 3 (namespace/pack/name)`.
- For [`GlobalNameErrorKind::Empty`]: `segments must not be empty`.
- For [`GlobalNameErrorKind::Control`] on a control byte, which is a byte below `0x20` or the byte `0x7f`: `segments must not contain a control character`.
- For [`GlobalNameErrorKind::Control`] on any other disallowed byte: `segments may contain only lowercase ASCII letters, digits, '-', '_', '.'`.

It implements [`std::error::Error`].

## GlobalNameErrorKind

[`GlobalNameErrorKind`] is the matchable classification of a [`GlobalNameError`], returned by [`GlobalNameError::kind`]. Branch on it instead of the message text. It is [`Copy`] and `#[non_exhaustive]`, and hosts never build one.

- [`GlobalNameErrorKind::SegmentCount`]: the text does not split on `/` into 2 or 3 segments. The host sees it for 1 segment such as `promptforge`, for the empty string, and for 4 or more such as `promptforge/web/fetch/extra`. Supply `namespace/pack` for a capability or `namespace/pack/name` for a tool.
- [`GlobalNameErrorKind::Empty`]: the count is 2 or 3, but a leading, trailing, or doubled `/` leaves a segment empty, as in `/web`, `promptforge/`, or `promptforge//web`. Remove the stray separator or fill in the missing segment.
- [`GlobalNameErrorKind::Control`]: a segment contains a byte outside the allowed set. The host sees it for a control byte such as a tab, newline, or DEL, uppercase letters such as `Promptforge/web`, an `@` pin such as `promptforge/web@2`, spaces or other punctuation, and non-ASCII characters such as `promptforge/wéb`. The [`Display`](std::fmt::Display) text tells a control byte apart from the other cases. Use only `a` to `z`, `0` to `9`, `-`, `_`, and `.`, and drop any version suffix.

## Prelude

[`Prelude`] is the Lua source one activated capability contributes to every section of a run, paired with that capability's [`CapabilityId`]. The host builds one per capability that has a prelude and hands the list to [`Environment::preludes`](crate::Environment::preludes). [Handing preludes to a run](#handing-preludes-to-a-run) shows the whole path. Its fields are private, so build it with [`Prelude::new`].

[`Prelude::new`] takes two arguments, cannot fail, and returns the [`Prelude`].

- `capability`, a [`CapabilityId`], is the capability that contributes the source. It names the prelude in tracebacks, as the chunk name `@capability:<id>`, and in every error message about the prelude.
- `source`, anything that converts into a [`String`], is the Lua source. Nothing checks it here. The engine compiles it as text in every section, so a syntax error or a colliding global surfaces when the run sets up its first section.

The other methods take `&self` and cannot fail.

- [`Prelude::capability`] returns a reference to the contributing [`CapabilityId`].
- [`Prelude::source`] returns the Lua source as a [`&str`](str).

Caller-relevant traits: [`Clone`], [`Debug`](std::fmt::Debug), [`PartialEq`], and [`Eq`] are derived, so two preludes are equal when both their capability ids and their sources are equal.
