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
