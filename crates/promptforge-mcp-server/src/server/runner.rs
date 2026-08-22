//! Running one catalog entry, and what happens when the run outlives its call.
//!
//! A call that reaches a prompt goes through four gates in order. A broken entry
//! fails before anything is reserved. Then admission: one of
//! `max_concurrent_runs` slots, waited for up to `admission_timeout` and
//! refused with a message the calling model can retry on. Then the run itself,
//! which happens on a task of its own rather than inline, and whose clock starts
//! once admission is through so the duration it reports is the run's and not the
//! queue's. Then the reply deadline, which decides whether this call carries the
//! result or a `run_id`.
//!
//! The run is a spawned task even when it finishes in milliseconds, because that
//! is what lets the deadline give up on it without cancelling it: past the
//! deadline the registry hands the join handle to a supervisor, which detaches
//! the run from this call while still owning its outcome. The work continues, the
//! record lands in the registry when it ends however it ends, and `check_run`
//! collects it.
//!
//! Progress belongs to the call that asked for it. The `progressToken` names a
//! stream that closes when this call is answered, so the pump is finished on
//! both paths - a bounded flush, then abandonment - and a run that outlived its
//! call keeps reporting into a queue with nothing on the other end, where every
//! later frame is counted as dropped. Nothing on the run's path blocks either
//! way, and the alternative, holding the stream open past its reply, is not
//! something the protocol offers.

use std::sync::Arc;
use std::time::Duration;

use promptforge_core::CancelHandle;
use promptforge_core::client::GatewayClient;
use promptforge_core::execute::{self, ResolutionContext, RunConfig};
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use rmcp::model::{CallToolResult, ErrorData};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::catalog::Entry;
use crate::config::Config;
use crate::progress::{McpObserver, ProgressPump};
use crate::registry::{DuplicateRun, RunRegistry, RunSlot, elapsed_ms};
use crate::result::{NO_TURNS, RunResult};

use super::{PreparedTools, Reporting, bind, run_result, text_error};

/// Everything one background run owns for as long as it lasts.
///
/// It is owned rather than borrowed because the run outlives the call: the
/// prompt is the snapshot the run started under, so a reload part way through
/// cannot change the definition underneath it.
struct Launch {
    /// The run's identifier, which is also how it is collected.
    run_id: String,
    /// The prompt to run, cloned from the catalog snapshot.
    prompt: Prompt,
    /// The prompt's name, as the result reports it.
    name: String,
    /// The run's whole input.
    args: String,
    /// Filesystem path to read into the prompt's store (empty if unused).
    input_file: String,
    /// Text to place directly in the prompt's store (empty if unused).
    input_text: String,
    /// Filesystem path to write the prompt's output to (empty if unused).
    output_file: String,
    /// The complete immutable live tool catalog and prepared picker.
    tools: Arc<PreparedTools>,
    /// Where the run reports itself, and what counts its turns.
    observer: Arc<McpObserver>,
    /// The gateway the run's model calls go through.
    client: GatewayClient,
    /// When the run started, for `elapsed_ms`.
    started: Instant,
    /// The admission slot, returned when this value drops.
    slot: RunSlot,
    /// The run's cancellation handle, installed on the core run so an abandoned
    /// call stops it cooperatively.
    cancel: CancelHandle,
}

/// One run's observer, correlation id, and optional progress pump.
struct Observation {
    run_id: String,
    observer: Arc<McpObserver>,
    pump: Option<ProgressPump>,
}

/// Runs one entry and answers the call that asked for it.
///
/// # Errors
/// Returns `-32603` when the result cannot be serialized. Everything the
/// calling model can act on - a broken prompt, an unresolvable capability, a refused
/// admission, a run that failed - is an `Ok` result carrying `isError`.
#[expect(
    clippy::too_many_arguments,
    reason = "the entry point threads the call's borrowed context and the run's four inputs through to run_observed"
)]
pub(super) async fn run(
    config: &Config,
    registry: &Arc<RunRegistry>,
    tools: Arc<PreparedTools>,
    entry: &Entry,
    args: &str,
    input_file: &str,
    input_text: &str,
    output_file: &str,
    reporting: Option<Reporting>,
) -> Result<CallToolResult, ErrorData> {
    let run_id = new_run_id();

    let Some(source) = entry.source() else {
        let problem = entry
            .problem()
            .unwrap_or("the prompt is unavailable")
            .to_owned();
        return run_result(&RunResult::failed(
            run_id,
            entry.name(),
            problem,
            NO_TURNS,
            0,
        ));
    };

    let (observer, pump) = match reporting {
        Some((peer, token)) => {
            let (observer, pump) = McpObserver::reporting(peer, token);
            (Arc::new(observer), Some(pump))
        }
        None => (Arc::new(McpObserver::silent()), None),
    };

    run_observed(
        config,
        registry,
        tools,
        entry,
        args,
        input_file,
        input_text,
        output_file,
        source,
        Observation {
            run_id,
            observer,
            pump,
        },
    )
    .await
}

/// Runs one validated source snapshot under an already-created observation
/// context. Keeping this boundary explicit lets the runner regression retain
/// the observer while exercising the same parse-to-run path as production.
#[expect(
    clippy::too_many_arguments,
    reason = "keeps the run's borrowed context and inputs explicit so the run_recorded test seam exercises the same parse-to-run path"
)]
async fn run_observed(
    config: &Config,
    registry: &Arc<RunRegistry>,
    tools: Arc<PreparedTools>,
    entry: &Entry,
    args: &str,
    input_file: &str,
    input_text: &str,
    output_file: &str,
    source: &str,
    observation: Observation,
) -> Result<CallToolResult, ErrorData> {
    let Observation {
        run_id,
        observer,
        pump,
    } = observation;
    let prompt = match Prompt::parse(source, &run_id, observer.as_ref()) {
        Ok(prompt) => prompt,
        Err(error) => {
            return preparation_failed(run_id, entry, error.to_string(), observer, pump).await;
        }
    };

    let Some(slot) = registry.admit().await else {
        drop(observer);
        finish_pump(pump).await;
        return Ok(text_error(refused(registry.admission_timeout())));
    };

    // The clock starts after admission, so a run's reported duration includes
    // live H1 resolution and execution but not the admission wait.
    let started = Instant::now();

    let client = match bind::gateway_client(&config.gateway) {
        Ok(client) => client,
        Err(error) => {
            drop(slot);
            return preparation_failed(run_id, entry, error.to_string(), observer, pump).await;
        }
    };

    let cancel = CancelHandle::new();
    let (result_tx, result_rx) = oneshot::channel();
    let launch = Launch {
        run_id: run_id.clone(),
        prompt,
        name: entry.name().to_owned(),
        args: args.to_owned(),
        input_file: input_file.to_owned(),
        input_text: input_text.to_owned(),
        output_file: output_file.to_owned(),
        tools,
        observer,
        client,
        started,
        slot,
        cancel: cancel.clone(),
    };

    // The supervisor owns the run from the instant it is registered, so the
    // slot and observer inside `launch` are released whether the run is
    // admitted or refused as a duplicate id.
    if let Err(DuplicateRun) = registry.launch(
        run_id.clone(),
        entry.name().to_owned(),
        cancel.clone(),
        result_tx,
        move || tokio::spawn(execute_run(launch)),
    ) {
        finish_pump(pump).await;
        return run_result(&RunResult::failed(
            run_id,
            entry.name(),
            "the run id was already in use".to_owned(),
            NO_TURNS,
            0,
        ));
    }

    let result = registry.settle(&run_id, entry.name(), result_rx).await;
    if let Some(pump) = pump {
        pump.finish().await;
    }
    run_result(&result)
}

/// Drives the production runner lifecycle while retaining its observer for a
/// correlation regression.
#[cfg(test)]
pub(super) async fn run_recorded(
    config: &Config,
    registry: &Arc<RunRegistry>,
    tools: Arc<PreparedTools>,
    entry: &Entry,
    args: &str,
    observer: Arc<McpObserver>,
) -> Result<CallToolResult, ErrorData> {
    let run_id = new_run_id();
    let source = entry
        .source()
        .expect("the recorded runner fixture must be a healthy entry");
    run_observed(
        config,
        registry,
        tools,
        entry,
        args,
        "",
        "",
        "",
        source,
        Observation {
            run_id,
            observer,
            pump: None,
        },
    )
    .await
}

/// Runs one prompt to a [`RunResult`].
///
/// This is the body of the spawned task, so nothing here borrows from the call
/// that started it. Its returned result is what the supervisor in
/// [`RunRegistry::launch`] records; the run installs its own
/// [`CancelHandle`](promptforge_core::CancelHandle) so an abandoned call stops
/// it cooperatively and it ends as the failure the core reports.
async fn execute_run(launch: Launch) -> RunResult {
    let Launch {
        run_id,
        prompt,
        name,
        args,
        input_file,
        input_text,
        output_file,
        tools,
        observer,
        client,
        started,
        slot,
        cancel,
    } = launch;

    // Build the store from the declared input, if an input source was provided.
    let store = match mock_store(&prompt, &input_file, &input_text) {
        Ok(store) => store,
        Err(message) => {
            let elapsed = elapsed_ms(started);
            let turns = observer.turns();
            drop(observer);
            drop(slot);
            return RunResult::failed(run_id, &name, message, turns, elapsed);
        }
    };

    let config = RunConfig::new(run_id.as_str())
        .observer(Arc::clone(&observer) as Arc<dyn promptforge_core::observe::Observer>)
        .client(client)
        .cancel(cancel);
    let outcome = execute::run(
        &prompt,
        &args,
        ResolutionContext::new(tools.picker(), tools.models(), tools.tools()),
        &store,
        config,
    )
    .await;

    let turns = observer.turns();
    // Timed here rather than after the flush, so the run's duration is the run's
    // own and not the client's reading speed.
    let elapsed = elapsed_ms(started);
    // Dropping the observer closes the queue, which is what lets the pump finish
    // rather than wait for a frame that will never come.
    drop(observer);

    let mut result = match outcome {
        Ok(value) => RunResult::completed(run_id.clone(), &name, value, turns, elapsed),
        Err(error) => RunResult::failed(run_id.clone(), &name, error.to_string(), turns, elapsed),
    };

    // Extract declared output from the store and write to disk or annotate.
    if result.value().is_some() {
        extract_output(&prompt, &store, &output_file, &mut result);
    }

    log_terminal_result(&result);
    // Explicit, because returning the slot is the point of having held it.
    drop(slot);
    result
}

/// Builds the store for a run, pre-populated with the declared input file if
/// the caller provided an input source.
///
/// When the prompt declares an input file and the caller supplied either
/// `input_file` (a filesystem path to read) or `input_text` (literal content),
/// the store is created with that content under the declared store-internal
/// path. When neither is provided, an empty store is returned.
fn mock_store(prompt: &Prompt, input_file: &str, input_text: &str) -> Result<StoreRef, String> {
    let declared_input = prompt.frontmatter().input();
    let content = if !input_file.is_empty() {
        Some(
            std::fs::read_to_string(input_file)
                .map_err(|e| format!("cannot read input_file \"{input_file}\": {e}"))?,
        )
    } else if !input_text.is_empty() {
        Some(input_text.to_owned())
    } else {
        None
    };

    match (declared_input, content) {
        (Some(decl), Some(content)) => StoreRef::with_files([(decl.path().to_owned(), content)])
            .map_err(|e| format!("cannot seed store with declared input: {e}")),
        (None, Some(_)) => {
            Err("input_file or input_text was provided but the prompt declares no input".to_owned())
        }
        _ => Ok(StoreRef::memory()),
    }
}

/// Extracts the declared output from the store after a successful run.
///
/// If the prompt declares an output file, reads it from the store. If
/// `output_file` was specified, writes the content to disk. If the declared
/// output was not produced by the prompt, annotates the result but does not
/// fail it.
fn extract_output(prompt: &Prompt, store: &StoreRef, output_file: &str, result: &mut RunResult) {
    let Some(decl) = prompt.frontmatter().output() else {
        return;
    };
    let Ok(content) = store.read(decl.path()) else {
        // The prompt declared an output but did not produce it. Annotate
        // the result value with a note rather than failing the run.
        if let Some(value) = result.value().map(str::to_owned) {
            let annotated = format!(
                "{value}\n\n[note: the prompt declares output \"{}\" but it was not produced]",
                decl.path()
            );
            *result = RunResult::completed(
                result.run_id().to_owned(),
                result.prompt(),
                annotated,
                result.turns(),
                result.elapsed_ms(),
            );
        }
        return;
    };

    if output_file.is_empty() {
        // No output_file specified - the output content replaces the result
        // value so the caller receives exactly what the prompt produced.
        *result = RunResult::completed(
            result.run_id().to_owned(),
            result.prompt(),
            content,
            result.turns(),
            result.elapsed_ms(),
        );
    } else if let Err(e) = std::fs::write(output_file, &content) {
        // Write failed - annotate the result with the error but keep the
        // run as completed so the caller still gets the value.
        if let Some(value) = result.value().map(str::to_owned) {
            let annotated =
                format!("{value}\n\n[note: could not write output to \"{output_file}\": {e}]");
            *result = RunResult::completed(
                result.run_id().to_owned(),
                result.prompt(),
                annotated,
                result.turns(),
                result.elapsed_ms(),
            );
        }
    }
}

/// Converts a parse or preparation failure into a caller-facing failed run.
async fn preparation_failed(
    run_id: String,
    entry: &Entry,
    message: String,
    observer: Arc<McpObserver>,
    pump: Option<ProgressPump>,
) -> Result<CallToolResult, ErrorData> {
    let turns = observer.turns();
    drop(observer);
    finish_pump(pump).await;
    run_result(&RunResult::failed(run_id, entry.name(), message, turns, 0))
}

/// Closes a run's optional progress path after its observer has been dropped.
async fn finish_pump(pump: Option<ProgressPump>) {
    if let Some(pump) = pump {
        pump.finish().await;
    }
}

/// Logs one payload-free terminal record after every field is final.
fn log_terminal_result(result: &RunResult) {
    tracing::info!(
        run_id = %result.run_id(),
        prompt = %result.prompt(),
        status = ?result.status(),
        turns = result.turns(),
        elapsed_ms = result.elapsed_ms(),
        "run reached its terminal state"
    );
}

/// What a call refused admission is told: the wait it spent, so the calling
/// model can decide to spend it again.
fn refused(waited: Duration) -> String {
    format!(
        "every run slot is busy and none came free within {}. Retry in a moment.",
        humantime::format_duration(waited)
    )
}

/// A fresh run identifier: 128 random bits in hex, which is unguessable enough
/// for a value that only has to be unique within one process's lifetime.
fn new_run_id() -> String {
    format!("{:016x}{:016x}", fastrand::u64(..), fastrand::u64(..))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tracing::Level;

    use super::{log_terminal_result, new_run_id, refused};
    use crate::levels::recording;
    use crate::result::RunResult;

    #[test]
    fn a_run_id_is_thirty_two_hex_digits() {
        let id = new_run_id();
        assert_eq!(id.len(), 32);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(id, new_run_id(), "two runs do not share an identifier");
    }

    #[test]
    fn a_refusal_names_the_wait_it_spent() {
        let message = refused(Duration::from_secs(30));
        assert!(message.contains("30s"), "{message}");
    }

    #[test]
    fn terminal_log_carries_the_completed_result_measurements() {
        let (levels, _recording) = recording();
        let result = RunResult::completed("r1".into(), "echo", "secret".into(), 3, 42);

        log_terminal_result(&result);

        assert_eq!(levels.operator_visible(), vec![Level::INFO]);
        for field in [
            "message=run reached its terminal state",
            "run_id=r1",
            "prompt=echo",
            "status=Completed",
            "turns=3",
            "elapsed_ms=42",
        ] {
            assert!(
                levels.said(Level::INFO, field),
                "terminal log omitted {field}"
            );
        }
        assert!(
            !levels.mentioned(Level::INFO, "secret"),
            "terminal logging must exclude the run payload"
        );
    }
}
