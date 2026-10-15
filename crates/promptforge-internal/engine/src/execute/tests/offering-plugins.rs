//! Tests for `plugins.offer` across section entries: an offered Plugin's
//! tools reach the rounds of the section entry that offered it, through
//! `require` too, and no other entry's.

use super::*;

/// The section a round ran for: the text of the user message its list
/// starts with.
fn round_section(messages: &[Value]) -> String {
    messages
        .iter()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .expect("each round's list starts with its section's name")
        .to_owned()
}

#[test]
fn an_offered_plugin_reaches_only_the_rounds_of_the_section_entry_that_offered_it() {
    let round = |section: &str| {
        format!(
            "local history = messages.new()\n\
             history:user('{section}')\n\
             models.loop(history)\n"
        )
    };
    let source = format!(
        "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
         # Plugins\n\n\
         ```lua\nplugins.offer('kit')\n{h1}```\n\n\
         ## A\n\n\
         ```lua\n\
         tools.offer('kit/a')\n\
         require '## offers'\n\
         plugins.offer('kit')\n\
         {a}\
         call('## Called')\n\
         tasks.join({{ tasks.spawn('## Worker') }})\n\
         ```\n\n\
         ## B\n\n\
         ```lua\n{b}return 'ok'\n```\n\n\
         ## offers\n\n\
         ```lua\nplugins.offer('web')\n```\n\n\
         ## Called\n\n\
         ```lua\n{called}return 'called'\n```\n\n\
         ## Worker\n\n\
         ```lua\nrequire '## offers'\n{worker}return 'worker'\n```\n",
        h1 = round("Plugins"),
        a = round("A"),
        b = round("B"),
        called = round("Called"),
        worker = round("Worker"),
    );
    let gateway = ScriptedChat::new(vec![text("done"); 5]);
    let rig = Rig::run(
        catalog_scenario(source)
            .tool(offered_tool("kit/a"))
            .tool(offered_tool("kit/b"))
            .tool(offered_tool("web/fetch"))
            .chat(&gateway),
    );
    assert_eq!(rig.trace().text(), "ok", "the walk ends at B");
    let records = rig.issued();
    let (advertised, calls) = rounds_and_calls(records);
    assert!(calls.is_empty(), "no tool is called: {calls:?}");
    let mut by_section: Vec<(String, Vec<String>)> = records
        .iter()
        .filter_map(|record| match record {
            EffectRecord::Chat { messages, .. } => Some(round_section(messages)),
            _ => None,
        })
        .zip(advertised)
        .collect();
    by_section.sort();
    let names = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        by_section,
        vec![
            ("A".to_owned(), names(&["kit_a", "web_fetch", "kit_b"])),
            ("B".to_owned(), names(&[])),
            ("Called".to_owned(), names(&[])),
            ("Plugins".to_owned(), names(&["kit_a", "kit_b"])),
            ("Worker".to_owned(), names(&["web_fetch"])),
        ],
        "each section entry's round advertises only its own offers, \
         a required section's offers included"
    );
}
