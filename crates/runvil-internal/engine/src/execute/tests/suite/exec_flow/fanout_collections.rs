//! `fanout`'s collection and worker arguments: the results it returns,
//! the members each arm receives, the workers it resolves, and the
//! arguments it refuses.

use super::*;

/// The prompt whose `## Parent` runs `parent` beside a `### Worker` that
/// returns its `item`.
fn item_worker(parent: &str) -> String {
    prompt("", &[("Parent", parent), ("### Worker", "return item")])
}

#[test]
fn fanout_returns_structured_results() {
    let source = PromptSource::new()
        .section(
            "Parent",
            "
            local r = tasks.fanout('### Worker', prompt.list('### Items'))
            assert(r[1].text == 'alpha-1')
            assert(r[1].ok == true)
            assert(r[1].item == 'alpha')
            assert(r[1].exhausted == false)
            assert(r[2].text == 'beta-2')
            assert(r[2].ok == true)
            assert(r[2].item == 'beta')
            assert(r[2].exhausted == false)
            assert(tostring(r[1]) == r[1].text)
            assert(table.concat(r, ',') == 'alpha-1,beta-2')
            return 'ok'",
        )
        .prose(
            "### Worker",
            "
            ```lua
            return item .. '-' .. sys.index
            ```

            Do work.",
        )
        .prose("### Items", "- alpha\n- beta");
    assert_eq!(Scenario::new(source.build()).run().text(), "ok");
}

/// An off-walk child section still runs as a fanout worker.
#[test]
fn off_walk_worker_runs_as_a_fanout_arm() {
    let source = PromptSource::new()
        .section(
            "Parent",
            "
            local r = tasks.fanout('### Worker', prompt.list('### Items'))
            return r[1].text .. ',' .. r[2].text",
        )
        .prose("### Worker", "---\n\n```lua\nreturn item .. '-done'\n```")
        .prose("### Items", "- alpha\n- beta");
    let trace = Scenario::new(source.build()).run();
    assert_eq!(trace.text(), "alpha-done,beta-done");
}

/// A 1025-member collection runs through the prompt-level path: the
/// parent's Lua builds the table, `collection_to_items` converts it, and
/// `fanout` dispatches every member.
/// The worker is pure Lua with an immediate return, so the run works
/// without a client and stays fast.
#[test]
fn fanout_accepts_a_prompt_built_collection_over_the_old_default_cap() {
    let source = item_worker(
        "
        local members = {}
        for i = 1, 1025 do members[i] = 'm' .. i end
        local r = tasks.fanout('### Worker', members)
        assert(#r == 1025, 'every member ran')
        return r[1025].text",
    );
    assert_eq!(Scenario::new(source).run().text(), "m1025");
}

/// Array members arrive as themselves, in collection order: a string stays a
/// string, a number a number, a boolean a boolean, a nested table a table.
#[test]
fn fanout_collection_array_members_arrive_as_themselves_in_order() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                local r = tasks.fanout('### Worker', {'b', 2, true, {nested='x'}})
                assert(#r == 4)
                assert(r[1].text == 'string:b')
                assert(r[2].text == 'number:2')
                assert(r[3].text == 'boolean:true')
                assert(r[4].text == 'table:x')
                return 'ok'",
            ),
            (
                "### Worker",
                "
                if type(item) == 'table' then return 'table:' .. item.nested end
                return type(item) .. ':' .. tostring(item)",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

/// Hash members arrive as pair tables (`item.key` / `item.value`), and
/// `.item` on each arm result returns the same pair table.
#[test]
fn fanout_collection_hash_members_arrive_as_pair_tables() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                local r = tasks.fanout('### Worker', {alpha=1, beta='two'})
                assert(#r == 2)
                local seen = {}
                for i = 1, #r do
                  assert(type(r[i].item) == 'table')
                  assert(r[i].item.key ~= nil and r[i].item.value ~= nil)
                  seen[r[i].text] = true
                end
                assert(seen['alpha=1'] and seen['beta=two'])
                return 'ok'",
            ),
            (
                "### Worker",
                "return item.key .. '=' .. tostring(item.value)",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

/// An empty collection is an error: no work is likely a bug (a list section
/// that parsed empty, a wrong variable), so the fanout is rejected before
/// any scheduling rather than returning zero results.
#[test]
fn fanout_collection_empty_errors() {
    let trace = Scenario::new(item_worker("tasks.fanout('### Worker', {})")).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("empty collection"),
        "error was: {rendered}"
    );
}

/// `.item` on each arm result returns the member value as a Lua value.
#[test]
fn fanout_collection_item_round_trips_members() {
    let source = prompt(
        "",
        &[
            (
                "Parent",
                "
                local r = tasks.fanout('### Worker', {1, 'two', {n=3}})
                assert(r[1].item == 1)
                assert(r[2].item == 'two')
                assert(type(r[3].item) == 'table' and r[3].item.n == 3)
                return 'ok'",
            ),
            ("### Worker", "return 'done'"),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

/// A worker resolves as a sibling: a top-level worker marked off-walk is
/// shared infrastructure the walk never visits.
#[test]
fn fanout_worker_resolves_as_a_sibling() {
    let source = PromptSource::new()
        .section(
            "Main",
            "
            local r = tasks.fanout('## Worker', {'x'})
            return r[1].text",
        )
        .prose("Worker", "---\n\n```lua\nreturn item .. '-done'\n```");
    assert_eq!(Scenario::new(source.build()).run().text(), "x-done");
}

/// One off-walk sibling worker is shared by two sibling callers.
#[test]
fn fanout_worker_shared_by_two_sibling_callers() {
    let source = PromptSource::new()
        .section(
            "A",
            "
            local r = tasks.fanout('## Worker', {'a'})
            store.write('a.txt', r[1].text)",
        )
        .section(
            "B",
            "
            local r = tasks.fanout('## Worker', {'b'})
            return store.read('a.txt') .. ',' .. r[1].text",
        )
        .prose("Worker", "---\n\n```lua\nreturn item .. '-done'\n```");
    assert_eq!(Scenario::new(source.build()).run().text(), "a-done,b-done");
}

/// A sibling's child (a niece) is outside the caller's visible set and
/// resolves as not-found.
#[test]
fn fanout_niece_worker_is_not_visible() {
    let source = prompt(
        "",
        &[
            ("Main", "tasks.fanout('### Niece', {'x'})"),
            ("Other", "local x = 1"),
            ("### Niece", "return item"),
        ],
    );
    let trace = Scenario::new(source).run();
    let error = trace.error();
    assert!(
        error.to_string().contains("not found"),
        "error was: {error}"
    );
}

/// The two-string form errors at the boundary, pointing at `prompt.list`.
#[test]
fn fanout_string_second_parameter_errors_pointing_at_prompt_list() {
    let source = PromptSource::new()
        .section("Parent", "tasks.fanout('### Worker', '### Items')")
        .section("### Worker", "return item")
        .prose("### Items", "- alpha");
    let trace = Scenario::new(source.build()).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("tasks.fanout's second parameter is a collection"),
        "error was: {rendered}"
    );
    assert!(
        rendered.contains("prompt.list(heading)"),
        "the error must point at prompt.list: {rendered}"
    );
}

/// A number or boolean second parameter is not a collection.
#[test]
fn fanout_number_and_boolean_second_parameters_error() {
    let source = item_worker(
        "
        local ok_n, err_n = pcall(tasks.fanout, '### Worker', 5)
        assert(not ok_n and tostring(err_n):find('collection'), tostring(err_n))
        local ok_b, err_b = pcall(tasks.fanout, '### Worker', true)
        assert(not ok_b and tostring(err_b):find('collection'), tostring(err_b))
        return 'ok'",
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

/// A function member cannot cross into an arm; the error names its index.
#[test]
fn fanout_function_member_errors_naming_the_index() {
    let source = item_worker("tasks.fanout('### Worker', {'a', function() end})");
    let trace = Scenario::new(source).run();
    let rendered = trace.error().to_string();
    assert!(rendered.contains("index 2"), "error was: {rendered}");
    assert!(rendered.contains("function"), "error was: {rendered}");
}

/// A non-scalar (table) key cannot be represented; the error says so.
#[test]
fn fanout_table_keyed_member_errors() {
    let source = item_worker(
        "
        local t = {}
        t[{}] = 'x'
        tasks.fanout('### Worker', t)",
    );
    let trace = Scenario::new(source).run();
    let error = trace.error();
    assert!(
        error
            .to_string()
            .contains("key must be a string, number, or boolean"),
        "error was: {error}"
    );
}

/// A list section is not a worker template; naming one as the worker errors.
#[test]
fn fanout_worker_that_is_a_list_section_errors() {
    let source = PromptSource::new()
        .section("Parent", "tasks.fanout('### Items', {'x'})")
        .prose("### Items", "- a\n- b");
    let trace = Scenario::new(source.build()).run();
    let error = trace.error();
    assert!(
        error
            .to_string()
            .contains("is a list section, not a worker template"),
        "error was: {error}"
    );
}
