# Errors

PromptForge uses typed errors at every public boundary rather than a single
crate-wide error enum. Each error type exposes a stable `kind()` classifier
for programmatic handling, and public structs are `#[non_exhaustive]` so they
can evolve without breaking downstream code.

This chapter covers the error taxonomy across all three crates and the
debugging facilities that help you diagnose failures in practice.

## Core Error Types

The core library defines five error types, each with its own set of kinds
and boolean query methods. Backend error bodies are accessible through
opt-in accessors but never leak into `Display` output.

| Error | Kinds | Queries |
|-------|-------|---------|
| `RunError` | Parse, Version, Binding, Completion, Tool, Store, Lua, Quota, Substitution, Cancelled, Internal | `is_retryable()`, `is_cancelled()` |
| `CompletionError` | Transport, Backend, MalformedResponse, EmptyReply, Disabled, Config | `is_retryable()`, `is_timeout()`, `status()` |
| `StoreError` | NotFound, Anchor, InvalidAnchor, InvalidPath, InvalidPattern, Backend | `is_not_found()`, `path()` |
| `ToolError` | InvalidArguments, Backend, Transport, Cancelled, Other | `is_retryable()`, `is_cancelled()` |
| `ParseError` | (by kind) | `kind()`, `span()` |
| `DialectError` | NoMatch, Tie, Unknown | `kind()` |

### RunError

`RunError` is the top-level error returned by the `run` function. It wraps
failures from every subsystem - parsing, model inference, tool dispatch, Lua
execution, and store operations - into a single type with discriminated kinds.

```rust
match run(&prompt, input, ctx, &store, config).await {
    Ok(result) => println!("{result}"),
    Err(e) if e.is_cancelled() => println!("run was cancelled"),
    Err(e) if e.is_retryable() => println!("transient failure: {e}"),
    Err(e) => println!("fatal: {e}"),
}
```

### CompletionError

`CompletionError` covers model inference failures. The `is_retryable()` query
distinguishes transient network issues from permanent configuration problems,
and `is_timeout()` identifies request timeouts specifically. The `status()`
accessor exposes the HTTP status code when the backend returned one.

Key kinds:

- **Transport** - network-level failure (DNS, connection refused, TLS). Retryable.
- **Backend** - the gateway or upstream returned an error HTTP status.
- **MalformedResponse** - response body could not be decoded.
- **EmptyReply** - the model returned no content.
- **Disabled** - the client was constructed with `GatewayClient::disabled()`.
- **Config** - missing or invalid client configuration (bad URL, empty key).

### StoreError

`StoreError` covers virtual filesystem operations. The `is_not_found()` query
identifies missing-file reads, and `path()` returns the offending path when
available.

Path validation rejects backslashes, traversal segments (`.` and `..`),
Windows reserved device names, trailing dots or spaces, and paths exceeding
1024 bytes.

### ToolError

`ToolError` covers tool dispatch failures. Tools can fail from invalid
arguments, backend errors, transport problems, or cancellation. The
`is_retryable()` and `is_cancelled()` queries work the same as on `RunError`.

### ParseError

`ParseError` reports prompt-file structural problems at parse time. Each error
carries a stable `kind()` discriminant and an optional byte `span()` for editor
diagnostics. Lua compilation errors include absolute source-line numbers that
map back to the original prompt file.

### DialectError

`DialectError` fires when tool-calling dialect resolution fails:

- **NoMatch** - no dialect matched the model's evidence.
- **Tie** - multiple dialects matched equally.
- **Unknown** - the dialect name was not recognized.

### Version Detection

`promptforge_version(source)` detects whether a file is a promptforge prompt
without requiring a full parse - it needs only the `promptforge:` key in the
frontmatter. Use this for fast filtering before committing to a parse.

## Gateway Errors

The gateway uses the OpenAI error envelope for all HTTP error responses,
so an unmodified OpenAI SDK surfaces these as its own error types rather
than unparseable blobs.

```json
{
  "error": {
    "message": "unknown model reasoning-large",
    "type": "invalid_request_error",
    "code": "model_not_found"
  }
}
```

### Error Codes

| Condition | Status | `type` | `code` |
|-----------|--------|--------|--------|
| Wrong or missing bearer | 401 | `authentication_error` | `unauthorized` |
| Unknown model | 404 | `invalid_request_error` | `model_not_found` |
| Tool not configured | 404 | `invalid_request_error` | `not_found` |
| Bad request body | 400 | `invalid_request_error` | `malformed_request` |
| Backend unreachable | 502 | `server_error` | `upstream_transport` |
| Backend decode failure | 502 | `server_error` | `upstream_protocol` |
| Backend 4xx | upstream's | `invalid_request_error` | `upstream_client_error` |
| Backend 5xx | 502 | `server_error` | `upstream_error` |
| Queue full | 503 | `server_error` | `queue_full` |

A 502 with `upstream_transport` is a network-level failure (DNS, connection
refused, TLS handshake). A 502 with `upstream_protocol` means the backend
responded but its body could not be decoded. Both are transient and worth
retrying.

A `queue_full` 503 means every concurrency slot on the endpoint is occupied and
the queue depth has been exceeded. Retry after a back-off.

### Boot-Time Failures

The gateway validates its TOML configuration strictly. Every configuration
struct uses `deny_unknown_fields`, so a misspelled key is a boot failure
rather than a setting silently ignored. An unresolved `${VAR}` reference
fails the load, so a deployment that forgot to export a credential never
starts serving with a blank one.

## MCP Server Errors

The MCP server surfaces errors through the `run_prompt` result envelope.

### Run Result Status

Every `run_prompt` result carries a `status` field:

- **`completed`** - the run finished successfully; `value` contains the artifact.
- **`failed`** - the run finished with an error; `error` contains the message.
- **`running`** - the run exceeded `reply_deadline` and continues in background.

```json
{
  "run_id": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
  "prompt": "research_person",
  "status": "failed",
  "value": null,
  "turns": 1,
  "elapsed_ms": 3200,
  "error": "CompletionError: transport timeout after 120s"
}
```

### Admission and Timeouts

- **Admission timeout** - when all `max_concurrent_runs` slots are occupied and
  none frees within `admission_timeout` (default 30s), the call gets a
  retryable refusal.
- **Reply deadline** - when a run exceeds `reply_deadline` (default 240s, inside
  Cursor's 300s call ceiling), the call returns immediately with status
  `running` and a `run_id`. Collect the result later with `check_run`.

### Gateway Connectivity

At startup, the MCP server distinguishes transient gateway failures from
fatal ones:

- **Transient** (connection timeout, 5xx) - the server warns and serves with an
  empty model catalog. Prompts that do not use `models.bind` keep working.
- **Fatal** (401, bad URL, malformed response) - the server refuses to boot
  rather than hiding a misconfiguration behind runtime failures.

### Catalog Errors

A broken prompt (parse error, invalid name) appears in `list_prompts` with its
`problem` field populated rather than silently disappearing:

```json
{
  "name": "broken_one",
  "description": "",
  "problem": "parse error at line 3"
}
```

## Gateway Client Configuration

The gateway client is how errors from the model layer surface in practice.
Two environment variables configure it:

```bash
export PROMPTFORGE_GATEWAY_URL="https://your-gateway.example.com"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
```

Or construct programmatically:

```rust
let client = GatewayClient::new(endpoint, key);
```

Point `PROMPTFORGE_GATEWAY_URL` at a local server or another gateway to
retarget all model calls. The credential is automatically redacted in `Debug`
output, `Display`, and logs. Empty credentials are rejected at construction
time.

Gateway URLs are validated at construction:

- Non-HTTP schemes are rejected
- Embedded credentials are rejected
- Query strings and fragments are rejected
- Trailing slashes are normalized

For testing, `GatewayClient::disabled()` creates a client that always returns
a `Disabled` error - useful for running parse-only or Lua-only tests without
a live gateway.

Without an explicit `.client()` on `RunConfig`, the runtime lazily constructs
one from the environment variables above.

## Observation and Debugging

### Observer Trait

The observer is a pluggable, report-only seam for watching execution in
flight. Implement the `Observer` trait:

```rust
fn observe(&self, execution: &str, section: &str, event: Observation<'_>);
```

Events include:

- Parse started/completed
- Run started/succeeded/failed
- Section started/finished
- Model turn completed/truncated
- Tool call succeeded/failed
- Store operations
- Fanout arm lifecycle
- Lua log checkpoints

All observations are correlated by execution id and section name.
`NullObserver` discards all events when no tracing is needed. Attaching or
detaching an observer does not change execution results.

### Debug Capture

A separate debug sink records raw request and response JSON for each model
turn:

```rust
fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
```

Debug events capture the full request body as JSON and the response finish
reason with reasoning content. Events from nested `model:infer` calls and
fanout arms are forwarded to the same sink.

### Cancellation

Cancellation is cooperative via a caller-supplied `CancelHandle`. It
propagates into tools, models, Lua instruction hooks, and fanout arms.

```rust
let cancel = CancelHandle::new();

// From another task:
cancel.cancel();

// The run returns:
match result {
    Err(e) if e.is_cancelled() => { /* clean shutdown */ }
    _ => {}
}
```

A cancelled run returns a `RunError` with `is_cancelled() == true`,
distinguishable from faults. In the MCP server, client disconnection during
a run triggers cancellation cooperatively.
