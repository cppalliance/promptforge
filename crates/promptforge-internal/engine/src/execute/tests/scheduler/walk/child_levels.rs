//! Child-level walks: a jump into a child level starts that level's walk,
//! down to H4; a running child addresses only its own siblings and
//! children; child levels and call children take nested `sys.id` values,
//! the same on every run; and the fall-through walk never descends.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn jump_to_a_child_starts_the_child_level_walk() {
    // A jump to an H3 child starts a child-level walk at the target, which
    // falls through to the target's following siblings; when the level
    // exhausts, the parent walk resumes after the jumper.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Descend\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("a jump to a child must start the child-level walk");

    assert_eq!(out, "A\nX\nY\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn child_walk_recurses_to_h4() {
    // The child-level rule recurses - a jump from an H3 child to an H4
    // grandchild starts an H4-level walk, and each level's exhaustion
    // resumes its parent after the jumper.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Recurse\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\n\
        store.append('order.txt', 'X\\n')\n\
        jump('#### P')\n\
        ```\n\n\
        #### P\n\n\
        ```lua\nstore.append('order.txt', 'P\\n')\n```\n\n\
        #### Q\n\n\
        ```lua\nstore.append('order.txt', 'Q\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("the child-level rule must recurse to H4");

    assert_eq!(out, "A\nX\nP\nQ\nY\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_an_off_walk_child_runs_it() {
    // An off-walk child stays addressable - a jump to it runs it, and the
    // fall-through that follows skips nothing addressed.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # OffChild\n\n\
        ## A\n\n\
        ```lua\njump('### Off')\n```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Off\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'Off\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("a jump to an off-walk child must run it");

    assert_eq!(out, "Off\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn running_child_addresses_its_own_siblings_and_children() {
    // A running child's visible set is its own siblings plus its own
    // children - it can execute a child and jump to a sibling.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Visible\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\n\
        local r = call('#### Grand')\n\
        store.append('order.txt', 'X:' .. r .. '\\n')\n\
        jump('### Y')\n\
        ```\n\n\
        #### Grand\n\n\
        ```lua\nreturn 'grand-ran'\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("a running child must address its own siblings and children");

    assert_eq!(out, "X:grand-ran\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn running_child_cannot_address_a_top_level_section() {
    // A running child cannot address a top-level section - the parent level
    // is not in its visible set, so the jump resolves as not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Escape\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect_err("a child jumping to a top-level section must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a top-level section is not in a child's visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_a_niece_errors() {
    // A sibling's child (a niece or nephew) is not in the visible set, so
    // the jump resolves as not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Niece\n\n\
        ## A\n\n\
        ```lua\njump('### Niece')\n```\n\n\
        ## B\n\n\
        ### Niece\n\n\
        ```lua\nreturn 'niece-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect_err("a jump to a niece must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a niece is not in the visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sys_id_counts_the_sections_one_chain_enters_across_a_jump_into_a_child_level() {
    // `sys.id` counts the sections the walk chain has entered - the detour
    // into a child level is the same chain, so it continues the count
    // rather than restarting it.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Ids\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\nstore.append('ids.txt', tostring(sys.id) .. '\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('ids.txt', tostring(sys.id) .. '\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
        return store.read('ids.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("sys.id must count the sections the one chain enters");

    assert_eq!(out, "0.1\n0.2\n0.3\n0.4\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_child_takes_ids_nested_under_its_own_chain_distinct_from_the_parent() {
    // Hierarchical identity: the parent walk is the root chain `0`, so its
    // entries are `0.N`; a `call` child is the root's first child chain
    // `0.0`, so its entries are `0.0.N`. The two never collide because
    // they are different chains, and a fanout inside the child nests its
    // arms under the child's chain id, not the root's.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Identity\n\n\
        ## Main\n\n\
        ```lua\n\
        store.append('ids.txt', 'main:' .. sys.id .. '\\n')\n\
        call('## Sub')\n\
        store.append('ids.txt', 'after:' .. sys.id .. '\\n')\n\
        return store.read('ids.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        store.append('ids.txt', 'sub:' .. sys.id .. '\\n')\n\
        local r = fanout('## Worker', {'a', 'b'})\n\
        store.append('ids.txt', r[1].text .. '\\n' .. r[2].text .. '\\n')\n\
        return 'sub-done'\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        return 'arm' .. sys.index .. ':' .. sys.id\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("the call child and its fanout complete");

    assert_eq!(
        out, "main:0.1\nsub:0.0.0\narm1:0.0.0.0\narm2:0.0.1.0\nafter:0.1\n",
        "parent entries are `0.N`, the call child's are `0.0.N`, and the \
         child's fanout arms nest under `0.0`"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_runs_of_the_same_prompt_produce_identical_ids() {
    // No run-global counter: every id is a path of chain-local counters,
    // so two runs of one prompt allocate the same ids for the walk, a call
    // child, a fanout's arms, and a section entered after both.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Identity\n\n\
        ## Main\n\n\
        ```lua\n\
        local ids = { sys.id }\n\
        ids[#ids + 1] = call('## Sub')\n\
        local r = fanout('## Worker', {'x', 'y', 'z'})\n\
        for i = 1, #r do ids[#ids + 1] = r[i].text end\n\
        ids[#ids + 1] = call('## Sub')\n\
        var.ids = table.concat(ids, ',')\n\
        ```\n\n\
        ## Last\n\n\
        ```lua\n\
        return var.ids .. ',' .. sys.id\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        return sys.id\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        return sys.id\n\
        ```\n";
    let prompt = parse(md);
    let mut outputs = Vec::new();
    for _ in 0..2 {
        let (ctx, harness) = scheduler_context(&prompt);
        let out = TokioDriver::new(&ctx, harness, None)
            .drive()
            .await
            .expect("the identity prompt completes");
        outputs.push(out);
    }

    assert_eq!(outputs[0], outputs[1], "two runs allocate the same ids");
    assert_eq!(
        outputs[0], "0.1,0.0.0,0.1.0,0.2.0,0.3.0,0.4.0,0.2",
        "the walk's entries are `0.N`, and call children and fanout arms \
         share the root's child counter in dispatch order"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn walk_never_descends_into_children() {
    // The walk never descends - a section's children do not run unless
    // addressed. This is the negative half of the child-descent rule: a
    // fall-through that descended would run the child and trip its error.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # NoDescent\n\n\
        ## A\n\n\
        ```lua\nstore.append('order.txt', 'A\\n')\n```\n\n\
        ### Child\n\n\
        ```lua\nerror('a child must not run by fall-through')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, harness) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, harness, None)
        .drive()
        .await
        .expect("the walk must never descend into children");

    assert_eq!(out, "A\nB\n");
}
