//! Lazy `prose` behavior and section-boundary handoff: the pending
//! Markdown buffer installs as a fresh read-only lazy `prose` template
//! before each Lua coroutine starts, every `{{ }}` substitution evaluates
//! once on the first runtime read and memoizes, unconsumed buffers discard
//! at section end, and no `reply` global or automatic result handoff
//! crosses a section boundary.

use runvil_parser::test_support::{PromptSource, one_section, prompt};

use crate::test_support::scenario::Scenario;

/// The prompt whose one section `Only` holds the Markdown `body`.
fn only(body: &str) -> String {
    PromptSource::new().prose("Only", body).build()
}

#[test]
fn prose_snapshots_section_state_at_the_first_read() {
    // The template's `{{ var.word }}` resolves when Lua first reads
    // `prose`, not when the buffer installs: the mutation before the read
    // is visible in the rendered string.
    let source = only(
        "
        The word is {{ var.word }}.

        ```lua
        var.word = 'mutated'
        return prose
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "The word is mutated.");
}

#[test]
fn prose_memoizes_after_the_first_read() {
    // One evaluation per prose-Lua pair: a mutation after the first read
    // never reaches the memoized string.
    let source = only(
        "
        The word is {{ var.word }}.

        ```lua
        var.word = 'one'
        local first = prose
        var.word = 'two'
        assert(prose == first, 'a later read returns the memoized string')
        return prose
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "The word is one.");
}

#[test]
fn unread_prose_never_evaluates_and_never_errors() {
    // The template contains an unclosed `{{` and a missing key; a block
    // that never reads `prose` runs clean because neither is evaluated.
    let source = only(
        "
        An unclosed {{ placeholder and a {{ var.missing }} key.

        ```lua
        return 'ok'
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn a_second_prose_lua_pair_evaluates_fresh() {
    // Every prose-Lua pair owns a separate lazy value: the second pair's
    // read renders its own buffer against the state at its first read,
    // while the first pair's memoized string survives in `var`.
    let source = only(
        "
        First: {{ var.word }}.

        ```lua
        var.word = 'one'
        var.first = prose
        ```

        Second: {{ var.word }}.

        ```lua
        var.word = 'two'
        assert(var.first == 'First: one.', 'the first pair keeps its memo')
        assert(prose == 'Second: two.', 'the second pair renders fresh')
        return prose
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "Second: two.");
}

#[test]
fn prose_assignment_is_rejected() {
    let source = only(
        "
        Some template.

        ```lua
        local ok, err = pcall(function() prose = 'x' end)
        assert(not ok, 'assigning prose must fail')
        assert(tostring(err):match('read%-only'), 'the error names the read-only contract: ' .. tostring(err))
        return 'ok'
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn recursive_prose_reference_is_rejected_at_the_read_site() {
    let source = only(
        "
        Recursion: {{ prose }}.

        ```lua
        local ok, err = pcall(function() return prose end)
        assert(not ok, 'a recursive {{ prose }} must fail')
        assert(tostring(err):match('prose'), 'the error names prose: ' .. tostring(err))
        return 'ok'
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn pcall_catches_a_read_site_substitution_error() {
    let source = only(
        "
        Missing: {{ var.missing }}.

        ```lua
        local ok, err = pcall(function() return prose end)
        assert(not ok, 'the read must fail')
        assert(tostring(err):match('missing'), 'the substitution error surfaces: ' .. tostring(err))
        return 'caught'
        ```",
    );
    assert_eq!(Scenario::new(source).run().text(), "caught");
}

#[test]
fn an_uncaught_read_error_fails_the_run() {
    let source = only(
        "
        Missing: {{ var.missing }}.

        ```lua
        return prose
        ```",
    );
    let trace = Scenario::new(source).run();
    let rendered = trace.error().to_string();
    assert!(
        rendered.contains("missing {{ var.missing }}"),
        "the substitution error reaches the run failure: {rendered}"
    );
}

#[test]
fn unconsumed_prose_is_discarded_at_section_end() {
    // The first section's buffer is never followed by a Lua fence; it
    // discards without evaluation, so its bad placeholder cannot fail.
    let source = PromptSource::new()
        .prose("First", "{{ var.missing }} is never read here.")
        .section("Second", "return 'ok'");
    assert_eq!(Scenario::new(source.build()).run().text(), "ok");
}

#[test]
fn trailing_prose_after_the_last_fence_is_inert() {
    let source = only(
        "
        ```lua
        return 'ok'
        ```

        Trailing {{ var.missing }} commentary.",
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn prose_with_no_pending_markdown_renders_empty() {
    let source = one_section(
        "
        assert(prose == '', 'no pending markdown renders as empty')
        return 'ok'",
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn the_reply_register_is_gone_and_nothing_hands_off_at_fall_through() {
    // `reply` is an ordinary global: a write dies with the section VM,
    // and the next section sees nil - no register, no roll-forward.
    let source = prompt(
        "",
        &[
            ("First", "reply = 'custom'"),
            (
                "Second",
                "
                assert(reply == nil, 'no reply register crosses a section boundary')
                return 'ok'",
            ),
        ],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn lazy_prose_composes_with_a_library_metatable() {
    // A library's `_G` metatable keeps working beside the prose guard:
    // its `__newindex` still captures ordinary globals, and `prose` stays
    // read-only and lazy.
    let source = PromptSource::new()
        .prose(
            "Only",
            "
            Hello {{ var.word }}.

            ```lua
            require '## capture'
            var.word = 'world'
            plain = 'x'
            assert(captured.plain == 'x', 'the library __newindex still captures')
            local ok = pcall(function() prose = 'y' end)
            assert(not ok, 'prose stays read-only over a library metatable')
            return prose
            ```",
        )
        .section(
            "capture",
            "
            captured = {}
            setmetatable(_G, { __newindex = function(_, k, v) captured[k] = v end })",
        );
    assert_eq!(Scenario::new(source.build()).run().text(), "Hello world.");
}
