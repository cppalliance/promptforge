# The Lua Environment

Every `lua` fence in a prompt runs real Lua 5.5, with the host's work, the run's metadata, and the operator one plain function call away. This chapter shows you exactly what that Lua can reach: the sandbox and its globals, calls that wait on the host without callbacks, a table order that never changes between runs, the `var` table that carries your values along the walk, the `sys`, `ui`, `log`, and `user_input` globals, and error values you can catch, inspect, and trace back to a line in your prompt file.

## The sandbox and its globals

A Lua block is one `lua` fence in the H1 body or in a section ([Lua blocks and prose blocks](03-blocks-and-prose.md#lua-blocks-and-prose-blocks)). Every Lua region of a prompt, the shared library included, is written in Lua 5.5 syntax. The parser compiles each region when the prompt file is parsed; compiling never runs the code, and a block that does not compile stops the prompt before any Lua runs.

Each section's Lua runs in a sandbox whose standard libraries are `string`, `table`, and `math`, plus these base functions and values:

- `assert`, `error`, `pcall`, and `xpcall`
- `getmetatable` and `setmetatable`
- `ipairs`, `pairs`, `next`, and `select`
- `tonumber`, `tostring`, and `type`
- `_G` and `_VERSION`

That list is the whole toolkit. File access, the operating system, loading modules, and loading code from strings are outside it. Four of the base functions behave in a PromptForge way: `pairs` and `next` visit keys in a fixed order ([Deterministic table iteration](#deterministic-table-iteration)), and `pcall` and `xpcall` hand back error values ([Catching and inspecting errors](#catching-and-inspecting-errors)).

The smallest block that uses the sandbox calls a library function and returns the result:

````markdown
---
name: shout
description: Returns a word in capitals
promptforge: 0
---

# Shout

## Shout

```lua
return string.upper('hello')
```
````

The run result is:

````text
HELLO
````

### Host globals

On top of the sandbox, the runtime installs host globals in every section VM, with nothing to import. These are always present:

- `args`, `argv`, `sys`, `var`, and `prose`
- `log` and `user_input`
- `store` and `untrusted`
- `models`, `tools`, `messages`, and `compactors`
- `call`, `jump`, `fanout`, and `list_from_section`
- `tasks`

Three more appear only when they apply. `ui` is present when the host supplies a host-state snapshot. `item` is present inside a fanout arm, one of the concurrent runs that `fanout` starts ([Inside an arm](14-fanout.md#inside-an-arm)). And every declared model role label and every tool slot alias becomes a bare global of its own. This chapter teaches `var`, `sys`, `ui`, `log`, and `user_input`; each of the others is taught in its own chapter.

### Blocks, sections, and section VMs

A section VM is the fresh Lua instance a section runs in ([How the shared library loads](03-blocks-and-prose.md#how-the-shared-library-loads)). Every section entry gets a brand-new section VM, the H1 pass included ([The H1 pass](04-how-a-prompt-runs.md#the-h1-pass)), however the section is reached: by falling through from the section before it, by `jump`, or by `call`. The VM is torn down when the section ends.

That gives you two rules to write by:

- All blocks of one section run in the same section VM, so state set in one block is still there in every later block of that section. That covers globals you define, `var` fields, and saved references to host globals, including anything set while the shared library loaded.
- A plain global set in one section reads as nil in the next. Of all the Lua values, only `var` passes from one section to the next.

This prompt shows both rules. The two blocks of `## First` share a plain global, and `## Second` sees only what went through `var`:

````markdown
---
name: carry
description: Shows what survives between blocks and sections
promptforge: 0
---

# Carry

## First

```lua
var.trail = 'a'
note = 'set in the first block'
```

This prose sits between the two blocks of the section.

```lua
assert(note == 'set in the first block')
var.trail = var.trail .. 'b'
```

## Second

```lua
assert(note == nil)
return var.trail .. 'c'
```
````

`## First` returns nothing, so the walk falls through to `## Second` ([The section walk](04-how-a-prompt-runs.md#the-section-walk)), and the run result is:

````text
abc
````

A saved reference works the same way. A first block can run `saved_log = log` and a later block of the same section can call `saved_log('still here')`, because `log` stays valid for the whole life of the section VM.

### The three globals you meet first

`var` is where a block keeps its own values. Assign and read its fields, such as `var.greeting = 'hi there'` and then `var.greeting`. Strings, numbers, and booleans all work, and the runtime reads `var` back after each block. [Keeping values in var](#keeping-values-in-var) gives the full rules.

`sys` is a sealed, read-only global of run metadata. Read it by field name, as `sys.id` or `sys["when"]`. Each value arrives as the matching Lua value: strings, numbers, and booleans as Lua scalars, and objects and arrays as tables. [Run metadata in sys](#run-metadata-in-sys) lists the fields.

`log(message)` records an author checkpoint from any Lua block: in any section, in a fanout arm, in the H1 pass, and in prologues and epilogs alike ([Lua blocks and prose blocks](03-blocks-and-prose.md#lua-blocks-and-prose-blocks)). The run records the message verbatim, attributed to the section that called it, in call order across blocks. `log` is the only output channel in every section VM, in shared library code, a prologue, an epilog, and a lone block alike. [Checkpoints with log](#checkpoints-with-log) gives its rules.

## Calls that wait and errors that raise

Some host globals ask the host to do work and wait for the answer. These suspending calls are `models.infer`, `models.loop`, `call`, `fanout`, `tools.call`, the `tasks` functions, the `store` operations, and `user_input`. You write each one as an ordinary Lua call in straight-line code:

````lua
local reply = models.infer(prose)
return 'The model said: ' .. reply
````

The block pauses at the call and resumes with the result, so you never write callbacks or manage coroutines. Only the calling chain waits ([The section walk](04-how-a-prompt-runs.md#the-section-walk)); other chains keep running while it does.

### Failing a block on purpose

A block fails on purpose, and with it the run, through `error(message)` or through `assert(condition, message)` with a false condition:

````markdown
---
name: checked
description: Stops the run when a check fails
promptforge: 0
---

# Checked

## Check

```lua
local answer = 'no'
assert(answer == 'yes', 'the answer must be yes')
return answer
```
````

The failure is a Lua runtime error whose text includes your message, here `the answer must be yes`. Left uncaught, it ends the run with run error kind `Lua`, or with `RequirementsUnmet` when it happens in the H1 pass ([How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)). [Failure and cancellation](04-how-a-prompt-runs.md#failure-and-cancellation) covers what that does to the run as a whole.

### Failed host calls raise

A host call returns its result directly when it succeeds. When it fails, it raises the host's error value right at the call site, and `pcall` catches it so the block can keep going. That holds for `models.infer`, `call`, `fanout`, `tools.call`, the `store` operations, `user_input`, and the rest. One store failure is different: a conflict between two chains over the same store file, in block code, never raises at the call, and instead ends the run with run error kind `Determinism`.

Whatever failed, `pcall` gives you one kind of thing back: an error value, a Lua table holding a `kind` and a `message`, plus any fields that kind carries. That is true for an argument error from a suspending call, for a host request that failed, such as a model round, and for a host function that fails on the spot:

````lua
local ok, result = pcall(models.infer, prose)
if not ok then
  log('model round failed: ' .. result.kind)
  return 'no reply this time'
end
return result
````

[Catching and inspecting errors](#catching-and-inspecting-errors) teaches error values in full.

## Standard Lua and host calls

The standard functions and operators work inside blocks as they do in any Lua 5.5 program: `assert`, `error`, `pcall`, `tostring`, `type`, `setmetatable`, `string.upper`, `string.find`, string methods such as `s:match(pattern)`, `table.concat`, the length operator `#`, and `..` concatenation.

````lua
local words = { 'alpha', 'beta', 'gamma' }
local line = table.concat(words, ', ')
if string.find(line, 'beta') then
  return string.upper(line) .. ' (' .. #words .. ' words)'
end
````

That block returns `ALPHA, BETA, GAMMA (3 words)`.

### Joining values with table.concat

`table.concat(list, sep, i, j)` joins the elements `list[i]` through `list[j]`. `i` defaults to `1`, `j` defaults to `#list`, and `sep` defaults to the empty string, so `table.concat(list)` joins the whole list with nothing between the elements.

Strings and numbers join as they are. `table.concat` also joins any value that renders through `__tostring`, converting each such element with `tostring` first. That covers the host's own values, such as fanout results, which are one table per arm ([Results](14-fanout.md#results)), and model handles, the Lua values that stand for a model role ([Model handles](10-models.md#model-handles)). It also covers a table of your own with a `__tostring` metamethod:

````lua
local point = setmetatable({ x = 1, y = 2 }, {
  __tostring = function(p) return '(' .. p.x .. ',' .. p.y .. ')' end,
})
return table.concat({ 'at', point }, ' ')
````

That block returns `at (1,2)`.

Every element in the range needs a string rendering. When one has none, `table.concat` raises one of these messages, where `{k}` is the element's index inside `i..j`:

````text
invalid value (nil) at index {k} in table for 'concat'
invalid value ({type}) at index {k} in table for 'concat'
````

The first is for a nil slot in the range. The second is for a boolean, a function, or a table without `__tostring`, and `{type}` names it. A `j` past `#list` reaches a nil slot, so it gives the nil message at index `#list + 1`. The error is an ordinary Lua runtime error: `pcall` can catch it, and left uncaught it ends the run as `Lua`, or as `RequirementsUnmet` in the H1 pass.

### Host calls in every section

The suspending calls `models.infer`, `call`, `fanout`, `tools.call`, and the `tasks` functions work in every section's Lua, the H1 pass and fanout arms included, because every section VM's setup installs them. A block pauses only at one of the host's suspending calls; it never yields on its own.

`models.infer` takes an optional leading model handle and then the prompt, and a call with three or more arguments raises `models.infer takes (handle?, prompt)`. `models.infer` and `models.loop` both take an optional leading model handle. When the first argument is userdata but not a model handle, the call raises `{call} handle must be a model handle`, where `{call}` is `models.infer` or `models.loop`. For `models.infer` called with a handle and a prompt, any other non-nil value in the handle position raises `models.infer handle must be a model handle, got {type}`, naming the type; `models.loop` treats only a userdata first argument as its handle. Both are `lua`-kind errors raised at the call site, where `pcall` catches them.

### Your own metatable on _G

You can install your own metatable on `_G`, for example from the `lua shared` fence ([The shared library](03-blocks-and-prose.md#the-shared-library)), and it keeps working alongside the `prose` global ([The prose global](03-blocks-and-prose.md#the-prose-global)). Reads and writes of every other global still go through your `__index` and `__newindex`, the metatable's other fields are kept, and your handlers run once per lookup no matter how many blocks have run.

````lua
local defaults = { tone = 'friendly' }
setmetatable(_G, { __index = defaults })
````

With that in the shared library, reading the unset global `tone` in any block gives `friendly`, while `prose` still reads the section's rendered prose.

## Deterministic table iteration

Stock Lua leaves the order of `pairs` unspecified. In a prompt, `pairs` and `next` visit a table's keys in the same order on every run, in every section VM, and in shared code:

1. The array part `1..#t` comes first, in index order.
2. Then booleans, `false` before `true`.
3. Then numbers, by value.
4. Then strings, bytewise.

````lua
local seen = {}
for k in pairs({ 'a', 'b', z = 1, [true] = 2, [10] = 3 }) do
  seen[#seen + 1] = tostring(k)
end
return table.concat(seen, ' ')
````

That block always returns `1 2 true 10 z`.

The order depends only on the keys, never on the order you inserted them. The same order applies wherever PromptForge reads a table's keys in order, including the member list `fanout` builds from a collection ([Collections and member order](14-fanout.md#collections-and-member-order)).

### The order in detail

| Table | `pairs` visits |
|---|---|
| `{zeta=1, alpha=2, mid=3, beta=4, omega=5}` | `alpha, beta, mid, omega, zeta` |
| `{10, 20, 30, extra='x', another='y'}` | `1, 2, 3, "another", "extra"` |
| `{[true]='t', [7]='seven', b='bee', [false]='f', [2.5]='half', a='ay'}` | `false, true, 2.5, 7, "a", "b"` |
| `{[true] = 1, "x"}` | `1, true` |

- The array part `1..#t` comes before every other key. Integer keys outside `1..#t`, such as `0`, negative keys, or keys past the border, sort among the other numbers by value.
- Keys outside the array part follow one cross-type order: booleans, then numbers, then strings. A `false` key comes before a `true` key.
- Integer and float keys share one ascending sequence by exact numeric value, so `-1` comes before `-0.5` before `0`, and `2` before `2.5` before `3`.
- Very large integer keys, past 2^53, each keep their own exact place, with no rounding collisions between nearby integers or against nearby float keys.
- Float keys outside the 64-bit integer range sort beyond every integer key: a huge positive float such as `1e300` after all integers, and a huge negative float such as `-1e300` before all integers.
- String keys go in bytewise order, so for ASCII `"B"` comes before `"a"`, and `"a"` before `"ab"` before `"b"`.
- Tables, functions, and userdata used as keys are still visited, after every scalar key, but their order among themselves is not fixed. String keys that are not valid UTF-8 and infinite number keys go in that same trailing group.
- A table with holes never yields a nil value: nil slots inside `1..#t` are skipped, and only live keys are visited.
- `pairs` and `next` read raw values. They fetch each value without consulting `__index`, and they visit only keys actually stored in the table.

### Stepping with next

`next(t)` or `next(t, nil)` returns the first key and its value, in the order `pairs` uses. `next(t, k)` returns the key after `k`, and past the last key it returns `nil, nil`. So `next(t) == nil` tests for an empty table:

````lua
local k, v = next({ only = 1 })
assert(k == 'only' and v == 1)
return tostring(next({}) == nil)
````

That block returns `true`. The stateless loop `for k in next, t do ... end` walks the same ordered sequence as `pairs`: over `{10, 20, [false] = 'f', [true] = 't'}` it visits `1, 2, false, true`.

`pairs(t)` returns the usual three values `f, s, init`. `f` is an iterator that keeps its own position and ignores its arguments, `s` is the table itself, and `init` is nil. Calling `f()` repeatedly steps through the walk and keeps returning `nil, nil` after the end.

On large tables, prefer `pairs`. `next` rebuilds the full ordered key list from the live table on every call, while `pairs` builds it once when the loop starts.

### A __pairs metamethod

A `__pairs` metamethod controls what `pairs` returns for a table. When the table's metatable holds a function in `__pairs`, `pairs` calls it with the table and returns all of its results, in place of the ordered walk; with no `__pairs`, the ordered walk runs.

````lua
local t = setmetatable({}, {
  __pairs = function()
    local i = 0
    return function()
      i = i + 1
      if i <= 2 then return i, i * 10 end
    end
  end,
})
local out = {}
for k, v in pairs(t) do out[#out + 1] = k .. ':' .. v end
return table.concat(out, ' ')
````

That block returns `1:10 2:20`, even though the table itself is empty.

Two errors come from `pairs`. Both are Lua runtime errors: `pcall` catches them, and left uncaught they end the run as `Lua`, or as `RequirementsUnmet` in the H1 pass.

- When `__pairs` holds something other than a function, `pairs` raises `attempt to call a {type} value (metamethod '__pairs')`, naming the type it found.
- `pairs` accepts only a table, even when another value's metatable has `__pairs`. Any other argument raises `bad argument #1 to 'pairs' (table expected, got {type})`, naming the type it received, such as `got nil`.

### Changing a table during a loop

A `pairs` loop sees only the keys present when it started. Keys you add during the loop are not visited by that loop, while a new value you assign to an existing key the loop has not reached yet is read live.

Clearing keys with `t[k] = nil` while a `pairs` loop runs is safe:

- A cleared key the loop has not reached yet is skipped, never visited with a nil value. Over `{a=1, b=2, c=3}`, clearing `b` when `k == 'a'` visits `a` and `c`.
- Clearing the current key moves on to the next live key, so clearing each key as it is visited still visits `a`, `b`, and `c`.
- The loop runs to the end whatever the key's type. Clearing a boolean key in a table that also has an array part, such as `false` in `{10, 20, [false] = 'f'}`, still visits each key exactly once: `1, 2, false`.
- Clearing a table-valued key still visits the remaining table-valued keys before the loop ends.

Clearing keys while stepping with `next`, in `for k in next, t` or in a manual `next(t, k)` loop, never loses a key, and clearing array or boolean keys visits each key exactly once. Clearing an integer key outside the array part is the one case to watch: the walk can visit booleans and smaller numbers again. Over `{ [false]='b', [3]='c', [7]='y' }`, clearing `7` when `k == 7` visits `false, 3, 7, false, 3`. A `pairs` loop has no such repeat, which is one more reason to prefer it.

## Keeping values in var

`var` is the table that carries your values: within a section, from section to section along the walk, and into the section's prose. Assign fields and read them back:

````markdown
---
name: relay
description: Passes values along the walk in var
promptforge: 0
---

# Relay

## A

```lua
var.from_a = 'a'
```

## B

```lua
assert(var.from_a == 'a', 'fall through keeps var')
var.from_b = 'b'
```

## C

```lua
return var.from_a .. var.from_b
```
````

The run result is:

````text
ab
````

### How var travels

After each block runs, the runtime reads `var` back as JSON. That is how a value set in a section's prologue reaches the prose after it, and it is still readable in the section's epilog.

Along the walk, each section VM starts with the walk's current `var`. The section's final `var` is read back before its VM is torn down and becomes the next section's starting `var`. That holds on fall through and across a `jump`: if the H1 pass sets `var.from_h1 = 'seed'`, `## A` sets `var.from_a = 'a'` and jumps to `## C`, and `## C` sets `var.from_c = 'c'`, then `## D`, reached from `## C` by fall through, can return `var.from_h1 .. var.from_a .. var.from_c` as `seedac`.

The H1 pass runs before the walk ([The H1 pass](04-how-a-prompt-runs.md#the-h1-pass)). It starts from an empty `var`, since it runs first and is never entered again. Fields it writes are readable in the walked sections, and writes accumulate across several H1 blocks: a first H1 block can run `var.executions = (var.executions or 0) + 1`, a second `var.executions = var.executions + 1`, and a walked section then reads `var.executions` as `2`. A prompt without H1 blocks starts its first walked section with `var` set to an empty table.

Reading a `var` key that was never set gives `nil`, with no error, which is what makes the `(var.executions or 0)` pattern work.

### What var can hold

A `var` field holds any JSON data: numbers, strings, booleans, and tables of arrays and objects nested to any depth. A Lua sequence becomes a JSON array. These assignments:

````lua
var.n = 1
var.s = 'x'
var.t = { a = { 1, 2 } }
var.b = true
````

read back as:

````text
{ "n": 1, "s": "x", "t": { "a": [1, 2] }, "b": true }
````

Nested tables can be built step by step, as `var.t = {}` and then `var.t.kept = 'yes'`. Every nested write is checked like a top-level one.

Assigning a table into `var` stores a copy. Later changes to the original local table do not show up in `var`, so write through `var` itself when you want a change kept:

````lua
local list = { 'one' }
var.list = list
list[2] = 'two'
var.list[2] = 'three'
return var.list[2]
````

That block returns `three`; the change to the local `list` never reached `var`.

Only JSON data goes into `var`. Assigning anything else, such as a function or userdata, fails at the assigning line with:

````text
{path} must be JSON data, got {type}
````

`{path}` is the field's full path and `{type}` is the type found. The whole value is checked deeply, so a function nested anywhere inside an assigned table fails the same way, and the message names `function`. The path is dotted for string keys, as in `var.t.f`, and bracketed for any other key.

The assigning statement itself raises the error, so `pcall` can catch it and the block keeps running. Later writes still work. The caught value is an error value, so match its text through `tostring(err)`:

````lua
local ok, err = pcall(function() var.handler = log end)
assert(not ok and tostring(err):match('must be JSON data'))
var.kept = 'yes'
return var.kept
````

That block returns `yes`.

### The var guard

`var` and every nested `var` table are guarded. Read them by key or with `ipairs`: `pairs` over a `var` table yields nothing, and `#` on it gives `0`.

````lua
var.items = { 'a', 'b', 'c' }
local out = {}
for _, value in ipairs(var.items) do out[#out + 1] = value end
return table.concat(out, '')
````

That block returns `abc`. `getmetatable` on `var`, or on any nested `var` table, returns the string `"var is guarded"`, and `setmetatable` cannot replace the guard.

Change `var` only through its fields, and keep the `var` global itself in place. Once `var` has been reassigned, the next read-back fails with:

````text
the `var` global was reassigned; write `var.<field>` instead
````

The read-back runs at the end of a section, when prose renders, and when `call`, `fanout`, or a task start takes its copy of `var`. Because it runs on every exit, a reassigned `var` fails the run with run error kind `Lua` even when the H1 pass returns a scalar, and this failure stays `Lua` in the H1 pass too.

## Run metadata in sys

`sys` tells a block where it is running. Every section and every fanout arm gets these six fields:

| Field | Type | Value |
|---|---|---|
| `sys.when` | string | The instant the run started, in RFC 3339 |
| `sys.id` | string | The current section entry's id, such as `0.1` |
| `sys.taskid` | string | The id of the nearest enclosing task, such as `0` |
| `sys.section_name` | string | The heading name of the section whose Lua is running |
| `sys.execution` | string | The run's name, which the host assigns |
| `sys.section_count` | number | The number of top-level sections in the prompt |

````markdown
---
name: where-am-i
description: Reports its own section and entry id
promptforge: 0
---

# Where Am I

## Only

```lua
return sys.section_name .. ' ' .. sys.id .. ' of ' .. sys.section_count
```
````

The run result is:

````text
Only 0.1 of 1
````

`sys.when`, `sys.execution`, and `sys.section_count` are run-wide: every section, the H1 pass included, reads the same values. Fanout arms and chains started by `call` see the same `sys.section_count` as the run. `sys.section_name` is the heading name of the running section ([Sections and nesting](02-file-structure.md#sections-and-nesting)); in the H1 pass it is the prompt's title ([The H1 title and its content](02-file-structure.md#the-h1-title-and-its-content)). `sys.execution` is the execution identity the host gives the run, the same string in every section.

### The start instant in sys.when

`sys.when` is the instant the run started, as an RFC 3339 string, in every section and in the H1 pass. It is rendered once and is not a live clock: it is the identical string in every section and on every read.

It always has the UTC shape `YYYY-MM-DDTHH:MM:SS[.fff]Z`: a four-digit year, a two-digit month, day, hour, minute, and second, a literal `T`, an optional fraction, and a trailing `Z` with no offset.

| Start instant | `sys.when` |
|---|---|
| The Unix epoch | `1970-01-01T00:00:00Z` |
| A whole second | `2023-11-14T22:13:20Z` |
| 789 milliseconds past a second | `2024-02-29T12:34:56.789Z` |
| 780 milliseconds past a second | `2024-02-29T12:34:56.78Z` |
| 700 milliseconds past a second | `2024-02-29T12:34:56.7Z` |
| One millisecond before the epoch | `1969-12-31T23:59:59.999Z` |

- Precision is at most one millisecond. A whole second has no fraction; otherwise trailing zeros are dropped, so the fraction has one, two, or three digits.
- Dates follow the proleptic Gregorian calendar in UTC with correct leap years, including the century rules: 1900 and 2100 are not leap years, and 2000 is.
- A start instant before 1970 reads as an ordinary earlier calendar date.
- The year is four zero-padded digits for start instants from year 0000 through 9999.
- Any standard RFC 3339 parser reads `sys.when`, because it matches a standard RFC 3339 rendering byte for byte.

The host, not the prompt, supplies the start instant, together with a seed, when it creates the run. `sys.when` does not depend on the seed: a different seed changes the run's seeded values but leaves `sys.when` unchanged. With the same seed, the same start instant, and the same host answers, a prompt produces the same `sys.when` and the same seeded values, so its text result matches byte for byte ([Waiting and reproducibility](04-how-a-prompt-runs.md#waiting-and-reproducibility)).

### Section entry ids in sys.id

`sys.id` is the current section entry's id, a dot-separated string: the entering chain's id, a dot, and that chain's zero-based entry counter. The first section entered by chain `0.3`, for example, reads `0.3.0`. Compare it as a string, as in `sys.id == '0.1'`.

The main walk is chain `0`, and its numbering is stable. The H1 pass always takes entry `0.0`, whether or not the prompt has H1 blocks, so the first walked section is `0.1`, the next `0.2`, and so on:

````markdown
---
name: numbered
description: Checks the entry ids on the main walk
promptforge: 0
---

# Numbered

```lua
assert(sys.id == '0.0', 'the H1 pass is entry 0')
```

## First

```lua
assert(sys.id == '0.1', 'the first walked section is entry 1')
```

## Second

```lua
return sys.id
```
````

The run result is:

````text
0.2
````

- Every section entry gets a fresh id, including a section entered again with `jump`.
- Every `sys.id` in a run is distinct. A parent chain's entry id never collides with a child chain's, because a child's ids are one path segment longer, as `0.3` against `0.3.0`. That makes `sys.id` a unique key per entry, for example as part of a name you build.
- `sys.id` values are reproducible. Two runs of the same prompt with the same inputs get the same ids however their chains interleave or their tasks finish, because every counter belongs to one chain.

A chain started by `call` numbers its own section entries under its own chain id, such as `0.0.0` and then `0.0.1`, and the outer walk resumes its own count after the `call` returns ([Chain ids under call](08-jump-and-call.md#chain-ids-under-call)).

### The enclosing task in sys.taskid

`sys.taskid` is the id of the nearest enclosing task, as a string. It is `0` on the main walk and in the H1 pass. Inside a fanout arm it is the arm's own task, such as `0.0`, inside a chain started by `tasks.spawn` it is the spawned task, inside a `call` it is the caller's task, and passing it to the `tasks` functions names the current chain ([Task handles and ids](15-tasks.md#task-handles-and-ids)). Task ids are reproducible in the same way as `sys.id`.

### Fields that only exist in some places

Inside a fanout arm, `sys` also has the per-fanout `index`, the member's position, beside the `id` every entry gets; `index` is present only on a spawned task's first entry, such as an arm, so reading `sys.index` in an ordinary walked section raises `unknown sys field 'index'` ([Inside an arm](14-fanout.md#inside-an-arm)).

`sys.model` becomes readable only after the section's first tool call, and reading it before then raises `unknown sys field 'model'` ([The bound model in sys.model](10-models.md#the-bound-model-in-sysmodel)).

### Reading rules and errors

`sys` has a fixed set of fields, so a misspelled field is an error, never a silent nil. Reading a field `sys` does not have raises a `lua`-kind runtime error naming the field, whether the read comes from Lua or from prose:

````text
unknown sys field '{name}'
````

`pcall` catches it, and `tostring` on the caught error value reads `runtime error: unknown sys field '{name}'`. Uncaught, it ends the run as `Lua`, or as `RequirementsUnmet` in the H1 pass.

- A field that is present with a JSON null value reads as `nil`; only a field that is not there at all raises.
- `sys` takes string keys only. Any other key type raises `sys fields must be accessed by string key`.
- `sys` is read-only. Assigning any field, existing or new, raises `sys is read-only; cannot set '{field}'`, naming the field; `sys.when = 'x'` gives `sys is read-only; cannot set 'when'`.
- `getmetatable(sys)` returns the string `"sys is sealed"`, and `setmetatable` cannot replace the seal.
- Read `sys` fields by name. Iterating `sys` with `pairs` or `next` yields no entries.

## Host state with ui

Some hosts hand the run a host-state snapshot: a JSON object describing the host's state when the run started, such as the model currently selected in the host. `ui()` returns that snapshot as a Lua table whose fields are the snapshot's JSON fields as Lua values, for example `ui().selected_model`. Which fields a snapshot holds is up to the host.

The `ui` global exists only when the host supplies a snapshot. A run without one has no `ui` global at all, so test for it before calling it:

````markdown
---
name: host-model
description: Reports the host's selected model when there is one
promptforge: 0
---

# Host Model

## Report

```lua
if ui then
  local state = ui()
  if state.selected_model then
    return 'selected: ' .. state.selected_model
  end
end
return 'no host state'
```
````

`type(ui)` works as a test too: it is `'nil'` in a run with no snapshot.

- A JSON null field in the snapshot reads as nil, the same as an absent field, never as a special null value. With the snapshot `{ "selected_model": "m-1", "workspace_root": null }`, `ui().selected_model .. '/' .. tostring(ui().workspace_root)` gives `m-1/nil`.
- Each `ui()` call builds a new table. You can change the returned table freely, and the next call never sees the change.
- `ui()` shows the host state as the host captured it at run start, identically in every section. A change on the host takes effect on the next run.
- `ui` is installed before the shared library loads, so shared code can call it too.

When the host supplies a snapshot, `models.get` also accepts a model id taken from it that no role declares, as in `models.get(ui().selected_model)`, which returns a model handle for that model ([Model handles](10-models.md#model-handles)); without a snapshot there is no `ui` global, and `models.get` resolves only declared role labels.

## Checkpoints with log

`log(message)` records one author checkpoint in the run's event stream. It works from any Lua block: every section, fanout arm, and the H1 pass, prologue and epilog alike. It works from shared library code too, because it is installed before the shared library loads and stays available for the section VM's whole life.

````markdown
---
name: progress
description: Leaves checkpoints as it runs
promptforge: 0
---

# Progress

## Prepare

```lua
log('prepare started')
var.ready = true
log('prepare finished')
```

## Finish

```lua
log('finish started')
return 'done preparing'
```
````

The run records three checkpoints, in call order, each attributed to the section whose code called `log`:

| Section | Message |
|---|---|
| `Prepare` | `prepare started` |
| `Prepare` | `prepare finished` |
| `Finish` | `finish started` |

- A message is attributed to the section whose code called it, including a section reached through `call`.
- Checkpoints land in the event stream in order with the runtime's own reports, including those from the shared library load, teardown, and tool calls.
- Checkpoints interleave with store reports in statement order: `log('before write')`, then a `store.write`, then `log('after write')` records the first checkpoint, then the write's report, then the second checkpoint.
- Logging never changes a block's return value, the `var` contents, or the store's contents, so you can add `log` calls freely.

### Message rules

`log` takes exactly one argument: a single-line UTF-8 string of at most 256 Unicode characters with no control characters. A call that breaks a rule raises a Lua error in the block naming the broken rule, and records no checkpoint:

| Rule | Message |
|---|---|
| Exactly one argument; `log()` and `log('one', 'two')` both break it | `log expects exactly one argument` |
| A valid UTF-8 string; `log` does not turn numbers into strings, so pass `tostring(n)` | `log message must be a UTF-8 string` |
| At most 256 characters, counted as Unicode characters rather than bytes | `log message must be at most 256 characters` |
| One line, with no newline, tab, other control character, U+2028, or U+2029 | `log message must not contain newline or control characters` |

The character count is in characters, so 256 copies of `é`, which take 512 bytes, pass, and 257 fail. A string holding invalid UTF-8 bytes breaks the UTF-8 rule just as a number does.

A `log` message is recorded verbatim, with no redaction. Keep it to your own status wording, and leave out arguments, replies, tool data, credentials, paths, and store contents.

### Log quotas

Each section VM records up to 1024 `log` checkpoints by default, within a log byte quota of 256 bytes per allowed checkpoint, 262,144 bytes by default. The host can change the checkpoint count, and the byte quota follows it.

- Every one-argument call spends one checkpoint from the log event quota before the other checks run, so only an argument-count error costs nothing. A message that passes the checks then spends its UTF-8 byte length from the log byte quota.
- The shared library's load-time `log` calls spend the same section quotas.
- The next section starts with full quotas.

A call past either quota raises one of these messages:

````text
lua log event budget exceeded
lua log cumulative byte budget exceeded
````

`pcall` catches either as a `lua`-kind error value. Left uncaught, either ends the run with run error kind `Quota` ([How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)). [Lua block budgets](17-limits-and-errors.md#lua-block-budgets) sets these quotas beside the run's other limits.

## Asking the operator with user_input

The operator is the person the host puts in front of the run, answering its questions. `user_input()` asks the operator for text mid-run:

````markdown
---
name: ask-operator
description: Asks the operator for a topic
promptforge: 0
---

# Ask Operator

## Ask

```lua
local text, available = user_input()
if available then
  return 'Topic: ' .. text
end
return 'No operator, so the default topic it is.'
```
````

`user_input` is a suspending call, installed as a global in every section VM. The block waits until the host answers, then gets two values: the text, and an `available` boolean that is `true` when the text is the operator's own input. If the operator types `lighthouses`, the run result is:

````text
Topic: lighthouses
````

### Branch on available

When the host has no input to give, `user_input()` returns this fixed sentence with `available` set to `false`:

````text
User input is unavailable in this host; continue without it.
````

That is a normal return, not an error: the section keeps running, and no input is recorded. A host with no input handling at all gives the same answer.

Always branch on `available`, never on the text. An operator who types that exact sentence still gets `available == true`, so the flag is the only reliable test.

### What the wait keeps

- `user_input()` takes no arguments. Passing any raises an error value of kind `lua` with the message `user_input takes no arguments`.
- The operator's reply arrives byte for byte as typed, with `available` set to `true`, and the run records that text as operator input. Any text is valid.
- The section VM's state survives the wait. A local set before the call, such as `local before = 41`, still holds `41` after it, however long the operator takes.
- The wait pauses only the calling chain. The rest of the run keeps going while it waits.
- A task started with `tasks.spawn` that waits in `user_input()` stays live while other chains keep running, and it ends only after its own answer arrives or the chain that started it ends or cancels it.
- Each `user_input()` call reaches the host as one input request naming the run's execution and the section that asked. The host decides where the question goes: a terminal, a chat window, a web form, or nowhere.
- Only Lua asks the operator. A `models.loop` conversation offers the model exactly the tools the prompt adds, and sends no tool list at all when there are none, so the model has no way of its own to reach the operator.

### When input fails or is cancelled

When the host's input source fails, `user_input` raises at its call site an error value of kind `internal` with this message, where `{message}` is the host's failure text:

````text
user input request was not answered: {message}
````

`pcall(user_input)` catches it:

````lua
local ok, text, available = pcall(user_input)
if not ok then
  log('no operator input this time')
  return 'Continuing without the operator.'
end
return text
````

Left uncaught, an input-source failure ends the run with run error kind `Input` and the same message ([How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)).

When the host abandons a `user_input()` wait, the call raises a cancelled error; left uncaught, the run ends with the cancelled outcome, not as a failure. When the run is cancelled while a section waits in `user_input()`, the run stops promptly with the cancelled outcome, and the code after the call never runs ([Calls waiting during a cancel](17-limits-and-errors.md#calls-waiting-during-a-cancel)).

## Error locations in the prompt file

Every Lua block is compiled when the prompt file is parsed. A block that does not compile fails the parse with parse error kind `Lua` ([Parse error kinds](17-limits-and-errors.md#parse-error-kinds)), so the prompt never runs. At run time, a Lua error names the failing line of the prompt file itself, so you can go straight to it.

Both kinds of message name a Lua region by its location label, such as ``section `Check` epilog`` or ``section `Only` prologue`` ([Writing a Lua fence](03-blocks-and-prose.md#writing-a-lua-fence)). The label is the name Lua gives the block, and Lua prints it as `[string "{location}"]:N:`.

### Compile errors

A compile error reads:

````text
lua compilation error at {location} (line {source_line}): {message}
````

- `{location}` is the region's location label.
- `{source_line}` is the 1-based prompt-file line where that region's Lua starts, the line after the opening fence. It is not the line of the fault.
- `{message}` is the Lua 5.5 compiler's diagnostic, which itself names the region too.

### Runtime errors

A Lua runtime error names the failing line in the prompt file. The line inside the block is rewritten to the file line:

````text
file line = the region's first Lua line + the line inside the block - 1
````

Line numbers count every line of the file, the frontmatter included. Take this prompt:

````markdown
---
name: checker
description: Shows how error lines map
promptforge: 0
---
# Checker

## Check

Ask the model.

```lua
local a = 1
assert(false)
```
````

The frontmatter takes lines 1 to 5, the title is line 6, `## Check` is line 8, the prose is line 10, and the fence opens on line 12. The region's first Lua line is 13, and `assert(false)` is on line 2 of the block, so the error is reported at line 13 + 2 - 1 = 14. The message opens with a `{location}:{line}: ` tag taken from the first rewritten line, which a host can show next to the file name:

````text
section `Check` epilog:14: [string "section `Check` epilog"]:14: assertion failed!
````

Had that block failed to compile instead, the compile error would read ``lua compilation error at section `Check` epilog (line 13): {message}``, with line 13 being the region's first Lua line.

- A fence written before a section's prose, the section's prologue, uses the same absolute numbering. In a section that opens with a fence whose first Lua line is file line 11, an `assert(false)` on that first line is reported at line 11.
- An error deep inside a multi-line fence names the exact file line of the failing statement, not the fence's first line. With the first Lua line on 13 and the failure on the block's third line, the report says line 15.
- Each block's first line is its true line in the `.md` file, for the shared library too and in files with CRLF line endings.
- The run reports the mapped, location-tagged diagnostic as the entire error message, with no extra type label in front.
- A message that carries no line from the failing region passes through unchanged.

### Tracebacks

Runtime errors and tracebacks always show region names and real line numbers, never `?:` placeholders.

- A failed block's traceback is taken where the error was raised, so it shows your own frames, mapped to prompt lines, rather than the host's wrapper around the block.
- An error raised in a fanout arm or a called section traces back through its caller, and each frame's line points at its own region's prompt line, because only the current region's own markers are rewritten. A caller's frame such as ``[string "section `Main` prologue"]:3: in main chunk`` maps to the caller's own file line, and the arm's already-mapped line is left intact.
- Some host functions, such as `fanout` and the `tasks` functions, are written in Lua inside PromptForge. A failure that unwinds through them shows frames naming a built-in helper file and an exact line in it. Those frames are never rewritten, while your own frames still map to absolute prompt lines.

## Catching and inspecting errors

`pcall` catches every failure a host call or host function raises as an error value: a Lua table whose `kind` field is its error kind and whose `message` field is its text, plus any fields that kind carries. `type(err)` is `'table'`, and branching on `err.kind` is the way to decide what to do:

````markdown
---
name: careful
description: Tells an arm from an ordinary section by catching an error
promptforge: 0
---

# Careful

## Only

```lua
local ok, result = pcall(function() return sys.index end)
if ok then
  return 'arm ' .. result
end
if result.kind == 'lua' then
  return 'not an arm: ' .. tostring(result)
end
error(result, 0)
```
````

The run result is:

````text
not an arm: runtime error: unknown sys field 'index'
````

### The twelve error kinds

`err.kind` is always one of exactly twelve tags. Each tag is raised by the feature its link points to:

| Kind | Its own fields | Raised by |
|---|---|---|
| `tool_loop_exhausted` | none | [The round cap](11-conversations.md#the-round-cap) |
| `context_exhausted` | `reason` | [Compactors and context exhaustion](11-conversations.md#compactors-and-context-exhaustion) |
| `empty_model_reply` | `finish_reason` | [Empty and truncated replies](11-conversations.md#empty-and-truncated-replies) |
| `out_of_scope_tool` | `name` | [Advertising tools to the model](12-tools.md#advertising-tools-to-the-model) |
| `unbound_tool` | `name` | [Calling tools from Lua](12-tools.md#calling-tools-from-lua) |
| `tool` | none | [Tool failures](12-tools.md#tool-failures) |
| `task_not_owned` | `task` | [Task errors](15-tasks.md#task-errors) |
| `task_consumed` | `task` | [Task errors](15-tasks.md#task-errors) |
| `tasks_live` | `tasks` | [Cancellation and task lifetimes](15-tasks.md#cancellation-and-task-lifetimes) |
| `cancelled` | `task`, for a cancelled task | [Calls waiting during a cancel](17-limits-and-errors.md#calls-waiting-during-a-cancel) |
| `lua` | none | This chapter |
| `internal` | none | [Errors caught in Lua](17-limits-and-errors.md#errors-caught-in-lua) |

`err.kind == 'lua'` marks an authoring or runtime failure: a compile error, a runtime error in your own code, an argument or misuse error from a PromptForge function, or an exhausted log quota. A placeholder in prose that fails to render and running out of Lua memory are `lua` too. A host function failure caught by `pcall` is kind `lua` when it is an authoring or argument problem and `internal` when the Lua runtime's own machinery failed. A typed host error, such as a model round that ran out of context, keeps its own kind and fields.

### Message, fields, and tostring

- `err.message` is always a string. When the raiser gave no message, the message is the kind tag itself.
- `tostring(err)` gives exactly the message, with no traceback appended and no `file:line:` position prefix, so printing a caught host error shows exactly the host's message.
- A caught error value joins with a string using `..` on either side, as `'prefix: ' .. err` or `err .. ' suffix'`, exactly as if it were its message string.
- A kind's own fields sit beside `kind` and `message`: `reason` for `context_exhausted`, `finish_reason` for `empty_model_reply`, `name` for `out_of_scope_tool` and `unbound_tool`, `tasks` for `tasks_live`, and `task` for `task_not_owned`, `task_consumed`, and a cancelled task. Every such field is a string, and kinds without fields have only `kind` and `message`.

A caught host-request failure is inspected the same way: branch on `err.kind`, read the kind's own fields, and get the host's message verbatim from `tostring(err)`:

````lua
local ok, result = pcall(models.infer, prose)
if ok then
  return result
end
if result.kind == 'context_exhausted' then
  log('context ran out: ' .. result.reason)
  return 'The input was too long for one round.'
end
error(result, 0)
````

A `models.loop` failure works the same way: catch it with `pcall` and read `err.kind` and `err.name`, for example to tell which tool was out of scope.

### Catching at the call site

An argument error from a host call such as `models.infer`, `models.loop`, `call`, `fanout`, `tasks.spawn`, `tools.call`, or a `store` function is raised where the call was made, not as a failure of the whole block. It is an error value of kind `lua` whose `tostring` is the message, and `pcall` catches it at the call site.

Every string-argument failure has one of two shapes:

````text
{name} must be a string, got {type}
{name} must be a valid UTF-8 string
````

`{name}` is the argument's name: `prompt`, `input`, `path`, `contents`, `old`, `new`, or `pattern`. The first shape is for a value of the wrong type, and a missing required argument reads `got nil`. The second is for a Lua string holding invalid bytes. Type names tell integers from floats: an integer reads as `integer` and a float as `number`, so `3` reports `got integer` and `2.5` reports `got number`.

Host functions that fail on the spot, such as `models.get`, `tools.add`, a `sys` field read, or the `var` guard, also give error values under `pcall`, so `err.kind` works on them like on any other error value.

Suspending calls work inside `pcall`. The block still pauses inside the `pcall` and resumes there, so a successful call makes `pcall` return `true` and the result:

````lua
local ok, reply = pcall(function() return models.infer(prose) end)
assert(ok, reply)
return reply
````

A message handler passed to `xpcall` receives the same error value `pcall` returns, so `e.kind` is readable inside it. A handler that is not a function behaves exactly as in standard Lua.

### Values you raise yourself

A string or table you raise with `error` comes back from `pcall` exactly as raised, table identity included:

````lua
local own = { reason = 'my own failure' }
local ok, err = pcall(error, own)
assert(not ok and err == own)
local count = select('#', pcall(function() return 1, nil, 3 end))
return tostring(count)
````

That block returns `4`: a successful `pcall` returns `true` and then every value the function returned, nils included.

Only error values the host builds take a kind out of a block. A table you build and raise, even one whose `kind` matches a PromptForge kind, ends the block as an ordinary Lua runtime error.

### Raising a caught error again

When a failed host call's error goes uncaught, the run reports the original failure with its kind and structure. If you catch it and raise a different error, the run reports your new error instead. Raising the caught error value again unchanged works like this:

- Raised again with `error(err)` before any other suspending call, an error value ends the run exactly as if it had never been caught, with the same run error kind.
- Raised again later, after another suspending call, an error value of kind `context_exhausted` (with its `reason`), `tool_loop_exhausted`, `empty_model_reply`, or `tool` keeps its run error kind, and a `cancelled` value ends the run with the cancelled outcome. A `task_not_owned` or `task_consumed` value that still has its `task` field ends the run as `Lua`, in the H1 pass too. Any other error value, or one missing its fields, ends the run as `Lua`, or as `RequirementsUnmet` in the H1 pass.
- A `lua`-kind error value that leaves a block surfaces as a Lua runtime error with the same message and the absolute prompt line.

### Uncaught failures

An uncaught Lua failure, a runtime error in your code or an error you raise yourself, ends the run with run error kind `Lua`, holding the failure's message. That holds in a walked section, a `call` chain, a task, a fanout arm, and the shared library load. In the H1 pass the same failure ends the run as `RequirementsUnmet`, whose notice is the Lua error text. Only failures that would end as `Lua` become `RequirementsUnmet` there: other kinds keep their own run error kind in the H1 pass, and a failed shared library load, a failed `var` read-back, and a bad `jump` target in the H1 pass stay `Lua`. [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) lists every run error kind.

## Section VM lifecycle and reports

Every section, the H1 pass, and every fanout arm gets its own fresh section VM ([Inside an arm](14-fanout.md#inside-an-arm) shows the arm's side). The runtime sets each one up in the same fixed order:

1. The host values: `args`, `argv`, `sys`, and `var`.
2. The host functions: `log`, `store`, `tools`, and `models`.
3. `ui`, when the host supplied a snapshot.
4. `item`, in a fanout arm.
5. `jump` and `list_from_section`.
6. The suspending calls.
7. `models.loop`.
8. `user_input`.
9. The shared library load.
10. The store's suspending calls.
11. The declared alias globals.

That order is why shared library code can already call `log` and `ui` while it loads. Of all the Lua state a section builds up, only `var` passes to the next section VM; ordinary globals end with their VM.

Errors raised while a section VM is set up, including failures in the shared library's top-level code, name the section they happened in. The shared library load fails when its code raises, returns a value that is not a scalar, or calls `jump` ([How the shared library loads](03-blocks-and-prose.md#how-the-shared-library-loads)).

### Lifecycle reports

A section's Lua lifecycle shows in the run's reports, which carry the section name and never any values, such as a shared global's value or the text the run was given. For one section with a shared library and two blocks, the reports come in this order:

1. Shared library load started, then succeeded.
2. First block started, then succeeded.
3. Second block started, then succeeded.
4. Teardown started, then succeeded.

A block's `log` checkpoints fall between its started and succeeded reports. When the shared library raises, for example with `error(...)`, the load reports started and then failed, and teardown still runs and reports started and succeeded. A failing block reports a block failure. A section whose VM fails to build is torn down the same way before the error is reported.

At parse time, each Lua block's compilation reports a started report followed by exactly one succeeded or failed report, with no source text and no location label. [Run and section boundaries](16-task-events.md#run-and-section-boundaries) shows how these reports appear when you read a task's events.
