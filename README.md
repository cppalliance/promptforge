# PromptForge

A runtime that executes analysis pipelines defined in a single markdown file. The
markdown is the program, the model is the CPU.

## Workspace

- `crates/promptforge-core` - library: prompt parser, gateway client, section execution
- `crates/promptforge-cli` - binary: the `promptforge` command-line tool
- `crates/promptforge-gateway` - binary: the inference gateway that holds backend credentials and routes OpenAI-shaped chat completions
- `crates/promptforge-tool-picker` - library: resolves a plain-English capability need to a tool from an abstract catalog (skeleton)

## Build

```
cargo build
cargo test
```

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
