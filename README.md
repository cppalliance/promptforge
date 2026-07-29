# PromptForge

A runtime that executes analysis pipelines defined in a single markdown file. The
markdown is the program, the model is the CPU.

## Workspace

- `crates/promptforge-core` - library: prompt parser, HTTP client, section execution
- `crates/promptforge-cli` - binary: the `promptforge` command-line tool

## Build

```
cargo build
cargo test
```

## Run

```
export ANTHROPIC_API_KEY=sk-ant-...
cargo run -p promptforge-cli -- run prompts/hello.md
```

Prints the model's response to the first section of the prompt.

## Prompt file anatomy

```
---
name: hello
description: Say hello
version: 1
---

# Title

Human-readable description (not executed).

## Section

Prose the model reads. Sections fall through in file order; context clears on
each transition.
```
