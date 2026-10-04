//! Tests for the access a `ToolCall` effect carries: the call's own
//! identity, forked from the calling chain when the call is issued and
//! joined back when its answer is applied. The driver performs each call
//! through that access and runs store effects on their own, and a
//! claims conflict ends the run, so a run that succeeds read every file
//! with no conflict. The fork orders the chain's earlier writes before
//! the tool's reads, the join orders the tool's writes before the
//! chain's next step, a task's call forks from the task, and a dropped
//! answer is joined like any other.

use promptforge_vfs::ExecId;
use promptforge_vfs::detail::access_id;

use super::model_tasks::model_task_context_with;
use super::serial_driver::{perform_locally, text_of};
use super::*;
use crate::execute::run::{Effect, EffectAnswer, Run};
use crate::test_support::drive;

/// The suite's one tool, called by its full id: it is the run's whole
/// catalog.
const FILES: &str = "test/tools/files";

/// The tool the run binds. The driver performs its calls itself, through
/// the effect's access, so its own `call` never runs.
struct FilesTool;

#[async_trait::async_trait]
impl TestTool for FilesTool {
    fn id(&self) -> ToolId {
        ToolId::parse(FILES).expect("valid files tool id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn wire_name(&self) -> &str {
        "files"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "reads or writes one of the run's files"
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn call(&self, _args: Value) -> std::result::Result<ToolOutput, ToolError> {
        Err(ToolError::message("the suite's driver performs this tool"))
    }
}

/// Performs one files call through `access`: `read` names a file whose
/// text is the output, and `write` names a file that receives `text`.
fn use_access(access: &Access, args: &Value) -> std::result::Result<ToolOutput, ToolError> {
    let failed = |error: VfsError| ToolError::message(error.to_string());
    if let Some(path) = args["read"].as_str() {
        return access
            .read_string(path)
            .map(ToolOutput::trusted)
            .map_err(failed);
    }
    let path = args["write"]
        .as_str()
        .expect("a files call names a file to read or write");
    let text = args["text"].as_str().unwrap_or_default();
    access.write(path, text.as_bytes()).map_err(failed)?;
    Ok(ToolOutput::trusted("written"))
}

/// How the driver answers a files call once it has performed it.
#[derive(Clone, Copy)]
enum Reply {
    /// The call's own output.
    Output,
    /// `Dropped`, as a Harness that gave the call up would answer.
    Dropped,
}

/// The identities a driven run's effects carried, in issue order: each
/// tool call's access, and each store effect's view, which keeps its
/// chain's identity.
#[derive(Default)]
struct Identities {
    tools: Vec<ExecId>,
    stores: Vec<ExecId>,
}

impl Identities {
    /// Whether every tool call ran under an identity of its own: distinct
    /// from every other call's and from every chain's.
    fn each_call_is_its_own(&self) -> bool {
        self.tools.iter().enumerate().all(|(index, tool)| {
            !self.stores.contains(tool) && !self.tools[index + 1..].contains(tool)
        })
    }
}

/// Drives `md` with the files tool as the run's catalog, answering each
/// files call `reply`'s way after performing it.
fn drive_files(md: &str, reply: Reply) -> (RunResult, Identities) {
    let prompt = parse(md);
    let (state, _harness) = model_task_context_with(
        &prompt,
        Arc::new(NullObserver::default()),
        Arc::new(FilesTool),
    );
    let mut seen = Identities::default();
    let (result, _events) = drive(Run::from_state(state), |_, effect| match effect {
        Effect::ToolCall { access, args, .. } => {
            seen.tools.push(access_id(access));
            let output = use_access(access, args);
            match reply {
                Reply::Output => EffectAnswer::ToolCall(output),
                Reply::Dropped => EffectAnswer::Dropped,
            }
        }
        other => {
            if let Effect::Vfs { access, .. } = other {
                seen.stores.push(access_id(access));
            }
            perform_locally(other, &mut |round| {
                panic!("no model round is issued: {round:?}")
            })
        }
    });
    (result, seen)
}

#[test]
fn a_tool_reads_a_file_the_chain_wrote_just_before_the_call() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fork\n\n\
        ## Only\n\n\
        ```lua\n\
        store.write('before.txt', 'chain wrote this')\n\
        return tools.call('test/tools/files', { read = 'before.txt' })\n\
        ```\n";
    let (result, seen) = drive_files(md, Reply::Output);
    assert_eq!(text_of(result), "chain wrote this");
    assert_eq!(seen.tools.len(), 1, "one call was issued");
    assert!(
        seen.each_call_is_its_own(),
        "the call ran under an identity forked from the chain, not the chain's own"
    );
}

#[test]
fn the_chain_reads_a_file_the_tool_wrote() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Join\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.call('test/tools/files', { write = 'after.txt', text = 'tool wrote this' })\n\
        return store.read('after.txt')\n\
        ```\n";
    let (result, _) = drive_files(md, Reply::Output);
    assert_eq!(text_of(result), "tool wrote this");
}

#[test]
fn a_tool_in_a_spawned_task_forks_from_the_task_and_the_owner_reads_its_write_after_the_join() {
    // The tool reads the task's own write, which only the task's identity
    // orders before it; the owner reads the tool's write once the task is
    // joined, which carries the call's join with it.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Task\n\n\
        ## Main\n\n\
        ```lua\n\
        local t = tasks.spawn('## Worker')\n\
        tasks.join({ t })\n\
        return store.read('out.txt')\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        store.write('in.txt', 'task wrote this')\n\
        local seen = tools.call('test/tools/files', { read = 'in.txt' })\n\
        tools.call('test/tools/files', { write = 'out.txt', text = 'tool saw: ' .. seen })\n\
        return 'done'\n\
        ```\n";
    let (result, seen) = drive_files(md, Reply::Output);
    assert_eq!(text_of(result), "tool saw: task wrote this");
    assert_eq!(seen.tools.len(), 2, "the task issued two calls");
    assert!(
        seen.each_call_is_its_own(),
        "each call ran under its own identity"
    );
}

#[test]
fn a_tool_that_writes_and_is_answered_dropped_is_still_joined() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Dropped\n\n\
        ## Only\n\n\
        ```lua\n\
        local ok, err = pcall(tools.call, 'test/tools/files', { write = 'dropped.txt', text = 'tool wrote this' })\n\
        return tostring(ok) .. ':' .. err.kind .. '|' .. store.read('dropped.txt')\n\
        ```\n";
    let (result, _) = drive_files(md, Reply::Dropped);
    assert_eq!(text_of(result), "false:cancelled|tool wrote this");
}
