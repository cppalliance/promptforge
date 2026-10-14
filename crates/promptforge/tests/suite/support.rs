//! Shared test support for the facade suite: a prompt builder, preloaded
//! answers for the effects a run issues, and [`Scenario`], which holds a
//! test's setup as data and drives its run.

use std::error::Error;
use std::num::NonZeroU32;

use promptforge::RunContext;
use promptforge::model::{CompletionResult, ModelDescriptor, ModelId, ThinkingMode};
use promptforge::timestamp::Timestamp;
use promptforge::tools::{ToolError, ToolOutput};
use serde_json::Value;

mod scenario;

pub(super) use scenario::Scenario;

/// What every facade test returns.
pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// A tool a scenario registers: its full id, and the handler that answers
/// each call from the call's arguments.
pub(super) struct Tool {
    pub(super) id: &'static str,
    pub(super) handler: fn(&Value) -> Result<ToolOutput, ToolError>,
}

/// A [`RunContext`] for the run `name` under the fixed seed and clock
/// every fixture shares: the Engine takes both from its caller, and no
/// fixture here asserts on the nonce or `sys.when`.
pub(super) fn context(name: impl Into<String>) -> RunContext {
    RunContext::new(name, 1, Timestamp::UNIX_EPOCH)
}

/// The prompt `greeter`: fixed frontmatter, then the `front` lines, then
/// `# Greeter` and one `lua`-fenced `## heading` section per entry. Every
/// line is trimmed, so `front` and each body can be indented raw strings.
pub(super) fn prompt(front: &str, sections: &[(&str, &str)]) -> String {
    let mut source = String::from(
        "---\nname: greeter\ndescription: A prompt the facade suite builds.\npromptforge: 0\n",
    );
    push_trimmed(&mut source, front);
    source.push_str("---\n\n# Greeter\n");
    for (heading, lua) in sections {
        source.push_str("\n## ");
        source.push_str(heading);
        source.push_str("\n\n```lua\n");
        push_trimmed(&mut source, lua);
        source.push_str("```\n");
    }
    source
}

/// [`prompt`] with no extra frontmatter and the one section `## Greet`.
pub(super) fn greeter(lua: &str) -> String {
    prompt("", &[("Greet", lua)])
}

fn push_trimmed(source: &mut String, text: &str) {
    for line in text.trim().lines() {
        source.push_str(line.trim());
        source.push('\n');
    }
}

/// A scripted model reply of plain text.
pub(super) fn text(reply: &str) -> CompletionResult {
    CompletionResult::Text(reply.to_owned())
}

/// `example/text/shout`: answers with its `text` argument in capital
/// letters, as trusted output.
pub(super) fn shout() -> Tool {
    Tool {
        id: "example/text/shout",
        handler: |args| match args.get("text").and_then(Value::as_str) {
            Some(text) => Ok(ToolOutput::trusted(text.to_uppercase())),
            None => Err(ToolError::message(format!(
                "shout needs a string `text` argument; got {args}"
            ))),
        },
    }
}

/// `gateway/canned`, the model every scenario sets before prepare. It
/// fills only roles a prompt declares, so a prompt without roles ignores
/// it.
pub(super) fn canned() -> TestResult<ModelDescriptor> {
    let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
    Ok(ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Replies from the scenario's script",
        window,
        ThinkingMode::Never,
    ))
}
