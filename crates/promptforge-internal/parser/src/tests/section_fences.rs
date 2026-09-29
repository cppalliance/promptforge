//! Section fence tests: exact `lua` fences around section prose and their
//! compilation reports.

use super::*;

#[test]
fn lua_fence_separated_from_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nreturn 42\n```\n\nActual prose here.\n";
    let p = parse(src).unwrap();
    assert_eq!(
        p.sections[0].prologue().map(LuaProgram::source),
        Some("return 42")
    );
    assert_eq!(p.sections[0].prose(), "Actual prose here.");
    assert!(p.sections[0].epilog().is_none());
}

#[test]
fn section_compiles_prologue_and_epilog_around_prose() {
    let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n## Transform\r\n\r\n \t\r\n```lua\r\nvar.before = args\r\n```\r\n\r\nAsk about {{ var.before }}.\r\n\r\n```lua\r\nreturn reply\r\n```\r\n";
    let prompt = parse(src).expect("both exact section phases must compile");
    let section = prompt.entry().expect("has sections");

    assert_eq!(
        section.prologue().map(LuaProgram::source),
        Some("var.before = args")
    );
    assert_eq!(section.prose(), "Ask about {{ var.before }}.");
    assert_eq!(
        section.epilog().map(LuaProgram::source),
        Some("return reply")
    );
}

#[test]
fn section_compiles_epilog_after_prose_without_prologue() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Transform\n\nAsk the model.\n\n```lua\nreturn reply\n```\n";
    let prompt = parse(src).expect("the trailing epilog must compile");
    let section = prompt.entry().expect("has sections");

    assert!(section.prologue().is_none());
    assert_eq!(section.prose(), "Ask the model.");
    assert_eq!(
        section.epilog().map(LuaProgram::source),
        Some("return reply")
    );
}

#[test]
fn exact_middle_lua_fences_become_compiled_blocks() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nBefore.\n\n```lua\nvar.mid = 1\n```\n\nAfter.\n";
    let prompt = parse(src).expect("middle Lua fences compile as blocks");
    let section = prompt.entry().expect("has sections");

    assert!(section.prologue().is_none());
    assert!(section.epilog().is_none());
    assert_eq!(section.prose(), "After.");
    assert_eq!(section.blocks.len(), 3);
    match &section.blocks[0] {
        Block::Prose { text } => assert_eq!(text, "Before."),
        other => panic!("expected leading prose, got {other:?}"),
    }
    match &section.blocks[1] {
        Block::Lua(program) => assert_eq!(program.source(), "var.mid = 1"),
        other => panic!("expected lua block, got {other:?}"),
    }
    match &section.blocks[2] {
        Block::Prose { text } => assert_eq!(text, "After."),
        other => panic!("expected trailing prose, got {other:?}"),
    }
}

#[test]
fn invalid_middle_lua_fence_fails_parse() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nBefore.\n\n```lua\nnot valid lua =\n```\n\nAfter.\n";
    let err = parse(src).expect_err("invalid middle Lua must fail compilation");
    assert_eq!(err.kind(), ParseErrorKind::Lua);
}

#[test]
fn one_exact_fence_is_the_prologue_and_two_can_surround_empty_prose() {
    let one = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n";
    let prompt = parse(one).expect("one fence is the prologue");
    let entry = prompt.entry().expect("has sections");
    assert!(entry.prologue().is_some());
    assert!(entry.epilog().is_none());

    let two = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n\n```lua\nreturn reply\n```\n";
    let prompt = parse(two).expect("two fences can enclose empty prose");
    let entry = prompt.entry().expect("has sections");
    assert_eq!(entry.prose(), "");
    assert!(entry.prologue().is_some());
    assert!(entry.epilog().is_some());
}

#[test]
fn section_fence_markers_must_be_exact() {
    for near_miss in [
        "````lua\nreturn 1\n````",
        " ```lua\nreturn 1\n ```",
        "```Lua\nreturn 1\n```",
        "```lua extra\nreturn 1\n```",
    ] {
        let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{near_miss}\n");
        let prompt = parse(&src).expect("near-miss fence must remain prose");
        let entry = prompt.entry().expect("has sections");
        assert!(entry.prologue().is_none());
        assert!(entry.epilog().is_none());
        assert_eq!(entry.prose(), near_miss.trim());
    }
}

#[test]
fn non_exact_section_closing_before_another_lua_fence_is_a_parse_error() {
    for near_miss_close in ["``` ", "  ```", "````"] {
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.a = 1\n{near_miss_close}\n\n```lua\nvar.b = 2\n```\n"
        );
        let error =
            parse(&src).expect_err("a near-miss closing fence must not panic or close the block");
        assert!(error.to_string().contains("not closed exactly"));
    }
}

#[test]
fn section_markers_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n````markdown\n```lua\nreturn 1\n```\n````\n";
    let prompt = parse(src).expect("nested markers must remain prose");

    let entry = prompt.entry().expect("has sections");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_none());
    assert!(entry.prose().contains("```lua"));
}

#[test]
fn malformed_section_phases_report_locations_and_safe_boundaries() {
    for (phase, content, expected_location, expected_details) in [
        (
            "prologue",
            "```lua\nprivate_payload =\n```\n\nProse.",
            "section `Private section` prologue",
            vec![
                detail::PARSE_STARTED,
                detail::LUA_COMPILATION_STARTED,
                detail::LUA_COMPILATION_FAILED,
                detail::PARSE_FAILED,
            ],
        ),
        (
            "epilog",
            "Prose.\n\n```lua\nprivate_payload =\n```",
            "section `Private section` epilog",
            vec![
                detail::PARSE_STARTED,
                detail::LUA_COMPILATION_STARTED,
                detail::LUA_COMPILATION_FAILED,
                detail::PARSE_FAILED,
            ],
        ),
    ] {
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## Private section\n\n{content}\n"
        );
        let (outcome, events) = Prompt::parse(&src, "test");
        let recorder = Recorder(events);
        let Err(error) = outcome else {
            panic!("malformed {phase} unexpectedly parsed");
        };
        match error.into_inner() {
            Error::Lua(promptforge_lua::Error::LuaCompile {
                location,
                lua_source,
                ..
            }) => {
                assert_eq!(location, expected_location);
                assert_eq!(lua_source, "private_payload =");
            }
            other => panic!("expected LuaCompile, got {other:?}"),
        }

        let observations = recorder.observations();
        assert_eq!(
            observations
                .iter()
                .map(|(_, detail)| detail.clone())
                .collect::<Vec<_>>(),
            expected_details
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
        );
        assert!(
            observations
                .iter()
                .all(|(_, detail)| !detail.contains("private_payload"))
        );
    }
}

#[test]
fn unclosed_reserved_section_fences_are_location_errors() {
    for (content, phase) in [
        ("```lua\nreturn 1", "prologue"),
        ("Prose.\n\n```lua\nreturn reply", "epilog"),
    ] {
        let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{content}\n");
        let error = parse(&src).expect_err("reserved fence must close exactly");
        assert!(error.to_string().contains(phase));
        assert!(error.to_string().contains("not closed"));
    }
}

#[test]
fn successful_section_compilation_reports_fixed_ordered_boundaries() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.secret = 1\n```\n\nProse.\n\n```lua\nreturn reply\n```\n";
    let (prompt, events) = Prompt::parse(src, "section-programs");
    prompt.expect("section programs must compile");
    let recorder = Recorder(events);

    assert_eq!(
        recorder.observations(),
        vec![
            ("Prompt".into(), detail::PARSE_STARTED.to_string()),
            ("S".into(), detail::LUA_COMPILATION_STARTED.to_string()),
            ("S".into(), detail::LUA_COMPILATION_SUCCEEDED.to_string()),
            ("S".into(), detail::LUA_COMPILATION_STARTED.to_string()),
            ("S".into(), detail::LUA_COMPILATION_SUCCEEDED.to_string()),
            ("Prompt".into(), detail::PARSE_SUCCEEDED.to_string()),
        ]
    );
}

#[test]
fn non_lua_fence_stays_in_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nHere is code:\n\n```python\nprint(1)\n```\n";
    let p = parse(src).unwrap();
    assert!(p.sections[0].prologue().is_none());
    assert!(p.sections[0].epilog().is_none());
    assert!(p.sections[0].prose().contains("```python"));
}
