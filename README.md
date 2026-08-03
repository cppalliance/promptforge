# PromptForge

A runtime that executes analysis pipelines defined in a single markdown file. The
markdown is the program, the model is the CPU.

## Workspace

- `crates/promptforge-core` - library: prompt parser, gateway client, section execution, and `observe`, the progress-reporting seam (`Observer`, `Event`, `NullObserver`) that a caller hooks to watch a long run. A run reports through it as it goes (see "Watching a run" below)
- `crates/promptforge-cli` - binary: the `promptforge` command-line tool
- `crates/promptforge-gateway` - binary: the inference gateway that holds backend credentials and routes OpenAI-shaped chat completions
- `crates/promptforge-mcp` - library (a binary follows): the MCP server that publishes prompts to an agentic harness as callable tools. Today it parses its `prompts.toml`, resolves the catalog that configuration names, turns that catalog into the tool list a harness sees, answers a call by running the prompt against the gateway, reports that run as it goes through `notifications/progress`, and hands back a run id rather than losing the work when a run outlasts the client's patience (see "MCP server configuration" below); the transports that carry all of this are being built on top of it
- `crates/promptforge-tool-picker` - library: resolves a plain-English capability need to a tool from an abstract catalog. `ToolPicker::build(Catalog, Config)` embeds the whole catalog once with a compiled-in CPU model; `resolve(need)` answers with one of four outcomes (`Outcome::Bind`, `Duplicate`, `Ambiguous`, or `Absent`) and `shortlist(need, k)` hands back the matching tools, best first, for a caller that would rather choose for itself. No Lua, no MCP, no network

## Build

```
cargo build
cargo test
```

The first build downloads the tool picker's embedding model (about 130MB from
the Hugging Face Hub, pinned to one commit and checksummed) and compiles it into
the library. Later builds reuse the Hugging Face cache and need no network.

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

## MCP server configuration

The MCP server reads one `prompts.toml`. It names the socket and the shared
bearer, the prompts directory, the gateway runs go through, and which prompts
the harness sees. Only `[server]` and `[gateway]` are required; every other
table and key has a default, and an unknown key fails the load rather than being
silently ignored.

```toml
[server]
bind = "127.0.0.1:9310"              # default
token = "${PROMPTFORGE_MCP_TOKEN}"   # shared bearer; every /mcp request must present it
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
exclude = ["_*.md", "drafts/**"]
default_expose = "list"              # default

# Individual prompts, keyed by the prompt's frontmatter name. A block with no
# `file` is an exception to the globs.
[prompts.research_person]
expose = "tool"                      # promote one globbed prompt to its own tool

[prompts.scratch_test]
enabled = false                      # drop one the globs caught

[prompts.staker]
file = "experiments/staker-v3.md"    # reach a file no glob matches
expose = "tool"
```

Two exposures. `expose = "list"` (the default) keeps a prompt out of the
harness's tool list and reachable through the server's own listing and retrieval
tools, which costs one extra round trip and no permanent context slot.
`expose = "tool"` gives the prompt its own entry in `tools/list` for the calling
model to select directly. Direct exposure is a promotion, not a starting point:
a client caches its tool list for the life of its process, so a newly added or
renamed direct tool is invisible until it restarts, while the listing tools have
fixed names and their catalog changes freely.

Durations are plain strings (`500ms`, `30s`, `1h`). As in `gateway.toml`, any
string value may contain `${VAR}`, expanded from the process environment at load
time, with `$$` for a literal `$`; an unset variable fails the load, so the
server never starts with a blank credential.

`reply_deadline` must stay under the calling client's own ceiling. Cursor's
remote calls fail at about 300 seconds and a progress notification does not
reset that clock, so the default leaves margin and a run that outlives it is
collected by id rather than lost. A stdio-only deployment can raise it, since no
such limit applies there.

### How the catalog is resolved

The server expands `include`, subtracts `exclude`, and then applies the
`[prompts.NAME]` blocks. A block promotes one globbed prompt, drops one with
`enabled = false`, or reaches a file no glob matches by naming it. Patterns are
relative to `[paths].prompts`, and `exclude` is matched against that same
relative path, so `drafts/**` means the `drafts` directory and not any path that
happens to contain the word.

A prompt's identity is its frontmatter `name`. That name is used verbatim as the
MCP tool name rather than transformed into one, so it must match
`^[a-z][a-z0-9_]{0,47}$`; transforming would let two different frontmatter names
collide in `tools/list`. The four built-in names - `list_prompts`,
`run_prompt`, `need_prompt`, and `check_run` - are reserved: a call is matched
against the built-ins first, so a prompt claiming one of those names would be
published as a tool that could never run, and the boot refuses it instead,
naming the collision.

Boot either produces a complete catalog or the server refuses to start. Every
resolved file must be readable, must parse, and must declare a legal name; two
prompts declaring one name is an error naming both files, a block with no `file`
that matches no globbed prompt is a stale override and an error, and an empty
catalog is an error. Failures accumulate and all of them print before the
non-zero exit, so fixing a configuration takes one pass rather than one restart
per mistake. The one thing a glob skips in silence is a markdown file that
declares no `promptforge:` version: a glob names a directory, and a file in it
that is not a prompt is not the operator's mistake.

Once the server is running, `watch = true` re-runs that same resolution on save
with one difference: a prompt that fails validation is kept as a broken entry
carrying its error - still listed, and answering a call with the failure -
instead of stopping the process. Refusing the whole catalog is right at boot,
where nothing depends on the server yet, and wrong on save, where one typo in one
file would freeze every other prompt.

### What the harness sees

Each prompt exposed as `tool` gets its own entry in `tools/list`, named and
described from its frontmatter. Its input schema is one optional string property
named `args`, which is a run's whole input; omitting it passes the empty string.
A prompt the reload left broken keeps its tool, described by the problem that
stops it running, since every connected client is still holding a cached copy of
the list and would send the call regardless.

Alongside them are the built-ins, published by what the catalog holds rather
than by configuration:

| Tool | Arguments | Published when |
|---|---|---|
| `list_prompts` | none | at least one prompt is `list` |
| `run_prompt` | `prompt`, optional `args` | at least one prompt is `list` |
| `need_prompt` | `capability` | at least one prompt is `list`, and the `picker` feature is compiled in |
| `check_run` | `run_id` | anything at all is published |

`list_prompts` reports every enabled prompt with its name, description, version,
whether it is also direct, and any problem; `run_prompt` runs one by name and
its description sends a caller unsure of a name to `list_prompts` first;
`need_prompt` takes a plain-English capability and hands back up to three
candidates for the caller to choose among, never running one. `check_run`
collects a run that outlived the call which started it, which a direct call can
do just as a listed one can, so it is published whenever anything is.

`need_prompt` asks for its `capability` in author register - an imperative
phrase naming the operation and what it acts on, with no entity names or
conversational framing - because a need phrased that way retrieves the right
prompt far more often than the same need phrased as a user goal, and no ranking
engine closes that gap. The instruction and its two examples sit both in the
tool's description and in the parameter's own, since a client may surface only
one of the two.

### What a call returns

A call at a prompt's own tool and a call at `run_prompt` end in the same place:
one run against the configured gateway, reported as a `RunResult`. The result's
`structuredContent` carries the whole record - `run_id`, `prompt`, `version`,
`status` (`running`, `completed`, or `failed`), `value`, `turns`, `elapsed_ms`,
`error` - and the text block beside it carries the plain product: the returned
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
reads and acts on. A tool name this catalog does not publish at all comes back
as `-32601`: that covers an unknown name, a listed prompt called as though it
had its own tool, and a built-in the table above leaves out - `need_prompt` in a
build without the `picker` feature, or the listing tools over an all-direct
catalog. What the server answers and what it advertises are read from the same
statement, so they cannot disagree.

`need_prompt`, where published, is not yet answered: it returns a result saying
so, rather than a fault, since the tool was advertised and the caller did
nothing wrong. Its behavior arrives with the picker.

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
| the run starts | 0 | the prompt's name |
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
the executor itself reported, since the observer that receives progress is also
what hears the run's final tally.

## Prompt file anatomy

```
---
name: hello
description: Say hello
version: 1
promptforge: 1
---

# Title

Human-readable description (not executed).

## Section

Prose the model reads.
```

- `---` delimited YAML frontmatter (`name`, `description`, `version`, `promptforge` required; `tools`, `default_return`, `max_tool_iterations` optional).
- `promptforge:` is the **engine version** that marks the file as a promptforge prompt (supported major: `1`). It is distinct from `version:`, which is the author's own revision of the prompt. A file without a `promptforge:` version is not a promptforge prompt and the CLI declines to run it; an unsupported major is refused, never silently degraded.
- `# Title` and the text before the first `##` are human-readable, not executed.
- `## Section` headings are executable units; they run top to bottom (fall-through). Each may begin with a single ` ```lua ` block followed by prose.

## Prompt language

A run takes one raw input string and executes the prompt's sections top to
bottom (fall-through):

```
promptforge run <file.md> [input]
```

`input` is exposed to the prompt as `args`.

### The Lua block

A section may open with one ` ```lua ` fence. It runs before the model, in a
sandbox (no filesystem, network, or `os`). It can read `args` and `sys`, write
the `var` table, and end the run early by returning a value:

```lua
return args              -- finishes the run with this value; no model call
```

If the block returns nothing (or there is no block), the section's prose is sent
to the model.

### Substitution

Before the model sees the prose, `{{ path }}` placeholders are resolved from
three namespaces:

- `{{ args }}` - the raw input string.
- `{{ var.x }}` - values the Lua block wrote (`var.x = ...`).
- `{{ sys.when }}` / `{{ sys.now }}` / `{{ sys.id }}` - runtime metadata: the run's
  launch timestamp, a build-time timestamp, and the context id.

Scalars render as strings, tables as JSON, and a missing path is an error.
Substitution does no arithmetic - compute in Lua and reference the result
(`var.total = var.a + var.b`, then `{{ var.total }}`).

### Fall-through and the result

Top-level `##` sections run in file order, each in a fresh context (nothing is
carried between them). A section ends by either:

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
    observer: &NullObserver,   // or your own Observer
    client: None,              // None builds the gateway client from the environment
};
let result = execute::run(&prompt, input, &tools, &store, opts).await?;
```

- `observer` receives an `Event` when the run starts and ends, at each section
  boundary, at each model turn, and after each tool call. `on_event` is
  synchronous and sits on the run's own path, so an implementation that
  forwards elsewhere queues the event and returns rather than blocking. Events
  are a report, never a decision: dropping them cannot change the result, which
  is why `NullObserver` (the discarding one) is what the CLI passes.
- `Event::SectionStarted` carries `completed`, the number of sections entered so
  far counting from 1. It never decreases. There is deliberately no total: an
  early return means the number of sections a run will visit is not known when
  it starts, so a denominator would be a guess.
- `client` is the gateway client the run's model calls go through. `None` builds
  one from `PROMPTFORGE_BASE_URL` / `PROMPTFORGE_TOKEN` / `PROMPTFORGE_MODEL` on
  the first call that needs it, which is what the CLI uses. A caller configured
  from a file passes its own, because setting a process environment variable is
  `unsafe` under edition 2024 and this workspace forbids unsafe.

## Tools

A prompt can let the model reach outside itself while a section runs. Two tools
ship built in:

- `web_fetch` - fetch a URL and get back its main content as markdown. It runs
  locally in the CLI (no credential), extracting the article body with a
  readability pass and falling back to a whole-page conversion for pages that
  are not article-shaped.
- `web_search` - search the web and get back a list of results (title, URL,
  description). It proxies through the gateway, which holds the Brave API key,
  so the credential never reaches the CLI.

A prompt declares the tools it needs in its frontmatter:

```
---
name: research
description: Research a topic and summarize it
version: 1
tools: [web_search, web_fetch]
---

## Main

Research the topic "{{ args }}". Search the web, read the most relevant
pages, and write a short summary with links.
```

`web_fetch` is always available. `web_search` needs the gateway, so
`PROMPTFORGE_BASE_URL` and `PROMPTFORGE_TOKEN` must be set; a prompt that asks
for it without a configured gateway fails fast with a clear error.

### The tool-call loop

When a section declares tools, the executor advertises their JSON schemas to the
model on that section's call. If the model replies with a tool call instead of
text, the executor dispatches it (locally for `web_fetch`, or to the gateway for
`web_search`), appends the result to the conversation, and re-sends. This repeats
until the model returns a final text reply, capped at 24 round trips per section
(the default when a prompt does not set `max_tool_iterations`) to prevent a
runaway loop. Sections without a `tools` list behave exactly as
before - one round trip, no tool advertising.
