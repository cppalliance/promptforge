# Blocks and Prose

A prompt keeps its instructions and its logic side by side: you write the instructions in ordinary Markdown and put Lua in fences right beside them. This chapter shows how fences and prose split a section into blocks, how the Markdown above a fence reaches that code as `prose`, how to keep working notes out of it, how to write helper functions once for every section, and how to turn a bullet list into data your code can read, so you can lay out every part of a prompt exactly the way you intend.

## Lua blocks and prose blocks

Blocks fill two places in a [prompt file](01-what-a-prompt-is.md#what-a-prompt-file-is): the [H1 body](02-file-structure.md#the-h1-title-and-its-content), which is everything between the H1 title and the first `##` heading, and every [section](02-file-structure.md#sections-and-nesting), which is a heading with the content under it. Both are filled the same way. Lua lives only inside fences, and there are exactly two fence forms: a `lua` fence holds live code and can appear in the H1 body or in any section, and a `lua shared` fence holds the prompt's shared library, helper code that every section can use, and sits in the H1 body. Everything outside a fence is Markdown prose.

Here is the smallest prompt that uses both prose and Lua. As in [The smallest complete prompt](01-what-a-prompt-is.md#the-smallest-complete-prompt), the value a block returns becomes the run's result:

````markdown
---
name: echo-prose
description: Returns the prose written above its block
promptforge: 0
---

# Echo Prose

## Main

Say hello to the reader.

```lua
return prose
```
````

Its run result:

````text
Say hello to the reader.
````

The section holds two blocks: the paragraph is a prose block, and the fence is a Lua block. When the Lua block runs, it reads the Markdown written above it as the global `prose`, and `return prose` hands that text back.

The H1 body and every section can alternate `lua` fences and prose freely, in any number. Each fence becomes a Lua block, each run of text between fences becomes a prose block, and the blocks keep their file order: the paragraph `Before.`, a fence, and the paragraph `After.` give a prose block, a Lua block, and a prose block. Every Lua block is compiled when the prompt loads, before any block runs, and keeps its starting line in the file, so the line numbers in its errors are line numbers of the prompt file.

A Lua block's instructions are the Markdown written directly above its fence. After each heading and after each `lua` fence, the Markdown you write collects as pending prose, and the next fence receives exactly that Markdown as its `prose`. With the paragraph `First.`, a fence, the paragraph `Second.`, and a second fence, the first fence reads `First.` and the second reads `Second.`.

The classic section shape is Lua, then prose, then Lua. A `lua` fence that opens the section is its prologue and runs before the prose; blank lines before it are fine. A `lua` fence after the prose is its epilog and runs after the prose. A section can have a prologue alone, an epilog alone, or both, and they run in that order:

````markdown
---
name: three-blocks
description: A prologue, prose, and an epilog in one section
promptforge: 0
---

# Three Blocks

## Main

```lua
-- the prologue runs first, before the prose
```

Tell the reader what happens next.

```lua
return prose
```
````

Its run result:

````text
Tell the reader what happens next.
````

The most common use of this shape is a prologue that stores a value, prose that uses it, and an epilog that sends the finished prose to a model. The prologue keeps the value in `var`, a table whose values later blocks can read ([Keeping values in var](05-lua-environment.md#keeping-values-in-var)); the prose names the value with a placeholder such as `{{ var.topic }}`, which is filled in when a block reads the prose ([What substitution does](07-substitution.md#what-substitution-does)); and the epilog calls `models.infer(prose)`, which sends the text to the model and returns its reply ([Running a round with models.infer](10-models.md#running-a-round-with-modelsinfer)).

````markdown
## Explain

```lua
var.topic = 'tides'
```

Explain {{ var.topic }} in one sentence.

```lua
return models.infer(prose)
```
````

The model is asked `Explain tides in one sentence.`, and its reply is the result. A runnable prompt also declares and selects a model, the setup that [Prose and Lua](01-what-a-prompt-is.md#prose-and-lua) showed: a `models:` entry such as `writer: {}` in the frontmatter and `models.default('writer')` in a fence in the H1 body.

Prose never calls a model on its own. It only feeds the next Lua block, and nothing reaches a model until Lua sends it. A section with two prose paragraphs and two `models.infer(prose)` calls makes exactly two model calls, one for each call; the prose itself adds none.

## Writing a Lua fence

A Lua block opens with a line that is exactly three backticks followed by lowercase `lua`, starting at column one, with nothing else on the line. It closes at the first line that is exactly three backticks, and every line before that one, including any other line of backticks, is the block's Lua source. LF and CRLF line endings both work.

Only that exact opening line starts a Lua block. Every other fenced code block, such as one tagged `python` or `text`, is part of the prose: it stays in the Markdown the next block reads, and it never runs as Lua.

````markdown
---
name: code-in-prose
description: A python fence stays part of the prose
promptforge: 0
---

# Code in Prose

## Main

Explain what this code prints:

```python
print(1)
```

```lua
return prose
```
````

Its run result:

````text
Explain what this code prints:

```python
print(1)
```
````

A `text` fence stays whole inside the prose in the same way, even when it holds a `---` line.

To show Lua code, or a fence line itself, to the model as text, put it inside a code block whose opening line is not an exact fence line, such as a `markdown` fence of four backticks wrapped around the `lua` example. Only top-level exact fence lines open Lua blocks, so fence lines nested inside another code block stay prose and reach `prose` as plain text. A `lua shared` line nested the same way in the H1 body creates no shared library.

An empty `lua` block, an opening line followed at once by its closing line, is valid and does nothing. It can stand as a section's prologue.

### Fence and syntax errors

Every fence needs its exact closing line. When a fence has none, the prompt fails to load with parse error kind `Fence` ([Parse error kinds](17-limits-and-errors.md#parse-error-kinds)), and the message names the block's position:

````text
prompt `lua shared` fence is not closed
section `{section}` prologue `lua` fence is not closed
section `{section}` epilog `lua` fence is not closed
section `{section}` `lua` fence is not closed
````

The first message is for the shared library, `prologue` is for a fence that opens its section, `epilog` is for a section's last fence, and the plain form is for any other fence. `{section}` is the heading text without the `#` marks; for blocks in the H1 body, it is the H1 title.

When a block's closing line is not exactly three backticks and another `lua` fence follows, the block's source runs on into that fence, and the prompt fails to load with parse error kind `Fence` and this message:

````text
section `{section}` `lua` fence is not closed exactly
````

A Lua syntax error in any block, whether in the H1 body, a section, or the shared library, is found when the prompt loads, before any block runs. The prompt fails to load with parse error kind `Lua`, and the error keeps the Lua compiler's own diagnostic and position.

### Location labels

Error messages name the Lua block they refer to with a location label:

| Lua block | Location label |
|---|---|
| The `lua shared` fence | `prompt shared library` |
| Any block in the H1 body | ``H1 `{title}` lua`` |
| A section's first block | ``section `{name}` prologue`` |
| A section's last block, when the section also has prose | ``section `{name}` epilog`` |
| Any other block in a section | ``section `{name}` lua`` |

`{title}` is the H1 title, and `{name}` is the heading text without the `#` marks. The empty prose between two back-to-back fences counts as prose, so in a section of just two back-to-back fences the second is labeled `epilog`. An error raised while a block runs reports the absolute line in the prompt file, not a line counted from the top of the block; [Error locations in the prompt file](05-lua-environment.md#error-locations-in-the-prompt-file) shows the full layout of these messages.

## Section shapes

Where the fences sit decides a section's shape. Blank lines before a leading fence and empty text after the last fence produce no block:

| Section content | Blocks | Prologue | Epilog |
|---|---|---|---|
| Prose only | One prose block | None | None |
| One fence | One Lua block | The fence | None |
| A fence, then prose | Lua, prose | The fence | None |
| Prose, then a fence | Prose, Lua | None | The fence |
| Two fences back to back | Lua, empty prose, Lua | First fence | Second fence |
| Fences with prose between them | Lua, prose, Lua, and so on | First fence | Last fence |

A prose-only section loads cleanly and can sit beside sections that use fences; since no block reads its prose, it runs nothing. An H1 body with no content has no blocks at all.

A single `lua` fence is only a prologue, because an epilog needs prose before it, even empty prose. Two `lua` fences back to back stay two blocks around an empty prose block, so the first is the prologue and the second is the epilog. Both run in order and no model call happens, and the same holds when only blank or whitespace lines sit between them. A prologue can hold nothing but a Lua comment:

````markdown
---
name: two-fences
description: A prologue and an epilog with no prose between them
promptforge: 0
---

# Two Fences

## Only

```lua
-- prologue
```

```lua
return 'ok'
```
````

Its run result:

````text
ok
````

A section can hold several `lua` fences with prose between them. They run top to bottom in file order as separate blocks of that one section, and a global that one block assigns, a section global, stays visible to the later blocks of the same section:

````markdown
---
name: greeting
description: Two blocks in one section share a global
promptforge: 0
---

# Greeting

## Main

```lua
greeting = 'hello'
```

world

```lua
return greeting .. ', ' .. prose
```
````

Its run result:

````text
hello, world
````

The epilog is where a section acts on its prose: it can read `prose`, call the model, write to the store, and `return` a value that ends the section. In this epilog, `store.write(path, text)` saves the reply as a file in the run's store ([Writing and reading files](09-the-store.md#writing-and-reading-files)):

````markdown
---
name: save-reply
description: The epilog asks the model, saves the reply, and returns
promptforge: 0
models:
  writer: {}
---

# Save Reply

```lua
models.default('writer')
```

## Main

Suggest a name for a lighthouse cat.

```lua
local reply = models.infer(prose)
store.write('reply.txt', reply)
return 'saved'
```
````

Its run result is `saved`, and the store file `reply.txt` holds the model's reply.

With several fences, an earlier fence can also set `var` fields that the prose between the fences uses and a later fence reads. This section asks the model twice, and the second question uses the first reply:

````markdown
## Tour

```lua
var.city = 'Lisbon'
```

Name one landmark in {{ var.city }}.

```lua
var.landmark = models.infer(prose)
```

Write one sentence about {{ var.landmark }}.

```lua
return models.infer(prose)
```
````

The first question is `Name one landmark in Lisbon.`, its reply fills `{{ var.landmark }}` in the second question, and the second reply is the result.

## The pending prose buffer

Pending prose builds up after each heading and after each `lua` fence, and it stays inside its section. Every section starts with none, so prose that one section leaves unread never reaches the next section's first fence, and nothing is handed on when the run moves from one section to the next: with `## A` holding only `Prose for A.` and `## B` holding a fence, that fence reads an empty `prose`. Data crosses sections only through explicit channels, such as the store.

Pending prose is the Markdown body text only, trimmed of the blank lines around it, and the heading is never part of it: `## Run`, a blank line, and `Done.` give the prose `Done.`.

Prose that no Lua block reads is commentary. It is dropped at the end of its section without being rendered or sent anywhere, so even a placeholder naming a missing value cannot fail the run. Markdown after a section's last `lua` fence is the same: no fence reads it, its placeholders are never filled, and it never causes an error, which makes it a good place for notes to human readers.

````markdown
---
name: commentary
description: Prose that no block reads never renders
promptforge: 0
---

# Commentary

## First

{{ var.missing }} is never read here.

## Second

```lua
return 'ok'
```

Trailing {{ var.missing }} commentary.
````

Its run result:

````text
ok
````

No block ever sets `var.missing`, yet the run succeeds, because neither piece of prose is ever read. A block that never reads `prose` is untouched even by an unclosed `{{` in the prose above it.

## The prose global

Inside a Lua block, the global `prose` holds that block's pending prose as a plain Lua string, with every [placeholder](07-substitution.md#what-substitution-does) already filled in. It needs no assignment, and `return prose` hands back the finished text:

````markdown
---
name: word
description: Returns its prose after setting the value it names
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

`prose` is rendered at its first read, not when the block starts. Its placeholders are filled from the section's state at that moment, including its [`var`](05-lua-environment.md#keeping-values-in-var) values and its globals, so a value the block sets before its first read shows up in the text. That is why setting `var.word` to `'mutated'` before `return prose` renders `mutated`.

`prose` is rendered at most once per block. Every later read in the same block returns the same string, even after `var` or a global changes.

Each prose-then-fence pair in a section gets its own fresh `prose`, rendered from the Markdown right before that fence, while a string an earlier block already rendered and kept stays as it was:

````markdown
---
name: two-pairs
description: Each block reads the prose written just above it
promptforge: 0
---

# Two Pairs

## Only

First: {{ var.word }}.

```lua
var.word = 'one'
var.first = prose
```

Second: {{ var.word }}.

```lua
var.word = 'two'
return var.first .. ' ' .. prose
```
````

Its run result:

````text
First: one. Second: two.
````

The first block keeps its rendered text in `var.first`. The second block's `prose` is rendered fresh from the second paragraph with the new value of `var.word`, and the kept string does not change.

A block with no Markdown before it reads `prose` as the empty string `''`.

`prose` is read-only. Assigning to it at any time, before or after the first read, raises this Lua error:

````text
prose is read-only: assign to `var` or a section global instead
````

Put derived text in `var` or in another global.

[`models.infer(prose)`](10-models.md#running-a-round-with-modelsinfer) sends the prose written above a block to the model: the rendered text is what the model is asked, and the call returns the reply. This prompt makes one model call carrying `Say something.` and returns the reply:

````markdown
---
name: speaker
description: Sends a section's prose to the model
promptforge: 0
models:
  writer: {}
---

# Speaker

```lua
models.default('writer')
```

## Main

Say something.

```lua
return models.infer(prose)
```
````

Its run result is the model's reply.

An earlier fence can call a tool from Lua, and a later fence of the same section still sends the prose between them as usual:

````markdown
## Main

```lua
tools.call('echo', { value = 'x' })
```

Say something.

```lua
return models.infer(prose)
```
````

The model call works as usual, and its reply is the result.

## Thematic breaks

A thematic break keeps notes out of a block's prose. A `---` line resets pending prose, so only the Markdown below the last break before the next fence is captured, and the break line itself is never part of it:

````markdown
---
name: notes
description: Keeps working notes out of a block's prose
promptforge: 0
---

# Notes

## Draft

Working notes the model should never see.

---

Write the actual instructions here.

```lua
return prose
```
````

Its run result:

````text
Write the actual instructions here.
````

A break works before the first fence, as here, and between fences, where it keeps notes on an earlier step out of the next block's prose. With several breaks in a row, only the text below the last one counts.

Headings and breaks follow CommonMark, so any CommonMark thematic break line works as the reset, such as `---`, `***`, or `___` on a line of its own after a blank line. A `---` line inside any fenced code block, including a Lua comment inside a `lua` fence, is code and resets nothing.

Resetting pending prose is all a break does. It never ends a section: fences, prose, and headings below a break work as usual, and a section whose first content is a break runs like any other.

A break needs a blank line before it. A prose line directly followed by `---` is a setext heading underline, so that line becomes a new H2 section named after it:

````markdown
## S

Some prose
---

More prose
````

This is two sections: `## S`, and a second H2 section named `Some prose` whose prose is `More prose`. With a blank line between `Some prose` and the `---`, the `---` is an ordinary break.

## Blocks under the H1

The H1 body can hold live blocks too: any mix of plain `lua` fences and prose before the first `##` section, kept in file order as the H1 body's own blocks. The H1 pass runs these blocks before any section runs ([The H1 pass](04-how-a-prompt-runs.md#the-h1-pass)). A plain `lua` fence there is an ordinary live block, separate from the `lua shared` fence that may sit beside it, and a lone plain `lua` fence in the H1 body never becomes the shared library. A syntax error in one of these blocks names the location ``H1 `{title}` lua``.

Plain prose under the H1 title describes what the prompt does, and a `---` line can set it off from the title. With nothing above it in the H1 body, that break drops nothing and only separates:

````markdown
---
name: summary
description: Describes itself under the title
promptforge: 0
---

# Summary

---

Returns a fixed greeting, entirely in Lua, with no model call.

## Main

```lua
return 'hello'
```
````

Its run result:

````text
hello
````

No block reads the description, so it sends nothing to a model, and the prompt needs no `models:` entry. H1 prose meant only for human readers can stay unread like this, even when it holds a placeholder.

The H1 body and each section collect their own pending prose, so each part's prose reaches the model only through its own `models.infer(prose)` call, and those calls run in file order:

````markdown
---
name: two-turns
description: The H1 body and a section each send their own prose
promptforge: 0
models:
  writer: {}
---

# Two Turns

```lua
models.default('writer')
```

Name one planet.

```lua
var.planet = models.infer(prose)
```

## Answer

Name one ocean.

```lua
return models.infer(prose)
```
````

The model is asked `Name one planet.` first and `Name one ocean.` second, and the run's result is the second reply.

A thematic break in the H1 body works as in a section: Markdown above the last break is left out of the H1 body's prose, so a following H1 `lua` block reads only the text below it as `prose`. A `lua shared` fence below the break is still live. With `Description above.`, a `---` line, a `lua shared` fence, and `Below prose.` in the H1 body, the fence still defines the shared library, and the H1 body's only block is the prose `Below prose.`.

## The shared library

One exact `lua shared` fence in the H1 body defines the prompt's shared library. Functions and globals defined there can be used from any section's blocks, before and after the prose:

````markdown
---
name: decorate
description: A shared helper used before and after the prose
promptforge: 0
---

# Decorate

```lua shared
function decorate(value) return '<' .. value .. '>' end
```

## Only

```lua
first = decorate('input')
```

Wrap this too.

```lua
return first .. ' ' .. decorate(prose)
```
````

Its run result:

````text
<input> <Wrap this too.>
````

The same helper can wrap a model's reply: an epilog of `return decorate(models.infer(prose))` returns the reply in angle brackets. A library global, such as a table created with `captured = {}`, is readable in every section.

The fence can sit anywhere after the title and before the first `##` section, with blank lines before it if you like. It is compiled when the prompt loads, under the label `prompt shared library`. It is not one of the H1 body's blocks and does not split the prose around it, and every other block keeps its own file line numbers. A prompt without the fence has no shared library of its own.

The opening line is exactly three backticks followed by `lua shared`: lowercase, one space, at the start of the line, and nothing after it. Any other opening line is an ordinary Markdown code block in the prose, wherever it sits in the H1 body. The fence closes like a `lua` fence, at the first line that is exactly three backticks.

A prompt has at most one `lua shared` fence, and it belongs in the H1 body. Both rules are checked when the prompt loads, before anything runs, and a failure of either has parse error kind `Fence` ([Parse error kinds](17-limits-and-errors.md#parse-error-kinds)). Two or more fences fail with the first message below, and a fence anywhere else, such as in a section or in the [notes before the H1 title](02-file-structure.md#the-h1-title-and-its-content), fails with the second. The count is checked first.

````text
prompt allows at most one `lua shared` fence
`lua shared` fence is allowed only in H1
````

## How the shared library loads

Each section runs in a section VM, a fresh Lua instance of its own, and so do the H1 body's blocks. So does every fanout arm: a fanout runs a worker section once for each member of a collection, each of those runs is an arm, and the arm reads its member as the global `item` ([Inside an arm](14-fanout.md#inside-an-arm)). Before any of its own blocks run, each section VM replays the shared library, running the library's code first, which is why the library's functions and globals are available everywhere.

Each section VM gets its own fresh copy of the library's globals. The library's top-level statements run once per section VM, so they must be safe to repeat. A change to a library global in one section never reaches another section, while within one section, changes persist from block to block:

````markdown
# Counter

```lua shared
counter = 0
```

## First

```lua
counter = counter + 1 -- counter is 1
```

```lua
counter = counter + 1 -- counter is 2
```

## Second

```lua
counter = counter + 1 -- counter is 1 again
```
````

A prompt that needs no shared code leaves the `lua shared` fence out. Every section VM then replays an empty library, runs exactly the same way, and still reports a shared library load.

### What top-level library code can use

The host installs its globals before the replay, so the library's top-level code can use them as it loads: `args`, which holds the run's argument string, `sys`, `var`, `log`, `store`, the `tools` and `models` tables, and the control globals such as `jump` and `call`. In a block, the calls that wait on the host are suspending calls: `models.infer`, `models.loop`, `tools.call`, `call`, `fanout`, `user_input`, the `tasks` functions, and `store` calls ([Calls that wait and errors that raise](05-lua-environment.md#calls-that-wait-and-errors-that-raise)). The library's top-level code runs directly rather than as a block, so it cannot make suspending calls, and its `store` calls run as direct calls instead:

| In the library's top-level code | What happens |
|---|---|
| Reading `args`, `var`, and `sys` | Works |
| `store` calls, `log`, and `tools.add` | Work directly |
| `models.infer`, `models.loop`, `tools.call`, `call`, `fanout`, `user_input`, or a `tasks` function | Fails with `attempt to yield from outside a coroutine` |
| `jump` | Fails with `jump is not available during shared library load` |
| No `return`, or a `return` of a string, number, boolean, or nil | The value is discarded |
| A `return` of any other value | Fails with ``cannot return a {type} as a result`` |

These failures happen at run time, when a section VM replays the library, not when the prompt loads. A scalar `return` is discarded because loading the library produces no result, and a `return` of a table, for example, fails with ``cannot return a table as a result``.

This library writes to the store and records a checkpoint with `log` as it loads, and the section reads the file back with `store.read`:

````markdown
---
name: load-time
description: The shared library writes to the store as it loads
promptforge: 0
---

# Load Time

```lua shared
store.write('loaded.txt', args)
log('shared loaded')
```

## Result

```lua
return store.read('loaded.txt')
```
````

With the argument string `load-time args`, its run result:

````text
load-time args
````

To use a suspending call from shared code, put it inside a library function and call that function from a section's blocks:

````lua
function ask(question)
  return models.infer(question)
end
````

A block can then `return ask(prose)`. Library functions look up globals when they are called, not when they are defined, so they can use everything a block can, and their effects take hold: a library `function read_args() return args end` called from a section returns that run's argument string, and a `tools.add` call or a `var` assignment inside a library function works just as it would in the block.

Declaring a tool slot under `tools:` or a model role under `models:` gives the prompt a global of the same name, an alias global ([Tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects)). Alias globals install after the replay, so they are nil while the library's top-level code runs and present in every block after it, and a declared alias wins over a same-named global the library defines. The `tools` and `models` tables themselves are present at load, so a top-level `tools.add('search')` works.

The library can install a metatable on `_G`:

````lua
captured = {}
setmetatable(_G, { __newindex = function(_, key, value) captured[key] = value end })
````

The host sets `args` and the alias globals directly, so they never pass through the metatable's `__newindex` hook: with this library, `captured.args` stays nil in a later block while `args` works normally. The metatable keeps working in section blocks, so a block's `plain = 'x'` lands in `captured.plain`, while `prose` stays read-only and is still rendered at its first read.

In a fanout arm, `item` is installed before the replay, so the library's top-level code sees the arm's member and can set globals the worker section reads. With a library line `captured_by_shared = item`, a worker section that returns `tostring(captured_by_shared) .. '|' .. tostring(item)` gives `alpha|alpha` for the member `alpha`.

### When the library fails to load

Any failure while the library loads, such as a runtime error or one of the failures above, fails the run with run error kind `Lua` ([How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)), and the message includes the error's own text. A library holding `error('shared boom')` compiles when the prompt loads and then fails the run with a message containing `shared boom`. The kind is `Lua` wherever the replay fails, including the replay before the H1 body's blocks run.

## List sections

A section is a list section when it has no Lua block and every nonblank line is a list item. Its items are parsed when the prompt loads, at any heading depth, so Lua can read them by heading:

````markdown
## Topics

- alpha
- beta
3. gamma
````

This section's items are `alpha`, `beta`, and `gamma`, and a `### Items` [child section](02-file-structure.md#sections-and-nesting) under `## Parent` works the same way. The run still reaches a list section like any other section, and with no Lua in it, nothing runs there.

### Item markers and item text

Each item line starts with one of four markers, and each marker includes its space:

| Marker | Example line | Item |
|---|---|---|
| `- ` | `- alpha` | `alpha` |
| `* ` | `* beta` | `beta` |
| `N. ` | `1. first` | `first` |
| `N) ` | `3) third` | `third` |

`N` is one or more digits. One list can mix all four markers, and the numbers are never checked for order or starting value: `1. first`, `2. second`, and `3) third` give `first`, `second`, and `third`, and `- alpha`, `* beta`, and `- gamma` give `alpha`, `beta`, and `gamma`.

An item's text is everything after the marker and its space, with the line's outer whitespace trimmed, so `- item` and `1. item` both give `item`. Any further spaces after the marker's space stay at the start of the item text.

Blank and whitespace-only lines between items are ignored, and indentation never nests items: an indented marker line is one more flat item. `- alpha`, a blank line, `- beta`, a line of spaces, and `- gamma` give `alpha`, `beta`, and `gamma`.

### What makes a section a list

Both parts of the rule matter. A section with any Lua block keeps its bullets as ordinary prose, and a blank or whitespace-only section is an empty prose section, not a list. A bullet line inside ordinary prose does not make a list either, because any other line keeps the whole section as prose with no items: `Here is context.`, `- one incidental bullet`, and `More prose follows.` make a prose section, bullet included.

A worker section is an ordinary section, not a list, even when its prose names the member with `{{ item }}`: a `### Worker` holding a fence `return item` and the line `Do work on {{ item }}.` has no items.

A `---` break keeps commentary out of a list: only the marker lines below the last break become items, and the Markdown above it, bullets included, is commentary.

````markdown
## Items

- alpha

---

- beta
````

This list has the single item `beta`. A list that opens with a break, such as `---` followed by `- alpha` and `- beta`, has the items `alpha` and `beta`.

Every item needs text. A marker with no text after it still counts as a marker line, so a section whose lines are all markers stays a list, and the prompt fails to load with parse error kind `List` ([Parse error kinds](17-limits-and-errors.md#parse-error-kinds)), whatever the section is named:

````text
empty bullet item in list section `{section}`
````

For a `### Items` list with an empty middle item, the message is `` empty bullet item in list section `Items` ``.

## Reading list items from Lua

`list_from_section(heading)` takes a heading reference, a string such as `'## Topics'` that names a section by its level and name ([Referring to a section by heading](02-file-structure.md#referring-to-a-section-by-heading)), and returns a Lua array of that section's items in order, starting at index 1, with the markers removed:

````markdown
---
name: topics
description: Reads a sibling list section from Lua
promptforge: 0
---

# Topics

## Main

```lua
local items = list_from_section('## Topics')
return table.concat(items, ', ')
```

## Topics

- alpha
- beta
````

Its run result:

````text
alpha, beta
````

The items were parsed when the prompt loaded, so `## Topics` never has to run for `## Main` to read them. For a list of `- alpha` and `- beta`, the array has `#items == 2`, `items[1] == 'alpha'`, and `items[2] == 'beta'`, and numbered items `1. one`, `2. two`, and `3. three` give `'one'`, `'two'`, and `'three'`. The call does not suspend.

In a section or a fanout arm, `list_from_section` reaches the list sections in the caller's visible set, which is the calling section's siblings other than itself plus its own direct children ([Reachable sections](08-jump-and-call.md#reachable-sections)). From a block in the H1 body, the visible set is every top-level section, so the call can reach any of them:

````markdown
---
name: items-first
description: Reads a list section from the H1 body
promptforge: 0
---

# Items First

```lua
return table.concat(list_from_section('## Items'), ',')
```

## Items

- one
- two
````

Its run result:

````text
one,two
````

A bullet list is the usual source of a fanout collection: passing `list_from_section('### Topics')` as the collection gives one member per bullet, and each bullet's text reaches its arm as `item`, so a worker section can write `Reply about {{ item }}.` above `return models.infer(prose)` ([Collections and member order](14-fanout.md#collections-and-member-order)). With `### Topics` holding `- alpha` and `- beta`, the arms see `item` as `alpha` and `beta`.

Naming a section that has no list items, such as a prose section, fails with an error that names it, where `{name}` is the heading text without the `#` marks:

````text
section `{name}` has no pre-parsed items
````

`list_from_section('## Prose')` on a prose-only `## Prose` fails with ``section `Prose` has no pre-parsed items``. Uncaught, it ends the run like any other Lua error in that block ([How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified)).
