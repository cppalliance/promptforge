# Jump and Call

Sections run one after another until a block says otherwise, and this chapter shows how a block takes charge. `jump` moves the walk to another section, and `call` runs another section like a function and hands its result back. You learn how a heading reference finds its target, which sections a block can reach, how input and `var` travel into a called section, where nesting stops, how control works from the H1 pass, and how `sys.id` tracks each chain. By the end you can write prompts that branch, loop, descend into helper sections, and reuse sections like functions.

## Jump and call at a glance

By default a prompt's top-level sections run in order in the [walk](04-how-a-prompt-runs.md#the-section-walk). Two globals let a Lua block change that. `jump(heading)` transfers control outright: the block ends and the walk continues at the target. `call(heading, input)` runs another section like a function and returns its result to the block as a Lua string. Both name the target with a [heading reference](02-file-structure.md#referring-to-a-section-by-heading) such as `'## Help'`.

Here a section skips the one after it:

````markdown
---
name: triage
description: Jumps past a section it does not need
promptforge: 0
---

# Triage

## Check

```lua
var.seen = 'check'
jump('## Help')
var.seen = 'should-not-run'
```

## Accept

```lua
return 'accepted'
```

## Help

```lua
return 'helped:' .. var.seen
```
````

The result is:

````text
helped:check
````

`## Accept` never runs. `jump` checks that its argument is a string, records the target, and ends the block right there, so the line after it never runs either. It is not a [suspending call](05-lua-environment.md#calls-that-wait-and-errors-that-raise), and nothing waits on the host. The heading reference is looked up only after the block has ended, and the walk then continues at `## Help`, which reads the value `## Check` left in [`var`](05-lua-environment.md#keeping-values-in-var).

Here a section calls another one and uses its result:

````markdown
---
name: research
description: Runs a helper section and uses its result
promptforge: 0
---

# Research

## Main

```lua
local found = call('## Research')
return 'found: ' .. found
```

## Research

```lua
return 'research-reply'
```
````

The result is:

````text
found: research-reply
````

`call` is a suspending call. The block pauses at `call` while the target runs in its own [section VM](03-blocks-and-prose.md#how-the-shared-library-loads), so the caller and the called section never interleave. When the called section returns, its result becomes the value of `call`, and the caller's block and the walk carry on from there. `## Research` sits after `## Main` on purpose: `## Main` returns, which ends the run, so the walk never reaches `## Research` on its own.

Every call that takes a heading reference, [`list_from_section`](03-blocks-and-prose.md#reading-list-items-from-lua) included, resolves it with the same rules and reports the same errors. The next two sections cover those rules.

## Heading addresses

A heading reference is one or more `#` markers, whitespace, then the section's name. The number of markers is the target's heading level, and any depth works: `'## Help'` names a level-two section and `'#### Step'` a level-four one. A section matches only when both parts agree exactly: the marker count equals its level, and the name equals its name, including case and the spacing inside it. When the section `### Worker` is in reach, `'### Worker'` finds it and `'## Worker'` does not.

Whitespace around the whole reference is ignored, and any whitespace, such as several spaces or a tab, can separate the markers from the name. Whitespace inside the name is kept and must match. These three references all name `### Worker`:

````lua
local a = call('### Worker')
local b = call('  ### Worker  ')
local c = call('###    Worker')
````

A reference resolves against the calling section's visible set: its sibling sections at the same level, not counting itself, plus its own direct children. Every call that takes a heading reference uses this one set. Sibling names are unique and children sit exactly one level deeper, so a reference never matches two sections of a visible set. It matches one, or it is not found.

### Heading errors

A reference that cannot be resolved fails with a Lua error. The checks run in this order, and the first one that fails gives the message:

````text
section target must be a string, got {type}
section heading must include ### markers, got bare name: {text}
section heading has no name: {text}
section heading must have whitespace after the {markers} markers: {text}
section heading `{heading}` not found; available sections: {list}
````

- The target of `call`, `jump`, and `list_from_section` is a string. For any other value, `{type}` names its Lua type: `integer` for an integer, `number` for a float, and `nil` or `table` for those values. This check runs at the call for all three, `jump` included.
- `{text}` and `{heading}` are the reference with surrounding whitespace trimmed, shown as written. `'Worker'` gives `section heading must include ### markers, got bare name: Worker`.
- The no-name check runs before the whitespace check, so `'###'` and `'### '` both give `section heading has no name: ###`.
- `{markers}` repeats the exact `#` run you wrote. `'###Worker'` gives `section heading must have whitespace after the ### markers: ###Worker`.
- `{list}` holds each section in the caller's visible set, written as its markers plus its name and separated by commas. It never lists the caller itself or anything else in the document.

With `### Worker` visible, `'## Worker'` gives:

````text
section heading `## Worker` not found; available sections: ### Worker
````

Because the list is exactly the visible set, a not-found message is a quick way to see what a section can reach.

From `call` and `list_from_section`, every one of these errors is raised at the call, so `pcall` catches it inside the calling block:

````lua
local ok, err = pcall(call, '## Missing')
return 'caught: ' .. tostring(err)
````

The run succeeds, and its result starts with `caught: ` and contains `not found`. Left uncaught in a walked section, the error ends the run with run error kind [`Lua`](17-limits-and-errors.md#how-a-failed-run-is-classified). A `jump` reference is looked up only after its block has ended, so `pcall` in the jumping block never sees a lookup error from `jump`. The next section shows what happens instead.

## Sibling jumps

`jump('## Heading')` to a sibling moves the walk straight to that sibling. The rest of the jumping block does not run, the section's remaining blocks do not run, and every section between the jumper and the target is skipped. The walk continues at the target on the same level and falls through from it to its following siblings as usual:

````markdown
---
name: skip-ahead
description: Jumps over one section and carries var along
promptforge: 0
---

# Skip Ahead

## A

```lua
var.from_a = 'a'
jump('## C')
```

## B

```lua
error('the jump must skip B')
```

## C

```lua
assert(var.from_a == 'a')
var.from_c = 'c'
```

## D

```lua
return var.from_a .. var.from_c
```
````

The result is:

````text
ac
````

`## B` never runs. `## C` has no `return`, so the walk falls through to `## D` after it.

Only `var` crosses a jump. The target's section VM starts from the jumper's final `var`, so the jumper's writes are visible in the target, and the target's writes carry on into the fall-through after it. Locals, globals, and unread [pending prose](03-blocks-and-prose.md#the-pending-prose-buffer) stay behind, so pass anything the target needs through `var`.

The target can come before or after the jumping section, and it runs whatever it holds. A jump backward makes a loop. Guard it with a value in `var` so it ends:

````markdown
---
name: redraft
description: Loops back to an earlier section three times
promptforge: 0
---

# Redraft

## Draft

```lua
var.trail = (var.trail or '') .. 'D'
```

## Review

```lua
if #var.trail < 3 then
  jump('## Draft')
end
return 'passes: ' .. var.trail
```
````

The result is:

````text
passes: DDD
````

### What a jump does to its section

A jump is a control transfer, not a failure. Inside `jump`, the block ends by unwinding like an error, but the recorded jump wins over that unwinding: the block counts as succeeded and the walk continues at the target. The jumping section counts as completed, exactly as when it falls through, and its final `var` rolls forward to the target. A jump affects only the block that recorded it.

`pcall` cannot cancel a jump. A `jump` made inside `pcall`, directly or in a function that `pcall` runs, still records its target. `pcall` returns false and the rest of the block keeps running, but when the block ends the recorded jump takes effect anyway, and the block's own return value or error is dropped:

````lua
local ok = pcall(jump, '## C')
-- ok is false, and the block keeps running
return 'never the result'
````

When this block ends, the walk moves to `## C`, and `'never the result'` is discarded.

### When a jump target is wrong

A jump's heading reference is looked up after the jumping block has ended. If it is malformed or matches no section in the visible set, as in `jump('## Missing')`, the run fails with one of the [heading errors](#heading-addresses), such as a not-found message, and run error kind [`Lua`](17-limits-and-errors.md#how-a-failed-run-is-classified). By then the jumping section has already closed as completed, so the error is never raised inside the jumping block, and `pcall` there cannot catch it. Only the string check runs at the `jump` call itself.

### Where jump works

`jump` works in the Lua blocks of any section, including a section running as a [fanout arm](14-fanout.md#nested-fanouts-and-arm-ids). It belongs in section code, not in the load-time code of the [shared library](03-blocks-and-prose.md#the-shared-library): a `jump` there has no walk to move and fails the load with:

````text
jump is not available during shared library load
````

Inside a [local tool handler](12-tools.md#local-tools), `jump` refuses: calling it, even through a reference saved earlier, raises an error that begins `jump is unavailable inside a local tool handler`, and left uncaught that error fails the handler and is raised where the tool was called. `call` works there exactly as in a block, and `jump` works again once the outermost handler returns or fails.

## Child-level walks

Child sections never run by plain fall-through. A walk moves only across siblings, so a child runs only when something addresses it by heading, such as `jump` or `call` from its parent.

`jump` to one of the running section's direct children descends: it starts a child walk at that child. Any other visible target is a sibling move. In a child walk, the target runs and falls through its following siblings at that level. When that level runs out, the parent walk resumes at the section after the jumper:

````markdown
---
name: descend
description: Jumps into child sections and comes back out
promptforge: 0
---

# Descend

## A

```lua
var.log = 'A\n'
jump('### X')
```

### X

```lua
var.log = var.log .. 'X\n'
```

### Y

```lua
var.log = var.log .. 'Y\n'
```

## B

```lua
return var.log .. 'B\n'
```
````

The result is:

````text
A
X
Y
B
````

The parent section and the child level share `var`. The children start from the jumper's final `var`, and the parent walk resumes with the child level's last `var`, which is why `## B` sees every line. Without the `jump`, `### X` and `### Y` would never run, and the result would be `A` and `B` alone.

Child walks nest to any depth. Each descent remembers where its parent level stopped, and the levels unwind in order. If `### X` in the prompt above ends with `jump('#### P')` over two children `#### P` and `#### Q`, the order becomes A, X, P, Q, Y, B: the H4 level runs out and resumes the H3 level after `### X`, and the H3 level runs out and resumes the H2 level after `## A`.

A jump to a later child, not the first, starts the child walk there. With children `### X`, `### Off`, and `### Y`, `jump('### Off')` runs `### Off` and then `### Y`, and `### X` never runs.

A `return` inside a child walk ends the whole chain, and the parent walk does not resume. If `### X` above does `return 'x-value'`, the result is `x-value`, and neither `### Y` nor `## B` runs.

A child walk is part of the same chain as its parent, so [`sys.id`](05-lua-environment.md#run-metadata-in-sys) keeps counting through it instead of restarting. Reading `sys.id` in each section above gives `0.1` in `## A`, `0.2` in `### X`, `0.3` in `### Y`, and `0.4` in `## B`.

## Reachable sections

A section can address by heading exactly two groups: its sibling sections at the same level, not counting itself, and its own direct children. That is its visible set, and nothing else in the document is in reach. Take this outline:

````markdown
## Main
### Kid
#### Grand
## Sibling
### Niece
````

From `## Main`, the targets resolve like this:

| Target | Relation to `## Main` | Reachable |
|---|---|---|
| `'## Sibling'` | sibling | yes |
| `'### Kid'` | direct child | yes |
| `'## Main'` | itself | no |
| `'#### Grand'` | grandchild | no |
| `'### Niece'` | sibling's child | no |

Every target outside the visible set gets the not-found error. From `## Main`, `list_from_section('## Missing')` fails with a message that lists the visible set and nothing more:

````text
section heading `## Missing` not found; available sections: ## Sibling, ### Kid
````

A section is not in its own visible set, so a section that names its own heading in `call` or `jump` gets a not-found error. A section also never reaches its parent or its parent's siblings. A child section that addresses a section at its parent's level, such as a top-level section, gets a not-found error, and so does a section that addresses a sibling's child. These limits hold wherever the section runs, so a worker section in a fanout arm cannot reach its own parent either.

A running child section has a visible set of its own: its siblings and its own children. From `### X`, `call('#### Grand')` runs its child and `jump('### Y')` moves to its sibling:

````markdown
---
name: child-reach
description: A child section calls its child and jumps to its sibling
promptforge: 0
---

# Child Reach

## A

```lua
var.log = ''
jump('### X')
```

### X

```lua
local r = call('#### Grand')
var.log = var.log .. 'X:' .. r .. '\n'
jump('### Y')
```

#### Grand

```lua
return 'grand-ran'
```

### Y

```lua
var.log = var.log .. 'Y\n'
```

## B

```lua
return var.log
```
````

The result is:

````text
X:grand-ran
Y
````

From `### X`, `jump('## B')` would fail as not found, because `## B` sits at its parent's level. `### X` gets back to the top level by running out of siblings, as in any child walk.

`list_from_section` has the same reach as `jump` and `call`. From `## Main` in the outline above, `pcall(list_from_section, '### Niece')`, `pcall(list_from_section, '#### Grand')`, and `pcall(list_from_section, '## Main')` each return false.

## Called chains

`call(target, input)` takes a heading reference for the section to run and an optional input string, which the next section covers. It starts a called chain at the target. Because `call` is a suspending call, the calling block pauses at the call and resumes with the called chain's result as the return value.

A called chain runs like a walk. It starts at the target and falls through the target's following siblings until one returns or the level runs out, and that return is what `call` gives back. The caller's own walk then continues after the caller, without running those sections again:

````markdown
---
name: fall-through-call
description: A called chain falls through to the next sibling
promptforge: 0
---

# Fall Through Call

## A

```lua
var.from_call = call('## S1')
```

## B

```lua
return 'B saw: ' .. var.from_call
```

## S1

```lua
var.trail = 'S1 '
```

## S2

```lua
return var.trail .. 'S2'
```
````

The result is:

````text
B saw: S1 S2
````

The called chain starts at `## S1`, which has no `return`, so it falls through to `## S2`, whose return goes back to `## A`. The main walk then continues at `## B`, the section after the caller, and `## B` returns before the walk reaches `## S1` again.

A `return value` inside a called chain ends only that chain. The value becomes what `call` returns, the chain's remaining sections are skipped, and the outer walk keeps going. A called chain whose walk runs off its last section without returning gives the empty string. A [task](15-tasks.md#starting-a-task) or a [fanout arm](14-fanout.md#nested-fanouts-and-arm-ids) that runs off its last section gives the empty string the same way.

The outer walk stays put while a called chain runs. Wherever the chain ends, the caller resumes, and the outer walk continues at the section after the caller. If `## S1` above did `jump('## Peer')` to a section placed after `## B`, the chain would move to `## Peer`, and once it returned, `## A` would still resume and the walk would still continue at `## B`.

### Where to put a call target

Put the targets of `call` as siblings after the section whose `return` ends the run, so they run only when called. A called chain falls through like any walk, so a call target placed before other sections would pull them into its chain, and the main walk would also run the target on its own.

### Calling a child section

`call` on a direct child runs a called chain that starts at that child and falls through the child's following siblings:

````markdown
---
name: call-child
description: Calls child sections and uses their result
promptforge: 0
---

# Call Child

## Main

```lua
local r = call('### Sub')
return 'got:' .. r
```

### Sub

```lua
log('Sub ran')
```

### After

```lua
return 'after-reply'
```
````

The result is:

````text
got:after-reply
````

Children make natural call targets, since the walk never reaches them by fall-through. `call` on a later child runs the children from that child onward. With children `### Sub1`, `### Sub2`, and `### Sub3`, `call('### Sub2')` runs `### Sub2` and then `### Sub3`, and `### Sub1` never runs.

### Jumps and calls inside a called chain

A `jump` inside a called section moves within the called chain. The sections between the jumper and the target do not run, the chain keeps falling through from the target under the normal walk rules, and the chain's final result becomes what `call` returns. With `## Main` doing `local r = call('## Sub')` and `return 'main:' .. r`, and `## Sub` doing `jump('## Peer')` over a `## Skipped` section to a `## Peer` that returns `'peer-ran'`, the result is `main:peer-ran`.

A called section can also jump to its own child to start a child walk inside the call. If `## Sub` does `jump('### S1')`, then `### S1` runs and falls through to `### S2`, and the value `### S2` returns becomes the result of `call`.

Calls nest. A called section can call further sections and pass the result back up:

````lua
return call('## Inner')
````

A called section can make its own suspending calls, such as [`models.infer`](10-models.md#running-a-round-with-modelsinfer), so `## Inner` can return a model's reply and `## Sub` hands it up to its caller unchanged.

## Call input and args

`call`'s second argument is the input string for the called chain. With an input, the chain gets its own [`args`](06-arguments.md#input-basics) set to that string, and its own [`argv`](06-arguments.md#frozen-argv) parsed fresh from it and read-only, as the run's is. The caller's own `args` stay unchanged:

````markdown
---
name: call-input
description: Hands a called section its own input
promptforge: 0
args:
  query:
    type: string
---

# Call Input

## Main

```lua
return call('## Sub', '{"query":"chain"}')
```

## Sub

```lua
assert(args == '{"query":"chain"}')
assert(argv.query == 'chain')
return argv.query
```
````

Run with the argument string `{"query":"run"}`, the result is:

````text
chain
````

Inside the chain, the `args` global and a [`{{ args }}`](07-substitution.md#run-input-with-args-and-argv) placeholder in the prose of any section of the chain both read the call's input, not the run's. This makes a pipeline easy to write: one section hands its output to the next as that section's input.

````lua
return call('## Deliver', draft)
````

Here `draft` is a string the calling section built, and `## Deliver` can do `return 'delivered: ' .. args` to work on it directly.

Leave the input out, or pass nil, and the called chain inherits the caller's input whole: its `args` and its frozen `argv`, including any repair the H1 made to `argv` with the [H1 repair pattern](06-arguments.md#the-h1-repair-pattern). From the main walk that is the run's own input. A no-input `call` made inside a called chain inherits that chain's input, not the run's.

The input is a string, nil, or left out. Any other value fails the call with the first message below, where `{type}` names the value's Lua type, and a string that is not valid UTF-8 fails it with the second:

````text
input must be a string, got {type}
input must be a valid UTF-8 string
````

## The var snapshot

`call` hands the called chain a copy of the caller's `var`, taken at the moment of the call. There is nothing to pass: every `call` takes the copy on its own. The copy is deep, so the called chain starts from the caller's values, and fields set before the call are visible in the sections it runs. Writes the chain makes to `var` stay in the chain and are discarded when it ends:

````markdown
---
name: var-snapshot
description: A called section reads the caller's var but cannot change it
promptforge: 0
---

# Var Snapshot

## Main

```lua
var.shared = 'caller'
local r = call('## Sub')
assert(r == 'sub saw caller')
assert(var.child_write == nil)
return r
```

## Sub

```lua
var.child_write = 'sub'
return 'sub saw ' .. var.shared
```
````

The result is:

````text
sub saw caller
````

`## Sub` reads `var.shared` from the copy, and its write to `var.child_write` never reaches `## Main`. A called chain's results come back only through its return value, so return what the caller needs.

Write fields of `var`, and keep the `var` global itself in place. The copy is taken from that global, so after it is reassigned, for example with `var = 5`, the next `call` cannot take its copy and fails at the call with an error value of kind `lua`:

````text
the `var` global was reassigned; write `var.<field>` instead
````

## Call failures and the depth cap

Calls nest up to 8 levels deep, counting the first call. The ninth nested level fails with:

````text
call recursion exceeded cap of 8
````

A section cannot target itself, so recursion goes through two sections that target each other. This pair recurses until the depth cap stops it:

````markdown
---
name: ping-pong
description: Two sections that call each other until the cap
promptforge: 0
---

# Ping Pong

## Alpha

```lua
return call('## Beta')
```

## Beta

```lua
return call('## Alpha')
```
````

Left uncaught, the depth cap error passes up through every calling section unchanged and fails the run with run error kind [`Lua`](17-limits-and-errors.md#how-a-failed-run-is-classified).

A jump never adds a level, so `jump` can descend into child sections without spending depth. If `## Main` does `jump('### X')`, and then `### X` and `### Y` ping-pong with `return call('### Y')` and `return call('### X')`, nine sections run before the cap trips: the one reached by the jump and eight reached by calls.

### Other calls that share the cap

Each `call` adds one level toward the cap, and so does each [task](15-tasks.md#starting-a-task) and each [fanout arm](14-fanout.md#nested-fanouts-and-arm-ids). All three share one depth cap, and depth adds up across them: sections that call each other in turn add one level per call, across task boundaries too. The refusal names the call that tripped it. It reads `call recursion exceeded cap of 8` when a `call` or a `tasks.spawn` trips it, and `fanout recursion exceeded cap of 8` when a `fanout` does. A `fanout` requested from a chain already at depth 8 fails the run with exactly `fanout recursion exceeded cap of 8`, with no prefix or traceback. An arm runs one level deeper than the section that fanned out, so a `call` inside an arm that already sits at depth 8 fails with `call recursion exceeded cap of 8`.

### Catching a failed call

Any failed `call` can be caught with `pcall`. The depth cap, a target outside the caller's visible set, a called section that cannot start, and an error that ends the called chain all come back to Lua as the call's ordinary error instead of ending the run:

````lua
local ok, result = pcall(call, '## Risky')
if not ok then
  return 'fallback: ' .. tostring(result)
end
return result
````

When `## Risky` returns, `result` is its result. When it fails, the block returns `fallback: ` followed by the text of the failure.

A caught `call` failure is the called chain's own [error value](05-lua-environment.md#catching-and-inspecting-errors), unchanged. It keeps its `kind`, its message, and its kind's fields, and it is not rewrapped as a generic `call` failure. For example, when the called section ends while a [task](15-tasks.md#starting-a-task) it started is still running, `pcall(call, '## Leaky')` sees `err.kind == 'tasks_live'`, with `err.tasks` holding the task id and a message naming it.

## Control from the H1 pass

The [H1 pass](04-how-a-prompt-runs.md#the-h1-pass) runs the H1 body's Lua blocks before the walk starts, and control works from there too. From the H1, the visible set is the whole top level: every `##` section, with nothing excluded and no children.

`jump('## Heading')` from the H1 ends the pass and starts the walk at that top-level section instead of the first, skipping the sections before it:

````markdown
---
name: h1-jump
description: Starts the walk at a later section
promptforge: 0
---

# H1 Jump

```lua
jump('## Target')
```

## Skipped

```lua
error('the jump target must skip this section')
```

## Target

```lua
return 'jumped'
```
````

The result is:

````text
jumped
````

`call('## Heading')` from the H1 runs a top-level section as a called chain and returns its result, resolving the target exactly as from any section. A common use is computing a value once before the walk:

````markdown
---
name: h1-call
description: Computes an answer in the H1 pass and returns it later
promptforge: 0
---

# H1 Call

```lua
var.answer = call('## Answer')
```

## Result

```lua
return var.answer
```

## Answer

```lua
return 'called from h1'
```
````

The result is:

````text
called from h1
````

`## Answer` follows the placement rule for call targets: it sits after `## Result`, whose return ends the run, so the walk never runs it on its own.

`list_from_section` works from the H1 the same way. Over a top-level `## Items` list section holding `- one` and `- two`, `table.concat(list_from_section('## Items'), ',')` in the H1 gives `one,two`. [`fanout`](14-fanout.md#the-fanout-call), which runs a section once per member of a collection, also works from the H1 and resolves against the top-level sections, as in `fanout('## Worker', {'a', 'b'})`.

### Failures from the H1 pass

A `call` from the H1 to a heading that names no top-level section raises at the call site, so `pcall` catches it, and the message names the missing heading:

````lua
local ok, err = pcall(call, '## Nope')
return tostring(ok) .. ':' .. tostring(err)
````

The run result starts with `false:` and contains `## Nope`.

Left uncaught, a heading error from `call`, `fanout`, or `list_from_section` is an error in the H1 block. The H1 pass is a hard gate, so the run ends with run error kind [`RequirementsUnmet`](17-limits-and-errors.md#how-a-failed-run-is-classified), whose notice is the Lua error text. A `jump` from the H1 is different: its heading is looked up after the H1 block has ended, so a bad `jump` target ends the run with run error kind [`Lua`](17-limits-and-errors.md#how-a-failed-run-is-classified). In every case the message names the heading.

## Chain ids under call

[`sys.id`](05-lua-environment.md#run-metadata-in-sys) shows each `call` running as its own chain, nested under the caller. Walked sections in the run's own chain read `0.1`, `0.2`, and so on. The first `call` from the walk starts the child chain `0.0`, whose sections read `0.0.0`, then `0.0.1` on fall-through. After the call, the outer walk resumes its own `0.N` sequence:

````markdown
---
name: chain-ids
description: Shows sys.id in the walk and in a called chain
promptforge: 0
---

# Chain Ids

## Main

```lua
assert(sys.id == '0.1')
call('## Sub')
```

## B

```lua
return 'B is ' .. sys.id
```

## Sub

```lua
assert(sys.id == '0.0.0')
```

## Tail

```lua
assert(sys.id == '0.0.1')
return 'tail-reply'
```
````

The result is:

````text
B is 0.2
````

Child chains are numbered by the calling chain's own child counter, in the order they start. That counter is shared with the chain's [tasks](15-tasks.md#starting-a-task) and [fanout arms](14-fanout.md#nested-fanouts-and-arm-ids), and it does not depend on which section made the call. Calling the same section twice gives two different ids, because each `call` starts its own child chain. When `## Sub` does `return sys.id`, this block returns `0.0.0,0.1.0`:

````lua
local a = call('## Sub')
local b = call('## Sub')
return a .. ',' .. b
````

The second child of the run's chain is `0.1`, so its section reads `0.1.0` even though the calling section is itself `0.1`.

A `call` made after a fanout takes the next child slot after the arms, while the caller keeps its own id. When `## Main` runs `fanout('## Worker', {'a', 'b'})` and then `call('## Sub')`, the two arms read `0.0.0` and `0.1.0`, `## Sub` reads `0.2.0`, and `## Main` stays `0.1`.

These ids are the same on every run of the same prompt. A section that records its own id, makes a call, fans out over three members, makes a second call, and is followed by a walked section that records its id gives `0.1,0.0.0,0.1.0,0.2.0,0.3.0,0.4.0,0.2` every time.

A call from the H1 pass takes the first child slot too, so it runs as `0.0.0`. A later `call` from the first walked section then runs as `0.1.0`, and that section still reads `sys.id == '0.1'`.
