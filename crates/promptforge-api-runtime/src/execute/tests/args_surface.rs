//! The args/argv surface: `args` is the exact passed string always; `argv`
//! is the parsed JSON on success and nil otherwise (`if argv then` is the
//! malformed check); a default-declared prompt wraps interface prose into
//! `argv.prose`; `argv` is writable in H1 only and frozen when H1 completes,
//! so an H1 repair reaches every downstream section while an H2 assignment
//! is an error; and `{{ argv }}` joins the prose substitution namespaces.

use super::*;

/// The frontmatter every structured-args test shares: one declared
/// (required) `query` string. The declaration advertises and documents; it
/// never enforces - enforcement is the prompt's H1.
macro_rules! args_prompt {
    ($body:literal) => {
        concat!(
            "---\nname: t\ndescription: d\npromptforge: 0\n",
            "args:\n  query:\n    type: string\n",
            "---\n\n",
            $body
        )
    };
}

/// Runs a structured-args fixture offline with the given args string.
async fn run_args(md: &str, args: &str) -> Result<String> {
    run(&fixture(md), args, &[], &TestStore::new(), silent()).await
}

#[tokio::test]
async fn args_is_the_exact_passed_string_and_substitutes_unmodified() {
    let md = args_prompt!(
        "## Only\n\n\
Args: {{ args }}\n\n\
```lua\n\
assert(args == '  spaced { not json ', 'args is the exact passed string')\n\
return prose\n\
```\n"
    );
    let out = run_args(md, "  spaced { not json ")
        .await
        .expect("the run succeeds");
    assert!(
        out.contains("Args:   spaced { not json "),
        "{{ args }} renders the raw string unmodified: {out:?}"
    );
}

#[tokio::test]
async fn argv_is_the_parsed_json_on_success() {
    let md = args_prompt!(
        "## Only\n\n\
```lua\n\
assert(argv, 'parsed JSON makes argv present')\n\
assert(argv.query == 'papers', 'structured access is argv.query')\n\
return argv.query\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"papers\"}")
        .await
        .expect("the run succeeds");
    assert_eq!(out, "papers");
}

#[tokio::test]
async fn argv_is_nil_on_malformed_json() {
    // `if argv then` is the idiomatic malformed check.
    let md = args_prompt!(
        "## Only\n\n\
```lua\n\
if argv then error('malformed JSON must read as nil argv') end\n\
return 'nil'\n\
```\n"
    );
    let out = run_args(md, "not json")
        .await
        .expect("malformed JSON is nil argv, not a run failure");
    assert_eq!(out, "nil");
}

#[tokio::test]
async fn valid_json_scalars_make_argv_a_number_or_boolean_and_null_reads_as_nil() {
    let md = args_prompt!("## Only\n\n```lua\nreturn tostring(argv)\n```\n");
    assert_eq!(
        run_args(md, "42").await.expect("a number argv"),
        "42",
        "a valid JSON number makes argv a number"
    );
    assert_eq!(
        run_args(md, "true").await.expect("a boolean argv"),
        "true",
        "a valid JSON boolean makes argv a boolean"
    );
    let md = args_prompt!(
        "## Only\n\n\
```lua\n\
if argv then error('JSON null must read as nil') end\n\
return 'nil'\n\
```\n"
    );
    assert_eq!(run_args(md, "null").await.expect("null runs"), "nil");
}

#[tokio::test]
async fn the_executor_never_hard_errors_on_shape() {
    // `query` is declared a string and arrives as a number: the run
    // succeeds; shape enforcement belongs to the prompt's H1.
    let md = args_prompt!("## Only\n\n```lua\nreturn tostring(argv.query)\n```\n");
    let out = run_args(md, "{\"query\":5}")
        .await
        .expect("a shape mismatch is the prompt's concern, not the executor's");
    assert_eq!(out, "5");
}

#[tokio::test]
async fn a_strict_h1_errors_on_a_missing_field() {
    let md = args_prompt!(
        "# T\n\n\
```lua\n\
assert(argv and argv.query, 'query is required')\n\
```\n\n\
## Only\n\n\
```lua\nreturn 'unreachable'\n```\n"
    );
    let error = run_args(md, "{}")
        .await
        .expect_err("the strict H1 path fails the run before the walk");
    assert!(
        error.to_string().contains("query is required"),
        "the assertion notice surfaces: {error}"
    );
}

#[tokio::test]
async fn an_h1_repair_is_visible_to_every_downstream_section() {
    let md = args_prompt!(
        "# T\n\n\
```lua\n\
assert(argv == nil, 'the broken input starts as nil argv')\n\
argv = { query = 'repaired' }\n\
```\n\n\
## First\n\n\
Query: {{ argv.query }}\n\n\
```lua\n\
assert(argv.query == 'repaired', 'the repair reaches the first section')\n\
var.from_first = prose\n\
```\n\n\
## Second\n\n\
```lua\n\
assert(argv.query == 'repaired', 'the repair reaches every downstream section')\n\
assert(var.from_first == 'Query: repaired', 'the repair reaches substitution')\n\
return argv.query\n\
```\n"
    );
    let out = run_args(md, "broken json")
        .await
        .expect("the H1 repair reaches downstream");
    assert_eq!(out, "repaired");
}

#[tokio::test]
async fn assigning_argv_in_an_h2_section_is_an_error() {
    let md = args_prompt!(
        "## Only\n\n\
```lua\n\
argv = { query = 'hijacked' }\n\
```\n"
    );
    let error = run_args(md, "{\"query\":\"x\"}")
        .await
        .expect_err("an H2 argv assignment must fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
}

#[tokio::test]
async fn an_h2_field_write_on_argv_is_an_error() {
    let md = args_prompt!(
        "## Only\n\n\
```lua\n\
argv.query = 'hijacked'\n\
```\n"
    );
    let error = run_args(md, "{\"query\":\"x\"}")
        .await
        .expect_err("an H2 field write on argv must fail");
    assert!(
        error.to_string().contains("argv is frozen"),
        "the error names the freeze: {error}"
    );
}

#[tokio::test]
async fn absent_is_not_the_empty_string() {
    // An optional field omitted is nil; passed empty it is "". The two are
    // distinguishable in Lua.
    let md = concat!(
        "---\nname: t\ndescription: d\npromptforge: 0\n",
        "args:\n  prose:\n    type: string\n    optional: true\n",
        "---\n\n",
        "## Only\n\n\
```lua\n\
if argv.prose == nil then return 'absent' end\n\
assert(argv.prose == '', 'present and empty is the empty string')\n\
return 'empty'\n\
```\n"
    );
    assert_eq!(
        run_args(md, "{}").await.expect("an omitted field runs"),
        "absent"
    );
    assert_eq!(
        run_args(md, "{\"prose\":\"\"}")
            .await
            .expect("an empty field runs"),
        "empty"
    );
}

#[tokio::test]
async fn a_default_declared_prompt_wraps_interface_prose() {
    // No `args:` key: the default declaration wraps prose into argv.prose,
    // and args still holds the exact passed string.
    let md = concat!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
        "## Only\n\n\
```lua\n\
assert(argv and argv.prose == 'hello there', 'prose wraps into argv.prose')\n\
assert(args == 'hello there', 'args holds the exact passed string')\n\
return argv.prose\n\
```\n"
    );
    let out = run_args(md, "hello there").await.expect("the wrap runs");
    assert_eq!(out, "hello there");
}

#[tokio::test]
async fn a_default_declared_prompt_wraps_empty_prose_as_present() {
    // Interface prose is always present, so empty prose wraps as the
    // present empty string, distinguishable from an absent field.
    let md = concat!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n",
        "## Only\n\n\
```lua\n\
assert(argv, 'empty prose still wraps')\n\
assert(argv.prose ~= nil, 'the field is present')\n\
assert(argv.prose == '', 'present and empty is the empty string')\n\
return 'ok'\n\
```\n"
    );
    let out = run_offline(md).await.expect("empty prose wraps");
    assert_eq!(out, "ok");
}

#[tokio::test]
async fn argv_joins_the_substitution_namespaces() {
    let md = args_prompt!(
        "## Only\n\n\
Whole: {{ argv }}; Field: {{ argv.query }}\n\n\
```lua\nreturn prose\n```\n"
    );
    let out = run_args(md, "{\"query\":\"papers\",\"n\":2}")
        .await
        .expect("the substitution runs");
    assert_eq!(out, "Whole: {\"n\":2,\"query\":\"papers\"}; Field: papers");
}

#[tokio::test]
async fn dotted_indexing_into_a_scalar_argv_is_a_catchable_substitution_error() {
    let md = args_prompt!(
        "## Only\n\n\
Value: {{ argv.query.x }}\n\n\
```lua\n\
local ok, err = pcall(function() return prose end)\n\
assert(not ok, 'dotted indexing into a scalar must fail')\n\
assert(tostring(err):match('missing'), 'a substitution error, never a silent empty string: ' .. tostring(err))\n\
return 'caught'\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"scalar\"}")
        .await
        .expect("pcall catches the substitution error");
    assert_eq!(out, "caught");
}

#[tokio::test]
async fn substituting_a_nil_argv_is_an_error() {
    let md = args_prompt!(
        "## Only\n\n\
Value: {{ argv }}\n\n\
```lua\nreturn prose\n```\n"
    );
    let error = run_args(md, "not json")
        .await
        .expect_err("a nil argv must not render as a silent empty string");
    assert!(
        error.to_string().contains("argv"),
        "the error names argv: {error}"
    );
}

#[tokio::test]
async fn a_call_chain_derives_argv_from_its_own_args_frozen() {
    let md = args_prompt!(
        "## Main\n\n\
```lua\n\
return call('## Sub', '{\"query\":\"chain\"}')\n\
```\n\n\
## Sub\n\n\
```lua\n\
assert(args == '{\"query\":\"chain\"}', 'the chain sees its own args')\n\
assert(argv.query == 'chain', 'argv derives from the chain args')\n\
local wrote = pcall(function() argv.query = 'x' end)\n\
assert(not wrote, 'the chain argv is frozen')\n\
return argv.query\n\
```\n"
    );
    let out = run_args(md, "{\"query\":\"run\"}")
        .await
        .expect("the chain runs");
    assert_eq!(out, "chain");
}

#[tokio::test]
async fn a_no_input_call_inherits_the_repaired_argv() {
    let md = args_prompt!(
        "# T\n\n\
```lua\nargv = { query = 'repaired' }\n```\n\n\
## Main\n\n\
```lua\n\
return call('## Sub')\n\
```\n\n\
## Sub\n\n\
```lua\n\
assert(argv.query == 'repaired', 'a no-input call inherits the run argv')\n\
return argv.query\n\
```\n"
    );
    let out = run_args(md, "broken")
        .await
        .expect("the no-input call inherits the repaired argv");
    assert_eq!(out, "repaired");
}
