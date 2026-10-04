//! Keeping a run's behavior flags: they store as one number, rebuild into
//! equal flags for the next run, and keep bits this version does not name.

use std::error::Error;
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::replay::Flags;
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

/// Steps `run` to its result, answering each store effect from its store.
fn run_to_end(mut run: Run) -> RunResult {
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
            Step::Done { result, .. } => return result,
        }
    }
}

#[test]
fn stored_flags_rebuild_into_equal_flags_for_the_next_run() -> Result<(), Box<dyn Error>> {
    let source = concat!(
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
    let (parsed, _parse_events) = Prompt::parse(source, "greeter");
    let prompt = Arc::new(parsed?);

    let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH);
    let original = ctx.run_flags();
    let stored: u32 = original.bits();
    let first = run_to_end(Run::new(Arc::clone(&prompt), "", ctx));
    assert!(matches!(first, RunResult::Ok(text) if text == "hello"));
    assert_eq!(stored, 0);

    let restored = Flags::from_bits(stored);
    assert_eq!(restored, original);
    let next = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).flags(restored);
    assert_eq!(next.run_flags(), original);
    let second = run_to_end(Run::new(prompt, "", next));
    assert!(matches!(second, RunResult::Ok(text) if text == "hello"));
    Ok(())
}

#[test]
fn flags_from_a_newer_record_keep_the_bits_this_version_does_not_name() {
    let newer = Flags::from_bits(0b101);
    assert_eq!(newer.bits(), 0b101);
    assert!(newer.contains(Flags::from_bits(0b100)));
    assert!(!newer.contains(Flags::from_bits(0b010)));
}
