//! The greeter, one tour at a time, up to the complete program in
//! `examples/greeter.rs`.

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::{Prompt, RunResult};

use super::support::{Scenario, TestResult, note, prompt, shout, text};

#[path = "../../examples/greeter.rs"]
#[expect(
    unreachable_pub,
    reason = "the example's items are pub at its own crate root, and the suite reaches them through this private module"
)]
#[expect(dead_code, reason = "only the example binary calls its main")]
mod example;

use example::{GREETER, answer};

const REPLY: &str = "
    store.write('note.md', 'hello')
    models.use('writer')
    return models.infer(store.read('note.md'))
";

#[test]
fn the_smallest_greeter_parses_and_its_title_is_the_h1_text() {
    let (parsed, _events) = Prompt::parse(&note("'hello'"), "greeter");
    assert!(parsed.is_ok_and(|prompt| prompt.title() == "Greeter"));
}

#[test]
fn answering_each_store_effect_from_the_runs_store_returns_the_note() -> TestResult {
    let trace = Scenario::default().run(&note("'hello'"))?;
    assert_eq!(trace.text()?, "hello");
    Ok(())
}

#[test]
fn the_example_answers_a_timer_as_fired_at_once() {
    assert!(matches!(
        answer(Effect::Timer {
            duration: std::time::Duration::from_secs(3_600)
        }),
        EffectAnswer::Timer
    ));
}

#[test]
fn prepare_fills_the_writer_role_and_the_canned_reply_becomes_the_result() -> TestResult {
    let source = prompt("models: { writer: {} }", &[("Greet", REPLY)]);
    let trace = Scenario::replies([text("hi there")]).run(&source)?;
    assert_eq!(trace.text()?, "hi there");
    Ok(())
}

#[test]
fn trusted_tool_output_reaches_the_calling_lua_unchanged() -> TestResult {
    let trace = Scenario::tool(shout(), [text("hi there")]).run(GREETER)?;
    assert_eq!(trace.text()?, "HI THERE");
    Ok(())
}

#[test]
fn a_cancel_from_another_thread_ends_the_run_cancelled_once_held_effects_drop() -> TestResult {
    let mut driver = Scenario::tool(shout(), [])
        .hold(|effect| matches!(effect, Effect::Chat { .. }))
        .start(GREETER)?;
    driver.pump_until_held()?;

    let handle = driver.run.cancel_handle();
    std::thread::spawn(move || handle.cancel())
        .join()
        .map_err(|_| "the cancelling thread panicked")?;

    assert!(driver.pump()? == 0 && driver.run.decided());
    assert!(matches!(driver.finish()?.result, RunResult::Cancelled));
    Ok(())
}

#[test]
fn the_complete_greeter_returns_hi_there_and_its_log_opens_with_the_parse_events() -> TestResult {
    assert_eq!(example::greet()?, "HI THERE");
    Ok(())
}
