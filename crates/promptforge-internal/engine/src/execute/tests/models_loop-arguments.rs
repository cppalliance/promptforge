//! Tests for how `models.loop` reads its arguments: every documented form
//! resolves the `messages.new()` list, and a plain array in the list's
//! place is refused with the list error before any request leaves, also
//! while a task notice is pending, which then waits for a round over a
//! list.

use super::super::model_task_notices::loop_owner;
use super::super::model_tasks::{PARKED_CHILD, model_task_context_with, owner_prompt};
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn every_argument_form_resolves_the_list() {
    let gateway = ScriptedChat::new(vec![
        resp_text("r1"),
        resp_text("r2"),
        resp_text("r3"),
        resp_text("r4"),
    ]);
    let md = loop_prompt(
        "local other = models.get('other')\n\
         local msgs = messages.new()\n\
         msgs:user('one')\n\
         models.loop(msgs)\n\
         msgs:user('two')\n\
         models.loop(msgs, compactors.fail)\n\
         msgs:user('three')\n\
         models.loop(other, msgs)\n\
         msgs:user('four')\n\
         models.loop(other, msgs, compactors.fail)\n\
         local replies = {}\n\
         for i = 2, #msgs, 2 do replies[#replies + 1] = msgs[i].content end\n\
         return #msgs .. '|' .. table.concat(replies, ',')",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("every argument form runs its round");
    assert_eq!(out, "8|r1,r2,r3,r4");
    let models: Vec<String> = gateway
        .requests()
        .iter()
        .map(|body| body.options.model().to_owned())
        .collect();
    assert_eq!(
        models,
        ["test-model", "test-model", "other-model", "other-model"],
        "only the forms with a leading handle run on the handle's binding"
    );
    let sizes: Vec<usize> = gateway
        .requests()
        .iter()
        .map(|body| body.messages.len())
        .collect();
    assert_eq!(
        sizes,
        [1, 3, 5, 7],
        "each round sends the whole list the author built"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_plain_array_in_the_lists_place_is_refused_with_the_list_error() {
    let gateway = ScriptedChat::new(vec![resp_text("unreachable")]);
    let md = loop_prompt(
        "local plain = { { role = 'user', content = 'hi' } }\n\
         local out = {}\n\
         for _, call in ipairs({\n\
           function() return models.loop(models.get('other'), plain) end,\n\
           function() return models.loop(plain) end,\n\
         }) do\n\
           local ok, err = pcall(call)\n\
           assert(not ok, 'a plain array is refused')\n\
           out[#out + 1] = err.kind .. ':' .. tostring(err)\n\
         end\n\
         return table.concat(out, '|')",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the refusal is pcall-able at the call site");
    let refusal = "lua:models.loop needs a messages.new() list; build one with \
                   messages.new() and :user, :append, or :replace";
    assert_eq!(out, format!("{refusal}|{refusal}"));
    assert_eq!(
        gateway.call_count(),
        0,
        "the refusal fires before any request leaves"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_list_is_refused_with_the_list_error_while_a_task_notice_waits_for_the_list() {
    // The author's cancel of the model's parked task queues its notice at
    // once, so every refused call below runs with that notice pending.
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_text("started"),
        resp_text("read"),
    ]);
    let md = owner_prompt(
        "",
        &loop_owner(
            "tasks.cancel(tasks.pending({ origin = 'model' })[1])\n\
             local plain = { { role = 'user', content = 'hi' } }\n\
             local out = {}\n\
             for _, call in ipairs({\n\
               function() return models.loop(models.get('other'), plain) end,\n\
               function() return models.loop(plain) end,\n\
               function() return models.loop(msgs[1]) end,\n\
             }) do\n\
               local ok, err = pcall(call)\n\
               assert(not ok, 'a non-list is refused')\n\
               out[#out + 1] = err.kind .. ':' .. tostring(err)\n\
             end\n\
             models.loop(msgs)\n\
             out[#out + 1] = msgs[5].role .. ':' .. msgs[5].content\n\
             return table.concat(out, '|')",
        ),
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let (ctx, fixture) = model_task_context_with(
        &prompt,
        Arc::new(NullObserver::default()),
        Arc::new(SlowTool),
    );
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("each refusal is pcall-able at the call site");
    let refusal = "lua:models.loop needs a messages.new() list; build one with \
                   messages.new() and :user, :append, or :replace";
    assert_eq!(
        out,
        format!(
            "{refusal}|{refusal}|{refusal}|\
             user:Task id=0.0 (## Child) was canceled: the author cancelled it"
        ),
        "the notice stays queued through the refusals and reaches the list's next round"
    );
    assert_eq!(gateway.call_count(), 3, "no refused call sends a request");
}
