//! Running one catalog entry, and what happens when the run outlives its call.
//!
//! A call that reaches a prompt goes through four gates in order. A broken entry
//! or an unbindable tool fails before anything is reserved, because neither ever
//! reaches a model and neither should cost a run slot. Then admission: one of
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

use promptforge_core::client::GatewayClient;
use promptforge_core::execute::{self, RunOptions};
use promptforge_core::parser::Prompt;
use promptforge_core::store::Store;
use promptforge_core::tools::Tool;
use rmcp::model::{CallToolResult, ErrorData};
use tokio::time::Instant;

use crate::catalog::Entry;
use crate::config::Config;
use crate::progress::McpObserver;
use crate::registry::{RunRegistry, RunSlot, elapsed_ms};
use crate::result::{NO_TURNS, RunResult};

use super::{Reporting, bind, run_result, text_error};

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
    /// The prompt's frontmatter contract version.
    version: u32,
    /// The run's whole input.
    args: String,
    /// The tools the prompt asked for.
    tools: Vec<Box<dyn Tool>>,
    /// Where the run reports itself, and what counts its turns.
    observer: McpObserver,
    /// The gateway the run's model calls go through.
    client: GatewayClient,
    /// When the run started, for `elapsed_ms`.
    started: Instant,
    /// The admission slot, returned when this value drops.
    slot: RunSlot,
}

/// Runs one entry and answers the call that asked for it.
///
/// # Errors
/// Returns `-32603` when the result cannot be serialized. Everything the
/// calling model can act on - a broken prompt, an unbindable tool, a refused
/// admission, a run that failed - is an `Ok` result carrying `isError`.
pub(super) async fn run(
    config: &Config,
    registry: &Arc<RunRegistry>,
    entry: &Entry,
    args: &str,
    reporting: Option<Reporting>,
) -> Result<CallToolResult, ErrorData> {
    let run_id = new_run_id();

    let Some(prompt) = entry.prompt() else {
        let problem = entry
            .problem()
            .unwrap_or("the prompt is unavailable")
            .to_owned();
        return run_result(&RunResult::failed(
            run_id,
            entry.name(),
            entry.version(),
            problem,
            NO_TURNS,
            0,
        ));
    };

    let tools = match bind::select_tools(&prompt.frontmatter.tools, &config.gateway) {
        Ok(tools) => tools,
        Err(message) => {
            return run_result(&RunResult::failed(
                run_id,
                entry.name(),
                entry.version(),
                message,
                NO_TURNS,
                0,
            ));
        }
    };

    let Some(slot) = registry.admit().await else {
        return Ok(text_error(refused(registry.admission_timeout())));
    };

    // The clock starts here, past admission, so a run's reported duration is the
    // run's own and carries none of the queue wait ahead of it.
    let started = Instant::now();

    let (observer, pump) = match reporting {
        Some((peer, token)) => {
            let (observer, pump) = McpObserver::reporting(peer, token);
            (observer, Some(pump))
        }
        None => (McpObserver::silent(), None),
    };

    registry.started(&run_id, entry.name(), entry.version());
    let task = tokio::spawn(execute_run(
        Arc::clone(registry),
        Launch {
            run_id: run_id.clone(),
            prompt: prompt.clone(),
            name: entry.name().to_owned(),
            version: entry.version(),
            args: args.to_owned(),
            tools,
            observer,
            client: bind::gateway_client(&config.gateway),
            started,
            slot,
        },
    ));

    let result = registry
        .settle(&run_id, entry.name(), entry.version(), task)
        .await;
    if let Some(pump) = pump {
        pump.finish().await;
    }
    run_result(&result)
}

/// Runs one prompt to a [`RunResult`] and records it.
///
/// This is the body of the spawned task, so nothing here borrows from the call
/// that started it.
async fn execute_run(registry: Arc<RunRegistry>, launch: Launch) -> RunResult {
    let Launch {
        run_id,
        prompt,
        name,
        version,
        args,
        tools,
        observer,
        client,
        started,
        slot,
    } = launch;

    let borrowed: Vec<&dyn Tool> = tools.iter().map(AsRef::as_ref).collect();
    let store = Store::memory();
    let options = RunOptions {
        observer: &observer,
        client: Some(client),
    };
    let outcome = execute::run(&prompt, &args, &borrowed, &store, options).await;

    let turns = observer.turns();
    // Timed here rather than after the flush, so the run's duration is the run's
    // own and not the client's reading speed.
    let elapsed = elapsed_ms(started);
    // Dropping the observer closes the queue, which is what lets the pump finish
    // rather than wait for a frame that will never come.
    drop(observer);

    let result = match outcome {
        Ok(value) => RunResult::completed(run_id.clone(), &name, version, value, turns, elapsed),
        Err(error) => RunResult::failed(
            run_id.clone(),
            &name,
            version,
            error.to_string(),
            turns,
            elapsed,
        ),
    };
    log_terminal_result(&result);
    registry.finished(&run_id, result.clone());
    // Explicit, because returning the slot is the point of having held it.
    drop(slot);
    result
}

/// Logs one payload-free terminal record after every field is final.
fn log_terminal_result(result: &RunResult) {
    tracing::info!(
        run_id = %result.run_id,
        prompt = %result.prompt,
        status = ?result.status,
        turns = result.turns,
        elapsed_ms = result.elapsed_ms,
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
    use tracing_subscriber::layer::SubscriberExt;

    use super::{log_terminal_result, new_run_id, refused};
    use crate::levels::Levels;
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
        let levels = Levels::default();
        let subscriber = tracing_subscriber::registry().with(levels.clone());
        let result = RunResult::completed("r1".into(), "echo", 1, "secret".into(), 3, 42);

        tracing::subscriber::with_default(subscriber, || log_terminal_result(&result));

        assert_eq!(levels.operator_visible(), vec![Level::INFO]);
        for field in [
            "run reached its terminal state",
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
            !levels.said(Level::INFO, "secret"),
            "terminal logging must exclude the run payload"
        );
    }
}
