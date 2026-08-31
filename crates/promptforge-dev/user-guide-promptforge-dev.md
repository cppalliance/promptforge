# promptforge-dev User Guide

You write prompts. You want to see what they do. `promptforge-dev` runs one prompt file against your already-running PromptForge gateway with a single command. Edit the file. Save it. See the result. Add `--watch` and every save triggers a fresh run, so the loop from edit to result takes seconds. The tool dumps the prompt's store to disk after each run, prints the final result on stdout, and keeps diagnostics on stderr. You get a fast, inspectable loop for prompt development with no gateway management, no model downloads, and no weight files.

## The Prompt Runner and Edit-Run Loop

`promptforge-dev` is a command-line tool. You point it at a prompt file. It runs that prompt against a PromptForge gateway that is already running. One command gives you one run.

The tool connects to an existing gateway. It never starts one. You do not manage a gateway lifecycle. You do not download models. You do not handle weight files.

Pass `--watch` to enable watch mode. Every save of the prompt file triggers a fresh run. You get a live edit-run feedback loop while you develop a prompt.

## Installation and Gateway Setup

Install the tool from crates.io with one command. The package is published. You do not build from source. The tool requires Rust 1.89 or later.

The tool needs two environment variables. Set both before you run it. There are no CLI flags for them.

````bash
export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
export PROMPTFORGE_GATEWAY_API_KEY=<bearer from your gateway profile>
````

`PROMPTFORGE_GATEWAY_URL` is the gateway API root. `PROMPTFORGE_GATEWAY_API_KEY` is your bearer credential. Both must be set and non-empty. An empty value counts as missing.

The tool validates the environment once at startup, before it does any work. If a variable is missing or empty, you get a startup error. The error names the missing variable. It tells you to start promptforge-gateway first, then export both variables. The tool exits with code 1.

A malformed gateway URL or a blank credential aborts startup before any prompt run. Your bearer credential never appears in logs or debug output. It renders as redacted.

## Running a Prompt

The simplest invocation names a prompt file.

````bash
promptforge-dev my-prompt.md
````

Pass an input string as the second positional argument. The input becomes the prompt's `args`. If you omit it, it defaults to empty.

````bash
promptforge-dev my-prompt.md "summarize this paragraph"
````

To pass an input that begins with `--`, place a bare `--` delimiter before it.

````bash
promptforge-dev my-prompt.md -- "--verbose"
````

Declare context, thinking, and max tokens on the prompt file itself. Use `models.bind` or `models.always`. The tool rejects CLI flags for these settings. `--context`, `--max-tokens`, `--no-think`, and `--verbose` all produce unknown-flag errors.

The tool runs only files that declare a `promptforge:` version in frontmatter. It refuses other files with a clear message.

Each run follows a fixed pipeline: validate environment, fetch model catalog, build tool set, parse prompt, execute, dump store. The catalog is fetched once and reused across watch-mode reruns. Every run prints a unique run id to stderr. The id correlates console output, traces, and store files.

When a run fails, you get a diagnostic that names the prompt file. The tool exits with code 1. A missing prompt file or more than two positional arguments produces a one-line problem description and the usage line, with exit code 2. Argument errors report before any credential check.

The exit codes are documented: 0 success, 1 runtime error, 2 usage error, 130 interrupted. Cancel a running prompt with Ctrl-C. You receive an "interrupted by Ctrl-C" message. The exit code is 130. Scripts can branch on these codes.

## Results and the Store Dump

When a run succeeds, the prompt's final result string prints on stdout. The result is separate from the diagnostic stream on stderr. You can pipe or redirect the result without observer noise.

After a run, the tool dumps the prompt store. You inspect what the prompt produced. Store dumping is part of the default run behavior. No extra flag is needed.

Every run's store lands in a directory beside the prompt file, named after it. For a prompt named `briefer.md`, the dump lands in `briefer/`. It contains the files the prompt wrote, such as `evidence.md` and `notes/deep.txt`.

````text
briefer.md
briefer/
  evidence.md
  notes/
    deep.txt
````

Every store write lands on disk immediately during the run. There is no post-run reconcile step. The tool clears the previous store directory before each new run, so stale files never masquerade as current output. A run that produces nothing removes its empty store directory when it finishes. Your directory tree stays clean. A failed run keeps its partial store on disk. You can debug from it.

The tool skips unsafe store paths and reports the status. Unsafe paths include absolute paths, `..` traversal, backslashes, control characters, and Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9).

## Watch Mode

Pass `--watch` to rerun the prompt automatically on every save.

````bash
promptforge-dev --watch my-prompt.md
````

The tool prints a startup line: it is watching the file, and Ctrl-C stops it. Edit the prompt. Save it. A fresh run fires. Successful results print to stdout on every rerun. Diagnostics stay on stderr.

A failed rerun prints its error on stderr. Watch mode keeps watching for the next save. You keep iterating.

A burst of rapid saves coalesces into one rerun. The rerun fires after 300ms of quiet. One logical edit produces exactly one run. Editors that save through atomic write-then-rename still trigger reruns.

Watch mode watches only the prompt file. Changes to other files in the same directory never trigger a rerun. This includes the store directory's contents. A bare file name as the prompt path watches the current directory. If the filesystem watcher backend fails, watch mode stops with a descriptive error.

Reruns are fast. The run environment is built once and reused across every save. The tool does not refetch the model catalog or rebuild the tool picker on each save.

Stop watch mode at any time with Ctrl-C. The exit is clean. You see "interrupted by Ctrl-C". No spurious final rerun fires, even if a save was mid-debounce.

## Web Tools and Tool Picking

Your prompts get web fetching and web search tools during a run. Both tools are always available on every run. There is no offline mode. There is zero configuration.

The model fetches a web page with `web_fetch`. The tool returns the page's main content as markdown. It runs locally.

The model searches the web mid-run with `web_search`. The tool proxies through the PromptForge gateway. It uses your validated bearer credential.

The run picks relevant tools for the prompt automatically. A semantic tool picker resolves natural-language capability descriptions to the matching tool. The picker is built over the live tool catalog and an embedding model. The live tool set is validated before the picker is derived, so every advertised tool is actually callable. Duplicate tool identities or illegal wire names produce clear startup errors instead of silent breakage.

## Raw Capture and Trace Files

Pass `--capture-raw` to persist verbatim request and response bodies. This covers full prompts, tool arguments and results, and model output.

````bash
promptforge-dev --capture-raw my-prompt.md
````

This flag is the only way trace capture activates. An ordinary run never silently persists sensitive data. When the flag is active, a warning on stderr names the trace directory.

The traces go to a `.trace/` directory inside the prompt's store directory: `<prompt-stem>/.trace/`. Each model turn produces one pretty-printed JSON file per direction, named `turn-{N}-request.json` and `turn-{N}-response.json`. Each file holds one verbatim request or response body. You inspect or replay exactly what happened during a session.

Trace capture never blocks the run. A background worker writes the files. If the capture queue falls behind, events drop. The tool reports the exact drop count on stderr when the run finishes. Each written trace file gets a stderr confirmation. A failed trace write produces a stderr diagnostic, and the run continues.

## Progress and Console Output

During setup, progress bars render when stderr is a terminal. They cover the catalog fetch, the embedding-model load, and tool indexing. Bars clear as phases finish. Off a terminal, stderr stays clean.

During the run, you watch a live verbose trace on stderr. Every observation is its own bracketed line. Each line is prefixed with the run id.

````text
[dev-3a7f...] Research: Run started
[dev-3a7f...] Section: Lua: step one
````

The final result prints separately to stdout. You can pipe or redirect output without observer noise.

## Diagnostics and Failure Reporting

When a run fails inside a Lua section, the error message leads with the prompt file path and the exact line number.

````text
dev run failed: briefer.md:51: <detail>
````

The line number points at the innermost failing section. A failure not tied to a prompt line shows a plain message without a line number. Errors name the failing file and stage, whether the file cannot be read, parsed, or executed.

Use the run id prefix on each trace line to correlate console output, traces, and store files for one run.

## Filesystem Security

All dump and trace writes are owner-only. Directories are mode 0o700 and files are mode 0o600 on Unix. On Windows, full control goes to the current user alone. You do not configure this. It is always active. On Windows, this hardening depends on the USERNAME environment variable.

The tool refuses to write through symlinked or reparse-point ancestors. A planted link cannot redirect sensitive output outside the dump tree. Dump files are written atomically. Content goes to a temporary file, then renames over the destination. An interrupted write never corrupts a previously dumped file. A failed write removes its partial temporary file.
