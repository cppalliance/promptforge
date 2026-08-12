# Prompt Files

A prompt file is a Markdown document with YAML frontmatter. The frontmatter must declare `name` and `description`. A `promptforge:` key identifies the file as a promptforge prompt - the runtime refuses files that lack a supported version number.

```yaml
---
name: summarizer
description: Summarize a document into bullet points
promptforge: 2
---
```

Below the frontmatter, the document has one H1 title and one or more H2 sections. Execution walks the H2 sections top to bottom in fall-through order. The H1 region runs first, resolving tools and models before any section begins.

## Minimal Prompt File

````markdown
---
name: hello
description: A greeting prompt
promptforge: 2
---

# Hello

## Greet

Say hello to the user in a friendly tone.
````

The parser compiles Lua code at parse time. A successfully parsed prompt is syntactically executable without any runtime compilation step - Lua syntax errors surface before any network call is made.

## Structural Rules

The parser enforces strict structure:

- The first and every root heading must be exactly H2.
- Sibling section names must be unique; duplicates produce a diagnostic naming both heading locations.
- Orphan deep headings (H4 under H2 with no H3) are rejected rather than silently reparented.
- Unknown frontmatter fields are rejected so misspelled keys fail loudly.
- Sections nest recursively using heading levels H2 through H6.
- Executable Lua fences must use exact unindented triple-backtick `lua` openers. Longer markers, indentation, or extra info-string words remain inert prose.

Parse errors report stable kind discriminants and optional byte spans for editor diagnostics. Lua compilation errors include absolute source-line numbers that map back to the original prompt file.

## Optional Frontmatter Fields

- `max_tool_iterations` - integer between 1 and 1000 (default: 24)
- `default_return` - string returned when execution falls off the last section
