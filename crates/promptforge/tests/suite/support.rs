//! Shared test support for the facade suite: a prompt builder, scripted
//! replies and tools, and the [`Scenario`] that drives a run.

use std::error::Error;
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use promptforge::model::{
    CompletionError, CompletionErrorKind, CompletionResult, ModelDescriptor, ModelId, ThinkingMode,
    ToolCall,
};
use promptforge::tools::{ToolDescriptor, ToolError, ToolId, ToolOutput};
use serde_json::Value;

mod scenario;

pub(super) use scenario::{Prepared, Scenario};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

type Handler = Box<dyn Fn(&Value) -> Result<ToolOutput, ToolError>>;

/// A tool a scenario registers under its full id, not its wire name.
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

pub(super) fn greeter(lua: &str) -> String {
    prompt("", &[("Greet", lua)])
}

pub(super) fn note(value: &str) -> String {
    greeter(&format!(
        "store.write('note.md', {value})\nreturn store.read('note.md')"
    ))
}

fn push_trimmed(source: &mut String, text: &str) {
    for line in text.trim().lines() {
        source.push_str(line.trim());
        source.push('\n');
    }
}

pub(super) enum Reply {
    Completion(CompletionResult),
    Failure(CompletionError),
}

pub(super) fn text(reply: &str) -> Reply {
    Reply::Completion(CompletionResult::Text(reply.to_owned()))
}

/// A scripted reply that calls the tool whose wire name is `name`.
pub(super) fn calls(name: &str, args: Value) -> TestResult<Reply> {
    let call = ToolCall::from_parts("call_1", name, args)?;
    Ok(Reply::Completion(CompletionResult::ToolCalls(vec![call])))
}

pub(super) fn fail(kind: CompletionErrorKind) -> Reply {
    Reply::Failure(CompletionError::new(kind, kind.phrase()))
}

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

/// The model every scenario sets before prepare unless it names its own.
/// It fills only roles a prompt declares, so a prompt without roles
/// ignores it.
pub(super) fn canned() -> TestResult<ModelDescriptor> {
    model("canned", 8_192, ThinkingMode::Never)
}

pub(super) fn model(
    name: &str,
    window: u32,
    thinking: ThinkingMode,
) -> TestResult<ModelDescriptor> {
    Ok(ModelDescriptor::new(
        ModelId::gateway(name)?,
        "A model the facade suite describes.",
        NonZeroU32::new(window).ok_or("a context window is never zero")?,
        thinking,
    ))
}

pub(super) fn descriptor(id: &str, schema: Value) -> TestResult<ToolDescriptor> {
    Ok(ToolDescriptor::new(
        ToolId::parse(id)?,
        "A tool the facade suite describes.",
        schema,
    ))
}

/// Tells apart roots one process makes; nextest runs each test in its own
/// process, so the process id tells those apart.
static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// A real-filesystem folder at
/// `<temp>/promptforge-tests/<crate>/<label>-<pid>-<n>`, removed on drop.
/// Its name reads no clock and no random generator.
pub(super) struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    /// Makes the folder for the test named `label`, first removing anything
    /// an earlier process with the same id left at that path.
    #[track_caller]
    pub(super) fn new(label: &str) -> Self {
        let n = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join("promptforge-tests")
            .join(env!("CARGO_PKG_NAME"))
            .join(format!("{label}-{}-{n}", std::process::id()));
        let cleared = match fs::symlink_metadata(&path) {
            Ok(found) if found.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        };
        if let Err(err) = cleared.and_then(|()| fs::create_dir_all(&path)) {
            panic!("the temp root {} must be created: {err}", path.display());
        }
        Self { path }
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_dir_all(&self.path)
            && !std::thread::panicking()
        {
            panic!(
                "the temp root {} must be removed: {err}",
                self.path.display()
            );
        }
    }
}

#[test]
fn temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder() -> TestResult {
    let label = "temp_roots_with_one_label_get_distinct_paths_and_drop_removes_the_folder";
    let (first, second) = (TempRoot::new(label), TempRoot::new(label));
    assert_ne!(first.path(), second.path());
    let dropped = first.path().to_owned();
    fs::write(dropped.join("f.txt"), b"x")?;
    drop(first);
    assert!(!dropped.exists(), "{} survived its drop", dropped.display());
    assert!(second.path().is_dir(), "the second root outlives the first");
    Ok(())
}
