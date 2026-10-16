//! Pending-Markdown tests: what binds to a Lua fence, and what a heading, a
//! fence, or a thematic break resets.

use super::*;

#[test]
fn leading_break_resets_prose_and_content_below_parses() {
    // A `---` rule as a section's first content only resets the pending
    // buffer, and the content below the break parses and runs normally.
    let body = "---\n\n```lua\nvar.x = 1\n```\n\nBelow the break.";
    let src = PromptSource::new().prose("S", body);
    let prompt = parse_ok(&src.prose("Plain", "p").build());
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 3);
    assert!(matches!(
        &section.blocks()[0],
        Block::Prose { text } if text.as_str().is_empty()
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
    let body = "Intro.\n\n```lua\nvar.x = 1\n```\n\n---\n\nNotes.\n\n```lua\nvar.y = 2\n```";
    let src = PromptSource::new().prose("S", body);
    let prompt = parse_ok(&src.prose("After", "after prose").build());
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
    let body = "First draft.\n\n---\n\nSecond draft.\n\n---\n\nFinal prose.";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 1);
    assert_eq!(section.prose(), "Final prose.");
}

#[test]
fn list_items_below_a_leading_break_parse() {
    let body = "---\n\n- alpha\n- beta";
    let prompt = parse_ok(&PromptSource::new().prose("Items", body).build());
    let section = &prompt.sections[0];
    assert!(section.is_list_only());
    assert_eq!(section.items(), ["alpha", "beta"]);
}

#[test]
fn a_break_resets_list_item_capture() {
    // List items parse from the pending buffer: markers above a break are
    // commentary, and only the markers below the last break parse.
    let body = "- alpha\n\n---\n\n- beta";
    let prompt = parse_ok(&PromptSource::new().prose("Items", body).build());
    let section = &prompt.sections[0];
    assert_eq!(section.items(), ["beta"]);
}

#[test]
fn h1_break_resets_prose_and_lua_fence_below_stays_live() {
    // The H1 follows the same reset rule: Markdown above the break is
    // commentary excluded from the description, and a `lua` fence below
    // the break is live because a break makes nothing reader-only.
    let h1 = "Description above.\n\n---\n\n```lua\nlocal live = 1\n```\n\nBelow prose.";
    let prompt = parse_ok(&PromptSource::new().h1_prose(h1).prose("S", "p").build());
    assert_eq!(prompt.description_text, "Below prose.");
    assert!(matches!(
        prompt.h1_blocks.as_slice(),
        [Block::Prose { text: above }, Block::Lua(program), Block::Prose { text: below }]
            if above.as_str().is_empty()
                && program.source() == "local live = 1"
                && below == "Below prose."
    ));
}

#[test]
fn rule_inside_a_fenced_code_block_is_not_a_marker() {
    // Pulldown reports only a genuine thematic break: a `---` inside a
    // fenced code block is code, not a rule, so it resets nothing.
    let body = "Live prose.\n\n```text\n---\n```\n\nAlso live.";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
    let section = &prompt.sections[0];
    assert!(section.prose().contains("Also live."));
    assert!(section.prose().contains("---"));

    // With a leading break, a fenced `---` still is not a reset point.
    let body = "---\n\n```text\n---\n```\n\nLive.";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
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
    let body = "Some prose\n---\n\nMore prose";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
    assert_eq!(prompt.sections.len(), 2);
    assert_eq!(prompt.sections[0].name, "S");
    assert_eq!(prompt.sections[1].name, "Some prose");
    assert_eq!(prompt.sections[1].prose(), "More prose");
}

#[test]
fn pending_markdown_binds_to_the_following_lua_fence() {
    // Capture: Markdown after a section heading accumulates as the pending
    // buffer that the next ordinary Lua fence consumes.
    let body = "Gather the facts.\n\n```lua\nreturn 1\n```";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
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
    let src = PromptSource::new().prose("A", "Prose for A.");
    let prompt = parse_ok(&src.section("B", "return 1").build());
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
    let body = "First.\n\n```lua\nvar.a = 1\n```\n\nSecond.\n\n```lua\nvar.b = 2\n```";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
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
    let body = "Draft notes.\n\n---\n\nAsk the question.\n\n```lua\nreturn 1\n```";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 2);
    match &section.blocks()[0] {
        Block::Prose { text } => {
            assert_eq!(text, "Ask the question.");
            assert!(!text.as_str().contains("Draft notes."));
            assert!(!text.as_str().contains("---"));
        }
        other => panic!("expected pending prose, got {other:?}"),
    }
    assert!(matches!(&section.blocks()[1], Block::Lua(_)));
}

#[test]
fn per_fence_commentary_is_excluded_by_a_break() {
    // Between fences, a break drops commentary on the previous step so only
    // the Markdown below the last break is pending for the next fence.
    let body = "```lua\nvar.a = 1\n```\n\nNotes on step one.\n\n---\n\nPending for step two.\n\n```lua\nvar.b = 2\n```";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
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
    let body = "```lua\nreturn 1\n```\n\nTrailing notes for the reader.";
    let prompt = parse_ok(&PromptSource::new().prose("S", body).build());
    let section = &prompt.sections[0];
    assert_eq!(section.blocks().len(), 2);
    assert!(matches!(&section.blocks()[0], Block::Lua(_)));
    assert_eq!(section.prose(), "Trailing notes for the reader.");
}

#[test]
fn prose_without_a_following_fence_is_not_an_error() {
    // Prose with no Lua fence at all, and prose left pending at a section's
    // end, both parse cleanly.
    parse_ok(&PromptSource::new().prose("S", "Just prose.").build());

    let pending_at_end = "```lua\nreturn 1\n```\n\nDiscarded at section end.";
    parse_ok(&PromptSource::new().prose("S", pending_at_end).build());
}
