//! Run-level termination and the tasks it strands: when the host cancels a
//! run while an author-spawned task is parked on a model round, the run's
//! end settles that task - one `TaskAbandoned` with `run_terminated`,
//! observed before the run's own `RUN_FAILED` boundary - so the
//! exactly-one-terminal contract holds on the whole-run exit path as it
//! does on every per-chain ending. Driven on the serial driver, so the
//! cancel lands at a chosen suspension point with no runtime involved.

use std::collections::BTreeMap;

use promptforge_api_types::ids::{AbandonReason, TaskId};

use super::model_task_acceptance::{task_events, terminals_per_started_task};
use super::scheduler::scheduler_context_on;
use super::serial_driver::{perform_locally, text_reply};
use super::tasks::TaskRecorder;
use super::*;
use crate::execute::run::{Effect, Run, Step};
use crate::test_support::recording::forward;

/// The spawner starts `Child` and waits on it; the child parks on a model
/// round the test never answers, so the run is cancelled with the task
/// live.
const PARKED_CHILD: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Termination\n\n\
    ## Spawner\n\n\
    ```lua\n\
    local t = tasks.spawn('## Child')\n\
    tasks.when_any({ t })\n\
    return 'unreachable'\n\
    ```\n\n\
    ## Child\n\n\
    ```lua\n\
    return models.infer('park')\n\
    ```\n";

/// Steps `run` until its first chat round is outstanding, cancels it
/// there, answers the orphaned effects so the run can report `Done`, and
/// returns the result. Every step's events are replayed onto `recorder`
/// in order, as a host driver replays them onto its observer.
fn cancel_at_first_chat_round(mut run: Run, recorder: &TaskRecorder) -> RunResult {
    let mut cancelled = false;
    loop {
        match run.step() {
            Step::Done { result, events } => {
                forward(events, recorder, None);
                return result;
            }
            Step::Pending { effects, events } => {
                forward(events, recorder, None);
                assert!(
                    !effects.is_empty() || cancelled,
                    "a live run issues an effect on every pending step"
                );
                if !cancelled
                    && effects
                        .iter()
                        .any(|(_, _, effect)| matches!(effect, Effect::Chat { .. }))
                {
                    run.cancel();
                    cancelled = true;
                }
                // After the cancel every outstanding effect is an orphan
                // whose answer the run discards; before it, none of the
                // effects here is a chat round.
                for (id, _, effect) in effects {
                    run.resume(
                        id,
                        perform_locally(&effect, &mut |_| text_reply("too late")),
                    );
                }
            }
        }
    }
}

#[test]
fn cancelling_a_run_settles_every_live_task_with_one_terminal_before_the_run_ends() {
    let prompt = parse(PARKED_CHILD);
    let ctx = scheduler_context_on(
        &prompt,
        &TestStore::new(),
        Arc::new(NullObserver::default()),
    );
    let recorder = TaskRecorder::default();

    let result = cancel_at_first_chat_round(Run::from_state(ctx), &recorder);

    assert!(
        matches!(result, RunResult::Cancelled),
        "the host's cancel ends the run as cancelled: {result:?}"
    );
    let records = recorder.records();
    let child: TaskId = "0.0".parse().expect("a task id parses");
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([(child.clone(), vec!["abandoned"])]),
        "the stranded task has exactly one terminal, and it is abandoned: {:?}",
        task_events(&records)
    );
    assert!(
        records.iter().any(|(_, observation)| matches!(
            observation,
            Observation::TaskAbandoned { task, reason: AbandonReason::RunTerminated } if *task == child
        )),
        "the terminal names the run's end as the reason: {records:?}"
    );
    let run_end = records
        .iter()
        .position(|(_, observation)| *observation == Observation::RunFailed)
        .expect("a cancelled run reports RUN_FAILED");
    let last_terminal = records
        .iter()
        .rposition(|(_, observation)| {
            matches!(
                observation,
                Observation::TaskSucceeded { .. }
                    | Observation::TaskFailed { .. }
                    | Observation::TaskCancelled { .. }
                    | Observation::TaskAbandoned { .. }
            )
        })
        .expect("the stranded task reports a terminal");
    assert!(
        last_terminal < run_end,
        "every task terminal precedes the run's end boundary: {records:?}"
    );
}
