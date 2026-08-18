# Development Runner

`promptforge-dev` is the edit-run-inspect loop for PromptForge prompts. Point it at a prompt file, and it runs the prompt against your already-running gateway, dumps the store for inspection, and optionally watches for saves so every edit triggers a fresh run. No gateway management, no model downloads, no weight files - just the prompt and its output, tight enough that your iteration cycle is limited by how fast you can think, not how long you wait.

## Prerequisites

`promptforge-dev` requires a running `promptforge-gateway`. Start it yourself, then export two environment variables:

```bash
export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
export PROMPTFORGE_GATEWAY_API_KEY=<bearer from your gateway profile>
```

Both must be set and non-empty. If either is missing, the binary fails immediately with a message naming the missing variable and reminding you to start the gateway. No prompt file is read until both are validated.

## Your First Run

From the PromptForge repository root:

```bash
cargo run -p promptforge-dev -- my-prompt.md
```

This runs `my-prompt.md` with an empty input. The second positional argument supplies an input string:

```bash
cargo run -p promptforge-dev -- my-prompt.md "summarize this paragraph"
```

The input becomes the prompt's `args`. If you omit it, it defaults to empty.

Model runtime parameters - context window, thinking mode, max tokens - are not CLI flags. Declare them on the prompt file under `models.need` or `models.default`. The binary's argument surface is deliberately minimal:

```text
promptforge-dev [--watch] [--capture-raw] <prompt.md> [input]
```

## What Happens During a Run

Each invocation follows a fixed pipeline:

1. **Validate environment.** Confirm `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` are set.
2. **Fetch the model catalog.** One HTTP call to the gateway. The catalog is fetched once and reused across watch-mode reruns.
3. **Build the tool set.** Two tools are always constructed: `web_fetch` (runs locally) and `web_search` (proxies through the gateway). A semantic tool picker is derived from the same live set, so no picker descriptor can advertise a tool without a matching callable.
4. **Parse the prompt.** The file must declare `promptforge:` in its YAML frontmatter. A file without it is refused: "is not a promptforge prompt."
5. **Execute.** The prompt runs against the gateway. The store stays in memory during execution - no filesystem writes happen on the async path.
6. **Dump the store.** After the run (success or failure), the in-memory store is reconciled to disk beside the prompt file.

### Execution ID

A unique execution id is minted for each run: `dev-` followed by 128 random hex bits. It prints to stderr before any observer output, so you can always tell which run produced which output:

```text
run id: dev-3a7f1b2c9e4d5a8f0011223344556677
```

### Observer Output

Observer records stream to stderr as single trace lines:

```text
[dev-3a7f1b2c9e4d5a8f0011223344556677] Research: Run started
[dev-3a7f1b2c9e4d5a8f0011223344556677] Research: Lua: checkpoint
```

The final result prints to stdout. This separation lets you pipe or redirect output without observer noise.

## Inspecting Output

Every run dumps its store to `<prompt-stem>.store/` beside the prompt file. For a prompt named `briefer.md`, the dump lands in `briefer.store/`:

```text
briefer.md
briefer.store/
  evidence.md
  notes/
    deep.txt
```

The dump reconciles on every run:

- Changed files are overwritten with current contents.
- Files from a previous run that are no longer in the store are deleted.
- The `.trace/` subdirectory (used by raw trace capture) is preserved across reconciles.
- When the store is empty and no trace files remain, the dump directory is removed entirely.

A failed run still dumps its partial store. That partial output is exactly what you need when debugging a prompt that errored partway through.

## Watch Mode

Add `--watch` to enter a rerun loop:

```bash
cargo run -p promptforge-dev -- --watch my-prompt.md "test input"
```

The prompt runs once, then the file is watched for changes:

```text
watching my-prompt.md for changes; press Ctrl-C to stop
```

Every save triggers a rerun after a 300 ms debounce quiet period. The debounce absorbs editor write-then-rename save bursts so a single save produces a single rerun, not two or three.

The gateway catalog, tools, and picker built at startup are reused across every rerun - no repeated network calls. Each rerun gets a fresh execution id.

If a rerun fails, the error prints to stderr and watching continues. A broken edit does not kill your session.

### Watcher Internals

The watcher monitors the prompt's parent directory, filtered to the prompt's file name. Store dump writes (to the `.store/` directory) do not retrigger reruns. The watcher uses a capacity-one bounded channel, so a slow rerun or a noisy filesystem cannot grow an unbounded event backlog. Watcher backend errors surface through a separate loss-proof slot - they are never silently dropped, even when the channel is full.

## Raw Trace Capture

Add `--capture-raw` to persist the verbatim request and response bodies for each model turn:

```bash
cargo run -p promptforge-dev -- --capture-raw my-prompt.md
```

A warning prints to stderr:

```text
warning: --capture-raw persists verbatim prompts, tool arguments and results, and model output to my-prompt.store/.trace
```

Each model turn writes two files under `.trace/`:

```text
my-prompt.store/
  .trace/
    turn-1-request.json
    turn-1-response.json
    turn-2-request.json
    turn-2-response.json
```

These contain the full, unredacted request and response JSON. The material is sensitive - raw prompts, tool arguments and results, model output - which is why capture is off by default and requires an explicit flag.

### Capture Internals

Trace capture uses a bounded queue (128 events) with a dedicated worker thread. The worker serializes and writes each payload with owner-only permissions and atomic semantics. If the worker falls behind, events are counted as dropped and the count is reported when the run finishes. I/O never blocks the run's async task.

All queued writes are flushed before the store dump reconcile, so trace files are always complete when you inspect the dump directory.

## Filesystem Security

All dump writes - store files and trace captures - go through a security layer:

- **Owner-only permissions.** Directories are created `0o700` and files `0o600` on Unix. On Windows, inherited access is stripped and full control is granted to the current user alone via `icacls`.
- **No symlink traversal.** Every write checks the target and all existing ancestors for symlinks and Windows reparse points. A planted link at any path component is refused, preventing writes from escaping the dump tree.
- **Atomic writes.** Each file is written to a sibling temporary (`.{name}.tmp{random}`), flushed, permission-restricted, then renamed over the destination. An interrupted write cannot truncate a prior file. The temporary is cleaned up on failure.
- **Path safety.** Store paths that are absolute, traverse with `..`, contain backslashes, control characters, or Windows reserved characters (`*`, `?`, `"`, `<`, `>`, `|`) are skipped with a status report. Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9 - including Unicode superscript digit variants) are also rejected.

You do not configure any of this. It is always active.

## Diagnostics

When a Lua error maps to a prompt line, the failure message leads with the file and line number:

```text
dev run failed: briefer.md:51: run briefer.md: lua error: section `Web Search` epilog:51: assertion failed!
```

This format enables click-to-navigate in editors that recognize `file:line:` patterns.

Errors without a mapped prompt line omit the line prefix:

```text
dev run failed: some transport error
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Runtime error (gateway, parse, execution, dump) |
| 2 | Usage error (bad arguments) |
| 130 | Interrupted by Ctrl-C |

Ctrl-C is handled cooperatively: the run is cancelled, its completion is awaited (so blocking fanout joins are not abandoned), and the process exits with code 130.

## Edge Cases and Validation

**Unknown flags.** Any flag starting with `--` that is not `--watch`, `--capture-raw`, or `--` is rejected with usage text. This includes former server knobs like `--context`, `--max-tokens`, and `--no-think` that were removed when model parameters moved to the prompt file.

**Non-PromptForge files.** A markdown file whose YAML frontmatter does not declare `promptforge:` is refused with a clear message rather than producing a confusing parse error.

**The `--` delimiter.** Use `--` to pass an input that begins with dashes:

```bash
cargo run -p promptforge-dev -- my-prompt.md -- --this-is-input-not-a-flag
```

Everything after `--` is treated as a positional argument.

**Credential protection.** The bearer key is wrapped in a `GatewayKey` type that renders as `<redacted>` in Debug output. An accidental `{:?}` on a `GatewayEnv` cannot leak the credential.
