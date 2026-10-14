//! Shared test support for the facade suite: a prompt builder, preloaded
//! answers for the effects a run issues, and [`Scenario`], which holds a
//! test's setup as data and drives its run.

use std::error::Error;
use std::num::NonZeroU32;

use promptforge::model::{
    CompletionError, CompletionErrorKind, CompletionResult, ModelDescriptor, ModelId, ThinkingMode,
    ToolCall,
};
use promptforge::tools::{ToolError, ToolOutput};
use serde_json::Value;

mod scenario;

pub(super) use scenario::{Prepared, Scenario};

/// What every facade test returns.
pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// Writes a note to the store and reads it back.
pub(super) const NOTE: &str = "
    store.write('note.md', 'hello')
    return store.read('note.md')
";

/// Answers a tool call from the call's arguments.
type Handler = Box<dyn Fn(&Value) -> Result<ToolOutput, ToolError>>;

/// A tool a scenario registers: its full id, and its handler.
pub(super) struct Tool {
    pub(super) id: &'static str,
    pub(super) handler: Handler,
}

impl Tool {
    /// The tool `id`, which answers every call with `text` as untrusted
    /// output.
    pub(super) fn untrusted(id: &'static str, text: &str) -> Self {
        let text = text.to_owned();
        Self {
            id,
            handler: Box::new(move |_| Ok(ToolOutput::untrusted(text.clone()))),
        }
    }
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

/// How the model answers one chat effect: with a completion, or as a
/// failed round.
pub(super) enum Reply {
    Completion(CompletionResult),
    Failure(CompletionError),
}

/// A scripted model reply of plain text.
pub(super) fn text(reply: &str) -> Reply {
    Reply::Completion(CompletionResult::Text(reply.to_owned()))
}

/// A scripted model reply that calls the tool `name` once, by its wire
/// name, with `args`.
pub(super) fn calls(name: &str, args: Value) -> TestResult<Reply> {
    let call = ToolCall::from_parts("call_1", name, args)?;
    Ok(Reply::Completion(CompletionResult::ToolCalls(vec![call])))
}

/// A scripted model reply that fails the round as `kind`.
pub(super) fn fail(kind: CompletionErrorKind) -> Reply {
    Reply::Failure(CompletionError::new(kind, kind.phrase()))
}

/// `example/text/shout`: answers with its `text` argument in capital
/// letters, as trusted output.
pub(super) fn shout() -> Tool {
    Tool {
        id: "example/text/shout",
        handler: Box::new(|args| match args.get("text").and_then(Value::as_str) {
            Some(text) => Ok(ToolOutput::trusted(text.to_uppercase())),
            None => Err(ToolError::message(format!(
                "shout needs a string `text` argument; got {args}"
            ))),
        }),
    }
}

/// `gateway/canned`, the model every scenario sets before prepare unless
/// it names its own. It fills only roles a prompt declares, so a prompt
/// without roles ignores it.
pub(super) fn canned() -> TestResult<ModelDescriptor> {
    let window = NonZeroU32::new(8_192).ok_or("a context window is never zero")?;
    Ok(ModelDescriptor::new(
        ModelId::gateway("canned")?,
        "Replies from the scenario's script",
        window,
        ThinkingMode::Never,
    ))
}
