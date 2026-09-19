//! Exit rules: the section walk's (fall-through, explicit return, the
//! generic result, H1-only prompts, the version gate, cross-section store
//! persistence) and the `models.loop` shim's (the terminal reply, the
//! model's clean empty exit after tool work, and the empty rounds that
//! raise `empty_model_reply`).

use super::models_loop::{
    echo_tools, loop_context, loop_context_observed, loop_events, loop_prompt,
};
use super::run;
use super::*;
use crate::execute::tokio_driver::TokioDriver;
use crate::lua::ToolSet;

#[tokio::test]
async fn falls_through_to_next_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn \"second\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "second");
}

#[tokio::test]
async fn explicit_return_stops_fall_through() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## First\n\n```lua\nreturn \"first\"\n```\n\n\
## Second\n\n```lua\nreturn \"unreached\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "first");
}

#[tokio::test]
async fn generic_result_when_nothing_produced() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nlocal x = 1\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "done");
}

#[tokio::test]
async fn sys_id_increments_per_section() {
    // First section files nothing and falls through; second returns its id:
    // entry 2 of the root chain (entry 0 is the H1 pass, present or not).
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## First\n\n```lua\nlocal x = 1\n```\n\n\
## Second\n\n```lua\nreturn tostring(sys.id)\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "0.2");
}

// --- H1-only prompts (no ## sections) ---

#[tokio::test]
async fn h1_only_lua_return() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Title\n\n```lua\nreturn \"hello\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "hello");
}

#[tokio::test]
async fn h1_only_lua_no_return() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Title\n\n```lua\nlocal x = 1\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "done");
}

// --- Version gate at the top of `run` ---

#[tokio::test]
async fn supported_major_zero_proceeds() {
    // A `promptforge: 0` prompt clears the gate and runs to completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "ran");
}

#[tokio::test]
async fn unsupported_major_one_is_refused() {
    // Major 1 is no longer implemented: the gate refuses it and names the
    // declared version rather than silently degrading to major 0.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("major 1 must be refused after the 0-only gate flip");
    assert!(matches!(err, Error::UnsupportedVersion(1)));
}

#[tokio::test]
async fn unsupported_major_is_refused() {
    // A future major is refused, never silently degraded to major 0.
    let md = "---\nname: t\ndescription: d\npromptforge: 2\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("an unsupported major must be refused");
    assert!(matches!(err, Error::UnsupportedVersion(2)));
}

#[tokio::test]
async fn missing_version_is_not_a_promptforge_prompt() {
    // No `promptforge:` key: not our prompt, so `run` declines with a Parse
    // error rather than executing it.
    let md = "---\nname: t\ndescription: d\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("a prompt with no promptforge version must be declined");
    match err {
        Error::ParseStructured {
            kind,
            message,
            name,
            ..
        } => {
            assert_eq!(kind, ParseErrorKind::Structure);
            assert_eq!(
                name.as_deref(),
                Some("t"),
                "the parsed prompt's frontmatter name rides the error"
            );
            assert!(
                message.contains("not a promptforge prompt"),
                "the Parse message must name the missing version, got: {message}"
            );
        }
        other => panic!("expected Parse, got {other:?}"),
    }
}

// --- Cross-section store persistence ---

#[tokio::test]
async fn store_persists_across_sections() {
    // One store is created for the run and threaded to every section. The
    // first section's Lua writes a file; the second, in a fresh context,
    // reads it back - proving the store outlives the context-clearing
    // transition. The read lands in `var`, so it round-trips the value.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Writer\n\n```lua\nstore.write('note.txt', 'carried across')\n```\n\n\
## Reader\n\n```lua\nvar.seen = store.read('note.txt')\nreturn var.seen\n```\n";
    let store = TestStore::new();
    let out = run(&fixture(md), "", &[], &store, silent()).await.unwrap();
    assert_eq!(
        out, "carried across",
        "the second section must read what the first wrote"
    );
    // The very same handle still holds the file after the run, confirming
    // both sections shared one store rather than each getting a fresh one.
    assert_eq!(
        store.read("note.txt").expect("read"),
        "carried across",
        "the run's store must retain the written file"
    );
}

// --- The `models.loop` shim's exit rules ---

/// The section body every loop exit-rule test runs: one loop over a single
/// user message, then the list's length and the terminal record's text.
const LOOP_TO_TEXT: &str = "local msgs = messages.new()\n\
     msgs:user('ask the model')\n\
     models.loop(msgs)\n\
     return #msgs .. '|' .. msgs[#msgs].content";

/// Drives `LOOP_TO_TEXT` against `replies` with `tools` in scope, returning
/// the section's result, the loop's observation sequence, and the run's
/// turn count.
async fn drive_loop(
    replies: Vec<GatewayReply>,
    tools: ToolSet,
) -> (Result<String>, Vec<String>, u32) {
    let gateway = ScriptedGateway::start(replies).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let recorder = Arc::new(Recorder::default());
    let ctx = loop_context_observed(&prompt, tools, Arc::clone(&recorder) as Arc<dyn Observer>);
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await;
    (
        out,
        loop_events(&recorder),
        ctx.turns().load(Ordering::Relaxed),
    )
}

#[tokio::test(flavor = "current_thread")]
async fn a_text_reply_is_the_loops_terminal_record() {
    let (out, events, turns) = drive_loop(
        vec![resp_text_finish("all done", "stop")],
        ToolSet::default(),
    )
    .await;
    assert_eq!(out.expect("a text reply ends the loop"), "2|all done");
    assert_eq!(turns, 1);
    assert_eq!(events, vec![detail::MODEL_TURN_COMPLETED.to_string()]);
}

#[tokio::test(flavor = "current_thread")]
async fn length_finish_reason_reports_model_turn_truncated() {
    let (out, events, turns) = drive_loop(
        vec![resp_text_finish("partial answer", "length")],
        ToolSet::default(),
    )
    .await;
    assert_eq!(
        out.expect("a truncated reply still ends the loop"),
        "2|partial answer"
    );
    assert_eq!(turns, 1);
    assert_eq!(
        events,
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::MODEL_TURN_TRUNCATED.to_string(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn empty_stop_turn_after_tool_call_is_a_clean_exit() {
    // The model stopped deliberately after doing its work through a tool
    // call: the loop accepts the empty turn and appends an empty assistant
    // record as the terminal text.
    let (out, events, turns) = drive_loop(
        vec![
            resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
            resp_text_finish("", "stop"),
        ],
        echo_tools(),
    )
    .await;
    assert_eq!(
        out.expect("the run must succeed"),
        "4|",
        "a clean stop-exit appends an empty terminal record after the exchange"
    );
    assert_eq!(turns, 2, "the tool-call turn and the accepted empty turn");
    assert_eq!(
        events,
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_SUCCEEDED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn empty_stop_turn_without_tool_calls_fails() {
    // Zero prior dispatches: the acceptance conditions cannot hold, so the
    // empty "stop" turn is an `EmptyModelReply` failure. The round itself
    // completed - the scheduler counts and reports it - and the shim's exit
    // rule raises against its finish reason.
    for tools in [ToolSet::default(), echo_tools()] {
        let (out, events, turns) = drive_loop(vec![resp_text_finish("", "stop")], tools).await;
        match out {
            Err(Error::EmptyModelReply {
                finish_reason,
                detail: phrase,
            }) => {
                assert_eq!(finish_reason.as_deref(), Some("stop"));
                assert_eq!(
                    phrase, "empty model reply",
                    "the client's phrase is the message"
                );
            }
            other => panic!("expected EmptyModelReply, got {other:?}"),
        }
        assert_eq!(turns, 1, "the empty round is a completed turn");
        assert_eq!(events, vec![detail::MODEL_TURN_COMPLETED.to_string()]);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn empty_truncated_final_text_fails_without_truncation_detail() {
    // `finish_reason: "length"` is never a clean exit, even with empty text,
    // and an empty round reports no truncation: there is no reply to have
    // truncated.
    let (out, events, _) =
        drive_loop(vec![resp_text_finish("", "length")], ToolSet::default()).await;
    match out {
        Err(Error::EmptyModelReply { finish_reason, .. }) => {
            assert_eq!(finish_reason.as_deref(), Some("length"));
        }
        other => panic!("expected EmptyModelReply, got {other:?}"),
    }
    assert_eq!(events, vec![detail::MODEL_TURN_COMPLETED.to_string()]);
}

#[tokio::test(flavor = "current_thread")]
async fn empty_turn_without_finish_reason_after_tool_call_fails() {
    // Fail closed: a missing finish reason is not "stop", so the empty turn
    // is an error even after a successful dispatch.
    let (out, events, turns) = drive_loop(
        vec![
            resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
            resp_text(""),
        ],
        echo_tools(),
    )
    .await;
    match out {
        Err(Error::EmptyModelReply { finish_reason, .. }) => {
            assert_eq!(finish_reason, None);
        }
        other => panic!("expected EmptyModelReply, got {other:?}"),
    }
    assert_eq!(turns, 2, "the tool-call turn and the completed empty round");
    assert_eq!(
        events,
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_SUCCEEDED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_reply_is_readable_at_the_call_site_and_appends_nothing() {
    // The raise is pcall-able as the `empty_model_reply` kind carrying the
    // finish reason as its field and the client's phrase as its message,
    // and the rejected round leaves the author's list untouched.
    let gateway = ScriptedGateway::start(vec![resp_text_finish("", "stop")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('say nothing')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'the empty round raises')\n\
         assert(#msgs == 1, 'a rejected round appends nothing')\n\
         return err.kind .. '|' .. tostring(err.finish_reason) .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert_eq!(out, "empty_model_reply|stop|empty model reply");
}
