//! H1 fence tests: live H1 blocks and the fence label rule.

use super::*;

const LABEL_NEAR_MISSES: [&str; 4] = ["lua extra", "Lua", "LUA", "luau"];

fn assert_unrecognized_label(src: &str, label: &str) {
    let error = parse(src).expect_err("a label near-miss must fail the parse");
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    assert_eq!(
        error.to_string(),
        format!("unrecognized fence label `{label}`; the only Lua fence label is `lua`")
    );
}

#[test]
fn unrecognized_lua_label_in_the_h1_fails_the_parse() {
    let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua extra\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
    assert_unrecognized_label(src, "lua extra");
}

#[test]
fn unrecognized_lua_label_in_a_section_fails_the_parse() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nProse.\n\n```lua extra\nlocal a = 1\n```\n";
    assert_unrecognized_label(src, "lua extra");
}

#[test]
fn unrecognized_lua_label_before_the_h1_fails_the_parse() {
    let src = "---\nname: x\ndescription: d\n---\n\nNotes.\n\n```lua extra\nlocal a = 1\n```\n\n# T\n\n## S\n\np\n";
    assert_unrecognized_label(src, "lua extra");
}

#[test]
fn the_first_unrecognized_label_is_the_one_reported() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```Lua\nlocal a = 1\n```\n\n```lua extra\nlocal b = 2\n```\n\n## S\n\np\n";
    assert_unrecognized_label(src, "Lua");
}

#[test]
fn h1_plain_lua_and_prose_are_live_blocks() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal first = 1\n```\n\nPlan {{ args }}.\n\n```lua\nstore.write('done', reply)\n```\n\n## S\n\np\n";
    let prompt = parse(src).expect("H1 blocks must parse");
    assert_eq!(prompt.h1_blocks.len(), 3);
    assert!(matches!(
        &prompt.h1_blocks[0],
        Block::Lua(program) if program.source() == "local first = 1"
    ));
    assert!(matches!(
        &prompt.h1_blocks[1],
        Block::Prose { text } if text == "Plan {{ args }}."
    ));
    assert!(matches!(
        &prompt.h1_blocks[2],
        Block::Lua(program) if program.source() == "store.write('done', reply)"
    ));
    assert_eq!(prompt.description_text, "Plan {{ args }}.");
}

#[test]
fn lone_plain_h1_lua_is_a_live_block() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal live = true\n```\n\n## S\n\np\n";
    let prompt = parse(src).expect("plain H1 Lua must parse");
    assert!(matches!(
        prompt.h1_blocks.as_slice(),
        [Block::Lua(program)] if program.source() == "local live = true"
    ));
}

#[test]
fn h1_label_near_misses_fail_whatever_the_opener_shape() {
    // Each opener shape here is one pulldown-cmark reports as a fence, so
    // the label alone decides, leading or after prose.
    for label in LABEL_NEAR_MISSES {
        for opener in [
            format!("```{label}\nreturn 1\n```"),
            format!("````{label}\nreturn 1\n````"),
            format!(" ```{label}\nreturn 1\n ```"),
        ] {
            for intro in ["", "Intro.\n\n"] {
                let src = format!(
                    "---\nname: x\ndescription: d\n---\n\n# T\n\n{intro}{opener}\n\n## S\n\np\n"
                );
                assert_unrecognized_label(&src, label);
            }
        }
    }

    let unclosed =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua extra\nreturn 1\n````\n\n## S\n\np\n";
    assert_unrecognized_label(unclosed, "lua extra");
}

fn assert_opener_violation(src: &str) {
    let Err(error) = parse(src) else {
        panic!("a non-exact Lua opener must fail the parse: {src:?}");
    };
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    assert_eq!(error.to_string(), OPENER_MESSAGE, "{src:?}");
}

#[test]
fn non_exact_lua_openers_fail_in_the_h1_and_the_preface() {
    for fence in NON_EXACT_LUA_OPENERS {
        for intro in ["", "Intro.\n\n"] {
            assert_opener_violation(&format!(
                "---\nname: x\ndescription: d\n---\n\n# T\n\n{intro}{fence}\n\n## S\n\np\n"
            ));
        }
        assert_opener_violation(&format!(
            "---\nname: x\ndescription: d\n---\n\nNotes.\n\n{fence}\n\n# T\n\n## S\n\np\n"
        ));
    }
}

#[test]
fn label_near_misses_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua extra\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua extra\nreturn 2\n```\n````\n";
    let prompt = parse(src).expect("nested label near-misses must remain prose");

    assert!(prompt.description_text.contains("```lua extra"));
    assert!(prompt.sections[0].prologue().is_none());
    assert!(prompt.sections[0].prose().contains("```lua extra"));
}

#[test]
fn unrecognized_lua_label_fails_before_any_lua_compiles() {
    let source = "private_payload =";
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua\nlocal ok = 1\n```\n\n## S\n\n```lua extra\n{source}\n```\n"
    );
    let (error, events) = Prompt::parse(&src, "parse-failure");
    let recorder = Recorder(events);
    let error = error.expect_err("the label near-miss must fail");
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    assert!(!error.to_string().contains(source), "{error}");
    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Prompt".into(), detail::PARSE_STARTED.to_string()),
            ("Prompt".into(), detail::PARSE_FAILED.to_string()),
        ]
    );
}

#[test]
fn non_exact_lua_opener_fails_before_any_lua_compiles() {
    let source = "private_payload =";
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua\nlocal ok = 1\n```\n\n## S\n\n~~~lua\n{source}\n~~~\n"
    );
    let (error, events) = Prompt::parse(&src, "parse-failure");
    let recorder = Recorder(events);
    let error = error.expect_err("the non-exact opener must fail");
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    assert!(!error.to_string().contains(source), "{error}");
    assert_eq!(
        recorder.observations(),
        vec![
            ("Prompt".into(), detail::PARSE_STARTED.to_string()),
            ("Prompt".into(), detail::PARSE_FAILED.to_string()),
        ]
    );
}

#[test]
fn successful_parse_reports_only_fixed_boundaries() {
    let source =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal secret = 42\n```\n\n## S\n\np\n";
    let (prompt, events) = Prompt::parse(source, "parse-success");
    prompt.expect("prompt must parse");
    let recorder = Recorder(events);
    assert!(
        recorder
            .records()
            .iter()
            .all(|(execution, _, _)| execution == "parse-success")
    );
    assert_eq!(
        recorder.observations(),
        vec![
            ("Prompt".into(), detail::PARSE_STARTED.to_string()),
            ("T".into(), detail::LUA_COMPILATION_STARTED.to_string()),
            ("T".into(), detail::LUA_COMPILATION_SUCCEEDED.to_string()),
            ("Prompt".into(), detail::PARSE_SUCCEEDED.to_string()),
        ]
    );
}
