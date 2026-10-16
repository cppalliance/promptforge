//! Section fence tests: exact `lua` fences around section prose and their
//! compilation reports.

use super::*;

#[test]
fn lua_fence_separated_from_prose() {
    let body = "```lua\nreturn 42\n```\n\nActual prose here.";
    let p = parse_ok(&PromptSource::new().prose("S", body).build());
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
    let prompt = parse_ok(src);
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
    let body = "Ask the model.\n\n```lua\nreturn reply\n```";
    let prompt = parse_ok(&PromptSource::new().prose("Transform", body).build());
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
    let body = "Before.\n\n```lua\nvar.mid = 1\n```\n\nAfter.";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
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
    let body = "Before.\n\n```lua\nnot valid lua =\n```\n\nAfter.";
    let err = parse_err(&PromptSource::new().prose("S", body).build());
    assert_eq!(err.kind(), ParseErrorKind::Lua);
}

#[test]
fn one_exact_fence_is_the_prologue_and_two_can_surround_empty_prose() {
    let prompt = parse_ok(&one_section("var.x = 1"));
    let entry = prompt.entry().expect("has sections");
    assert!(entry.prologue().is_some());
    assert!(entry.epilog().is_none());

    let two = "```lua\nvar.x = 1\n```\n\n```lua\nreturn reply\n```";
    let prompt = parse_ok(&PromptSource::new().prose("S", two).build());
    let entry = prompt.entry().expect("has sections");
    assert_eq!(entry.prose(), "");
    assert!(entry.prologue().is_some());
    assert!(entry.epilog().is_some());
}

#[test]
fn non_exact_lua_openers_fail_in_a_section() {
    for fence in NON_EXACT_LUA_OPENERS {
        for intro in ["", "Intro.\n\n"] {
            let src =
                format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{intro}{fence}\n");
            let error = parse_err(&src);
            assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
            assert_eq!(error.to_string(), OPENER_MESSAGE, "{src:?}");
        }
    }
}

#[test]
fn lua_fence_lookalikes_outside_every_fence_are_parse_errors() {
    for content in [
        "Intro.\n\n    ```lua\n    return 1\n    ```",
        "Intro.\n\n    ~~~lua\n    return 1\n    ~~~",
        "Intro.\n\n    ``` lua\n    return 1\n    ```",
        "Intro.\n\n    ```\tlua\n    return 1\n    ```",
        "Intro.\n\n    ```LUA\n    return 1\n    ```",
        "<div>\n```lua\nreturn 1\n```\n</div>",
    ] {
        let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{content}\n");
        let error = parse_err(&src);
        assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
        assert_eq!(error.to_string(), OPENER_MESSAGE, "{src:?}");
    }
}

#[test]
fn section_fence_label_near_misses_are_parse_errors() {
    for label in ["Lua", "lua extra", "LUA", "luau"] {
        let body = format!("```{label}\nreturn 1\n```");
        let error = parse_err(&PromptSource::new().prose("S", &body).build());
        assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
        assert_eq!(
            error.to_string(),
            format!("unrecognized fence label `{label}`; the only Lua fence label is `lua`")
        );
    }
}

#[test]
fn fence_labels_not_starting_with_lua_stay_in_prose() {
    for fence in [
        "```\nreturn 1\n```",
        "```json\n{}\n```",
        "```text lua\nreturn 1\n```",
        "```moonlua\nreturn 1\n```",
    ] {
        let prompt = parse_ok(&PromptSource::new().prose("S", fence).build());
        let entry = prompt.entry().expect("has sections");
        assert!(entry.prologue().is_none());
        assert_eq!(entry.prose(), fence);
    }
}

#[test]
fn non_exact_section_closing_lines_fail_the_parse() {
    for bad_close in ["``` ", "  ```", "````"] {
        for after in [
            "```lua\nvar.b = 2\n```",
            "Prose.\n\n```python\nprint(1)\n```",
        ] {
            let src = format!(
                "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.a = 1\n{bad_close}\n\n{after}\n"
            );
            let error = parse_err(&src);
            assert_eq!(error.kind(), ParseErrorKind::Fence, "{error} in {src:?}");
            assert_eq!(error.to_string(), CLOSER_MESSAGE, "{src:?}");
        }
    }
}

#[test]
fn a_lookalike_line_and_a_bad_closing_line_report_whichever_comes_first() {
    let lookalike = "Intro.\n\n    ```lua";
    let fence = "```lua\nvar.a = 1\n````";
    for (content, message) in [
        (format!("{lookalike}\n\n{fence}"), OPENER_MESSAGE),
        (format!("{fence}\n\n{lookalike}"), CLOSER_MESSAGE),
    ] {
        let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{content}\n");
        let error = parse_err(&src);
        assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
        assert_eq!(error.to_string(), message, "{src:?}");
    }
}

#[test]
fn closer_and_lookalike_errors_report_the_offending_line() {
    // `## S` is on line 8 and the content starts on line 10.
    for (content, message, line) in [
        ("```lua\nvar.a = 1\nvar.b = 2\n  ```\n", CLOSER_MESSAGE, 13),
        (
            "Prose.\n\n    ```lua\n    return 1\n    ```\n",
            OPENER_MESSAGE,
            12,
        ),
    ] {
        let error = parse_err(&format!(
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{content}"
        ));
        assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
        assert_eq!(error.to_string(), message);
        assert_eq!(error.line(), NonZeroU32::new(line), "{error}");
        assert_eq!(error.column(), NonZeroU32::new(1), "{error}");
    }
}

#[test]
fn a_fence_split_that_disagrees_with_the_markdown_parser_is_an_internal_error() {
    // The parse refuses the four-backtick closing line before any split, so
    // only a direct split can reach the disagreement.
    let content = "```lua\nvar.a = 1\n````\n\n```lua\nvar.b = 2\n```\n";
    let Err(error) = crate::fence::split_section_blocks(content, "S") else {
        panic!("required a split failure, actual a split of {content:?}");
    };
    assert!(matches!(error, Error::Internal(_)), "{error:?}");
    assert_eq!(
        ParseError::from_inner(error).kind(),
        ParseErrorKind::Internal
    );
}

#[test]
fn section_markers_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n````markdown\n```lua\nreturn 1\n```\n````\n";
    let prompt = parse_ok(src);

    let entry = prompt.entry().expect("has sections");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_none());
    assert!(entry.prose().contains("```lua"));
}

#[test]
fn lines_inside_another_fence_body_are_never_inspected() {
    for fence in [
        "````\nsome crap example\n```lua caca\nblahblah\n```\nmore\n````",
        "````\n```lua \nreturn 1\n```\n````",
        "````\n    ```lua\nreturn 1\n```\n````",
    ] {
        let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n{fence}\n");
        let prompt = parse_ok(&src);
        let entry = prompt.entry().expect("has sections");
        assert!(entry.prologue().is_none());
        assert!(entry.epilog().is_none());
        assert_eq!(entry.prose(), fence);
    }
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
        let src = PromptSource::new()
            .prose("Private section", content)
            .build();
        let (outcome, events) = Prompt::parse(&src, "test");
        let recorder = Recorder(events);
        let Err(error) = outcome else {
            panic!("malformed {phase} unexpectedly parsed");
        };
        match error.into_inner() {
            Error::Lua(runvil_lua::Error::LuaCompile {
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
        let error = parse_err(&PromptSource::new().prose("S", content).build());
        assert!(error.to_string().contains(phase));
        assert!(error.to_string().contains("not closed"));
    }
}

#[test]
fn backtick_lines_that_cannot_close_a_lua_fence_leave_it_unclosed() {
    for last_line in ["    ```", "``` x"] {
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nreturn 1\n{last_line}\n"
        );
        let error = parse_err(&src);
        assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
        assert_eq!(
            error.to_string(),
            "section `S` prologue `lua` fence is not closed",
            "{src:?}"
        );
    }
}

#[test]
fn successful_section_compilation_reports_fixed_ordered_boundaries() {
    let body = "```lua\nvar.secret = 1\n```\n\nProse.\n\n```lua\nreturn reply\n```";
    let src = PromptSource::new().prose("S", body).build();
    let (prompt, events) = Prompt::parse(&src, "section-programs");
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
    let body = "Here is code:\n\n```python\nprint(1)\n```";
    let p = parse_ok(&PromptSource::new().prose("S", body).build());
    assert!(p.sections[0].prologue().is_none());
    assert!(p.sections[0].epilog().is_none());
    assert!(p.sections[0].prose().contains("```python"));
}
