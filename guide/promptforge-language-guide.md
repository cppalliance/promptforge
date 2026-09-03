# The Prompt Language

---

# Frontmatter and Structure

A PromptForge prompt is one markdown file, and this chapter teaches you how that file is put together: the frontmatter block at the top, the title, and the sections that divide the body. It is worth learning first because the parser checks the whole shape before anything runs, so a prompt that gets the structure right never fails halfway through a run.

## The smallest complete prompt

Here is a complete working prompt:

````markdown
---
name: greeter
description: says hi
promptforge: 1
---

# Greeter

## Say hi

Say hello.
````

Every prompt has this skeleton. A frontmatter block opens the file, a level-1 heading titles the prompt, and level-2 headings divide the body into sections.

## The frontmatter block

The file must begin with a `---` delimiter line, and a second `---` line closes the frontmatter. Between the delimiters you write YAML with three keys: `name` for the prompt's name, `description` for a short summary, and `promptforge` for the format version.

The `promptforge` key is what makes the file a promptforge prompt at all. A file without the key is not one, and the runtime refuses an unsupported major version before anything runs. This build supports major version 1, so write `promptforge: 1`.

The parser is strict here. A leading UTF-8 byte-order mark is dropped. Malformed YAML fails the parse and preserves the underlying cause. Unknown or misspelled keys are rejected at parse time rather than silently ignored, so a typo such as `desciption:` fails loudly instead of being skipped.

## Declaring input and output files

Two optional frontmatter keys declare the store files your prompt works with. The `input:` key names a file the prompt expects at start. The `output:` key names a file it leaves at finish. Each declaration pairs a store-internal `path` with a human-readable `description` that documents the file's role.

## The title

After the frontmatter comes the title: a single level-1 heading. The prompt must contain exactly one H1, and it must not be empty.

Anything written between the frontmatter and the H1 is preface. Preface has no prompt semantics, so use it for notes to human readers, never for instructions.

## Sections

Level-2 headings divide the body into named sections. Sections nest one level at a time: an H3 sits under an H2, an H4 under an H3, and so on through H6. A heading that skips a level, such as an H4 placed directly under an H2, is rejected as an orphan.

Sibling section names must be unique. Two siblings with the same name are rejected, and the error names both declaration line numbers so you can find the collision. The same name under different parents is allowed, because the nesting path differs.

## The shared library fence, structurally

One last structural rule. A prompt allows at most one `lua shared` fence, and only inside the H1 body. A second one, or one placed inside a section, fails the parse. The older `lua prompt` fence form was removed; writing it fails with a fence error that names the two valid forms, `lua` and `lua shared`. The placement rule is structural: the parser enforces it before anything runs, before the fence's contents ever matter.

---

# The Run

You can now write a well-formed prompt file, so the next question is what happens when it runs. This chapter walks a run from beginning to end: the live pass over the title, the ordered walk through the sections, how replies appear, and how a run finishes. Once you can picture a run, every other feature of the language has a place to attach.

## The preamble

When a run starts, the H1 section's Lua and prose blocks run first, in a live pass with full host access. This pass is the prompt's preamble. It is where the prompt declares which models and tools the run may use: `models.bind` and `models.default` declare model aliases resolved from capability descriptions, and `tools.bind` declares a tool alias the same way. Once the preamble finishes, those bindings are structurally frozen for the rest of the run.

A prompt with only an H1 title and no sections still runs. And a scalar `return` from the live H1 pass short-circuits the whole run: the returned value becomes the run's result, and no section ever fires.

Four calls are unavailable from the preamble: `execute`, `jump`, `fanout`, and `list_from_section`. Each fails with "only available in sections". These calls move control between sections, so they exist only once the section walk has begun.

## The section walk

After the preamble, the top-level sections run in file order. The first H2 section in the file is the entry point, and control falls through from each section to the next.

Each section runs in its own isolated, sandboxed Lua state. Only the `string`, `table`, and `math` standard libraries plus safe base functions are available. The state is created at section entry and torn down at exit, so one section's Lua cannot leak into the next.

A section that talks to the model needs a model. `models.use` selects a bound alias for one section, and the prompt-wide default covers sections that select nothing; a model-facing section with neither fails with a model-required error. Tools follow the same pattern: `tools.always` or `tools.add` scope a bound tool to the model under its local alias.

## Lua blocks and prose blocks

The content of the H1 and of each section is an alternating sequence of `lua` fences and prose blocks. The classic shape is prologue, prose, epilog: a Lua block, then a prose block, then a Lua block.

Prose is how the prompt talks to the model. Prose written under a section heading is sent to the model as its instructions, and the model's reply becomes the section's reply. Before the prose is sent, `{{ }}` placeholders in it are substituted with values.

## Replies and the run result

A scalar `return` from a section's Lua block ends the run early with that value. When the first section returns `"first"`, a later section's own `return "unreached"` is never reached. A run in which no section produces a reply finishes with the generic completion "done".

## What carries between sections

Sections are isolated in Lua, but three things roll forward through the walk. The `reply` value is seeded into each section from the previous section's final reply. The `var` table is a per-run clipboard: it is seeded into each section's Lua state on entry and read back before teardown, so the next section sees the updates. And the run-scoped `store` persists bulk state as virtual files addressed by logical string paths, shared across every section of the run.

## Moving control between sections

Fall-through is only the default. A running section can also call `execute(heading)` to run another section as a contained chain and get its final reply back, `jump(heading)` to transfer control outright, and `fanout(worker, collection)` to run a worker section once per collection member concurrently. For now, hold the picture of the walk: preamble first, then sections in file order, with the reply, the clipboard, and the store rolling forward.

---

# Sections and Blocks

You have seen the run from the outside. This chapter goes inside a section and teaches the pieces you write there: the exact fence forms, the horizontal rule that changes a section's meaning, list sections, and the shared library. These are the parts you touch in every prompt, so it pays to learn their exact shapes now.

## The two fence forms

A Lua block opens with an exact, unindented fence line. Only two forms are valid:

````markdown
```lua
reply = "hello"
```
````

````markdown
```lua shared
function shout(s)
  return s:upper()
end
```
````

The marker is recognized only as an exact, unindented opening line. Near-miss forms, and marker-looking lines nested inside longer code blocks, stay in prose. An unclosed fence is a parse error that names the phase. The removed `lua prompt` form fails with a fence error naming the two valid forms.

## The shared library

One `lua shared` fence in the H1 body defines a shared library. It is replayed as every section's first chunk, so its functions and globals are available in every section and every fanout arm. Because each section gets its own Lua state, the shared library is how you give every section the same helpers without repeating them.

## The off-walk rule

A `---` rule placed as a section's first content marks the section off-walk:

````markdown
## Helper

---

This section never runs by fall-through.
````

Fall-through skips an off-walk section. It runs only when addressed by `jump`, `execute`, or `fanout`. This is how you write subroutine sections that wait to be called.

## The comment boundary

A `---` rule anywhere else in a section is a comment boundary. Everything below it, until the next heading, is reader-only: no Lua compiles, no prose reaches the model, and no list items parse from it. Use it to keep working notes inside a prompt without affecting the run.

One formatting rule matters here. A blank line must precede a `---` rule. A prose line directly followed by `---` parses as a setext heading underline, not a rule, so `Some prose` immediately followed by `---` becomes a new section named `Some prose`.

## List sections

A section with no Lua blocks whose every nonblank prose line is a list marker is a list section. Its items are pre-parsed at load time:

````markdown
## Topics

- alpha
- beta
````

The item markers are `- `, `* `, `N. `, and `N) `. Blank lines are ignored. Empty items, non-list content, and empty list sections are parse errors.

From Lua, `list_from_section(heading)` returns a visible list section's items as an array of strings, with the bullet and number markers stripped. Over the list above, `list_from_section('## Topics')` yields the strings `alpha` and `beta`.

## Naming a section exactly

Calls such as `jump`, `execute`, `fanout`, and `list_from_section` take a heading reference. Write it exactly: one or more `#` markers, whitespace, then a non-empty name. Forms like `###Name` with no whitespace, or a bare name with no markers, are rejected, so a malformed heading can never be silently reinterpreted.

## The frozen preamble

Remember that the H1 pass runs first with full host access. The tool and model bindings declared there become structurally frozen for the rest of the run: once the preamble's Lua state is gone, nothing can add or change a binding. Sections select from what the preamble declared; they do not declare their own.

---

# Lua Globals and the Store

Every section runs sandboxed Lua, but it does not run empty-handed. This chapter teaches the globals the runtime seeds into each section, `args`, `sys`, `var`, `reply`, and `log`, plus the run-scoped `store` where a prompt keeps its bulk state. These are your everyday tools, so we take them one at a time.

## args: the run's input

Every section's Lua block can read the run's argument string through the `args` global:

````lua
log('the run was started with: ' .. args)
````

## sys: runtime metadata

Every section receives a `sys` JSON value carrying `when`, `now`, `id`, `section_name`, `execution`, and `section_count`.

The `sys.when` and `sys.now` values are the current UTC time formatted as RFC 3339 strings. The `when` value is stamped once at the walk's start, so every section agrees on when the run began, while `now` is fresh at each read.

The `sys.id` value is a run-global counter. The H1 pass keeps id 0, and every section entry and every fanout arm takes the next value, so entering the same section twice yields two distinct ids.

One field is conditional. `sys.index` exists only when the section runs as one arm of a fanout, a concurrent walk over a collection. Reading it in an ordinary walked section raises an unknown-field error. Arms of a nested fanout restart `sys.index` numbering at 1.

After a section's prose block finishes, the model's finish reason is recorded into `sys.reply_finish_reason`, so the prompt can inspect why generation stopped.

## log: checkpoints

Call `log(...)` from any section's Lua block to emit a checkpoint. Checkpoints are reported through the run's observer under the current section name, which makes them the simplest way to trace a run.

## var: the per-run clipboard

The `var` table is a per-run clipboard. It is seeded into each section's Lua state on entry and read back before teardown, so the next section sees the updates:

````lua
var.topic = 'governance'
````

Two rules keep the clipboard safe. Reassigning the `var` global itself fails the run; you mutate its fields, never replace it. And assigning a non-JSON value to a field fails, naming the field and the type: `var.f = function() end` errors because a function is not JSON data.

## reply: the rolling result

Each section entry is seeded with the previous section's final reply, and the section's own final reply replaces it for the next. You can assign `reply` directly, but the value must be nil or a string; anything else fails with a Lua error.

## store: virtual files

The run-scoped `store` persists bulk state as virtual files addressed by logical string paths, shared across every section of the run. The core operations read and write whole files:

````lua
store.write('state.txt', 'first')
store.append('state.txt', '\nsecond')
local text = store.read('state.txt')
if store.exists('state.txt') then
  log('state is present')
end
````

The call `store.write(path, text)` writes a virtual file, `store.append(path, text)` appends to it, `store.read(path)` returns its verbatim contents, and `store.exists(path)` returns true when a store file is present.

Three more operations help with larger files. The call `store.read_numbered(path)` reads a file with absolute 1-based line numbers attached. Both `store.read` and `store.read_numbered` accept optional 1-based start and end line numbers that select a range, so `store.read_numbered('a.txt', 84, 85)` returns only lines 84 to 85, numbered. And `store.glob(pattern)` lists store entries matching a wildcard, as in `store.glob("ready-*.md")`.

## untrusted: guarding re-injected content

When store content goes back to the model, wrap it first. The `untrusted(text)` global wraps store content in a guard envelope before it is re-injected, so the model treats it as data rather than instructions.

---

# Prose Substitution

Prose blocks are not static text. Before prose is sent to the model, `{{ }}` placeholders in it are replaced with live values from the run. This chapter teaches the substitution language: the namespaces, the path syntax, the escapes, and the errors. It is a small language, and learning it well keeps your prompts honest, because substitution never computes anything.

## The namespaces

Each placeholder names a namespace and, for most of them, a key:

- `{{ args }}` inserts the run's input string.
- `{{ reply }}` inserts the previous section's reply.
- `{{ item }}` inserts the current member when the section runs as an arm of a fanout.
- `{{ var.key }}` inserts a field of the `var` clipboard.
- `{{ sys.key }}` inserts runtime metadata.
- A bare name, such as `{{ kind }}`, inserts a section-local Lua global.

So `hi {{ args }}!` with the run argument `Acme Corp` reaches the model as `hi Acme Corp!`.

## Dotted paths and structured values

Dotted paths index into nested values. With `var.row = { a = 1 }`, the placeholder `{{ var.row.a }}` renders `1`. A placeholder that resolves to a whole table or array renders as compact JSON, so `{{ var.row }}` renders `{"a":1}`.

## Escapes

To emit a literal `{{`, `}}`, or backslash, escape it with a backslash. The text `\{{ args }}` renders as the literal characters `{{ args }}`.

## One pass, no arithmetic

Substitution is a single pass over prose only. Replacement output is never rescanned, so a substituted value that happens to contain `{{ }}` stays literal. No arithmetic is performed: compute in Lua, keep the result in `var` or a global, and reference it. Lua source is never substituted.

A prose block that substitutes to empty or whitespace-only text is skipped silently and never reaches the model.

## Hard errors

Substitution failures are hard errors with specific messages. The failures cover an unknown namespace or global, a missing key, a null value, a bare `{{ var }}` or `{{ sys }}`, dotted indexing into a string, an unclosed `{{`, empty path segments, and non-JSON globals.

Two placeholders have preconditions. Using `{{ reply }}` in the first section is a hard error because no prior section reply exists. Using `{{ item }}` outside a fanout arm is a hard error because no collection member exists.

## How item renders

Inside a fanout arm, `{{ item }}` renders the current collection member by type. Strings render verbatim. Numbers and booleans render in natural string form, so `1.5` renders as `1.5` and `true` as `true`. Arrays and objects render as compact JSON.

---

# Models

A prompt does not name a model directly. It describes the capability it needs, and the runtime resolves that description against the catalog. This chapter teaches the three calls that declare and select models, `models.bind`, `models.default`, and `models.use`, plus the `infer` family for direct inference from Lua. Capability-based binding is what keeps a prompt portable across catalogs, so it is worth learning as a habit from the start.

## Binding a model

Declare a model alias in the preamble with `models.bind`:

````lua
models.bind('analyst', 'careful analysis', { temperature = 0.25, max_tokens = 64, thinking = false })
````

The first argument is the local alias, the second is a natural-language capability description, and the third attaches invocation options such as `temperature`, `max_tokens`, `thinking`, and `context`. The options freeze at bind time and ride on every request that uses the binding.

## The default model

The call `models.default` designates the prompt-wide default, and it comes in two forms. The multi-argument form binds and designates in one call:

````lua
models.default("writer", "A tiny model", { thinking = false, temperature = 0 })
````

The single-argument form designates an already-bound alias: `models.default("writer")`. The two forms cannot be combined, and `models.default` may be called at most once per prompt, only during the live H1 pass.

## Selecting a model for a section

Inside a section, `models.use('analyst')` selects a bound alias for that section. It may be called at most once per section. A section that sends prose to the model needs a model from `models.use` or from the prompt-wide default; with neither, the run fails with a model-required error.

## Inspecting a binding

The call `models.get(alias)` returns an inspectable handle with `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, and `max_tokens` fields. Reading a handle does not change the model the section uses for its prose.

## Direct inference

Sometimes you want one quick model round from Lua, without prose. The call `handle:infer(prompt)` runs one tool-free inference round on the handle's frozen model, and `models.infer(prompt)` runs the same round on the section's current model. A bound alias is also callable directly as a Lua global:

````lua
local greeting = writer:infer('say hello')
````

A handle from `models.get` can run `infer` even when the section has no model binding at all, while `models.infer` fails in that case, naming the section. Neither form advertises tools, sets `reply`, or touches `sys`.

Two edge cases are worth knowing. An infer round that hits the model's length limit is reported as truncated while still returning the text produced so far. And if the model answers an infer round with tool calls, the run fails, because no tools were advertised.

## Reading which model answered

The field `sys.model` is not readable from Lua before the model scope closes; reading it in a prologue fails with an unknown-field error. After the prose runs, an epilog can read it, and `{{ sys.model }}` in later prose renders the catalog model id, not the alias.

## Environment variables

A run that needs an environment variable that is not set fails with an error naming the missing variable. A variable that is set but holds a non-Unicode value is a distinct failure.

---

# Tools

Models reach the outside world through tools, and a prompt controls exactly which tools the model can see. This chapter teaches the declaration and scoping calls, `tools.bind`, `tools.always`, and `tools.add`, plus local tools written in Lua, direct dispatch with `tool_call`, and the failure modes you will meet. Tool scoping is the prompt's main safety surface, so we build it up one call at a time.

## Declaring a tool

Declare a tool alias in the preamble with a natural-language capability description:

````lua
tools.bind('search', 'search the web')
````

The call `tools.bind` alone advertises nothing to the model; it only declares the alias. Binding resolves the description against the live catalog, and the failures are typed and specific: no match for the description, an ambiguous match listing the candidate identities, a duplicate alias, the same tool selected twice, or a picked tool absent from the live catalog. A capability description is resolved at most once per run, so repeated binds of the same description return the identical cached outcome, including identical failures.

## Scoping a tool to the model

Two calls scope a declared tool to the model under its local alias. The call `tools.always('search')` advertises the tool in every section. The call `tools.add('search')` advertises it in the current section only. To add several declared aliases at once, pass an array:

````lua
tools.add({"search", "fetch"})
````

The array form takes no per-element overrides.

You can replace the description the model sees. The call `tools.add(alias, override)` takes an override, and `tools.bind` and `tools.always` accept the same override as a trailing parameter. Precedence is the `tools.add` override over the `tools.bind` or `tools.always` override over the tool's catalog text.

The calls `tools.bind` and `tools.always` return a frozen Tool object with `name`, `description`, `parameters`, `wire_name`, and `untrusted` fields, and `tools.add` accepts Tool objects as well as alias strings.

## The tool loop

When a section's prose asks the model to use tools, the runtime loops: each tool call is dispatched and its result sent back as a tool message until the model replies with final text. Only the section's last prose block runs the full loop; earlier prose blocks run a single round. A `tools.add` called in a Lua block between prose blocks takes effect starting with the next prose block, not the one before it.

Calling `tools.add` with an alias that no `tools.bind` declared fails the run loudly, and the section prose never reaches a model. A model that calls a tool outside the section's advertised scope fails with an error listing the in-scope aliases, and the error notes when the alias was declared by `tools.bind` but not added to this section's scope.

## Local tools

You can write a tool in Lua with `tools['add_local']`:

````lua
tools['add_local']('grab', 'Grab a value', { value = 'string' }, function(args)
  return 'got ' .. args.value
end)
````

The handler runs as a Lua function in the section's own state. The parameter table is rendered to the model as a JSON schema with required properties. The handler's returned string goes back to the model verbatim and trusted. The handler can use `store` and section-global variables, but it cannot call `jump`, and a handler error fails the run with the handler's message. A local tool alias cannot collide with a `tools.bind` alias or with another local alias, and every tool schema advertised to the model is validated before it is sent.

## Direct dispatch and call counts

The call `tool_call(alias, args)` invokes any tool bound in the document directly from a Lua block, even one not scoped into the section, without widening the set advertised to the model. A `tool_call` with an alias that has no binding fails with an error listing every bound alias.

The counter `tools.calls[alias]` reads how many times the model has called a tool in the section. Reading it with an alias that was never bound is a hard error naming the bad key and listing the seeded aliases. The counter records a call even when the tool errors.

## Trusted and untrusted output

Output from a tool that marks its result untrusted is wrapped in a preface and nonce-tagged `<untrusted_input_...>` markers before the model sees it. Trusted tool output appends verbatim, and structured JSON output from a trusted tool resumes into Lua as a table.

## Validation and edge cases

Two semantic near-duplicate tools in one model-visible scope fail validation, with an error naming both aliases, both identities, and the similarity score. If you genuinely need both, isolate them in separate sections with per-section `tools.add`.

An empty final reply from the model fails the section unless a tool call preceded it and the finish reason is `stop`. A `length` finish reason returns the partial text and reports truncation. And a tool handler failure aborts the tool loop and fails the run with the tool's own error, preserving the underlying cause in the error chain.

---

# Control Flow

Fall-through runs sections in file order, but real prompts need to choose their path. This chapter teaches the two calls that move control, `jump` for transfer and `execute` for subroutines, together with the visibility rules that decide which sections you may name. Learn the visible set first; everything else follows from it.

## The visible set

A running section can name as the target of `jump`, `execute`, `fanout`, or `list_from_section` only its visible set: its sibling sections at the same heading level, excluding itself, plus its own direct child sections.

Heading references resolve on an exact level-and-name match. Zero matches is a not-found error listing only the visible set. Two matches is an ambiguity error rather than a silent pick.

## jump: transfer control

The call `jump(heading)` transfers control to a visible section, and the jumping section's remaining blocks never run:

````lua
jump('## Help')
store.write('seen.txt', 'should-not-run')  -- never runs
````

A jump clears the conversation, while `reply` and `var` carry across. Assigning `reply = nil` or a custom string before the jump steers what the target sees.

A jump to a direct child heading starts a child-level walk over the jumper's children under the same rules, and the parent walk resumes after the jumper when the child level exhausts.

## execute: a contained subroutine

The call `execute(heading)` runs a visible section as a contained chain with a fresh Lua state and conversation, and returns the chain's final reply to the caller:

````lua
local by_name = execute('## Research')
````

The call clones the caller's `var` into the child chain and discards the child's writes when the chain ends, so a subroutine cannot disturb the caller's clipboard. An optional second parameter supplies an input string that overrides the run's `args` for the chain.

## Recursion and failure

Nested `execute` and `fanout` recursion is capped at 8 levels, counting the first call. Exceeding the cap fails the call.

Suspending host calls such as `execute`, `fanout`, and `models.infer` deliver failures as ordinary Lua errors. That means you can catch them with `pcall` and continue:

````lua
local ok, result = pcall(execute, '## Research')
if not ok then
  log('research failed: ' .. tostring(result))
end
````

---

# Limits and Errors

Every run operates inside budgets, and every failure arrives in a stable shape. This chapter teaches the limits you can set, the defaults you get, and the error vocabulary you will see when something goes wrong. Knowing the failure shapes in advance is what makes a prompt debuggable.

## Capping the tool loop

The frontmatter key `max_tool_iterations` caps a section's tool-call loop:

````yaml
max_tool_iterations: 5
````

A model that keeps calling tools without converging stops after exactly that many round trips, and the run then fails with a tool-loop-exhausted error. The value must be a positive integer from 1 to 1000; zero, negative, and over-limit values are rejected at parse time.

## Default budgets

A run ships with these default limits:

- 24 tool iterations per section
- 8-way fanout concurrency
- a 16 MiB model response cap
- 64 MiB of Lua memory per section state
- 1024 Lua log events per section state
- a 120 second request timeout

A Lua block that exhausts a host resource quota fails with a typed quota error naming the exhausted resource: log events, log bytes, or instructions.

## The error kinds

A run failure is classified into one stable kind: parse, version, binding, completion, tool, store, lua, quota, substitution, cancelled, or internal. The kind tells you which layer rejected the run before you read the message.

Parse failures carry a stable classification kind and, when known, the location of the offending region. Lua compile errors name the prompt region and map back to the original source line numbers, so the error points at your file, not at generated code.

## Retrying and cancelling

Transient failures are marked retryable, so you can retry the run: transport errors, malformed responses, and backend failures with a 5xx status.

You can cancel a run with Ctrl-C. In-flight requests abort, and even an unbounded Lua loop stops, because an instruction-counting hook polls the cancel flag. The run ends with an "interrupted by Ctrl-C" error.

---

# Fanout

Some work is the same task repeated over a collection: summarize each file, grade each answer, research each topic. The call `fanout` runs a worker section once per collection member, concurrently, and hands you one result per member. This chapter teaches the call, the shape of its results, and the isolation and failure rules that make concurrency safe. It closes the set because it uses everything before it: sections, the store, control flow, and limits.

## The basic pattern

The common shape pairs a list section with a worker section:

````lua
local replies = fanout("### Worker", list_from_section("### Topics"))
````

This runs the worker once per item of the list section. The second parameter must be a collection. The retired two-string form errors and points at `list_from_section`, and numbers and booleans error as not a collection. The worker must be a worker template section, not a list section; naming a list section is a Lua error. Fanout over an empty collection is an error raised before any scheduling, because no work is likely a bug.

## The collection

Fanout accepts any Lua table as its collection. The array part iterates in order first, then the hash part iterates in undefined order, with each hash member arriving as a pair table carrying `item.key` and `item.value`. Function members and table-keyed members cannot cross into an arm.

## Inside an arm

Each concurrent run of the worker is an arm. Inside an arm, the current collection member is available as the `item` global and as the `{{ item }}` substitution seed, and the arm's 1-based position is `sys.index`. Arms of a nested fanout restart numbering at 1.

## Results

Fanout results arrive in collection order, never finish order. Each result is a structured object with four fields: `.ok`, `.text`, `.item`, and `.exhausted`. Calling tostring on a result yields its text, so `table.concat(results, ',')` joins the texts directly.

An arm that produces no output inherits the reply incoming to the parent as its text. With no incoming reply it yields empty text and still reports ok.

## Isolation

Concurrency comes from interleaving chains at I/O points, not from worker threads, and at most the run's fanout window, 8 by default, run at once.

Each arm seeds `var` from a fresh clone of the caller's snapshot, so arm writes never cross arm boundaries or reach the caller. The store is shared, with one guard: two arms of one fanout writing the same store path fail with a write-write race error, while `store.append` to one path stays legal with unspecified order.

Arms can still rendezvous through the store. They write marker files and poll with `store.glob`, and each poll iteration yields through `execute` on a no-op section so sibling arms get scheduled.

## Failure semantics

A fatal arm error fails the fanout and aborts the sibling arms; the caller can catch it with `pcall` and continue. A softer case degrades instead: an arm whose tool loop exhausts becomes an incomplete stub result with `.ok == false` and `.exhausted == true`, and the sibling arms survive.

## Control flow from an arm

A fanout arm can jump. The arm's visible set is the fanout caller's visible set minus the worker, plus the worker's children. A child walk started from an arm runs with no item seed.

Recursion depth accumulates across a fanout boundary: an arm runs one execute level deeper than its caller, so an execute or fanout near the cap of 8 trips it.

## Workers on the shelf

An off-walk section still counts in `sys.section_count`, and it can serve as a fanout worker shared by multiple sibling callers. This is the natural home for a worker: written once, skipped by fall-through, and called by whoever needs it.
