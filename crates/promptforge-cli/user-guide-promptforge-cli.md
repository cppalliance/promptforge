# promptforge-cli User Guide

`promptforge` is a command-line tool that runs PromptForge prompt files in a single process. Point it at a prompt file, and it parses the sections, executes them top to bottom, and prints the returned value. No server to start, no connection to manage, no configuration to write. You edit a prompt, run it, and see what it produces. This guide covers every capability the CLI provides, from the first invocation to gateway configuration and cancellation.

## Running Your First Prompt

The binary is named `promptforge`. It has one command:

```bash
promptforge run <file.md> [input]
```

The file must be a PromptForge prompt. That means its YAML frontmatter must declare a `promptforge:` version. If it does not, the CLI refuses the file before attempting to parse it:

```
error: prompt.md is not a promptforge prompt: its frontmatter declares no `promptforge:` version
```

A valid prompt file is read from disk, parsed by the core parser, and executed in-process. The binary links the PromptForge executor directly rather than connecting to an MCP server or any other service. This is a development tool for the edit-run loop: you edit a prompt file, run it with `promptforge run`, and see the result immediately.

The simplest invocation takes just a file path:

```bash
promptforge run prompts/hello.md
```

Prompts are addressed by file path, not by name from a catalog. There is no configuration file, no resolution rule, and no catalog lookup. Shell completion, relative paths, and `..` work as they do with any file argument.

## Input and Output

The optional second argument is a raw input string that becomes the prompt's `args` value in its entirety:

```bash
promptforge run prompts/staker.md "Bloomberg"
```

The prompt body decides what that text means. The binary does not inspect, split, or coerce it. An input containing spaces must be quoted as a single shell argument.

When the prompt completes, its returned value goes to stdout. Errors go to stderr. Nothing is mixed. On success, stdout contains exactly the returned value and nothing else. On failure, nothing appears on stdout. This clean separation means shell substitution works:

```bash
report=$(promptforge run prompts/digest.md "2026-08")
```

The variable `report` captures exactly what the prompt returned.

## Gateway Configuration

Gateway credentials come from two environment variables:

- `PROMPTFORGE_GATEWAY_URL` - the gateway base URL
- `PROMPTFORGE_GATEWAY_API_KEY` - the bearer token

There are no CLI flags for credentials. This is deliberate: secrets never appear in `argv`, where `ps` and shell history can expose them.

**Local-only mode** is the default. With neither variable set (or with empty/whitespace-only values), the CLI runs without a gateway. The `web_fetch` tool is available, but there is no `web_search` and no remote model catalog. A prompt that makes no model calls works entirely self-contained in this mode.

**Remote mode** activates when both variables are set:

```bash
export PROMPTFORGE_GATEWAY_URL="https://gateway.example.com/v1"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
promptforge run prompts/search-demo.md "latest Rust news"
```

This enables the `web_search` tool and fetches the remote model catalog, so prompts can perform inference through the gateway.

Setting a key without a URL is rejected explicitly:

```
error: PROMPTFORGE_GATEWAY_API_KEY is set but PROMPTFORGE_GATEWAY_URL is missing or empty; both are required to reach the gateway
```

## Tools

Two tools are available to prompts, depending on the gateway configuration:

**`web_fetch`** runs locally and is always available regardless of gateway mode. It needs no credentials.

**`web_search`** proxies through the gateway and is available only when both `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` are set. When the gateway is not configured, `web_search` is omitted entirely rather than advertised as a tool that would fail on its first call.

The tool picker resolves `tools.bind` calls from prompts against the live tool set. Picker descriptors are derived from the same live tool instances, so the tool catalog and picker catalog have identical entries by construction. If a prompt needs a tool that is not available (for example, `web_search` without gateway credentials), the resolution produces the standard absent-capability error before any section executes.

## File Validation

Before parsing, the CLI checks whether the file's YAML frontmatter declares a `promptforge:` version key. If the key is absent, the file is refused with a clear message naming the reason.

This matters because pointing the tool at an ordinary markdown file without this check would produce a confusing parse error about syntax, sending the user to fix the wrong thing. The version check answers a different question: is this file one of ours at all?

## Cancellation and Exit Codes

Press Ctrl-C to cancel a running prompt. The signal trips a cooperative cancellation handle, and the process exits with code 130.

The four exit codes:

| Code | Meaning |
|------|---------|
| 0 | Success - the prompt completed and its value was printed |
| 1 | Operational failure - unreadable file, not a prompt, parse error, setup failure, or execution failure |
| 2 | Usage error - owned by the argument parser (missing file, unknown command) |
| 130 | Cancelled - the run was interrupted with Ctrl-C |

In a script, check `$?` to branch on success or failure. If you need to distinguish failure causes, read the error message on stderr.

## Runtime Behavior

Each run creates an in-memory store. A prompt's filed state lives exactly as long as the process. Nothing is written to disk unless the prompt itself writes something. State does not accumulate across runs. A prompt that needs durable artifacts requires a caller that provides a durable store.

Each run generates a unique execution ID, a 36-character string prefixed with `cli-`, for correlating observations within a single invocation.

Progress is discarded by default. The binary installs a null observer, so long runs produce no progress output. The result appears when the run finishes, and silence in between is expected. A rendering client or progress display would be a separate concern.
