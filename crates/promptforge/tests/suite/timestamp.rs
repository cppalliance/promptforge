//! Stamping a run's start: the prompt reads the context's start time as
//! `sys.when` in RFC 3339 form, and a stamp's millisecond count
//! round-trips.

use std::time::{SystemTime, UNIX_EPOCH};

use promptforge::RunResult;
use promptforge::timestamp::Timestamp;

use super::support::{Scenario, TestResult, greeter};

#[test]
fn the_prompt_reads_the_contexts_start_time_as_sys_when() -> TestResult {
    let source = greeter(
        "
        store.write('note.md', sys.when)
        return store.read('note.md')
    ",
    );
    let elapsed = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let stamp = Timestamp::from_unix_millis(i64::try_from(elapsed.as_millis())?);

    let trace = Scenario::default().started_at(stamp).run(&source)?;
    assert!(matches!(trace.result, RunResult::Ok(text) if text == stamp.to_rfc3339()));
    assert_eq!(Timestamp::from_unix_millis(stamp.unix_millis()), stamp);
    Ok(())
}
