//! The yield shims installed on a scheduler-mode section VM produce
//! well-formed protocol requests: `models.infer`, `call`, `fanout`, and
//! `tools.call` in their alias and handle forms, the local tool
//! handshake, the loop's per-call requesting turn, the optional leading
//! model handle, the captured alias globals, and the methodless handle
//! contract.

use promptforge_types::ids::TaskOrigin;
use promptforge_types::metrics::ToolCallEvent;
use serde_json::json;

use crate::execute::protocol::{
    Answer, ChatResult, LocalToolOutcome, Request, StoreOp, StoreOutcome, ToolCallOutcome,
    YieldParse,
};
use crate::model::ModelSet;

use super::{
    parse_request, resume_with, scheduler_vm, scheduler_vm_with_tools, start, test_models,
    test_tools, yielded_request,
};

#[test]
fn models_infer_yields_a_well_formed_request() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    match yielded_request(&vm, r#"return models.infer("summarize this")"#) {
        Request::Infer { prompt, binding } => {
            assert_eq!(prompt, "summarize this");
            assert_eq!(binding, None);
        }
        other => panic!("expected an infer request, got {other:?}"),
    }
}

#[test]
fn call_yields_target_input_and_the_var_snapshot() {
    let var = json!({ "k": 1 });
    let vm = scheduler_vm(&ModelSet::default(), Some(&var));
    match yielded_request(&vm, r###"return call("## Child", "override")"###) {
        Request::Call { target, input, var } => {
            assert_eq!(target, "## Child");
            assert_eq!(input.as_deref(), Some("override"));
            assert_eq!(var, json!({ "k": 1 }));
        }
        other => panic!("expected a call request, got {other:?}"),
    }
}

#[test]
fn fanout_yields_a_spawn_per_member_starting_with_the_first() {
    // The fanout shim is Lua over the task protocol: its first yield is the
    // `spawn` of the first member, including the worker as the target, the
    // member as the `item` seed, its 1-based position as `index`, the
    // caller's `var` snapshot, the author origin, and the fanout mark.
    let vm = scheduler_vm(&ModelSet::default(), None);
    match yielded_request(&vm, r####"return fanout("### Worker", {"a", "b"})"####) {
        Request::Spawn {
            target,
            input,
            item,
            index,
            var,
            origin,
            fanout,
        } => {
            assert_eq!(target, "### Worker");
            assert_eq!(input, None);
            assert_eq!(item, Some(json!("a")));
            assert_eq!(index, Some(1));
            assert_eq!(var, json!({}));
            assert_eq!(origin, TaskOrigin::Author);
            assert!(fanout, "an arm's spawn sets the fanout mark");
        }
        other => panic!("expected a spawn request, got {other:?}"),
    }
}

#[test]
fn fanout_rejects_an_empty_collection_before_any_spawn() {
    // The empty-collection guard runs in the shim before the first spawn
    // yield, so the call fails at the call site with the fixed message and
    // the driver never sees a request.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (kind, message): (String, String) = vm
        .lua()
        .load(
            "local ok, err = pcall(fanout, '### Worker', {})\n\
             assert(not ok, 'an empty collection must fail')\n\
             return err.kind, tostring(err)",
        )
        .call(())
        .expect("the rejection is a pcall-able error table");
    assert_eq!(kind, "lua");
    assert_eq!(
        message,
        "fanout over an empty collection: no work is likely a bug"
    );
}

#[test]
fn tasks_spawn_yields_the_target_seeds_var_and_author_origin() {
    let var = json!({ "k": 1 });
    let vm = scheduler_vm(&ModelSet::default(), Some(&var));
    let source =
        r###"return tasks.spawn("## Child", { input = "in", item = { name = "a" }, index = 2 })"###;
    match yielded_request(&vm, source) {
        Request::Spawn {
            target,
            input,
            item,
            index,
            var,
            origin,
            fanout,
        } => {
            assert_eq!(target, "## Child");
            assert_eq!(input.as_deref(), Some("in"));
            assert_eq!(item, Some(json!({ "name": "a" })));
            assert_eq!(index, Some(2));
            assert_eq!(var, json!({ "k": 1 }));
            assert_eq!(origin, TaskOrigin::Author);
            assert!(!fanout, "`tasks.spawn` is not a fanout arm");
        }
        other => panic!("expected a spawn request, got {other:?}"),
    }
}

#[test]
fn tasks_spawn_resumes_with_a_methodless_task_table() {
    // The shim wraps the resumed id in `{ task = id }`: a plain table with
    // no metatable and no methods, so a handle stored in `var` survives
    // the serde boundary unchanged.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (thread, _yielded) = start(
        &vm,
        "local t = tasks.spawn('## Child')\n\
         return t.task, getmetatable(t) == nil, next(t, 'task') == nil",
    );
    let (id, methodless, single_field): (String, bool, bool) = thread
        .resume((true, "0.3"))
        .expect("the shim returns the task table");
    assert_eq!(id, "0.3");
    assert!(methodless, "the task table is a plain table");
    assert!(single_field, "the task table holds exactly one field");
}

#[test]
fn tasks_spawn_rejects_non_table_options_at_the_call_site() {
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (kind, message): (String, String) = vm
        .lua()
        .load(
            "local ok, err = pcall(tasks.spawn, '## Child', 5)\n\
             assert(not ok, 'a non-table opts argument must fail')\n\
             return err.kind, tostring(err)",
        )
        .call(())
        .expect("the rejection is a pcall-able error table");
    assert_eq!(kind, "lua");
    assert_eq!(message, "tasks.spawn opts must be a table, got integer");
}

#[test]
fn tools_call_yields_a_well_formed_request() {
    // The tools.call shim installs in section VMs through the same setup
    // path as the other suspending calls; its yield parses into the
    // protocol's ToolCall variant with the author's args as JSON.
    let vm = scheduler_vm(&ModelSet::default(), None);
    match yielded_request(&vm, r#"return tools.call("echo", { value = "hi" })"#) {
        Request::ToolCall {
            alias,
            args,
            call_id,
            turn,
        } => {
            assert_eq!(alias, "echo");
            assert_eq!(args, json!({ "value": "hi" }));
            assert_eq!(call_id, None, "a script call leaves the call id unset");
            assert_eq!(turn, None, "a script call leaves the turn unset");
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_local_tool_handler_runs_inside_the_block_coroutine() {
    // The two-yield handshake end to end: the `tool_call` yield, the
    // `Local` answer handing over the handler, the handler's own `store`
    // yield from inside the same coroutine, the `local_tool_done` yield
    // carrying its return, and the block's return with the answered text.
    // `jump` is withheld while the handler runs and back afterward.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (thread, yielded) = start(
        &vm,
        "local withheld\n\
         tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           withheld = jump == nil\n\
           store.write('grab.txt', args.value)\n\
           return 'stored ' .. args.value\n\
         end)\n\
         local out = tools.call('grab', { value = 'hi' })\n\
         return out .. '|' .. tostring(withheld) .. '|' .. tostring(jump ~= nil)",
    );
    match parse_request(&vm, yielded) {
        Request::ToolCall { alias, call_id, .. } => {
            assert_eq!(alias, "grab");
            assert_eq!(call_id, None);
        }
        other => panic!("the block's first yield is the tool call, got {other:?}"),
    }
    let yielded = resume_with(
        &vm,
        &thread,
        Answer::ToolCallResult(Ok(ToolCallOutcome::Local {
            alias: "grab".to_owned(),
            args: json!({ "value": "hi" }),
        })),
    );
    match parse_request(&vm, yielded) {
        Request::Store {
            op: StoreOp::Write { path, contents },
        } => {
            assert_eq!(path, "grab.txt");
            assert_eq!(contents, "hi");
        }
        other => panic!("the handler's store call yields from the block, got {other:?}"),
    }
    let yielded = resume_with(&vm, &thread, Answer::Store(Ok(StoreOutcome::Unit)));
    match parse_request(&vm, yielded) {
        Request::LocalToolDone {
            outcome: LocalToolOutcome::Returned(text),
        } => assert_eq!(text, "stored hi"),
        other => panic!("the handler's return is reported, got {other:?}"),
    }
    let returned = resume_with(
        &vm,
        &thread,
        Answer::ToolCallResult(Ok(ToolCallOutcome::Plain("stored hi".to_owned()))),
    );
    let text: String = vm
        .lua()
        .unpack(
            returned
                .into_iter()
                .next()
                .expect("the block returns a value"),
        )
        .expect("the block returns a string");
    assert_eq!(text, "stored hi|true|true");
}

#[test]
fn the_loops_tool_call_yields_carry_the_turn_of_the_requesting_round() {
    // A round requesting two calls under turn 7: each `tool_call` yield
    // carries that turn beside its call id, so a call reports the round
    // that requested it however far the counter moved in between.
    let vm = scheduler_vm_with_tools(&test_models(), &test_tools(), None);
    let (thread, yielded) = start(
        &vm,
        "local msgs = messages.new()\nmsgs:user('hi')\nmodels.loop(msgs)\nreturn #msgs",
    );
    assert!(matches!(
        parse_request(&vm, yielded),
        Request::DrainTaskNotices
    ));
    let yielded = resume_with(&vm, &thread, Answer::DrainTaskNotices(Ok(Vec::new())));
    assert!(matches!(parse_request(&vm, yielded), Request::Chat { .. }));
    let call = |id: &str| ToolCallEvent {
        id: id.to_owned(),
        name: "echo".to_owned(),
        arguments: json!({ "value": id }),
    };
    let round = ChatResult {
        overflow: false,
        overflow_reason: None,
        reply: None,
        empty_detail: None,
        tool_calls: Some(vec![call("c1"), call("c2")]),
        finish_reason: Some("tool_calls".to_owned()),
        model: "test-model".to_owned(),
        metrics: None,
        turn: 7,
    };
    let mut yielded = resume_with(&vm, &thread, Answer::Chat(Ok(Box::new(round))));
    for expected in ["c1", "c2"] {
        match parse_request(&vm, yielded) {
            Request::ToolCall { call_id, turn, .. } => {
                assert_eq!(call_id.as_deref(), Some(expected));
                assert_eq!(
                    turn,
                    Some(7),
                    "{expected} carries the requesting round's turn"
                );
            }
            other => panic!("the loop yields the model's tool call, got {other:?}"),
        }
        yielded = resume_with(
            &vm,
            &thread,
            Answer::ToolCallResult(Ok(ToolCallOutcome::Plain("echoed".to_owned()))),
        );
    }
    assert!(matches!(
        parse_request(&vm, yielded),
        Request::DrainTaskNotices
    ));
}

#[test]
fn the_bare_tool_call_global_is_not_installed() {
    // Every tool operation lives under the `tools.*` namespace; the bare
    // global from before the rename must be gone, not aliased.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let is_nil: bool = vm
        .lua()
        .load("return tool_call == nil")
        .eval()
        .expect("the global read evaluates");
    assert!(is_nil, "the bare `tool_call` global must not exist");
}

#[test]
fn tools_call_accepts_a_tool_handle_in_place_of_the_alias() {
    // The captured alias global is an inspectable Tool object; passing it
    // as the leading argument dispatches the binding it names.
    let vm = scheduler_vm_with_tools(&ModelSet::default(), &test_tools(), None);
    match yielded_request(&vm, r#"return tools.call(echo, { value = "hi" })"#) {
        Request::ToolCall { alias, args, .. } => {
            assert_eq!(alias, "echo");
            assert_eq!(args, json!({ "value": "hi" }));
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn tools_call_rejects_a_non_alias_non_tool_first_argument() {
    // The polymorphism is alias string or Tool object; anything else is
    // the call's own error at the protocol boundary, so an author pcall
    // catches it at the call site.
    let vm = scheduler_vm(&ModelSet::default(), None);
    let (_thread, yielded) = start(&vm, "return tools.call(42, {})");
    let value = yielded.into_iter().next().expect("one yielded value");
    match Request::from_yield(vm.lua(), &value) {
        YieldParse::Call(answer) => {
            let message = format!("{answer:?}");
            assert!(
                message.contains("tools.call alias must be a string or Tool object"),
                "the rejection names the expected forms: {message}"
            );
        }
        other => panic!("expected the call's own error, got {other:?}"),
    }
}

#[test]
fn models_infer_takes_an_optional_leading_handle() {
    let vm = scheduler_vm(&test_models(), None);
    let request = yielded_request(
        &vm,
        r#"
            local h = models.get("fast")
            local u = models.use("fast")
            assert(h.name == "fast" and h.model_id == "test-model")
            assert(u.name == "fast")
            return models.infer(h, "yo")
            "#,
    );
    match request {
        Request::Infer {
            prompt,
            binding: Some(binding),
        } => {
            assert_eq!(prompt, "yo");
            assert_eq!(binding.alias(), "fast");
            assert_eq!(binding.id().name(), "test-model");
        }
        other => panic!("expected an infer request with a binding, got {other:?}"),
    }
}

#[test]
fn captured_model_aliases_install_as_plain_handles() {
    let vm = scheduler_vm(&test_models(), None);
    match yielded_request(&vm, r#"return models.infer(fast, "yo")"#) {
        Request::Infer {
            prompt,
            binding: Some(binding),
        } => {
            assert_eq!(prompt, "yo");
            assert_eq!(binding.alias(), "fast");
        }
        other => panic!("expected an infer request with a binding, got {other:?}"),
    }
}

#[test]
fn handles_reject_colon_methods() {
    // Namespace-only invocation: a handle is a frozen, inspectable value,
    // so the old `handle:infer` method is gone - reading `infer` off the
    // userdata fails, and the one invocation form is the leading handle
    // argument to `models.infer`.
    let vm = scheduler_vm(&test_models(), None);
    let (is_userdata, read_failed): (bool, bool) = vm
        .lua()
        .load(
            r#"
            local h = models.get("fast")
            local ok = pcall(function() return h.infer end)
            return type(h) == "userdata" and type(fast) == "userdata", not ok
            "#,
        )
        .eval()
        .expect("the handle probe evaluates");
    assert!(is_userdata, "handles install as bare userdata");
    assert!(read_failed, "a handle has no `infer` field to call");
}
