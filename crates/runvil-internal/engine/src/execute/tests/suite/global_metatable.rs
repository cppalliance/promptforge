//! An author's own metatable on `_G`: it keeps working for every other
//! global (defaults, strict errors, write hooks) in the H1 pass and in
//! later sections, set by a required library and across fences, while
//! `argv` and `prose` stay guarded whatever author code does to `_G`'s
//! metatable.

use runvil_parser::test_support::PromptSource;

use crate::test_support::scenario::Scenario;

/// One declared `query` string, so `argv` is the parsed JSON or nil.
fn query_source() -> PromptSource {
    PromptSource::new().front("args: { query: { type: string } }")
}

#[test]
fn a_library_defaults_metatable_works_beside_frozen_argv_and_read_only_prose() {
    let source = query_source()
        .h1(
            "
            require '## defaults'
            assert(tone == 'friendly', 'H1 reads the default')
            argv = { query = 'repaired' }
            assert(not pcall(function() prose = 'x' end), 'prose is read-only in H1')",
        )
        .prose(
            "First",
            "
            Tone: {{ tone }}.

            ```lua
            require '## defaults'
            assert(prose == 'Tone: friendly.', 'a default renders in prose: ' .. prose)
            assert(missing == nil, 'a global with no default reads nil')
            local ok, err = pcall(function() argv = 'hijacked' end)
            assert(not ok and tostring(err):find('argv is frozen outside H1: assign it in H1 only', 1, true), 'argv stays frozen: ' .. tostring(err))
            var.n = 2
            ```

            Second fence {{ var.n }}.

            ```lua
            assert(prose == 'Second fence 2.', 'the second fence renders fresh: ' .. prose)
            local ok, err = pcall(function() prose = 'x' end)
            assert(not ok and tostring(err):find('prose is read-only: assign to `var` or a section global instead', 1, true), 'prose stays read-only: ' .. tostring(err))
            assert(tone == 'friendly', 'the default survives the second fence')
            ```",
        )
        .section(
            "Second",
            "
            require '## defaults'
            assert(argv.query == 'repaired', 'the H1 repair reaches a later section')
            assert(not pcall(function() argv = 'hijacked' end), 'argv stays frozen in a later section')
            return argv.query .. ' ' .. tone",
        )
        .section(
            "defaults",
            "
            defaults = { tone = 'friendly' }
            setmetatable(_G, { __index = defaults })",
        );
    let trace = Scenario::new(source.build()).args("broken json").run();
    assert_eq!(trace.text(), "repaired friendly");
}

#[test]
fn a_strict_metatable_raises_its_own_error_while_the_guard_serves_argv_and_prose() {
    let source = query_source()
        .h1(
            "
            require '## strict'
            local ok, err = pcall(function() return undefined_in_h1 end)
            assert(not ok and tostring(err):find('undefined global undefined_in_h1', 1, true), 'H1 is strict: ' .. tostring(err))",
        )
        .prose(
            "Only",
            "
            Hi.

            ```lua
            require '## strict'
            local ok, err = pcall(function() return undefined_here end)
            assert(not ok, 'an undefined global raises')
            assert(type(err) == 'string', 'the author error comes back as raised, got ' .. type(err))
            assert(err:find(']:2: undefined global undefined_here', 1, true), 'the error names the reading line: ' .. err)
            assert(argv.query == 'x', 'the guard serves argv, not the strict handler')
            assert(prose == 'Hi.', 'the guard serves prose, not the strict handler')
            return 'strict'
            ```",
        )
        .section(
            "strict",
            "
            setmetatable(_G, {
              __index = function(_, key) error('undefined global ' .. key, 2) end,
            })",
        );
    let trace = Scenario::new(source.build())
        .args("{\"query\":\"x\"}")
        .run();
    assert_eq!(trace.text(), "strict");
}

#[test]
fn an_h1_repair_lands_under_a_strict_index_and_a_write_hook() {
    let source = query_source()
        .h1("
            require '## hooks'
            if argv == nil then argv = { query = 'repaired' } end
            assert(seen.argv == nil, 'the write hook never sees argv')")
        .section(
            "Only",
            "
            require '## hooks'
            assert(argv.query == 'repaired', 'the repair reaches the walk')
            return argv.query",
        )
        .section(
            "hooks",
            "
            seen = {}
            setmetatable(_G, {
              __index = function(_, key) error('undefined global ' .. key, 2) end,
              __newindex = function(_, key, value) seen[key] = value end,
            })",
        );
    let trace = Scenario::new(source.build()).args("broken json").run();
    assert_eq!(trace.text(), "repaired");
}

#[test]
fn setmetatable_on_g_in_a_block_frees_neither_argv_nor_prose() {
    let source = query_source().prose(
        "Only",
        "
        First.

        ```lua
        setmetatable(_G, nil)
        local a = pcall(function() argv = 'x' end)
        setmetatable(_G, {})
        local b = pcall(function() prose = 'x' end)
        captured = {}
        setmetatable(_G, { __newindex = function(_, k, v) captured[k] = v end })
        local c = pcall(function() argv = 'x' end)
        local d = pcall(function() prose = 'x' end)
        assert(not (a or b or c or d), 'every assignment is refused')
        assert(captured.argv == nil and captured.prose == nil, 'the author hook never sees argv or prose')
        plain = 'y'
        assert(captured.plain == 'y', 'the author hook sees other globals')
        assert(argv.query == 'x' and prose == 'First.', 'both still read as the Engine set them')
        ```

        Second.

        ```lua
        assert(prose == 'Second.', 'the next fence installs its prose under the author metatable')
        assert(getmetatable(_G).__newindex ~= nil, 'the author metatable survives the next fence')
        assert(not pcall(function() argv = 'x' end), 'argv is still frozen')
        return 'guarded'
        ```",
    );
    let trace = Scenario::new(source.build())
        .args("{\"query\":\"x\"}")
        .run();
    assert_eq!(trace.text(), "guarded");
}

#[test]
fn a_library_cannot_assign_prose() {
    let source = query_source()
        .prose(
            "Only",
            "
            The real prose.

            ```lua
            require '## lib'
            assert(library_prose == 'Library prose.', 'the library reads its own prose: ' .. tostring(library_prose))
            assert(library_refusal and library_refusal:find('prose is read-only', 1, true), 'the library cannot assign prose: ' .. tostring(library_refusal))
            return prose
            ```",
        )
        .prose(
            "lib",
            "
            Library prose.

            ```lua
            library_prose = prose
            local ok, err = pcall(function() prose = 'shadowed' end)
            library_refusal = not ok and tostring(err)
            ```",
        );
    let trace = Scenario::new(source.build())
        .args("{\"query\":\"x\"}")
        .run();
    assert_eq!(trace.text(), "The real prose.");
}

#[test]
fn getmetatable_on_g_returns_a_librarys_metatable_and_edits_apply_live() {
    let source = query_source()
        .section(
            "Only",
            "
            require '## metatable'
            assert(getmetatable(_G) == author_mt, 'getmetatable(_G) is the library metatable')
            getmetatable(_G).__index = function(_, key) return 'live ' .. key end
            assert(tone == 'live tone', 'an edit applies at once')
            getmetatable(_G).__newindex = function() end
            assert(not pcall(function() argv = 'x' end), 'an edit cannot free argv')
            return tone",
        )
        .section(
            "metatable",
            "
            author_mt = { __index = { tone = 'friendly' } }
            setmetatable(_G, author_mt)",
        );
    let trace = Scenario::new(source.build())
        .args("{\"query\":\"x\"}")
        .run();
    assert_eq!(trace.text(), "live tone");
}
