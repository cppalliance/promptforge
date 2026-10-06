//! Guarding and watching a run's files: a mode policy lets Agent mode
//! write and refuses a write in Ask mode, and an operation watcher sees
//! every store call the run makes, in order, under one origin.

use std::error::Error;
use std::sync::{Arc, Mutex};

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::{
    MemoryBackend, Mode, ModePolicy, Op, OpEvent, Origin, VfsError, VfsOp, VfsOutcome, VfsRef,
    perform_vfs_op,
};
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

/// Writes a note to the store and reads it back.
const NOTE: &str = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "return store.read('note.md')\n",
    "```\n",
);

/// Runs [`NOTE`] over `vfs`, answering each store effect against it.
fn run_note(vfs: VfsRef) -> Result<RunResult, Box<dyn Error>> {
    let (parsed, _parse_events) = Prompt::parse(NOTE, "greeter");
    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).vfs(vfs);
    let mut run = Run::new(Arc::new(parsed?), "", ctx);
    loop {
        match run.step() {
            Step::Pending { effects, .. } => {
                for (id, _provenance, effect) in effects {
                    let answer = match effect {
                        Effect::Vfs { access, op } => {
                            EffectAnswer::Vfs(perform_vfs_op(&access, op))
                        }
                        _ => EffectAnswer::Dropped,
                    };
                    run.resume(id, answer);
                }
            }
            Step::Done { result, .. } => return Ok(result),
        }
    }
}

#[test]
fn agent_mode_lets_the_run_write_and_ask_mode_refuses_a_later_write() -> Result<(), Box<dyn Error>>
{
    let policy = ModePolicy::new(Mode::Agent);
    let mode = policy.handle();
    let vfs = VfsRef::builder()
        .store("/", MemoryBackend::new())
        .policy(policy)
        .build();

    let result = run_note(vfs.clone())?;
    assert!(matches!(result, RunResult::Ok(text) if text == "hello"));

    mode.set(Mode::Ask);
    let store = vfs.acquire_store(Origin::new("caller"))?;
    let second = VfsOp::Write {
        path: "note.md".to_owned(),
        contents: "changed".to_owned(),
    };
    let refused = perform_vfs_op(&store, second);
    assert!(
        matches!(&refused, Err(VfsError::PermissionDenied { reason, .. }) if reason.contains("Ask"))
    );

    let read = VfsOp::Read {
        path: "note.md".to_owned(),
        start: None,
        end: None,
    };
    assert_eq!(
        perform_vfs_op(&store, read)?,
        VfsOutcome::Text("hello".to_owned())
    );
    Ok(())
}

#[test]
fn an_operation_watcher_sees_the_notes_write_then_read_under_one_origin()
-> Result<(), Box<dyn Error>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink_log = Arc::clone(&log);
    let vfs = VfsRef::builder()
        .store("/", MemoryBackend::new())
        .on_op(move |event: OpEvent<'_>| {
            if let Ok(mut entries) = sink_log.lock() {
                entries.push((
                    event.op(),
                    event.path().to_string(),
                    event.origin().label.clone(),
                ));
            }
        })
        .build();

    let result = run_note(vfs)?;
    assert!(matches!(result, RunResult::Ok(text) if text == "hello"));

    let entries = log.lock().map_err(|_| "the watcher panicked")?;
    let note: Vec<_> = entries
        .iter()
        .filter(|(_, path, _)| path == "/note.md")
        .collect();
    assert!(
        matches!(note.as_slice(), [(Op::Write, _, first), (Op::Read, _, second)] if first == second)
    );
    Ok(())
}
