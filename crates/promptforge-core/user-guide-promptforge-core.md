# promptforge-core User Guide

promptforge-core is a Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then executes it against any OpenAI-compatible endpoint. No process-global state, no framework lock-in, no runtime compilation surprises. The caller owns every resource. What you get: structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, all driven from a single `run` call that returns a string.

---

## Prompt Files

A prompt file is a Markdown document with YAML frontmatter. The frontmatter must declare `name` and `description`. A `promptforge:` key identifies the file as a promptforge prompt - the runtime refuses files that lack a supported version number.

```yaml
---
name: summarizer
description: Summarize a document into bullet points
promptforge: 1
---
```

Below the frontmatter, the document has one H1 title and zero or more H2 sections. A prompt with H2 sections walks them top to bottom in fall-through order. A prompt with no H2 sections executes the H1 blocks and returns the model reply. The H1 region always runs first, resolving tools and models before any section begins.

### Minimal Prompt File

```markdown
---
name: hello
description: A greeting prompt
promptforge: 1
---

# Hello

## Greet

Say hello to the user in a friendly tone.
```

The parser compiles Lua code at parse time. A successfully parsed prompt is syntactically executable without any runtime compilation step - Lua syntax errors surface before any network call is made.

### Structural Rules

The parser enforces strict structure:

- When H2 sections are present, the first and every root heading must be exactly H2.
- Sibling section names must be unique; duplicates produce a diagnostic naming both heading locations.
- Orphan deep headings (H4 under H2 with no H3) are rejected rather than silently reparented.
- Unknown frontmatter fields are rejected so misspelled keys fail loudly.
- Sections nest recursively using heading levels H2 through H6.
- Executable Lua fences must use exact unindented triple-backtick `lua` openers. Longer markers, indentation, or extra info-string words remain inert prose.

Parse errors report stable kind discriminants and optional byte spans for editor diagnostics. Lua compilation errors include absolute source-line numbers that map back to the original prompt file.

### Optional Frontmatter Fields

- `max_tool_iterations` - integer between 1 and 1000 (default: 24)

---

## Execution Model

Execution is a free function call over caller-owned resources. There is no process-global state. The caller owns the prompt, the execution id, the tool picker, the tool catalog, the model catalog, the store, and the observer.

```rust
use promptforge_core::{run, Prompt, RunConfig, StoreRef, ResolutionContext};
use promptforge_core::tools::ToolCatalog;

let prompt = Prompt::parse(source, "my-execution", &observer)?;
let tool_catalog = ToolCatalog::new(&tools)?;

let result = run(
    &prompt,
    "user input here",
    ResolutionContext::new(&picker, &models, &tool_catalog),
    &StoreRef::memory(),
    RunConfig::new("my-execution"),
).await?;
```

The run resolves the H1 block once, then walks H2 sections top to bottom. A section falls through to the next when its Lua does not return a value. An explicit return stops fall-through. When execution falls off the last section, the result is the last model reply, or the generic string `"done"` only if no model reply exists.

### Run Configuration

`RunConfig` uses a builder pattern:

```rust
RunConfig::new("execution-id")
    .observer(my_observer)
    .debug(my_debug_capture)
    .client(gateway_client)
    .cancel(cancel_handle)
    .limits(run_limits)
```

All builder methods are optional. Without `.client()`, the runtime lazily constructs one from environment variables.

### Run Limits

Configurable limits cap resource consumption:

```rust
RunLimits::new()
    .max_tool_iterations(NonZeroU32::new(24).unwrap())        // model round-trips per section
    .max_fanout_concurrency(NonZeroUsize::new(8).unwrap())    // parallel arms
    .max_response_bytes(NonZeroU64::new(16 * 1024 * 1024).unwrap())
    .lua_memory_bytes(NonZeroUsize::new(64 * 1024 * 1024).unwrap())
    .lua_log_events(NonZeroU32::new(1024).unwrap())
    .request_timeout(Duration::from_secs(120))
```

---

## Lua Scripting

A prompt is built from alternating Lua and prose blocks. Each section can contain any number of Lua blocks interleaved with prose segments. The last prose block in a section runs a full tool-call loop; earlier prose blocks run single-shot (one model round, then control continues to the next Lua block).

Preamble, prologue, and epilog are positions, not phases: the preamble is the H1 region, the prologue is a section's Lua before its first prose, and the epilog is Lua after the last prose.

### The H1 Preamble

Lua blocks in the H1 region execute once in source order before any H2 section. The preamble declares tools and models, sets variables, and can short-circuit the entire run:

````markdown
# My Prompt

```lua
models.default("writer", "a capable writing model")
tools.bind("search", "web search capability")
tools.always("search")
var.topic = "Rust async patterns"
```

## Write

Write an article about {{ var.topic }}.
````

Returning a scalar value (string, integer, number, or boolean) from H1 skips all H2 sections and becomes the run result.

### Shared Libraries

A `lua shared` fence in the H1 defines a reusable library compiled once and replayed into every section VM as its first chunk:

````markdown
```lua shared
function summarize(text)
    return "Summary: " .. text
end
```
````

The replay runs with the full section environment installed (`args`, `sys`, `var`, `reply`, `store`, `log`, the `tools`/`models` tables, and the control globals), so top-level shared code may use them at load. Two exclusions apply: the captured tool/model alias globals install only after the replay (a declared alias wins over a same-named shared global), and `jump` during the load is a hard error. A scalar top-level return is discarded - the replay loads a library, it does not produce the section's result.

### Section Environment

Each section VM provides these globals:

| Global | Purpose |
|--------|---------|
| `args` | Input string passed to the run |
| `sys` | Sealed read-only runtime metadata |
| `var` | Writable data bridge, persists across sections |
| `store` | Virtual filesystem |
| `tools` | Tool scope and call counts |
| `log` | Diagnostic checkpoint function |
| `reply` | Previous section's model answer |

The `sys` table includes `when`, `now`, `id`, `section_name`, `execution`, `section_count`, `model` (after first model interaction), and `reply_finish_reason` (after inference). It is sealed - writes raise errors and the metatable cannot be replaced. `sys.id` is the run-global execution-unit counter: H1 keeps id 0, and every section entry and every fanout arm takes the next value. A fanout arm also carries `sys.index`, its 1-based position within the current fanout; `sys.index` is absent outside fanout.

`var` is the walk-local clipboard: writes persist across sections on the same walk (H1 included), and `execute`/`fanout` clone it in and discard it out - child writes never reach the caller. `var` holds JSON data only; a non-JSON value fails at the assigning line. Bare globals (`x = 42` without `local`) are section-local scratch, visible to prose as `{{ x }}`.

### Template Substitution

Prose blocks support `{{ path }}` template substitutions. The sources are `args`, `reply`, `var`, `sys`, `item` (fanout arms only), and bare globals - a section-local Lua global resolves as `{{ x }}`, with dotted paths indexing into its JSON form:

````markdown
## Research

```lua
var.query = "latest Rust async runtimes"
```

Search for {{ var.query }} and summarize the results for {{ args }}.
The previous section said: {{ reply }}
Current item: {{ item }}
Run id: {{ sys.id }}
````

Escape literal delimiters with backslash: `\{{` emits `{{`.

### Control Flow

`jump(target)` transfers control to another section by heading name, clearing conversation context. The current `reply` value is preserved across the jump. Clear it explicitly with `reply = nil` before jumping when the target should not inherit the previous reply. `execute(target, input)` runs a section as a subroutine with a fresh VM and conversation, returning that section's reply:

````markdown
## Router

```lua
local result = execute("## Research", "find Rust crates for HTTP")
var.research = result
jump("## Synthesize")
```

## Research

Research the topic: {{ args }}

## Synthesize

Using this research: {{ var.research }}

Write a summary.
````

`execute()` nests up to 8 levels deep, and the count accumulates across `fanout` boundaries. A chain starts with `reply` set to nil - pass context through the `input` parameter instead - and with a clone of the caller's `var`. A `jump()` inside a chain moves within the chain, and a `return` inside a chain ends the chain, not the run. Sections are referenced by heading string.

### Sandbox Constraints

The Lua sandbox provides only `string`, `table`, and `math` standard libraries. Dangerous globals (`load`, `dofile`, `require`, `print`, `rawget`, `rawset`, `collectgarbage`) are removed. A runaway Lua block is automatically aborted after exceeding the instruction budget (approximately 10 million instructions). Per-VM memory ceiling defaults to 64 MiB. The `log()` function accepts messages limited to 256 Unicode scalars with no newlines or control characters.

Tool and model aliases must match `[A-Za-z][A-Za-z0-9_-]{0,63}`.

---

## Models

Models are declared by capability description and resolved semantically against a model catalog at runtime.

### Declaring and Binding

```lua
-- Declare a model by what you need it to do
models.bind("writer", "a creative writing model", {
    thinking = true,
    temperature = 0.7,
    context = 128000,
    max_tokens = 4096
})

-- Set it as the prompt-wide baseline
models.default("writer")
```

The `models.default(alias, description, opts)` form declares and designates in one atomic call; the single-alias form designates a model already declared with `models.bind`. Within sections, `models.use(alias)` selects a specific model and returns its handle:

```lua
local analyst = models.use("analyst")
```

Sections without `models.use` inherit the `models.default` baseline. A prompt can carry both - the baseline applies everywhere a section does not override it. Sections with non-empty prose but no model binding receive a clear error.

### Hard Constraints

The opts table filters the catalog before semantic resolution:

- `thinking` - boolean, required or forbidden
- `context` - minimum context window (positive integer)
- `temperature` - float in range 0.0 to 2.0
- `max_tokens` - positive integer

Duplicate model aliases or duplicate `models.default` calls are rejected atomically. `models.use` may be called at most once per section.

### Model Inference from Lua

`infer` has one shape: a single tool-free inference round on a fresh conversation. It never sets `reply` and never touches `sys`. Two forms exist:

```lua
-- The section's current model (the models.use selection, else the models.default baseline)
local tag = models.infer("One-word sentiment of: " .. args)

-- Any declared model, via its handle
local critic = models.get("critic")
local review = critic:infer("Critique this draft: " .. reply)
```

`models.get(alias)` returns the handle for a declared model without changing the section's model selection, so `handle:infer` is the way to consult a different model inside a section. A Lua block that needs tools uses `execute` on a section instead.

### Inspecting Model Properties

After binding, a model handle's frozen properties are accessible from Lua: `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, and `max_tokens`.

### Model Catalog

The library fetches a live model catalog from a gateway's `GET /v1/models` endpoint with bearer authentication. The caller provides a model catalog built from descriptors with identity, description, context window, and thinking mode (Always, Switchable, or Never).

---

## Tools

### Declaring Tools

Tools are declared by capability description and resolved semantically at runtime via a picker:

```lua
-- Declare a tool binding
local search = tools.bind("search", "web search capability")

-- Promote to prompt-wide scope (available in all sections)
tools.always("search")
```

A tool declared with `tools.bind` is not exposed to the model unless `tools.always` or `tools.add` is called. This is explicit - you control exactly what the model sees.

```lua
-- Section-local scoping
tools.add("search")            -- by alias string
tools.add(search)              -- by handle object
tools.add({"a", "b", tool_c}) -- arrays of strings or handles
```

`tools.add` calls are atomic: a failure rolls back all entries. An empty add is a no-op.

### Tool Properties

After `tools.bind`, the returned handle exposes: `name`, `description`, `parameters` (JSON schema), `wire_name`, and `untrusted` flag. Tool objects are frozen - assigning a field errors. The model-facing description is overridden positionally at declaration or scoping time:

```lua
tools.bind("search", "web search capability", "Search the web for current information")
tools.always("search", "Search the web for current information")
tools.add("search", "Search the web for current information")
```

Precedence is `add` over `bind`/`always` over the catalog description.

### Tool Dispatch Loop

The tool loop runs the model in a cycle: dispatch tool calls, feed results back, re-prompt until the model produces a final text reply or the iteration cap is reached (default 24 rounds, configurable via `max_tool_iterations` in frontmatter).

### Tool Safety

Untrusted tool output is wrapped with a CSPRNG nonce envelope before reaching the model, preventing prompt injection. One nonce per run; envelopes are deterministic within a run. Trusted tool output passes verbatim. Trust marking is mandatory at construction time.

Near-duplicate tools in the same section scope are detected and rejected before any model call, with similarity diagnostics. Out-of-scope tool calls produce a clear error distinguishing globally-declared-but-unscoped tools from truly unknown ones.

### Tool Call Counts

Per-alias call counts are tracked during execution. Read them from Lua to measure or assert model behavior:

```lua
tools.add("search")
```

After the prose block runs with the tool loop:

```lua
if tools.calls.search == 0 then
    log("model never searched")
end
```

Counts increment even when a tool call fails. Mistyped aliases produce a hard error with the available scope listed.

### Local Tools

`tools.add_local(alias, description, params, handler)` declares a tool backed by a Lua function, available from any H2 Lua block. When the model calls the tool, the handler runs synchronously in the declaring section's VM rather than reaching an external service:

```lua
tools.add_local("grab", "Grab a value from the store", {
    key = {"string", "Store path to read"},
}, function(args)
    return store.read(args.key)
end)
```

The alias must be unique within the section. It cannot reuse an alias declared by `tools.bind` or `tools.always`, and a second `tools.add_local` call with the same alias is an error.

The params table maps each parameter name to a bare type string or a `{type, description}` array. Supported types are `"string"`, `"integer"`, `"number"`, and `"boolean"`; all declared parameters are required. The handler receives the arguments as a Lua table and returns a string. It shares the section's VM (store, `var`, globals), may call `execute()`, `fanout`, and the `infer` forms (`models.infer(prompt)`, `handle:infer(prompt)`), and cannot call `jump()`. Local tool output is trusted - no nonce envelope. A local tool becomes visible to the model starting from the next prose block.

### Implementing Custom Tools

A custom tool requires:

- A stable `ToolId` (server + name pair)
- A wire name matching `[A-Za-z0-9_.-]`
- A description string
- A JSON-Schema parameters definition
- An async `call` method returning `ToolOutput` (marked trusted or untrusted)

Tools can run locally in-process or proxy through a remote gateway, both dispatched uniformly through the `Tool` trait.

### Built-in Web Search

The web search tool sends queries through a gateway proxy so the search provider credential never leaves the server. Results are automatically marked as untrusted output. Parameters include count (1-20), freshness filter (pd/pw/pm/py), SafeSearch level (off/moderate/strict), domain inclusion/exclusion lists (up to 20 each), country code, and language code.

---

## Fanout

`fanout(worker, collection)` maps a worker section over a collection in parallel. Each member is processed by its own isolated execution arm with a fresh Lua VM.

````markdown
## Process

```lua
local results = fanout("### Worker", list_from_section("### URLs"))
var.output = table.concat(results, "\n\n")
```

### Worker

Fetch and summarize: {{ item }}

### URLs

- https://example.com/page1
- https://example.com/page2
- https://example.com/page3
````

The worker is referenced by markdown heading address (level + name). The second parameter is always a collection, never a section name: any Lua table works, and `list_from_section("### List")` feeds a list section's pre-parsed items straight in. An empty collection is an error - no work is likely a bug.

### Arm Execution

Each arm receives the current member as the `item` variable, a `sys.index` giving its 1-based position within the current fanout, and a unique run-global `sys.id`. Each arm starts with a fresh clone of the caller's `var` - arm writes to `var` never reach the caller. The arm can:

- Run Lua blocks that short-circuit before any prose (enabling pure-Lua map operations)
- Substitute `{{ item }}` in prose
- Run the full model tool loop
- Run Lua blocks after the prose for post-processing

Results are returned in collection order (not finish order). Each result has `.text`, `.ok`, `.item`, and `.exhausted` fields; an arm that produces no reply yields `.text == ""` with `.ok == true`. The result array supports `table.concat` since objects coerce via `__tostring`. All arms share the run's store: two arms of one fanout writing the same path is a hard error (a write-write race), while `store.append` from concurrent arms stays legal.

### Resilience

An exhausted arm (tool loop budget exceeded) soft-degrades into an incomplete stub rather than failing the entire fanout. A fatal error in any arm aborts all sibling arms, preventing wasted work. Cancellation propagates from the parent into each spawned arm cooperatively.

Default concurrency is 8 parallel arms, configurable via `RunLimits`.

---

## Store

The store is a run-scoped virtual filesystem shared across all sections. Data persists within a single run and the handle is thread-safe across concurrent tasks.

```lua
store.write("notes/summary.md", "# Summary\n" .. reply)
store.append("log.txt", "processed: " .. args .. "\n")

local content = store.read("notes/summary.md")
local numbered = store.read_numbered("notes/summary.md")

store.str_replace("notes/summary.md", "old text", "new text")

local files = store.glob("notes/*.md")
local exists = store.exists("notes/summary.md")

store.delete("notes/summary.md")
```

`store.delete` on a missing path is silent - delete is idempotent. Within a single `fanout`, two arms calling `store.write` on the same path is a hard error (a write-write race); `store.append` from concurrent arms stays legal.

### Safe Injection

Wrap stored content in the untrusted-input guard envelope with the `untrusted(s)` global before re-injecting it into model prompts: `untrusted(store.read(path))`. Forged close-tags in stored content are escaped, so injected data cannot break out of the envelope:

```lua
store.write("user-data.txt", user_provided_content)
-- Later, safely inject into a prompt context:
local safe = untrusted(store.read("user-data.txt"))
```

### Path Validation

All store paths are validated:

- Forward-slash only (backslash rejected)
- No path traversal (`.` and `..` segments rejected)
- No Windows reserved device names (CON, NUL, COM1-9, LPT1-9)
- No trailing dots or spaces
- Maximum 1024 bytes

### Glob Matching

- `*` matches within a single path segment
- `**` matches across path separators
- Unsupported syntax (backslash escapes, triple-star, misplaced `**`) is rejected
- Matching uses a bounded, non-backtracking algorithm

The `str_replace` operation requires the old text to be unique in the file; ambiguous matches are refused with a count of occurrences.

The default in-memory backend (`StoreRef::memory()`) requires no filesystem or network and drops cleanly with the run. Custom backends implement the `Store` trait.

---

## Gateway Client

The gateway client sends requests to an OpenAI-compatible chat completions endpoint with bearer authentication.

### Configuration

Set two environment variables:

```bash
export PROMPTFORGE_GATEWAY_URL="https://your-gateway.example.com"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
```

Or construct programmatically:

```rust
let client = GatewayClient::new(endpoint, key);
```

Point `PROMPTFORGE_GATEWAY_URL` at a local server or another gateway to retarget. The credential is automatically redacted in Debug output, Display, and logs. Empty credentials are rejected at construction time.

Gateway URLs are validated: non-HTTP schemes, embedded credentials, query strings, and fragments are rejected. Trailing slashes are normalized.

For testing, `GatewayClient::disabled()` creates a client that always returns a Disabled error.

---

## Observation and Debugging

### Observer

The observer is a pluggable, report-only seam for watching execution in flight. Implement the `Observer` trait:

```rust
fn observe(&self, execution: &str, section: &str, event: Observation<'_>);
```

Events include parse started/completed, run started/succeeded/failed, section started/finished, model turn completed/truncated, tool call succeeded/failed, store operations, fanout arm lifecycle, and Lua log checkpoints. All observations are correlated by execution id and section name.

`NullObserver` discards all events when no tracing is needed. Attaching or detaching an observer does not change execution results.

### Debug Capture

A separate debug sink records raw request and response JSON for each model turn:

```rust
fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
```

Debug events capture the full request body as JSON and the response finish reason with reasoning content. Events from nested `model:infer` calls and fanout arms are forwarded to the same sink.

### Cancellation

Cancellation is cooperative via a caller-supplied `CancelHandle`. It propagates into tools, models, Lua instruction hooks, and fanout arms. A cancelled run returns a `RunError` with `is_cancelled() == true`, distinguishable from faults.

---

## Error Handling

Every public boundary returns its own typed error rather than one crate-wide error type. Each error exposes a stable `kind()` classifier for programmatic handling without matching on private representations. Public structs are `#[non_exhaustive]` so they evolve without breaking downstream code.

| Error | Kinds | Queries |
|-------|-------|---------|
| `RunError` | Parse, Version, Binding, Completion, Tool, Store, Lua, Quota, Substitution, Cancelled, Internal | `is_retryable()`, `is_cancelled()` |
| `CompletionError` | Transport, Backend, MalformedResponse, EmptyReply, Disabled, Config | `is_retryable()`, `is_timeout()`, `status()` |
| `StoreError` | NotFound, Anchor, InvalidAnchor, InvalidPath, InvalidPattern, Backend | `is_not_found()`, `path()` |
| `ToolError` | InvalidArguments, Backend, Transport, Cancelled, Other | `is_retryable()`, `is_cancelled()` |
| `ParseError` | (by kind) | `kind()`, `span()` |

Backend error bodies are accessible through opt-in accessors but never leak into Display output.

`promptforge_version(source)` detects whether a file is a promptforge prompt without requiring a full parse - it needs only the `promptforge:` key.
