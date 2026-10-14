//! Keeping a run's behavior flags: they store as one number and rebuild
//! into equal flags for the next run.

use promptforge::replay::Flags;

use super::support::{Scenario, TestResult, note};

#[test]
fn stored_flags_rebuild_into_equal_flags_for_the_next_run() -> TestResult {
    let source = note("'hello'");
    let original = Scenario::default().prepare(&source)?.ctx.run_flags();
    let stored: u32 = original.bits();
    let first = Scenario::default().run(&source)?;
    assert_eq!(first.text()?, "hello");
    assert_eq!(stored, 0);

    let restored = Flags::from_bits(stored);
    assert_eq!(restored, original);
    let next = Scenario::default().context(move |ctx| ctx.flags(restored));
    assert_eq!(next.prepare(&source)?.ctx.run_flags(), original);
    let second = next.run(&source)?;
    assert_eq!(second.text()?, "hello");
    Ok(())
}
