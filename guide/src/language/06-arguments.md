# Arguments

Every run hands a prompt one string of input, and this chapter shows you everything you can do with it: read it raw as `args`, declare typed arguments so callers know what to send, read the parsed value as `argv`, check and repair bad input before any section runs, and rely on `argv` staying fixed for the rest of the run. By the end you can write a prompt that accepts plain text or structured JSON and handles malformed input on its own terms.

## Input basics

Every run starts with exactly one argument string. When the caller passes nothing, that string is empty. A prompt reads it through the global `args`:

````markdown
---
name: greet
description: Greets whoever the caller names
promptforge: 0
---

# Greet

## Say hello

```lua
return "Hello, " .. args .. "!"
```
````

Run with the argument string `world`, the result is:

````text
Hello, world!
````

`args` is exactly the string that was passed. Nothing parses or trims it, so leading spaces stay and text that is not JSON is fine. It is an ordinary Lua string: you can return it with `return args`, concatenate it, or keep it in [`var`](05-lua-environment.md#keeping-values-in-var) with `var.answer = args`. With the argument string `  spaced { not json `, this check passes:

````lua
assert(args == '  spaced { not json ', 'args is the exact passed string')
````

`args` is a global in the Lua of every section, and also in the prompt's [shared library](03-blocks-and-prose.md#the-shared-library), the code in the H1 body's `lua shared` fence that every section can use. A shared helper can read it directly:

````markdown
---
name: shared-echo
description: Returns the argument string through a shared helper
promptforge: 0
---

# Shared Echo

```lua shared
function the_input()
  return args
end
```

## Main

```lua
return the_input()
```
````

Run with `later host value`, the result is `later host value`. Every [fanout arm](14-fanout.md#inside-an-arm), one run of a worker section per member of a collection, reads `args` too.

To tell callers what the string should contain, declare typed arguments with the `args:` key in the [frontmatter](02-file-structure.md#the-frontmatter-header). Its value is a map from each arg name to that arg's declaration. Each declaration needs `type:`, which is one of `string`, `boolean`, `integer`, or `number`, and can add `optional:`, `default:`, and `description:`. An arg is required unless it is marked optional. This declaration has three args:

````yaml
args:
  use_mcp:
    type: boolean
    default: true
    description: Search private sources
  limit:
    type: integer
    optional: true
  query:
    type: string
````

The parsed form of the argument string is the global `argv`, and every section can read it. Its shape follows the prompt's `args:` declaration, as the next section shows. Under an explicit declaration, `argv` is nil when the string does not parse. The two globals always sit side by side: `args` is the raw string and `argv` is the parsed form.

## Prose input and structured input

A prompt with no `args:` key still has a declaration. It gets the implicit default: one optional `string` arg named `prose`, described as "Freeform input for this prompt", with no default. Under this default declaration, the caller's whole argument string goes into `argv.prose` unchanged and is never parsed as JSON:

````markdown
---
name: echo-prose
description: Echo the caller's text
promptforge: 0
---

# Echo Prose

## Only

```lua
return argv.prose
```
````

Run with `hello there`, both `argv.prose == 'hello there'` and `args == 'hello there'` hold, and the result is:

````text
hello there
````

Empty input still arrives as a present empty string, so `argv` is not nil and `argv.prose == ''`. `args` still holds the exact string, here the empty string. The same wrapping applies to every argument string, including the ones described in [Input for calls, tasks, and fanout arms](#input-for-calls-tasks-and-fanout-arms).

Writing any explicit `args:` key makes the prompt's input structured. That holds even when the key copies the default shape exactly:

````yaml
args:
  prose:
    type: string
    optional: true
    description: Freeform input for this prompt
````

Under a structured declaration, the argument string is parsed as JSON into `argv` and is never wrapped into `argv.prose`. The caller passes a JSON string, and `argv` holds the decoded value. A JSON object becomes a table you read with dot access:

````markdown
---
name: search-query
description: Returns the query from structured input
promptforge: 0
args:
  query:
    type: string
  limit:
    type: integer
    optional: true
---

# Search Query

## Only

```lua
return argv.query
```
````

Run with `{"query": "papers", "limit": 5}`, `argv.query` is `papers` and `argv.limit` is `5`, and the result is:

````text
papers
````

A bare JSON number or boolean becomes a Lua number or boolean: with the argument string `42`, `tostring(argv)` returns `"42"`, and with `true` it returns `"true"`. Plain text that a default-declared prompt would wrap is not JSON, so under a structured declaration it leaves `argv` nil. [Checking input](#checking-input) shows how to test for that.

The argument string also reaches prose through placeholders, `{{ }}` forms that are replaced with values when a block reads `prose`, which [Substitution](07-substitution.md#what-substitution-does) explains in full. Three placeholders show it: `{{ args }}` renders the raw string unchanged, `{{ argv }}` renders the whole parsed value as JSON, and `{{ argv.field }}` renders one field. Both `argv` forms render the `argv` that the section itself holds.

````markdown
---
name: show-input
description: Shows the parsed input in prose
promptforge: 0
args:
  query:
    type: string
  n:
    type: integer
    optional: true
---

# Show Input

## Only

Whole: {{ argv }}; Field: {{ argv.query }}

```lua
return prose
```
````

Run with `{"query":"papers","n":2}`, the result is:

````text
Whole: {"n":2,"query":"papers"}; Field: papers
````

`{{ args }}` keeps every character, so the prose line `Args: {{ args }}` with the argument string `  spaced { not json ` renders as `Args:   spaced { not json `.

Three placeholder failures involve `args` and `argv`:

- When `argv` is nil, `{{ argv }}` and `{{ argv.field }}` fail with a message of the form `{{ {path} }} is nil (the args string is not JSON)`, where `{path}` is the placeholder's own path, such as `{{ argv }} is nil (the args string is not JSON)`.
- A dotted path into a field `argv` does not have, or into a scalar, such as `{{ argv.query.x }}` when `query` is a string, fails with `missing {{ {path} }}`, as in `missing {{ argv.query.x }}`.
- `args` is a string, so a dotted placeholder into it, such as `{{ args.x }}`, fails with `args is a string, not a table`.

Each of these is an ordinary Lua error raised where the block reads `prose`. You can catch it by reading `prose` inside [`pcall`](05-lua-environment.md#catching-and-inspecting-errors). Uncaught, it ends the run with the run error kind `RequirementsUnmet` in the H1 pass and `Lua` anywhere else; [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists every run error kind.

## Arg declarations

Here is a declaration that uses every key:

````yaml
args:
  query:
    type: string
    description: What to search for
  limit:
    type: integer
    optional: true
    default: 3
  use_mcp:
    type: boolean
    default: true
    description: Search MCP-connected private sources
````

Each arg name follows the [name grammar](02-file-structure.md#names-for-aliases-roles-and-args) shared by every name in the frontmatter, and each name appears once. An arg declaration is itself a map, and its only keys are these four:

| Key | Value | When left out |
|---|---|---|
| `type:` | `string`, `boolean`, `integer`, or `number` | Required |
| `optional:` | A YAML boolean | `false`, so the arg is required |
| `default:` | A value matching `type:` | No default |
| `description:` | A YAML string | No description |

The smallest valid declaration is a name with only a type:

````yaml
args:
  query:
    type: string
````

### Arg types

`type:` takes one of exactly four lowercase words, each naming what the caller supplies in the JSON argument string:

| `type:` | The caller supplies |
|---|---|
| `string` | A JSON string |
| `boolean` | A JSON `true` or `false` |
| `integer` | A whole JSON number |
| `number` | Any JSON number |

### Descriptions

`description:` takes a string that documents what the arg is for, and the text is kept exactly as written. An arg without it has no description.

### Defaults

A `default:` value matches the declared type. `string` takes a YAML string, `boolean` takes a YAML bool, and `number` takes any YAML number, whole or fractional. `integer` takes a whole number in 64-bit integer range, so `default: 3` works for an `integer` arg. YAML quoting decides the type: a quoted value is always a string, so quote a default only for a `string` arg, and write `true`, `false`, and numbers bare. `default: null` counts as no default.

A default is advertised to callers, not applied to `argv`; [The H1 repair pattern](#the-h1-repair-pattern) shows how to fill it in yourself.

### Optional args

`optional: true` means a caller can leave the arg out entirely. Without the key, `optional` is `false` and the arg is declared required, even when it has a `default:`. The value is a YAML boolean.

### A prompt with no arguments

To declare a prompt that takes no arguments, write an explicit empty map:

````yaml
args: {}
````

This is an explicit declaration, so the argument string is parsed as JSON like under any other structured declaration.

### Declaration errors

Every declaration mistake is a `Frontmatter` parse error, so the prompt never runs. The message starts with `invalid frontmatter: ` and the error gives the 1-based line and column in the file; [Parse error kinds](17-limits-and-errors.md#parse-error-kinds) covers the parse error kinds.

| Mistake | Message after `invalid frontmatter: ` |
|---|---|
| A declaration without `type:` | `` missing field `type` `` |
| A `type:` word outside the four | `` unknown variant `{value}`, expected one of `string`, `boolean`, `integer`, `number` `` |
| A `default:` that does not match `type:` | `` the default does not match the declared type `{type}` `` |
| An `args:` value that is not a map | `invalid type: {found}, expected a map of arg name keys to declarations` |
| A key other than the four in a declaration, such as a typo | `` unknown field `{key}`, expected one of `type`, `optional`, `default`, `description` `` |

A mismatched `default:` names the declared type, as in `` the default does not match the declared type `integer` ``. A quoted `'true'` for a `boolean` arg, an unquoted `42` for a `string` arg, and `1.5` for an `integer` arg all hit it. An arg whose value is a scalar or a list instead of a map is also a `Frontmatter` parse error, and so is an arg name used twice in one `args:` map.

## Checking input

The `args:` declaration advertises and documents your prompt's input, and it is never enforced at run time. A value of the wrong type, or a missing required arg, reaches the prompt unchanged and the run does not fail. If `query` is declared `type: string` and the caller passes `{"query":5}`, the run succeeds and `tostring(argv.query)` returns `5`. Checking the input's shape and types is the prompt's own job, usually done in its H1 body.

Under an explicit `args:` declaration, test `if argv then` (or `argv == nil`) to detect malformed input. When the argument string is not valid JSON, or is the JSON literal `null`, `argv` is nil in the H1 body and in every section, and the run carries on instead of failing:

````markdown
---
name: safe-query
description: Returns the query or a note about unusable input
promptforge: 0
args:
  query:
    type: string
---

# Safe Query

## Only

```lua
if not argv then
  return 'no usable input'
end
return argv.query
```
````

Run with `not json`, or with `null`, the result is:

````text
no usable input
````

A prompt with no `args:` key never gets a nil `argv` from its argument string, because the default declaration wraps the argument string into `argv.prose`.

You can tell an omitted optional arg apart from one passed as an empty string. An omitted field reads as nil in `argv`, while a field passed as `""` reads as the empty string:

````markdown
---
name: absent-or-empty
description: Tells an omitted arg from an empty one
promptforge: 0
args:
  prose:
    type: string
    optional: true
---

# Absent or Empty

## Only

```lua
if argv.prose == nil then return 'absent' end
assert(argv.prose == '')
return 'empty'
```
````

Run with `{}`, the result is `absent`. Run with `{"prose":""}`, the result is `empty`.

To enforce a required field, check it in the H1 body. The [H1 pass](04-how-a-prompt-runs.md#the-h1-pass) runs the H1 body's Lua before the walk, so bad input fails the run before any `##` section runs:

````markdown
---
name: strict-query
description: Requires a query before any section runs
promptforge: 0
args:
  query:
    type: string
---

# Strict Query

```lua
assert(argv and argv.query, 'query is required')
```

## Search

```lua
return argv.query
```
````

Run with `{}`, the assertion fails and `## Search` never runs. Because the failure happens in the H1 pass, the run ends with the run error kind `RequirementsUnmet`, and its notice is the Lua error text, which includes `query is required`. [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) explains the run error kinds.

## The H1 repair pattern

The H1 body's Lua is the only place a prompt can write `argv`. When the H1 pass completes, before the walk starts, the value the H1 body left in `argv` is read back and frozen. Every other section gets `argv` read-only, including every section on the walk and every section in a [called chain](08-jump-and-call.md#call-input-and-args), the walk that a `call` starts.

Inside the H1 body, `argv` is an ordinary writable global with no guard in the way. That makes it the place to repair input: read the raw `args` string and assign `argv` a fixed-up value. Whatever `argv` holds when the H1 body finishes, the parsed input or your repair, is what every later section reads, both in Lua and in `{{ argv }}` and `{{ argv.field }}` placeholders:

````markdown
---
name: repaired-query
description: Treats plain text as the query
promptforge: 0
args:
  query:
    type: string
---

# Repaired Query

```lua
if not argv then
  argv = { query = args }
end
```

## Show

Query: {{ argv.query }}

```lua
return prose
```
````

Run with `broken json`, `argv` is nil in the H1 body, the repair assigns the table, and the result is:

````text
Query: broken json
````

You can replace `argv` wholesale, as above, or change it in place. Assigning a field, as in `argv.query = 'fixed'`, or adding one, as in `argv.extra = 1`, edits the value that gets read back.

A declared `default:` is advertised, not applied. When the caller leaves the arg out, `argv` does not receive the default, so the prompt fills it in itself. For the `limit` arg declared earlier with `default: 3`, the H1 body can fill it in like this, replacing anything that is not a table first:

````lua
if type(argv) ~= 'table' then
  argv = { query = args }
end
if argv.limit == nil then
  argv.limit = 3
end
````

Leave `argv` as JSON data, meaning strings, numbers, booleans, and tables of them, or as nil when the H1 body finishes. Assigning anything else, such as a function, raises no error at the assignment. The read-back at the freeze, after the H1 pass and before any section on the walk, then fails the run with the run error kind `Lua`, even though it happens at the end of the H1 pass ([How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)). For a top-level function, userdata, or coroutine, the message is `argv must be JSON data, got {type}`, naming the Lua type `function`, `userdata`, or `thread`. A table holding a value that cannot be JSON also fails the read-back with a Lua error.

## Frozen argv

In every section other than the H1 body, `argv` is frozen. You read it freely with ordinary dot access at any depth, such as `argv.nested.hits`, and a field the argument string does not carry reads as nil instead of raising an error, so `argv.absent == nil` holds. Any write to it fails, as [Freeze errors](#freeze-errors) shows.

Outside the H1 body, the parsed JSON maps to Lua in a fixed way:

- A JSON object becomes a table with string keys.
- A JSON array becomes a 1-based sequence, so `argv.items[1]` is the first element.
- A string, number, or boolean becomes a plain Lua value.
- A JSON `null` nested anywhere reads as nil.

Read an array with `ipairs` or numeric indexing. Because the declaration is never enforced, the argument string can carry fields and arrays beyond the declared args:

````markdown
---
name: list-items
description: Joins the items that arrive with the query
promptforge: 0
args:
  query:
    type: string
---

# List Items

## Only

```lua
local names = {}
for i, name in ipairs(argv.items) do
  names[i] = name
end
return argv.query .. ': ' .. table.concat(names, ', ')
```
````

Run with `{"query":"papers","items":["a","b"]}`, the result is:

````text
papers: a, b
````

Outside the H1 body, read arrays with `ipairs` or indexing and read object fields by name. The length operator `#`, `pairs`, and `next` see an empty table there, because each frozen table is an empty stand-in that forwards reads to the data. Inside the H1 body, `argv` is a plain table, so `#`, `pairs`, `next`, and `ipairs` all work normally.

`argv` can hold a scalar instead of a table, and you read it directly. With the argument string `5` under a structured declaration, `argv == 5` holds and `tostring(argv)` returns `"5"`.

Outside the H1 body, `getmetatable` on any `argv` table returns the string `"argv is frozen"`, and its metatable cannot be replaced, so `setmetatable` on it raises an error.

Only the name `argv` is guarded. Every other global can still be defined, read, and assigned normally, so `scratch = 42` followed by `assert(scratch == 42)` works in any section. The [`prose` global](03-blocks-and-prose.md#the-prose-global), the rendered Markdown above a fence, keeps working normally beside it.

## Freeze errors

Assigning `argv` or writing into it in any section other than the H1 body raises a runtime error:

| Write outside the H1 body | Message |
|---|---|
| Assigning the global, as in `argv = { ... }` or `argv = nil` | `argv is frozen outside H1: assign it in H1 only` |
| Writing a field at any depth, including a new field | `argv is frozen outside H1: cannot set field {field}` |

The field message names the key being written. A string key is shown quoted, so `argv.mode = "x"` gives:

````text
argv is frozen outside H1: cannot set field 'mode'
````

A nested write names the innermost key, so `argv.opts.depth = 2` names `'depth'`. A non-string key, such as the array index in `argv.items[1] = x`, is shown in an internal debug notation instead of as a quoted name.

Freeze errors are ordinary Lua errors. [`pcall`](05-lua-environment.md#catching-and-inspecting-errors) catches them:

````lua
local wrote = pcall(function() argv.mode = 'x' end)
if wrote then return 'written' end
return 'refused'
````

This block returns `refused`. Uncaught, a freeze error ends the run with the run error kind `Lua`, which [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) covers.

A nil `argv` is frozen too. Under an explicit `args:` declaration, a run with no arguments gets a nil `argv`, because the empty string is not JSON; a prompt with no `args:` key gets `argv.prose == ''` instead. Assigning the nil `argv` outside the H1 body, as in `argv = {}`, fails with `argv is frozen outside H1: assign it in H1 only`.

## Input for calls, tasks, and fanout arms

A [`call`](08-jump-and-call.md#call-input-and-args) or a [`tasks.spawn`](15-tasks.md#starting-a-task) can give the section it starts its own argument string, which becomes that chain's `args`, with a fresh `argv` derived from it under the prompt's declaration (wrapped into `argv.prose` with no `args:` key, parsed as JSON with one) and frozen in that chain, so a field write there fails and `pcall` catches it. A `call` or spawn without an argument string inherits the caller's `args` and frozen `argv` whole, the H1 repair included, and a `call` without one, made inside a chain that was given one, inherits that chain's argument string, not the run's. Every [fanout arm](14-fanout.md#inside-an-arm) inherits its caller's `args` and frozen `argv` the same way.
