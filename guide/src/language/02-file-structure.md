# Prompt File Structure

Every prompt follows one exact layout, and once you know it you can write any prompt file without guessing: frontmatter keys that are checked before anything runs, a single title, and a tree of sections that code can name by heading. This chapter gives you each rule the file format enforces, the error each broken rule produces and what that error names, and the one string form every call uses to point at a section, so a prompt you write parses on the first try and a failure tells you exactly what to fix.

## What a prompt file looks like

A [prompt file](01-what-a-prompt-is.md#what-a-prompt-file-is) is one Markdown file whose parts come in a fixed order: YAML frontmatter between two `---` lines, one H1 title, optional content under the title called the H1 body, then `##` sections. Each section holds prose, `lua` fences, or both; how fences and prose fill a section is the subject of [Lua blocks and prose blocks](03-blocks-and-prose.md#lua-blocks-and-prose-blocks).

Here is the smallest prompt that runs:

````markdown
---
name: greeter
description: Returns a greeting
promptforge: 0
---

# Greeter

## Say hi

```lua
return 'hello'
```
````

Its run result:

````text
hello
````

The frontmatter keys `name` and `description` are all a file needs to parse, and `promptforge: 0` is what lets it run, as [The promptforge version](#the-promptforge-version) explains, so every example in this book carries all three.

This prompt uses every part in order, with a line of prose in the H1 body and a section that holds both prose and a `lua` fence:

````markdown
---
name: layout
description: Shows every part of a prompt file in order
promptforge: 0
---

# Layout

This prompt returns one fixed line.

## Answer

The Lua below returns the line.

```lua
return 'laid out'
```
````

Its run result:

````text
laid out
````

A prompt has exactly one H1 heading, written as a real `# Title` line with non-empty text. Plain prose never stands in for the title, so a section always has an H1 above it. Breaking the rule fails the parse with parse error kind `Structure` and a message that states the rule:

````text
prompt requires an H1 title
prompt must contain exactly one H1 title
prompt H1 title must not be empty
````

The first message means the file has no H1, the second means it has more than one anywhere in the body, even above the title, and the third means the title is empty or only whitespace. These messages name no section and no line. Parse error kinds are explained in [Parse error kinds](17-limits-and-errors.md#parse-error-kinds).

H2 headings placed after the H1 divide the body into named sections. Only headings after the H1 at level 2 or deeper become sections, and the top-level sections are the H2s, kept in file order. Each section has a name taken from its heading text, a level, and its own prose and Lua. No particular name is required; `## Main` is a common choice. The headings `## First` and `## Second` give two sections named `First` and `Second`, both at level 2. A deeper heading such as `### Author note` under `## Prepare` belongs to `Prepare` and is not another top-level section.

## The frontmatter header

Every prompt file opens with frontmatter: a `---` line as the very first line of the file, the YAML keys, then a closing `---` line, with the Markdown body after it. Each delimiter line is compared after trimming surrounding whitespace.

The usual header holds three keys, `name`, `description`, and `promptforge: 0`, and a header with just these three parses and runs:

````markdown
---
name: header-only
description: Shows the usual three-key header
promptforge: 0
---

# Header Only
````

Further keys sit alongside the three when a prompt needs them. A prompt that calls a model, for example, adds a `models:` key, one of the [contract keys](01-what-a-prompt-is.md#the-prompt-and-its-host):

````markdown
---
name: live-h1
description: d
promptforge: 0
models:
  writer: {}
---

# Live H1
````

A file whose first line is not the `---` delimiter fails to parse with parse error kind `Frontmatter`, and frontmatter that is never closed fails the same way:

````text
file must begin with a --- frontmatter delimiter
frontmatter was not closed with ---
````

The missing-opener error names no prompt, because the name is not known yet.

## Name and description

Every prompt has two required frontmatter strings: `name:` identifies the prompt, and `description:` summarizes it in one line. Neither has a default, and both are kept exactly as written: `name: shared_library` reads back as `shared_library`, and `description: Exercise an H1 shared library and nested author prose` reads back word for word.

````yaml
name: greet
description: Greet the named input using a Lua-computed value
promptforge: 0
````

The `name:` string need not match the file name. A file saved as `research-person.md` can declare `name: research_person`, and one saved as `echo.md` can declare `name: echo`. Common values are lowercase identifiers such as `echo`, `greet`, `analyst_example`, and `vfs-end-to-end`. Every parse error found after the frontmatter reports this name, so you can tell which prompt failed.

The `description:` string is a one-line, free-text sentence, kept verbatim, and hosts show it in prompt listings. A plain unquoted sentence with spaces and commas works, such as `description: Research a person from the open web and return a concise, factual summary.`

Leaving out either key fails the parse with parse error kind `Frontmatter`, and the message names the missing field. The message follows the frontmatter form `invalid frontmatter: {detail}` described in [Frontmatter rules and errors](#frontmatter-rules-and-errors), with one of these details:

````text
missing field `name`
missing field `description`
````

## The promptforge version

The `promptforge:` key declares which major version of the engine the file targets, written `promptforge: 0`. Its value is a non-negative integer, and its presence marks the file as a PromptForge prompt. `0` is the only major this build runs.

````markdown
---
name: version-zero
description: Runs because it declares major 0
promptforge: 0
---

# Version Zero

## Only

```lua
return "ran"
```
````

Its run result:

````text
ran
````

Parsing accepts a file with or without the key, but the version is checked when the run starts, before anything executes. A file that declares `promptforge: 0` runs. A file that declares any other major still parses, but its run fails on its first step with run error kind `Version`, where `{N}` is the declared major:

````text
unsupported promptforge version: {N} (this build supports major 0)
````

That failure comes before any section executes, so nothing runs and nothing is reported, and the prompt's own `return` never runs. It is not retryable, and the prompt never falls back to running as major 0. Run error kinds and retrying are explained in [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified).

Every prompt needs the `promptforge:` key. A file whose frontmatter holds only `name` and `description` still parses, even with a plain prose section, but its run fails on the first step, before anything executes, with parse error kind `Structure` and this message:

````text
not a promptforge prompt: no promptforge version
````

The run error kind for this failure is `Parse`, and the error carries the prompt's frontmatter `name`.

## Input and output files

A prompt that works on files can declare one `input:` file it expects in the store when it starts and one `output:` file it leaves there when it finishes. The store is the run's own set of virtual files, which [What the store is](09-the-store.md#what-the-store-is) introduces along with `store.read` and `store.write`.

Each declaration is a mapping with exactly two required strings: `path`, a filename in the store such as `paper.md` or `report.md`, and `description`, a readable purpose. These are the only keys a declaration accepts.

````yaml
input:
  path: paper.md
  description: The input paper
output:
  path: report.md
  description: The output report
````

Both keys are optional and stay out of prompts that do not work on files, so a frontmatter with only `name`, `description`, and `promptforge: 0` declares neither. The declarations stay with the parsed prompt for the host to read: `input:` with `path: paper.md` and `description: The input paper` reads back as exactly that path and description. Together they tell the host which store file to put in place before the run and which one to collect after it.

The run itself never acts on either declaration, so the prompt writes its declared output file itself:

````markdown
---
name: copy-paper
description: Copies the input paper into the output report
promptforge: 0
input:
  path: paper.md
  description: The input paper
output:
  path: report.md
  description: The output report
---

# Copy Paper

## Copy

```lua
store.write('report.md', store.read('paper.md'))
return 'copied'
```
````

With a host that puts `paper.md` in the store first, its run result is:

````text
copied
````

Afterwards `report.md` holds the paper's text, ready for the host to collect. Because the run never checks the `output:` declaration, a prompt that declares `report.md` but never writes it still runs to success; the missing file shows up only when the host goes to collect it.

## Frontmatter rules and errors

The frontmatter recognizes exactly ten top-level keys. Only `name` and `description` are required to parse, and every other key has a default when omitted:

| Key | To parse | When omitted | Taught in |
|---|---|---|---|
| `name` | required | the parse fails | [Name and description](#name-and-description) |
| `description` | required | the parse fails | [Name and description](#name-and-description) |
| `promptforge` | optional, needed to run | absent, and the run refuses | [The promptforge version](#the-promptforge-version) |
| `max_tool_iterations` | optional | the default round cap | [The round cap](11-conversations.md#the-round-cap) |
| `input` | optional | absent | [Input and output files](#input-and-output-files) |
| `output` | optional | absent | [Input and output files](#input-and-output-files) |
| `capabilities` | optional | no capabilities | [Declaring capabilities](12-tools.md#declaring-capabilities) |
| `tools` | optional | no tool slots | [Tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects) |
| `args` | optional | the default argument declaration | [Arg declarations](06-arguments.md#arg-declarations) |
| `models` | optional | no model roles | [Declaring roles](10-models.md#declaring-roles) |

A frontmatter of just `name: x` and `description: d` parses on that basis, with no capabilities, no tool slots, no model roles, and the default argument declaration. The last four rows are the contract keys; each links to the chapter that explains its entries.

A prompt file can be saved with or without a leading UTF-8 byte order mark, and with either LF (Unix) or CRLF (Windows) line endings. The byte order mark is dropped before the check for the opening `---` line, and `lua` and `lua shared` fence openings, fence closings, and the Lua code inside them treat CRLF exactly like LF.

Every frontmatter failure has parse error kind `Frontmatter` and one message form, where the detail is the YAML reader's own message:

````text
invalid frontmatter: {detail}
````

This form covers a YAML syntax slip, a value of the wrong type, a missing required key, an unknown key, and an out-of-range `max_tool_iterations`. When the YAML error has a position, the error reports it as a line and column counted from 1 at the top of the file, where the opening `---` is line 1, so a YAML slip on the fourth line of the file is reported at line 4. The error carries no prompt name, because the name is read from the frontmatter itself.

Only the recognized keys are accepted, at every level. A misspelled or unknown key fails the parse instead of being ignored, and the message names the key. This holds at the top level, inside an `input:` or `output:` declaration, and inside a capability `ref:` entry, an arg entry, or a model role. For the top level and the file declarations, the detail names the key and then lists the keys that are accepted there:

````text
invalid frontmatter: unknown field `{key}`, expected one of ...
````

## Names for aliases, roles, and args

Three of the contract keys are maps from a name to a declaration, and all three names follow one name grammar. The keys under `tools:` are tool aliases, the keys under `models:` are model role labels, and the keys under `args:` are arg names:

````yaml
capabilities:
  - promptforge/web
tools:
  search: promptforge/web/search
  fetch: promptforge/web/fetch
models:
  writer: {}
  analyst:
    keywords: [frontier, thinking]
args:
  use_mcp:
    type: boolean
    default: true
````

The names here are `search` and `fetch`, `writer` and `analyst`, and `use_mcp`. The values on the right belong to their own chapters:

- Under `tools:`, each key is a tool alias and each value is a tool path such as `promptforge/web/search`; the model calls a tool by its alias, never by its tool path, as [Tool slots and Tool objects](12-tools.md#tool-slots-and-tool-objects) explains.
- Under `models:`, each key is a model role label with its role declaration, covered in [Declaring roles](10-models.md#declaring-roles).
- Under `args:`, each key is an arg name with its declaration, covered in [Arg declarations](06-arguments.md#arg-declarations).

The name grammar is `[A-Za-z][A-Za-z0-9_-]{0,63}`: 1 to 64 ASCII characters, a letter first, then letters, digits, `_`, or `-`. Letters are ASCII only. A 64-character name parses. Names such as `search`, `fetch`, `writer`, `analyst`, `use_mcp`, `limit`, and `query` all fit. An alias is the only name a model ever sees for a tool slot or a model role.

Each of `tools:`, `models:`, and `args:` is a YAML map keyed by alias, role label, or arg name, and each key appears once within its map. Every name is checked when the prompt loads, the grammar first and uniqueness second. All of these failures have parse error kind `Frontmatter`, and each message is the detail inside `invalid frontmatter: {detail}`:

````text
invalid tool alias `{key}`: expected [A-Za-z][A-Za-z0-9_-]{0,63}
invalid model role label `{key}`: expected [A-Za-z][A-Za-z0-9_-]{0,63}
invalid arg name `{key}`: expected [A-Za-z][A-Za-z0-9_-]{0,63}
duplicate {kind} `{key}`: contract map keys must be unique
invalid type: {found}, expected a map of {kind} keys to declarations
````

Here `{key}` is the name as written, `{kind}` is `tool alias`, `model role label`, or `arg name`, and `{found}` is the YAML reader's description of the value given when a key holds something other than a map. Every error from these keys reports its line and column in the frontmatter.

The same grammar applies when Lua passes an alias to `tools.add`, `tools.always`, `tools.add_local`, `models.use`, or `models.default`, and `models.get` applies it to a name that matches none of the prompt's model roles. So uppercase, lowercase, mixed case, snake_case, kebab-case, trailing digits, and single-letter aliases all work, such as `a`, `A`, `search`, `web_fetch`, `web-fetch2`, and a 64-letter alias:

````lua
models.default('writer')
tools.add('search')
````

An alias is a plain name from the grammar, and a tool path such as `promptforge/web/search` is the value the slot holds. When a call gets an alias outside the grammar (empty, longer than 64 characters, not starting with a letter, or holding any other character), it raises a Lua error at the call. The message quotes the alias in double quotes and states the pattern:

````text
invalid alias "{alias}": expected [A-Za-z][A-Za-z0-9_-]{0,63}
````

Lua code can catch this error with `pcall`, as [Catching and inspecting errors](05-lua-environment.md#catching-and-inspecting-errors) shows. Left uncaught, it fails the run with run error kind `Lua`, unless it reaches the H1 body's own Lua, as [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) explains.

## The H1 title and its content

The H1 heading's text is the prompt's title. Inner spaces and case stay exactly as written, surrounding whitespace is trimmed, and inline code or other markup keeps only its text: `# Demo Title` gives the title `Demo Title`, and `# Phase Boundaries` gives `Phase Boundaries`. The title is separate from the frontmatter `name`: `# Greeter` with `name: greeter` has the title `Greeter` and the name `greeter`.

Lua in the H1 body runs under the title, so `sys.section_name` there reads it:

````markdown
---
name: title-demo
description: Returns its own title
promptforge: 0
---

# Title Demo

```lua
return sys.section_name
```
````

Its run result:

````text
Title Demo
````

The H1 body is everything between the H1 heading and the first section heading, and it is optional. It can hold any sequence of `lua` fences and prose, plus at most one `lua shared` fence, or nothing at all, leaving the H1 as a title only. A `lua` fence and prose fill the H1 body the same way they fill a section, as [Lua blocks and prose blocks](03-blocks-and-prose.md#lua-blocks-and-prose-blocks) explains. The `lua shared` fence holds the prompt's shared library, helper code every section can use, described in [The shared library](03-blocks-and-prose.md#the-shared-library). A lone plain `lua` fence in the H1 body is ordinary Lua that runs, never the shared library.

````markdown
---
name: decorated
description: Wraps a word in angle brackets
promptforge: 0
---

# Decorated

Wraps one word using a helper from the shared library.

```lua shared
function decorate(value) return '<' .. value .. '>' end
```

## Answer

```lua
return decorate('hello')
```
````

Its run result:

````text
<hello>
````

Nothing in the H1 body becomes a section. The prose line under `# Decorated` and the `lua shared` fence leave this prompt with exactly one section, `Answer`.

Free notes, prose or fenced code blocks, can sit between the frontmatter and the H1. These notes before the H1 title have no meaning to the prompt, and headings in them never become sections:

````markdown
---
name: with-notes
description: Keeps editing notes above the title
promptforge: 0
---

Editing notes for this prompt live here and change nothing.

# With Notes

## Answer

```lua
return 'ok'
```
````

Its run result:

````text
ok
````

Two rules still reach the notes before the H1 title. An H1 there counts toward the one-H1 rule, so the file fails with "prompt must contain exactly one H1 title". The shared library belongs in the H1 body, so a `lua shared` fence in the notes fails the parse with parse error kind `Fence`:

````text
`lua shared` fence is allowed only in H1
````

A prompt needs no minimum number of sections. A prompt with an H1 and no `##` sections at all parses with no sections and runs, with or without a Lua `return`:

````markdown
---
name: only-a-title
description: A complete prompt with no sections
promptforge: 0
---

# Only a title

Text.
````

## Sections and nesting

Sections nest by heading level: H3 under H2, H4 under H3, and so on down to H6, each heading exactly one level deeper than its parent. Section heading levels run from 2 through 6. A nested heading becomes a child section of the section above it, with its own level and prose, and sections that share a parent are siblings. The headings `## A`, `### B`, and `#### C` give section `A` with child `B` at level 3, which has child `C` at level 4.

A section's content ends at the next heading of any level, so a parent's own prose and Lua are only the lines between its heading and its first child heading:

````markdown
---
name: nesting
description: Shows a section with a child section
promptforge: 0
---

# Nesting

## Prepare

Gather the subject.

### Author note

A child section with its own prose.

## Finish

```lua
return 'finished'
```
````

This prompt has two top-level sections, `Prepare` and `Finish`. `Prepare` has one child section, `Author note`, at level 3, and `Prepare`'s own prose is only `Gather the subject.`; the line under `### Author note` belongs to the child.

A section's name is its heading text. Inline code keeps its text without the backticks, other inline markup keeps only its text, and surrounding whitespace is trimmed, so `` ## Run `fetch` `` gives the section name `Run fetch`. Every section heading needs text after trimming, because a section's name is how it is addressed while the prompt runs. An empty heading fails the parse with parse error kind `Structure`:

````text
an H{level} section heading must not be empty
````

Heading levels step down one at a time, and the first section is an H2. A heading that skips a level, such as an H4 directly under an H2 or a first section written as H3 or H4, fails the parse with parse error kind `Structure`:

````text
section `{name}` is an orphan H{level} heading with no parent H{parent}
````

Here `{parent}` is one level below the heading the orphan sits under, so for an H4 directly under an H2 the message ends "no parent H3". The deep heading is never moved under a shallower section.

Sibling section names are unique, while the same name can repeat under different parents, so this parses:

````markdown
## A

### S

## B

### S
````

Two siblings with the same name fail the parse with parse error kind `Structure`, with both lines counted from the top of the file:

````text
duplicate sibling section name `{name}`: first declared at line {first}, again at line {second}; sibling section names must be unique
````

The error also reports the prompt's `name` and the second heading's line and column, and marks that heading in the file. In a prompt named `dup` whose second same-named sibling sits at line 12, the error names `dup` and points at line 12, column 1.

The body is read as CommonMark, so Markdown's own heading rules decide what is a heading. A line starting with `#` inside a fenced code block, `lua` fences included, is code and never a heading or a section. One CommonMark rule can also turn a line of prose into a heading when a `---` line sits directly under it; [Thematic breaks](03-blocks-and-prose.md#thematic-breaks) covers this pitfall.

## Referring to a section by heading

Calls that target a section take a heading reference: the section's heading written as a string with its `#` markers, such as `'## Main'` for an H2 sibling or `'### Worker'` for an H3 child. The section a heading reference names is its target, and a target is always named by its heading reference.

These calls take one:

- `list_from_section` reads the items of a list section, as [Reading list items from Lua](03-blocks-and-prose.md#reading-list-items-from-lua) shows.
- `jump` and `call` move to another section, as [Jump and call at a glance](08-jump-and-call.md#jump-and-call-at-a-glance) shows.
- `fanout` runs one section once per member of a collection, as [The fanout call](14-fanout.md#the-fanout-call) shows.
- `tasks.spawn` starts a section as a task, as [Starting a task](15-tasks.md#starting-a-task) shows.
- `tools.allow_tasks` lets the model start sections through its `task` built-in, as [Letting the model start tasks](15-tasks.md#letting-the-model-start-tasks) shows.

````lua
call('## Inner')
jump('### S1')
fanout('### Worker', {'alpha'})
list_from_section('## List')
tasks.spawn('## Sibling')
tools.allow_tasks({ '## Child' })
````

The model names a target the same way in the arguments of the `task` built-in:

````text
{ "target": "## Child" }
````

This prompt reads a list section by its heading reference from Lua in the H1 body:

````markdown
---
name: items
description: Reads a list section by its heading reference
promptforge: 0
---

# Items

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

A heading reference names its target by exact level and name, matched as a pair against the sections the calling code can address, its visible set, which [Reachable sections](08-jump-and-call.md#reachable-sections) defines. The number of `#` markers equals the target's heading level, so an H3 named `Worker` is `'### Worker'`. When nothing matches, the call raises a Lua error:

````text
section heading `{heading}` not found; available sections: {list}
````

Here `{heading}` is the trimmed reference, and `{list}` gives only the visible sections, each written as its own heading reference such as `## Main`, joined by a comma and a space.

A heading reference is one or more `#` markers, whitespace, then a non-empty name, and it is never silently reinterpreted. Whitespace around the whole reference and around the name is trimmed, so `' ##  Main '` names `## Main`. A reference that has no markers, has no whitespace after its markers, or is markers only raises a Lua error, where `{text}` is the trimmed reference and `{markers}` is its run of `#` marks:

````text
section heading must include ### markers, got bare name: {text}
section heading must have whitespace after the {markers} markers: {text}
section heading has no name: {text}
````

Left uncaught, the not-found error and each of these errors fails the run with run error kind `Lua`, unless it reaches the H1 body's own Lua, as [How a failed run is classified](17-limits-and-errors.md#how-a-failed-run-is-classified) explains.
