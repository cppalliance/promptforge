use std::sync::Mutex;

use super::list::parse_bullet_items;
use super::*;
use crate::observe::{NullObserver, Observation, detail};

#[test]
fn invalid_frontmatter_preserves_the_yaml_cause_as_source() {
    // error.rs F3: a malformed-YAML frontmatter must classify as
    // `Frontmatter` and retain the underlying serde_yaml_ng failure as the
    // public error's `source()`, instead of flattening it into a string.
    let src = "---\nname: p\ndescription: d\n: : :\n---\n\n# T\n\n## S\n\nhi\n";
    let error = Prompt::parse(src, "test", &NullObserver)
        .expect_err("malformed YAML frontmatter must fail to parse");
    assert_eq!(error.kind(), ParseErrorKind::Frontmatter);
    assert!(
        std::error::Error::source(&error).is_some(),
        "the YAML decode failure must be preserved as the error source: {error}"
    );
}

#[test]
fn mixed_prose_with_one_bullet_is_not_a_list() {
    // PF-PARSER-005: an incidental bullet line in ordinary prose must not
    // force strict list parsing; the section stays prose.
    let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\nHere is context.\n- one incidental bullet\nMore prose follows.\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(!section.is_list_only(), "mixed prose is not a list");
    assert!(section.items().is_empty());
    assert!(section.prose().contains("incidental bullet"));
}

#[test]
fn pure_list_section_parses_items() {
    let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\n- alpha\n- beta\n3. gamma\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_list_only());
    assert_eq!(section.items(), ["alpha", "beta", "gamma"]);
}

#[test]
fn all_marker_list_with_empty_item_is_rejected() {
    // Every nonblank line is a marker, so it is a list; the empty marker is
    // then a hard error rather than a detector miss.
    let src = "---\nname: p\ndescription: d\n---\n\n# T\n\n## S\n\n- alpha\n1.\n- beta\n";
    let error = Prompt::parse(src, "test", &NullObserver).expect_err("empty item must fail");
    assert_eq!(error.kind(), ParseErrorKind::List);
}

#[test]
fn list_error_kind_does_not_depend_on_the_section_name() {
    for section in ["frontmatter", "fence"] {
        let src =
            format!("---\nname: p\ndescription: d\n---\n\n# T\n\n## {section}\n\n- alpha\n1.\n");
        let error =
            Prompt::parse(&src, "test", &NullObserver).expect_err("an empty list item must fail");
        assert_eq!(error.kind(), ParseErrorKind::List);
    }
}

#[test]
fn parsed_prompt_value_types_are_equatable() {
    // PF-PARSER-011: parsing the same source twice yields equal values, and
    // a differing source yields unequal values, across the finalized parser
    // value types (`Prompt`, `Frontmatter`, `Section`, `Block`).
    let src = "---\nname: p\ndescription: d\n---\n\n# Title\n\n## One\n\ndo a thing\n";
    let a = Prompt::parse(src, "test", &NullObserver).unwrap();
    let b = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert_eq!(a, b, "identical sources must parse equal");
    assert_eq!(a.frontmatter, b.frontmatter);
    assert_eq!(a.sections, b.sections);

    let other = "---\nname: p\ndescription: d\n---\n\n# Title\n\n## Two\n\ndo a thing\n";
    let c = Prompt::parse(other, "test", &NullObserver).unwrap();
    assert_ne!(a, c, "differing section headings must parse unequal");
}

#[derive(Default)]
struct Recorder(Mutex<Vec<(String, String, String)>>);

impl Observer for Recorder {
    fn observe(&self, execution: &str, section: &str, event: Observation) {
        self.0
            .lock()
            .expect("recording lock must remain usable")
            .push((
                execution.to_string(),
                section.to_string(),
                event.to_string(),
            ));
    }
}

impl Recorder {
    fn records(&self) -> Vec<(String, String, String)> {
        self.0
            .lock()
            .expect("recording lock must remain usable")
            .clone()
    }

    fn observations(&self) -> Vec<(String, String)> {
        self.0
            .lock()
            .expect("recording lock must remain usable")
            .iter()
            .map(|(_, section, detail)| (section.clone(), detail.clone()))
            .collect()
    }
}

#[test]
fn parses_multi_section_with_all_features() {
    let src = "---\n\
name: demo\n\
description: A demo\n\
---\n\
\n\
# Demo Title\n\
\n\
Human-readable intro text.\n\
\n\
## First\n\
\n\
```lua\n\
local x = 1\n\
```\n\
\n\
Prose for the first section.\n\
\n\
### Child\n\
\n\
Child prose.\n\
\n\
## Second\n\
\n\
Prose for the second section.\n";

    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert_eq!(p.frontmatter.name, "demo");
    assert_eq!(p.frontmatter.description, "A demo");
    assert_eq!(p.title, "Demo Title");
    assert!(p.replay.is_none());
    assert_eq!(
        p.h1_blocks,
        vec![Block::Prose {
            text: "Human-readable intro text.".to_owned(),
            loop_capable: true,
        }]
    );
    assert_eq!(p.description_text, "Human-readable intro text.");

    assert_eq!(p.sections.len(), 2);
    let first = &p.sections[0];
    assert_eq!(first.name, "First");
    assert_eq!(first.level, 2);
    assert_eq!(
        first.prologue().map(LuaProgram::source),
        Some("local x = 1")
    );
    assert_eq!(first.prose(), "Prose for the first section.");
    assert!(first.epilog().is_none());
    assert_eq!(first.children.len(), 1);
    assert_eq!(first.children[0].name, "Child");
    assert_eq!(first.children[0].level, 3);
    assert_eq!(first.children[0].prose(), "Child prose.");

    assert_eq!(p.sections[1].name, "Second");
    assert!(p.sections[1].prologue().is_none());
    assert!(p.sections[1].epilog().is_none());
}

#[test]
fn parses_single_minimal_section() {
    let src = "---\nname: hi\ndescription: d\n---\n\n# T\n\n## Greet\n\nSay hi\n";
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert_eq!(p.sections.len(), 1);
    assert_eq!(p.sections[0].name, "Greet");
    assert_eq!(p.sections[0].prose(), "Say hi");
}

#[test]
fn name_and_description_are_sufficient_frontmatter_for_parsing() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("minimum frontmatter must parse");
    assert_eq!(prompt.frontmatter.name, "x");
}

#[test]
fn missing_frontmatter_delimiter_errors() {
    let src = "# T\n\n## S\n\np\n";
    assert!(Prompt::parse(src, "test", &NullObserver).is_err());
}

#[test]
fn h1_only_prompt_parses_with_empty_sections() {
    let src = "---\nname: x\ndescription: d\npromptforge: 1\n---\n\n# Only a title\n\nText.\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("H1-only prompt must parse");
    assert!(prompt.sections.is_empty());
}

#[test]
fn empty_h1_title_errors() {
    let src = "---\nname: x\ndescription: d\n---\n\n#\n\n## S\n\np\n";
    let error = Prompt::parse(src, "test", &NullObserver).expect_err("H1 title must not be empty");
    assert!(error.to_string().contains("title must not be empty"));
}

#[test]
fn preface_before_h1_is_ignored() {
    let src = "---\nname: x\ndescription: d\n---\n\nIgnored preface.\n\n```text\nalso ignored\n```\n\n# T\n\nDescription.\n\n## S\n\np\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("preface is not semantic");
    assert_eq!(prompt.title, "T");
    assert_eq!(prompt.description_text, "Description.");
    assert_eq!(prompt.entry().expect("has sections").name, "S");
}

#[test]
fn shared_library_allows_blank_lines_and_is_compiled() {
    let src = "---\r\nname: x\r\ndescription: d\r\n---\r\n\r\n# T\r\n\r\n \t\r\n```lua shared\r\nfunction answer() return 42 end\r\n```\r\n\r\nDescription.\r\n\r\n## S\r\n\r\np\r\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("shared Lua must parse");
    let replay = prompt.replay.expect("replay program must be present");
    assert_eq!(replay.source(), "function answer() return 42 end");
    assert_eq!(prompt.description_text, "Description.");
}

#[test]
fn h1_plain_lua_and_prose_are_live_blocks() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal first = 1\n```\n\nPlan {{ args }}.\n\n```lua shared\nfunction helper() return 1 end\n```\n\n```lua\nstore.write('done', reply)\n```\n\n## S\n\np\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("H1 blocks must parse");
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
        Block::Prose {
            text,
            loop_capable: true
        } if text == "Plan {{ args }}."
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
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("plain H1 Lua must parse");
    assert!(prompt.replay.is_none());
    assert!(matches!(
        prompt.h1_blocks.as_slice(),
        [Block::Lua(program)] if program.source() == "local live = true"
    ));
}

#[test]
fn second_shared_fence_is_a_parse_error() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nlocal a = 1\n```\n\n```lua shared\nlocal b = 2\n```\n\n## S\n\np\n";
    let error =
        Prompt::parse(src, "test", &NullObserver).expect_err("a second shared fence must fail");
    assert!(error.to_string().contains("at most one `lua shared`"));
}

#[test]
fn shared_fence_in_h2_is_a_parse_error() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua shared\nlocal a = 1\n```\n";
    let error =
        Prompt::parse(src, "test", &NullObserver).expect_err("a shared fence in H2 must fail");
    assert!(error.to_string().contains("allowed only in H1"));
}

#[test]
fn removed_lua_prompt_form_is_a_targeted_error_when_leading() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua prompt\nlocal a = 1\n```\n\n## S\n\np\n";
    let error = Prompt::parse(src, "test", &NullObserver)
        .expect_err("the removed leading form must be rejected by name");
    assert!(
        error
            .to_string()
            .contains("`lua prompt` fence form was removed")
    );
}

#[test]
fn lua_prompt_form_after_prose_is_ordinary_prose() {
    let in_h1 = "---\nname: x\ndescription: d\n---\n\n# T\n\nIntro.\n\n```lua prompt\nnot compiled =\n```\n\n## S\n\np\n";
    let prompt = Prompt::parse(in_h1, "test", &NullObserver)
        .expect("the removed form after prose is ordinary Markdown");
    assert!(prompt.replay.is_none());
    assert!(prompt.description_text.contains("```lua prompt"));

    let in_section =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua prompt\nnot compiled =\n```\n";
    let prompt = Prompt::parse(in_section, "test", &NullObserver)
        .expect("the removed form in a section is ordinary Markdown");
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
        let prompt = Prompt::parse(&src, "test", &NullObserver)
            .expect("leading near-miss shared markers must remain prose");
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
        let prompt = Prompt::parse(&src, "test", &NullObserver)
            .expect("near-miss shared markers must remain prose");
        assert!(prompt.replay.is_none());
        assert!(prompt.description_text.contains(near_miss));
    }

    let unclosed =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nreturn 1\n````\n\n## S\n\np\n";
    let error = Prompt::parse(unclosed, "test", &NullObserver)
        .expect_err("near-miss closing marker must not close the fence");
    assert!(error.to_string().contains("not closed"));
}

#[test]
fn shared_markers_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n````markdown\n```lua shared\nreturn 1\n```\n````\n\nIntro.\n\n## S\n\n````markdown\n```lua shared\nreturn 2\n```\n````\n";
    let prompt =
        Prompt::parse(src, "test", &NullObserver).expect("nested shared markers must remain prose");

    assert!(prompt.replay.is_none());
    assert!(prompt.description_text.contains("```lua shared"));
    assert!(prompt.sections[0].prologue().is_none());
    assert!(prompt.sections[0].prose().contains("```lua shared"));
}

#[test]
fn malformed_shared_lua_retains_diagnostics_and_reports_safe_boundaries() {
    let recorder = Recorder::default();
    let source = "private_payload =";
    let src = format!(
        "---\nname: x\ndescription: d\n---\n\n# Private title\n\n```lua shared\n{source}\n```\n\n## S\n\np\n"
    );
    let error = Prompt::parse(&src, "parse-failure", &recorder)
        .expect_err("malformed shared Lua must fail");
    match Error::from(error) {
        Error::LuaCompile {
            location,
            lua_source,
            ..
        } => {
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
    let recorder = Recorder::default();
    let source =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua\nlocal secret = 42\n```\n\n## S\n\np\n";
    Prompt::parse(source, "parse-success", &recorder).expect("prompt must parse");
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

#[test]
fn lua_fence_separated_from_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nreturn 42\n```\n\nActual prose here.\n";
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
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
    let prompt =
        Prompt::parse(src, "test", &NullObserver).expect("both exact section phases must compile");
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
    let prompt =
        Prompt::parse(src, "test", &NullObserver).expect("the trailing epilog must compile");
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
    let prompt =
        Prompt::parse(src, "test", &NullObserver).expect("middle Lua fences compile as blocks");
    let section = prompt.entry().expect("has sections");

    assert!(section.prologue().is_none());
    assert!(section.epilog().is_none());
    assert_eq!(section.prose(), "After.");
    assert_eq!(section.blocks.len(), 3);
    match &section.blocks[0] {
        Block::Prose {
            text,
            loop_capable: false,
        } => assert_eq!(text, "Before."),
        other => panic!("expected non-final prose, got {other:?}"),
    }
    match &section.blocks[1] {
        Block::Lua(program) => assert_eq!(program.source(), "var.mid = 1"),
        other => panic!("expected lua block, got {other:?}"),
    }
    match &section.blocks[2] {
        Block::Prose {
            text,
            loop_capable: true,
        } => assert_eq!(text, "After."),
        other => panic!("expected final prose, got {other:?}"),
    }
}

#[test]
fn invalid_middle_lua_fence_fails_parse() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nBefore.\n\n```lua\nnot valid lua =\n```\n\nAfter.\n";
    let err = Prompt::parse(src, "test", &NullObserver)
        .expect_err("invalid middle Lua must fail compilation");
    assert!(
        err.to_string().contains("lua") || err.to_string().contains("Lua"),
        "error was: {err}"
    );
}

#[test]
fn one_exact_fence_is_the_prologue_and_two_can_surround_empty_prose() {
    let one = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n";
    let prompt = Prompt::parse(one, "test", &NullObserver).expect("one fence is the prologue");
    let entry = prompt.entry().expect("has sections");
    assert!(entry.prologue().is_some());
    assert!(entry.epilog().is_none());

    let two = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.x = 1\n```\n\n```lua\nreturn reply\n```\n";
    let prompt =
        Prompt::parse(two, "test", &NullObserver).expect("two fences can enclose empty prose");
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
        let prompt =
            Prompt::parse(&src, "test", &NullObserver).expect("near-miss fence must remain prose");
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
        let error = Prompt::parse(&src, "test", &NullObserver)
            .expect_err("a near-miss closing fence must not panic or close the block");
        assert!(error.to_string().contains("not closed exactly"));
    }
}

#[test]
fn section_markers_inside_longer_fences_remain_prose() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n````markdown\n```lua\nreturn 1\n```\n````\n";
    let prompt =
        Prompt::parse(src, "test", &NullObserver).expect("nested markers must remain prose");

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
        let recorder = Recorder::default();
        let src = format!(
            "---\nname: x\ndescription: d\n---\n\n# T\n\n## Private section\n\n{content}\n"
        );
        let Err(error) = Prompt::parse(&src, "test", &recorder) else {
            panic!("malformed {phase} unexpectedly parsed");
        };
        match Error::from(error) {
            Error::LuaCompile {
                location,
                lua_source,
                ..
            } => {
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
        let error = Prompt::parse(&src, "test", &NullObserver)
            .expect_err("reserved fence must close exactly");
        assert!(error.to_string().contains(phase));
        assert!(error.to_string().contains("not closed"));
    }
}

#[test]
fn successful_section_compilation_reports_fixed_ordered_boundaries() {
    let recorder = Recorder::default();
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.secret = 1\n```\n\nProse.\n\n```lua\nreturn reply\n```\n";
    Prompt::parse(src, "section-programs", &recorder).expect("section programs must compile");

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
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert!(p.sections[0].prologue().is_none());
    assert!(p.sections[0].epilog().is_none());
    assert!(p.sections[0].prose().contains("```python"));
}

#[test]
fn recursive_nesting_h2_h3_h4() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### B\n\nb\n\n#### C\n\nc\n";
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    let a = &p.sections[0];
    assert_eq!(a.name, "A");
    let b = &a.children[0];
    assert_eq!(b.name, "B");
    assert_eq!(b.level, 3);
    let c = &b.children[0];
    assert_eq!(c.name, "C");
    assert_eq!(c.level, 4);
}

#[test]
fn skipped_heading_level_is_rejected_as_orphan() {
    // H4 directly under H2 (no intervening H3) is an orphan deep heading:
    // it has no parent H3, so it must be rejected, not reparented to the H2.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n#### D\n\nd\n";
    let err = Prompt::parse(src, "test", &NullObserver)
        .expect_err("an H4 with no parent H3 must be rejected");
    assert!(
        err.to_string().contains("orphan"),
        "expected an orphan-heading error, got: {err}"
    );
}

#[test]
fn orphan_top_level_deep_heading_is_rejected() {
    // The first section heading is an H3 with no parent H2: an orphan.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n### A\n\na\n";
    let err = Prompt::parse(src, "test", &NullObserver)
        .expect_err("an H3 top-level section with no parent H2 must be rejected");
    assert!(
        err.to_string().contains("orphan"),
        "expected an orphan-heading error, got: {err}"
    );

    // An H4 top-level section (double skip) is likewise rejected.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n#### A\n\na\n";
    assert!(
        Prompt::parse(src, "test", &NullObserver).is_err(),
        "an H4 top-level section must be rejected"
    );
}

#[test]
fn unknown_frontmatter_field_is_rejected() {
    let src = "---\nname: x\ndescription: d\nnot_a_real_field: 1\n---\n\n# T\n\n## S\n\np\n";
    let err = Prompt::parse(src, "test", &NullObserver)
        .expect_err("an unknown frontmatter field must be rejected");
    assert!(
        err.to_string().contains("not_a_real_field") || err.to_string().contains("unknown field"),
        "expected an unknown-field error, got: {err}"
    );
    // A known-field-only frontmatter still parses.
    let ok = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    assert!(Prompt::parse(ok, "test", &NullObserver).is_ok());
}

#[test]
fn empty_section_heading_is_rejected() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## \n\na\n";
    let err = Prompt::parse(src, "test", &NullObserver)
        .expect_err("an empty section heading must be rejected");
    assert!(
        err.to_string().contains("must not be empty"),
        "expected an empty-heading error, got: {err}"
    );
}

#[test]
fn duplicate_sibling_section_names_are_rejected() {
    // Two H2 siblings named `S` are ambiguous section targets.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\na\n\n## S\n\nb\n";
    let err = Prompt::parse(src, "test", &NullObserver)
        .expect_err("duplicate sibling section names must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("duplicate sibling section name"),
        "expected a duplicate-sibling error, got: {err}"
    );
    // PF-PARSER-002: both duplicate locations are named. The first `## S` is
    // at body line 3 (line 8 overall) and the second at body line 7 (line 12).
    assert!(
        message.contains("first declared at line 8") && message.contains("again at line 12"),
        "both duplicate locations must be reported, got: {message}"
    );
    // PF-PARSER-008: a structured parse error carries a stable kind and a
    // byte span rather than inferring them from the message.
    assert_eq!(err.kind(), ParseErrorKind::Structure);
    let (start, end) = err.span().expect("duplicate section carries a span");
    assert!(
        start < end,
        "span must be a non-empty range, got {start}..{end}"
    );

    // The same name under DIFFERENT parents (not siblings) is allowed.
    let ok = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\na\n\n### S\n\nx\n\n## B\n\nb\n\n### S\n\ny\n";
    assert!(
        Prompt::parse(ok, "test", &NullObserver).is_ok(),
        "the same name under different parents is not a sibling collision"
    );
}

#[test]
fn max_tool_iterations_parses_positive_and_defaults_when_absent() {
    let declared =
        "---\nname: x\ndescription: d\nmax_tool_iterations: 20\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(declared, "test", &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Limit(std::num::NonZeroU32::new(20).unwrap())
    );

    let absent = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(absent, "test", &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Default
    );
}

#[test]
fn max_tool_iterations_rejects_zero_negative_and_overflow() {
    let body = |value: &str| {
        format!(
            "---\nname: x\ndescription: d\nmax_tool_iterations: {value}\n---\n\n# T\n\n## S\n\np\n"
        )
    };
    for bad in ["0", "-1", "1001", "100000000000"] {
        let error = Prompt::parse(&body(bad), "test", &NullObserver)
            .expect_err(&format!("max_tool_iterations {bad} must be rejected"));
        assert_eq!(
            error.kind(),
            ParseErrorKind::Frontmatter,
            "value {bad}: {error}"
        );
    }
}

#[test]
fn max_tool_iterations_accepts_the_upper_boundary() {
    let body = format!(
        "---\nname: x\ndescription: d\nmax_tool_iterations: {MAX_TOOL_ITERATIONS}\n---\n\n# T\n\n## S\n\np\n"
    );
    let p = Prompt::parse(&body, "test", &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter.max_tool_iterations,
        MaxToolIterations::Limit(std::num::NonZeroU32::new(MAX_TOOL_ITERATIONS).unwrap())
    );
}

#[test]
fn max_tool_iterations_resolve_uses_default_only_when_absent() {
    assert_eq!(MaxToolIterations::Default.resolve(24), 24);
    assert_eq!(
        MaxToolIterations::Limit(std::num::NonZeroU32::new(3).unwrap()).resolve(24),
        3
    );
}

#[test]
fn first_h2_is_entry_regardless_of_name() {
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## Zebra\n\nfirst\n\n## Main\n\nsecond\n";
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert_eq!(p.entry().expect("has sections").name, "Zebra");
}

#[test]
fn detection_reads_promptforge_major() {
    let src = "---\nname: x\ndescription: d\npromptforge: 1\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(src), Some(1));
}

#[test]
fn detection_needs_only_the_promptforge_key() {
    // No name or description, but the key is present.
    let src = "---\npromptforge: 2\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(src), Some(2));
}

#[test]
fn detection_absent_key_is_none() {
    let src = "---\nname: x\ndescription: d\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(src), None);
}

#[test]
fn detection_no_frontmatter_is_none() {
    let src = "# Just a title\n\nPlain prose with no frontmatter block at all.\n";
    assert_eq!(promptforge_version(src), None);
}

#[test]
fn detection_malformed_frontmatter_is_none() {
    // Opening delimiter but never closed.
    let unclosed = "---\npromptforge: 1\nname: x\n\n## S\n\np\n";
    assert_eq!(promptforge_version(unclosed), None);

    // Closed, but not valid YAML.
    let bad_yaml = "---\npromptforge: 1\n  : : oops\n---\n\n## S\n\np\n";
    assert_eq!(promptforge_version(bad_yaml), None);
}

#[test]
fn frontmatter_exposes_promptforge_field() {
    let with = "---\nname: x\ndescription: d\npromptforge: 1\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(with, "test", &NullObserver).unwrap();
    assert_eq!(p.frontmatter.promptforge, Some(1));

    let without = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(without, "test", &NullObserver).unwrap();
    assert_eq!(p.frontmatter.promptforge, None);
}

#[test]
fn bullet_parser_strips_unordered_markers() {
    let items = parse_bullet_items("- alpha\n* beta\n- gamma", "test").unwrap();
    assert_eq!(items, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn bullet_parser_strips_ordered_markers() {
    let items = parse_bullet_items("1. first\n2. second\n3) third", "test").unwrap();
    assert_eq!(items, vec!["first", "second", "third"]);
}

#[test]
fn bullet_parser_ignores_blank_lines() {
    let items = parse_bullet_items("- alpha\n\n- beta\n  \n- gamma", "test").unwrap();
    assert_eq!(items, vec!["alpha", "beta", "gamma"]);
}

#[test]
fn bullet_parser_rejects_non_list_content() {
    let err = parse_bullet_items("- alpha\nnot a bullet\n- gamma", "test")
        .expect_err("non-list content must error");
    assert!(
        err.to_string().contains("non-list content"),
        "error was: {err}"
    );
}

#[test]
fn bullet_parser_rejects_empty_list() {
    let err = parse_bullet_items("", "test").expect_err("empty list must error");
    assert!(err.to_string().contains("no items"), "error was: {err}");
}

#[test]
fn bullet_parser_rejects_empty_item() {
    let err =
        parse_bullet_items("- alpha\n- \n- gamma", "test").expect_err("empty item must error");
    assert!(
        err.to_string().contains("empty bullet item"),
        "error was: {err}"
    );
}

#[test]
fn list_h3_parses_items_at_load_time() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Parent\n\np\n\n### Items\n\n- alpha\n- beta\n";
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    let items_section = &p.sections[0].children[0];
    assert_eq!(items_section.name, "Items");
    assert_eq!(items_section.items, vec!["alpha", "beta"]);
}

#[test]
fn non_list_h3_has_empty_items() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Parent\n\np\n\n### Worker\n\n```lua\nreturn item\n```\n\nDo work on {{ item }}.\n";
    let p = Prompt::parse(src, "test", &NullObserver).unwrap();
    let worker = &p.sections[0].children[0];
    assert_eq!(worker.name, "Worker");
    assert!(worker.items.is_empty());
}

#[test]
fn epilog_source_line_maps_runtime_error_to_absolute_line() {
    // Lines:
    //  1: ---
    //  2: name: x
    //  3: description: d
    //  4: ---
    //  5: (empty)
    //  6: # T
    //  7: (empty)
    //  8: ## Check
    //  9: (empty)
    // 10: Ask the model.
    // 11: (empty)
    // 12: ```lua       <- epilog opens
    // 13: local a = 1  <- epilog line 1 (source_line = 13)
    // 14: assert(false) <- epilog line 2 (absolute = 14)
    // 15: ```
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Check\n\nAsk the model.\n\n```lua\nlocal a = 1\nassert(false)\n```\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
    let epilog = prompt
        .entry()
        .expect("has sections")
        .epilog()
        .expect("epilog must exist");

    assert_eq!(
        epilog.source_line().get(),
        13,
        "epilog Lua starts on line 13"
    );
    assert_eq!(epilog.source(), "local a = 1\nassert(false)");

    // Simulate a runtime error: assert(false) is on chunk line 2.
    // Absolute line = 13 + 2 - 1 = 14.
    let lua = mlua::Lua::new();
    let function = epilog.load(&lua).expect("bytecode must load");
    let raw_error = function
        .call::<()>(())
        .expect_err("assert(false) must fail");
    let mapped = epilog.map_runtime_error(&raw_error);
    let msg = mapped.to_string();
    assert!(
        msg.contains(":14:"),
        "error must contain absolute line 14: {msg}"
    );
}

#[test]
fn prologue_source_line_maps_correctly() {
    // Lines:
    //  1: ---
    //  2: name: x
    //  3: description: d
    //  4: ---
    //  5: (empty)
    //  6: # T
    //  7: (empty)
    //  8: ## Work
    //  9: (empty)
    // 10: ```lua       <- prologue opens
    // 11: assert(false) <- prologue line 1 (source_line = 11, absolute = 11)
    // 12: ```
    // 13: (empty)
    // 14: Do the work.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Work\n\n```lua\nassert(false)\n```\n\nDo the work.\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
    let prologue = prompt
        .entry()
        .expect("has sections")
        .prologue()
        .expect("prologue must exist");

    assert_eq!(
        prologue.source_line().get(),
        11,
        "prologue Lua starts on line 11"
    );

    let lua = mlua::Lua::new();
    let function = prologue.load(&lua).expect("bytecode must load");
    let raw_error = function
        .call::<()>(())
        .expect_err("assert(false) must fail");
    let mapped = prologue.map_runtime_error(&raw_error);
    let msg = mapped.to_string();
    assert!(
        msg.contains(":11:"),
        "error must contain absolute line 11: {msg}"
    );
}

#[test]
fn multi_line_chunk_maps_inner_line_correctly() {
    // Epilog with assert on line 2 of the fence.
    //  1-4: frontmatter
    //  5: empty
    //  6: # T
    //  7: empty
    //  8: ## S
    //  9: empty
    // 10: Prose.
    // 11: empty
    // 12: ```lua
    // 13: local x = 1    <- source_line = 13
    // 14: local y = 2
    // 15: assert(false)  <- chunk line 3, absolute = 13 + 3 - 1 = 15
    // 16: ```
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nProse.\n\n```lua\nlocal x = 1\nlocal y = 2\nassert(false)\n```\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
    let epilog = prompt
        .entry()
        .expect("has sections")
        .epilog()
        .expect("epilog must exist");

    assert_eq!(epilog.source_line().get(), 13);

    let lua = mlua::Lua::new();
    let function = epilog.load(&lua).expect("bytecode must load");
    let raw_error = function
        .call::<()>(())
        .expect_err("assert(false) must fail");
    let mapped = epilog.map_runtime_error(&raw_error);
    let msg = mapped.to_string();
    assert!(
        msg.contains(":15:"),
        "error must contain absolute line 15: {msg}"
    );
}

#[test]
fn shared_library_source_line_is_correct() {
    // Lines:
    //  1: ---
    //  2: name: x
    //  3: description: d
    //  4: ---
    //  5: (empty)
    //  6: # T
    //  7: (empty)
    //  8: ```lua shared <- shared opens
    //  9: function f()  <- source_line = 9
    // 10: end
    // 11: ```
    // 12: (empty)
    // 13: ## S
    // 14: (empty)
    // 15: p
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n```lua shared\nfunction f()\nend\n```\n\n## S\n\np\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).expect("prompt must parse");
    let replay = prompt.replay.as_ref().expect("replay must exist");
    assert_eq!(replay.source_line().get(), 9, "shared Lua starts on line 9");
}

#[test]
fn frontmatter_parses_input_and_output() {
    let source = concat!(
        "---\n",
        "name: test\n",
        "description: d\n",
        "promptforge: 1\n",
        "input:\n",
        "  path: paper.md\n",
        "  description: The input paper\n",
        "output:\n",
        "  path: report.md\n",
        "  description: The output report\n",
        "---\n\n",
        "# Title\n\n## Only\n\ndone\n",
    );
    let prompt = Prompt::parse(source, "test", &NullObserver).unwrap();
    let fm = prompt.frontmatter();
    let input = fm.input().expect("input declared");
    assert_eq!(input.path(), "paper.md");
    assert_eq!(input.description(), "The input paper");
    let output = fm.output().expect("output declared");
    assert_eq!(output.path(), "report.md");
    assert_eq!(output.description(), "The output report");
}

#[test]
fn frontmatter_without_input_output_still_parses() {
    let source = concat!(
        "---\n",
        "name: simple\n",
        "description: no files\n",
        "promptforge: 1\n",
        "---\n\n",
        "# Title\n\n## Only\n\ndone\n",
    );
    let prompt = Prompt::parse(source, "test", &NullObserver).unwrap();
    assert!(prompt.frontmatter().input().is_none());
    assert!(prompt.frontmatter().output().is_none());
}

#[test]
fn off_walk_marker_marks_section_and_content_below_parses() {
    // A `---` rule as a section's first content (only whitespace before it)
    // takes the section off the walk; the content below the marker parses
    // normally.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n---\n\n```lua\nvar.x = 1\n```\n\nBelow the marker.\n\n## Plain\n\np\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_off_walk());
    assert_eq!(
        section.prologue().map(LuaProgram::source),
        Some("var.x = 1")
    );
    assert_eq!(section.prose(), "Below the marker.");
    assert!(!prompt.sections[1].is_off_walk());
}

#[test]
fn comment_rule_excludes_everything_below_it() {
    // A `---` rule after executable content is a comment boundary: no Lua
    // below it compiles (the malformed fence would fail compilation if it
    // did) and no prose below it reaches the model. A heading below the rule
    // still splits sections.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nLive prose.\n\n```lua\nvar.x = 1\n```\n\n---\n\nDead prose.\n\n```lua\nnot compiled =\n```\n\n## After\n\nafter prose\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert_eq!(prompt.sections.len(), 2);
    let section = &prompt.sections[0];
    assert!(!section.is_off_walk());
    assert_eq!(section.blocks().len(), 2);
    assert_eq!(section.prose(), "Live prose.");
    assert_eq!(section.epilog().map(LuaProgram::source), Some("var.x = 1"));
    assert_eq!(prompt.sections[1].name, "After");
    assert_eq!(prompt.sections[1].prose(), "after prose");
}

#[test]
fn off_walk_marker_composes_with_a_later_comment_rule() {
    // The two roles compose: an off-walk marker at the top, then a Lua fence
    // and prose, then a blank line and a second rule starting a comment
    // region.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n---\n\n```lua\nvar.x = 1\n```\n\nLive prose.\n\n---\n\nDead prose.\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_off_walk());
    assert_eq!(section.blocks().len(), 2);
    assert_eq!(
        section.prologue().map(LuaProgram::source),
        Some("var.x = 1")
    );
    assert_eq!(section.prose(), "Live prose.");
}

#[test]
fn off_walk_list_section_parses_items_below_the_marker() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Items\n\n---\n\n- alpha\n- beta\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_off_walk());
    assert!(section.is_list_only());
    assert_eq!(section.items(), ["alpha", "beta"]);
}

#[test]
fn comment_rule_ends_list_items() {
    // List items parse only from the executable content above the boundary.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Items\n\n- alpha\n\n---\n\n- beta\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(!section.is_off_walk());
    assert_eq!(section.items(), ["alpha"]);
}

#[test]
fn h1_rule_is_a_comment_boundary() {
    // The off-walk role is meaningless on the H1 (no walk visits it), so a
    // rule there is simply a comment boundary: a `lua shared` fence below it
    // is inert and the description text comes from above the rule.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\nDescription above.\n\n---\n\n```lua shared\nlocal hidden = 1\n```\n\nBelow prose.\n\n## S\n\np\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert!(prompt.replay.is_none());
    assert_eq!(prompt.description_text, "Description above.");
    assert_eq!(
        prompt.h1_blocks,
        vec![Block::Prose {
            text: "Description above.".to_owned(),
            loop_capable: true,
        }]
    );
}

#[test]
fn rule_inside_a_fenced_code_block_is_not_a_marker() {
    // Pulldown reports only a genuine thematic break: a `---` inside a
    // fenced code block is code, not a rule.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nLive prose.\n\n```text\n---\n```\n\nAlso live.\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(!section.is_off_walk());
    assert!(section.prose().contains("Also live."));
    assert!(section.prose().contains("---"));

    // With a leading marker, a fenced `---` still is not the comment
    // boundary.
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n---\n\n```text\n---\n```\n\nLive.\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_off_walk());
    assert!(section.prose().contains("Live."));
    assert!(section.prose().contains("---"));
}

#[test]
fn setext_underline_is_not_a_rule() {
    // Found debt, pinned as-is: a prose line immediately followed by `---`
    // with no blank line is a CommonMark setext H2 underline, not a rule, so
    // the heading scanner reads it as a new section. The blank line before
    // the marker is required.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nSome prose\n---\n\nMore prose\n";
    let prompt = Prompt::parse(src, "test", &NullObserver).unwrap();
    assert_eq!(prompt.sections.len(), 2);
    assert_eq!(prompt.sections[0].name, "S");
    assert!(!prompt.sections[0].is_off_walk());
    assert_eq!(prompt.sections[1].name, "Some prose");
    assert!(!prompt.sections[1].is_off_walk());
    assert_eq!(prompt.sections[1].prose(), "More prose");
}
