//! H1-only prompts and H1 prose: an H1-only return or fall-through, a
//! scalar H1 return short-circuiting the walk, and H1 prose read only
//! through an explicit infer.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn h1_only_lua_return() {
    // An H1-only prompt's scalar return is the run's result.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ```lua\nreturn \"hello\"\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the H1-only return runs");

    assert_eq!(out, "hello");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_only_lua_no_return() {
    // An H1-only prompt that produces nothing ends in the shared generic
    // completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ```lua\nlocal x = 1\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the H1-only fall-through runs");

    assert_eq!(out, "done");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_scalar_return_short_circuits_the_walk() {
    // The short-circuit half of the H1 return rule: a scalar return from
    // the live H1 pass ends the whole run, so no section ever runs - the
    // walk's erroring section is the tripwire.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Short Circuit\n\n\
        ```lua\nreturn 'early'\n```\n\n\
        ## Never\n\n\
        ```lua\nerror('the walk must not start after an H1 return')\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the H1 return short-circuits the run");

    assert_eq!(out, "early");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_prose_inferred_explicitly_is_the_run_result() {
    // An H1-only prompt whose Lua reads its pending buffer into an explicit
    // infer ends the run with the inferred text: the scalar return
    // short-circuits the (empty) walk.
    let gateway = ScriptedChat::new(vec![resp_text("h1 reply")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Only Prose\n\n\
        ```lua\n\
        models.default('writer')\n\
        ```\n\n\
        say something\n\n\
        ```lua\n\
        return models.infer(prose)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the H1 infer of its prose ends the run");

    assert_eq!(out, "h1 reply");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn h1_and_h2_prose_each_infer_explicitly_in_source_order() {
    // The live H1 pass and the H2 section each read their own pending
    // buffer into an explicit infer - two completions, in source order.
    let gateway = ScriptedChat::new(vec![resp_text("h1 reply"), resp_text("h2 reply")]);
    let md = "---\nname: shared-loop\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Shared Loop\n\n\
        ```lua\n\
        models.default('writer')\n\
        ```\n\n\
        h1 prose turn\n\n\
        ```lua\n\
        var.h1 = models.infer(prose)\n\
        ```\n\n\
        ## Section Two\n\n\
        h2 prose turn\n\n\
        ```lua\n\
        return models.infer(prose)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("H1 prose and H2 prose each infer explicitly");

    assert_eq!(out, "h2 reply");
    assert_eq!(
        gateway.call_count(),
        2,
        "the H1 prose and the H2 prose each drive exactly one completion"
    );
    let requests = gateway.requests();
    let first_prose = requests[0].messages[0].content();
    let second_prose = requests[1].messages[0].content();
    assert!(
        first_prose.contains("h1 prose turn"),
        "the first completion is the H1 prose: {first_prose}"
    );
    assert!(
        second_prose.contains("h2 prose turn"),
        "the second completion is the H2 prose: {second_prose}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn unread_h1_prose_stays_inert_and_explicit_infer_requires_a_model() {
    // H1 prose does not drive inference, so an unread buffer - even one
    // whose substitution would fail or stay empty - discards at the pass's
    // end without requiring a model. Only an explicit `models.infer` of the
    // prose requires a binding.
    let unread = "---\nname: empty-h1\ndescription: d\npromptforge: 0\n---\n\n\
        # Empty H1\n\n\
        ```lua\nvar.omit = ''\n```\n\n\
        {{ var.omit }}\n\n\
        ## Result\n\n\
        ```lua\nreturn 'ok'\n```\n";
    let prompt = parse(unread);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("unread H1 prose must not require a model");
    assert_eq!(out, "ok");

    let reading = "---\nname: read-h1\ndescription: d\npromptforge: 0\n---\n\n\
        # Read H1\n\n\
        ask\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(reading);
    let (ctx, fixture) = h1_context(&prompt);
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("an explicit infer of H1 prose with no binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_prose_infers_explicitly_and_var_accumulates_into_the_walk() {
    // The pass reads its pending buffer only through an explicit infer, and
    // `var` writes accumulate across the pass into the walk.
    let gateway = ScriptedChat::new(vec![resp_text("final answer")]);
    let md = "---\nname: live-h1-prose\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1 Prose\n\n\
        ```lua\n\
        models.default('writer')\n\
        var.executions = (var.executions or 0) + 1\n\
        ```\n\n\
        Ask for one round.\n\n\
        ```lua\n\
        var.first = models.infer(prose)\n\
        var.executions = var.executions + 1\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\n\
        return var.first .. ':' .. var.executions\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("live H1 prose infers explicitly");

    assert_eq!(out, "final answer:2");
}
