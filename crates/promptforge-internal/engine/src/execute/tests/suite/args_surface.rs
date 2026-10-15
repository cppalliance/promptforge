//! The args/argv surface: `args` is the exact passed string always; `argv`
//! is the parsed JSON on success and nil otherwise (`if argv then` is the
//! malformed check); a default-declared prompt wraps interface prose into
//! `argv.prose`; `argv` is writable in H1 only and frozen when H1 completes,
//! so an H1 repair reaches every downstream section while an H2 assignment
//! is an error; and `{{ argv }}` joins the prose substitution namespaces.

use promptforge_parser::test_support::{PromptSource, prompt};

use crate::test_support::scenario::Scenario;

/// One declared (required) `query` string. The declaration advertises and
/// documents; it never enforces - enforcement is the prompt's H1.
const QUERY: &str = "args: { query: { type: string } }";

/// A prompt declaring [`QUERY`] whose one section `Only` runs `lua`.
fn query_prompt(lua: &str) -> String {
    prompt(QUERY, &[("Only", lua)])
}

#[test]
fn args_is_the_exact_passed_string_and_substitutes_unmodified() {
    let source = PromptSource::new().front(QUERY).prose(
        "Only",
        "
        Args: {{ args }}

        ```lua
        assert(args == '  spaced { not json ', 'args is the exact passed string')
        return prose
        ```",
    );
    let trace = Scenario::new(source.build())
        .args("  spaced { not json ")
        .run();
    let out = trace.text();
    assert!(
        out.contains("Args:   spaced { not json "),
        "{{ args }} renders the raw string unmodified: {out:?}"
    );
}

#[test]
fn argv_is_the_parsed_json_on_success() {
    let source = query_prompt(
        "
        assert(argv, 'parsed JSON makes argv present')
        assert(argv.query == 'papers', 'structured access is argv.query')
        return argv.query",
    );
    let trace = Scenario::new(source).args("{\"query\":\"papers\"}").run();
    assert_eq!(trace.text(), "papers");
}

#[test]
fn argv_is_nil_on_malformed_json() {
    // `if argv then` is the idiomatic malformed check.
    let source = query_prompt(
        "
        if argv then error('malformed JSON must read as nil argv') end
        return 'nil'",
    );
    let trace = Scenario::new(source).args("not json").run();
    assert_eq!(trace.text(), "nil", "malformed JSON is nil argv");
}

#[test]
fn valid_json_scalars_make_argv_a_number_or_boolean_and_null_reads_as_nil() {
    let source = query_prompt("return tostring(argv)");
    for (args, rendered) in [("42", "42"), ("true", "true")] {
        let trace = Scenario::new(source.clone()).args(args).run();
        assert_eq!(
            trace.text(),
            rendered,
            "a valid JSON {args} makes argv a scalar"
        );
    }
    let source = query_prompt(
        "
        if argv then error('JSON null must read as nil') end
        return 'nil'",
    );
    assert_eq!(Scenario::new(source).args("null").run().text(), "nil");
}

#[test]
fn the_executor_never_hard_errors_on_shape() {
    // `query` is declared a string and arrives as a number: the run
    // succeeds; shape enforcement belongs to the prompt's H1.
    let source = query_prompt("return tostring(argv.query)");
    let trace = Scenario::new(source).args("{\"query\":5}").run();
    assert_eq!(trace.text(), "5");
}

#[test]
fn a_strict_h1_errors_on_a_missing_field() {
    let source = PromptSource::new()
        .front(QUERY)
        .h1("assert(argv and argv.query, 'query is required')")
        .section("Only", "return 'unreachable'");
    let trace = Scenario::new(source.build()).args("{}").run();
    let error = trace.error();
    assert!(
        error.to_string().contains("query is required"),
        "the assertion notice surfaces: {error}"
    );
}

#[test]
fn an_h1_repair_is_visible_to_every_downstream_section() {
    let source = PromptSource::new()
        .front(QUERY)
        .h1("
            assert(argv == nil, 'the broken input starts as nil argv')
            argv = { query = 'repaired' }")
        .prose(
            "First",
            "
            Query: {{ argv.query }}

            ```lua
            assert(argv.query == 'repaired', 'the repair reaches the first section')
            var.from_first = prose
            ```",
        )
        .section(
            "Second",
            "
            assert(argv.query == 'repaired', 'the repair reaches every downstream section')
            assert(var.from_first == 'Query: repaired', 'the repair reaches substitution')
            return argv.query",
        );
    let trace = Scenario::new(source.build()).args("broken json").run();
    assert_eq!(trace.text(), "repaired");
}

#[test]
fn assigning_argv_in_an_h2_section_is_an_error() {
    let source = query_prompt("argv = { query = 'hijacked' }");
    let trace = Scenario::new(source).args("{\"query\":\"x\"}").run();
    let error = trace.error();
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
}

#[test]
fn an_h2_field_write_on_argv_is_an_error() {
    let source = query_prompt("argv.query = 'hijacked'");
    let trace = Scenario::new(source).args("{\"query\":\"x\"}").run();
    let error = trace.error();
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
}

#[test]
fn absent_is_not_the_empty_string() {
    // An optional field omitted is nil; passed empty it is "". The two are
    // distinguishable in Lua.
    let source = prompt(
        "args: { prose: { type: string, optional: true } }",
        &[(
            "Only",
            "
            if argv.prose == nil then return 'absent' end
            assert(argv.prose == '', 'present and empty is the empty string')
            return 'empty'",
        )],
    );
    let omitted = Scenario::new(source.clone()).args("{}").run();
    assert_eq!(omitted.text(), "absent");
    let empty = Scenario::new(source).args("{\"prose\":\"\"}").run();
    assert_eq!(empty.text(), "empty");
}

#[test]
fn a_default_declared_prompt_wraps_interface_prose() {
    // No `args:` key: the default declaration wraps prose into argv.prose,
    // and args still holds the exact passed string.
    let source = prompt(
        "",
        &[(
            "Only",
            "
            assert(argv and argv.prose == 'hello there', 'prose wraps into argv.prose')
            assert(args == 'hello there', 'args holds the exact passed string')
            return argv.prose",
        )],
    );
    let trace = Scenario::new(source).args("hello there").run();
    assert_eq!(trace.text(), "hello there");
}

#[test]
fn a_default_declared_prompt_wraps_empty_prose_as_present() {
    // Interface prose is always present, so empty prose wraps as the
    // present empty string, distinguishable from an absent field.
    let source = prompt(
        "",
        &[(
            "Only",
            "
            assert(argv, 'empty prose still wraps')
            assert(argv.prose ~= nil, 'the field is present')
            assert(argv.prose == '', 'present and empty is the empty string')
            return 'ok'",
        )],
    );
    assert_eq!(Scenario::new(source).run().text(), "ok");
}

#[test]
fn argv_joins_the_substitution_namespaces() {
    let source = PromptSource::new().front(QUERY).prose(
        "Only",
        "
        Whole: {{ argv }}; Field: {{ argv.query }}

        ```lua
        return prose
        ```",
    );
    let trace = Scenario::new(source.build())
        .args("{\"query\":\"papers\",\"n\":2}")
        .run();
    assert_eq!(
        trace.text(),
        "Whole: {\"n\":2,\"query\":\"papers\"}; Field: papers"
    );
}

#[test]
fn dotted_indexing_into_a_scalar_argv_is_a_catchable_substitution_error() {
    let source = PromptSource::new().front(QUERY).prose(
        "Only",
        "
        Value: {{ argv.query.x }}

        ```lua
        local ok, err = pcall(function() return prose end)
        assert(not ok, 'dotted indexing into a scalar must fail')
        assert(tostring(err):match('missing'), 'a substitution error, never a silent empty string: ' .. tostring(err))
        return 'caught'
        ```",
    );
    let trace = Scenario::new(source.build())
        .args("{\"query\":\"scalar\"}")
        .run();
    assert_eq!(
        trace.text(),
        "caught",
        "pcall catches the substitution error"
    );
}

#[test]
fn substituting_a_nil_argv_is_an_error() {
    let source = PromptSource::new().front(QUERY).prose(
        "Only",
        "
        Value: {{ argv }}

        ```lua
        return prose
        ```",
    );
    let trace = Scenario::new(source.build()).args("not json").run();
    let error = trace.error();
    assert!(
        error.to_string().contains("argv"),
        "a nil argv must not render as a silent empty string: {error}"
    );
}

#[test]
fn a_call_chain_derives_argv_from_its_own_args_frozen() {
    let source = prompt(
        QUERY,
        &[
            ("Main", "return call('## Sub', '{\"query\":\"chain\"}')"),
            (
                "Sub",
                "
                assert(args == '{\"query\":\"chain\"}', 'the chain sees its own args')
                assert(argv.query == 'chain', 'argv derives from the chain args')
                local wrote = pcall(function() argv.query = 'x' end)
                assert(not wrote, 'the chain argv is frozen')
                return argv.query",
            ),
        ],
    );
    let trace = Scenario::new(source).args("{\"query\":\"run\"}").run();
    assert_eq!(trace.text(), "chain");
}

#[test]
fn a_no_input_call_inherits_the_repaired_argv() {
    let source = PromptSource::new()
        .front(QUERY)
        .h1("argv = { query = 'repaired' }")
        .section("Main", "return call('## Sub')")
        .section(
            "Sub",
            "
            assert(argv.query == 'repaired', 'a no-input call inherits the run argv')
            return argv.query",
        );
    let trace = Scenario::new(source.build()).args("broken").run();
    assert_eq!(trace.text(), "repaired");
}
