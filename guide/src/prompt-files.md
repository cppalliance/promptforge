# Prompt Files

A prompt file is a Markdown document with YAML frontmatter. The frontmatter must declare `name` and `description`. A `promptforge:` key identifies the file as a promptforge prompt - the runtime refuses files that lack a supported version number.

```yaml
---
name: summarizer
description: Summarize a document into bullet points
promptforge: 1
---
```

Below the frontmatter, the document has one H1 title and zero or more H2 sections. A prompt with H2 sections walks them top to bottom in fall-through order. A prompt with no H2 sections executes the H1 blocks and returns the model reply. The H1 region always runs first, resolving tools and models before any section begins.

## Minimal Prompt File

````markdown
---
name: hello
description: A greeting prompt
promptforge: 1
---

# Hello

## Greet

Say hello to the user in a friendly tone.
````

The parser compiles Lua code at parse time. A successfully parsed prompt is syntactically executable without any runtime compilation step - Lua syntax errors surface before any network call is made.

## Structural Rules

The parser enforces strict structure:

- When H2 sections are present, the first and every root heading must be exactly H2.
- Sibling section names must be unique; duplicates produce a diagnostic naming both heading locations.
- Orphan deep headings (H4 under H2 with no H3) are rejected rather than silently reparented.
- Unknown frontmatter fields are rejected so misspelled keys fail loudly.
- Sections nest recursively using heading levels H2 through H6.
- Executable Lua fences must use exact unindented triple-backtick `lua` openers. Longer markers, indentation, or extra info-string words remain inert prose.

Parse errors report stable kind discriminants and optional byte spans for editor diagnostics. Lua compilation errors include absolute source-line numbers that map back to the original prompt file.

## The `---` Marker

A `---` thematic break inside a section carries one of two roles, decided by position.

As a section's first content (only blank lines before it), the marker takes the section **off the walk**: the top-to-bottom walk skips it entirely, and it runs only when addressed directly by `execute`, `jump`, or `fanout`. Content below the marker executes normally. This is how a shared worker or a list section lives as a top-level section without running in the walk:

````markdown
## Main

```lua
local reply = execute("## Helper")
```

## Helper

---

```lua
return "helper reply"
```
````

Anywhere else, the rule is a **comment boundary**: everything below it until the next heading is reader-only. No Lua below it compiles or runs, no prose below it reaches the model, and no list items parse from it. The two roles compose - a section may carry the off-walk marker at the top and a later rule starting a comment region. On the H1 only the comment role applies.

An off-walk list section is the natural home for a shared item list. `list_from_section("## List")` returns a list section's pre-parsed items (bullets or numbered lines) as a Lua array of strings without running the section. It resolves a heading string (or a Section object from `tasks`) against the caller's sibling sections plus its own direct children; anything else - the parent, nieces and nephews, grandchildren, the caller itself - is not found, and the error lists only the visible sections. Naming a section with no pre-parsed items is an error.

Authoring note: the marker is recognized only as a genuine Markdown thematic break. After a prose line it needs a blank line before it - a text line immediately followed by `---` is a setext heading underline, not a marker. After a heading or a fence it stands alone.

## Optional Frontmatter Fields

- `max_tool_iterations` - integer between 1 and 1000 (default: 24)

## H1-Only Prompts

A prompt with no H2 sections is valid. The H1 blocks execute, the model reply becomes the run result, and no section walk occurs:

````markdown
---
name: summarize
description: Summarize the input
promptforge: 1
---

# Summarize

```lua
models.default("m", "A model suited for careful analysis")
```

Summarize this text in one paragraph.

{{ args }}
````

## Shared Libraries

A `lua shared` fence in the H1 region defines a library compiled once at parse time and replayed into every section VM as its first chunk - before any of the section's own Lua blocks run. The replay runs with the full section environment installed (`args`, `sys`, `var`, `reply`, `store`, `log`, the `tools`/`models` tables, and the control globals), so top-level shared code may use them at load. Two exclusions apply: the captured tool/model alias globals install only after the replay (a declared alias wins over a same-named shared global), and `jump` during the load is a hard error. A scalar top-level return is discarded - the replay loads a library, it does not produce the section's result.

## Input and Output Declarations

A prompt can declare a file it expects to find in the store and a file it will leave there:

```yaml
---
name: gate_paper
description: Produce a gating report for a paper
promptforge: 1
input:
  path: paper.md
  description: The paper markdown to analyze
output:
  path: report.md
  description: The gating report produced by analysis
---
```

Both `input` and `output` are optional. A prompt may declare one, both, or neither.

- `path` is the store-internal filename the prompt uses in Lua (`store.read('paper.md')`)
- `description` documents the file's purpose and appears in MCP tool listings via `list_prompts`

The declarations are metadata only. The runtime does not enforce that the prompt actually reads the input or writes the output - they tell callers what the prompt expects and produces.
