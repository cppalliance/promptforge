//! The `models.loop` contract case no real round produces: an empty answer
//! that names no empty detail, which the model client refuses before it
//! can reach the loop. The loop's yields are driven by hand, each answer
//! rendered through `Answer::into_envelope`. The loop's other contract
//! cases run end to end in `execute/tests/models_loop_contract.rs`.

use promptforge_lua::Error;

use crate::execute::protocol::{Answer, ChatResult, Request};

use super::{parse_request, resume_with, scheduler_vm, start, test_models};

/// A completed round with no reply, no tool calls, and no empty detail,
/// ending for `finish_reason`.
fn empty_round(finish_reason: Option<&str>) -> Answer<Error> {
    Answer::Chat(Ok(Box::new(ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: None,
        finish_reason: finish_reason.map(str::to_owned),
        model: "test-model".to_owned(),
        metrics: None,
        turn: 1,
    })))
}

#[test]
fn an_empty_answer_with_no_empty_detail_raises_the_fallback_message() {
    for (finish_reason, expected) in [
        (
            Some("length"),
            "empty_model_reply|empty model reply|length|1",
        ),
        (None, "empty_model_reply|empty model reply|nil|1"),
    ] {
        let vm = scheduler_vm(&test_models(), None);
        let (thread, yielded) = start(
            &vm,
            "local msgs = messages.new()\n\
             msgs:user('hi')\n\
             local ok, err = pcall(models.loop, msgs)\n\
             assert(not ok, 'an empty round raises')\n\
             return err.kind .. '|' .. tostring(err) .. '|' .. tostring(err.finish_reason) \
             .. '|' .. #msgs",
        );
        assert!(
            matches!(parse_request(&vm, yielded), Request::Chat { .. }),
            "the round opens with the chat"
        );
        let returned = resume_with(&vm, &thread, empty_round(finish_reason));
        let text: String = vm
            .lua()
            .unpack(
                returned
                    .into_iter()
                    .next()
                    .expect("the block returns a value"),
            )
            .expect("the block returns a string");
        assert_eq!(text, expected, "finish reason {finish_reason:?}");
    }
}
