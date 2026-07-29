# PromptForge

A runtime that executes analysis pipelines defined in a single markdown file. The
markdown is the program, the model is the CPU.

## Workspace

- `crates/promptforge-core` - library: prompt parser, gateway client, section execution
- `crates/promptforge-cli` - binary: the `promptforge` command-line tool
- `crates/promptforge-gateway` - binary: the inference gateway that holds backend credentials and routes OpenAI-shaped chat completions

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

Prints the model's response to the first section of the prompt.

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
