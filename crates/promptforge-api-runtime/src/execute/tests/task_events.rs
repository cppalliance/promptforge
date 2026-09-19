//! The task history read: `tasks.events` answers an owner (or the task
//! itself) with the task's events after `last`, refuses a task the caller
//! neither owns nor runs inside, and is answered by the tokio driver from
//! its own history; the model's `task_events` built-in answers with the
//! events nonce-wrapped as untrusted, `no new events` when nothing is new,
//! and a refusal for an unknown task or a malformed `last`.

use promptforge_api_types::event::Event;

use super::model_tasks::{NeverBroker, model_task_context_with};
use super::serial_driver::{perform_locally, text_reply, tool_call_reply};
use super::*;
use crate::execute::run::Run;
use crate::test_support::drive;

/// A prompt whose `Only` section spawns `Child`, waits for it, then reads
/// its history twice - whole, and after the first event - and reports the
/// last kind, both counts, and whether every event names the child.
const OWNER_READS_CHILD: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n\
    ## Only\n\n\
    ```lua\n\
    local t = tasks.spawn('## Child')\n\
    tasks.when_any({ t })\n\
    local all = tasks.events(t)\n\
    local same = true\n\
    for _, e in ipairs(all) do same = same and e.provenance.task == t.task end\n\
    local later = tasks.events(t, { last = all[1].provenance.seq })\n\
    return all[#all].kind .. '|' .. #all .. '|' .. #later .. '|' .. tostring(same)\n\
    ```\n\n\
    ## Child\n\n\
    ```lua\n\
    return 'done'\n\
    ```\n";

/// Drives `md` capability-free on the serial driver.
fn drive_plain(md: &str) -> (RunResult, Vec<Event>) {
    let run = Run::new(Arc::new(parse(md)), "", test_context(EXECUTION));
    drive(run, |_, effect| {
        perform_locally(effect, &mut |_| panic!("no model round is issued"))
    })
}

fn text_of(result: RunResult) -> String {
    match result {
        RunResult::Ok(text) => text,
        other => panic!("the run succeeds: {other:?}"),
    }
}

#[test]
fn an_owner_reads_its_tasks_history_and_last_narrows_it_to_later_events() {
    let (result, events) = drive_plain(OWNER_READS_CHILD);
    let text = text_of(result);
    let parts: Vec<&str> = text.split('|').collect();
    assert_eq!(
        parts[0], "task_succeeded",
        "the terminal is the last event of the task's own record: {text}"
    );
    let all: usize = parts[1].parse().expect("a count");
    let later: usize = parts[2].parse().expect("a count");
    assert!(
        all >= 2,
        "the child reports its chunk and its terminal: {text}"
    );
    assert_eq!(
        later,
        all - 1,
        "`last` drops exactly the events already seen"
    );
    assert_eq!(parts[3], "true", "every event carries the child's task");
    assert!(
        events
            .iter()
            .filter(|event| matches!(event, Event::TaskSucceeded { .. }))
            .count()
            == 1,
        "the read itself reports nothing"
    );
}

#[test]
fn a_task_may_read_itself_and_a_task_it_does_not_own_is_refused() {
    // The child reads its own record through `sys.taskid` and is refused
    // the parent's task, which it neither owns nor runs inside; the main
    // walk reads itself as task 0.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        local mine = #tasks.events(sys.taskid) > 0\n\
        local t = tasks.spawn('## Child')\n\
        local _, ok, result = tasks.when_any({ t })\n\
        assert(ok, tostring(result))\n\
        return tostring(mine) .. '|' .. result\n\
        ```\n\n\
        ## Child\n\n\
        ```lua\n\
        local own = #tasks.events(sys.taskid) > 0\n\
        local ok, err = pcall(tasks.events, '0')\n\
        return tostring(own) .. '/' .. tostring(ok) .. '/' .. err.kind .. '/' .. err.task\n\
        ```\n";
    let (result, _) = drive_plain(md);
    assert_eq!(text_of(result), "true|true/false/task_not_owned/0");
}

#[test]
fn a_malformed_opts_argument_is_the_calls_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        local ok, err = pcall(tasks.events, sys.taskid, 'soon')\n\
        local ok2, err2 = pcall(tasks.events, sys.taskid, { last = 'x' })\n\
        return tostring(ok) .. '|' .. tostring(err) .. '|' .. tostring(ok2) .. '|' .. tostring(err2)\n\
        ```\n";
    let (result, _) = drive_plain(md);
    assert_eq!(
        text_of(result),
        "false|tasks.events opts must be a table, got string|false|tasks.events last must be a number, got string"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_tokio_driver_answers_a_history_read_from_its_own_events() {
    let prompt = parse(OWNER_READS_CHILD);
    let RunResult::Ok(text) = crate::execute::run(&prompt, "", test_context(EXECUTION)).await
    else {
        panic!("the run succeeds through the tokio driver");
    };
    assert!(
        text.starts_with("task_succeeded|"),
        "the driver's history answers the read: {text}"
    );
    assert!(
        text.ends_with("|true"),
        "every event carries the child's task: {text}"
    );
}

/// The owner section every built-in test runs: the model loop under
/// `tools.allow_tasks`, returning `tail`.
fn owner_prompt(tail: &str) -> String {
    format!(
        "---\nname: mt\ndescription: d\npromptforge: 0\n---\n\n\
         # ModelTasks\n\n\
         ## Only\n\n\
         ```lua\n\
         tools.allow_tasks({{ '## Child' }})\n\
         local msgs = messages.new()\n\
         msgs:user('go')\n\
         models.loop(msgs)\n\
         {tail}\n\
         ```\n\n\
         ## Child\n\n\
         ```lua\n\
         return 'child result'\n\
         ```\n"
    )
}

/// Drives `md` with the model played by `rounds`, one canned answer per
/// `chat` round in order.
fn drive_scripted(md: &str, rounds: Vec<EffectAnswer>) -> (RunResult, Vec<Event>) {
    let prompt = parse(md);
    let state = model_task_context_with(
        &prompt,
        Arc::new(NullObserver::default()),
        Arc::new(NeverBroker),
    );
    let mut rounds = rounds.into_iter();
    drive(Run::from_state(state), |_, effect| {
        perform_locally(effect, &mut |_| {
            rounds.next().expect("the script covers every round")
        })
    })
}

#[test]
fn the_task_events_builtin_answers_the_model_with_the_history_nonce_wrapped() {
    // Round 1 starts the child, which runs to its end before round 2's
    // answer arrives; round 2 reads its history. The tool record the
    // model reads is the child's events, one JSON line each, inside the
    // untrusted wrap, and the `ToolResult` says untrusted.
    let (result, events) = drive_scripted(
        &owner_prompt("return msgs[5].content"),
        vec![
            tool_call_reply("call_1", "task", json!({ "target": "## Child" })),
            tool_call_reply("call_2", "task_events", json!({ "id": "0.0" })),
            text_reply("bye"),
        ],
    );
    let text = text_of(result);
    assert!(
        text.contains("<untrusted_input_") && text.contains("</untrusted_input_"),
        "the history is nonce-wrapped: {text}"
    );
    assert!(
        text.contains("\"kind\":\"task_succeeded\"") && text.contains("\"task\":\"0.0\""),
        "the history carries the child's terminal as JSON: {text}"
    );
    let trusted = events
        .iter()
        .find_map(|event| match event {
            Event::ToolResult {
                alias,
                tool_call_id,
                trusted,
                ..
            } if alias == "task_events" && tool_call_id == "call_2" => Some(*trusted),
            _ => None,
        })
        .expect("the built-in reports its ToolResult under the model's call id");
    assert!(!trusted, "a history read's answer is untrusted");
}

#[test]
fn the_task_events_builtin_reports_nothing_new_after_last_and_refuses_bad_arguments() {
    // Round 2 reads past the child's last event and gets the trusted
    // nothing-new sentence; rounds 3 and 4 are refused - an id the model
    // never started, and a negative `last`.
    // The child's completion notice lands as a user record between the
    // rounds, so the tool records are gathered by role rather than by
    // position.
    let (result, events) = drive_scripted(
        &owner_prompt(
            "local answers = {}\n\
             for _, m in ipairs(msgs) do\n\
               if m.role == 'tool' then answers[#answers + 1] = m.content end\n\
             end\n\
             return table.concat(answers, '|')",
        ),
        vec![
            tool_call_reply("call_1", "task", json!({ "target": "## Child" })),
            tool_call_reply(
                "call_2",
                "task_events",
                json!({ "id": "0.0", "last": 1000 }),
            ),
            tool_call_reply("call_3", "task_events", json!({ "id": "0.7" })),
            tool_call_reply("call_4", "task_events", json!({ "id": "0.0", "last": -1 })),
            text_reply("bye"),
        ],
    );
    assert_eq!(
        text_of(result),
        "Task id=0.0 started|no new events|task_events: no model task with id 0.7|\
         task_events: `last` must be a non-negative integer sequence number when given"
    );
    let trusted: Vec<bool> = events
        .iter()
        .filter_map(|event| match event {
            Event::ToolResult { alias, trusted, .. } if alias == "task_events" => Some(*trusted),
            _ => None,
        })
        .collect();
    assert_eq!(
        trusted,
        [true, true, true],
        "the engine's own sentences resume trusted"
    );
    let failed = events
        .iter()
        .filter(|event| matches!(event, Event::ToolCallFailed { .. }))
        .count();
    assert_eq!(failed, 2, "the two refusals are observed as failed calls");
}
