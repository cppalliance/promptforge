# Getting Started

This chapter walks you through your first PromptForge CLI invocation, explains how input and output work, and shows how to configure a gateway for remote capabilities.

## Running Your First Prompt

The CLI binary is named `promptforge`. It has one command:

```bash
promptforge run <file.md> [input]
```

The file must be a PromptForge prompt. That means its YAML frontmatter must declare a `promptforge:` version. If it does not, the CLI refuses the file before attempting to parse it:

```text
error: prompt.md is not a promptforge prompt: its frontmatter declares no `promptforge:` version
```

A valid prompt file is read from disk, parsed, and executed in-process. The binary links the PromptForge executor directly rather than connecting to an MCP server or any other service. This makes the CLI a development tool for the edit-run loop: you edit a prompt file, run it, and see the result immediately.

The simplest invocation takes just a file path:

```bash
promptforge run prompts/hello.md
```

Prompts are addressed by file path, not by name from a catalog. There is no configuration file, no resolution rule, and no catalog lookup. Shell completion, relative paths, and `..` work as they do with any file argument.

## Input and Output

### Passing Input

The optional second argument is a raw input string that becomes the prompt's `args` value in its entirety:

```bash
promptforge run prompts/staker.md "Bloomberg"
```

The prompt body decides what that text means. The binary does not inspect, split, or coerce it. An input containing spaces must be quoted as a single shell argument.

### Capturing Output

When the prompt completes, its returned value goes to stdout. Errors go to stderr. Nothing is mixed. On success, stdout contains exactly the returned value and nothing else. On failure, nothing appears on stdout.

This clean separation means shell substitution works:

```bash
report=$(promptforge run prompts/digest.md "2026-08")
```

The variable `report` captures exactly what the prompt returned.

## Gateway Configuration

Gateway credentials come from two environment variables:

- `PROMPTFORGE_GATEWAY_URL` - the gateway base URL
- `PROMPTFORGE_GATEWAY_API_KEY` - the bearer token

There are no CLI flags for credentials. This is deliberate: secrets never appear in `argv`, where `ps` and shell history can expose them.

### Local-Only Mode

Local-only mode is the default. With neither variable set (or with empty values), the CLI runs without a gateway. The `web_fetch` tool is available, but there is no `web_search` and no remote model catalog. A prompt that makes no model calls works entirely self-contained in this mode.

### Remote Mode

Remote mode activates when both variables are set:

```bash
export PROMPTFORGE_GATEWAY_URL="https://gateway.example.com/v1"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
promptforge run prompts/search-demo.md "latest Rust news"
```

This enables the `web_search` tool and fetches the remote model catalog, so prompts can perform inference through the gateway.

Setting a key without a URL is rejected explicitly:

```text
error: PROMPTFORGE_GATEWAY_API_KEY is set but PROMPTFORGE_GATEWAY_URL is missing or empty; both are required to reach the gateway
```

## File IO

A prompt can declare input and output files. This lets callers pass file content into the prompt's store before execution and collect results from it afterward.

A minimal prompt with file IO:

````markdown
---
name: summarize_paper
description: Summarize a paper in three bullet points
promptforge: 1
input:
  path: paper.md
  description: The paper to summarize
---

# Summarize

## Run

```lua
local content = store.read("paper.md")
```

Summarize the following paper in exactly three bullet points:

{= content =}
````

Call it via the MCP server:

```json
{
  "prompt": "summarize_paper",
  "input_file": "/home/user/papers/p2996r7.md"
}
```

The server reads the file, seeds `paper.md` in the store, and the prompt accesses it through `store.read()` without knowing where the content came from.

---

Next, see [Prompt Files](./prompt-files.md) for a detailed look at how prompt files are structured and parsed.
