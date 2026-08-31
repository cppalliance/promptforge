# PromptForge MCP Server - User Guide

The PromptForge MCP server runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. It puts your prompt catalog behind four fixed MCP tools, so the agent in your harness can discover, pick, and run your prompts as tools. Follow this guide and you end with a working server connected to your harness.

## What This Server Is

The server runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. You connect it to your harness as a standard MCP server.

A prompt is a plain Markdown file. Its YAML frontmatter declares `name`, `description`, and the format version `promptforge: 1`. Its body carries executable Lua sections. A prompt library is a directory of `.md` files.

One `prompts.toml` file configures the whole server. It names the server settings, the prompts directory, the model gateway, and the prompts the harness sees.

The catalog sits behind four fixed built-in MCP tools. Prompts are never published as tools of their own. `tools/list` never changes when you add, edit, rename, or break a prompt.

## The Four Built-In Tools

`list_prompts` enumerates every enabled prompt in the catalog, healthy or broken. Each entry carries the prompt's name, its description, any problem that stops it, and its declared input and output contracts. The agent reads this list to learn what the server can run.

`run_prompt` executes an enabled prompt by name. Naming a prompt to `run_prompt` is the only way to invoke one.

`need_prompt` resolves a plain-English capability description to a ranked shortlist of up to three candidate prompts, best first. It runs nothing. The agent uses it to find a prompt, then passes one of the returned names to `run_prompt`.

`check_run` collects the outcome of a run that outlived its originating call. The agent passes the `run_id` that the earlier result named.

## Deployment and Execution Model

The server speaks MCP over two transports. Streamable HTTP serves remote or shared access behind a shared bearer token. Stdio serves harnesses that spawn the server as a local child process, as line-framed JSON-RPC.

Prompt runs make their model calls through an OpenAI-compatible chat-completions gateway. You configure the gateway in `prompts.toml`. Its credentials stay separate from the MCP-facing credentials.

A filesystem watcher keeps the catalog current while the server runs. Edit a prompt file and the change takes effect immediately. You do not restart the server. The client does not reconnect.

Boot is fail-fast. The server validates the catalog before it binds any transport. It refuses to serve an incomplete or broken catalog rather than start with prompts silently missing.

## Installation, Launch, and Boot

Install the `promptforge-mcp-server` package from crates.io. Any toolchain at Rust 1.89 or later builds it. The install puts one `promptforge-mcp-server` binary on your path.

Launch the server with one of exactly two command-line shapes. Use the first for HTTP. Use the second for stdio.

````console
promptforge-mcp-server serve prompts.toml
promptforge-mcp-server serve --stdio prompts.toml
````

Any other command line prints a usage error and exits nonzero.

A healthy boot logs each boot step - catalog resolve, retrieval index, tool build - and ends serving:

````text
promptforge-mcp-server serving on http://127.0.0.1:9310/mcp
````

A refused boot prints every catalog fault in one pass. Each fault names its prompt and its file. The process then exits nonzero, so you fix all faults in one pass rather than one restart each. A missing config file error reports the exact path and distinguishes a missing file from a permission failure.

Script around the binary with conventional exit codes: zero on a clean serve, nonzero on any boot or argument failure. Stop the server with Ctrl-C. Both transports drain and close.

## Server Configuration and Secrets

Start with a minimal `prompts.toml`. This is a complete config for HTTP:

````toml
[server]
api_key = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "gateway-bearer"
````

Every setting you omit takes a default. A full config shows what you can tune:

````toml
[server]
bind = "127.0.0.1:9310"
api_key = "shared-bearer"
allowed_hosts = ["localhost", "127.0.0.1", "::1"]
max_concurrent_runs = 4
admission_timeout = "30s"
reply_deadline = "240s"
retain_completed = "1h"
watch = true
watch_debounce = "500ms"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "gateway-bearer"

[paths]
prompts = "prompts"

[tools]
web_fetch = true
web_search = true
````

Under `[server]`:

- `bind` sets the HTTP bind address. Default `127.0.0.1:9310`.
- `api_key` sets the shared bearer token every `/mcp` request must present. Omit it for a local stdio install; `serve --stdio` never reads it.
- `allowed_hosts` lists the host authorities the server accepts.
- `max_concurrent_runs` sets how many prompts run at once. Default 4.
- `admission_timeout` sets how long a call waits for a run slot. Default `30s`.
- `reply_deadline` sets how long a call waits for its run. Default `240s`, kept under Cursor's ~300-second call ceiling.
- `retain_completed` sets how long a finished run stays collectable. Default `1h`.
- `watch` toggles hot reload. Default on.
- `watch_debounce` sets how long the watcher waits for filesystem events to settle. Default `500ms`.

Write durations in human-readable form: `30s`, `4m`, `500ms`, `1h`.

Under `[gateway]`, set `url` and `api_key`. Both are required. The URL must be a real http or https URL with a host. All prompt-run model traffic goes to this gateway.

Under `[tools]`, opt into `web_fetch` and `web_search` to grant prompts live web access. Both default to disabled. A prompt with no `[tools]` section runs in a true sandbox with no network access.

Keep secrets and machine-specific values out of `prompts.toml`. Write `${VAR}` references in any TOML string value:

````toml
[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "${GATEWAY_KEY}"
````

A name-matched `prompts.env` file beside `prompts.toml` can supply the values. Real environment variables always win over the file. File values never enter the process environment. A missing or malformed `prompts.env` never fails the load. Interpolation works in nested arrays and sub-tables, not just top-level strings.

An unset variable aborts the load and names the exact field. The one exception is `[server].api_key`: an unset variable there leaves the key absent, so stdio installs stay unblocked. Write `$$` for a literal dollar sign. A bare `$` not followed by `$` or `{` passes through literally. An unclosed `${...` is a load error.

The server refuses blank or whitespace-only secrets at load. Secrets redact as `Secret(redacted)` in all debug and display output and never serialize. Unknown or misspelled config keys fail the load and name the offending key. A config file over 4 MiB is refused.

## Catalog Configuration and Resolution

Point the server at your prompts directory with `[paths].prompts`. The default is `prompts/` relative to the working directory. Relative and absolute paths both work.

Select which prompt files enter the catalog with glob patterns:

````toml
[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["_*.md", "drafts/**"]
````

`*` matches within one path segment. `**` crosses separators. Matching is case-sensitive. A recursive pattern like `governance/**/*.md` reaches nested directories while `*.md` matches only the top level. Exclusions always win over inclusions. Exclude patterns match root-relative paths, so `drafts/**` means what it reads as.

Override glob results per prompt with a `[prompts.NAME]` block:

````toml
[prompts.scratch_test]
enabled = false

[prompts.staker]
file = "experiments/staker-v3.md"
````

Set `enabled = false` to drop a prompt the globs caught. Set `file` to publish a file no glob matches. The path is relative to the prompts directory. Absolute paths and any `..` component are rejected at config load. A leading `./` is accepted, and Windows backslash paths parse.

Keep ordinary non-prompt Markdown files in the prompts directory. A glob-matched file with no `promptforge:` frontmatter marker is skipped without comment. Notes and drafts never leak into the tool surface.

Prompt names must match `^[a-z][a-z0-9_]{0,47}$`: a lowercase ASCII start, then lowercase letters, digits, and underscores, 48 characters maximum. The four built-in tool names are reserved in every build. Two healthy prompts declaring the same name is a fault that lists every file that declared it. An empty resolved catalog is a hard fault; the server never boots serving nothing. A block whose `file` declares a different frontmatter name than the block key is a hard fault naming both names. A block with no `file` that matches no globbed prompt is a stale-override fault naming the dead key.

A prompt file over 2 MiB is refused as a broken entry. Every served file is confined to the prompts directory: a symlink or reparse point under the root that points outside it is resolved and dropped.

Boot and reload treat a broken prompt differently. Boot rejects it and refuses to serve. Reload retains it as a broken entry: still listed under a placeholder name suffixed `(broken)`, sorted after healthy entries, exposing no source text. Calling it returns the validation failure rather than silently running a stale copy. The catalog listing is always ordered by prompt name.

## Prompt Authoring

A prompt is a Markdown file with YAML frontmatter and Lua code blocks. This is a complete prompt:

````markdown
---
name: echo
description: Returns its argument
promptforge: 1
---

# Echo

## Main

```lua
return args
```
````

The frontmatter declares `name`, `description`, and `promptforge: 1`. Each `##` section carries a Lua code block.

A Lua prologue runs before any model call. Return a value from it to short-circuit the whole run. In a multi-section prompt, each section either falls through to the next or returns a final value to end the run. A Lua-visible `var` store shares state across section boundaries.

Declare input and output file contracts in the frontmatter. Give each a path and a description:

````markdown
---
name: reader
description: Reads its input and writes it to its output
promptforge: 1
input:
  path: paper.md
  description: The input file
output:
  path: report.md
  description: The output file
---

# Reader

## Main

```lua
local content = store.read("paper.md")
store.write("report.md", content)
return "done"
```
````

The prompt reads and writes the declared files at run time through `store.read(path)` and `store.write(path, content)`.

Bind external capabilities to tool names with `tools.bind(name, capability)`, and activate them per section with `tools.add(name)`. Declare capabilities in natural language; the server resolves them to enabled tools at run time. This scopes which tools a model may use in each section.

Bind a model to a named role with `models.default(role, description)` or `models.bind(alias, description, opts)`, and pick a role per section with `models.use(alias)`. The prompt chooses which gateway model serves each prose section. Role bindings resolve live against the gateway model catalog at boot.

Try the shipped example catalog to see working prompts: analyst_example, echo, greet, hello, research_person. It loads and serves exactly as shipped, out of the box.

## Running and Collecting

The simplest call names a prompt and nothing else:

````json
{ "prompt": "echo" }
````

Pass the prompt's input as one raw string with `args`. Omitting it passes the empty string.

````json
{ "prompt": "echo", "args": "hello" }
````

Seed a declared input with `input_file` (a filesystem path) or `input_text` (text placed directly in the prompt's store). The two are mutually exclusive; specify one, not both. Write a declared output to disk with `output_file`. Omit it to receive the output inline as the result value.

Every run outcome comes back in `structuredContent`, a flat JSON object:

````json
{ "run_id": "0123456789abcdef0123456789abcdef", "prompt": "echo", "status": "completed", "value": "hello", "turns": 0, "elapsed_ms": 4, "error": null }
````

A plain text block mirrors it: the value on completion, the error on failure, a collection instruction while running. `status` serializes as `running`, `completed`, or `failed`. A `completed` result always carries a `value` and a null `error`. A `failed` result always carries an `error` and a null `value`. `turns` counts model round trips; a Lua-only prompt reports zero. `elapsed_ms` measures only the run itself, never the queue wait.

A run can outlive its call. Past `reply_deadline`, the call returns a `running` result naming a `run_id` instead of failing. The run keeps executing in the background under a supervisor. Collect the finished record later with `check_run`:

````json
{ "run_id": "0123456789abcdef0123456789abcdef" }
````

Run ids are 128 random bits rendered as 32 hex digits. A finished run stays collectable for `retain_completed` and is then evicted. A still-running run is never evicted and reports its live elapsed time. Polling an unknown or evicted `run_id` returns a tool error whose message names the retention window. A run started in one HTTP session is collectable by `check_run` from a different session.

Stop a run by abandoning the awaiting call. When the client cancels the request or disconnects mid-wait, the run is signalled to cancel and its concurrency slot frees for a fresh run.

Recover from a mistyped prompt name by reading the error result. It lists every enabled prompt name ordered nearest-first, and nothing is run on a miss. Name resolution folds letter case and treats `-` and `_` as the same character, so `Research-Person` reaches `research_person`.

Admission is bounded. A call waits up to `admission_timeout` for one of `max_concurrent_runs` slots, then is refused with a retryable answer naming the exact wait spent.

## Discovery, Retrieval, and Tool Surface

Call `list_prompts` with no arguments to read the first page of the catalog:

````json
{}
````

Page through a large catalog with the optional `cursor` parameter. A page carries at most 100 entries. `next_cursor` in the response fetches the next page. A cursor the server never issued is a `-32602` invalid-params error.

Call `need_prompt` with a `capability` string to find a prompt without reading the whole catalog:

````json
{ "capability": "Build a stakeholder position report for one entity." }
````

Phrase the capability in author register: a short imperative phrase naming the operation and what it acts on, with no entity names or conversational framing. Good: "Build a stakeholder position report for one entity." Bad: "I need to know what Herb Sutter has said about ABI stability." Casual phrasings still return candidates. A capability over 4096 bytes is rejected with a message telling you to restate it as one short imperative.

The shortlist holds up to three candidates, best first. Each candidate carries a `name` you can pass to `run_prompt` and its verbatim `description`. An empty candidate list is a complete answer - "no prompt is close to this" - not an error. Broken prompts are never recommended. If the retrieval index is unavailable, `need_prompt` says so and redirects you to `list_prompts`.

The tool list is fixed for the life of the process: `list_prompts`, `run_prompt`, `need_prompt`, `check_run`, in that order. All four input schemas set `additionalProperties: false`, so a misspelled or obsolete argument is refused, not silently dropped. A prompt name is never dispatchable as a tool: calling `echo` directly returns METHOD_NOT_FOUND. A build without the `picker` feature publishes three tools instead of four, dropping `need_prompt`.

## Progress, Logging, and Error Surface

Attach a `progressToken` to a `tools/call` to receive live `notifications/progress` updates in Cursor or Claude Code while a multi-minute run is in flight. Frame 0 is captioned with the prompt's H1 title the moment the run starts. Each later frame is captioned with a section's H2 heading. Values latch monotonically from 0 and `total` is never sent, so the client shows a changing caption rather than a filling bar. Progress is strictly best-effort: a client that stops accepting notifications silently ends the stream without failing the call. Omit the token and the run is silent with an identical result.

Watch operations through structured logs. Every run boundary is an `info` event. Within-run chatter stays at `debug`. Failed tool calls and failed model turns surface at `warn`. Logs go to stdout normally and to stderr in stdio mode, so log capture never collides with the MCP wire protocol. Terminal run records carry run_id, prompt, status, turns, and elapsed_ms. Prompt content never reaches the log. Boot progress appears as log records for catalog resolve, retrieval index, and tool build, weighted by expected duration.

Read model-correctable failures as ordinary tool results with `isError` set: a broken prompt, an unresolvable capability, a refused admission, a failed run. The calling model reads the corrective detail and acts. Only malformed argument shapes are protocol errors. A missing required argument is a `-32602` error naming the key. An explicit `null` for an optional string is rejected as a client bug rather than coerced to empty.

## Transports and Security

On stdio, the server binds no network listener and reads no token. A config that sets `bind` or `api_key` anyway is logged as ignored. Each JSON-RPC message is one line. A line over 4 MiB, or a malformed line, is dropped and the session survives. EOF on stdin ends the session cleanly.

Over HTTP, the server serves MCP at `/mcp`. Every `/mcp` request must present the shared bearer token from `[server].api_key`. The check runs per request, not per session: a rotated-away token is refused on the very next request, even on an initialized session. The `Bearer` scheme is matched case-insensitively. Refusals are 401 with a `WWW-Authenticate: Bearer` header. HTTP refuses to bind without `[server].api_key`, before the socket is bound.

`allowed_hosts` validates the request `Host` header as a DNS-rebinding defence. Empty on a loopback bind defaults to `localhost`, `127.0.0.1`, `::1`. Empty on a non-loopback bind refuses to start with an error naming the bind address and the required setting; enumerate the public authorities instead, for example `["example.com", "example.com:8080"]`. A disallowed Host is rejected with 403 even with a valid token.

`/healthz` is public, outside the bearer layer, and returns `{"status": "serving"}`. A 15-second SSE keep-alive keeps long-running tool calls alive through proxies. The server speaks MCP protocol revision 2025-06-18 and does not advertise tool-list change notifications, because the tool list never moves.

## Hot Reload and the Watcher

The watcher is on by default. Add, edit, rename, or delete prompt files while the server runs. The change is live on the very next tool call on the same already-open MCP session, with no reconnect and no client notification. `watch_debounce` (default `500ms`) lets filesystem events settle before a reload, so one save costs one reload.

Edit `prompts.toml` itself and the save triggers the same reload path. Catalog-shaping changes apply on the next reload. These settings stay pinned to boot values and are logged by name as requiring a restart: `[server].bind`, `[server].api_key`, `[server].max_concurrent_runs`, `[server].admission_timeout`, `[server].reply_deadline`, `[server].retain_completed`, `[server].watch`, `[server].watch_debounce`, `[server].allowed_hosts`, `[paths].prompts`, `[gateway].url`, and `[gateway].api_key`.

Tolerate a prompt broken by a bad save. It stays listed as a broken entry carrying its error, and the rest of the catalog keeps serving. A reload that cannot re-resolve - an unparsable `prompts.toml`, a stale override, duplicate names, an empty result - keeps the previous catalog and logs the reason. A typo in one file never takes the running service down. Each reload logs its outcome: how many prompts loaded, how many are broken, whether ranking changed, and whether the retrieval index is current or stale.

Set `watch = false` to serve exactly the boot-resolved catalog for the life of the process.
