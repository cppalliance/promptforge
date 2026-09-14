# Frontmatter and Structure

A PromptForge prompt is one markdown file, and this chapter teaches you how that file is put together: the frontmatter block at the top, the title, and the sections that divide the body. It is worth learning first because the parser checks the whole shape before anything runs, so a prompt that gets the structure right never fails halfway through a run.

## The smallest complete prompt

Here is a complete working prompt:

````markdown
---
name: greeter
description: says hi
promptforge: 0
---

# Greeter

## Say hi

Say hello.
````

Every prompt has this skeleton. A frontmatter block opens the file, a level-1 heading titles the prompt, and level-2 headings divide the body into sections.

## The frontmatter block

The file must begin with a `---` delimiter line, and a second `---` line closes the frontmatter. Between the delimiters you write YAML with three keys: `name` for the prompt's name, `description` for a short summary, and `promptforge` for the format version.

The `promptforge` key is what makes the file a promptforge prompt at all. A file without the key is not one, and the runtime refuses an unsupported major version before anything runs. This build supports major version 0, so write `promptforge: 0`.

The parser is strict here. A leading UTF-8 byte-order mark is dropped. Malformed YAML fails the parse and preserves the underlying cause. Unknown or misspelled keys are rejected at parse time rather than silently ignored, so a typo such as `desciption:` fails loudly instead of being skipped.

## The contract keys

Four optional frontmatter keys declare what the prompt needs from its host. Together they form the prompt's contract, and the host satisfies it before anything runs (see [The Run](02-the-run.md)):

- `capabilities:` lists the capabilities the prompt activates, by global id. A capability id has exactly two segments, `namespace/pack`. A bare id declares a required capability; the map form, `{ ref: namespace/pack, optional: true }`, declares one the run skips when absent, and may carry prompt-side `config` data. See [Tools](07-tools.md).
- `tools:` declares the run's tool slots, keyed by a prompt-local alias. A bare string is an exact global tool path (`namespace/pack/name`, exactly three segments); the map form, `{ want: "prose description" }`, is a fuzzy slot filled at prepare. See [Tools](07-tools.md).
- `models:` declares the run's model roles, keyed by a prompt-local label, each with a keyword set, an optional `min_context` token floor, and a description. See [Models](06-models.md).
- `args:` declares the run's typed input fields, each with a `type` (`string`, `boolean`, `integer`, or `number`), an `optional` flag, an optional `default`, and a description. A prompt with no `args:` key gets the default declaration: one optional string field named `prose`. See [Lua Globals and the Store](04-lua-globals-and-store.md).

Aliases, labels, and arg names share one grammar: `[A-Za-z][A-Za-z0-9_-]{0,63}`. These names are prompt-local; the model only ever sees them, never a global path. The parser's strictness covers the contract keys too: a malformed capability id, an unknown model keyword, or an arg default whose type differs from its declaration fails the parse with the position named.

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

