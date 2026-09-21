//! The host-drawn inputs on `RunContext` that replaced the engine's own
//! clock and RNG: `seed` (the untrusted-envelope nonce derives from it),
//! `started_at` (rendered as `sys.when` for the H1 pass and every walked
//! section alike), and `ui` (the snapshot the `ui()` global serves). Two
//! runs under the same inputs agree byte for byte; `sys.now` no longer
//! exists.

use promptforge_api_types::replay::Flags;
use promptforge_api_types::timestamp::Timestamp;

use super::task_events::text_of;
use super::*;
use crate::execute::run::Run;
use crate::test_support::drive;

/// A fixed instant with a millisecond fraction, so the rendering exercises
/// the fraction branch: `2000-02-29T00:00:00.123Z`.
const STARTED_AT: Timestamp = Timestamp::from_unix_millis(951_782_400_123);
const STARTED_AT_RFC3339: &str = "2000-02-29T00:00:00.123Z";

/// Runs `md` with no effects under `seed` and [`STARTED_AT`], returning
/// the run's text.
fn run_seeded(md: &str, seed: u64) -> RunResult {
    let ctx = RunContext::new(EXECUTION, seed, STARTED_AT);
    let (result, _) = drive(Run::new(Arc::new(parse(md)), "", ctx), |_, effect| {
        panic!("no effect is issued: {effect:?}")
    });
    result
}

/// The nonce between `<untrusted_input_` and `>` in a wrapped envelope.
fn nonce_in(text: &str) -> String {
    let marker = "<untrusted_input_";
    let start = text.find(marker).expect("the text includes an envelope") + marker.len();
    let end = text[start..].find('>').expect("the open tag closes") + start;
    text[start..end].to_owned()
}

const WHEN_AND_WRAP: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n\
    ## Only\n\n\
    ```lua\n\
    return sys.when .. '|' .. untrusted('data')\n\
    ```\n";

#[test]
fn two_runs_with_the_same_seed_and_started_at_produce_identical_nonces_and_sys_when() {
    let first = text_of(run_seeded(WHEN_AND_WRAP, 7));
    let second = text_of(run_seeded(WHEN_AND_WRAP, 7));
    assert_eq!(first, second, "same inputs, same text");
    assert_eq!(
        STARTED_AT.to_rfc3339(),
        STARTED_AT_RFC3339,
        "the fixture's expected rendering is Timestamp::to_rfc3339's"
    );
    assert!(
        first.starts_with(&format!("{STARTED_AT_RFC3339}|")),
        "sys.when is the host's started_at rendered as RFC 3339: {first}"
    );
    assert_eq!(
        nonce_in(&first).len(),
        32,
        "the nonce keeps its 32 hex digits"
    );
}

#[test]
fn a_different_seed_changes_the_nonce_but_not_sys_when() {
    let seven = text_of(run_seeded(WHEN_AND_WRAP, 7));
    let eight = text_of(run_seeded(WHEN_AND_WRAP, 8));
    assert_ne!(
        nonce_in(&seven),
        nonce_in(&eight),
        "the nonce is the seed's"
    );
    assert!(
        eight.starts_with(&format!("{STARTED_AT_RFC3339}|")),
        "sys.when does not depend on the seed: {eight}"
    );
}

#[test]
fn the_h1_pass_reads_the_same_sys_when_as_the_walk() {
    // H1 used to stamp its own `now`; both now read the run's `started_at`.
    // A scalar H1 return short-circuits the run with that value.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ```lua\n\
        return sys.when\n\
        ```\n\n\
        ## Only\n\n\
        ```lua\n\
        return 'unreached'\n\
        ```\n";
    assert_eq!(text_of(run_seeded(md, 1)), STARTED_AT_RFC3339);
}

#[test]
fn sys_when_is_timestamp_to_rfc3339_for_any_started_at() {
    // Whatever instant the host stamps, `sys.when` is that value's own
    // rendering: here one on a whole second, so the fraction is omitted,
    // which the millisecond fixture above cannot show.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return sys.when\n\
        ```\n";
    let whole_second = Timestamp::from_unix_millis(1_700_000_000_000);
    let ctx = RunContext::new(EXECUTION, 1, whole_second);
    let (result, _) = drive(Run::new(Arc::new(parse(md)), "", ctx), |_, effect| {
        panic!("no effect is issued: {effect:?}")
    });
    let when = text_of(result);
    assert_eq!(when, whole_second.to_rfc3339());
    assert_eq!(
        when, "2023-11-14T22:13:20Z",
        "no fraction on a whole second"
    );
}

#[test]
fn the_context_holds_the_flags_and_starts_them_empty() {
    // `Flags` is a run input like the seed: empty from `new`, kept
    // verbatim when the host sets it (a replay hands back the recorded
    // set), and readable beside the other inputs.
    let fresh = test_context(EXECUTION);
    assert_eq!(fresh.run_flags(), Flags::EMPTY);
    assert!(fresh.run_flags().is_empty(), "no flag is set by default");

    let recorded = Flags::from_bits(0b101);
    let ctx = RunContext::new(EXECUTION, 42, STARTED_AT).flags(recorded);
    assert_eq!(ctx.run_flags(), recorded, "the host's flags are kept");
    assert_eq!(ctx.seed(), 42);
    assert_eq!(ctx.started_at(), STARTED_AT);
}

#[test]
fn sys_now_is_absent_from_the_globals() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return sys.now\n\
        ```\n";
    let RunResult::Failure(error) = run_seeded(md, 1) else {
        panic!("reading sys.now fails the section");
    };
    assert!(
        error.to_string().contains("unknown sys field 'now'"),
        "sys.now is an unknown field, not a stale clock: {error}"
    );
}

#[test]
fn the_ui_global_serves_the_snapshot_taken_at_run_start() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return ui().selected_model .. '/' .. tostring(ui().workspace_root)\n\
        ```\n";
    let ctx =
        test_context(EXECUTION).ui(json!({ "selected_model": "m-1", "workspace_root": null }));
    let (result, _) = drive(Run::new(Arc::new(parse(md)), "", ctx), |_, effect| {
        panic!("no effect is issued: {effect:?}")
    });
    assert_eq!(
        text_of(result),
        "m-1/nil",
        "the snapshot's fields read as Lua values, a JSON null as nil"
    );
}

#[test]
fn a_run_without_a_ui_snapshot_installs_no_ui_global() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return type(ui)\n\
        ```\n";
    assert_eq!(text_of(run_seeded(md, 1)), "nil");
}
