# Papergate migration to Harness

A note for the `wg21-paperflow` repository. Papergate (`crates/papergate`) is a command-line tool that runs the vendored `papergate.md` prompt against one WG21 paper and prints the report. It was written against `promptforge-core`, a crate that no longer exists, and against `promptforge-tool-picker`, which was removed with the tool picker. Its path dependencies are broken today, so this migration starts from a build that does not compile, not from a working one.

The Engine (`promptforge-api-runtime`) is now sans-I/O: every network call and clock reading is an effect it hands to the Harness, which performs it and returns the answer. The Harness, the Engine's only production caller, has the public surface `harness`. Papergate stops driving the Engine itself and drives a Harness session instead, the same way Workshop does. The Harness owns the tokio performers, the model client, the run log, and the run's store; Papergate supplies the gateway binding as data and reads the run's events back.

## Dependency change

Replace both path dependencies with one:

```toml
[dependencies]
harness = { path = "../../../promptforge/crates/harness" }
```

`harness` is the one door into `crates/harness-internal/`; nothing under that directory may be named directly. It re-exports every type Papergate needs. `promptforge-api-runtime` and `promptforge-api-types` remain public and may be added for the `Event` enum and `RunError` when typed access to event payloads is wanted; the session hands events over as `serde_json::Value`, so they are optional.

The `tokio` dependency stays. `Harness::launch` is async and the Harness spawns its performers on the runtime the caller is already inside; the multi-threaded runtime is no longer a requirement of the Engine (the Engine blocks nothing), so `#[tokio::main]` may stay as it is or drop to `flavor = "current_thread"`.

## Call-by-call replacement

Each row is one thing Papergate does today (`src/app.rs`, `src/main.rs`) and what replaces it.

| Today (`promptforge-core`) | Replacement (`harness`) |
|---|---|
| `Prompt::parse(&source, &execution, observer.as_ref())` returning `Result<Prompt>` and reporting parse events to the observer | Nothing: the Harness parses at launch. The Engine's own signature is now `Prompt::parse(input, execution) -> (Result<Prompt, ParseError>, Vec<Event>)`, with no observer parameter; the second element is the parse-time events, which the Harness records in the run log ahead of the run's own events and replays them into the session's event stream. A parse failure surfaces as a `parse_failed` event in the transcript and a report on `Session::subscribe_errors`. |
| `Arc<dyn Observer>` and the `StderrObserver` printing `[{execution}] {section}: {event}` per `Observation` | The `Observer` trait and `Observation` enum are gone. Subscribe to `Session::subscribe_events()` (a `broadcast::Receiver<SessionEvent>`; each carries `index`, an optional `reply` id, and `event`, the logged Engine `Event` as JSON with a `kind` tag, `execution`, `section`, and `provenance`). Print `event["section"]` and `event["kind"]` for the same progress line. `Session::transcript(from)` reads the same sequence from the log after the fact. Live model text arrives separately on `Session::subscribe_deltas()`. |
| `fetch_model_catalog(&endpoint, &token)` and `ResolutionContext::new(&picker, &models, &ToolCatalog::new(&[])?)` | Nothing to call: the Harness fetches the catalog and binds the prompt's `writer` role itself at launch. The Harness resolves the model from `HostSnapshot::selected_model`, or, when that is `None`, from the first entry of the `CatalogBinding` it was given; with neither, the role stays unbound and the launch is refused with the Engine's requirements notice. Papergate pushes one of the two before launching (see "Model selection" below). |
| `promptforge_tool_picker::{Catalog, Config, ToolPicker}` built over an empty catalog | Gone. The Harness assembles the tool catalog from the prompt's `capabilities:` declarations against its capability registry. `papergate.md` declares no capabilities and defines its one tool with `tools.add_local`, so nothing replaces this. |
| `RunConfig::new(execution).observer(observer).cancel(cancel)` and `execute::run(&parsed, "", resolution, &store, config).await` | `Harness::new(HarnessConfig { agents_path, state_dir })`, then `Harness::set_gateway(GatewayBinding { base_url, key, generation })`, then `Harness::launch(LaunchRequest { agent: "papergate".into(), args, input_text: Some(paper_md) }).await -> Result<Session, LaunchError>`. The session runs the agent to completion; await `Session::subscribe_state()` reaching `SessionState::Closed`, or watch the transcript for `run_succeeded` or `run_failed`. |
| `execution` id minted with `fastrand` as `papergate-<hex>` | The Harness mints the session id (`SessionId::fresh()`, 128 random bits) and uses it as the run's `execution`. Read it back with `Session::id()`. Drop `fastrand` unless it is used elsewhere. |
| `promptforge_core::CancelHandle::new()`, `.clone()`, `.cancel()` from the Ctrl-C task; `RunError::is_cancelled` for exit code 130 | `harness::cancel::CancelHandle` has the same `new`, `child`, `cancel`, `is_cancelled` and adds the awaitable `cancelled()`, plus the task-local helpers `scope`, `maybe_scope`, `current`, `wait_cancelled`, `is_cancelled`. It moved here from the Engine because the Harness owns the cancel flag. For the session itself, Ctrl-C calls `Session::close()` (cancel for good: outstanding effects are answered `Dropped`, state drains to `Closed`), not `Session::cancel()` (a turn cancel that relaunches the program). The durable `run_failed` event carries no reason, so detect the cancelled ending in Papergate: close was requested and then `Closed` arrived. |
| `FileStore::new(temp_dir)`, `StoreRef`, `seed_store` writing `paper.md`, `read_report` reading `report.md`, `remove_dir_all` afterwards | `LaunchRequest::input_text` carries the paper, which the Harness stages at the prompt's declared `input:` path, and `Session::output_text()` returns what the run left at the declared `output:` path. No store directory to create or remove. See "Declared files" below. |
| `PROMPTFORGE_GATEWAY_URL`, `PROMPTFORGE_GATEWAY_API_KEY` from the environment | Keep the variables; they populate `GatewayBinding { base_url, key, generation: 1 }`. Note `GatewayBinding::api_root()` appends `/v1` to `base_url`, so the URL variable must hold the gateway origin without the `/v1` suffix (or Papergate strips it). |
| `Prompt` source read from `--prompt <PATH>` or the embedded `DEFAULT_PROMPT` | The Harness launches agents by discovered name: the `.md` file stems under `HarnessConfig::agents_path`. Papergate writes its prompt source to `<agents_path>/papergate.md` (a temporary directory is fine) and launches `"papergate"`. `--prompt` writes the given file's contents to that path instead. |
| Model-readable failure text from `execute::run` (`RunError`) | `LaunchError` for a refused launch (`UnknownAgent`, `GatewayUnusable`, `SessionState`, `Log`); `Session::subscribe_errors()` for a run that ended in error; `run_failed` in the transcript for the durable record. |

## Model selection

A session launches only once the Harness holds a catalog with at least one model: `Harness::set_catalog(CatalogBinding { generation, models })` with an empty or absent `models` list parks the session in a waiting state, and the run never starts. Workshop supplies the gateway's chat-capable list; Papergate has no menu and today binds `writer` to whatever `models.default` resolves against the fetched catalog.

The Harness fetches the gateway's model list itself at launch and checks the selection against it (`SelectionAbsent` when the id is not there), so Papergate need not fetch anything. Two calls before `launch` are enough:

- `Harness::set_catalog(CatalogBinding { generation: 1, models: vec![json!({ "id": model })] })`
- `Harness::set_host(HostSnapshot { selected_model: Some(model), workspace_roots: vec![] })`

where `model` is the catalog id Papergate wants the `writer` role bound to. Take it from a new `PAPERGATE_MODEL` environment variable or a `--model` flag; there is no gateway-side default the Harness will pick for an unattended client. The model-catalog fetch helper (`fetch_model_catalog`) now lives in a private Harness crate and is not reachable from outside the family.

## Declared files

Today Papergate seeds the run store with `paper.md` before the run and reads `report.md` from it afterwards, through the Engine's `StoreRef` over a temporary directory. The prompt's frontmatter declares both paths as `input:` and `output:`, and the Harness now honors both declarations, so `papergate.md` keeps its `store.read("paper.md")`, `store.read_numbered`, and `store.write("report.md", reply)` as they are.

- `LaunchRequest::input_text` is the paper's markdown. Before each run the Harness writes it at the declared input path through the store's strict path rules. A run is refused, reported on `Session::subscribe_errors` as `FailureKind::RunFailed`, when the prompt declares no input file, or when it declares one and the launch supplies no text and the store does not already hold it.
- `Session::output_text()` returns what the completed run left at the declared output path. The Harness reads it as the run completes and before the session reports `Closed`, so await `Closed` and then call it. It returns `OutputError::Missing { path }` when the run never wrote the file (the old "did not produce its declared output" error), `OutputError::Unfinished` when no run completed (the run failed, or Ctrl-C closed it first), and `OutputError::Undeclared` for a prompt with no output file. A missing output never fails the run.
- The default filesystem is a fresh memory store per run, which is what Papergate wants: nothing persists between runs, so `evidence.md` never leaks from one paper into the next, and there is no directory to remove. A Host that wants its own filesystem (a store on real files to keep `evidence.md` for debugging, real mounts, overlays, a policy, or an operation sink) builds a `promptforge::vfs::VfsRef` and launches with `Harness::launch_with(request, LaunchOptions { vfs: Some(handle) })`. Every run of that session then works in the handle, and the Harness stages and reads the declared files through its store wherever it is mounted.

The vendored prompt also needs its version line fixed: it says `promptforge: 1`, and the Engine accepts only `promptforge: 0`, so an unfixed copy fails its run with "unsupported promptforge version: 1 (this build supports major 0)".

## The shape of the new run

```rust
let harness = Arc::new(Harness::new(HarnessConfig { agents_path, state_dir }));
harness.set_gateway(GatewayBinding { base_url, key, generation: 1 });
harness.set_catalog(CatalogBinding { generation: 1, models: vec![json!({ "id": &model })] });
harness.set_host(HostSnapshot { selected_model: Some(model), workspace_roots: Vec::new() });

let request = LaunchRequest { agent: "papergate".into(), args: String::new(), input_text: Some(paper_md) };
let session = harness.launch(request).await?;
let mut events = session.subscribe_events();
let mut state = session.subscribe_state();
// Ctrl-C task: session.close()
loop {
    tokio::select! {
        Ok(event) = events.recv() => print_progress(&event),
        Ok(()) = state.changed() => if *state.borrow() == SessionState::Closed { break },
    }
}
let report = session.output_text()?; // OutputError::Missing when report.md was never written
```

`agents_path` holds `papergate.md` (the embedded default or the `--prompt` file), and `state_dir` receives the Harness's `runs.db`; both may be temporary directories removed after the run. The run log is the durable record the old stderr observer approximated; keep `state_dir` when the transcript is worth retaining.

## Checklist

- Replace the two path dependencies with `harness`; drop `fastrand` if nothing else uses it.
- Delete `StderrObserver`, `ResolutionContext`, `RunConfig`, `ToolCatalog`, and the tool-picker construction.
- Write the prompt to `<agents_path>/papergate.md` and launch by name.
- Push the gateway binding, a one-entry catalog, and the selected model before launch; strip `/v1` from the URL variable if present.
- Move Ctrl-C to `Session::close()`; keep exit code 130 when close preceded `Closed`.
- Pass the paper as `LaunchRequest::input_text`, read the report with `Session::output_text()` after `Closed`, and delete `with_temp_store`, `seed_store`, and `read_report`.
- Change the vendored `papergate.md` to `promptforge: 0`.
- `CancelHandle` imports move from the Engine to `harness::cancel`; `RunError::is_cancelled` is no longer on Papergate's path.
