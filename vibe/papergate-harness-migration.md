# Papergate migration to harness

A note for the `wg21-paperflow` repository. Papergate (`crates/papergate`) is a command-line tool that runs the vendored `papergate.md` prompt against one WG21 paper and prints the report. It was written against `promptforge-core`, a crate that no longer exists, and against `promptforge-tool-picker`, which was removed with the tool picker. Its path dependencies are broken today, so this migration starts from a build that does not compile, not from a working one.

The engine (`promptforge-api-runtime`) is now sans-I/O: it performs no network calls, reads no clock, and holds no host callbacks. Its only production host is the harness, whose public surface is `harness`. Papergate stops driving the engine itself and drives a harness session instead, the same way Workshop does. The harness owns the tokio performers, the model client, the run log, and the run's store; Papergate supplies the gateway binding as data and reads the run's events back.

## Dependency change

Replace both path dependencies with one:

```toml
[dependencies]
harness = { path = "../../../promptforge/crates/harness" }
```

`harness` is the one door into `crates/harness-internal/`; nothing under that directory may be named directly. It re-exports every type Papergate needs. `promptforge-api-runtime` and `promptforge-api-types` remain public and may be added for the `Event` enum and `RunError` when typed access to event payloads is wanted; the session hands events over as `serde_json::Value`, so they are optional.

The `tokio` dependency stays. `Harness::launch` is async and the harness spawns its performers on the runtime the caller is already inside; the multi-threaded runtime is no longer a requirement of the engine (the engine blocks nothing), so `#[tokio::main]` may stay as it is or drop to `flavor = "current_thread"`.

## Call-by-call replacement

Each row is one thing Papergate does today (`src/app.rs`, `src/main.rs`) and what replaces it.

| Today (`promptforge-core`) | Replacement (`harness`) |
|---|---|
| `Prompt::parse(&source, &execution, observer.as_ref())` returning `Result<Prompt>` and reporting parse events to the observer | Nothing: the harness parses at launch. The engine's own signature is now `Prompt::parse(input, execution) -> (Result<Prompt, ParseError>, Vec<Event>)`, with no observer parameter; the second element is the parse-time events for the host to log, and the harness records them in the run log ahead of the run's own events and replays them into the session's event stream. A parse failure surfaces as a `parse_failed` event in the transcript and a report on `Session::subscribe_errors`. |
| `Arc<dyn Observer>` and the `StderrObserver` printing `[{execution}] {section}: {event}` per `Observation` | The `Observer` trait and `Observation` enum are gone. Subscribe to `Session::subscribe_events()` (a `broadcast::Receiver<SessionEvent>`; each carries `index`, an optional `reply` id, and `event`, the logged engine `Event` as JSON with a `kind` tag, `execution`, `section`, and `provenance`). Print `event["section"]` and `event["kind"]` for the same progress line. `Session::transcript(from)` reads the same sequence from the log after the fact. Live model text arrives separately on `Session::subscribe_deltas()`. |
| `fetch_model_catalog(&endpoint, &token)` and `ResolutionContext::new(&picker, &models, &ToolCatalog::new(&[])?)` | Nothing to call: the harness fetches the catalog and binds the prompt's `writer` role itself at launch. The harness resolves the model from `HostSnapshot::selected_model`, or, when that is `None`, from the first entry of the `CatalogBinding` it was given; with neither, the role stays unbound and the launch is refused with the engine's requirements notice. Papergate pushes one of the two before launching (see "Model selection" below). |
| `promptforge_tool_picker::{Catalog, Config, ToolPicker}` built over an empty catalog | Gone. The harness assembles the tool catalog from the prompt's `capabilities:` declarations against its capability registry. `papergate.md` declares no capabilities and defines its one tool with `tools.add_local`, so nothing replaces this. |
| `RunConfig::new(execution).observer(observer).cancel(cancel)` and `execute::run(&parsed, "", resolution, &store, config).await` | `Harness::new(HarnessConfig { agents_path, state_dir })`, then `Harness::set_gateway(GatewayBinding { base_url, key, generation })`, then `Harness::launch(LaunchRequest { agent: "papergate".into(), args }).await -> Result<Session, LaunchError>`. The session runs the agent to completion; await `Session::subscribe_state()` reaching `SessionState::Closed`, or watch the transcript for `run_succeeded` or `run_failed`. |
| `execution` id minted with `fastrand` as `papergate-<hex>` | The harness mints the session id (`SessionId::fresh()`, 128 random bits) and uses it as the run's `execution`. Read it back with `Session::id()`. Drop `fastrand` unless it is used elsewhere. |
| `promptforge_core::CancelHandle::new()`, `.clone()`, `.cancel()` from the Ctrl-C task; `RunError::is_cancelled` for exit code 130 | `harness::cancel::CancelHandle` has the same `new`, `child`, `cancel`, `is_cancelled` and adds the awaitable `cancelled()`, plus the task-local helpers `scope`, `maybe_scope`, `current`, `wait_cancelled`, `is_cancelled`. It moved here from the engine because it is a host concern. For the session itself, Ctrl-C calls `Session::close()` (cancel for good: outstanding effects are answered `Dropped`, state drains to `Closed`), not `Session::cancel()` (a turn cancel that relaunches the program). The durable `run_failed` event carries no reason, so detect the cancelled ending in Papergate: close was requested and then `Closed` arrived. |
| `FileStore::new(temp_dir)`, `StoreRef`, `seed_store` writing `paper.md`, `read_report` reading `report.md`, `remove_dir_all` afterwards | No equivalent through the door today. See "The store gap" below; it is the one item that needs a decision. |
| `PROMPTFORGE_GATEWAY_URL`, `PROMPTFORGE_GATEWAY_API_KEY` from the environment | Keep the variables; they populate `GatewayBinding { base_url, key, generation: 1 }`. Note `GatewayBinding::api_root()` appends `/v1` to `base_url`, so the URL variable must hold the gateway origin without the `/v1` suffix (or Papergate strips it). |
| `Prompt` source read from `--prompt <PATH>` or the embedded `DEFAULT_PROMPT` | The harness launches agents by discovered name: the `.md` file stems under `HarnessConfig::agents_path`. Papergate writes its prompt source to `<agents_path>/papergate.md` (a temporary directory is fine) and launches `"papergate"`. `--prompt` writes the given file's contents to that path instead. |
| Model-readable failure text from `execute::run` (`RunError`) | `LaunchError` for a refused launch (`UnknownAgent`, `GatewayUnusable`, `SessionState`, `Log`); `Session::subscribe_errors()` for a run that ended in error; `run_failed` in the transcript for the durable record. |

## Model selection

A session launches only once the harness holds a catalog with at least one model: `Harness::set_catalog(CatalogBinding { generation, models })` with an empty or absent `models` list parks the session in a waiting state, and the run never starts. Workshop supplies the gateway's chat-capable list; Papergate has no menu and today binds `writer` to whatever `models.default` resolves against the fetched catalog.

The harness fetches the gateway's model list itself at launch and checks the selection against it (`SelectionAbsent` when the id is not there), so Papergate need not fetch anything. Two calls before `launch` are enough:

- `Harness::set_catalog(CatalogBinding { generation: 1, models: vec![json!({ "id": model })] })`
- `Harness::set_host(HostSnapshot { selected_model: Some(model), workspace_roots: vec![] })`

where `model` is the catalog id Papergate wants the `writer` role bound to. Take it from a new `PAPERGATE_MODEL` environment variable or a `--model` flag; there is no gateway-side default the harness will pick for an unattended client. The model-catalog fetch helper (`fetch_model_catalog`) now lives in a private harness crate and is not reachable from outside the family.

## The store gap

Today Papergate seeds the run store with `paper.md` before the run and reads `report.md` from it afterwards, through the engine's `StoreRef` over a temporary directory. The prompt's frontmatter declares both paths as `input:` and `output:`.

Through `harness` there is no store access in either direction. A session's run is prepared over an empty host VFS (`shared_vfs::VfsRef::builder().build()`) with a fresh store mount added per run, and nothing on `Harness` or `Session` reads or writes it. The run's return value (the `RunResult::Ok(final_text)`) is written to the run log's `runs` row as `final_text`, but the session supervisor discards it and the door exposes no log reader, so a client cannot obtain it either.

Two ways to close the gap, for Papergate's own plan to choose:

1. Change the prompt, not the door. Deliver the paper as the run's argument (`LaunchRequest::args`) and have `papergate.md` read `args` instead of `store.read("paper.md")`, keeping `store.write("paper.md", args)` as its first statement if the `read_numbered` line ranges in `### Evaluate` are to stay as they are. Deliver the report as model text: the `## Analyze` section's `models.infer(prose)` already produces the report, and that call leaves an `assistant_reply` event with `origin: infer` carrying the report in `text` under `section == "Analyze"`. Papergate takes the last such event from the transcript. The `input:` and `output:` frontmatter declarations become documentation only. Cost: a 32k-context paper travels as one argument string, and the report is read from an event rather than a declared output. Recommended: it needs no change to the promptforge repository (confidence: medium; depends on Papergate accepting an event as the report's channel).
2. Extend the door. Give `LaunchRequest` an optional host root (a directory mounted into the run's VFS beside the fresh store, or a set of seed files written into the store), and give `Session` a way to read the run's final text or a store file once `Closed`. This is a promptforge change with its own plan; the `HostSnapshot::workspace_roots` field already exists and is the natural carrier, but today it feeds only the `ui()` snapshot and mounts nothing.

## The shape of the new run

```rust
let harness = Arc::new(Harness::new(HarnessConfig { agents_path, state_dir }));
harness.set_gateway(GatewayBinding { base_url, key, generation: 1 });
harness.set_catalog(CatalogBinding { generation: 1, models: vec![json!({ "id": &model })] });
harness.set_host(HostSnapshot { selected_model: Some(model), workspace_roots: Vec::new() });

let session = harness.launch(LaunchRequest { agent: "papergate".into(), args }).await?;
let mut events = session.subscribe_events();
let mut state = session.subscribe_state();
// Ctrl-C task: session.close()
loop {
    tokio::select! {
        Ok(event) = events.recv() => print_progress(&event),
        Ok(()) = state.changed() => if *state.borrow() == SessionState::Closed { break },
    }
}
let transcript = session.transcript(0).await?;
// option 1: the report is the last `assistant_reply` event with `origin: infer` under section "Analyze"
```

`agents_path` holds `papergate.md` (the embedded default or the `--prompt` file), and `state_dir` receives the harness's `runs.db`; both may be temporary directories removed after the run, as the store directory is today. The run log is the durable record the old stderr observer approximated; keep `state_dir` when the transcript is worth retaining.

## Checklist

- Replace the two path dependencies with `harness`; drop `fastrand` if nothing else uses it.
- Delete `StderrObserver`, `ResolutionContext`, `RunConfig`, `ToolCatalog`, and the tool-picker construction.
- Write the prompt to `<agents_path>/papergate.md` and launch by name.
- Push the gateway binding, a one-entry catalog, and the selected model before launch; strip `/v1` from the URL variable if present.
- Move Ctrl-C to `Session::close()`; keep exit code 130 when close preceded `Closed`.
- Decide the store gap (option 1 or 2) and, under option 1, update `papergate.md` and read the report from the transcript.
- `CancelHandle` imports move from the engine to `harness::cancel`; `RunError::is_cancelled` is no longer on Papergate's path.
