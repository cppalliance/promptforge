//! H1 fence tests: live H1 blocks and the `lua shared` library fence.

use super::*;

#[test]
fn shared_library_allows_blank_lines_and_is_compiled() {
    let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua shared\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
    let prompt = parse(src).expect("shared Lua must parse");
    let replay = prompt.replay.expect("replay program must be present");
    assert_eq!(replay.source(), "function answer() return 42 end");
    assert_eq!(prompt.description_text, "Description.");
}

#[test]
fn h1_plain_lua_and_prose_are_live_blocks() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal first = 1\n```\n\nPlan {{ args }}.\n\n```lua shared\nfunction helper() return 1 end\n```\n\n```lua\nstore.write('done', reply)\n```\n\n## S\n\np\n";
    let prompt = parse(src).expect("H1 blocks must parse");
    assert_eq!(
        prompt.replay.as_ref().map(LuaProgram::source),
        Some("function helper() return 1 end")
    );
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
fn lone_plain_h1_lua_is_not_a_shared_library() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal live = true\n```\n\n## S\n\np\n";
    let prompt = parse(src).expect("plain H1 Lua must parse");
    assert!(prompt.replay.is_none());
    assert!(matches!(
        prompt.h1_blocks.as_slice(),
        [Block::Lua(program)] if program.source() == "local live = true"
    ));
}

#[test]
fn second_shared_fence_is_a_parse_error() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nlocal a = 1\n```\n\n```lua shared\nlocal b = 2\n```\n\n## S\n\np\n";
    let error = parse(src).expect_err("a second shared fence must fail");
    assert!(error.to_string().contains("at most one `lua shared`"));
}

#[test]
fn shared_fence_in_h2_is_a_parse_error() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua shared\nlocal a = 1\n```\n";
    let error = parse(src).expect_err("a shared fence in H2 must fail");
    assert!(error.to_string().contains("allowed only in H1"));
}

#[test]
fn removed_lua_prompt_form_is_a_targeted_error_when_leading() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n## S\n\np\n";
    let error = parse(src).expect_err("the removed leading form must be rejected by name");
    assert!(
        error
            .to_string()
            .contains("`lua prompt` fence form was removed")
    );
}

#[test]
fn lua_prompt_form_after_prose_is_ordinary_prose() {
    let in_h1 = "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n```lua prompt\nnot compiled =\n```\n\n## S\n\np\n";
    let prompt = parse(in_h1).expect("the removed form after prose is ordinary Markdown");
    assert!(prompt.replay.is_none());
    assert!(prompt.description_text.contains("```lua prompt"));

    let in_section =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua prompt\nnot compiled =\n```\n";
    let prompt = parse(in_section).expect("the removed form in a section is ordinary Markdown");
    let entry = prompt.entry().expect("has sections");
    assert!(entry.prologue().is_none());
    assert!(entry.prose().contains("```lua prompt"));
}

#[test]
fn shared_fence_markers_must_be_exact() {
    // Only the exact ```lua shared opener is reserved, so each near-miss
    // remains H1 prose.
    // The removed ```lua prompt form is excluded because leading it is a
    // targeted error, pinned by
    // `removed_lua_prompt_form_is_a_targeted_error_when_leading`.
    for near_miss in [
        "````lua shared\nreturn 1\n````",
        " ```lua shared\nreturn 1\n ```",
        "```Lua shared\nreturn 1\n```",
        "```lua  shared\nreturn 1\n```",
        "```lua shared extra\nreturn 1\n```",
    ] {
        let src = format!("---\nname: x\ndescription: d\n---\n\n# T\n\n{near_miss}\n\n## S\n\np\n");
        let prompt = parse(&src).expect("leading near-miss shared markers must remain prose");
        assert!(prompt.replay.is_none());
        assert!(prompt.description_text.contains(near_miss.trim()));
    }

    // Placement does not change exact-marker recognition.
    for near_miss in [
        "````lua shared\nreturn 1\n````",
        " ```lua shared\nreturn 1\n ```",
        "```Lua shared\nreturn 1\n```",
        "```lua shared extra\nreturn 1\n```",
        "```lua prompt\nreturn 1\n```",
    ] {
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n{near_miss}\n\n## S\n\np\n"
        );
        let prompt = parse(&src).expect("near-miss shared markers must remain prose");
        assert!(prompt.replay.is_none());
        assert!(prompt.description_text.contains(near_miss));
    }

    let unclosed =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nreturn 1\n````\n\n## S\n\np\n";
    let error = parse(unclosed).expect_err("near-miss closing marker must not close the fence");
    assert!(error.to_string().contains("not closed"));
}

#[test]
fn shared_markers_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua shared\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua shared\nreturn 2\n```\n````\n";
    let prompt = parse(src).expect("nested shared markers must remain prose");

    assert!(prompt.replay.is_none());
    assert!(prompt.description_text.contains("```lua shared"));
    assert!(prompt.sections[0].prologue().is_none());
    assert!(prompt.sections[0].prose().contains("```lua shared"));
}

#[test]
fn malformed_shared_lua_retains_diagnostics_and_reports_safe_boundaries() {
    let source = "private_payload =";
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua shared\n{source}\n```\n\n## S\n\np\n"
    );
    let (error, events) = Prompt::parse(&src, "parse-failure");
    let recorder = Recorder(events);
    let error = error.expect_err("malformed shared Lua must fail");
    match error.into_inner() {
        Error::Lua(promptforge_lua::Error::LuaCompile {
            location,
            lua_source,
            ..
        }) => {
            assert_eq!(location, "prompt shared library");
            assert_eq!(lua_source, source);
        }
        other => panic!("expected LuaCompile, got {other:?}"),
    }
    let observations = recorder.observations();
    assert_eq!(
        observations,
        vec![
            ("Prompt".into(), detail::PARSE_STARTED.to_string()),
            (
                "Private title".into(),
                detail::LUA_COMPILATION_STARTED.to_string()
            ),
            (
                "Private title".into(),
                detail::LUA_COMPILATION_FAILED.to_string()
            ),
            ("Prompt".into(), detail::PARSE_FAILED.to_string()),
        ]
    );
    assert!(
        observations
            .iter()
            .all(|(section, detail)| !section.contains(source) && !detail.contains(source))
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
