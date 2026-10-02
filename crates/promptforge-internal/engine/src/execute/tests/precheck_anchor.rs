//! Tests for the `chat` arm's context precheck beyond the bare estimate:
//! the room it keeps for the reply (the binding's `max_tokens`, else an
//! eighth of the window) and the anchor it takes from the provider's usage
//! after a served round, for the model that served it. Each case drives two
//! or three rounds over one list and one 4096-token window, so the last
//! round's count decides whether the request leaves. The chat arm's overflow flag is in `chat_arm`; the
//! compactor that receives it in `models_loop_compactors`.

use super::models_loop::{loop_context, loop_prompt};
use super::*;
use crate::lua::ToolSet;
use crate::model::{ModelBinding, ModelInvocation};
use crate::test_support::tokio_driver::TokioDriver;
use promptforge_types::metrics::{CallMetrics, Usage};

/// A text reply with the usage a provider would report for the round.
fn reply_with_usage(
    content: &str,
    prompt: u32,
    completion: u32,
    reasoning: Option<u32>,
) -> ScriptedReply {
    ScriptedReply::Text {
        model: MOCK_MODEL.to_owned(),
        content: content.to_owned(),
        finish_reason: None,
        reasoning: None,
        metrics: Some(Box::new(CallMetrics {
            usage: Some(Usage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
                cached_tokens: None,
                reasoning_tokens: reasoning,
            }),
            llama: None,
            vllm: None,
            client: None,
        })),
    }
}

/// Runs `lua` as the section of a loop prompt on the 4096-token window and
/// returns what it returned, beside the gateway that served its rounds.
async fn run_rounds(lua: &str, replies: Vec<ScriptedReply>) -> (String, ScriptedChat) {
    let gateway = ScriptedChat::new(replies);
    let prompt = parse(&loop_prompt(lua));
    let (ctx, harness) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, harness, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the section catches its own overflow");
    (out, gateway)
}

/// The section: a first round over `first` characters of user text, then a
/// second user message of `second` characters, and the second round's
/// outcome as `sent` or the exhaustion `kind:reason`.
fn two_rounds(first: usize, second: usize) -> String {
    format!(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', {first}))\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('y', {second}))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn an_anchored_count_refuses_a_second_round_the_estimate_would_pass() {
    // The first message is tiny, but the provider counted 3500 tokens for
    // the round and its reply (the system template and tool schemas weigh
    // in). The 100 tokens of new text bring the count to 3604, and the
    // 512-token reserve (an eighth of the window) tips it over 4096. The
    // plain estimate of the second request is about 113 tokens.
    let (out, gateway) = run_rounds(
        &two_rounds(5, 400),
        vec![reply_with_usage("ok", 3400, 100, None)],
    )
    .await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(
        gateway.call_count(),
        1,
        "the second request never leaves; only the first round ran"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_anchored_count_admits_a_second_round_the_estimate_would_refuse() {
    // The estimate of the second request is about 3762 tokens, over the
    // 3584 that fit beside the reserve. The provider counted 105 tokens
    // for the first round and its reply, so the anchored count is about
    // 1109 and the request leaves.
    let (out, gateway) = run_rounds(
        &two_rounds(11_000, 4000),
        vec![
            reply_with_usage("ok", 100, 5, None),
            reply_with_usage("done", 1200, 5, None),
        ],
    )
    .await;
    assert_eq!(out, "sent");
    assert_eq!(gateway.call_count(), 2, "both rounds left");
}

#[tokio::test(flavor = "current_thread")]
async fn a_round_with_no_reported_usage_leaves_the_second_round_on_the_estimate() {
    // The same conversation as the admitted case, but the first reply
    // reports no usage, so nothing anchors the second request and its full
    // estimate refuses it.
    let (out, gateway) = run_rounds(&two_rounds(11_000, 4000), vec![resp_text("ok")]).await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(gateway.call_count(), 1, "only the first round ran");
}

#[tokio::test(flavor = "current_thread")]
async fn reported_reasoning_tokens_come_off_the_anchor() {
    // 3000 prompt and 1200 completion tokens, 800 of them reasoning, anchor
    // at 3400. With the 104 tokens of new text and the 512 reserve the
    // count is 4016, which fits. Counting the reasoning would give 4816.
    let (out, gateway) = run_rounds(
        &two_rounds(5, 400),
        vec![
            reply_with_usage("ok", 3000, 1200, Some(800)),
            resp_text("done"),
        ],
    )
    .await;
    assert_eq!(out, "sent");
    assert_eq!(gateway.call_count(), 2);

    let (out, gateway) = run_rounds(
        &two_rounds(5, 400),
        vec![reply_with_usage("ok", 3000, 1200, None)],
    )
    .await;
    assert_eq!(
        out, "context_exhausted:precheck",
        "without the reasoning detail the same totals anchor at 4200"
    );
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_rewritten_history_falls_back_to_the_estimate() {
    // After a round with tiny usage, the author sends a rewritten list: the
    // same shape, but its first message differs from the one that was
    // sent. The anchor does not apply, and the full estimate refuses it.
    let section = "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 11000))\n\
         models.loop(msgs)\n\
         local rewritten = messages.new()\n\
         rewritten:user(string.rep('w', 11000)):assistant('ok'):user(string.rep('y', 4000))\n\
         local ok, err = pcall(models.loop, rewritten)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason";
    let (out, gateway) = run_rounds(section, vec![reply_with_usage("ok", 100, 5, None)]).await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(gateway.call_count(), 1, "only the first round ran");
}

#[tokio::test(flavor = "current_thread")]
async fn a_merged_history_falls_back_to_the_estimate() {
    // The author drops the reply and adds another user message, so the
    // projection merges the two user messages into one. That is not the
    // list the anchor measured, so the full estimate applies.
    let section = "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 11000))\n\
         models.loop(msgs)\n\
         msgs[#msgs] = nil\n\
         msgs:user(string.rep('y', 4000))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason";
    let (out, gateway) = run_rounds(section, vec![reply_with_usage("ok", 100, 5, None)]).await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(gateway.call_count(), 1, "only the first round ran");
}

/// [`run_rounds`] with a third binding, `twin`: the default model under
/// other invocation settings, so it shares `writer`'s tokenizer. `other`
/// names a different model.
async fn run_rounds_with_twin(lua: &str, replies: Vec<ScriptedReply>) -> (String, ScriptedChat) {
    let gateway = ScriptedChat::new(replies);
    let prompt = parse(&loop_prompt(lua));
    let (ctx, harness) = loop_context(&prompt, ToolSet::default());
    {
        let shared = ctx.model_set();
        let mut models = shared.lock().expect("the model set mutex is not poisoned");
        let writer = models.bindings[0].clone();
        let twin = ModelBinding::new(
            "twin",
            "The default model with other settings",
            writer.id().clone(),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: Some(false),
            },
            writer.context(),
        );
        models.bindings.push(twin);
    }
    let out = TokioDriver::new(&ctx, harness, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the section catches its own overflow");
    (out, gateway)
}

/// The model each request named, in the order they left.
fn request_models(gateway: &ScriptedChat) -> Vec<String> {
    gateway
        .requests()
        .iter()
        .map(|body| body.options.model().to_owned())
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn a_model_switch_leaves_the_second_round_on_the_estimate() {
    // The first round on `writer` anchors at 3500 tokens, which would
    // refuse the second request (3604 plus the 512 reserve). The second
    // request goes to `other`, a different model, so another tokenizer's
    // count does not apply and the estimate of about 113 tokens admits it.
    let section = "local msgs = messages.new()\n\
         msgs:user('xxxxx')\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('y', 400))\n\
         models.use('other')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason";
    let (out, gateway) = run_rounds(
        section,
        vec![reply_with_usage("ok", 3400, 100, None), resp_text("done")],
    )
    .await;
    assert_eq!(out, "sent");
    assert_eq!(gateway.call_count(), 2, "both rounds left");
    assert_eq!(request_models(&gateway), ["test-model", "other-model"]);
}

#[tokio::test(flavor = "current_thread")]
async fn another_alias_of_the_same_model_keeps_the_anchor() {
    // `twin` is a different alias with different settings, but it names the
    // model that `writer` named, so the 3500-token anchor still refuses the
    // second request.
    let section = "local msgs = messages.new()\n\
         msgs:user('xxxxx')\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('y', 400))\n\
         models.use('twin')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason";
    let (out, gateway) =
        run_rounds_with_twin(section, vec![reply_with_usage("ok", 3400, 100, None)]).await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(gateway.call_count(), 1, "only the first round ran");
}

#[tokio::test(flavor = "current_thread")]
async fn the_next_served_round_anchors_on_the_new_model() {
    // The switch to `other` runs on the estimate, because the first
    // round's 3500 tokens would refuse it. That round reports 3500 tokens
    // of its own. The third request stays on `other` and extends the
    // second round, so the new measurement anchors it and refuses it. The
    // estimate of the third request is about 220 tokens.
    let section = "local msgs = messages.new()\n\
         msgs:user('xxxxx')\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('y', 400))\n\
         models.use('other')\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('z', 400))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason";
    let (out, gateway) = run_rounds(
        section,
        vec![
            reply_with_usage("ok", 3400, 100, None),
            reply_with_usage("again", 3400, 100, None),
        ],
    )
    .await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(gateway.call_count(), 2, "only the first two rounds ran");
    assert_eq!(request_models(&gateway), ["test-model", "other-model"]);
}

#[tokio::test(flavor = "current_thread")]
async fn a_return_to_the_first_model_uses_the_estimate() {
    // The second round on `other` replaced the measurement, so the third
    // request back on `writer` meets another model's anchor and counts by
    // the estimate.
    let section = "local msgs = messages.new()\n\
         msgs:user('xxxxx')\n\
         models.loop(msgs)\n\
         msgs:user('yyyy')\n\
         models.use('other')\n\
         models.loop(msgs)\n\
         msgs:user(string.rep('z', 400))\n\
         models.use('writer')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason";
    let (out, gateway) = run_rounds(
        section,
        vec![
            reply_with_usage("ok", 3400, 100, None),
            reply_with_usage("again", 3400, 100, None),
            resp_text("done"),
        ],
    )
    .await;
    assert_eq!(out, "sent");
    assert_eq!(gateway.call_count(), 3);
    assert_eq!(
        request_models(&gateway),
        ["test-model", "other-model", "test-model"]
    );
}

/// Runs `lua` on a binding that sets `max_tokens`, over the 4096-token
/// window.
async fn run_with_max_tokens(lua: &str, max_tokens: u32) -> (String, ScriptedChat) {
    let gateway = ScriptedChat::new(vec![resp_text("ok")]);
    let prompt = parse(&loop_prompt(lua));
    let (ctx, harness) = loop_context(&prompt, ToolSet::default());
    {
        let shared = ctx.model_set();
        let mut models = shared.lock().expect("the model set mutex is not poisoned");
        for binding in &mut models.bindings {
            *binding = binding.clone().with_invocation(ModelInvocation {
                temperature: None,
                max_tokens: NonZeroU32::new(max_tokens),
                thinking: None,
            });
        }
    }
    let out = TokioDriver::new(&ctx, harness, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the section catches its own overflow");
    (out, gateway)
}

/// A one-round section over `size` characters of user text, returning
/// `sent` or the exhaustion `kind:reason`.
fn one_round(size: usize) -> String {
    format!(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', {size}))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         if ok then return 'sent' end\n\
         return err.kind .. ':' .. err.reason"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn a_bindings_max_tokens_is_the_room_kept_for_the_reply() {
    // A `max_tokens` of 1000 reserves 1000 tokens of the 4096 window. A
    // 12,000-character request estimates to 3004 tokens and fits; 12,400
    // characters estimate to 3104 and do not. The default reserve of 512
    // would admit both.
    let (out, gateway) = run_with_max_tokens(&one_round(12_000), 1000).await;
    assert_eq!(out, "sent");
    assert_eq!(gateway.call_count(), 1);

    let (out, gateway) = run_with_max_tokens(&one_round(12_400), 1000).await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck refuses before dispatch"
    );

    let (out, _gateway) = run_rounds(&one_round(12_400), vec![resp_text("ok")]).await;
    assert_eq!(out, "sent", "the default reserve admits the longer request");
}

#[tokio::test(flavor = "current_thread")]
async fn a_max_tokens_above_the_window_still_admits_a_small_request() {
    // The reserve stops at half the window, 2048 tokens, so a `max_tokens`
    // of 100,000 leaves room for a request that fits beside that half.
    let (out, gateway) = run_with_max_tokens(&one_round(400), 100_000).await;
    assert_eq!(out, "sent");
    assert_eq!(gateway.call_count(), 1);

    // The cap is the reserve, not a free pass: past half the window the
    // request is still refused (8,400 characters estimate to 2104 tokens).
    let (out, gateway) = run_with_max_tokens(&one_round(8400), 100_000).await;
    assert_eq!(out, "context_exhausted:precheck");
    assert_eq!(gateway.call_count(), 0);
}
