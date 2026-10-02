//! A capability's prelude through preparation: the prelude its `create`
//! returns reaches every section VM of the prepared run, and each tool
//! call a prelude function makes is logged as a `ToolCall` effect
//! attributed to the calling script, its execution, and its section.

use super::*;

use harness_log::{RecordFilter, RecordKind};
use serde_json::json;

/// The prelude the speaker contributes: one table global whose function
/// calls the speaker's echo tool by its full id.
const SPEAKER_PRELUDE: &str = "speaker = {}\n\
    function speaker.say(value)\n\
      return tools.call('tests/speaker/echo', { value = value })\n\
    end\n";

/// A prompt declaring the speaker, binding none of its tools, and calling
/// its prelude from two sections; the second also tries to replace the
/// prelude's function and reports whether the table refused.
const SPEAKS: &str = "---\nname: speaks\ndescription: d\npromptforge: 0\n\
    capabilities:\n  - tests/speaker\n---\n\n# Title\n\n\
    ## First\n\n```lua\nspeaker.say('one')\n```\n\n\
    ## Second\n\n```lua\n\
    local sealed = not pcall(function() speaker.say = nil end)\n\
    return speaker.say('two') .. '|' .. tostring(sealed)\n```\n";

/// A fixture capability contributing the echo tool under its own id and
/// a prelude that calls it.
struct Speaker {
    id: CapabilityId,
}

impl Capability for Speaker {
    fn id(&self) -> &CapabilityId {
        &self.id
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Capability trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "Speaks through its echo tool from a prelude."
    }

    fn create(&self, _services: &RunServices) -> Result<Contribution, CapabilityError> {
        Ok(Contribution {
            tools: vec![Arc::new(Echo {
                id: ToolId::parse("tests/speaker/echo").unwrap(),
            })],
            prelude: Some(SPEAKER_PRELUDE.to_owned()),
        })
    }
}

/// The logged record of the speaker's echo called with `value` from
/// `section` by the section's script.
fn script_call(value: &str, section: &str) -> serde_json::Value {
    json!({
        "ToolCall": {
            "tool": "tests/speaker/echo",
            "alias": "tests/speaker/echo",
            "args": { "value": value },
            "origin": { "execution": "session-1", "section": section, "caller": "script" },
        }
    })
}

#[tokio::test]
async fn a_preludes_tool_calls_are_logged_as_script_calls_from_the_section_that_made_each() {
    let dir = tempfile::tempdir().unwrap();
    let log = log().await;
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(Speaker {
            id: CapabilityId::parse("tests/speaker").unwrap(),
        }))
        .unwrap();
    let prepared = prepare_run(
        &prompt_file(dir.path(), SPEAKS),
        "",
        services(&log, Some(Arc::new(registry))),
    )
    .await
    .unwrap();
    let run_id = prepared.run_id;
    let outcome = drive_run(
        prepared.run,
        prepared.performers,
        Arc::clone(&log),
        run_id,
        CancelHandle::new(),
        |_event| {},
    )
    .await
    .unwrap();
    assert_eq!(
        completed(outcome),
        "two|true",
        "both sections reached the prelude's function, and its table refused a write"
    );

    let effects: Vec<serde_json::Value> = log
        .lock()
        .await
        .records(run_id, RecordFilter::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|stored| stored.record.kind == RecordKind::Effect)
        .map(|stored| stored.record.payload)
        .collect();
    assert_eq!(
        effects,
        [script_call("one", "First"), script_call("two", "Second")],
        "each prelude call is one logged ToolCall effect naming its caller and section"
    );
}
