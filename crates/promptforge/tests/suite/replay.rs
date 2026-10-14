//! Keeping a run's behavior flags: they store as one number, rebuild into
//! equal flags for the next run, and keep bits this version does not name.

use promptforge::RunResult;
use promptforge::replay::Flags;

use super::support::{NOTE, Scenario, TestResult, greeter};

#[test]
fn stored_flags_rebuild_into_equal_flags_for_the_next_run() -> TestResult {
    let source = greeter(NOTE);
    let original = Scenario::default().prepare(&source)?.ctx.run_flags();
    let stored: u32 = original.bits();
    let first = Scenario::default().run(&source)?;
    assert!(matches!(first.result, RunResult::Ok(text) if text == "hello"));
    assert_eq!(stored, 0);

    let restored = Flags::from_bits(stored);
    assert_eq!(restored, original);
    let next = Scenario::default().context(move |ctx| ctx.flags(restored));
    assert_eq!(next.prepare(&source)?.ctx.run_flags(), original);
    let second = next.run(&source)?;
    assert!(matches!(second.result, RunResult::Ok(text) if text == "hello"));
    Ok(())
}

#[test]
fn flags_from_a_newer_record_keep_the_bits_this_version_does_not_name() {
    let newer = Flags::from_bits(0b101);
    assert_eq!(newer.bits(), 0b101);
    assert!(newer.contains(Flags::from_bits(0b100)));
    assert!(!newer.contains(Flags::from_bits(0b010)));
}
