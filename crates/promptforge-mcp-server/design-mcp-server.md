# promptforge-mcp-server: a server that runs the PromptForge prompt a caller names

## Executive summary

`promptforge-mcp-server` is one binary that lets an agentic harness (Cursor, Claude Code) run a PromptForge prompt as a command. The caller names a prompt, the server runs it to completion against the gateway, reports progress while it runs, and returns what the prompt returned. One `prompts.toml` names the socket, the shared bearer, the prompts directory, the gateway, and which prompts the harness can reach.

Five decisions carry the design. A prompt is a command, so `run_prompt` is the only way to invoke one and no prompt is published as a tool of its own: nothing this server offers can be selected for a task the caller did not ask for by name. That makes the published tool list fixed at four entries - `list_prompts`, `run_prompt`, `check_run`, and `need_prompt` when the ranking engine is compiled in - for the life of the process, which is why a prompt saved a second ago is callable immediately in every client, with no reconnect, no restart, and nothing to notify. A prompt is carried on the MCP *tools* primitive and never on the `prompts` one, because execution happens here, against the gateway, rather than in the client's model. A call that outlives the client's patience is answered with a `run_id` rather than dropped, since Cursor's remote calls fail at about 300 seconds whatever the server does. And a failure is routed by who can fix it: a malformed argument shape is a JSON-RPC error the client owns, while anything the calling model could correct comes back as a result carrying what it needs to correct itself.

To operate it: run `promptforge-mcp-server serve prompts.toml` for streamable HTTP behind a shared bearer at `/mcp`, or `serve --stdio prompts.toml` for a local harness, where nothing is bound and no bearer is read. Boot resolves the whole catalog and refuses to serve on an incomplete one, printing every fault before a non-zero exit. Then write a prompt and iterate against `list_prompts` and `run_prompt`: the tool list never moves, so no client cache can go stale, and the catalog behind those two names changes freely on save.

## The key design choices

1. **A prompt is invoked only by name, through `run_prompt`.** A PromptForge prompt is a command: it runs because a user asked for a report, a developer tested a pipeline, or a scheduler fired a run, and never because a model noticed a tool that looked relevant. So no prompt is published as a tool of its own, and a model that wants to run one must name it. The cost is real and accepted: a prompt can never be called under its own name as a tool, and per-prompt typed argument schemas are permanently off the table, since every call goes through one tool with one schema. The rejected alternative, one tool per prompt, was chosen originally to let a model select from names and descriptions, which is exactly the behaviour this reverses.

2. **The published tool list is fixed at four entries and never changes.** This is the largest practical consequence of choice 1, and it retires a problem rather than solving it. Cursor's tool-list cache is per application process and Claude Code freezes its tool index at session start, so a tool list that moved would be invisible to both until a restart. This one does not move: `list_prompts` and `run_prompt` keep their names whatever the catalog holds, every call reads the catalog fresh, and a prompt written since the client connected is already reachable. The `listChanged` capability is therefore not advertised and no `notifications/tools/list_changed` is sent, which is the honest answer rather than an omission - there is nothing a client could be told.

3. **A prompt is an MCP tool, never an MCP prompt.** The `prompts/get` primitive returns messages for the *client's* model to run, which would hand execution to Cursor and leave the gateway, the Lua sandbox, and the tool pool unused. Execution belongs here, so the catalog is reached through a tool. The `prompts` primitive is not implemented at all, so there is no second discovery path to keep in agreement with the first.

4. **The tool text is written in the register of a command interpreter.** Every description says what this server executes and that the caller supplies the name. There is no trigger phrasing, no "use this when", and no claim on a situation, because a surface that competes for selection against a client's own tools is the failure choice 1 exists to prevent; a model that never calls this server is behaving correctly. Session-level `instructions` say the same two rules once, for a client that surfaces instructions but no descriptions. One sentence rides `run_prompt` and `check_run` beyond that: a prompt's value is a finished artifact written for the user to read, so pass it through as it stands rather than restating it. Without it a caller treats a report as source material and paraphrases away the artifact it just spent minutes producing.

5. **A run that outlives the client's patience is collected, not lost.** Cursor's remote streamable-HTTP calls fail at about 300 seconds, and a progress notification does not reset that clock: the specification says a client *may*, and Cursor does not. So a call blocks for at most `reply_deadline` (default 240 seconds, inside the wall with margin), and past it returns a result whose `status` is `running`, carrying a `run_id`, while the run continues detached. `check_run` collects it by id, and a finished record stays collectable for `retain_completed` (default one hour). The two alternatives both lose something real: blocking forever wastes the whole run on the transport we ship, and always returning an id makes every two-second prompt a two-call affair.

6. **Boot either produces a complete catalog or the process refuses to serve.** A service that starts with nine of ten prompts is one whose catalog silently disagrees with its own configuration, and the client discovers that as a missing prompt with no explanation. Every resolved file must be readable, must parse, and must declare a legal name; faults accumulate and all of them print before the non-zero exit, so fixing a configuration takes one pass rather than one restart per mistake. The single silent skip is a glob-matched markdown file that declares no `promptforge:` version: a glob names a directory, and a file in it that is not a prompt is not the operator's mistake.

7. **A reload is judged per prompt, and a broken prompt stays visible carrying its error.** Refusing a whole candidate is right at boot, where nothing depends on the service yet, and wrong on save, where one typo in one file would freeze every other prompt in the catalog. A prompt that fails revalidation is retained as a broken entry: still in `list_prompts` with its `problem`, and answering a call with that parse failure. Serving the last good copy instead would be the worst of the three options, because the developer would watch a stale version run with no way to tell. A fault about the catalog as a whole - a stale override, two prompts under one name, an empty result - still keeps the previous catalog whole, because there is no partial answer to give.

8. **The catalog is assembled by glob and corrected by name.** `[catalog].include` and `exclude` are glob lists over the prompts directory, and a `[prompts.NAME]` block is an exception: drop one with `enabled = false`, or reach a file no glob matches by naming it. Directory patterns are what make "everything I write here is available" a one-line statement, while a named block keeps per-prompt control without turning folder layout into policy. The rejected alternative, an include block per pattern, would mean moving a file silently changes what the harness sees and two matching patterns need a precedence rule. A named block with no `file` that matches no globbed prompt fails the boot, so a stale override is never a silent no-op.

9. **A prompt's frontmatter name is the name a caller passes, verbatim.** Nothing derives or transforms it, because a transformation could map two legal, distinct prompt names onto one callable name, and a name is what the calling model types. So the name must match `^[a-z][a-z0-9_]{0,47}$`, and two prompts declaring one name fails the boot naming both files. The four built-in names are reserved even though nothing shadows them any more: "run `check_run`" is ambiguous to a person and to a model alike, and a boot refusal naming the file is the only version of that a prompt author can act on. `need_prompt` is reserved whether or not this build publishes it, since a name legal in one build and illegal in another is worse than one that is never legal.

10. **A guessed prompt name is a correctable answer, not a protocol error.** `run_prompt` takes a name the calling model produced, and a model that skipped the listing will guess. Resolution folds case and treats `-` and `_` as the same character (safe, because a legal name may contain neither), then matches exactly, and never runs a near miss: fuzzy matching onto a different prompt spends minutes of gateway time producing the wrong artifact, and the caller gets a plausible result for a prompt it did not ask for. An unresolvable name comes back as `isError: true` listing the enabled names, closest first by edit distance, which turns the guess into a self-correcting second call. A protocol error would be wrong here for a specific reason: some clients surface `-32602` as a hard failure the model never gets to react to, and a bad name is precisely the model's own mistake.

11. **Arguments are one string, named `args`.** The runtime takes a single raw argument string, so `run_prompt` takes the prompt's name and one optional string and no JSON Schema enters frontmatter. This costs client-side autocompletion and typed validation, and choice 1 makes it permanent rather than merely current: with one tool serving every prompt there is no per-prompt schema to attach one to.

12. **Progress is first-class, and the runtime grew a public event stream to carry it.** Without it every run is one silent multi-minute call. `promptforge-core` gained an `Observer` trait and an `Event` enum, and the server forwards them as `notifications/progress`, which a client renders as a caption changing in place. The cost is a breaking change to `execute::run` and a standing obligation to keep the event stream stable enough for the notification path to mean something. Progress buys visibility only, never time: choice 5, not this one, is what keeps a long run alive.

13. **`need_prompt` hands back three candidates and never runs one.** A plain-English capability is ranked against the catalog and up to three prompts come back, best first, for the calling model to choose among with the whole conversation in front of it. It then calls `run_prompt` with a name it was handed rather than one it guessed, which closes the gap choice 10 can only mitigate. It does not contradict choice 1, because the intent still came from a person naming what they want: the caller who was told to run "the prompt that builds a stakeholder report" without being told it is called `staker`. A retriever that bound instead of suggesting would spend minutes of gateway time on the wrong artifact, and the caller could not tell that it had.

    Its text steers the caller into author register, because the phrasing is worth more than the engine. Measured against this catalog's distractor regime, a need that restates a prompt's own documentation retrieves correctly 0.984 of the time, while the same request phrased as a user goal retrieves 0.857, and nothing about the ranking engine closes that gap. So the parameter is named `capability` rather than `need` (the first invites "Build a stakeholder position report for one entity", the second invites "I need to find out what Herb thinks about ABI"), and the register instruction with one good and one bad example sits in both the tool description and the parameter's own, since a client may surface either alone. Steering cannot enforce, so the backstop is that three candidates come back rather than one: a conversational phrasing degrades the ordering instead of producing a wrong answer.

14. **One shared bearer, on the transport that has one, checked per request.** The token belongs to the HTTP surface: it is optional in the file, required before `serve` binds a port, and never read by `serve --stdio`, where the harness that spawned the process already has whatever authority the process has. That is also why an unset `${VAR}` in `[server].token` alone drops the token instead of failing the load, so a local stdio install is not stopped by a credential its transport never reads. `/healthz` is exempt structurally, by being registered after the auth layer, so nothing inside the middleware compares a path. Two independent defences guard `/mcp`: a present-but-empty token is refused when the configuration loads, and a request with no `Bearer` credential is refused before any comparison happens, so neither a config typo nor a comparison bug can open the surface alone. The comparison itself is constant-time across the content, which hides the position of the first differing byte and so denies a caller the byte-at-a-time search that would otherwise recover the token; the length check in front of it returns early, so the configured token's length is readable from how long a refusal took, and the token's secrecy has to rest on its bytes rather than on its length. A refusal carries `WWW-Authenticate: Bearer`, which tells a client the scheme it failed rather than that the endpoint is gone. Per-request rather than per-session means a rotated token refuses an established session's next call. The accepted cost is that logs cannot distinguish callers: a leaked token is indistinguishable from legitimate use except by source address.

15. **What the server answers and what it advertises are read from one statement.** Every built-in is one row in a single table carrying its name, description, schema, and the rule that publishes it, and both `tools/list` and the dispatcher read that table. A tool the table does not publish is `-32601`, method not found, rather than a name the handler answers anyway, so the two can never drift into a tool that serves a call it never advertised - and a prompt's name, which is on no row, is method-not-found by the same rule rather than by a check somebody has to remember to write. Adding a fifth built-in is adding a row.

## The tool surface is four entries, whatever the catalog holds

| Tool | Arguments | Published |
|---|---|---|
| `list_prompts` | none | always |
| `run_prompt` | `prompt`, optional `args` | always |
| `need_prompt` | `capability` | with the `picker` feature |
| `check_run` | `run_id` | always |

Every property of every schema is a string, because a prompt name, a raw argument string, a run id, and a capability are all strings and no other property type is offered. `run_prompt` requires `prompt` alone, so a missing `args` is the empty string.

`list_prompts` returns every enabled prompt as `{name, description, version, problem}`, where `problem` is absent on a healthy prompt and carries the parse failure on a broken one. The one publication rule that survives is a property of the build rather than of the catalog: `need_prompt` needs the ranking engine compiled in.

The catalog is deliberately *not* embedded in `run_prompt`'s description. It would pay a context cost in prose no client can filter, and a client caches that description for the life of its process, so it would go stale the moment a prompt was added. Instead the description names `list_prompts` as where the names come from, the session instructions say the same once, and the error path carries the catalog for the models that skip both.

## `prompts.toml` is the whole configuration, because a run cannot be configured from the environment

The gateway URL, token, and model live in the file and the server constructs a gateway client explicitly. This is forced rather than chosen: setting a process environment variable is `unsafe` under edition 2024 and this workspace forbids unsafe, so the runtime's environment-based client cannot be driven from a config file. The runtime therefore takes an optional client, and `None` keeps the environment path the CLI uses.

One file serves both transports, which is why a setting the current transport does not read is logged as ignored rather than refused. The full surface:

```toml
[server]
bind = "127.0.0.1:9310"              # default
token = "${PROMPTFORGE_MCP_TOKEN}"   # required to bind HTTP, unread on stdio
max_concurrent_runs = 4              # default
admission_timeout = "30s"            # default
reply_deadline = "240s"              # default; must stay under the client's call ceiling
retain_completed = "1h"              # default
watch = true                         # default
watch_debounce = "500ms"             # default

[paths]
prompts = 'C:\ProgramData\promptforge\prompts'   # default: prompts

[gateway]
url = "http://127.0.0.1:8081/v1"
token = "${PROMPTFORGE_TOKEN}"
model = "claude-sonnet-4-6"          # optional; the runtime default otherwise

[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["**/_*.md", "drafts/**"]

[prompts.scratch_test]
enabled = false                      # drop one the globs caught

[prompts.staker]
file = "experiments/staker-v3.md"    # reach a file no glob matches
```

Only `[server]` and `[gateway]` are required; every other table and key has a default. Resolution order is: expand `include`, subtract `exclude`, then apply the named blocks, so a block is the last word on the prompt it names. Patterns are relative to `[paths].prompts`, `*` stops at a separator and `**` crosses one, and `exclude` matches that same relative path so `drafts/**` means the directory rather than any path containing the word. A block that names a `file` a glob already matched replaces that entry rather than adding a second, and a file whose frontmatter name is not the block's key fails naming both spellings. An unknown key fails the load rather than being silently ignored, which is what keeps a misspelled setting from reading as a default and what makes a configuration still carrying a retired key fail by name. Every duration is a string rather than a number, because TOML has no duration type and a bare integer of unstated units is precisely the ambiguity a configuration file exists to avoid.

Two details of the interpolation are load-bearing. `${VAR}` is expanded over the *parsed* document rather than the raw text, which is what attributes an unset variable to the field that carried it; that is the only way `[server].token` alone can survive one while every other field still fails the load. And an unset variable anywhere else fails the load, so the server never starts with a blank credential.

## A prompt reaches two tools, and `prompts.toml` holds no credential of its own

A prompt's frontmatter names the tools it needs, and the runner binds that list before handing it to the executor. `web_fetch` runs in this process; `web_search` proxies to the gateway on the gateway's own token, so the search credential stays with the process that already holds the LLM credentials and this file carries no non-LLM secret to protect. A name outside the two fails the run naming the tool, rather than being dropped: a run that quietly proceeded without the search it declared would return an artifact that looks finished and was assembled from nothing.

## The result carries the value, not a path

```rust
pub struct RunResult {
    pub run_id: String,
    pub prompt: String,
    pub version: u32,
    pub status: RunStatus,     // running | completed | failed
    pub value: Option<String>,
    pub turns: u32,
    pub elapsed_ms: u64,
    pub error: Option<String>,
}
```

`structuredContent` carries the whole record; the `content` text block beside it carries the plain product, which is the returned value verbatim on completion, the error on failure, and on `running` a line naming the `run_id` and telling the caller to collect with `check_run`. A failed run sets `isError`; a `running` one does not, because nothing has gone wrong. Every enum on the wire is lowercase, from a serde attribute on the type rather than a hand-written serializer, so the value a client reads is `completed` and never `Completed`. The attribute is `snake_case` rather than `lowercase`: every variant is one word today and the two spell the same strings, and `snake_case` is the one that stays readable if a later state needs two words. Re-emitting the value is not duplication: the runtime writes no output files, so there is no path to hand back instead.

Three reported numbers mean something specific. `elapsed_ms` measures the run and not the wait for one, so it starts when admission grants a slot and an `admission_timeout` spent queueing is no part of it. A run that never started reports zero rather than the microseconds spent discovering it could not. And a `running` result reports `turns` as zero, because the tally is not final and a partial count read as a total would be worse than an obvious zero.

A backgrounded run always reaches a terminal record, whatever happens to it: a supervisor owns the join handle past the reply deadline, so a run that panics or is aborted is recorded as `failed` saying it did not finish. That matters beyond the reading, because eviction only reaches a record that finished, so a run left `running` would be both uncollectable and permanent, which is a leak for the life of the process rather than a missing answer.

## Progress reports sections completed, and never a total

| Event | Frame |
|---|---|
| run started | `progress` 0, `message` the prompt's name |
| section started | `progress` the sections entered so far, from 1, `message` the section's heading |
| section finished, model turn, tool call | none; the latter two are logged |
| run finished | none; it latches the turn total the result reports |

`total` is always absent because a jump or an early return means the number of sections a run will visit is unknown when it starts, so a denominator would be a guess wearing a measurement's clothes. The client shows a changing caption rather than a filling bar. `progress` is latched with a maximum so it never decreases, which the protocol requires. A section's end sends nothing because the frame would be identical to the one already on the wire, and a client renders the caption in place, so a duplicate is invisible to the reader and pure cost on the stream.

A frame rides the SSE stream the `tools/call` POST opened, which is why the HTTP transport is streamable and sessioned rather than stateless JSON: a stateless transport has no channel to carry a notification and would reduce every run to one long opaque call. The stream is pinged every 15 seconds when it is otherwise idle, because a run that thinks for longer than the first proxy's idle timeout between sections must not look dead to it.

Progress is best-effort in both directions, and that is a deliberate trade rather than an omission. Reporting is synchronous on the run's own path, so the queue behind it is bounded at 64 frames and a frame that finds it full is dropped and counted rather than allowed to stall the run. A frame is one section boundary, so that depth is far more than any run produces in a burst: the bound is a backstop against a peer that stopped reading, not a throttle on a healthy one. The final flush before the reply is bounded at a quarter second for the same reason: a client that stalls its stream without closing it must not hold the `tools/call` reply open, because a caption nobody will read is never worth a reply nobody receives. A call carrying no `progressToken` is answered identically, with no queue and no forwarding task behind it.

A progress token names a stream that closes with its call, and the protocol offers nowhere else to send a frame, so a run that outlived its call keeps reporting into a queue with no reader and every later frame lands on the drop counter. The alternatives were worse: holding the reply open until the run ends is exactly what choice 5 refuses, and re-attaching the run to whatever stream `check_run` arrives on would make a caption's meaning depend on which call happened to be open.

## The log at its default level shows that a run happened and how long it took

A run that quietly takes too long is this service's characteristic failure, and the log is where an operator sees it, so what reaches the default level is chosen against that failure rather than by event importance. The two run boundaries are `info`: the prompt at the start, then the turn count, the elapsed time, and the outcome at the end. A run handed back as `running` is `info` under the same id the caller was given, and so is that run reaching its terminal state later, which is the only place its outcome is observable once the call has gone; between them they are the symptom of a `reply_deadline` set too short. An admission refusal is `warn`, because it means `max_concurrent_runs` is biting. A failed tool call is `warn`, because a search that came back empty is worth knowing without turning on debug and is rare enough not to flood. Everything else inside a run, and every piece of bookkeeping - section boundaries, model turns, successful tool calls, dropped frames, evicted records - is `debug`, because a section can make dozens of tool calls and burying the boundaries under them defeats the purpose.

Logs go to stdout, except on stdio, where the protocol owns stdout and a log line written there would corrupt the wire, so that transport logs to stderr.

## Where a failure lands is decided by who can fix it

A malformed argument shape is the client's own bug and returns `-32602`, which the calling model never sees and could not act on. A name this server does not publish returns `-32601`, since the caller is asking for something that does not exist rather than asking wrongly for something that does; that covers an unknown name, a prompt's own name called as though it were a tool, and `need_prompt` in a build without the ranking engine.

Everything the calling model can correct on its own comes back as a result with `isError` set and the information needed to correct it: an unresolvable prompt name carries the enabled names closest first, a refused admission names the wait it spent so the model can retry, an unknown or evicted `run_id` names the retention window so a model that polled too late learns why, and a run that started and failed carries its error and its whole record. An advertised built-in that cannot answer is also a result rather than a fault - `need_prompt` whose model would not load says so and sends the caller to `list_prompts` - because the tool was published, so the caller did nothing wrong and blaming it for the server's own state would be a lie about whose bug it is. Everything left over is `-32603`.

The failures that are the process's own rather than a caller's are four public types, split by who reads them and when: `ConfigError` for a `prompts.toml` that will not load, `ServeError` for a transport that will not start or that stops abnormally, `WatchError` for a path that cannot be watched, and `CatalogError`, which carries a list of `Fault` values rather than a single cause, because boot reports every fault it found in one pass and an operator fixing a catalog should not need one restart per mistake. A boot failure prints its whole cause chain, one indented line per link, since the outermost message alone would reach the operator as `bind 127.0.0.1:9310` with "address already in use" discarded.

## What a save changes, and what it cannot

A filesystem event under the prompts directory or on `prompts.toml` opens a debounce window that restarts on each event, and the same resolution pass boot runs then runs against the candidate, on a blocking pool rather than on a runtime worker, since it reads and parses every prompt file in the directory. The window earns its place on Windows above all: an editor saves through a temporary file and renames it into place, which arrives as several events for one save, and one settled window costs one re-resolution rather than one per event. The configuration file's *directory* is what gets watched, for the same reason - a watch on the file itself would go with the file the editor replaced - and events in it are filtered back down by file name.

Matching an event to the prompts directory is a comparison between two paths that different code spelled. A relative `[paths].prompts`, which is the shipped default, is never a prefix of the absolute path a platform watcher delivers; a backend that canonicalizes its own watch root delivers a third spelling; and on Windows one side arrives with a verbatim `\\?\` prefix that the other never has, so neither is a prefix of the other. All three forms of the root are resolved once at startup and both sides of every comparison have the prefix stripped, which is what keeps a save on Windows from being silently ignored. The root cannot move under a running server, because `[paths].prompts` is one of the settings a reload refuses to apply.

A run is one Tokio task owning its prompt, its tools, its store, and its observer, and the live catalog sits behind an `ArcSwap` replaced whole, so a snapshot already handed out is unaffected by the swap. A prompt that vanishes from disk leaves the catalog, and a run already in flight holds its catalog snapshot and finishes under the definition it started with. Nothing is announced to any client, for the reason choice 2 gives.

What does not reload is every setting the running service already wired into something: all of `[server]` (the bound socket, the bearer layer, the registry's limits, the watcher's own window), all of `[gateway]`, and `[paths].prompts`, which is the directory being watched, since resolving against a different one would publish prompts nothing is watching. Each changed setting is named in the log and the boot value is put back, so only the catalog-shaping tables take effect on save.

Two failure modes of the watch itself are handled deliberately rather than incidentally. A path that cannot be watched at all refuses the boot, because `watch = false` is the way to ask for a server without live reload and losing it silently is worse than being told to fix the path. A watch that breaks after boot is logged at error level and registered again on the next settled window, and the process keeps serving either way: a lost watch is not worth ending a server that is still answering calls, and a developer whose saves quietly stop taking effect has no other way to find out.

## Retrieval is optional twice over, and neither absence stops the server

Every runnable prompt becomes one index entry over its name and its description, which is exactly what `list_prompts` reports, so what a caller reads is what retrieval matched on. A broken prompt is never a candidate: it cannot run, so offering it would spend the caller's next call on a certain failure, and it carries no description to rank on either. The similarity floor is set to zero, against the ranking engine's own tuned default, because a floor exists to stop an unattended binding and nothing here binds unattended; three weak candidates are self-evidently weak to the model reading them, while an empty answer to a casually phrased request helps nobody.

The ranking engine is an optional dependency that is on by default. It compiles roughly 67MB of weights into the binary and its first build anywhere needs Hugging Face access, so `--no-default-features` exists for an offline or size-sensitive build and drops `need_prompt` alone. Default on, because a server whose retrieval tool depends on remembering a flag is a server whose retrieval tool does not get used. If offline builds become routine, flip the default.

With the feature compiled in, the model is loaded once at boot - before the Tokio runtime exists, since parsing the weights is seconds of blocking CPU and there is no worker yet to block - and a load that fails cannot fail the boot. Loading is the slow part and the one part that can fail on its own, while everything else about the server (every prompt, the listing, the runner, the collector) works without it, so a model that will not load is reported at error level and the process serves on.

The index is rebuilt on the same catalog swap a save already performs, and only when a content hash over every entry's name and description moved, so a body-only edit costs nothing. The rebuild reuses the model already in memory, which is what makes it one embedding pass per prompt rather than a reload of the weights: building from scratch on every save would be seconds of CPU on a directory that changes whenever a developer types `:w`. The two alternatives both cost something visible. Reloading the model in a background task leaves a window where `need_prompt` recommends a prompt that was just renamed, and rebuilding lazily makes the first call after any save wait for the weights.

The rebuild happens *after* the catalog is stored, and the order matters because the two failure modes are not symmetric. Retrieval hands a name to a caller that will pass it to the runner, so an index briefly behind the catalog offers a name the runner refuses with the correctable listing choice 10 already carries, while one briefly ahead offers a name that does not exist yet for the same reason and buys nothing. A rebuild that fails keeps the previous index and logs why, since a stale shortlist is a name `run_prompt` corrects and no shortlist at all is a tool that stopped working over one bad save.

## The runtime's event stream is a public interface, and the costliest thing here to reverse

Forwarding progress required `promptforge-core` to grow one:

```rust
pub trait Observer: Send + Sync { fn on_event(&self, ev: &Event); }

#[non_exhaustive]
pub enum Event {
    RunStarted { prompt: String, sections: usize },
    SectionStarted { completed: u32, name: String },
    SectionFinished { name: String },
    ModelTurn { section: String, turn: u32 },
    ToolCalled { section: String, tool: String, ok: bool },
    RunFinished { turns: u32, elapsed_ms: u64, ok: bool },
}

pub struct RunOptions<'a> {
    pub observer: &'a dyn Observer,
    /// `None` builds the gateway client from the environment, which is the CLI's path.
    pub client: Option<GatewayClient>,
}

pub async fn run(prompt: &Prompt, args: &str, tools: &[&dyn Tool], store: &Store, opts: RunOptions<'_>) -> Result<String>;
```

`completed` starts at 1 for the first section and never decreases. `on_event` is synchronous and must never block, await, or panic, which is the contract that lets the executor call it inline on the run's own path. `RunStarted::sections` is the count of top-level sections the prompt declares, which bounds rather than predicts what a run will visit, and this server does not forward it: a bound rendered as a denominator is the guess the progress path refuses.

This is the change most expensive to walk back. It is a breaking change to the runtime's central entry point, every call site passes `RunOptions`, and the enum is the shape a notification path now depends on: `Event` is `#[non_exhaustive]` so variants can be added, but renaming or repurposing one breaks consumers silently in the sense that matters, by changing what a caption means. Events are a report and never a decision, so dropping one cannot change a result, and that is what makes the queue behind the forwarding path allowed to lose frames.

## The `rmcp` pin is exact and the run registry forgets a restart, both deliberately

The `rmcp` dependency is pinned exactly, at `=3.1.0`. Its handler signatures and tool field set have moved across minor releases, so an upgrade should be a diff to read rather than a number to bump. This is a deliberate exception to the usual objection to exact pins, which protects a *library's* consumers from an over-constrained resolver: nothing consumes this crate.

The run registry is in memory, and a restart forgets every run, finished or not. Recovery is to fire the prompt again, which is the right trade for a service whose unit of work is a prompt the caller can re-issue. Making it durable is additive and can wait for a deployment that wants it. Admission is a refusal rather than a queue for a related reason: every waiting call holds a client connection, and a queue long enough to outlast the reply deadline would turn into a crowd of background runs the operator never sized for.

## Decide by use

- Whether `list_prompts` needs a filter argument. It returns the whole catalog today; at forty prompts a substring or keyword filter may earn its place, and adding one is additive.
- Whether three is the right shortlist size, and whether a floor above zero becomes worth having if shortlists read as noise. Both are one constant.
- Whether frontmatter should carry `keywords`, which would sharpen both the listing and retrieval.
- Whether the run registry should survive a restart.

*2026-08-03 - claude-opus-5*
