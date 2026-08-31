# promptforge-cli User Guide

PromptForge CLI is a command-line tool that runs PromptForge prompt files against LLM providers. You point it at a prompt file, and it runs the prompt in a single process. There is no server to start, no connection to manage, and no configuration file to write. Your credentials stay in environment variables, never on the command line. Built-in web tools let your prompts fetch pages and search the web. If you can type one command in a shell, you can run any PromptForge prompt.

## What promptforge-cli Does

You run a PromptForge prompt file from the command line by pointing the tool at the file. The tool parses the prompt and executes its sections top to bottom in a single process. Running a prompt is a single command.

The tool runs only genuine PromptForge prompts. If a file's frontmatter does not declare a `promptforge:` version, the tool refuses to run it.

On success, the prompt's returned value is printed to stdout. Stdout contains exactly that returned value and nothing else. Errors go to stderr. This contract makes the tool safe to use in scripts and pipelines.

## Getting Started

Install the tool from crates.io. The install produces an executable named `promptforge`. The install requires Rust 1.89 or later.

Run a prompt file with the `run` subcommand:

````
promptforge run prompts/hello.md
````

The tool reads the file from your local filesystem, executes it, and prints the result. You address prompts directly by file path. There is no configuration file, no name resolution rule, and no catalog lookup.

You can pass a raw input string to the prompt as an optional positional argument after the file path:

````
promptforge run prompts/staker.md "Bloomberg"
````

Inside the prompt, the input is exposed as `args`. It defaults to empty. The tool does not inspect, split, or coerce the input. An input that contains spaces must be quoted as a single shell argument.

## Configuring the Gateway

You connect to a remote PromptForge gateway by setting two environment variables together:

````
export PROMPTFORGE_GATEWAY_URL="https://gateway.example.com/v1"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
````

This enables the `web_search` tool and the remote model catalog for inference through the gateway.

Credentials are accepted only through environment variables, never through command-line flags. This keeps tokens out of argv, process listings, and shell history.

You run entirely local-only, with no network access, by leaving the gateway API key unset or blank. A gateway URL set without a key also yields local-only mode. Local-only mode yields an empty model catalog and a local tool set.

The error cases are strict. A key set without a URL is a startup error, not a fallback. A gateway endpoint that is not a valid URL fails at startup, before the run begins. Blank or whitespace-only credential values are treated as absent. Whitespace around otherwise valid values is tolerated.

The bearer token never appears in logs or diagnostic output. When the tool renders the gateway configuration for diagnostics, it shows the endpoint but replaces the token with a redaction marker.

## Built-in Tools

Any prompt can fetch a web page and return its main content as markdown using the built-in `web_fetch` tool. It runs locally. It is always available in every mode. It needs no credentials.

A prompt can search the web and receive a list of results with title, url, and description using the `web_search` tool. The tool offers `web_search` only when gateway credentials are configured.

If a prompt explicitly binds to a tool that is not available, the run fails before any section executes. For example, a prompt that explicitly binds to `web_search` without gateway credentials fails with an absent-capability error.

## Selecting Tools for a Run

The tool selects tools semantically for each prompt. During startup, it loads an embedding model and builds a semantic tool picker over the available tool catalog.

Capability binding is automatic. When `web_search` is unavailable, a prompt's search capability request falls back to `web_fetch`. When the gateway is configured, the search capability binds to `web_search`. You do not configure this mapping. The tool derives it from the available catalog.

## Startup Progress

While stderr is an interactive terminal, you see live progress bars during startup. The startup sequence has three labeled phases: "model catalog", "embedding model", and "tool index". Each bar shows its phase name and a numeric percentage. Finished phases disappear as their bars are cleared.

The bars are suppressed entirely when output is piped. This keeps stderr clean for scripts. If the progress display itself fails to start, the tool prints a warning and the run proceeds without bars. A progress failure never fails the run.

## Cancelling a Run and Exit Codes

You can interrupt a running prompt with Ctrl-C. This cooperatively cancels the run. If the Ctrl-C listener cannot be installed, the tool prints a stderr warning that the run is not cancellable, and the run proceeds.

Scripts can branch on four exit codes:

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | Operational failure (unreadable file, not a prompt, parse error, setup failure, execution failure) |
| 2 | Usage error (missing file argument, unknown subcommand) |
| 130 | Cancelled with Ctrl-C |

In a script, check `$?` to branch on success or failure. Remember the output contract: on success, stdout holds exactly the returned value. Errors go to stderr as an error chain.

## State and Observability

By default, each run uses a fresh in-memory store. A prompt's state lives exactly as long as the process. Nothing persists or accumulates across runs.

You can persist the prompt's store across runs with the `--store DIR` option:

````
promptforge run prompts/staker.md "Bloomberg" --store ./state
````

This switches from the default ephemeral in-memory store to a persistent file-backed store in the directory you name.

Each run generates a unique execution ID, a 36-character string prefixed with `cli-`. Use it to correlate observations within a single invocation.

During the run itself, the tool produces no progress output. Long runs are silent until the result appears. Silence in between is expected.

## Errors and Invocation Edge Cases

Error messages name the failing stage. Examples include "read prompt file <path>", "parse prompt file <path>", "fetch the model catalog", and "load the tool embedding model". Each error goes to stderr as an error chain, so you see the full context of the failure.

The invocation parser is strict. A missing file argument is an error. An unknown subcommand is an error. Extra trailing arguments are an error. None of these are silently ignored.

The frontmatter gate applies to every run. A file whose frontmatter declares no `promptforge:` version is rejected before execution. This is how the tool guarantees it runs only genuine PromptForge prompts.
