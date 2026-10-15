//! Run-level termination and the tasks it strands: when the caller cancels a
//! run while an author-spawned task is parked on a model round, the run's
//! end settles that task - one `TaskAbandoned` with `run_terminated`,
//! observed before the run's own `RUN_FAILED` boundary - so the
//! exactly-one-terminal contract holds on the whole-run exit path as it
//! does on every per-chain ending. The scenario holds the child's round,
//! so the cancel lands at a chosen suspension point with no runtime
//! involved.

use std::collections::BTreeMap;

use promptforge_types::ids::{AbandonReason, TaskId};

use super::model_task_acceptance::{task_lifecycle, terminals_per_started_task};
use super::scheduler::scheduler_scenario;
use super::tasks::TaskRecorder;
use super::*;

/// The spawner starts `Child` and waits on it; the child parks on a model
/// round the test never answers, so the run is cancelled with the task
/// live.
const PARKED_CHILD: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Termination\n\n\
    ## Spawner\n\n\
    ```lua\n\
    local t = tasks.spawn('## Child')\n\
    tasks.join_any({ t })\n\
    return 'unreachable'\n\
    ```\n\n\
    ## Child\n\n\
    ```lua\n\
    return models.infer('park')\n\
    ```\n";

#[test]
fn cancelling_a_run_settles_every_live_task_with_one_terminal_before_the_run_ends() {
    // The child's round is held, and the run is cancelled once only that
    // round is outstanding.
    let mut driving = scheduler_scenario(PARKED_CHILD)
        .hold(|effect| matches!(effect, Effect::Chat { .. }))
        .start();
    while driving.step() {}
    driving.cancel();
    let trace = driving.finish();

    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "the test driver's cancel ends the run as cancelled: {:?}",
        trace.result()
    );
    assert!(
        matches!(
            trace.effects(),
            [(EffectRecord::Chat { .. }, AnswerRecord::Dropped)]
        ),
        "the cancel lands while the child's round is outstanding: {:?}",
        trace.effects()
    );
    let recorder = TaskRecorder::default();
    trace.replay(&recorder);
    let records = recorder.records();
    let child: TaskId = "0.0".parse().expect("a task id parses");
    assert_eq!(
        terminals_per_started_task(&records),
        BTreeMap::from([(child.clone(), vec!["abandoned"])]),
        "the stranded task has exactly one terminal, and it is abandoned: {:?}",
        task_lifecycle(&records)
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
