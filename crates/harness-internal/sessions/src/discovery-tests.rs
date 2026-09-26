//! Tests for agent discovery in the agents directory and the built-in chat fallback.

use super::*;

#[test]
fn discovery_lists_sorted_markdown_stems_and_tolerates_a_missing_dir() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(dir.path().join("zeta.md"), "# zeta").expect("seed zeta");
    std::fs::write(dir.path().join("alpha.md"), "# alpha").expect("seed alpha");
    std::fs::write(dir.path().join("notes.txt"), "not an agent").expect("seed noise");
    std::fs::create_dir(dir.path().join("nested.md")).expect("seed a decoy directory");
    assert_eq!(
        discover_agents(dir.path()),
        vec!["alpha".to_owned(), "chat".to_owned(), "zeta".to_owned()],
        "discovery lists .md file stems plus the built-in chat, sorted, \
         and skips everything else"
    );
    assert_eq!(
        discover_agents(&dir.path().join("missing")),
        vec!["chat".to_owned()],
        "a missing agents directory still offers the built-in chat rather than failing"
    );
}

#[test]
fn the_built_in_chat_is_always_offered_and_a_dir_file_shadows_its_source() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    assert_eq!(
        discover_agents(dir.path()),
        vec!["chat".to_owned()],
        "an empty agents directory still offers the built-in chat"
    );
    assert_eq!(
        agent_source(dir.path(), "chat").expect("the built-in serves"),
        AgentSource::Markdown(BUILTIN_CHAT_SOURCE.to_owned()),
        "with no directory file, the embedded source is what launches"
    );

    std::fs::write(dir.path().join("chat.md"), "# shadowed").expect("seed the shadow");
    assert_eq!(
        discover_agents(dir.path()),
        vec!["chat".to_owned()],
        "a directory chat.md lists once, never beside the built-in"
    );
    assert_eq!(
        agent_source(dir.path(), "chat").expect("the shadow reads"),
        AgentSource::Markdown("# shadowed".to_owned()),
        "a directory chat.md shadows the embedded source"
    );

    assert_eq!(
        agent_source(dir.path(), "ghost")
            .expect_err("only the built-in name falls back to embedded source")
            .kind(),
        io::ErrorKind::NotFound,
        "a non-built-in name surfaces its filesystem error"
    );
}

#[test]
fn an_unreadable_chat_md_surfaces_its_error_rather_than_the_built_in() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // A directory named chat.md cannot be read as a file on any
    // platform, and its failure is never NotFound - the one kind
    // that falls back to the embedded source.
    std::fs::create_dir(dir.path().join("chat.md")).expect("seed the unreadable shadow");
    agent_source(dir.path(), "chat").expect_err(
        "an existing chat.md that cannot be read surfaces its error; \
         silently serving the built-in would mask the operator's own file",
    );
}
