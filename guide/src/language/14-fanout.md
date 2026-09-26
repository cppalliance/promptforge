# Fanout

`fanout` runs one section once for every member of a collection, concurrently, and hands back one result per member in the collection's order. With it a prompt runs the same work over many inputs, such as asking a model about every item in a list section, instead of one input after another, and the results still line up the same way on every run. This chapter shows you the call, the collections it takes and the order their members run in, where the worker section goes, what each run sees and returns, how many run at once and how they share the store, what happens when one fails, and how fanouts nest.

## The fanout call

`fanout(worker, collection)` runs a worker section once per member of a collection, concurrently, and returns an array with one fanout result per member, indexed from 1 in collection order. The worker section is the section that does the work. The collection is a Lua table, and each value in it is a member. Each run of the worker section, one per member, is an arm:

````markdown
---
name: tagger
description: Runs one worker section per member
promptforge: 0
---

# Tagger

## Main

```lua
local r = fanout('### Worker', {'alpha', 'beta'})
return r[1].text .. ',' .. r[2].text
```

### Worker

```lua
return item .. '-done'
```
````

The run result joins the two arms' texts:

````text
alpha-done,beta-done
````

The first argument names the worker section by its [heading reference](02-file-structure.md#referring-to-a-section-by-heading), `#` marks included, the same form `call` and `jump` take. The usual layout puts the worker section under the calling `##` section as a `###` child section, as here. The walk never runs a child section by falling through to it ([the section walk](04-how-a-prompt-runs.md#the-section-walk)), so `### Worker` runs only as an arm: once for `alpha` and once for `beta`.

Inside each arm, the `item` global holds that arm's member, so the arm for `alpha` returns `alpha-done`. `r[1]` is the result for the first member and `r[2]` the result for the second. A result's `.text` field is the text of the arm's result: here the string the worker section returned, and in a worker section that ends with `return models.infer(...)`, the model's reply.

`fanout` works from any Lua block of a section. It also works as a bare statement whose return value is unused, for when only what the arms do matters, such as the store files they write.

The members usually come from a [list section](03-blocks-and-prose.md#list-sections), which keeps the work items as a plain Markdown list in the prompt. Pass `list_from_section(heading)` as the collection ([reading list items from Lua](03-blocks-and-prose.md#reading-list-items-from-lua)): each list item becomes one member, and so one arm. This version also reads `sys.index`, the arm's 1-based position in the collection, and joins the results with `table.concat`:

````markdown
---
name: topics
description: Two-member fanout over a list section
promptforge: 0
---

# Topics

## Research

```lua
local results = fanout('### Worker', list_from_section('### Topics'))
return table.concat(results, '\n')
```

### Worker

```lua
return item .. '-' .. sys.index
```

### Topics

- alpha
- beta
````

````text
alpha-1
beta-2
````

The first member's arm reads `sys.index` as 1 and the second member's arm reads 2. `tostring(result)` returns the result's text, so `table.concat(results, sep)` joins the arms' texts directly, as it does for any value with a `__tostring` ([standard Lua and host calls](05-lua-environment.md#standard-lua-and-host-calls)).

Results land in collection order no matter which arm finishes first, so a join or merge built from them is the same on every run. In this worker section, which sends prompts with [`models.infer`](10-models.md#running-a-round-with-modelsinfer), the arm for `a` makes a second model call and so finishes after the arms for `b` and `c`:

````lua
local first = models.infer(item .. ':1')
if item == 'a' then return first .. models.infer('a:2') end
return first
````

Over the collection `{'a', 'b', 'c'}`, with a model that replies `A1`, `B`, `C`, and `A2` to the prompts `a:1`, `b:1`, `c:1`, and `a:2`, the caller's `r[1].text .. '|' .. r[2].text .. '|' .. r[3].text` is still:

````text
A1A2|B|C
````

## Collections and member order

A collection is a Lua table with at least one member. A literal table of strings, such as `{'alpha', 'beta'}`, gives one arm per string, as in the first example of this chapter.

Each array member reaches its arm as `item` exactly as itself: a string stays a string, a number a number, a boolean a boolean, and a table a table:

````markdown
---
name: kinds
description: Shows each member arriving as itself
promptforge: 0
---

# Kinds

## Main

```lua
local r = fanout('### Worker', {'b', 2, true, {nested = 'x'}})
return table.concat(r, ',')
```

### Worker

```lua
if type(item) == 'table' then
  return 'table:' .. item.nested
end
return type(item) .. ':' .. tostring(item)
```
````

````text
string:b,number:2,boolean:true,table:x
````

A member stored under a key, in the table's hash part, reaches its arm as a pair table instead: the key is `item.key` and the value is `item.value`. Over `{alpha = 1, beta = 'two'}`, a worker section returning `item.key .. '=' .. tostring(item.value)` gives two arms whose texts join to:

````text
alpha=1,beta=two
````

### Member order

Member order is fixed and never depends on Lua's string hash seed. The array part, positions 1 through `#t`, comes first, in index order. The keyed members follow, sorted by key. So `{'a', 'b', extra = 'c'}` fans out as `'a'`, then `'b'`, then the pair `{ key = 'extra', value = 'c' }`.

Keys sort by type first, then by value:

- Booleans come first, `false` before `true`.
- Numbers come next, in exact numeric order.
- Strings come last, in byte order, which is alphabetical for plain ASCII names.

| Collection | Members in fanout order |
|---|---|
| `{'a', 'b', extra = 'c'}` | `'a'`, `'b'`, then the pair with the key `'extra'` |
| `{zeta = 1, alpha = 'two', mid = true, beta = 4, omega = 5}` | keys `alpha`, `beta`, `mid`, `omega`, `zeta` |
| `{[true] = 't', [7] = 'seven', b = 'bee', [false] = 'f', [2.5] = 'half', a = 'ay'}` | keys `false`, `true`, `2.5`, `7`, `'a'`, `'b'` |
| `{[5] = 'five'}` | one pair, `{ key = 5, value = 'five' }` |

Number keys sort exactly. Integers compare as integers, so `9007199254740992` and `9007199254740993`, which are 2^53 and 2^53 + 1, stay distinct and in order. A float key sorts exactly among the integers, and a float beyond the 64-bit integer range, such as `1e300`, sorts past every integer. The collection `{[9007199254740993] = 'b', [9007199254740992] = 'a', [1e300] = 'big', [-1e300] = 'small', [2.5] = 'half', [2] = 'two', [3] = 'three'}` fans out by the keys `-1e300`, `2`, `2.5`, `3`, `9007199254740992`, `9007199254740993`, `1e300`.

An integer key outside the array part is a keyed member, as the `{[5] = 'five'}` row shows: its one arm gets the pair `{ key = 5, value = 'five' }` as `item`.

This is the same key order the deterministic `pairs` and `next` use ([deterministic table iteration](05-lua-environment.md#deterministic-table-iteration)), so a keyed table that `fanout` accepts fans out in the order `pairs` visits its keys.

An arm's position, and so its `sys.index`, follows member order: array members take positions 1 to `#t`, and the sorted keyed members take the positions after them. Result slots follow the same order on every run. Over `{ zeta = 1, alpha = 2, mid = 3 }`, a worker section returning `item.key .. '=' .. item.value .. '@' .. sys.index` joins to:

````text
alpha=2@1,mid=3@2,zeta=1@3
````

The arm for `alpha` is at position 1, and its result is `r[1]`.

## Collection rules and errors

`fanout` checks the collection at the call. Every rule below except the last is checked before any arm starts, so a collection that breaks one never runs the worker section. Each broken rule raises an [error value](05-lua-environment.md#catching-and-inspecting-errors) of kind `lua` with that rule's message:

| Rule | Message when the rule is broken |
|---|---|
| The second argument is a Lua table | `fanout's second parameter is a collection; for a list section use list_from_section(heading)` |
| The collection holds at least one member | `fanout over an empty collection: no work is likely a bug` |
| Each member is a string, number, boolean, or table | `fanout collection member at index {index} is a {type}; members must be data` |
| Each key is a string, number, or boolean | `fanout collection key must be a string, number, or boolean, got {type}` |
| Each number key is finite | `fanout collection key is not a finite number` |
| Each string key is valid UTF-8 | The Lua runtime's own string conversion message, which has no fixed wording |
| Every value nested inside a member is JSON-representable | `item must be a JSON-representable value` |

The second argument is always a table. Any other value, such as a heading string, a number, or a boolean, raises the first message, which points to `list_from_section` for a list section.

An empty collection raises an error value whose `message`, and so `tostring(err)`, is exactly `fanout over an empty collection: no work is likely a bug`.

Members are data. A member that is a function, userdata, or thread raises the member message, where `{type}` is the member's type and `{index}` is the member's label. An array member's label is its position. A keyed member's label is its key as plain text: a string key's text, `true` or `false` for a boolean, an integer's decimal digits, or a float in its usual form, such as `2.5`. So `{'a', function() end}` raises:

````text
fanout collection member at index 2 is a function; members must be data
````

and a function stored under the key `cb` is named `index cb`.

Keys are strings, numbers, or booleans. A key of any other type raises the key message naming that type, such as `got table` for a table used as a key. Number keys are finite, so an infinite key such as `math.huge` raises `fanout collection key is not a finite number`. String keys are valid UTF-8, and a key whose bytes are not raises the Lua runtime's own string conversion message.

The checks run in order: first the table, its keys, and its members, then the empty check, and only then do arms start. The member check looks only at each member itself, not inside it, so a member table holding a function passes it. That member fails when its own arm starts, with `item must be a JSON-representable value`, raised from the `fanout` call after any arms already live are cancelled.

All of these are ordinary error values of kind `lua`, so `pcall` catches them and the section keeps running:

````markdown
---
name: guarded
description: Catches a fanout collection error
promptforge: 0
---

# Guarded

## Main

```lua
local ok, err = pcall(fanout, '### Worker', {})
if not ok then
  return err.kind .. ': ' .. tostring(err)
end
return 'ran'
```

### Worker

```lua
return item
```
````

````text
lua: fanout over an empty collection: no work is likely a bug
````

`pcall(fanout, '### Worker', 5)` likewise returns `false` and an error value whose text contains `collection`. Left uncaught, any of these errors fails the run with the same message text, as run error kind `Lua` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)), or as `RequirementsUnmet` when the call is in the H1 pass ([control from the H1 pass](08-jump-and-call.md#control-from-the-h1-pass)).

## The worker section

Any section in the caller's [visible set](08-jump-and-call.md#reachable-sections) can be the worker section, including a sibling of the calling section. One worker section can serve several callers, each fanning out to it on its own:

````markdown
---
name: shared
description: Two sibling sections fan out to one worker section
promptforge: 0
---

# Shared

## A

```lua
local r = fanout('## Worker', {'a'})
store.write('a.txt', r[1].text)
```

## B

```lua
local r = fanout('## Worker', {'b'})
return store.read('a.txt') .. ',' .. r[1].text
```

## Worker

```lua
return item .. '-done'
```
````

````text
a-done,b-done
````

A top-level worker section such as `## Worker` is also a section of the main walk, which reaches it in turn unless an earlier section returns or jumps. Here `## A` falls through to `## B`, and `## B` returns, which ends the run before the walk reaches `## Worker`. Place a top-level worker section after a caller that returns.

That is why the usual worker section is a child section, such as `### Worker` under the calling `##` section. The walk never descends into child sections by itself, so a child worker section runs only when `fanout` or `call` addresses it: a `### Worker` under `## Parent` runs exactly three times for `fanout('### Worker', {'a', 'b', 'c'})`, once per member, and never without an `item`. A `---` [thematic break](03-blocks-and-prose.md#thematic-breaks) inside the worker section only resets its pending prose, as it does in any section.

`fanout` works in the H1 body's Lua during the [H1 pass](04-how-a-prompt-runs.md#the-h1-pass) as it does in any section. There the worker section resolves against the top-level sections, and the arms still join in collection order:

````markdown
---
name: early
description: Fans out from the H1 pass
promptforge: 0
---

# Early

```lua
local r = fanout('## Worker', {'a', 'b'})
return r[1].text .. '|' .. r[2].text
```

## Worker

```lua
return 'item:' .. item
```
````

````text
item:a|item:b
````

The H1 pass returns a result, which ends the run, so the walk never reaches `## Worker`. An uncaught failure out of `fanout` in the H1 pass ends the run as `RequirementsUnmet` instead of `Lua` ([control from the H1 pass](08-jump-and-call.md#control-from-the-h1-pass)).

`fanout` also works from a section's epilog, its later `lua` block ([prologue and epilog](03-blocks-and-prose.md#lua-blocks-and-prose-blocks)), even when the prologue block is empty and the section has no prose:

````markdown
---
name: late
description: Fanout invoked from the epilog with empty prose
promptforge: 0
---

# Late

## Research

```lua
```

```lua
local results = fanout('### Worker', list_from_section('### Items'))
return table.concat(results, ',')
```

### Worker

```lua
return item .. '-' .. sys.index
```

### Items

- x
- y
````

````text
x-1,y-2
````

Inside an arm, and inside a chain the arm starts with `call`, `sys.section_count` is the run's top-level section count, the same value walked sections see ([run metadata in sys](05-lua-environment.md#run-metadata-in-sys)). A child worker section is not a top-level section and does not add to it: with `## Parent` as the only top-level section and the worker section under it, the arm reads `sys.section_count == 1`.

The worker section lies in the caller's visible set. A section under a sibling of the caller, a niece, is outside it, and naming one fails with the heading not-found error, an error value of kind `lua` ([heading addresses](08-jump-and-call.md#heading-addresses)). For example, `## Main` fanning out to a `### Niece` under `## Other` fails with this message:

````text
section heading `{heading}` not found; available sections: {list}
````

The worker section is always an ordinary section, never a list section. A section whose body is only list items, with no Lua, is a list section, which feeds `list_from_section` and cannot be a worker section. Naming one fails with an error value of kind `lua`. For example, `fanout('### Items', {'x'})`, where `### Items` holds only `- a` and `- b`, fails with:

````text
section `{name}` is a list section, not a worker template
````

## Inside an arm

Each arm runs its worker section in a fresh [section VM](03-blocks-and-prose.md#how-the-shared-library-loads), like any section, with the shared library replayed into it first, so every helper the `lua shared` fence defines is there. An arm also inherits its caller's context whole, so the run's models and tools work in it as in any section. Every arm reads its caller's `args` and frozen `argv` unchanged, an `argv` repaired in the H1 pass included, because `fanout` passes no input of its own ([the arguments chapter](06-arguments.md#input-for-calls-tasks-and-fanout-arms)).

On top of that, each arm gets two values of its own:

- The `item` global holds the arm's member, a copy made when the arm starts.
- `sys.index` holds the arm's 1-based position in the collection.

`{{ item }}` in the worker section's prose substitutes the arm's member into the text the worker section reads as [`prose`](03-blocks-and-prose.md#the-prose-global) ([what substitution does](07-substitution.md#what-substitution-does)), and [fanout items](07-substitution.md#fanout-items) shows how each member type renders. This prompt asks the model about each member:

````markdown
---
name: notes
description: Asks the model about each member
promptforge: 0
models:
  writer: {}
---

# Notes

```lua shared
models.default('writer')
```

## Parent

```lua
local r = fanout('### Worker', {'alpha', 'beta'})
return table.concat(r, '\n\n')
```

### Worker

Reply about {{ item }}.

```lua
return models.infer(prose)
```
````

The arm for `alpha` sends `Reply about alpha.` and the arm for `beta` sends `Reply about beta.`. The [`models.default`](10-models.md#choosing-a-sections-model) call in the shared library makes `writer` the default role, and that default holds in every arm. `models.infer` works inside an arm as in any section and returns the reply text as a string, and returning it makes it the arm's `.text`. A prompt built in Lua from `item` works the same way: `local first = models.infer(item .. ':1')` sends `a:1` from the arm for `a`.

A `return value` in the worker section's prologue ends the arm: that value is the arm's result, and the worker section's prose is never sent to a model. Such an arm needs no model at all:

````markdown
### Worker

```lua
return item .. '-' .. sys.index
```

Do work.
````

Over `alpha` and `beta`, this worker section gives `alpha-1` and `beta-2`, and the prose `Do work.` goes unused. The return hands its value back to `fanout` as the arm's result and never ends the run, because a scalar return ends the run only from the H1 pass or the main walk ([block and section returns](04-how-a-prompt-runs.md#block-and-section-returns)).

`tools.call` reaches the run's tool slots from an arm ([calling tools from Lua](12-tools.md#calling-tools-from-lua)), and once the arm's first tool call has run, [`sys.model`](10-models.md#the-bound-model-in-sysmodel) reads the selected model's id. With `models.default('writer')` in the shared library and a tool slot with the alias `echo`, this worker section returns the model's id and the member, such as `claude-sonnet-4-6:a` for the member `a`:

````markdown
### Worker

```lua
tools.call('echo', { value = item })
```

```lua
return sys.model .. ':' .. item
```
````

Before the arm's first tool call, whether a script `tools.call` or a model tool call, reading `sys.model` raises `unknown sys field 'model'`. A `models.infer` or `models.loop` round alone does not make it readable.

### Where item and sys.index exist

An arm sets `item` and `sys.index` only in the first section it enters, its worker section, and there `sys.index` always holds the arm's position in the collection. Outside the worker section, the sections and chains this book has covered so far have neither:

- A section the walk visits has no `item` global, so `item` there is nil.
- A walked section has no `sys.index` field, and reading it raises a Lua error with the message `unknown sys field 'index'`.
- A later section the arm reaches, for example by `jump`, has neither.
- The H1 pass, the main walk, and a chain started with `call` have neither, even when the `call` is made from inside an arm.

## Results

Each fanout result has four fields, and `#r` counts the results, one per member:

| Field | Type | Holds |
|---|---|---|
| `.text` | string | The text of the arm's result, or `''` when the arm ended without one |
| `.ok` | boolean | `true` when the arm completed normally |
| `.item` | the member | The member the arm processed |
| `.exhausted` | boolean | `true` only when the arm's `models.loop` hit the round cap |

In a fanout over `{'alpha', 'beta'}` whose worker section returns `item .. '-' .. sys.index`, these checks all pass:

````lua
assert(#r == 2)
assert(r[1].text == 'alpha-1' and r[1].ok == true)
assert(r[1].item == 'alpha' and r[1].exhausted == false)
````

`.ok` is `true` for an arm that completed normally and `false` for an arm whose [`models.loop`](11-conversations.md#a-first-conversation) hit the [round cap](11-conversations.md#the-round-cap). `.exhausted` is the reverse: `true` only for an arm that stopped at the round cap, and `false` for an arm that completed normally. An arm stopped at the round cap is the one arm failure a fanout survives. Its result keeps its `.item`, and its `.text` is a fixed stub that renders the member the way `{{ item }}` does, shown under [Arm failures](#arm-failures).

An arm that ends without a result, such as the one arm of a fanout over `{'alpha'}` whose worker section's only block runs `assert(item == 'alpha')` and returns nothing, still reports `.ok == true`, with `.text == ''`.

`.item` is the member the arm processed. For a keyed member it is the `{ key, value }` pair table, so in the `{ zeta = 1, alpha = 2, mid = 3 }` fanout from [Collections and member order](#collections-and-member-order), `r[1].item.key` is `alpha`. Over `{1, 'two', {n = 3}}`, `r[1].item == 1`, `r[2].item == 'two'`, and `r[3].item.n == 3`. A table member comes back as your own value, the very table the collection held rather than a copy. Inside the arm, by contrast, `item` is a copy of the member made when the arm starts.

### Read-only results

Results are read-only. Assigning any field raises the Lua error `fanout results are read-only`, reported at the assigning line. `pcall` receives that message, and an uncaught one fails the run with run error kind `Lua` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)). `setmetatable` on a result raises an error containing `protected metatable`, and `getmetatable(result)` returns a stand-in table that holds `__tostring` and no `__index` or `__newindex`:

````lua
local r = fanout('### Worker', {'alpha', 'beta'})
local wrote = pcall(function() r[1].text = 'forged' end)
local reset = pcall(setmetatable, r[1], nil)
local stand_in = getmetatable(r[1])
assert(not wrote and not reset)
assert(stand_in.__index == nil and stand_in.__newindex == nil)
assert(type(stand_in.__tostring) == 'function')
````

Read result fields by name: iterating a result with `pairs` visits no fields at all. Rebinding base globals such as `setmetatable` in your own code does not change how `fanout` builds its results.

## Concurrency

`fanout` is a [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise): the calling section pauses at it while the arms run, and every wait inside the fanout is an ordinary pause of that section. The calling section stays inside `fanout` until every arm has finished or been cancelled, so no arm outlives the call, whether `fanout` returns or raises. As with any suspending call, only the calling chain waits ([waiting and reproducibility](04-how-a-prompt-runs.md#waiting-and-reproducibility)).

Arms run concurrently by interleaving at suspending calls such as `models.infer`, not on separate threads. When every arm calls the model, every arm issues its first model call before any arm resumes, and each arm then waits on its own reply while the others go on. In a worker section that runs `local a = models.infer(item .. ':1')` and then `local b = models.infer(item .. ':2')`, a fanout over `{'one', 'two'}` sends `one:1` and `two:1` before either second prompt.

Conversations overlap the same way. With `models.loop` in every arm, all the arms' rounds are in flight together: every arm's first round goes out before any arm's second round, instead of one loop running after another.

### The concurrency cap

In one `fanout` call, at most the run's concurrency cap of arms are live at once, 8 by default. The host running the prompt sets the cap, and no frontmatter key or prompt call changes it.

`fanout` starts one arm per member, first member first, each seeded with its member as `item`, its position as `sys.index`, and a snapshot of the caller's `var` ([the var snapshot](08-jump-and-call.md#the-var-snapshot)). Arms start in member order until the cap is reached. From then on, whenever any live arm finishes, the next member's arm starts at once, even while an earlier arm is still waiting: over nine members, the ninth arm starts as soon as any one of the first eight finishes, not only when the first one does. Each arm is a task, started through the same request `tasks.spawn` uses, which can also give a task its own `item` and `sys.index`, as [Starting a task](15-tasks.md#starting-a-task) explains.

A collection far larger than the cap runs in full as the cap refills:

````markdown
---
name: many
description: Fans out over 1025 members
promptforge: 0
---

# Many

## Main

```lua
local items = {}
for i = 1, 1025 do items[i] = tostring(i) end
local r = fanout('### Worker', items)
return #r .. ':' .. r[1].text .. ':' .. r[1025].text
```

### Worker

```lua
return item
```
````

````text
1025:1:1025
````

When several arms finish at the same moment, the earliest-started arm is handled first, so its result, or its failure, is the one taken first.

Arms interleave at store calls too, so the order of the arms' side effects, such as which arm's store write lands first, is not fixed. Only the order of the returned results is.

## Isolation and the store

Arm isolation means each arm keeps its own `var`. Every arm starts from its own fresh clone of the caller's [`var`](05-lua-environment.md#keeping-values-in-var) as it stood when `fanout` was called, values seeded in the H1 pass included, and an arm's `var` writes never reach its sibling arms or the caller:

````markdown
---
name: isolated
description: Each arm works on its own copy of var
promptforge: 0
---

# Isolated

```lua
var.from_h1 = 'seeded'
```

## Parent

```lua
local r = fanout('### Worker', {'a', 'b'})
assert(var.a == nil and var.b == nil)
return r[1].text .. r[2].text
```

### Worker

```lua
local sibling = item == 'a' and 'b' or 'a'
assert(var.from_h1 == 'seeded')
assert(var[sibling] == nil)
var[item] = true
return item
```
````

````text
ab
````

Each arm sees the value the H1 pass seeded and never its sibling's key, and after `fanout` returns, the caller sees neither arm's write.

### Sharing the store

The store is the exception to arm isolation: the arms and the caller all use the run's one store. Arms keep out of each other's way through [claims](09-the-store.md#sharing-the-store-across-calls-and-tasks). An arm holds a claim on each path it writes or appends until the arm finishes, and while it is live, a sibling arm that writes, appends, reads, or globs that path causes a claims conflict, which ends the run. Once an arm has finished, its claims are released.

So the safe pattern gives each arm its own store path, for example one built from `sys.index`, and reads or combines the per-arm files in the calling section after `fanout` returns:

````markdown
---
name: perarm
description: Each arm writes its own store file and the caller merges
promptforge: 0
---

# Per arm

## Research

```lua
local results = fanout('### Worker', list_from_section('### Topics'))
local files = store.glob('arm-*.md')
local merged = table.concat(results, ',')
store.write('merged.md', merged)
return #files .. ':' .. merged
```

### Worker

```lua
store.write('arm-' .. sys.index .. '.md', item)
return item
```

### Topics

- alpha
- beta
````

````text
2:alpha,beta
````

After the run, `arm-1.md` holds `alpha`, `arm-2.md` holds `beta`, and `merged.md` holds `alpha,beta`. The path built from `sys.index` gives every arm a path of its own, so the arms never contend for a path. After `fanout` returns, [`store.glob`](09-the-store.md#listing-files-with-glob) in the caller finds both arm files, and the merge writes the ordered join of the results to one store file.

Two more patterns never conflict:

- One arm may write the same path several times, and the last write wins; rewriting its own path is never a conflict. `store.write('own.txt', 'first')` and then `store.write('own.txt', 'second')` in one arm leave `second`.
- One block may call `fanout` more than once, and a later fanout's arms may write paths an earlier fanout's arms wrote. The earlier arms have finished, so the later write simply overwrites. Two fanouts in a row whose worker section runs `store.write('seq.txt', item)`, over `{'one'}` and then `{'two'}`, leave `two`.

### When arms conflict

Two live arms writing the same path end the whole run:

````markdown
---
name: clash
description: Two arms write one store path
promptforge: 0
---

# Clash

## Parent

```lua
local r = fanout('### Worker', {'alpha', 'beta'})
return r[1].text
```

### Worker

```lua
store.write('shared.txt', item)
return item
```
````

The run fails with run error kind `Determinism` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)). Its message begins `store determinism violation:` and names the contested path, the words `conflicts with`, and both arms.

`store.append` counts as a write. Two live arms appending to one path end the run the same way, and only one arm's append lands: with both arms running `store.append('log.txt', item .. ';')`, `log.txt` afterward holds exactly `alpha;` or `beta;`.

`pcall` cannot catch a claims conflict, not even wrapped around the store call inside the arm. Over `{'alpha', 'beta'}`, this worker section still ends the run as `Determinism`:

````lua
local ok, err = pcall(store.append, 'notes.md', item .. '\n')
store.write('caught-' .. sys.index .. '.txt', tostring(ok))
````

The conflict ends the whole run: the losing arm never resumes, no arm counts as failed, arms still waiting are cancelled, and at most the winning arm completes. The losing arm's store call never reaches the store, so exactly one arm's change lands: `notes.md` ends as `alpha` or as `beta`, followed by a newline, never both. In block code a claims conflict is never raised at the call ([store errors](09-the-store.md#store-errors)). The one exception is a store call in shared library code while it loads, for example in an arm whose section VM is starting while a live sibling holds the claim, and the store chapter covers that case.

Live arms stay entirely off each other's claimed paths, and a read or glob counts too: an arm whose `store.read` or `store.glob` touches a path a live sibling has claimed ends the run with the same `Determinism` error. So arms coordinate only through the results `fanout` returns, never through store files a live sibling is writing, and they cannot meet by polling each other's marker files.

A run that ends on a claims conflict still leaves the store consistent. The run does not return until any arm store call still in flight has finished, so the winning arm's write is in the store and the store reads normally afterward.

## Arm failures

`fanout` is fail-fast: any arm failure other than round cap exhaustion fails the whole fanout. Every live sibling is cancelled without waiting on its pending model calls, members whose arms have not started never run, and the failure is raised from the `fanout` call. A claims conflict is not an arm failure at all; it ends the run, as [Isolation and the store](#isolation-and-the-store) shows.

Calling `error('message')` in an arm ([failure and cancellation](04-how-a-prompt-runs.md#failure-and-cancellation)) fails the fanout this way:

````markdown
---
name: broken
description: One worker section fails on purpose
promptforge: 0
---

# Broken

## Research

```lua
local results = fanout('### Worker', list_from_section('### Topics'))
return table.concat(results, '\n')
```

### Worker

```lua
error('arm deliberately failed')
```

Work.

### Topics

- alpha
- beta
````

Left uncaught, as here, the failure ends the run with run error kind `Lua` ([how a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)), and the run's message contains `arm deliberately failed`. The error `fanout` raises is the failing arm's own error value, with its original kind and message, not a wrapper around it. For an arm that called `error('message')`, that kind is `lua`. The same uncaught failure in the H1 pass ends the run as `RequirementsUnmet` instead ([control from the H1 pass](08-jump-and-call.md#control-from-the-h1-pass)).

`pcall(fanout, worker, collection)` catches a fanout failure ([catching and inspecting errors](05-lua-environment.md#catching-and-inspecting-errors)): it returns `false` plus the error value, and the block keeps going, further `models.infer` calls included. A late reply meant for a cancelled sibling is discarded instead of failing the run. Here the worker section calls the model and then raises an error for the member `boom`:

````lua
local ok, err = pcall(fanout, '### Worker', {'boom', 'slow'})
assert(not ok, tostring(err))
local a = models.infer('after')
return 'caught:' .. a
````

The arm for `boom` fails, the arm for `slow` is cancelled while it waits on its reply, and the block goes on to its own round and returns `caught:` followed by the model's reply.

### Round cap exhaustion

A fanout survives an arm whose `models.loop` hits the [round cap](11-conversations.md#the-round-cap), the failure with error kind `tool_loop_exhausted`. That arm's slot gets `.ok == false`, `.exhausted == true`, and its original `.item`, while the sibling arms keep running and return normal results. Over `{'loop', 'plain'}`, where the arm for `loop` hits the round cap and the arm for `plain` completes, these checks pass:

````lua
local r = fanout('### Worker', {'loop', 'plain'})
assert(#r == 2)
assert(r[1].ok == false and r[1].exhausted == true and r[1].item == 'loop')
assert(r[2].ok == true and r[2].exhausted == false)
````

An exhausted arm's `.text` is a fixed stub: a level-2 heading holding the member, rendered the way `{{ item }}` renders it, then `UNKNOWN`, then `(section incomplete: tool loop exhausted)`, separated by blank lines. For the member `loop`, `r[1].text` is:

````text
## loop

UNKNOWN

(section incomplete: tool loop exhausted)
````

### What stops with a cancelled arm

When an arm is cancelled because a sibling failed, everything under it stops too: a chain it started with `call` stops, a nested fanout's arms stop with it, and so on down, and anything that had not yet run its block never runs it. What a cancelled arm had started is abandoned with the reason `owner_aborted`, which [Cancellation and task lifetimes](15-tasks.md#cancellation-and-task-lifetimes) explains.

If the host cancels the run while arms wait on model replies, the run ends at once with the [cancelled outcome](04-how-a-prompt-runs.md#failure-and-cancellation), not a failure, without waiting for those replies, and no arm outlives the run ([Cancelling a run](17-limits-and-errors.md#cancelling-a-run)).

## Addressing from an arm

Inside an arm, heading references resolve over the worker section's own visible set, the same set it has as any running section: the sections at its level under the same parent, the worker section itself excluded, plus its own children ([reachable sections](08-jump-and-call.md#reachable-sections)). `call`, `jump`, `fanout`, and `list_from_section` all resolve over it. So when the worker section is a child of the calling section, the calling section and its siblings are not found from the arm.

`list_from_section` inside an arm reads a list section in the worker section's visible set as a Lua array of strings:

````markdown
---
name: reach
description: An arm reads a sibling list section
promptforge: 0
---

# Reach

## Parent

```lua
local r = fanout('### Worker', {'alpha'})
return r[1].text
```

### Worker

```lua
local items = list_from_section('### Items')
return item .. ':' .. table.concat(items, ',')
```

### Items

- x
- y
````

````text
alpha:x,y
````

From this arm, `### Items` resolves because it is the worker section's sibling, while `## Parent`, the section that fanned out, is outside the worker section's visible set.

`call` inside an arm runs a [called chain](08-jump-and-call.md#called-chains): it starts at the target, falls through the target's following siblings, and its result is what `call` returns to the arm. The called chain runs as plain sections, with no `item` global. Under the same `## Parent`, with these sections in place of `### Worker` and `### Items`:

````markdown
### Worker

```lua
local got = call('### Sub')
return 'worker:' .. got .. ':' .. item
```

### Sub

```lua
assert(item == nil)
```

### Tail

```lua
return 'tail-reply'
```
````

````text
worker:tail-reply:alpha
````

`### Sub` returns nothing, so the called chain falls through to `### Tail`, whose result `call` hands back to the arm.

`jump(heading)` inside an arm skips the rest of the arm's code and walks from the target through its following siblings ([sibling jumps](08-jump-and-call.md#sibling-jumps)). That walk's result becomes the arm's `.text`, and the target runs as the arm chain's next section, taking its next `sys.id`, such as `0.0.1` in the first arm:

````markdown
### Worker

```lua
jump('### Target')
error('never runs')
```

### Target

```lua
store.append('order.txt', 'Target\n')
```

### Tail

```lua
store.append('order.txt', 'Tail\n')
return 'tail-reply'
```
````

The arm's `.text` is `tail-reply`, `order.txt` holds `Target` and `Tail` on two lines, and the `error` after the `jump` never runs. When the walk from the target ends without a result, the arm reports `.text == ''`: if `### Target` only appends to `order.txt` and no section follows it, the arm's text is empty.

A `jump` from an arm into one of the worker section's own children starts a [child walk](08-jump-and-call.md#child-level-walks) over them: the target runs as a plain section with no `item` global and falls through to its following child siblings. Like a sibling target, it takes the arm chain's next `sys.id`, `0.0.1` in the first arm:

````markdown
### Worker

```lua
jump('#### Child')
```

#### Child

```lua
assert(item == nil)
```

#### ChildTail

```lua
return 'child-tail-reply'
```
````

The arm's `.text` is `child-tail-reply`.

A `jump` from an arm to a heading outside the worker section's visible set fails with the heading not-found error ([heading addresses](08-jump-and-call.md#heading-addresses)), whose message lists the worker section's visible sections. `pcall` in the arm's block cannot catch it, because a `jump` target is resolved after the block ends, so the arm fails, and the fanout with it. In the first prompt of this section, a worker section running `jump('## Parent')` fails with a message containing `not found` and the sibling `### Items`. `call` and `list_from_section` raise the same not-found error where they are called, so wrapped in `pcall` they return `false` and the arm goes on.

## Nested fanouts and arm ids

Fanouts nest: an arm's section can call `fanout` again over a collection it builds. The nested worker section resolves over the outer worker section's visible set, results place by collection index at both levels, and a nested fanout restarts `sys.index` at 1 for its own arms:

````markdown
---
name: nested
description: Each outer arm fans out again
promptforge: 0
---

# Nested

## Parent

```lua
local r = fanout('### Outer', {'a', 'b'})
return table.concat(r, ';')
```

### Outer

```lua
local inner = fanout('### Inner', {item .. '1', item .. '2'})
assert(inner[1].ok and inner[2].ok)
return item .. ':' .. table.concat(inner, ',')
```

### Inner

```lua
assert(tostring(sys.index) == string.sub(item, -1))
return item .. '!'
```
````

````text
a:a1!,a2!;b:b1!,b2!
````

`### Inner` is a sibling of `### Outer`, so it is in the outer worker section's visible set. The check in `### Inner` holds in every inner arm: `a1` and `b1` are at position 1 of their own fanouts, and `a2` and `b2` are at position 2.

### Arm ids

Every chain's sections have dotted `sys.id` values, and a chain started from another chain nests its ids under that chain's id ([chain ids under call](08-jump-and-call.md#chain-ids-under-call)). Each arm is a new child chain of the calling chain and has an arm id, a dotted id under the calling chain's id. There is one arm id per collection position, fixed by position rather than finish order: a first fanout from the main walk gives its arms the arm ids `0.0`, `0.1`, `0.2`, and so on. The arm id is also the arm's task id, which [Task handles and ids](15-tasks.md#task-handles-and-ids) covers.

Inside an arm, `sys.id` is the id of the section the arm is running, and the worker section is the first section of the arm's chain. Arms are numbered in collection order after any child chains the calling chain already started, such as called chains or earlier arms, and the calling section keeps its own `sys.id` after `fanout` returns. For a fanout from a top-level section, when the main walk has started no child chain before, counting arms from 0, the ids are:

| Section | `sys.id` |
|---|---|
| The worker section of arm K | `0.K.0` |
| The worker section of arm J of a nested fanout inside arm K | `0.K.J.0` |
| A `jump` target inside arm 0 | `0.0.1` |
| The first section of a chain that `call` starts inside arm 0 | `0.0.0.0` |
| The calling section, before and after `fanout` | Unchanged, such as `0.1` |

A fanout run inside a called chain nests its arms under that chain instead: inside the called chain `0.0`, the arms' worker sections run at `0.0.0.0` and `0.0.1.0`.

This prompt shows the ids of a nested fanout:

````markdown
---
name: tree
description: Shows arm ids in a nested fanout
promptforge: 0
---

# Tree

## Parent

```lua
local r = fanout('### Outer', {'a', 'b'})
return table.concat(r, '|')
```

### Outer

```lua
local r = fanout('### Inner', {'x', 'y'})
return sys.id .. '(' .. table.concat(r, ',') .. ')'
```

### Inner

```lua
return sys.id .. ':' .. item .. sys.index
```
````

````text
0.0.0(0.0.0.0:x1,0.0.1.0:y2)|0.1.0(0.1.0.0:x1,0.1.1.0:y2)
````

A chain started by `call` inside an arm is a child of the arm's chain: called from the first arm of a first fanout from the main walk, its first section has `sys.id == '0.0.0.0'`.

Arm `sys.id` values are the same on every run, whatever order the arms finish in, because each arm's id is given out when it starts, in collection order. For a keyed collection that is sorted key order: over `{ zeta = 1, alpha = 2, mid = 3 }`, a worker section returning `item.key .. '=' .. item.value .. '@' .. sys.index .. ':' .. sys.id` joins to this on every run:

````text
alpha=2@1:0.0.0,mid=3@2:0.1.0,zeta=1@3:0.2.0
````

### The depth cap

The [depth cap](08-jump-and-call.md#call-failures-and-the-depth-cap) of 8 applies to `fanout` as it does to `call`: each arm runs one call level deeper than its caller. A fanout whose arms would pass the cap fails with an error value of kind `lua`, after cancelling every arm already live:

````text
fanout recursion exceeded cap of 8
````

A `call` inside an arm that would pass the cap fails with `call recursion exceeded cap of 8`.
