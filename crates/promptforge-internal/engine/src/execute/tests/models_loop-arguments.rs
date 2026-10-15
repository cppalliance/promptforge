//! Tests for how `models.loop` and a model handle's `loop` read their
//! arguments: every documented form resolves the `messages.new()` list,
//! each entry's argument errors are raised in order before any request
//! leaves, and a plain array in the list's place is refused with the list
//! error, also while a task notice is pending, which then waits for a
//! round over a list.

use promptforge_parser::test_support::one_section;

use super::super::model_task_notices::loop_owner;
use super::super::model_tasks::{PARKED_CHILD, model_task_scenario, owner_prompt};
use super::*;

/// The list refusal both loop entries raise, as `kind:message`.
const LIST_REFUSAL: &str = "lua:models.loop needs a messages.new() list; build one with \
                            messages.new() and :user, :append, or :replace";

/// Runs each call in `calls`, a Lua list of functions that each make one
/// refused loop call, under `pcall`, and returns the section's result:
/// every error's `kind:message`, joined by `|`, then the list's size. No
/// gateway reply is scripted, so a call that sends a request fails the
/// run.
#[track_caller]
fn refusals(calls: &str) -> String {
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(&format!(
        "local other = models.get('other')
         local msgs = messages.new()
         msgs:user('hello')
         local out = {{}}
         for _, call in ipairs({{ {calls} }}) do
           local ok, err = pcall(call)
           assert(not ok, 'the call is refused')
           out[#out + 1] = err.kind .. ':' .. tostring(err)
         end
         return table.concat(out, '|') .. '|' .. #msgs"
    ));
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(
        gateway.call_count(),
        0,
        "every refusal fires before any request leaves"
    );
    trace.text().to_owned()
}

#[test]
fn every_argument_form_resolves_the_list() {
    let gateway = ScriptedChat::new(vec![text("r1"), text("r2"), text("r3"), text("r4")]);
    let source = one_section(
        "local other = models.get('other')
         local msgs = messages.new()
         msgs:user('one')
         models.loop(msgs)
         msgs:user('two')
         models.loop(msgs, compactors.fail)
         msgs:user('three')
         other:loop(msgs)
         msgs:user('four')
         other:loop(msgs, compactors.fail)
         local replies = {}
         for i = 2, #msgs, 2 do replies[#replies + 1] = msgs[i].content end
         return #msgs .. '|' .. table.concat(replies, ',')",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "8|r1,r2,r3,r4");
    let models: Vec<String> = gateway
        .requests()
        .iter()
        .map(|body| body.options.model().to_owned())
        .collect();
    assert_eq!(
        models,
        ["test-model", "test-model", "other-model", "other-model"],
        "only the handle's own loop runs on the handle's binding"
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

#[test]
fn a_handle_passed_to_models_loop_is_the_pointed_error() {
    let out = refusals(
        "function() return models.loop(other, msgs) end,
         function() return models.loop(other, msgs, compactors.fail) end,",
    );
    let pointed = "lua:models.loop takes (messages, compactor?); \
                   call handle:loop(messages, compactor?) to run on a model handle";
    assert_eq!(out, format!("{pointed}|{pointed}|1"));
}

#[test]
fn a_handles_loop_called_without_its_receiver_is_the_colon_error() {
    // A dot call passes the list where the receiver belongs, and the
    // function taken off the handle and called bare has no receiver at all.
    let out = refusals(
        "function() return other.loop(msgs) end,
         function() local run = other.loop; return run(msgs, compactors.fail) end,
         function() return other.loop() end,",
    );
    let colon = "lua:call loop on a model handle with a colon: handle:loop(messages, compactor?)";
    assert_eq!(out, format!("{colon}|{colon}|{colon}|1"));
}

#[test]
fn each_loop_entry_refuses_extra_arguments_counting_trailing_nils() {
    let out = refusals(
        "function() return models.loop(msgs, compactors.fail, 1) end,
         function() return models.loop(msgs, nil, nil) end,
         function() return other:loop(msgs, compactors.fail, 1) end,
         function() return other:loop(msgs, nil, nil) end,",
    );
    let entry = "lua:models.loop takes (messages, compactor?)";
    let method = "lua:handle:loop takes (messages, compactor?)";
    assert_eq!(out, format!("{entry}|{entry}|{method}|{method}|1"));
}

#[test]
fn a_non_list_in_either_entry_is_the_list_error() {
    let out = refusals(
        "function() return models.loop() end,
         function() return models.loop('hello', compactors.fail) end,
         function() return other:loop() end,
         function() return other:loop(42, compactors.fail) end,",
    );
    let refused = [LIST_REFUSAL; 4].join("|");
    assert_eq!(out, format!("{refused}|1"));
}

#[test]
fn each_entry_checks_the_handle_then_the_arity_then_the_compactor_then_the_list() {
    // Every call fails two adjacent checks, so the earlier check names
    // the error.
    let out = refusals(
        "function() return models.loop(other, msgs, 1, 2) end,
         function() return models.loop(42, 42, nil) end,
         function() return models.loop(42, 42) end,
         function() return other.loop(msgs, nil, nil, nil) end,
         function() return other:loop(42, 42, nil) end,
         function() return other:loop(42, 42) end,",
    );
    assert_eq!(
        out,
        "lua:models.loop takes (messages, compactor?); \
         call handle:loop(messages, compactor?) to run on a model handle\
         |lua:models.loop takes (messages, compactor?)\
         |lua:compactor must be a function, got integer\
         |lua:call loop on a model handle with a colon: handle:loop(messages, compactor?)\
         |lua:handle:loop takes (messages, compactor?)\
         |lua:compactor must be a function, got integer\
         |1"
    );
}

#[test]
fn a_plain_array_in_the_lists_place_is_refused_with_the_list_error() {
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "local other = models.get('other')
         local plain = { { role = 'user', content = 'hi' } }
         local out = {}
         for _, call in ipairs({
           function() return other:loop(plain) end,
           function() return models.loop(plain) end,
         }) do
           local ok, err = pcall(call)
           assert(not ok, 'a plain array is refused')
           out[#out + 1] = err.kind .. ':' .. tostring(err)
         end
         return table.concat(out, '|')",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), format!("{LIST_REFUSAL}|{LIST_REFUSAL}"));
    assert_eq!(
        gateway.call_count(),
        0,
        "the refusal fires before any request leaves"
    );
}

#[test]
fn a_non_list_is_refused_with_the_list_error_while_a_task_notice_waits_for_the_list() {
    // The author's cancel of the model's parked task queues its notice at
    // once, so every refused call below runs with that notice pending.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        text("started"),
        text("read"),
    ]);
    let source = owner_prompt(
        "",
        &loop_owner(
            "tasks.cancel(tasks.pending({ origin = 'model' })[1])\n\
             local other = models.get('other')\n\
             local plain = { { role = 'user', content = 'hi' } }\n\
             local out = {}\n\
             for _, call in ipairs({\n\
               function() return other:loop(plain) end,\n\
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
    let trace = model_task_scenario(source).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        format!(
            "{LIST_REFUSAL}|{LIST_REFUSAL}|{LIST_REFUSAL}|\
             user:Task id=0.0 (## Child) was canceled: the author cancelled it"
        ),
        "the notice stays queued through the refusals and reaches the list's next round"
    );
    assert_eq!(gateway.call_count(), 3, "no refused call sends a request");
}
