# PromptForge

A runtime that executes analysis pipelines defined in a single markdown file. The
markdown is the program, the model is the CPU.

## Workspace

- `crates/promptforge-core` - library: prompt parser, gateway client, section execution, source-retaining `LuaProgram` compilation to process-local Lua 5.4 bytecode, synchronous four-outcome picker binding with atomic one-to-one alias and stable-identity maps validated against the complete live registry, a sendable persistent `SectionVm` lifecycle seam, deterministic Lua tool declaration binding and exact per-section replay with immutable H2 scope closure, stable live-tool identity (`ToolId`, `ToolRegistry`, and transport-only wire names), and `observe`, the report-only progress seam (`Observer`, `NullObserver`) that a caller hooks to watch a long run. A run reports borrowed `(execution, section, detail)` strings through it as it goes, including constrained author checkpoints from phase-local Lua `log(message)` calls (see "Watching a run" below)
- `crates/promptforge-core-tests` - unpublished binary and test harness: owns complete author-shaped valid, invalid, deterministic offline, real-text, and real-tool-call prompt documents. Ordinary tests assert public parse and error contracts, exact Lua checkpoints, scalar preamble return, store fall-through, concurrent execution-ID separation, and artifact handling without external access. `cargo run -p promptforge-core-tests` provisions pinned official llama.cpp b10082 and Qwen3-0.6B Q8 assets under `.model-cache/`, starts a guarded local server, connects `GatewayClient` directly without `promptforge-gateway`, and behaviorally verifies text, one-string aliased tool dispatch, tool-result continuation, final answer, epilog, and turn budgets; `cargo run -p promptforge-core-tests -- dev <prompt> [input]` runs any prompt file against a separately pinned GPU-served Qwen3.5 9B for interactive prompt development, with `--watch` rerunning on every save
- `crates/promptforge-webfetch` - library: the `web_fetch` tool, which fetches a URL in-process and returns its main content as markdown. It needs no credential, so it runs wherever the prompt runs rather than through the gateway
- `crates/promptforge-cli` - binary `promptforge`: the command-line tool, `promptforge run <file.md> [input]`. It builds a matching live registry and semantic-picker catalog for the tools available at launch, binds H1 needs synchronously, and executes the resulting `BoundPrompt`
- `crates/promptforge-gateway` - binary `promptforge-gateway`: the inference gateway that holds backend credentials and routes OpenAI-shaped chat completions
- `crates/promptforge-mcp-server` - binary `promptforge-mcp-server`: the MCP server that runs a prompt an agentic harness names to `run_prompt`. It prepares one semantic picker and matching complete live registry at boot, binds each prompt's H1 capabilities on Tokio's blocking pool, executes the resulting `BoundPrompt` against the gateway, reports that run as it goes through `notifications/progress`, hands back a run id rather than losing the work when a run outlasts the client's patience, re-reads the catalog when a prompt is saved, retrieves the prompts closest to a plain-English capability, and serves all of it over streamable HTTP or stdio (see "MCP server configuration" below)
- `crates/promptforge-tool-picker` - library: resolves a plain-English capability need to a tool from an abstract catalog. `ToolPicker::build(Catalog, Config)` embeds the whole catalog once with a compiled-in CPU model; `resolve(need)` answers with one of four outcomes (`Outcome::Bind`, `Duplicate`, `Ambiguous`, or `Absent`) and `shortlist(need, k)` hands back the matching tools, best first, for a caller that would rather choose for itself. Loading the model is the expensive part, so a caller whose catalog changes keeps one encoder and re-indexes over it: `build_with(Arc<Embedder>, Catalog, Config)` is the one indexing path and `picker.rebuild(catalog)` reaches it with this engine's own encoder and configuration. No Lua, no MCP, no network

## Build

```
cargo build
cargo test
```

The first build downloads the tool picker's embedding model (about 130MB from
the Hugging Face Hub, pinned to one commit and checksummed) and compiles it into
the library. Later builds reuse the Hugging Face cache and need no network.

## Explicit real-model tests and artifact cache

The unpublished core-tests crate targets Windows and Linux on x86-64 or arm64, plus macOS on x86-64 or Apple Silicon. The scenario suite pins the llama.cpp sidecar to official CPU-only release b10082 archives and the model to official `Qwen/Qwen3-0.6B-GGUF` file `Qwen3-0.6B-Q8_0.gguf` with SHA-256 `9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031`. The dev loop below pins its own model and GPU-enabled archives from the same release.

Provisioned files live only under repository-root `.model-cache/`. Per-artifact file locks synchronize threads and processes, with cache validity rechecked after each lock is acquired. Downloads use `.part` files, verify SHA-256 before rename, reject cache symlink or Windows reparse-point components, confine extraction, reject portable traversal, NTFS alternate-data-stream names, and archive links, and install only by final rename. Valid files are reused; stale partials, corrupt downloads, corrupt extracted installations, and changed Unix executable modes are repaired. Normal `cargo test` uses local fake HTTP responses, temporary caches, tiny generated archives, and test-executable subprocesses for lock contention, so it performs no GitHub or Hugging Face request, launches no `llama-server`, and does not download the 639 MB model.

The explicit command downloads and verifies missing artifacts, then runs both real-model scenarios:

```text
cargo run -p promptforge-core-tests
```

The first run downloads approximately 639 MB for the model plus the current-platform server archive. Later runs print verified cache hits and make no artifact request. The process guard makes at most four fresh-port attempts, gives each child a unique model alias and bearer token, and accepts readiness only when the child remains alive across successful health and authenticated model-identity probes. A child bind loss to another listener triggers a fresh-port retry. Each attempt has a 180-second readiness deadline; the guard continuously captures bounded output tails and kills and waits for the child on success, error, panic, or Ctrl-C. The pinned server uses a fixed seed, temperature zero, one slot, a 4096-token context, a 256-token generation limit, Jinja tool handling, and disabled reasoning. The crate README records the exact invocation and complete URL and SHA-256 provenance.

## The prompt dev loop

Edit a prompt, run it with real inference, read the result. Build the harness once, then run the binary directly - `cargo run` re-checks the whole workspace and adds a minute per invocation:

```text
cargo build -p promptforge-core-tests --release
./target/release/promptforge-core-tests dev <prompt-file> [input]
```

The result is the only thing on stdout; every trace record and Lua `log()` checkpoint streams to stderr. After each run the store's files are dumped to a `<prompt-stem>.store` directory beside the prompt, so whatever the prompt wrote with `store.write` is there to inspect, even after a failed run. For iteration, add `--watch`: the provisioned artifacts and the server stay warm and the prompt reruns on every save, so a rerun costs only inference. `--context N` and `--max-tokens N` override the 131072-token default context and 8192-token generation ceiling, and `--no-think` switches the model to its non-thinking preset. The first run downloads about 5.7 GB of pinned artifacts into `.model-cache/`; later runs report only cache hits.

Dev mode pins its own artifacts beside the scenario assets under `.model-cache/`: the community `unsloth/Qwen3.5-9B-GGUF` Q4_K_M model (about 5.7 GB, SHA-256-pinned) and GPU-enabled llama-server archives from the same b10082 release - Vulkan on Windows and Ubuntu x86-64 and Ubuntu arm64, the already-Metal-enabled tars on macOS, and the CPU archive on Windows arm64, which has no Vulkan build. One mode never downloads the other's artifacts, and a second dev run reports only cache hits.

The dev server starts under the same process guard with a 131072-token default context, an 8192-token generation ceiling, full GPU offload, a q8_0-quantized KV cache, and thinking enabled with model-card sampling; `--no-think` switches to the non-thinking preset. Trace records - every observer report and Lua `log()` checkpoint - stream to stderr, and stdout carries only the final result. `web_fetch` is always available to the prompt; `web_search` joins only when `PROMPTFORGE_TOKEN` names a gateway, and a prompt needing search without one fails loudly at bind. `--watch` keeps the provisioned artifacts and the server warm and reruns the prompt after every save; Ctrl-C tears the server down through the guard. The crate README records the complete flag surface, watch semantics, and URL and SHA-256 provenance for both model pins and the GPU archives.

## Run

Two processes: the gateway holds the vendor credential, the client points at it.

```
export ANTHROPIC_API_KEY=sk-ant-...      # only the gateway sees this
export PROMPTFORGE_TOKEN=dev-secret      # shared bearer token, both processes
cargo run -p promptforge-gateway -- serve gateway.toml &

export PROMPTFORGE_BASE_URL=http://127.0.0.1:8081/v1
cargo run -p promptforge-cli -- run prompts/hello.md
```

Runs the prompt's sections top to bottom and prints the run's result.

The CLI always offers the local `web_fetch` tool for semantic capability binding. It offers gateway-backed `web_search` only when `PROMPTFORGE_TOKEN` is present; without that credential, a prompt needing web search fails as an absent capability before execution. The picker catalog is derived from the same concrete instances as the live registry, so every selectable identity has a callable match. `PROMPTFORGE_BASE_URL` defaults to `http://127.0.0.1:8081/v1`.

Only a **promptforge prompt** runs: the file's frontmatter must declare a
`promptforge:` version (see below). A file without it is not a promptforge
prompt, and the CLI declines to run it with a non-zero exit.

## Gateway configuration

The gateway reads one `gateway.toml`. It defines endpoints (backends) and models
(the names callers request), and holds the credentials.

```toml
[server]
bind = "127.0.0.1:8081"
token = "${PROMPTFORGE_TOKEN}"       # shared bearer; every /v1/* request must present it

[[endpoint]]
id = "anthropic"                     # operator-chosen handle, referenced by models below
protocol = "openai"                  # v0 speaks the OpenAI shape only
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"     # the vendor credential; only the gateway sees it

[[model]]
name = "claude-sonnet-4-6"           # the name callers request (the public contract)
upstream = "claude-sonnet-4-6"       # the string the backend knows this model by
endpoints = ["anthropic"]            # one or more endpoint ids (v0 uses the first)
```

Three distinct namespaces, on purpose:

- `endpoint.id` - an operator-chosen handle. Yours to name (`anthropic`, `pod-a`, `east-1`); unique within the file; referenced by each model's `endpoints` list.
- `model.name` - the caller-facing contract. This is what a client's `PROMPTFORGE_MODEL` (or a prompt) asks for. Changing it is a breaking change for callers.
- `model.upstream` - the vendor's own model string, substituted into the request before it leaves the gateway.

Several models can share one endpoint (same `base_url` + `api_key`), which is why
the endpoint is a separate entry rather than inlined per model.

Any string value may contain `${VAR}`, expanded from the process environment at
load time; `$$` is a literal `$`. An unset variable fails the load, so the gateway
never starts with a blank credential.

### Tool configuration

The gateway can also hold credentials for tools that need one. Today that is
`web_search`, backed by the Brave Search API. The section is optional: without
it the gateway still serves chat completions, and the `web_search` tool simply
returns a 404.

```toml
[tools.web_search]
provider = "brave"                   # v0 supports only "brave"
api_key = "${BRAVE_API_KEY}"         # the Brave Search subscription token; only the gateway sees it
base_url = "https://api.search.brave.com/res/v1"  # optional; override for a proxy or a test server
```

- `provider` - the search backend. Only `brave` is supported in v0.
- `api_key` - the Brave subscription token. Like every credential, it lives only
  in the gateway and is redacted from logs.
- `base_url` - optional; defaults to the Brave endpoint above. Override it to
  point at a mirror or a test server.

With this configured, the gateway exposes `POST /v1/tools/web_search` (bearer-authed
with the same shared token as `/v1/chat/completions`).

## Running the MCP server

```
cargo run -p promptforge-mcp-server -- serve prompts.toml            # streamable HTTP on [server].bind
cargo run -p promptforge-mcp-server -- serve --stdio prompts.toml    # stdio, for a local harness
```

Over HTTP the MCP endpoint is `/mcp` and every request to it must present the
shared bearer from `[server].token`; a request that does not - no
`Authorization` header at all, or one whose scheme is not `Bearer` - is refused
with `401` and a `WWW-Authenticate: Bearer` header before anything is compared.
A `[server].token` that is empty or whitespace alone is refused when the
configuration loads, and one that is absent altogether refuses the bind, naming
the field, so no unset variable can quietly leave `/mcp` open. The
check is per HTTP request
rather than per MCP session, so rotating the token refuses the next call on a
session that already initialized. `/healthz` is the one unauthenticated route,
and it is exempt because it is registered outside the auth layer rather than
because anything inside the layer looks at the path. SSE streams are pinged
every 15 seconds, since a run reports its progress on the stream its call
opened and an idle proxy must not close it.

Over stdio nothing is bound and no token is read: the harness that spawned the
process already has whatever authority the process has. `[server].bind` is
logged as ignored rather than silently obeyed, and `[server].token` is never
consulted - it may be left out of the file entirely, and a `${VAR}` that names
it does not have to be set. `[gateway].token` is required either way, because
every transport runs prompts and every run goes through the gateway. Logs go to
stdout on HTTP and to stderr on stdio, where stdout is the protocol wire.

Either way, boot resolves the whole catalog first and refuses to serve on an incomplete one, printing every fault before a non-zero exit. It then builds the complete live registry (`web_fetch` and `web_search`) and a picker catalog derived from those same concrete instances before starting Tokio. Every `run_prompt` reuses its existing run id as the observer execution id, reparses the validated catalog source snapshot under that id, binds H1 `tools.need` declarations in `spawn_blocking`, then executes the immutable `BoundPrompt` with the same observer and id. Binding reports that are not progress boundaries are tolerated as unknown details and logged at debug level.

The repository ships a working `prompts.toml` at its root, beside `gateway.toml`.
It serves this repository's own `prompts/` directory, expects `PROMPTFORGE_TOKEN`
in the environment (and `PROMPTFORGE_MCP_TOKEN` as well when serving over HTTP,
which is the transport that reads it), and its paths
are relative to the working directory the server is started from, so run it from
the repository root.

### Attaching Cursor

Cursor reaches the server over streamable HTTP, so the server has to be running
already and the request has to carry the bearer. In `~/.cursor/mcp.json` (or the
project's `.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "promptforge": {
      "url": "http://127.0.0.1:9310/mcp",
      "headers": {
        "Authorization": "Bearer dev-secret"
      }
    }
  }
}
```

The URL is `[server].bind` with `/mcp` on the end, and the bearer is the string
`[server].token` resolves to - the same value, written out, since this file is
read by the client and knows nothing about the server's own `${VAR}` expansion.

### Attaching Claude Code

Claude Code spawns the server over stdio, so nothing is bound and no bearer is
read. `cargo build` leaves the binary in `target/`, which is not on `PATH`, so
put it there first:

```
cargo install --path crates/promptforge-mcp-server
```

Then, in the project's `.mcp.json`:

```json
{
  "mcpServers": {
    "promptforge": {
      "command": "promptforge-mcp-server",
      "args": ["serve", "--stdio", "/abs/path/to/prompts.toml"],
      "env": {
        "PROMPTFORGE_TOKEN": "dev-secret"
      }
    }
  }
}
```

The `env` member is what the gateway needs: `[gateway].token` is required on
both transports and its `${VAR}` is expanded from the spawned process's own
environment, which the harness does not inherit from your shell. Give it the
same value `gateway.toml` resolves to. `[server].token` needs nothing here - it
is the HTTP surface's bearer, so it can be left out of the file entirely, and a
`${VAR}` naming it may go unset.

Give absolute paths here, in both the argument and `[paths].prompts` inside the
file: the harness chooses the working directory the process starts in, and a
relative prompts directory is resolved against it.

### The developer loop

Write a prompt, save it, call it. No step in between, and no client restart at
any point.

That is a consequence of publishing no prompt as a tool of its own. Both clients
freeze the tool list for the life of their process - in Cursor a new chat, the
MCP pane's reload button, and `Developer: Reload Window` all carry the old
snapshot over, and Claude Code fixes its tool index when the session starts - and
none of that matters here, because the published list is the same four built-ins
whatever the catalog holds. A prompt saved a second ago is in `list_prompts` and
callable through `run_prompt` on the very next call.

`watch = true` is what makes the edit half of the loop work: the prompts
directory is re-read on save, so the next call runs the version on disk. Keep the
draft out of the catalog while it is still a draft - a file whose name starts
with `_`, at any depth, or anything under `drafts/`, both of which the shipped
`[catalog].exclude` already drops.

## MCP server configuration

The MCP server reads one `prompts.toml`. It names the socket and the shared
bearer, the prompts directory, the gateway runs go through, and which prompts
the harness sees. Only `[server]` and `[gateway]` are required; every other
table and key has a default, and an unknown key fails the load rather than being
silently ignored.

```toml
[server]
bind = "127.0.0.1:9310"              # default
token = "${PROMPTFORGE_MCP_TOKEN}"   # shared bearer; required to serve over HTTP, unread on stdio
max_concurrent_runs = 4              # default; runs beyond it wait for admission
admission_timeout = "30s"            # default; how long a call waits for a slot
reply_deadline = "240s"              # default; past it a call returns a run id and the run continues
retain_completed = "1h"              # default; how long a finished run stays collectable
watch = true                         # default; re-read prompts on save
watch_debounce = "500ms"             # default

[paths]
prompts = 'C:\ProgramData\promptforge\prompts'   # default: prompts

[gateway]
url = "http://127.0.0.1:8081/v1"
token = "${PROMPTFORGE_TOKEN}"
model = "claude-sonnet-4-6"          # optional; the core default otherwise

# Whole directories. Patterns are relative to [paths].prompts; `*` does not
# cross a separator and `**` does. Omit this table to enumerate by hand.
[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["**/_*.md", "drafts/**"]

# Individual prompts, keyed by the prompt's frontmatter name. A block with no
# `file` is an exception to the globs.
[prompts.scratch_test]
enabled = false                      # drop one the globs caught

[prompts.staker]
file = "experiments/staker-v3.md"    # reach a file no glob matches
```

Every prompt in the catalog is reached the same way: `list_prompts` names it and
`run_prompt` runs it. No prompt has an entry of its own in `tools/list`, so
nothing this server offers can be selected for a task the caller did not ask for
by name, and nothing a client caches goes stale.

Durations are plain strings (`500ms`, `30s`, `1h`). As in `gateway.toml`, any
string value may contain `${VAR}`, expanded from the process environment at load
time, with `$$` for a literal `$`; an unset variable fails the load, so the
server never starts with a blank credential.

`[server].token` is the one field that may be left out, and the one whose
`${VAR}` may go unset. It belongs to the HTTP surface: `serve` refuses to bind
without it, naming the field, while `serve --stdio` never reads it and boots
without it. Present, it must carry something - an empty or whitespace-only token
is refused at load, since a request presenting no credential would compare equal
to it. `[gateway].token` is required on both transports, because every run goes
through the gateway.

`reply_deadline` must stay under the calling client's own ceiling. Cursor's
remote calls fail at about 300 seconds and a progress notification does not
reset that clock, so the default leaves margin and a run that outlives it is
collected by id rather than lost. A stdio-only deployment can raise it, since no
such limit applies there.

### How the catalog is resolved

The server expands `include`, subtracts `exclude`, and then applies the
`[prompts.NAME]` blocks. A block drops one globbed prompt with
`enabled = false`, or reaches a file no glob matches by naming it. Patterns are
relative to `[paths].prompts`, and `exclude` is matched against that same
relative path, so `drafts/**` means the `drafts` directory and not any path that
happens to contain the word.

A prompt's stored identity is its frontmatter `name`, which must match
`^[a-z][a-z0-9_]{0,47}$` and is never transformed in the catalog. For forgiving
caller lookup, `run_prompt` case-folds a requested name and treats `-` as `_`;
legal stored identities cannot collide under that normalization. The four
built-in names - `list_prompts`, `run_prompt`, `need_prompt`,
and `check_run` - are reserved: "run `check_run`" is ambiguous to a person and to
a model alike, so the boot refuses such a prompt, naming the collision.

Boot either produces a complete catalog or the server refuses to start. Every
resolved file must be readable, must parse, and must declare a legal name; two
prompts declaring one name is an error naming both files, a block with no `file`
that matches no globbed prompt is a stale override and an error, and an empty
catalog is an error. Failures accumulate and all of them print before the
non-zero exit, so fixing a configuration takes one pass rather than one restart
per mistake. The one thing a glob skips in silence is a markdown file that
declares no `promptforge:` version: a glob names a directory, and a file in it
that is not a prompt is not the operator's mistake.

### Reloading on save

Once the server is running, `watch = true` re-runs that same resolution on save
with one difference: a prompt that fails validation is kept as a broken entry
carrying its error - still listed, and answering a call with the failure -
instead of stopping the process. Refusing the whole catalog is right at boot,
where nothing depends on the server yet, and wrong on save, where one typo in one
file would freeze every other prompt.

The prompts directory is watched recursively and `prompts.toml` with it. Events
are collected for `watch_debounce` and the window restarts on each one, so a save
that an editor performs as a write to a temporary and a rename - which is most
Windows editors - costs one re-resolution rather than one per event. A prompt
written since the client connected is in `list_prompts` and callable by
`run_prompt` on the next call, with no reconnect; a deleted file leaves the
catalog; a run already in flight holds the catalog it started with and finishes
under that definition, whatever the save did to the file.

What a reload cannot change is the shape of the running service. `[server]`,
`[gateway]`, and `[paths].prompts` were read once - the bound socket and the
bearer layer, the run registry's limits, the gateway each run goes through, and
the directory being watched - so a change to any of them is logged as ignored,
naming each setting, and takes effect on the next restart. Only the catalog
tables reload. A candidate that cannot be resolved at all - an unparsable
`prompts.toml`, two prompts under one name, a stale override, an empty result -
leaves the previous catalog serving and logs why, since there is no partial
answer to give.

No client is told anything, and none needs to be. The published tool list is the
same four built-ins whatever the catalog holds, and the catalog behind them is
read fresh on every call, so a prompt saved a moment ago is callable on the next
call - on the session that is already open, with no notification and no
reconnect. The server does not advertise `listChanged` on its tool capability,
because there is no change it could announce.

`watch = false` turns the whole thing off: nothing is watched, and the catalog is
exactly what boot resolved for the life of the process. A directory that cannot
be watched at all is refused at boot rather than dropped silently, since losing
live reload without being told is worse than being told to fix the path.

A watch that breaks after boot - a prompts directory renamed or deleted out from
under the server - is logged at error level and registered again on the next
settled window. When it can be, saves resume; when it cannot, the log says that
live reload has stopped and a restart is needed once the path is back, because a
developer whose saves quietly stop taking effect has no other way to find out.
The server keeps serving either way: a lost watch is not worth ending a process
that is still answering calls.

### What the harness sees

Four tools, and never a prompt. The list does not depend on the catalog and does
not change while the process runs:

| Tool | Arguments | Published when |
|---|---|---|
| `list_prompts` | none | always |
| `run_prompt` | `prompt`, optional `args` | always |
| `need_prompt` | `capability` | the `picker` feature is compiled in |
| `check_run` | `run_id` | always |

`list_prompts` reports every enabled prompt with its name, description, and any
problem that stops it running; `run_prompt` runs one by name, taking the
run's whole input as one optional string, where omitting it passes the empty
string; `need_prompt` resolves a description of a prompt to the names of up to
three close ones, running none of them. `check_run` collects a run that outlived
the call which started it. A prompt the reload left broken stays in the listing
carrying its problem, and a call naming it answers with that problem.

That surface follows from what a PromptForge prompt is. A prompt is a command,
invoked because someone named it, so its text reads as a command interpreter's:
it says what the server executes and that a caller supplies the name, and it
makes no claim on any situation. A model that never calls this server is
behaving correctly. The rejected alternative was one tool per prompt, which put
the catalog into every conversation's context, made a prompt something a model
could reach for unbidden, and went stale the moment a prompt was saved, because
every client caches its tool list for the life of its process.

`need_prompt` asks for its `capability` in author register - an imperative
phrase naming the operation and what it acts on, with no entity names or
conversational framing - because a description phrased that way resolves to the
right prompt far more often than the same one phrased as a user goal, and no
ranking engine closes that gap. The instruction and its two examples sit both in
the tool's description and in the parameter's own, since a client may surface
only one of the two.

### What a call returns

A call at `run_prompt` is one run against the configured gateway, reported as a
`RunResult`. The result's
`structuredContent` carries the whole record - `run_id`, `prompt`, `status`
(`running`, `completed`, or `failed`), `value`, `turns`, `elapsed_ms`, `error` -
and the text block beside it carries the plain product: the returned
value verbatim on completion, the error on failure. A failed run also sets
`isError`. The value is the whole product, since the runtime writes no output
files and so has no path to hand back instead.

`run_prompt` resolves the name it is given leniently in exactly two ways: letter
case folds, and `-` and `_` are the same character. That is safe because a legal
prompt name may contain neither uppercase nor a hyphen, so normalizing cannot
merge two names. Past that the match is exact, and a near miss is never run - a
guess ranked onto a different prompt would spend minutes of gateway time
producing the wrong artifact, and the caller could not tell. A name that
resolves to nothing comes back as a result carrying every enabled name, closest
first, which is what the calling model needs to correct itself on its next call.

Where a failure lands is decided by who can fix it. A malformed argument shape
is the client's own bug and comes back as a JSON-RPC `-32602`; a name the model
guessed, or a run that started and failed, comes back as a result the model
reads and acts on. A tool name this server does not publish comes back as
`-32601`: that covers an unknown name, a prompt called as though it had a tool
of its own, and `need_prompt` in a build without the `picker` feature. What the
server answers and what it advertises are read from the same statement, so they
cannot disagree.

### What `need_prompt` retrieves

Every runnable prompt becomes one entry in a retrieval index built at boot, over
its name and its description - exactly what `list_prompts` reports, so what a
caller reads is what retrieval matched on. A capability is embedded by the same
model and the three closest prompts come back as `{name, description}`, best
first. Nothing is filtered out for scoring poorly: the similarity floor is zero,
because a floor exists to stop an unattended binding and this tool binds nothing
- a model reads the candidates and decides. Three weak candidates are
self-evidently weak to that reader, while an empty answer to a casually phrased
request helps nobody.

A broken prompt is never a candidate. It cannot run, so offering it would spend
the caller's next call on a certain failure; `list_prompts` is where a broken
prompt and its problem are read.

The index is rebuilt on the same catalog swap a save already performs, and only
when a name or a description moved - a body-only edit costs nothing. The rebuild
reuses the model already in memory, so it is one embedding pass per prompt rather
than a reload of 67MB of weights.

Retrieval never stops the server. `--no-default-features` drops the `picker` feature and `need_prompt` with it, but keeps the embedding weights required for execution-time capability binding. Failure to prepare the execution picker stops boot because prompts could not bind correctly without it. Failure to prepare the optional retrieval index is reported at error level while the process serves on: every prompt is still callable, and `need_prompt` answers that retrieval is unavailable and sends the caller to `list_prompts`.

### A run that outlasts the call

A call blocks for at most `reply_deadline`. Past it the caller is answered with
a result whose `status` is `running`, carrying the `run_id` and a line telling
it to collect with `check_run`; the run itself continues, and its value is
waiting when the caller comes back for it. A `running` result does not set
`isError`, because nothing has gone wrong.

The run is detached, not cancelled. That is the whole point of the deadline:
Cursor's remote calls fail at about 300 seconds whatever the server does, so a
prompt that takes longer either survives the call or is wasted. `check_run`
answers for a run still going as well as one that finished, and a run that
finished inside its deadline is collectable by id too, so a caller never has to
know which happened.

A finished run stays collectable for `retain_completed` and is then evicted. An
id that is unknown or evicted comes back as a result naming that window rather
than as a protocol fault, since a model that polled too late should learn why. A
run still going is never evicted.

Before any of that, a call has to be admitted: `max_concurrent_runs` prompts run
at once and a call waits up to `admission_timeout` for a slot. One that does not
get a slot is refused with a result naming the wait it spent, which the calling
model can act on by retrying. Refusing is deliberate - every waiting call holds a
client connection, and a queue long enough to outlast the reply deadline would
turn into a crowd of background runs the operator never sized for.

Progress belongs to the call that asked for it. A run that outlives its deadline
gets one bounded flush of whatever it had queued, and then reports into a stream
that has been answered and closed: the frames after that are counted and
dropped, and the run itself is never slowed by them. The record `check_run`
returns is what a caller follows a backgrounded run with.

The registry is in memory. A restart forgets every run, finished or not, which
is the right trade for a service whose recovery is to fire the prompt again.

### Progress while a run is in flight

A call that carries a `progressToken` gets `notifications/progress` as the run
walks the prompt, which is what turns one silent multi-minute call into a
caption a client updates in place. The frames are:

| When | `progress` | `message` |
|---|---|---|
| the run starts | 0 | the prompt's required H1 title |
| each section starts | sections entered so far, from 1 | the section's heading |

Nothing else is sent. A section's end would repeat the frame already on the
wire, and a model turn or a tool call is written to the log instead, where a
reader can ask for it without every caller paying for it.

`total` is always absent. An early return means the number of sections a run
will visit is not known when it starts, so a denominator would be a guess
wearing a measurement's clothes; the client shows a changing caption rather
than a filling bar. `progress` never decreases, which the protocol requires.

Progress buys visibility and never time: no client resets its call timeout on a
notification, so a long run is kept alive by the reply deadline rather than by
this.

A call carrying no `progressToken` is answered identically, with no channel and
no forwarding task behind it. Either way the run's reported `turns` is the count
of exact `Model turn completed` observations received by the same observer.

## Prompt file anatomy

```
---
name: hello
description: Say hello
promptforge: 1
---

# Title

```lua
tools.need("fetch", "Fetch a web page and return its main content as markdown.")

function normalize(value)
    return string.lower(value)
end
```

Human-readable description (not executed).

## Section

```lua
var.subject = args
tools.add("fetch")
```

Prose the model reads.

```lua
return reply
```
```

- `---` delimited YAML frontmatter (`name` and `description` required; `promptforge`, `default_return`, and `max_tool_iterations` optional to the parser).
- `promptforge:` is the **engine version** that marks the file as a promptforge prompt (supported major: `1`). A file without a `promptforge:` version is not a promptforge prompt and the CLI declines to run it; an unsupported major is refused, never silently degraded.
- Exactly one `# Title` is required. Markdown before it is ignored.
- The H1 may immediately open with one exact, unindented triple-backtick `lua` fence after any number of blank lines. Its exact triple-backtick closing line ends the shared library, and the remaining H1 text is the human-readable description. A leading reserved fence with an inexact closing marker is an error; a `lua` fence after description prose is ordinary Markdown prose, mirroring section semantics. Indented markers, longer backtick runs, different capitalization, and extra info tokens are ordinary Markdown. The old `lua prompt` info-token form was removed; an H1 that opens with it is a parse error naming the removed form.
- `## Section` headings are executable units; they run top to bottom (fall-through). Each section parses as an optional exact leading `lua` preamble fence, prose, and an optional exact trailing `lua` epilog fence. Reserved fences use exact unindented lowercase ` ```lua ` opening lines and exact unindented ` ``` ` closing lines. Blank lines may surround them. A lone reserved fence is the preamble. Lua fences between prose, longer or indented fences, different capitalization, extra info tokens, and marker-looking lines inside a longer fence remain model-facing prose.
- Shared Lua declares semantic capabilities under prompt-local aliases with `tools.need(alias, description)`. A section exposes an alias deliberately with `tools.add(alias)`; `tools.always(alias)` belongs in shared Lua only when every model-facing section genuinely needs that capability. Declaration alone exposes nothing.

## Prompt language

A run takes one raw input string and executes the prompt's sections top to
bottom (fall-through):

```
promptforge run <file.md> [input]
```

`input` is exposed to the prompt as `args`.

### Section Lua phases

A section may open with an exact ` ```lua ` preamble fence and close with an exact ` ```lua ` epilog fence. Both are compiled during parsing with errors located at the section heading and phase. Compilation reports contain only fixed strings, never Lua source or diagnostic payloads.

The executor creates one hardened VM per section, replays the compiled shared program, injects host values, runs the compiled preamble, sends substituted prose through the complete model tool loop, binds the final text as `reply`, runs the compiled epilog, and tears the VM down. Direct `print` is unavailable in binding, replay, preamble, epilog, and compatibility Lua VMs. Each executable phase instead receives a borrowed `log(message)` callback. It accepts exactly one valid UTF-8 string of at most 256 characters with no newline or control character, then reports `Lua: <message>` under the current execution id and H1 or H2 heading. The callback expires when that phase returns, so no observer reference survives into a model await and a saved callback cannot be reused in a later phase.

Lua log text is the sole author-controlled observer detail. Treat it as a short static checkpoint label. Never log prompt arguments, model replies, tool arguments or results, credentials, filesystem paths, store contents, or other sensitive payloads.

The preamble can read `args` and `sys`, write the `var` table, and end the run early by returning a value:

```lua
return args              -- finishes the run with this value; no model call
```

If the preamble returns nothing (or there is no preamble), the section's prose is sent to the model. Empty prose skips the model but still runs the epilog with `reply` nil. A scalar epilog return ends the run.

`bind_tool_declarations` runs shared code once against a deterministic `ToolResolver`, where `tools.need(alias, description)` records exact case-sensitive aliases and `tools.always(alias)` selects prompt-wide scope. `bind::bind_prompt` supplies the concrete `ToolPicker` adapter and complete live `ToolRegistry`, caches byte-identical capability descriptions during one synchronous H1 pass, maps picker failures and `Absent`, `Duplicate`, and `Ambiguous` outcomes to distinct core errors, rejects identity collisions, and precomputes the picker's near-duplicate pairs for the immutable selected set. `SectionVm::new_with_bindings` replays those declarations without resolving again. H2 `tools.add(alias)` records section-local additions, and scope closure orders prompt-wide aliases before those additions. Before a non-empty model turn, core rejects any precomputed near-duplicate pair present together in that effective scope. It advertises each surviving concrete tool's exact description and schema under the prompt-local alias, then dispatches returned aliases through the frozen `ToolId`. Similar tools remain valid when isolated in separate sections. Scope, model, and tool reports use fixed payload-free details.

### Substitution

Before the model sees the prose, `{{ path }}` placeholders are resolved from three namespaces. Substitution applies only to prose - never to preamble or epilog Lua source:

- `{{ args }}` - the raw input string.
- `{{ var.x }}` - values the Lua preamble wrote (`var.x = ...`).
- `{{ sys.when }}` / `{{ sys.now }}` / `{{ sys.id }}` - runtime metadata: the run's
  launch timestamp, the time when the current section started, and the 1-based
  section id.

Scalars render as strings, tables as JSON, and a missing path is an error.
In Lua, `sys` exposes only the keys the runtime injected: an unknown read or
any write raises. Substitution does no arithmetic - compute in Lua and
reference the result (`var.total = var.a + var.b`, then `{{ var.total }}`).

### Fall-through and the result

Top-level `##` sections run in file order, each with fresh Lua state and a fresh
model conversation. The run-scoped `StoreRef` is the sole mutable channel carried
between sections. A section ends by either:

- **returning a value from Lua** (`return ...`) - this finishes the whole run
  with that value, so sections after it are not reached by fall-through; or
- **falling through** - if it doesn't return, its prose (if any) is sent to the
  model and control moves to the next section.

Running off the last section ends the run. The result is `default_return` from
the frontmatter if set, otherwise the last model reply, otherwise a generic
completion. `sys.id` counts sections as you go (1, 2, 3, ...).

### Example

`prompts/greet.md`:

```
# Greet

## Main

` ` `lua
var.greeting = "Hello, " .. args .. "!"
` ` `

Repeat exactly, with no extra words: {{ var.greeting }}
```

`promptforge run prompts/greet.md "World"` sends `Repeat exactly, with no extra
words: Hello, World!` to the model. `prompts/echo.md` (just `return args`) prints
its input with no model call and no gateway.

## Watching a run

A multi-section run is minutes long, so `execute::run` reports itself as it
goes. Its last parameter is a `RunOptions`:

```rust
use promptforge_core::execute::{self, RunOptions};
use promptforge_core::observe::NullObserver;

let opts = RunOptions {
    execution: "example-run", // one caller-owned id for parse, bind, and run
    observer: &NullObserver,   // or your own Observer
    client: None,              // None builds the gateway client from the environment
};
let result = execute::run(&prompt, input, &tools, &store, opts).await?;
```

- `execution` is a caller-owned stable identifier. Pass the same value to `Prompt::parse`, `bind_prompt`, and `RunOptions`; the CLI generates one per invocation and MCP reuses its existing run id.
- `observer` receives borrowed `(execution, section, detail)` strings when parsing and binding occur, when the run starts and ends, at each section boundary, after each model turn and tool call, after each Lua-harness store operation, and for accepted Lua `log(message)` checkpoints. `observe` is synchronous and sits on the caller's path, so an implementation that forwards elsewhere copies the triple into a queue and returns rather than blocking. Observations are reports, never decisions: dropping them cannot change the result, which is why `NullObserver` is what the CLI passes. Observer implementations own synchronization; a recorder shared by concurrent executions can use `Mutex<Vec<_>>`, and no core-global observer lock is held across an await.
- Fixed details are stable exact strings from `promptforge_core::observe::detail`. They contain no prompt prose, model input or output, tool arguments or results, store paths or contents, credentials, or fetched content. The sole payload-bearing exception is a validated `Lua: <message>` author checkpoint, which must remain a short static label and must not contain arguments, replies, tool data, credentials, paths, or store contents.
- The MCP adapter recognizes `Run started` and `Section started` for cosmetic numeric progress. It counts recognized section starts from 1 and tolerates unknown details. There is deliberately no total: an early return means the number of sections a run will visit is not known when it starts, so a denominator would be a guess.
- `client` is the gateway client the run's model calls go through. `None` builds
  one from `PROMPTFORGE_BASE_URL` / `PROMPTFORGE_TOKEN` / `PROMPTFORGE_MODEL` on
  the first call that needs it, which is what the CLI uses. A caller configured
  from a file passes its own, because setting a process environment variable is
  `unsafe` under edition 2024 and this workspace forbids unsafe.

## Tools

A prompt can let the model reach outside itself while a section runs. Two tools
ship built in:

- `web_fetch` - fetch a URL and get back its main content as markdown. It runs
  in-process wherever the prompt runs, the CLI and the MCP server alike, because
  it needs no credential; it extracts the article body with a readability pass
  and falls back to a whole-page conversion for pages that are not
  article-shaped.
- `web_search` - search the web and get back a list of results (title, URL,
  description). It proxies through the gateway, which holds the Brave API key,
  so the credential never reaches the process running the prompt.

Concrete tool names no longer belong in YAML frontmatter or prompt code. The parser accepts a compiled H1 shared library, and `bind::bind_prompt` executes that source once to resolve `tools.need(alias, description)` through a `ToolPicker`, record `tools.always(alias)`, and validate the result against a complete live `ToolRegistry`. Its immutable `BoundPrompt` carries matching forward and reverse maps plus picker overlap analysis. `execute::run` combines explicit prompt-wide and H2 scopes, advertises concrete descriptions and schemas under local aliases, and dispatches aliases by stable identity. Declared tools are never injected automatically. The CLI and MCP server both use this complete path; the MCP server builds the complete live registry and matching picker catalog at boot, binds each run on Tokio's blocking pool, and executes the resulting `BoundPrompt`.

Binding intentionally has no reranker. The tool-picker spike found that `bge-reranker-v2-m3` improved the clean hackathon set to 0.856 but scored 0.735 on TOOLRET, below plain bge-small at 0.804, while adding a roughly 568M-parameter model. That domain-dependent regression does not justify another mandatory stage. A reranker remains a non-goal until author-register measurements on the deployment's own catalog show a reliable gain.

Progress labels are also outside binding and execution. The deterministic `(execution, section, detail)` observation record is the authoritative trace and is never rewritten by a model. A future label model may derive optional UI text off the critical path, but it must preserve the original record, avoid feeding labels back into decisions, and justify its measured quality, latency, and privacy cost independently.

### The tool-call loop

When a library caller supplies and a section scopes tools, the executor advertises their JSON schemas to the
model on that section's call. If the model replies with a tool call instead of
text, the executor dispatches it (locally for `web_fetch`, or to the gateway for
`web_search`), appends the result to the conversation, and re-sends. This repeats
until the model returns a final text reply, capped at 24 round trips per section
(the default when a prompt does not set `max_tool_iterations`) to prevent a
runaway loop. Sections that scope no supplied tools make one round trip with no tool advertising.
