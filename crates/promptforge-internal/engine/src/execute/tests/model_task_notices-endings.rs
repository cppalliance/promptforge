//! How a model task's end reads and where it reports: the notice text
//! names a cancel by the author, an abandonment, or a failure; a notice
//! queued after the owner left its section - at the walk's end, or
//! between two sections - reports under the section the owner last
//! entered; and a model-issued cancel queues no notice.

use std::cell::Cell;

use promptforge_types::event::Event;
use promptforge_types::ids::AbandonReason;

use super::*;
use crate::execute::RunResult;
use crate::execute::tests::context::task;
use crate::test_support::scenario::{chat_answer, infer_prompt};

/// Runs the two-section prompt with the model starting `Child` in round 1
/// and replying in round 2, then returns every notice reported.
#[track_caller]
fn notices_for(owner_tail: &str, child_body: &str) -> Vec<(String, TaskId, String)> {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[("Only", &loop_owner(owner_tail)), ("Child", child_body)],
    );
    let trace = model_task_scenario(md).chat(&gateway).run();
    assert!(
        matches!(trace.result(), RunResult::Ok(_)),
        "the owner ends clean: {:?}",
        trace.result()
    );
    NoticeRecorder::replayed(&trace).notices()
}

#[test]
fn notice_texts_name_how_a_task_ended() {
    let cancelled = notices_for(
        "local mine = tasks.pending({ origin = 'model' })\n\
         tasks.cancel(mine[1])\n\
         return 'ok'",
        CHILD_PARKED_ON_SLOW_TOOL,
    );
    assert_eq!(
        cancelled,
        vec![(
            "Only".to_owned(),
            task("0.0"),
            "Task id=0.0 (## Child) was canceled: the author cancelled it".to_owned()
        )],
        "an author cancel of a model task is one notice"
    );

    let abandoned = notices_for("return 'ok'", CHILD_PARKED_ON_SLOW_TOOL);
    assert_eq!(
        abandoned,
        vec![(
            "Only".to_owned(),
            task("0.0"),
            "Task id=0.0 (## Child) was abandoned: the section ended".to_owned()
        )],
        "an owner ending first is one abandonment notice"
    );

    let failed = notices_for("return 'ok'", "error('boom')");
    assert_eq!(failed.len(), 1, "a failed task is one notice: {failed:?}");
    assert!(
        failed[0].2.starts_with("Task id=0.0 (## Child) failed: ") && failed[0].2.contains("boom"),
        "the failure notice reports the task's error: {}",
        failed[0].2
    );
}

#[test]
fn a_walk_that_runs_off_its_last_section_reports_the_abandoned_task_under_that_section() {
    // `Last` is the walk's only section and falls through with the model's
    // task still parked, so the walk position is past the slice's end when
    // the chain's end abandons the task and queues its notice.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"### Child\"}")]),
        text("bye"),
    ]);
    let md = format!(
        "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
         # ModelTasks\n\n\
         ## Last\n\n\
         ```lua\n{}\n```\n\n\
         ### Child\n\n\
         ```lua\n{CHILD_PARKED_ON_SLOW_TOOL}\n```\n",
        loop_owner("")
    );
    let trace = model_task_scenario(md).chat(&gateway).run();
    assert!(
        matches!(trace.result(), RunResult::Ok(_)),
        "a model task the walk's end strands is abandoned, never leaked: {:?}",
        trace.result()
    );
    let recorder = NoticeRecorder::replayed(&trace);
    let events = recorder.events();
    assert!(
        events.iter().any(|(section, event)| section == "Child"
            && *event
                == Observation::TaskAbandoned {
                    task: task("0.0"),
                    reason: AbandonReason::OwnerReturned,
                }),
        "the walk's end abandons the live task: {events:?}"
    );
    assert_eq!(
        recorder.notices(),
        vec![(
            "Last".to_owned(),
            task("0.0"),
            "Task id=0.0 (## Child) was abandoned: the section ended".to_owned()
        )],
        "the notice reports under the walk's last section"
    );
}

#[test]
fn a_task_that_ends_while_its_owner_is_between_sections_reports_under_the_section_just_ended() {
    // The serial driver answers the owner's round 2 and the child's round
    // in one batch, in issue order, and the two chains then alternate
    // steps. The owner takes two to close `First` and the child two to end
    // (its round's resume runs to the inline-answered note), so the child
    // ends after `First` closes and before `Second` opens.
    let md = format!(
        "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
         # ModelTasks\n\n\
         ## First\n\n\
         ```lua\n{}\n```\n\n\
         ### Child\n\n\
         ```lua\n\
         models.infer('child work')\n\
         tasks.note('worked')\n\
         return 'child result'\n\
         ```\n\n\
         ## Second\n\n\
         ```lua\nreturn 'owner done'\n```\n",
        loop_owner("")
    );
    let rounds = Cell::new(0);
    let trace = model_task_scenario(md)
        .chat_with(move |effect| {
            if infer_prompt(effect) == "child work" {
                return chat_answer(&text("child result"));
            }
            rounds.set(rounds.get() + 1);
            match rounds.get() {
                1 => chat_answer(&calls(&[("call_1", "task", "{\"target\":\"### Child\"}")])),
                _ => chat_answer(&text("bye")),
            }
        })
        .run();
    assert_eq!(trace.text(), "owner done");
    let events = trace.run_events();
    let child = task("0.0");
    let first_finished = events
        .iter()
        .position(
            |event| matches!(event, Event::SectionFinished { section, .. } if section == "First"),
        )
        .expect("`First` completes");
    let second_started = events
        .iter()
        .position(
            |event| matches!(event, Event::SectionStarted { section, .. } if section == "Second"),
        )
        .expect("`Second` starts");
    let (notice_at, notice_section) = events
        .iter()
        .enumerate()
        .find_map(|(at, event)| match event {
            Event::TaskNotice { section, task, .. } if *task == child => {
                Some((at, section.clone()))
            }
            _ => None,
        })
        .expect("the child's end queues a notice");
    assert!(
        first_finished < notice_at && notice_at < second_started,
        "the child ends while its owner is between sections: {events:?}"
    );
    assert_eq!(
        notice_section, "First",
        "the notice reports under the section just ended"
    );
}

#[test]
fn a_model_issued_cancel_queues_no_notice() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "task_cancel", "{\"id\":\"0.0\"}")]),
        text("bye"),
    ]);
    let md = prompt(
        "",
        &[
            ("Only", &loop_owner("return 'ok'")),
            ("Child", CHILD_PARKED_ON_SLOW_TOOL),
        ],
    );
    let trace = model_task_scenario(md).chat(&gateway).run();
    assert!(
        matches!(trace.result(), RunResult::Ok(_)),
        "the cancel leaves nothing live: {:?}",
        trace.result()
    );
    let recorder = NoticeRecorder::replayed(&trace);
    assert!(
        recorder.notices().is_empty(),
        "the model already read `Task id=0.0 cancelled`; no notice repeats it: {:?}",
        recorder.notices()
    );
}
