//! Control flow out of the live H1 pass: `call` as a contained chain and
//! the child ids it takes, `jump` to start the walk at a target, `fanout`,
//! the decision-tool idiom, and `list_from_section`, with the
//! unknown-section failures.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn call_from_h1_runs_the_target_as_a_contained_chain() {
    // H1 is section 0, so `call` resolves against the top-level sections
    // exactly as in any section.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\nvar.answer = call('## Answer')\n```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n\n\
        ## Answer\n\n\
        ```lua\nreturn 'called from h1'\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("call from H1 runs the target section");

    assert_eq!(out, "called from h1");
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_from_h1_and_a_call_from_the_first_walked_section_take_consecutive_child_ids() {
    // The H1 pass and the walk that follows it are one root chain, so the
    // hand-off copies the pass's child counter into the walk: a `call`
    // the pass made is child `0.0`, and the walk's first `call` is child
    // `0.1`, not a second `0.0`. Were the counter copy dropped at the
    // hand-off, both calls would read `0.0.0`. The walk's own entry
    // counter continues too: its first section is still `0.1`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\nvar.first = call('## Answer')\n```\n\n\
        ## Result\n\n\
        ```lua\n\
        assert(sys.id == '0.1', 'the first walked section takes root entry 1')\n\
        return var.first .. ',' .. call('## Answer')\n\
        ```\n\n\
        ## Answer\n\n\
        ```lua\nreturn sys.id\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a call from H1 and a call from the walk both complete");

    assert_eq!(
        out, "0.0.0,0.1.0",
        "the H1 call is root child 0 and the walk's call is root child 1"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn call_from_h1_to_an_unknown_section_is_a_catchable_error() {
    // A `call` naming no visible section fails as the call's answer: the
    // shim raises it at the call site, where an author `pcall` catches it;
    // uncaught, it ends the run as the H1 gate failure.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\nlocal ok, err = pcall(call, '## Nope'); return tostring(ok) .. ':' .. tostring(err)\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the caught call failure is the run's result");

    assert!(
        out.starts_with("false:") && out.contains("## Nope"),
        "the caught error names the missing section: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_from_h1_starts_the_walk_at_the_target() {
    // A jump out of H1 ends the pass and starts the walk at the resolved
    // top-level target, skipping the sections before it.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\njump('## Target')\n```\n\n\
        ## Skipped\n\n\
        ```lua\nerror('the jump target must skip this section')\n```\n\n\
        ## Target\n\n\
        ```lua\nreturn 'jumped'\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("jump from H1 starts the walk at the target");

    assert_eq!(out, "jumped");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_from_h1_to_an_unknown_section_fails_the_run() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\njump('## Nope')\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("jump from H1 to an unknown section must fail");

    assert!(
        error.to_string().contains("## Nope"),
        "the failure names the missing section: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_from_h1_runs_the_worker_over_the_collection() {
    // `fanout` works in H1 as in any section: the worker resolves against
    // the top-level sections and the arms join in collection order.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\n\
        local r = fanout('## Worker', {'a', 'b'})\n\
        var.answer = r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n\n\
        ## Worker\n\n\
        ```lua\nreturn 'item:' .. item\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("fanout from H1 joins the arms");

    assert_eq!(out, "item:a|item:b");
}

#[tokio::test(flavor = "current_thread")]
async fn the_h1_decision_tool_idiom_runs_before_the_walk() {
    // The decision-tool idiom in H1: a local tool with an enum parameter is
    // the verdict channel - the model's loop call lands in the Lua handler,
    // and the captured verdict drives the run's shape before the walk. This
    // needs `tools.add_local` and `models.loop` in H1, both section-only
    // before the one-install-path consolidation.
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "decide", "{\"choice\":\"use_mcp\"}"),
        resp_text("decided"),
    ]);
    let md = "---\nname: h1-decision\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Decide\n\n\
        ```lua\n\
        models.default('writer')\n\
        tools.add_local('decide', 'Record the verdict', { choice = 'string' }, function(args)\n\
          var.verdict = args.choice\n\
          return 'recorded'\n\
        end)\n\
        local msgs = messages.new()\n\
        msgs:user('interpret the guidance')\n\
        models.loop(msgs)\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.verdict\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the H1 decision-tool idiom runs");

    assert_eq!(out, "use_mcp");
    assert_eq!(
        gateway.call_count(),
        2,
        "the loop runs the tool-call round and the terminal text round"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn list_from_section_works_on_the_h1() {
    // `list_from_section` resolves over H1's visible set - the whole
    // top-level slice.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\n\
        local items = list_from_section('## Items')\n\
        var.answer = table.concat(items, ',')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n\n\
        ## Items\n\n\
        - one\n\
        - two\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("list_from_section from H1 reads the target's items");

    assert_eq!(out, "one,two");
}
