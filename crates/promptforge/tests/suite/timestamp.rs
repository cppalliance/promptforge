//! Stamping a run's start: the prompt reads the context's start time as
//! `sys.when` in RFC 3339 form, and a stamp's millisecond count
//! round-trips.

use std::error::Error;
use std::sync::Arc;
use std::time::SystemTime;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::timestamp::Timestamp;
use promptforge::vfs::perform_vfs_op;
use promptforge::{Prompt, Run, RunContext, RunResult, Step};

#[test]
fn the_prompt_reads_the_contexts_start_time_as_sys_when() -> Result<(), Box<dyn Error>> {
    let source = concat!(
        "---\n",
        "name: greeter\n",
        "description: Writes its start time to the store and reads it back.\n",
        "promptforge: 0\n",
        "---\n\n",
        "# Greeter\n\n",
        "## Greet\n\n",
        "```lua\n",
        "store.write('note.md', sys.when)\n",
        "return store.read('note.md')\n",
        "```\n",
    );
    let (parsed, _parse_events) = Prompt::parse(source, "greeter");
    let prompt = Arc::new(parsed?);

    let elapsed = SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    let stamp = Timestamp::from_unix_millis(i64::try_from(elapsed.as_millis())?);

    let mut run = Run::new(prompt, "", RunContext::new("greeter", 7, stamp));
    let result = loop {
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
            Step::Done { result, .. } => break result,
        }
    };

    assert!(matches!(result, RunResult::Ok(text) if text == stamp.to_rfc3339()));
    assert_eq!(Timestamp::from_unix_millis(stamp.unix_millis()), stamp);
    Ok(())
}
