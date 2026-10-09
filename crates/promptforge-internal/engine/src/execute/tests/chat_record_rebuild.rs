//! A run log's Chat records rebuild every request a `messages.new()` list
//! sent: walking the records in log order, the first `keep` messages of
//! the request rebuilt for `after`, then the record's own messages, equal
//! the wire messages on that round's live effect. Each case edits the list
//! between sends in a way that changes what the log must hold.

use std::collections::BTreeMap;

use promptforge_types::ids::RoundId;

use super::models_loop::{echo_tools, loop_context, loop_prompt};
use super::serial_driver::{text_of, text_reply, tool_call_reply};
use super::*;
use crate::execute::run::{Effect, EffectAnswer, EffectRecord, Run};
use crate::model::{CompletionError, CompletionErrorKind};
use crate::test_support::drive;

/// One issued round as the driver saw it: its record, and the wire
/// messages the live effect sent.
struct Sent {
    record: EffectRecord,
    live: Vec<Value>,
}

impl Sent {
    /// The record's `after`, `keep`, and logged messages.
    fn shape(&self) -> (Option<RoundId>, u64, &[Value]) {
        let EffectRecord::Chat {
            after,
            keep,
            messages,
            ..
        } = &self.record
        else {
            panic!("a round records a chat record: {:?}", self.record);
        };
        (*after, *keep, messages)
    }
}

/// Drives `lua` as a loop section with `echo` bound, answering the model
/// rounds from `replies` in order and every tool call with trusted text,
/// and returns the section's result beside each round as it was sent, in
/// issue order.
fn drive_rounds(lua: &str, replies: Vec<EffectAnswer>) -> (String, Vec<Sent>) {
    let prompt = parse(&loop_prompt(lua));
    let (state, _fixture) = loop_context(&prompt, echo_tools());
    let mut replies = replies.into_iter();
    let mut sent = Vec::new();
    let (result, _events) = drive(Run::from_state(state), |_, effect| match effect {
        Effect::Chat { messages, .. } => {
            sent.push(Sent {
                record: effect.record(),
                live: messages
                    .iter()
                    .map(|message| serde_json::to_value(message).expect("a message serializes"))
                    .collect(),
            });
            replies.next().expect("the script covers every round")
        }
        Effect::ToolCall { .. } => EffectAnswer::ToolCall(Ok(ToolOutput::trusted("echoed"))),
        other => panic!("only model rounds and tool calls are issued: {other:?}"),
    });
    (text_of(result), sent)
}

/// Rebuilds every request from the records alone and asserts each equals
/// the wire messages its live effect sent. Every `after` must name a round
/// recorded earlier in the log.
fn assert_rebuilds(sent: &[Sent]) {
    let mut requests: BTreeMap<RoundId, Vec<Value>> = BTreeMap::new();
    for one in sent {
        let EffectRecord::Chat { round, .. } = &one.record else {
            panic!("a round records a chat record: {:?}", one.record);
        };
        let (after, keep, messages) = one.shape();
        let base: &[Value] = match after {
            Some(after) => requests.get(&after).unwrap_or_else(|| {
                panic!("round {round:?} extends round {after:?}, which the log has not recorded")
            }),
            None => &[],
        };
        let kept = usize::try_from(keep).expect("keep fits in usize");
        assert!(
            kept <= base.len(),
            "round {round:?} keeps {kept} of round {after:?}'s {} messages",
            base.len()
        );
        let rebuilt: Vec<Value> = base[..kept].iter().chain(messages).cloned().collect();
        assert_eq!(
            rebuilt, one.live,
            "round {round:?} rebuilds to the request it sent"
        );
        requests.insert(*round, rebuilt);
    }
}

/// The wire message a user record with `content` projects to.
fn user(content: &str) -> Value {
    json!({ "role": "user", "content": content })
}

/// The wire message an assistant reply with `content` projects to.
fn assistant(content: &str) -> Value {
    json!({ "role": "assistant", "content": content })
}

#[test]
fn a_tool_round_logs_only_the_call_and_its_result() {
    let (out, sent) = drive_rounds(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![
            tool_call_reply("call_1", "echo", json!({ "value": "hi" })),
            text_reply("done"),
        ],
    );
    assert_eq!(out, "done");
    assert_rebuilds(&sent);
    assert_eq!(sent.len(), 2, "two rounds");
    assert_eq!(sent[0].shape(), (None, 0, &[user("hello")][..]));
    let (after, keep, messages) = sent[1].shape();
    assert_eq!((after, keep), (Some(RoundId::new(0)), 1));
    let roles: Vec<&Value> = messages.iter().map(|message| &message["role"]).collect();
    assert_eq!(roles, [&json!("assistant"), &json!("tool")]);
}

#[test]
fn a_compaction_keeps_only_the_messages_before_the_first_change() {
    let (out, sent) = drive_rounds(
        "local msgs = messages.new()\n\
         msgs:system('be brief')\n\
         msgs:user('one')\n\
         models.loop(msgs)\n\
         msgs:user('two')\n\
         models.loop(msgs)\n\
         msgs:replace(2, 4, { role = 'user', content = 'one and two' })\n\
         msgs:user('three')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![text_reply("a1"), text_reply("a2"), text_reply("a3")],
    );
    assert_eq!(out, "a3");
    assert_rebuilds(&sent);
    assert_eq!(sent.len(), 3, "three sends");
    assert_eq!(
        sent[1].shape(),
        (
            Some(RoundId::new(0)),
            2,
            &[assistant("a1"), user("two")][..]
        )
    );
    assert_eq!(
        sent[2].shape(),
        (
            Some(RoundId::new(1)),
            1,
            &[user("one and two"), assistant("a2"), user("three")][..]
        ),
        "only the system message survives the compaction unchanged"
    );
}

#[test]
fn an_add_then_a_remove_between_two_sends_adds_nothing_to_the_log() {
    let (out, sent) = drive_rounds(
        "local msgs = messages.new()\n\
         msgs:user('one')\n\
         models.loop(msgs)\n\
         msgs:user('two')\n\
         msgs:user('scratch')\n\
         msgs:replace(#msgs, #msgs)\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![text_reply("a1"), text_reply("a2")],
    );
    assert_eq!(out, "a2");
    assert_rebuilds(&sent);
    assert_eq!(
        sent[1].shape(),
        (
            Some(RoundId::new(0)),
            1,
            &[assistant("a1"), user("two")][..]
        ),
        "the removed record never reaches the log"
    );
}

#[test]
fn a_resend_with_no_changes_logs_no_messages_and_keeps_the_whole_request() {
    let failure = CompletionError::new(CompletionErrorKind::MalformedResponse, "garbled reply");
    let (out, sent) = drive_rounds(
        "local msgs = messages.new()\n\
         msgs:user('one')\n\
         assert(not pcall(models.loop, msgs), 'the first round fails')\n\
         assert(#msgs == 1, 'a failed round adds no record')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![EffectAnswer::Chat(Err(failure)), text_reply("a1")],
    );
    assert_eq!(out, "a1");
    assert_rebuilds(&sent);
    assert_eq!(sent.len(), 2, "the failed send and its resend");
    assert_eq!(sent[1].shape(), (Some(RoundId::new(0)), 1, &[][..]));
}

#[test]
fn re_inserting_an_identical_record_adds_nothing_to_the_log() {
    let (out, sent) = drive_rounds(
        "local msgs = messages.new()\n\
         msgs:user('one')\n\
         models.loop(msgs)\n\
         msgs:user('two')\n\
         models.loop(msgs)\n\
         local first = msgs[1]\n\
         msgs:replace(1, 1)\n\
         msgs:replace(1, 0, first)\n\
         msgs:user('three')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![text_reply("a1"), text_reply("a2"), text_reply("a3")],
    );
    assert_eq!(out, "a3");
    assert_rebuilds(&sent);
    assert_eq!(
        sent[2].shape(),
        (
            Some(RoundId::new(1)),
            3,
            &[assistant("a2"), user("three")][..]
        ),
        "the re-inserted first record equals the one round 1 sent, so it is kept, not logged"
    );
}

#[test]
fn removing_the_terminal_reply_and_resending_repeats_the_request_it_answered() {
    let (out, sent) = drive_rounds(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         msgs:replace(#msgs, #msgs)\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![
            tool_call_reply("call_1", "echo", json!({ "value": "hi" })),
            text_reply("first"),
            text_reply("second"),
        ],
    );
    assert_eq!(out, "second");
    assert_rebuilds(&sent);
    assert_eq!(sent.len(), 3, "the tool round, its answer, and the resend");
    assert_eq!(
        sent[2].shape(),
        (Some(RoundId::new(1)), 3, &[][..]),
        "the resend is round 1's request again"
    );
}
