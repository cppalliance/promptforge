//! Pending-Markdown tests: what binds to a Lua fence, and what a heading, a
//! fence, or a thematic break resets.

use super::*;

#[test]
fn leading_break_resets_prose_and_content_below_parses() {
    // A `---` rule as a section's first content only resets the pending
    // buffer, and the content below the break parses and runs normally.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n---\n\n```lua\nvar.x = 1\n```\n\nBelow the break.\n\n## Plain\n\np\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 3);
    assert!(matches!(
        &section.blocks()[0],
        Block::Prose { text } if text.is_empty()
    ));
    assert!(matches!(
        &section.blocks()[1],
        Block::Lua(program) if program.source() == "var.x = 1"
    ));
    assert_eq!(section.prose(), "Below the break.");
    assert_eq!(prompt.sections[1].name, "Plain");
}

#[test]
fn a_break_never_terminates_section_content() {
    // A thematic break resets prose capture only: the fence below it still
    // compiles (an invalid fence would fail the parse) and prose below it
    // is ordinary pending Markdown. A heading below the break still splits
    // sections.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nIntro.\n\n```lua\nvar.x = 1\n```\n\n---\n\nNotes.\n\n```lua\nvar.y = 2\n```\n\n## After\n\nafter prose\n";
    let prompt = parse(src).unwrap();
    assert_eq!(prompt.sections.len(), 2);
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 4);
    assert_eq!(section.prose(), "Notes.");
    assert!(matches!(
        &section.blocks()[3],
        Block::Lua(program) if program.source() == "var.y = 2"
    ));
    assert_eq!(prompt.sections[1].name, "After");
    assert_eq!(prompt.sections[1].prose(), "after prose");
}

#[test]
fn multiple_breaks_each_reset_the_pending_buffer() {
    // Any number of breaks may clear pending prose; only the Markdown below
    // the last break remains pending.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nFirst draft.\n\n---\n\nSecond draft.\n\n---\n\nFinal prose.\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 1);
    assert_eq!(section.prose(), "Final prose.");
}

#[test]
fn list_items_below_a_leading_break_parse() {
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Items\n\n---\n\n- alpha\n- beta\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert!(section.is_list_only());
    assert_eq!(section.items(), ["alpha", "beta"]);
}

#[test]
fn a_break_resets_list_item_capture() {
    // List items parse from the pending buffer: markers above a break are
    // commentary, and only the markers below the last break parse.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## Items\n\n- alpha\n\n---\n\n- beta\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.items(), ["beta"]);
}

#[test]
fn h1_break_resets_prose_and_shared_fence_below_stays_live() {
    // The H1 follows the same reset rule: Markdown above the break is
    // commentary excluded from the description, and a `lua shared` fence
    // below the break is live because a break makes nothing reader-only.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\nDescription above.\n\n---\n\n```lua shared\nlocal shared = 1\n```\n\nBelow prose.\n\n## S\n\np\n";
    let prompt = parse(src).unwrap();
    assert_eq!(
        prompt.replay.as_ref().map(LuaProgram::source),
        Some("local shared = 1")
    );
    assert_eq!(prompt.description_text, "Below prose.");
    assert_eq!(
        prompt.h1_blocks,
        vec![Block::Prose {
            text: "Below prose.".to_owned(),
        }]
    );
}

#[test]
fn rule_inside_a_fenced_code_block_is_not_a_marker() {
    // Pulldown reports only a genuine thematic break: a `---` inside a
    // fenced code block is code, not a rule, so it resets nothing.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nLive prose.\n\n```text\n---\n```\n\nAlso live.\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert!(section.prose().contains("Also live."));
    assert!(section.prose().contains("---"));

    // With a leading break, a fenced `---` still is not a reset point.
    let src =
        "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n---\n\n```text\n---\n```\n\nLive.\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
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
    let prompt = parse(src).unwrap();
    assert_eq!(prompt.sections.len(), 2);
    assert_eq!(prompt.sections[0].name, "S");
    assert_eq!(prompt.sections[1].name, "Some prose");
    assert_eq!(prompt.sections[1].prose(), "More prose");
}

#[test]
fn pending_markdown_binds_to_the_following_lua_fence() {
    // Capture: Markdown after a section heading accumulates as the pending
    // buffer that the next ordinary Lua fence consumes.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nGather the facts.\n\n```lua\nreturn 1\n```\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 2);
    assert!(matches!(
        &section.blocks()[0],
        Block::Prose { text } if text == "Gather the facts."
    ));
    assert!(matches!(
        &section.blocks()[1],
        Block::Lua(program) if program.source() == "return 1"
    ));
}

#[test]
fn a_heading_resets_the_pending_buffer() {
    // Reset at headings: a section starts with an empty buffer; prose from
    // the previous section never leaks into it.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## A\n\nProse for A.\n\n## B\n\n```lua\nreturn 1\n```\n";
    let prompt = parse(src).unwrap();
    assert_eq!(prompt.sections[0].prose(), "Prose for A.");
    assert!(matches!(
        prompt.sections[1].blocks(),
        [Block::Lua(program)] if program.source() == "return 1"
    ));
}

#[test]
fn a_lua_fence_consumes_the_pending_buffer() {
    // Reset at Lua fences: each fence is preceded by exactly the Markdown
    // accumulated since the previous fence.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nFirst.\n\n```lua\nvar.a = 1\n```\n\nSecond.\n\n```lua\nvar.b = 2\n```\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 4);
    assert!(matches!(
        &section.blocks()[0],
        Block::Prose { text } if text == "First."
    ));
    assert!(matches!(
        &section.blocks()[1],
        Block::Lua(program) if program.source() == "var.a = 1"
    ));
    assert!(matches!(
        &section.blocks()[2],
        Block::Prose { text } if text == "Second."
    ));
    assert!(matches!(
        &section.blocks()[3],
        Block::Lua(program) if program.source() == "var.b = 2"
    ));
}

#[test]
fn a_thematic_break_resets_the_pending_buffer() {
    // Reset at thematic breaks: commentary above the break is excluded and
    // the break itself is never part of the prose.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nDraft notes.\n\n---\n\nAsk the question.\n\n```lua\nreturn 1\n```\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 2);
    match &section.blocks()[0] {
        Block::Prose { text } => {
            assert_eq!(text, "Ask the question.");
            assert!(!text.contains("Draft notes."));
            assert!(!text.contains("---"));
        }
        other => panic!("expected pending prose, got {other:?}"),
    }
    assert!(matches!(&section.blocks()[1], Block::Lua(_)));
}

#[test]
fn per_fence_commentary_is_excluded_by_a_break() {
    // Between fences, a break drops commentary on the previous step so only
    // the Markdown below the last break is pending for the next fence.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nvar.a = 1\n```\n\nNotes on step one.\n\n---\n\nPending for step two.\n\n```lua\nvar.b = 2\n```\n";
    let prompt = parse(src).unwrap();
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 3);
    assert!(matches!(
        &section.blocks()[0],
        Block::Lua(program) if program.source() == "var.a = 1"
    ));
    assert!(matches!(
        &section.blocks()[1],
        Block::Prose { text } if text == "Pending for step two."
    ));
    assert!(matches!(
        &section.blocks()[2],
        Block::Lua(program) if program.source() == "var.b = 2"
    ));
}

#[test]
fn trailing_commentary_after_the_last_fence_is_inert() {
    // Markdown after the final Lua fence is inert trailing commentary: it
    // parses without an unpaired-prose error and stays an ordinary block.
    let src = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nreturn 1\n```\n\nTrailing notes for the reader.\n";
    let prompt =
        parse(src).expect("trailing commentary must parse without an unpaired-prose error");
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 2);
    assert!(matches!(&section.blocks()[0], Block::Lua(_)));
    assert_eq!(section.prose(), "Trailing notes for the reader.");
}

#[test]
fn prose_without_a_following_fence_is_not_an_error() {
    // Prose with no Lua fence at all, and prose left pending at a section's
    // end, both parse cleanly.
    let prose_only = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\nJust prose.\n";
    parse(prose_only).expect("prose without any fence must parse");

    let pending_at_end = "---\nname: x\ndescription: d\n---\n\n# T\n\n## S\n\n```lua\nreturn 1\n```\n\nDiscarded at section end.\n";
    parse(pending_at_end).expect("unconsumed pending Markdown must parse");
}
