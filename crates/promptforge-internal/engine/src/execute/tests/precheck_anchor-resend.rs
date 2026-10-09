//! A send the context precheck refuses is not a send: it writes no Chat
//! record and leaves the list's last send in place, so the resend after an
//! edit extends the last round that was issued.

use promptforge_types::ids::RoundId;

use super::*;
use crate::execute::run::EffectRecord;

/// Runs `lua` as the section of a loop prompt on the 4096-token window and
/// returns what it returned, beside the record of every effect it issued.
async fn run_recorded(lua: &str, replies: Vec<ScriptedReply>) -> (String, Vec<EffectRecord>) {
    let gateway = ScriptedChat::new(replies);
    let prompt = parse(&loop_prompt(lua));
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let mut driver = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)));
    let records = driver.record_effects_for_test();
    let out = driver
        .drive()
        .await
        .expect("the section catches its own overflow");
    let records = records
        .lock()
        .expect("the tap mutex is not poisoned")
        .clone();
    (out, records)
}

#[tokio::test(flavor = "current_thread")]
async fn a_refused_send_writes_no_record_and_the_resend_extends_the_last_issued_round() {
    // The first round anchors at 3500 tokens, so 400 characters of new
    // text and the 512-token reserve refuse the second request before it
    // leaves. The author swaps that message for a short one and sends
    // again, which fits.
    let section = "local msgs = messages.new()\n\
         msgs:user('xxxxx')\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('y', 400))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok and err.reason == 'precheck', 'the precheck refuses the send')\n\
         msgs:replace(#msgs, #msgs, { role = 'user', content = 'zz' })\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content";
    let (out, records) = run_recorded(
        section,
        vec![reply_with_usage("ok", 3400, 100, None), resp_text("done")],
    )
    .await;
    assert_eq!(out, "done");
    let chats: Vec<(RoundId, Option<RoundId>, u64, &[Value])> = records
        .iter()
        .filter_map(|record| match record {
            EffectRecord::Chat {
                round,
                after,
                keep,
                messages,
                ..
            } => Some((*round, *after, *keep, messages.as_slice())),
            _ => None,
        })
        .collect();
    assert_eq!(
        chats.len(),
        2,
        "the refused send is not recorded: {chats:?}"
    );
    assert_eq!(
        chats[0],
        (
            RoundId::new(0),
            None,
            0,
            &[json!({ "role": "user", "content": "xxxxx" })][..]
        )
    );
    assert_eq!(
        chats[1],
        (
            RoundId::new(1),
            Some(RoundId::new(0)),
            1,
            &[
                json!({ "role": "assistant", "content": "ok" }),
                json!({ "role": "user", "content": "zz" }),
            ][..]
        ),
        "the resend extends round 0, and keep counts against round 0's one-message \
         request, not the refused one's"
    );
}
