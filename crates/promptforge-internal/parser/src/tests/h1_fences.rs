//! H1 fence tests: live H1 blocks and the removed `lua shared` fence form.

use super::*;

#[test]
fn lua_shared_fence_in_h1_is_a_parse_error_pointing_to_require() {
    let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua shared\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
    let error = parse(src).expect_err("the removed shared fence must fail in the H1");
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("`lua shared` fence form was removed"),
        "{message}"
    );
    assert!(message.contains("require '## heading'"), "{message}");
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
fn removed_shared_fence_error_points_at_the_first_fence() {
    // `# T` is on line 6, the first fence on line 8, the second on line 12.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nlocal a = 1\n```\n\n```lua shared\nlocal b = 2\n```\n\n## S\n\np\n";
    let error = parse(src).expect_err("every shared fence is rejected");
    assert!(error.to_string().contains("require"), "{error}");
    assert_eq!(error.line(), Some(8), "{error}");
}

#[test]
fn lua_shared_fence_in_h2_is_a_parse_error_pointing_to_require() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua shared\nlocal a = 1\n```\n";
    let error = parse(src).expect_err("the removed shared fence must fail in an H2");
    assert_eq!(error.kind(), ParseErrorKind::Fence, "{error}");
    let message = error.to_string();
    assert!(
        message.contains("`lua shared` fence form was removed"),
        "{message}"
    );
    assert!(message.contains("require '## heading'"), "{message}");
    assert!(!message.contains("allowed only in H1"), "{message}");
}

#[test]
fn removed_lua_prompt_form_is_a_targeted_error_when_leading() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n## S\n\np\n";
    let error = parse(src).expect_err("the removed leading form must be rejected by name");
    let message = error.to_string();
    assert!(message.contains("`lua prompt` fence form was removed"));
    assert!(
        message.contains("use `lua` for a live H1 block and `require` for shared code"),
        "{message}"
    );
    assert!(!message.contains("lua shared"), "{message}");
}

#[test]
fn lua_prompt_form_after_prose_is_ordinary_prose() {
    let in_h1 = "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n```lua prompt\nnot compiled =\n```\n\n## S\n\np\n";
    let prompt = parse(in_h1).expect("the removed form after prose is ordinary Markdown");
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
    // Only the exact ```lua shared opener is rejected, so each near-miss
    // remains H1 prose.
    // The ```lua prompt form is excluded because leading it is a
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
        assert!(prompt.description_text.contains(near_miss));
    }

    let unclosed =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nreturn 1\n````\n\n## S\n\np\n";
    let error = parse(unclosed).expect_err("an exact opener is rejected however it closes");
    assert!(
        error
            .to_string()
            .contains("`lua shared` fence form was removed"),
        "{error}"
    );
}

#[test]
fn shared_markers_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua shared\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua shared\nreturn 2\n```\n````\n";
    let prompt = parse(src).expect("nested shared markers must remain prose");

    assert!(prompt.description_text.contains("```lua shared"));
    assert!(prompt.sections[0].prologue().is_none());
    assert!(prompt.sections[0].prose().contains("```lua shared"));
}

#[test]
fn removed_shared_fence_fails_before_its_lua_compiles() {
    let source = "private_payload =";
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua shared\n{source}\n```\n\n## S\n\np\n"
    );
    let (error, events) = Prompt::parse(&src, "parse-failure");
    let recorder = Recorder(events);
    let error = error.expect_err("the removed shared fence must fail");
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
