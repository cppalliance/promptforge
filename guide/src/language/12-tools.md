# Tools

Tools let a prompt reach past the model's own text: in the middle of a conversation the model can search the web or fetch a page, your Lua code can call the same tools directly, and any Lua function can become a tool the model calls. This chapter shows how to declare the capabilities a prompt needs, bind their tools under short aliases, choose which tools the model sees in each section, call tools from Lua, read trusted and untrusted output, handle failures, count calls, and write local tools, so your prompts can research, check their work, and act.

## Tools at a glance

Every tool comes from the host. The host registers capabilities, each supplying a set of tools under an id such as `promptforge/web`, and a prompt declares the capabilities it uses and binds the tools it wants from them. The smallest tool prompt declares one capability, binds one tool, and calls it from Lua:

````markdown
---
name: page-fetcher
description: Fetches the page named in the argument string
promptforge: 0
capabilities:
  - promptforge/web
tools:
  fetch: promptforge/web/fetch
---

# Page fetcher

## Fetch

```lua
return tools.call('fetch', { url = args })
```
````

`capabilities:` lists the capabilities the prompt uses, here `promptforge/web`, the first-party capability that supplies the tools `promptforge/web/fetch` and `promptforge/web/search`. Each declared capability is activated at [prepare](04-how-a-prompt-runs.md#capability-activation), before any Lua runs, and the run's tool catalog, the set of tools the prompt can bind, is built from exactly the declared capabilities. A host tool reaches a run no other way.

`tools:` binds tool slots. Each entry is written `alias: namespace/pack/name`, a prompt-local alias mapped to one exact tool path, so `fetch` here is an alias for `promptforge/web/fetch`. Each alias then exists as a Lua global and as a name `tools.call` accepts. A prompt without a `tools:` key has no tool slots.

`tools.call(alias, args)` calls a bound tool from Lua: the alias names the tool, the Lua table becomes the tool's JSON arguments, and the call returns the tool's result. Here the table's `url` field is the run's argument string, [`args`](06-arguments.md#input-basics), and the section returns the tool's result as the run result.

Binding a tool does not show it to the model. A bound tool stays out of the model's view until `tools.always` or `tools.add` names it, which puts it in the section's scope. This prompt lets the model use both web tools while it answers:

````markdown
---
name: researcher
description: Answers a question from web sources
promptforge: 0
capabilities:
  - promptforge/web
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
models:
  writer: {}
---

# Researcher

## Research

```lua
models.use('writer')
tools.add({'search', 'fetch'})
local msgs = messages.new():user('Answer from web sources: ' .. args)
models.loop(msgs)
return msgs[#msgs].content
```
````

`models.use` selects the section's [model role](10-models.md#choosing-a-sections-model), and `models.loop` runs rounds on a message list until the model replies with text, as [Conversations](11-conversations.md#a-first-conversation) shows. `tools.add` puts `search` and `fetch` in this section's scope, and on every round `models.loop` offers the model every tool in scope. The host supplies each tool, the prompt only names it by path, and the model receives it as plain data: a name, a description, and a JSON Schema for its arguments.

That name is the alias. The alias is the only name the model sees or uses for a tool: the model never sees the tool path, and every call still runs the exact tool the path names.

When the model calls a tool it was offered, the loop runs the call under the model's call id, appends the assistant record holding the call and one tool record per result, linked by `tool_call_id` as [Conversations](11-conversations.md#what-the-loop-appends) describes, and asks the model again until it answers with text.

You can also make a tool out of a Lua function. `tools.add_local(alias, description, params, handler)` in a section's `lua` block registers a local tool that the model can call in that section beside the bound tools, and that your Lua code can call with `tools.call`. The engine answers these calls itself, without the host:

````lua
tools.add_local('grab', 'Grab a value', { value = 'string' }, function(a)
  return 'got ' .. a.value
end)
local out = tools.call('grab', { value = 'hi' })
````

`out` holds `got hi`.

Every tool operation lives in one Lua table, `tools`, the way model operations live in `models`. Besides `tools.add`, `tools.call`, and `tools.add_local`, the table holds `tools.always`, one member for letting the model start tasks, named under [Advertising tools to the model](#advertising-tools-to-the-model), and `tools.calls` once the section has made its first tool call.

Everything about tools lives in the prompt file. There are no command-line flags or config files for them, and credentials and server settings come from the host. Without a `capabilities:` key a prompt has no capabilities, without a `tools:` key it has no tool slots, and a section offers the model no tools until the prompt puts some in scope.

## Declaring capabilities

`capabilities:` is a YAML list in the [frontmatter](02-file-structure.md#frontmatter-rules-and-errors), with one declaration per entry, kept in the order written. Leaving the key out means the prompt uses no capabilities. Each entry is a plain capability id or a map:

````yaml
capabilities:
  - promptforge/web
  - ref: io.github.corp/mcp
    optional: true
````

A plain string entry, such as `- promptforge/web`, declares a required capability with no config. When the host does not have a required capability, or the capability fails to activate, prepare refuses the run before it starts with run error kind [`RequirementsUnmet`](17-limits-and-errors.md#how-a-failed-run-is-classified), and the [requirements notice](04-how-a-prompt-runs.md#when-a-run-cannot-start) names each missing capability:

````text
the environment cannot satisfy this prompt:
- missing required capability: promptforge/web
````

The map form has three keys, and plain and map entries mix freely in one list:

| Key | Value | Default |
|---|---|---|
| `ref` | the capability id, required | none |
| `optional` | a boolean | `false` |
| `config` | any YAML value | no config |

With `optional: true`, a capability the host lacks, or one that fails to activate, is skipped at prepare with a log line naming it, and the run goes ahead. The second entry above is optional, so a host without `io.github.corp/mcp` still runs the prompt.

`config` accepts any YAML value without a shape check. A capability receives only the run's filesystem and cancel signal when it activates, so no shipped capability reads `config`. Credentials, server lists, and similar settings always come from the host, never from the prompt.

### How declarations are matched

Prepare activates capabilities in the order declared, and their tools join the run's tool catalog in that order: by declaration first, then in each capability's own order.

A declared id matches the one capability the host installed under exactly that id. The capabilities a prompt can use are exactly the ones the host has registered, so an id the host never registered matches nothing: a required entry is reported missing, and an optional one is skipped.

A capability works against the run's own filesystem, so the files its tools read and write are the same files [the store](09-the-store.md#what-the-store-is) sees.

A capability can name another capability as a conflict, for example when each provides a different filesystem. A prompt that declares both is refused before the run starts with `RequirementsUnmet`: neither activates, and the notice lists the pair, the earlier-declared capability first. Declare only one of the two.

````text
- conflicting capabilities: {first} and {second} cannot be activated together; declare one or the other
````

### Entry errors

A malformed entry fails the parse with parse error kind [`Frontmatter`](17-limits-and-errors.md#parse-error-kinds), located at the entry's line and column:

- A map without `ref` fails with `` missing field `ref` ``.
- A key written twice fails with `` duplicate field `{key}` ``, where `{key}` is `ref`, `optional`, or `config`.
- Any other key in the map fails with `` unknown field `{key}`, expected one of `ref`, `optional`, `config` ``.
- An entry that is neither a string nor a map fails with `` invalid type: {found}, expected a capability id string or a map with `ref`, `optional`, and `config` ``, where `{found}` names the kind of value written, such as an integer or a sequence.

## Capability ids and tool paths

Capability ids and tool paths share one grammar of `/`-separated segments, and the segment count tells them apart: two segments name a capability, and three name a tool. A `capabilities:` entry with any other count fails the parse with an error that quotes the id.

````text
capability id = segment "/" segment
tool path     = segment "/" segment "/" segment
segment       = one or more characters, each one of a-z 0-9 - _ .
````

A capability id is written `namespace/pack`, such as `promptforge/web`, `io.github.corp/mcp`, or `acme/web-search`. The first segment is the namespace: a reverse-DNS name such as `org.rustalliance`, or the first-party `promptforge`. A dotted namespace is still one segment.

A tool path is written `namespace/pack/name`, such as `promptforge/web/fetch`, and every `tools:` value is a tool path. The last segment is the tool's short name, `fetch` here. Dropping it always gives the id of the capability that supplies the tool, so `promptforge/web/fetch` comes from `promptforge/web`:

| Tool path | Short name | Supplied by |
|---|---|---|
| `promptforge/web/fetch` | `fetch` | `promptforge/web` |
| `promptforge/web/search` | `search` | `promptforge/web` |
| `org.rustalliance/core/search` | `search` | `org.rustalliance/core` |
| `org.rustalliance/my-pack/v1_2.tool` | `v1_2.tool` | `org.rustalliance/my-pack` |

A capability supplies only tools whose path is its own id plus one name segment, compared by whole segments: `promptforge/web` supplies `promptforge/web/fetch` but never `promptforge/other/fetch`, and `promptforge/web2/fetch` belongs to `promptforge/web2`, not to `promptforge/web`. The host admits a contributed tool only when its path sits under the contributing capability's id, so every tool a prompt can bind has this shape.

### Segment rules

- Every segment has at least one character, and each character is a lowercase ASCII letter `a` to `z`, a digit `0` to `9`, `-`, `_`, or `.`. Tool path segments follow the same rules as capability id segments.
- A segment or a whole name has no length limit and no rule about its first or last character, so a segment may start or end with `-`, `_`, `.`, or a digit.
- Names are kept exactly as written, with no case folding, trimming, or other normalizing, and compare byte for byte. Write every id and path in lowercase; `promptforge/web` is the only spelling of that capability.
- `-`, `_`, and `.` are not interchangeable. The host looks ids up exactly, so `acme/web-search`, `acme/web_search`, and `acme/web.search` are three different capabilities.
- A capability id has no version part. It names the one capability the host installed under that id.
- By convention an organization's own capabilities live under a reverse-DNS namespace such as `org.rustalliance` or `io.github.corp`, and `promptforge` is the first-party namespace. The parser checks neither convention.

A capability id or tool path prints as its segments joined with `/`, and that text reads back as the same name, so run reports and error messages show it exactly as written, the missing-capability line of the requirements notice included.

## Name errors

A capability id or tool path that breaks the grammar fails the parse with parse error kind `Frontmatter`, located at the offending entry's line and column. The message quotes the text as written and names the broken rule.

A capability id under `capabilities:` gets one of two messages. Three otherwise valid segments give the first; every other failure gives the second, with a reason from the table below:

````text
invalid capability id `{text}`: a capability id has exactly 2 segments (namespace/pack)
invalid capability id `{text}`: invalid global name: {reason}
````

A `tools:` value that is a malformed string gets the first message below, and a value that is not a string at all, such as a map or a number, gets the second:

````text
invalid exact tool path `{text}`: invalid tool id: {reason}
invalid type: {found}, expected an exact tool path string
````

The reason depends on what is wrong and on which kind of name it is:

| Problem | Reason in a capability id | Reason in a tool path |
|---|---|---|
| One segment, four or more, or empty text | `must have exactly 2 segments (namespace/pack) or 3 (namespace/pack/name)` | `a tool id must have exactly 3 segments (namespace/pack/name)` |
| Valid segments, but three in a capability id or two in a tool path | the first capability message above | `a tool id must have exactly 3 segments (namespace/pack/name)` |
| An empty segment, left by a doubled, leading, or trailing `/` | `segments must not be empty` | `segments must not be empty` |
| A control character: a tab, a newline, any byte below 0x20, or DEL | `segments must not contain a control character` | `segments may contain only lowercase ASCII letters, digits, '-', '_', '.'` |
| An uppercase letter, a space, a non-ASCII character, or punctuation other than `-`, `_`, and `.` | `segments may contain only lowercase ASCII letters, digits, '-', '_', '.'` | `segments may contain only lowercase ASCII letters, digits, '-', '_', '.'` |

Each bad name produces one message, for the first check it fails. The overall segment count comes first, then each segment from left to right, with an empty segment reported before its characters and the first bad character deciding between the control-character and character-set reasons. The exact count for the position comes last: two for a capability id, three for a tool path.

## Tool slots and Tool objects

Each `tools:` entry declares a tool slot: an alias, which follows the prompt's [name grammar for aliases](02-file-structure.md#names-for-aliases-roles-and-args), bound to one tool path. The first two segments of the path name the declared capability that supplies the tool.

Prepare [fills each slot](04-how-a-prompt-runs.md#filling-tool-slots-and-model-roles) by exact match of its tool path against the run's tool catalog, which holds the activated capabilities' tools in declaration order. A slot whose path matches becomes a bound tool slot, and it stays bound to that same tool for the whole run. Slots are bound before any Lua runs, so Lua only chooses which bound slots the model sees, and scoping an alias that is not bound is an error.

Every capability named by a slot's first two segments belongs in `capabilities:`. If that capability contributed no tools to the catalog, prepare refuses the run with `RequirementsUnmet` and the line `- missing required capability: {id}`, listed once however many slots name it. This holds even for a capability declared `optional: true`. So `fetch: promptforge/web/fetch` needs `promptforge/web` to have contributed tools, and when it contributed none, the slot is reported under `promptforge/web`.

In all, a required capability is reported missing, and the run refused before it starts, in three cases: the host does not have it, it fails to activate, or a slot names it but it contributed no tools. Capability and slot problems at prepare are always reported as `RequirementsUnmet`.

Each bound slot records its alias, the tool's description, and the tool path, whose last segment is the tool's short name. The catalog finds a tool only by its full tool path, and only aliases declared under `tools:` are bound. Every tool in the catalog has a short name, a description, and a JSON Schema for its arguments. The description comes from the host, the model reads it when deciding whether to call the tool, and the engine sets no length or sentence rule on it.

Two aliases can name the same tool path, and both call that one tool:

````yaml
tools:
  fetch: promptforge/web/fetch
  getter: promptforge/web/fetch
````

A `tools.call` to an alias that is not bound fails with an error that lists the bound aliases.

### Alias globals

Each bound slot is also a bare Lua global named by its alias, holding a Tool object:

````markdown
---
name: tool-inspector
description: Reads the fields of a Tool object
promptforge: 0
capabilities:
  - promptforge/web
tools:
  page: promptforge/web/fetch
---

# Tool inspector

## Inspect

```lua
return page.name .. ' ' .. page.wire_name .. ' ' .. tostring(page.untrusted) .. ' ' .. type(page.parameters)
```
````

The section returns:

````text
page fetch false table
````

A Tool object has five fields:

| Field | Value |
|---|---|
| `name` | the alias the slot is bound under |
| `description` | the tool's catalog description |
| `parameters` | a table, always empty |
| `wire_name` | the last segment of the tool path |
| `untrusted` | a boolean, always `false` |

`.name` is the prompt-local alias, the same way `.name` on a [model handle](10-models.md#handle-fields) is its role label. `.description` is the tool's own catalog description, never an override passed to `tools.always` or `tools.add`. `.parameters` is always an empty table, so a tool's real argument schema is not readable from Lua. `.wire_name` is the last segment of the tool path, `fetch` for `promptforge/web/fetch`, whatever alias the prompt chose. `.untrusted` is always `false`, and it does not report whether the tool's output is untrusted.

A Tool object is frozen. Assigning any field, existing or new, raises an error naming the field:

````text
Tool objects are frozen: cannot assign field "{key}"
````

A Tool object has fields and no methods. Every operation goes through a `tools.*` function that takes the Tool object or the alias as its first argument.

The same `tools` table is present in every section VM, the H1 body and shared code included. The alias globals are installed after the [shared library](03-blocks-and-prose.md#how-the-shared-library-loads) replays, so top-level `lua shared` code sees them as nil, and a declared alias wins over a shared global of the same name. A shared function sees them when it runs:

````markdown
---
name: shared-helper
description: Uses an alias global from a shared function
promptforge: 0
capabilities:
  - promptforge/web
tools:
  page: promptforge/web/fetch
---

# Shared helper

```lua shared
function tool_label(tool)
  return tool.name .. ' (' .. tool.wire_name .. ')'
end
```

## Show

```lua
return tool_label(page)
```
````

The section returns `page (fetch)`. Alias globals exist everywhere Lua runs: in shared functions, walked sections, [`call` targets](08-jump-and-call.md#called-chains), and each fanout [arm](14-fanout.md#inside-an-arm), which all see the same Tool objects.

## Advertising tools to the model

A section's scope is the set of tools the model is offered in that section. Two calls build it: `tools.always` offers a tool in every section, and `tools.add` offers one in the current section only.

````markdown
---
name: two-step-research
description: Finds sources in one section and writes in the next
promptforge: 0
capabilities:
  - promptforge/web
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
models:
  writer: {}
---

# Two-step research

```lua
models.default('writer')
tools.always('search')
```

## Find sources

```lua
tools.add('fetch')
local msgs = messages.new():user('Find and read sources about ' .. args)
models.loop(msgs)
var.notes = msgs[#msgs].content
```

## Write

```lua
local msgs = messages.new():user('Write a short summary of these notes: ' .. var.notes)
models.loop(msgs)
return msgs[#msgs].content
```
````

`tools.always(alias)` offers a bound tool in every section of the run. It is usually called from the H1 body, which runs in [the H1 pass](04-how-a-prompt-runs.md#the-h1-pass), but it works from any section: it records the alias in one prompt-wide list that every later section sees. Pass it the alias as a string.

`tools.add(...)` offers bound tools in the current section only. That section's rounds include them, and other sections are unaffected. Here `Find sources` offers `search` and `fetch`, and `Write` offers only `search`.

`tools.add` takes a single alias as a string or as a Tool object, such as the alias's own global. It also takes an array of alias strings, Tool objects, or a mix, to scope several at once:

````lua
tools.add('search')
tools.add(fetch)
tools.add({'search', fetch})
````

`tools.add()` with no arguments, or with an empty array, does nothing.

### Scope order

The run's tool set is exactly the bound slots plus the `tools.always` aliases, each list kept in the order declared. A section's bound tools are offered in a fixed order: the prompt-wide `tools.always` aliases first, then the section's `tools.add` aliases in the order first added, each alias once. Adding an alias twice, or adding one already offered through `tools.always`, creates no duplicate and keeps the order of first addition. With `tools.always('search')` in effect, `tools.add({'fetch', 'search'})` gives the scope `search`, `fetch`.

Calling `tools.always` again for the same alias is harmless and records it once, which makes it safe in `lua shared` code, which replays in every section. `tools.add` also works in the shared library's top-level code, because the `tools` table exists before the library replays. The alias globals do not exist yet at that point, so name aliases there with strings.

### Description overrides

The model sees each bound tool with its catalog description and parameter JSON Schema, exactly as the host declared them, since the `tools:` entry is only a path. You can replace the description the model sees with your own text:

````lua
tools.always('search', 'Search the web for recent, reputable sources.')
tools.add('fetch', 'Fetch one page. Use only addresses found by search.')
````

`tools.always(alias, description)` sets the description for the whole run. Calling `tools.always(alias)` without the second argument keeps the current description, and a repeat call that passes a description updates it.

`tools.add(alias, description)` sets it for the current section only. A later override for the same alias replaces the earlier one, and the override applies even when the alias is already offered through `tools.always`. The array form takes no description.

The precedence is fixed: a `tools.add` override wins over a `tools.always` override, which wins over the tool's catalog description. With no override set, the model sees the catalog description. The description argument is the only way to change what the model sees, since Tool object fields are read-only, and `.description` keeps showing the catalog text.

### What each round offers

The tools in scope go out with every round of `models.loop` in the section. The model's tool list puts the bound tools first, in scope order, followed by every local tool in the order registered.

The scope is rebuilt for each round from the current bound slots and local tools, so the offered tools can change between model calls: a `tools.add` or `tools.add_local` call reaches the next round. In a section with `fetch` bound and nothing in scope yet:

````lua
local msgs = messages.new():user('What does the example.com home page say?')
models.loop(msgs)
tools.add('fetch')
msgs:user('Fetch https://example.com and check your answer.')
models.loop(msgs)
````

The first `models.loop` offers no tools, and the second offers `fetch`.

Besides the `tools.always` list it shares with the run, a section's scope holds three things of its own: the aliases it added, its description overrides, and the list set by `tools.allow_tasks`, a `tools` member that lets the model start tasks through the task built-ins, which [Tasks](15-tasks.md#letting-the-model-start-tasks) teaches. A section with no `tools.always` aliases, no added aliases, no local tools, and no such list offers the model no tools, and its rounds go out with no tool list at all.

The model can call only the aliases offered in the current round. A model call to any other name, whether a bound alias left out of scope or a name the model invents, fails with [error kind](05-lua-environment.md#catching-and-inspecting-errors) `out_of_scope_tool`, which names the tool and lists the aliases in scope.

### Scope errors

Every alias given to `tools.always`, `tools.add`, or `tools.add_local` follows the alias rule for `tools:` keys: 1 to 64 ASCII characters, a letter first, then letters, digits, `_`, or `-`. Each call checks this first and otherwise raises:

````text
invalid alias "{alias}": expected [A-Za-z][A-Za-z0-9_-]{0,63}
````

`tools.always` and `tools.add` accept only aliases that are bound tool slots, and name the alias when it is not one:

````text
tools.always alias "{alias}" is not a bound tool slot
tools.add alias "{alias}" is not a bound tool slot
````

Uncaught, these fail the run wherever the call is made. A slot can stay unbound even though prepare reported nothing: when its capability contributed tools but not the one the path names, the slot stays unbound without a report. Offering or calling that alias then fails at run time with an error naming it, such as `tools.add alias "search" is not a bound tool slot`.

`tools.add` is all-or-nothing: it checks every entry before it records any, so one bad alias in an array scopes none of them. The error can be caught with `pcall`. After a caught error nothing was recorded and the tool's description is unchanged, and a later valid `tools.add` still takes effect.

`tools.add` argument errors name the rule broken:

- `tools.add override must be a string, got {type}` for a description that is not a string.
- `tools.add takes one alias plus an optional override, got extra {type}` for a third argument.
- `tools.add expects strings, Tool objects, or arrays of either, got {type}` for an alias or array element that is neither a string nor a Tool object.
- `tools.add array form takes no override` for a description passed with the array form.

Every tool schema is checked before it reaches the model. A host tool whose parameter schema is not a JSON object fails the run when a section offers it, with run error kind [`Binding`](17-limits-and-errors.md#how-a-failed-run-is-classified) and a message naming the alias:

````text
model-facing schema build failure for tool alias "{alias}"
````

## Calling tools from Lua

`tools.call(alias, args)` calls a bound tool from your Lua code and returns only the tool's final output. Name the tool by its alias string, used exactly as written, or by a Tool object such as the alias's global, which stands for the alias it was bound under:

````lua
local a = tools.call('fetch', { url = 'https://example.com' })
local b = tools.call(fetch, { url = 'https://example.com' })
````

Both lines call the same tool. A call your Lua code makes this way is a script call, and a call the model makes inside `models.loop` is a model tool call. Both reach the tool the same way; a model tool call also includes the model's call id, which a script call lacks.

`tools.call` is a [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise): the block pauses while the host runs the tool and resumes with the result. Only the calling [chain](04-how-a-prompt-runs.md#the-section-walk) waits, and the rest of the run goes on. A bound tool is called the same way whatever the host runs behind it, its own code or a gateway: each call reaches the host as host work naming the tool path.

A script call can reach any tool bound in the run, even one outside the section's scope. The scope only limits what the model is offered.

A section's first tool call, either a script call to a bound or local tool or a model tool call inside `models.loop`, is when `tools.calls` appears, and also when [`sys.model`](10-models.md#the-bound-model-in-sysmodel) becomes readable if the section has a model. Each call is also recorded in the run's events with a succeeded or failed event, which [Task Events](16-task-events.md#tool-call-events) shows how to read.

### Arguments

The second argument is a plain Lua table that converts to a JSON object, and the tool receives that object. A call with no arguments leaves the table out or passes nil, and the tool receives the empty JSON object `{}`. Arguments match the tool's parameter schema, which is always a JSON Schema `object`. The schema is not readable from Lua, since `.parameters` is empty.

### What comes back

What `tools.call` returns follows the tool's output kind, which the tool decides, not the prompt. A plain tool's output comes back unchanged as a Lua string, and plain is the default for every tool not marked structured. A structured tool's JSON output comes back as a Lua table whose fields you index directly, with no decoding step. For a structured tool bound as `form` whose output is `{"text":"typed","images":[]}`:

````lua
local r = tools.call('form', {})
return r.text .. '|' .. tostring(#r.images)
````

The block returns `typed|0`. When a structured tool's output is not valid JSON, the call raises an [error value](05-lua-environment.md#catching-and-inspecting-errors) of kind `tool` naming the alias, and `tostring(err)` reads:

````text
tool call failure: structured tool "{alias}" returned invalid JSON
````

### Call errors

A `tools.call` that names neither a local tool nor a bound alias raises an error value of kind `unbound_tool`, whose `name` field holds the name. The message lists every alias bound in the run, not just the section's scope:

````text
tool "{name}" is not bound in this run; bound aliases: [...]
````

Five names belong to tools the engine itself offers the model, the ones [Advertising tools to the model](#advertising-tools-to-the-model) points to: `task`, `task_cancel`, `task_status`, `task_events`, and `await_tasks`. They take precedence over any alias of the same name. A model call to one of them goes to the engine's own tool, and a `tools.call` to one fails with `unbound_tool` even when a local tool is registered under that name, so give bound and local tools other aliases.

These argument errors raise at the call, where `pcall` catches them:

- `tools.call alias must be a string or Tool object, got {type}` when the first argument is anything else, a model handle included.
- `args must be a table, got {type}` when the arguments are not a table, such as `got integer`.
- `args must be a JSON-representable table` when the table holds a value JSON cannot represent, such as a function.

## Trusted and untrusted output

Every tool marks its output as trusted or untrusted, and the tool decides, not the prompt. The web fetch and web search tools return untrusted output, and any marking other than trusted counts as untrusted.

Output from an untrusted tool arrives in [the untrusted envelope](09-the-store.md#wrapping-untrusted-text), both in the value `tools.call` returns and in what the model reads on its next round: a preface line saying the text inside the tags is data, not instructions, then the tool's output between `<untrusted_input_{nonce}>` and `</untrusted_input_{nonce}>` tags, with every `<` inside escaped so the content cannot fake the closing tag. The wrapping happens before the calling script or the model sees the text, and it is automatic: the prompt never calls `untrusted()` for tool output.

Output from a trusted tool arrives exactly as the tool produced it, with no envelope, both in the script and in the model's next round.

The envelope is how a prompt recognizes untrusted output, since a Tool object's `.untrusted` field is always `false`:

````lua
local page = tools.call('fetch', { url = 'https://example.com' })
local wrapped = string.find(page, '<untrusted_input_', 1, true) ~= nil
````

`wrapped` is `true`, because the fetch tool's output is untrusted.

A run uses one nonce, 32 hex digits, for every wrap in every round and every chain, so identical untrusted content wraps identically. The nonce changes from run to run, so the tags cannot be guessed ahead of time, while two runs with the same [seed](04-how-a-prompt-runs.md#waiting-and-reproducibility) stay byte-for-byte identical.

Structured output works only with trusted tools. An untrusted tool's output is wrapped before the JSON parse, so even valid JSON from it fails with the invalid-JSON `tool` error.

## Model tool calls

Inside `models.loop` the model calls tools by alias. It can call one tool several times, or several tools, in one reply. Every call runs, in the order the model issued them, and each result goes back as its own tool record. The assistant record and all its results land together, in call order, before the model's text reply.

If the model calls `fetch` once and then replies, the list holds four records after `models.loop` returns:

| Record | Contents |
|---|---|
| `msgs[1]` | your user record |
| `msgs[2]` | the assistant record holding the call, with `tool_calls[1].name` equal to `fetch` |
| `msgs[3]` | the tool record, whose `role` is `tool`, whose `tool_call_id` equals `msgs[2].tool_calls[1].id`, and whose `content` is the tool's output |
| `msgs[4]` | the terminal assistant record with the reply |

The model loop always adds tool results to the conversation as text, whatever the tool's output kind. The structured table form applies only to a script's `tools.call`.

When a bound tool fails inside `models.loop`, the run keeps going. The failure becomes that call's tool record, with the tool's failure text wrapped as untrusted whatever its trust marking; the model reads it, and the loop continues instead of the run failing. The failed call still counts as answered.

### Calls outside the scope

The model can call only the tools offered in the current round. A batch of calls is all-or-nothing: the loop runs each call itself, but if any name in the batch is out of scope, the whole round fails before any tool in it runs. The failure is an error value of kind `out_of_scope_tool` whose `name` field holds the requested name:

````text
tool "{name}" is not in this section's scope; in-scope aliases: [...]
````

When the name is a bound tool slot in the run, the message ends with ` (alias is a bound tool slot but was not added to this section's scope)`. A name that is not bound anywhere gets the same message without that ending. Uncaught, the error fails the section and the run, and `pcall` catches it:

````lua
local ok, err = pcall(models.loop, msgs)
if not ok then
  if err.kind == 'out_of_scope_tool' then
    return 'the model asked for ' .. err.name .. ': ' .. tostring(err)
  end
  error(err)
end
````

`err.kind` is `out_of_scope_tool`, `err.name` is the requested name, and `tostring(err)` lists the section's in-scope aliases.

## Tool failures

How a tool failure reaches you depends on who made the call. A failing model tool call comes back to the model as wrapped failure text, as the previous section shows, and a failing script call raises at the call.

A script catches a failing bound tool with `pcall(tools.call, alias, args)`. The tool's own failure raises at the call as an error value of kind `tool`, and `tostring(err)` reads `tool call failure: {message}`, where the message is the tool's own failure text:

````lua
local ok, result = pcall(tools.call, 'fetch', { url = args })
if not ok then
  if result.kind == 'tool' then
    return 'could not fetch the page: ' .. tostring(result)
  end
  error(result)
end
return result
````

That text is the tool's short, model-safe message, and any deeper cause stays out of it. The model receives the same message wrapped as untrusted, and a script receives it as a `tool` error.

A caught tool error is read through `err.kind`, through `err.name` on `unbound_tool` and `out_of_scope_tool` errors, and through `tostring(err)` for the message, the same way as any [error value](05-lua-environment.md#catching-and-inspecting-errors):

| Kind | Raised when | `name` field | Message |
|---|---|---|---|
| `tool` | a called tool fails on its own in a script call | none | `tool call failure: {message}` |
| `unbound_tool` | a script call names neither a local tool nor an alias bound in the run, or uses one of the five engine tool names | the name | `tool "{name}" is not bound in this run; bound aliases: [...]` |
| `out_of_scope_tool` | the model calls a name outside the round's scope | the name | `tool "{name}" is not in this section's scope; in-scope aliases: [...]` |

`pcall` around a script `tools.call` catches every failure at the call alike: an unbound alias, one of the five engine tool names, a failure setting up the section's call counts, a local handler's error, or the tool's own failure.

Uncaught, these failures end the run with run error kind [`Tool`](17-limits-and-errors.md#how-a-failed-run-is-classified): a tool that failed, a model call outside the round's offered set, a script call to an alias not bound in the run, and a tool loop that reached its [round cap](11-conversations.md#the-round-cap) without a final reply. [The H1 pass](04-how-a-prompt-runs.md#the-h1-pass) has its own rule for uncaught failures.

## Counting calls

`tools.calls[alias]`, or `tools.calls.alias`, reads how many times the current section has called a tool, counting both script calls and model tool calls. Every section keeps its own counts, and counts never pass from one section to the next. This prompt answers only when the model read at least one page:

````markdown
---
name: sourced-answer
description: Answers only when the model read at least one page
promptforge: 0
capabilities:
  - promptforge/web
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
models:
  writer: {}
---

# Sourced answer

## Answer

```lua
models.use('writer')
tools.add({'search', 'fetch'})
local msgs = messages.new():user('Answer with sources: ' .. args)
models.loop(msgs)
local pages = tools.calls and tools.calls.fetch or 0
if pages == 0 then
  return 'no sources were read'
end
return msgs[#msgs].content
```
````

`tools.calls` appears with the section's first call to a bound or local tool, from a script or the model. Before that it is nil, so indexing it raises an ordinary Lua error. The `tools.calls and ... or 0` expression above covers a model that called no tool at all.

Every alias in the section's bound scope, from `tools.always` and `tools.add`, reads 0 until called, so an unused tool shows zero rather than an error: here `fetch` reads 0 when the model only searched. The scope is picked up at each tool call, so an alias scoped after the section's latest call has no count until the next call, and a local tool's alias gets a count only with its own first call. A script call to a bound tool outside the section's scope still gets a count, starting at 0 and counted up by the call.

Every call attempt adds one to its alias's count when the call is made, before the tool runs, whether it comes from a script or the model and whether the tool is local or bound. Failed and cancelled calls still count. The count goes to the calling section, under the alias the tool is bound to.

`tools.calls` is a read-only, live view of every counted alias: the section's bound scope as of its latest tool call, plus every alias the section has called. Each read returns the count at that moment, and assigning to it raises `tools.calls is read-only`.

Reading an alias that has no count, such as a misspelling, raises an error that names the key and lists the aliases that have counts:

````text
tools.calls: "{key}" has no seeded count; seeded aliases: [...]
````

The end of the message tells a real tool from a typo:

- ` (alias is a bound tool slot but was neither added to this section's scope nor dispatched by tools.call)` when the key is a bound alias the section never scoped or called, even when no alias has a count yet.
- ` - check for typos or add it via tools.add` for any other key, while at least one alias has a count.
- Nothing, for any other key when no alias has a count yet.

## Local tools

`tools.add_local` makes a tool out of a Lua function. A local tool needs nothing in the frontmatter, no `tools:` or `capabilities:` entry, and the engine answers its calls itself. This prompt gives the model a note-taking tool that writes to the store:

````markdown
---
name: note-taker
description: Lets the model save notes while it reads
promptforge: 0
models:
  writer: {}
---

# Note taker

## Take notes

```lua
models.use('writer')
local saved = 0
tools.add_local('save_note', 'Save one short note', { text = 'string' }, function(a)
  store.append('notes.md', a.text .. '\n')
  saved = saved + 1
  return 'saved'
end)
local msgs = messages.new():user('Save each key point of this text as a note: ' .. args)
models.loop(msgs)
return saved .. ' notes saved'
```
````

The four arguments of `tools.add_local(alias, description, params, handler)` are the alias, the description, the parameter table, and the handler function. The model sees the local tool under exactly that alias and description, and the tool is offered on the next model call without a separate `tools.add`. A local tool belongs to the section that registers it.

A local tool is called by alias, by the model or from Lua with `tools.call`. The handler runs as Lua inside the calling chain, in the section VM; the call itself involves no host work, and the handler's return value is the call's result. When the model makes the call, `models.loop` answers it itself: it runs the handler, appends the assistant record holding the call and a tool record with the handler's return, and continues until the model replies with text.

State across calls lives in ordinary Lua variables, like `saved` above, because the handler is a normal closure over the section's locals and runs in the same section VM as the rest of the section.

Local and bound tools mix in one round: the model is offered both, and each call goes to the section's Lua handler or to the host tool behind the alias.

### Parameters

The `params` table maps each parameter name to a type string, or to a `{type, description}` array whose description is optional. Each type is `"string"`, `"integer"`, `"number"`, or `"boolean"`:

````lua
tools.add_local('lookup', 'Find entries that match a query', { query = 'string', limit = { 'integer', 'maximum hits' } }, function(a)
  return 'looking up ' .. a.query .. ', at most ' .. a.limit
end)
````

The table becomes the tool's JSON Schema `object` parameters. Here the model sees `query` with type `string`, and `limit` with type `integer` and the description `maximum hits`; a bare type string gives its parameter no description. Every declared parameter is required, so the schema lists all of them as required, and an empty `params` table `{}` declares a tool that takes no arguments.

The schema is the same text in every run: the names in `properties` and `required` come out sorted bytewise, not in the order the table was written, so `required` lists `limit` before `query`.

The handler receives one table holding the call's arguments under the names declared in `params`, `a` in these examples. The table is built fresh from the call's JSON arguments, so a script caller's own table never reaches the handler.

### Return values

A handler returns a scalar, which becomes the call's result as text, following the [scalar return rule](04-how-a-prompt-runs.md#block-and-section-returns). Only the first return value counts: `nil` or no value gives the empty string, and a string, integer, number, or boolean becomes its text. Any other type fails the call with `cannot return a {type} as a result`, such as `cannot return a table as a result`.

The caller, script or model, receives the handler's text exactly as returned, as trusted output with no envelope.

### What a handler can do

Because the handler runs inside the calling chain, it can use [the store](09-the-store.md#what-the-store-is) and every other suspending call, such as `tools.call`, `models.infer`, `call`, and `user_input`, whether the model or a script called the tool. A store call made there is an ordinary store operation, like the `store.append` in the note-taker.

Inside a handler, `tools.call` is a script call. A failing bound tool raises there as kind `tool` instead of becoming text for the model, and a bound tool's untrusted output reaches the handler already wrapped and keeps its envelope if the handler returns it.

A local tool call involves no host work of its own: the engine hands it to the handler inside the calling chain, and only the calls the handler makes, such as store operations or bound tool calls, go to the host. Calls to a local tool count in `tools.calls` like any other tool call.

`jump` refuses while a handler runs, so a handler never [jumps](08-jump-and-call.md#sibling-jumps). Calling it there, even through a reference to `jump` saved before the handler ran, raises an ordinary error that `pcall` can catch, with the message `jump is unavailable inside a local tool handler: return a value from the handler and call jump from the block after the tool call returns`. The refusal lasts until the outermost handler returns or raises, including across nested local calls, and then `jump` works again in the block.

A handler that never returns still stops when the run is cancelled, as [Limits and Errors](17-limits-and-errors.md#calls-waiting-during-a-cancel) describes.

### Handler errors

A Lua error raised in a handler is raised again at the call, for script and model calls alike, with the handler's own error value, and an error value with a `kind` keeps it for your `pcall`. Uncaught, it fails the call and the run, and it never becomes failure text for the model. Only a bound tool's own failure goes back to the model as a result; a handler's error, a cancellation, and any other failure during the call end `models.loop`.

Local tool aliases follow the same alias rule as `tools.add` and `tools.always`, and each differs from every bound slot alias. Registration errors name the alias or parameter in quotes:

- `tools.add_local alias "{alias}" duplicates a bound tool slot` when the alias is already a bound tool slot.
- `tools.add_local alias "{alias}" is already registered` for a second `tools.add_local` with the same alias in the same section.
- `tools.add_local param "{name}" has unsupported type "{type}": expected "string", "integer", "number", or "boolean"` for any other type string.
- `tools.add_local param "{name}" must be a type string or a {type, description} array` for a parameter spec that is neither a string nor a table; the `{type, description}` part is literal text.

### A decision channel in the H1 pass

A local tool works as a decision channel in [the H1 pass](04-how-a-prompt-runs.md#the-h1-pass): the handler writes the model's choice into [`var`](05-lua-environment.md#keeping-values-in-var), and that choice shapes the rest of the run before any section is walked:

````markdown
---
name: router
description: Lets the model choose a route before the walk
promptforge: 0
models:
  writer: {}
---

# Router

```lua
models.default('writer')
tools.add_local('decide', 'Record the chosen route', { choice = 'string' }, function(a)
  var.route = a.choice
  return 'recorded'
end)
local msgs = messages.new():user('Call decide with short or long for this request: ' .. args)
models.loop(msgs)
```

## Result

```lua
return var.route or 'no choice'
```
````

A model that answers without calling the tool leaves the value unset, and `Result` then returns `no choice`. For weaker models, several no-argument local tools, one per choice, do the same job:

````lua
tools.add_local('choose_short', 'Pick the short route', {}, function()
  var.route = 'short'
  return 'ok'
end)
tools.add_local('choose_long', 'Pick the long route', {}, function()
  var.route = 'long'
  return 'ok'
end)
````
