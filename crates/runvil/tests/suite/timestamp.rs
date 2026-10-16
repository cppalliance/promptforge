//! Stamping a run's start, and reading it back as `sys.when`.

use runvil::timestamp::Timestamp;

use super::support::{Scenario, TestResult, note};

#[test]
fn the_prompt_reads_the_contexts_start_time_as_sys_when() -> TestResult {
    let source = note("sys.when");
    let stamp = Timestamp::from_unix_millis(1_700_000_000_000);

    let trace = Scenario::default().started_at(stamp).run(&source)?;
    assert_eq!(trace.text()?, stamp.to_rfc3339());
    assert_eq!(Timestamp::from_unix_millis(stamp.unix_millis()), stamp);
    Ok(())
}
