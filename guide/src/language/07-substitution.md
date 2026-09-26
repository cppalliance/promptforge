# Substitution

Placeholders let the Markdown you write in a section carry live values: the caller's argument string, its parsed fields, values your Lua just computed, and run metadata. This chapter shows you how to write a `{{ }}` placeholder, where each value comes from and how it renders, how to put literal braces in prose, exactly when the text is filled in, and what every failure tells you, so you can build prompt text straight from data instead of gluing strings together in Lua.

## What substitution does

A placeholder is `{{ path }}` written inside a section's prose. It stays in the text as written until the Lua block right after that prose reads the [`prose` global](03-blocks-and-prose.md#the-prose-global), which holds the block's [pending prose](03-blocks-and-prose.md#the-pending-prose-buffer). At that read, every placeholder in the prose is filled in together, in one pass, and the block gets the finished string. Filling placeholders is called substitution.

The smallest example puts the argument string, which Lua reads as [`args`](06-arguments.md#input-basics), into a sentence:

````markdown
---
name: hello-args
description: Greets the argument string through a placeholder
promptforge: 0
---

# Hello Args

## Greet

hi {{ args }}!

```lua
return prose
```
````

Run with the argument string `Acme Corp`, the result is:

````text
hi Acme Corp!
````

A placeholder draws its value from one of six sources, named by the first segment of its path:

| Source | Placeholder | Value |
|---|---|---|
| `args` | `{{ args }}` | The argument string, exactly as passed |
| `argv` | `{{ argv }}`, `{{ argv.field }}` | The parsed arguments |
| `item` | `{{ item }}` | The value a section starts with when `fanout` runs it once per value, covered in [Fanout items](#fanout-items) |
| `var` | `{{ var.key }}` | A value in the `var` table |
| `sys` | `{{ sys.key }}` | A runtime-provided `sys` field |
| A Lua global | `{{ name }}`, `{{ name.field }}` | A global the section's Lua set without `local` |

This prompt uses a Lua global, one field of a table global, and a whole table in one sentence:

````markdown
---
name: answer
description: Fills three placeholders from Lua globals
promptforge: 0
---

# Answer

## Report

```lua
answer = 42
data = { score = 9 }
```

The answer is {{ answer }}; score {{ data.score }}; raw {{ data }}.

```lua
return prose
```
````

Its run result:

````text
The answer is 42; score 9; raw {"score":9}.
````

`{{ data }}` renders the whole table as compact JSON. [Dotted paths and rendering](#dotted-paths-and-rendering) gives the rule for every value type.

Only prose is substituted. Lua source is never touched, so `{{ }}` can appear anywhere in Lua code, such as inside a string literal, and it stays exactly as written there:

````markdown
## Echo

```lua
return '{{ args }} stays literal; the input is ' .. args
```
````

Run with `docs`, this section returns `{{ args }} stays literal; the input is docs`.

Placeholders work in any prose that a Lua block reads. A common shape is a [prologue](03-blocks-and-prose.md#lua-blocks-and-prose-blocks) that sets a value, prose that uses it, and an epilog that reads the prose:

````markdown
---
name: subject
description: Writes about the argument string
promptforge: 0
---

# Subject

## Write

```lua
var.subject = args
```

Write about {{ var.subject }}.

```lua
return prose
```
````

Run with `rivers`, the result is:

````text
Write about rivers.
````

Prose in the H1 body works the same way when a block in the H1 body reads it. Parsing keeps every placeholder exactly as written, and nothing is filled in until the run reads `prose`. Prose with no placeholders comes back unchanged.

A placeholder that cannot be filled raises an ordinary Lua error at the `prose` read, and the message says what went wrong, most often quoting the placeholder. A bad placeholder costs nothing until that read. Each part of this chapter names the failures of its own forms, and [Substitution errors](#substitution-errors) shows how to catch them.

## Writing a placeholder

The text between `{{` and `}}` is trimmed before it is used, so spaces just inside the braces are optional and a placeholder can even span lines. These three placeholders mean the same thing:

````markdown
{{args}} and {{ args }} and {{
args
}}
````

A placeholder ends at the first `}}` after its opening `{{`, so a path never contains `}}`.

A path is one or more segments joined by dots, as in `var.row.a`. Every segment is nonempty and has no spaces of its own; spaces belong only just inside the braces, where they are trimmed. A path with a trailing dot, two dots in a row, nothing at all between the braces, or a space next to a dot fails with substitution kind `EmptySegment` and the message `empty or padded path segment in {{ {path} }}`. This check runs before any lookup, so it fails even when a key with that name exists.

Every `{{` needs a later `}}`. An opening with no close fails with substitution kind `Unclosed` and the message `unclosed '{{' in prose`, and the error points at the opening `{{`.

## Run input with args and argv

`{{ args }}` inserts the argument string exactly as it was passed: unparsed, unquoted, with leading spaces and stray braces kept, whether or not the string is JSON. It works under any `args:` declaration or none.

````markdown
## Echo

Args: {{ args }}

```lua
return prose
```
````

Run with the argument string `  spaced { not json `, the section returns:

````text
Args:   spaced { not json 
````

[`argv`](06-arguments.md#prose-input-and-structured-input) is the parsed form of the argument string, which holds the decoded JSON under an explicit `args:` declaration. `{{ argv }}` inserts the whole parsed value, and a dotted path such as `{{ argv.query }}` or `{{ argv.row.a }}` reads one field or a nested field:

````markdown
---
name: show-argv
description: Shows the parsed arguments whole and by field
promptforge: 0
args:
  query:
    type: string
---

# Show Argv

## Show

got {{ argv }}
q={{ argv.query }} cell={{ argv.row.a }}

```lua
return prose
```
````

Run with `{"query":"papers","row":{"a":1}}`, the result is:

````text
got {"query":"papers","row":{"a":1}}
q=papers cell=1
````

A table or array renders as compact JSON with no spaces and its keys in sorted order, whatever order the caller wrote them in: the argument string `{"query":"papers","n":2}` renders through `{{ argv }}` as `{"n":2,"query":"papers"}`. A scalar renders as its plain value, so the argument string `42` renders `42`.

### Input in called chains

`{{ args }}` and `{{ argv }}` show the argument string of the chain the section runs in, and on the main walk that is the run's argument string. [`call(target, input)`](08-jump-and-call.md#call-input-and-args) runs another section as its own chain with `input` as that chain's argument string, and [`tasks.spawn`](15-tasks.md#starting-a-task) with an `input` option does the same for a task. Every section of such a chain sees that argument string. A chain started without an input, such as a nested `call` with no input or a [fanout arm](14-fanout.md#inside-an-arm), keeps the argument string of the chain that started it:

````markdown
---
name: chain-input
description: Shows which input a nested chain sees
promptforge: 0
---

# Chain Input

## Main

```lua
return call('## Sub', 'chain-args')
```

## Sub

```lua
return call('## Inner')
```

## Inner

Args: {{ args }}

```lua
return prose
```
````

Run with the argument string `run-args`, the result is:

````text
Args: chain-args
````

`## Inner` runs in the chain `## Sub` started without an input, so it keeps the argument string `## Sub` was given, not the run's.

### When argv is nil

`{{ argv }}` and every `{{ argv.field }}` need `argv` to hold a value. When the argument string under an explicit declaration does not parse as JSON or is the JSON `null`, or the H1 pass leaves `argv` nil, both forms fail with substitution kind `NilArgv` and the message `{{ {path} }} is nil (the args string is not JSON)`, such as `{{ argv }} is nil (the args string is not JSON)`. They never render an empty string. Because nothing renders until the read, a block can test [`argv`](06-arguments.md#checking-input) first and read `prose` only when the input parsed:

````markdown
---
name: safe-show
description: Shows the parsed input only when it parsed
promptforge: 0
args:
  query:
    type: string
---

# Safe Show

## Show

Value: {{ argv }}

```lua
if not argv then
  return 'no usable input'
end
return prose
```
````

Run with `not json`, the result is `no usable input`. Run with `{"query":"x"}`, the result is `Value: {"query":"x"}`.

### Placeholders read the chain's input, not the globals

`{{ args }}` and `{{ argv }}` read the chain's input and its parse, never the `args` and `argv` Lua globals. Reassigning the `args` global in a block leaves `{{ args }}` as it was:

````markdown
## Show

Input: {{ args }}

```lua
args = 'changed'
return prose
```
````

Run with `original`, this section returns `Input: original`.

A repair made with the [H1 repair pattern](06-arguments.md#the-h1-repair-pattern) reaches `{{ argv }}` from the first walked section on. The H1 body's own prose still sees the parse of the run's argument string, because the repaired `argv` takes effect only when the H1 pass ends:

````markdown
---
name: repaired-show
description: Shows a repaired query in a walked section
promptforge: 0
args:
  query:
    type: string
---

# Repaired Show

```lua
if not argv then
  argv = { query = 'repaired' }
end
```

## Show

Query: {{ argv.query }}

```lua
return prose
```
````

Run with `broken input`, the result is:

````text
Query: repaired
````

## Values from var, sys, and Lua globals

`{{ var.key }}` inserts a value from the [`var` table](05-lua-environment.md#keeping-values-in-var), and a dotted path reaches nested fields, as in `{{ var.row.a }}`. The placeholder sees the current contents of `var`, nested tables included, as plain JSON, so a prologue or any earlier block can set the value the prose uses. Strings render verbatim and numbers in their natural form:

````markdown
---
name: catalog
description: Fills placeholders from var
promptforge: 0
---

# Catalog

## Describe

```lua
var.kind = 'library'
var.count = 3
var.row = { a = 1 }
```

a {{ var.kind }} paper, {{ var.count }} copies, cell {{ var.row.a }}

```lua
return prose
```
````

Its run result:

````text
a library paper, 3 copies, cell 1
````

`{{ sys.key }}` inserts a runtime-provided field of the [`sys` table](05-lua-environment.md#run-metadata-in-sys), such as `{{ sys.id }}`. The runtime decides which fields exist. A field that is not present fails like any missing key, with substitution kind `MissingKey` and a message such as `missing {{ sys.bogus }}`.

`var` and `sys` always need a key, as in `{{ var.key }}` and `{{ sys.key }}`. Either root alone fails with substitution kind `BadPath` and the message `bad path: {{ var }}` or `bad path: {{ sys }}`. Every other source can render whole: `{{ args }}`, `{{ argv }}`, `{{ item }}`, and a bare global name.

### Lua globals by bare name

A section-local Lua global, meaning a name assigned without `local` such as `answer = 42`, is inserted whole by its bare name, as in `{{ answer }}`. The global can be set in an earlier block of the same section, earlier in the block that reads `prose`, or in the [shared library](03-blocks-and-prose.md#the-shared-library), which runs at the start of every section:

````markdown
---
name: globals
description: Fills placeholders from a shared global and a block global
promptforge: 0
---

# Globals

```lua shared
greeting = 'hello'
```

## Only

{{ greeting }}, {{ name }}.

```lua
name = 'Ada'
return prose
```
````

Its run result:

````text
hello, Ada.
````

A dotted path reaches into a table held in a global: with `row = { a = { b = 2 } }`, `cell {{ row.a.b }}` renders `cell 2`.

A bare name reads the section's global of that name at the moment of the `prose` read. A set data value, meaning a string, number, boolean, or table, is converted to its JSON form. A global that holds nil counts as unset. A `local` variable is not a global, so no placeholder reaches it.

The five built-in roots `args`, `argv`, `item`, `var`, and `sys` always win over a Lua global of the same name, so a global with one of those names is never reached by bare name.

A path starts with a built-in root or the name of a Lua global that is set. Any other first segment, bare or dotted, fails at the `prose` read with substitution kind `UnknownNamespace` and the message `unknown namespace or global '{name}' in {{ {path} }}`. A misspelled root, a `local` variable, and a nil global all produce it. The prose `{{ ghost }} here.`, read in a section where no global `ghost` is set, fails with:

````text
unknown namespace or global 'ghost' in {{ ghost }}
````

A global named in prose must hold data. A bare name whose global holds a function, userdata, or coroutine thread, or a table that contains one or refers to itself, fails with substitution kind `Serialize` and the message `global '{name}' in {{ {path} }} is not JSON data`. The message does not name the Lua type, so check what the global holds when you see it.

## Dotted paths and rendering

A resolved value renders by its type:

| Value | Renders as | Example |
|---|---|---|
| String | The text as is | `library` |
| Boolean | `true` or `false` | `true` |
| Number | Its natural form | `3` |
| Table with keys | Compact JSON, keys sorted, no spaces | `{"a":1}` |
| Array | Compact JSON, no spaces | `[1,2,3]` |

A table, an array, or a table-valued global referenced without further indexing renders whole as compact JSON. With `var.row = { a = 1 }`, `{{ var.row }}` renders `{"a":1}`; with `var.arr = { 1, 2, 3 }`, `{{ var.arr }}` renders `[1,2,3]`; and with the global `row = { a = 1 }`, `{{ row }}` renders `{"a":1}`.

A placeholder is a plain lookup, with no arithmetic and no expressions. To show a derived value, compute it in Lua, keep it in `var` or a global, and reference the result:

````markdown
---
name: full-name
description: Shows a value computed in Lua
promptforge: 0
---

# Full Name

## Show

```lua
var.first = 'Ada'
var.last = 'Lovelace'
var.full = var.first .. ' ' .. var.last
```

Name: {{ var.full }}

```lua
return prose
```
````

Its run result:

````text
Name: Ada Lovelace
````

Every key in a path must exist. A dotted path whose key is absent under `var`, `sys`, `argv`, or a global, or that continues past a scalar value, fails with substitution kind `MissingKey` and the message `missing {{ {path} }}`, such as `missing {{ sys.bogus }}`. It never renders an empty string.

Dotted segments address keys of a table only. To use one element of an array, pick it in Lua first and reference the result. A key that itself contains a dot cannot be reached by a path either, so copy it to a plain key first. A whole array still renders as compact JSON.

````markdown
## Show

```lua
var.tags = { 'red', 'green' }
var.first_tag = var.tags[1]
```

First tag: {{ var.first_tag }}; all: {{ var.tags }}

```lua
return prose
```
````

This section returns `First tag: red; all: ["red","green"]`.

A path under `var`, `sys`, `argv`, or a global that lands on a JSON `null`, such as `{{ argv.note }}` when the argument string is `{"note":null}`, fails with substitution kind `NullValue` and the message `missing {{ {path} }}`, which reads the same as a missing key.

## Fanout items

[`fanout(worker, collection)`](14-fanout.md#inside-an-arm) runs a worker section once for each member of a [collection](14-fanout.md#collections-and-member-order), and each of those runs, called an arm, starts with its member as `item`. In an arm, `{{ item }}` inserts that member into the prose:

````markdown
### Worker

topic: {{ item }}

```lua
return prose
```
````

In the arm whose member is `the angle`, the worker section returns `topic: the angle`.

`{{ item }}` renders a member by its type:

| Member | Renders as |
|---|---|
| String | The text as is, such as `plain` |
| Number | Its natural form, such as `7` or `2.5` |
| Boolean | `true` or `false` |
| Array | Compact JSON, such as `[7,"x"]` |
| Table with keys | Compact JSON, such as `{"k":1}` |
| JSON null | `null` |

A member that is JSON null renders as `null` here, while the other roots fail on null. An array member renders whole, so with the call below, the worker prose `Item: {{ item }}.` renders `Item: [7,"x"].`:

````lua
fanout('### Worker', {{7, 'x'}})
````

A hash member, one entry of a collection with keys, renders as the whole pair in compact JSON, such as `{"key":"alpha","value":1}`. The same rendering names the member of an exhausted arm in its [fanout result](14-fanout.md#results).

`{{ item }}` works only where an item is seeded: the first section an arm enters, or the first section of a [task started with an `item` option](15-tasks.md#starting-a-task). Anywhere else, such as a section on the walk or a later section the same chain enters, reading `prose` fails with substitution kind `NilItem` and the message `{{ item }} is nil (not inside a fanout arm)`. Uncaught in a walked section, that ends the run with the run error kind `Lua`.

`{{ args }}` and `{{ item }}` are used whole only. A dotted key after either root fails with substitution kind `NotATable` and the message `{root} is a string, not a table`, such as `item is a string, not a table`, whatever type the item really has and whether or not an item is set. To show one field of a table member, pick it in Lua first and reference the result:

````markdown
### Worker

```lua
title = item.title
```

Title: {{ title }}

```lua
return prose
```
````

## Literal braces and one-pass output

To write a literal delimiter in prose, escape it with a backslash:

| Write | Get |
|---|---|
| `\{{` | `{{` |
| `\}}` | `}}` |
| `\\` | `\` |

Escapes compose, so an escaped delimiter can sit right next to a live placeholder, which still resolves. With the argument string `Acme Corp`, the prose `\{{x}}{{ args }}` renders:

````text
{{x}}Acme Corp
````

Placeholders are filled anywhere in the section's pending prose, because the text substitution works on is the raw Markdown before the fence. That includes inline code, fenced code blocks that are not `lua` fences, and HTML comments. An example that must show `{{ }}` literally escapes the delimiters:

````markdown
## Show

Insert a value with `\{{ var.name }}`. <!-- written for {{ args }} -->

```lua
return prose
```
````

Run with `docs`, this section returns:

````text
Insert a value with `{{ var.name }}`. <!-- written for docs -->
````

Only the two characters `{{` open a placeholder. A lone `}}` or a single `{` is ordinary text, so `Set {x} and close }} here.` comes back exactly as written.

Substitution is one left-to-right pass. Inserted text is emitted verbatim and never scanned again, so a value that itself contains `{{ ... }}` is safe to insert:

````markdown
## Show

```lua
var.payload = '{{ args }}'
```

value: {{ var.payload }}

```lua
return prose
```
````

Run with `SECRET`, this section returns `value: {{ args }}`, not the argument string.

A backslash before any other character, or at the very end of the prose, stays literal. Windows paths such as `C:\temp\new`, text such as `\n`, and regex text such as `\d+` need no doubling. Because `\\` becomes one backslash, write `\\\\` where the output needs two backslashes in a row.

## When prose is rendered

Substitution happens at the first read of `prose`, not when the block starts. The `var` and global values a block sets before that read show up in the rendered text, and `sys` is read as it stands at that moment:

````markdown
---
name: word
description: Sets a value before reading the prose that names it
promptforge: 0
---

# Word

## Only

The word is {{ var.word }}.

```lua
var.word = 'mutated'
return prose
```
````

Its run result:

````text
The word is mutated.
````

A block can read `prose` any number of times and always gets the same string, rendered once at the first read, even if the values it references change afterward:

````markdown
## Only

The word is {{ var.word }}.

```lua
var.word = 'one'
local first = prose
var.word = 'two'
assert(prose == first)
return prose
```
````

This section returns `The word is one.`

Each prose-and-Lua pair in a section renders once, against its own first read, so a later block's `prose` is rendered fresh from the prose above that block, as [The prose global](03-blocks-and-prose.md#the-prose-global) shows. A block with no pending prose reads `prose` as the empty string.

Markdown that no block reads through `prose` is never rendered. That covers prose above a block that never reads `prose`, prose with no `lua` fence after it in its section, and prose after a section's last fence. A missing `var` key or an unclosed `{{` in such prose causes no error:

````markdown
---
name: unread
description: Prose that is never read never fails
promptforge: 0
---

# Unread

## Only

An unclosed {{ placeholder and a {{ var.missing }} key.

```lua
return 'ok'
```
````

Its run result:

````text
ok
````

`prose` is read-only. Assigning to it raises this error, which `pcall` catches:

````text
prose is read-only: assign to `var` or a section global instead
````

Keep derived text in `var` or in another global, and reference it from prose with a placeholder.

## Substitution errors

A failed substitution is an ordinary Lua error, raised inside the section's Lua at the `prose` read. Wrap the read in [`pcall`](05-lua-environment.md#catching-and-inspecting-errors) to catch it and carry on:

````markdown
---
name: catch-missing
description: Catches a failed substitution and returns something else
promptforge: 0
---

# Catch Missing

## Only

Missing: {{ var.missing }}.

```lua
local ok, err = pcall(function() return prose end)
if not ok then
  return 'caught'
end
return prose
```
````

Its run result:

````text
caught
````

Here `ok` is `false`, `err.kind` is `lua`, and `tostring(err)` is `missing {{ var.missing }} [MissingKey at byte 9]`, so a block can also inspect the text before deciding what to do. The bracketed part is explained under Reading the error text below.

Left uncaught, the failure ends the run with the run error kind `Lua`, and the run's message names the failing placeholder, such as `missing {{ var.missing }}`. In the [H1 pass](04-how-a-prompt-runs.md#the-h1-pass), the run ends as `RequirementsUnmet` instead, with that message as its notice. Substitution has no run error kind of its own; [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists every run error kind.

`prose` is not a substitution source. A `{{ prose }}` placeholder inside the prose makes the read fail, catchable with `pcall`, with substitution kind `Serialize` and the message `global 'prose' in {{ prose }} is not JSON data`.

### Reading the error text

Every substitution failure carries a stable substitution kind and the byte offset of the offending `{{`. The substitution kind appears only in the text; the error value `pcall` returns always has `kind` `lua`. Its text, which `tostring(err)` returns under `pcall` and the run's failure message contains when uncaught, reads:

````text
{message} [{Kind} at byte {offset}]
````

The offset counts bytes from the start of the pending prose the block read, not from the start of the file. That prose is trimmed, and it begins after any [thematic break](03-blocks-and-prose.md#thematic-breaks) that reset it. The prose `prefix {{ ghost.x }}`, with no global `ghost` set, fails with:

````text
unknown namespace or global 'ghost' in {{ ghost.x }} [UnknownNamespace at byte 7]
````

Most messages quote the placeholder path. Paths are shown safely: a newline, carriage return, or tab in a path appears as `\n`, `\r`, or `\t`, any other control character as `\u{XXXX}` in lowercase hex, such as `\u{001b}`, and a path longer than 80 characters shows only its first 80 characters followed by `...`. The 80-character cut is taken before the escaping, so a preview with escaped characters can run a little longer.

### Substitution kinds

| Substitution kind | Message | Raised when |
|---|---|---|
| `Unclosed` | `unclosed '{{' in prose` | A `{{` has no later `}}` |
| `EmptySegment` | `empty or padded path segment in {{ {path} }}` | A path segment is empty or has a space next to a dot |
| `BadPath` | `bad path: {{ var }}` or `bad path: {{ sys }}` | `var` or `sys` has no key |
| `UnknownNamespace` | `unknown namespace or global '{name}' in {{ {path} }}` | The first segment is no built-in root and no set global |
| `NotATable` | `args is a string, not a table` or `item is a string, not a table` | A key follows `args` or `item` |
| `MissingKey` | `missing {{ {path} }}` | A key is absent, or the path continues past a scalar |
| `NullValue` | `missing {{ {path} }}` | The path lands on JSON null, under any root but `item` |
| `NilItem` | `{{ item }} is nil (not inside a fanout arm)` | `{{ item }}` is used where no item is seeded |
| `NilArgv` | `{{ {path} }} is nil (the args string is not JSON)` | An `argv` placeholder is read while `argv` is nil |
| `Serialize` | `global '{name}' in {{ {path} }} is not JSON data` | A global holds a function, userdata, thread, or cyclic table, or the placeholder is `{{ prose }}` |

Assigning to `prose` is a separate runtime error with no substitution kind: ``prose is read-only: assign to `var` or a section global instead``.
