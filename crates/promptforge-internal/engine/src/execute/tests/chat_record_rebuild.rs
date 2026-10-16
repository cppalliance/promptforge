//! A run log's Chat records rebuild every request a `messages.new()` list
//! sent: walking the records in log order, the first `keep` messages of
//! the request rebuilt for `after`, then the record's own messages, equal
//! the wire messages on that round's live effect. Each case edits the list
//! between sends in a way that changes what the log must hold.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use promptforge_parser::test_support::one_section;
use promptforge_types::ids::RoundId;

use super::fixtures::echo_tool;
use super::models_loop::loop_scenario;
use super::*;
use crate::execute::run::{Effect, EffectAnswer, EffectRecord};
use crate::model::{CompletionError, CompletionErrorKind};
use crate::test_support::scenario::chat_answer;

/// One issued round as the driver saw it: its record, and the wire
/// messages the live effect sent.
struct Sent {
    record: EffectRecord,
    live: Vec<Value>,
}

impl Sent {
    /// The record's `after`, `keep`, and logged messages.
    fn shape(&self) -> (Option<RoundId>, u64, &[Value]) {
        match &self.record {
            EffectRecord::Chat {
                after,
                keep,
                messages,
                ..
            } => (*after, *keep, messages),
            other => panic!("a round records a chat record: {other:?}"),
        }
    }
}

/// Each round a scenario's model was sent, as it was sent, in issue order.
type SentLog = Rc<RefCell<Vec<Sent>>>;

/// A loop section running `lua` with `echo` bound and offered, its model
/// rounds answered from `replies` in order and every tool call by the echo
/// tool, beside the log its model keeps of each round it is sent.
fn echo_rounds(lua: &str, replies: Vec<EffectAnswer>) -> (Scenario, SentLog) {
    let replies = RefCell::new(replies.into_iter());
    let sent = SentLog::default();
    let rounds = Rc::clone(&sent);
    let scenario = loop_scenario(one_section(&format!("tools.offer('tools/echo')\n{lua}")))
        .offer("echo", echo_tool())
        .chat_with(move |effect| {
            let messages = match effect {
                Effect::Chat { messages, .. } => messages,
                other => panic!("only model rounds reach the model: {other:?}"),
            };
            rounds.borrow_mut().push(Sent {
                record: effect.record(),
                live: messages
                    .iter()
                    .map(|message| serde_json::to_value(message).expect("a message serializes"))
                    .collect(),
            });
            replies
                .borrow_mut()
                .next()
                .expect("the script covers every round")
        });
    (scenario, sent)
}

/// Asserts `trace` issued only model rounds and tool calls, then rebuilds
/// every request from the records alone and asserts each equals the wire
/// messages its live effect sent. Every `after` must name a round recorded
/// earlier in the log.
#[track_caller]
fn assert_rebuilds(trace: &Trace, sent: &[Sent]) {
    assert!(
        trace.effects().iter().all(|(effect, _)| matches!(
            effect,
            EffectRecord::Chat { .. } | EffectRecord::ToolCall { .. }
        )),
        "only model rounds and tool calls are issued: {:?}",
        trace.effects()
    );
    let mut requests: BTreeMap<RoundId, Vec<Value>> = BTreeMap::new();
    for one in sent {
        let round = match &one.record {
            EffectRecord::Chat { round, .. } => round,
            other => panic!("a round records a chat record: {other:?}"),
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
    let (scenario, log) = echo_rounds(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![
            chat_answer(&calls(&[("call_1", "echo", r#"{"value":"hi"}"#)])),
            chat_answer(&text("done")),
        ],
    );
    let trace = scenario.run();
    let sent = log.take();
    assert_eq!(trace.text(), "done");
    assert_rebuilds(&trace, &sent);
    assert_eq!(sent.len(), 2, "two rounds");
    assert_eq!(sent[0].shape(), (None, 0, &[user("hello")][..]));
    let (after, keep, messages) = sent[1].shape();
    assert_eq!((after, keep), (Some(RoundId::new(0)), 1));
    let roles: Vec<&Value> = messages.iter().map(|message| &message["role"]).collect();
    assert_eq!(roles, [&json!("assistant"), &json!("tool")]);
}

#[test]
fn a_compaction_keeps_only_the_messages_before_the_first_change() {
    let (scenario, log) = echo_rounds(
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
        vec![
            chat_answer(&text("a1")),
            chat_answer(&text("a2")),
            chat_answer(&text("a3")),
        ],
    );
    let trace = scenario.run();
    let sent = log.take();
    assert_eq!(trace.text(), "a3");
    assert_rebuilds(&trace, &sent);
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
    let (scenario, log) = echo_rounds(
        "local msgs = messages.new()\n\
         msgs:user('one')\n\
         models.loop(msgs)\n\
         msgs:user('two')\n\
         msgs:user('scratch')\n\
         msgs:replace(#msgs, #msgs)\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![chat_answer(&text("a1")), chat_answer(&text("a2"))],
    );
    let trace = scenario.run();
    let sent = log.take();
    assert_eq!(trace.text(), "a2");
    assert_rebuilds(&trace, &sent);
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
    let (scenario, log) = echo_rounds(
        "local msgs = messages.new()\n\
         msgs:user('one')\n\
         assert(not pcall(models.loop, msgs), 'the first round fails')\n\
         assert(#msgs == 1, 'a failed round adds no record')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![EffectAnswer::Chat(Err(failure)), chat_answer(&text("a1"))],
    );
    let trace = scenario.run();
    let sent = log.take();
    assert_eq!(trace.text(), "a1");
    assert_rebuilds(&trace, &sent);
    assert_eq!(sent.len(), 2, "the failed send and its resend");
    assert_eq!(sent[1].shape(), (Some(RoundId::new(0)), 1, &[][..]));
}

#[test]
fn re_inserting_an_identical_record_adds_nothing_to_the_log() {
    let (scenario, log) = echo_rounds(
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
        vec![
            chat_answer(&text("a1")),
            chat_answer(&text("a2")),
            chat_answer(&text("a3")),
        ],
    );
    let trace = scenario.run();
    let sent = log.take();
    assert_eq!(trace.text(), "a3");
    assert_rebuilds(&trace, &sent);
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
    let (scenario, log) = echo_rounds(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         msgs:replace(#msgs, #msgs)\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content",
        vec![
            chat_answer(&calls(&[("call_1", "echo", r#"{"value":"hi"}"#)])),
            chat_answer(&text("first")),
            chat_answer(&text("second")),
        ],
    );
    let trace = scenario.run();
    let sent = log.take();
    assert_eq!(trace.text(), "second");
    assert_rebuilds(&trace, &sent);
    assert_eq!(sent.len(), 3, "the tool round, its answer, and the resend");
    assert_eq!(
        sent[2].shape(),
        (Some(RoundId::new(1)), 3, &[][..]),
        "the resend is round 1's request again"
    );
}
