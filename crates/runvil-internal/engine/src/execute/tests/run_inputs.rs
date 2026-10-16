//! The caller-drawn inputs on `RunContext`: `seed` (the untrusted-envelope
//! nonce derives from it), `started_at` (rendered as `sys.when` for the H1
//! pass and every walked section alike), and `ui` (the snapshot the `ui()`
//! global serves). Two runs under the same inputs agree byte for byte, and
//! `sys.now` is an unknown field.

use runvil_types::replay::Flags;
use runvil_types::seed::{RunSeed, RunUid};
use runvil_types::timestamp::Timestamp;

use super::context::{parse, test_context};
use super::*;
use crate::execute::run::Run;
use crate::test_support::drive;
use crate::test_support::scenario::{EXECUTION, TEST_SEED, text_of};
use crate::test_support::seeded::Seeded;

/// A fixed instant with a millisecond fraction, so the rendering exercises
/// the fraction branch: `2000-02-29T00:00:00.123Z`.
const STARTED_AT: Timestamp = Timestamp::from_unix_millis(951_782_400_123);
const STARTED_AT_RFC3339: &str = "2000-02-29T00:00:00.123Z";

/// The generator seed the run seeds here are drawn under; a failure
/// names it.
const DRAWS: u64 = 0x5EED;

/// Runs `md` with no effects under `seed` and [`STARTED_AT`], returning
/// the run's text.
fn run_seeded(md: &str, seed: RunSeed) -> RunResult {
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

const WHEN_AND_WRAP: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
    # Title\n\n\
    ## Only\n\n\
    ```lua\n\
    return sys.when .. '|' .. untrusted('data')\n\
    ```\n";

#[test]
fn two_runs_with_the_same_seed_and_started_at_produce_identical_nonces_and_sys_when() {
    let mut rng = Seeded::new(DRAWS);
    let seed = rng.run_seed();
    let first = text_of(run_seeded(WHEN_AND_WRAP, seed));
    let second = text_of(run_seeded(WHEN_AND_WRAP, seed));
    rng.check_eq(&first, &second, "same inputs, same text");
    assert_eq!(
        STARTED_AT.to_rfc3339(),
        STARTED_AT_RFC3339,
        "the fixture's expected rendering is Timestamp::to_rfc3339's"
    );
    assert!(
        first.starts_with(&format!("{STARTED_AT_RFC3339}|")),
        "sys.when is the context's started_at rendered as RFC 3339: {first}"
    );
    assert_eq!(
        nonce_in(&first).len(),
        32,
        "the nonce keeps its 32 hex digits"
    );
}

#[test]
fn a_different_seed_changes_the_nonce_but_not_sys_when() {
    let mut rng = Seeded::new(DRAWS);
    let one = text_of(run_seeded(WHEN_AND_WRAP, rng.run_seed()));
    let other = text_of(run_seeded(WHEN_AND_WRAP, rng.run_seed()));
    rng.check(
        nonce_in(&one) != nonce_in(&other),
        "the nonce is the seed's",
    );
    rng.check(
        other.starts_with(&format!("{STARTED_AT_RFC3339}|")),
        format_args!("sys.when does not depend on the seed: {other}"),
    );
}

#[test]
fn the_h1_pass_reads_the_same_sys_when_as_the_walk() {
    // The H1 pass and the walk both read the run's `started_at`. A scalar
    // H1 return short-circuits the run with that value.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Title\n\n\
        ```lua\n\
        return sys.when\n\
        ```\n\n\
        ## Only\n\n\
        ```lua\n\
        return 'unreached'\n\
        ```\n";
    assert_eq!(text_of(run_seeded(md, TEST_SEED)), STARTED_AT_RFC3339);
}

#[test]
fn sys_when_is_timestamp_to_rfc3339_for_any_started_at() {
    // Whatever instant the caller stamps, `sys.when` is that value's own
    // rendering: here one on a whole second, so the fraction is omitted,
    // which the millisecond fixture above cannot show.
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return sys.when\n\
        ```\n";
    let whole_second = Timestamp::from_unix_millis(1_700_000_000_000);
    let ctx = RunContext::new(EXECUTION, TEST_SEED, whole_second);
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
    // verbatim when the caller sets it, and readable beside the other
    // inputs.
    let fresh = test_context(EXECUTION);
    assert_eq!(fresh.run_flags(), Flags::EMPTY);
    assert!(fresh.run_flags().is_empty(), "no flag is set by default");

    let recorded = Flags::from_bits(0b101);
    let mut rng = Seeded::new(DRAWS);
    let seed = rng.run_seed();
    let ctx = RunContext::new(EXECUTION, seed, STARTED_AT).flags(recorded);
    assert_eq!(ctx.run_flags(), recorded, "the caller's flags are kept");
    rng.check_eq(&ctx.seed(), &seed, "the caller's seed is kept");
    assert_eq!(ctx.started_at(), STARTED_AT);
}

#[test]
fn a_run_contexts_debug_shows_the_run_id_and_not_the_seed() {
    let mut rng = Seeded::new(DRAWS);
    let seed = rng.run_seed();
    let ctx = RunContext::new(EXECUTION, seed, STARTED_AT);
    let debug = format!("{ctx:?}");
    let uid = RunUid::from_seed(&seed);
    rng.check_eq(&ctx.uid(), &uid, "the run id derives from the seed");
    rng.check(
        debug.contains(&format!("uid: {uid:?}")),
        format_args!("the Debug names the run id {uid}: {debug}"),
    );
    rng.check(
        !debug.contains(&seed.to_string()) && !debug.to_lowercase().contains("seed"),
        format_args!("the Debug shows no seed: {debug}"),
    );
}

#[test]
fn sys_now_is_absent_from_the_globals() {
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return sys.now\n\
        ```\n";
    let error = match run_seeded(md, TEST_SEED) {
        RunResult::Failure(error) => error,
        other => panic!("reading sys.now fails the section: {other:?}"),
    };
    assert!(
        error.to_string().contains("unknown sys field 'now'"),
        "sys.now is an unknown field, not a stale clock: {error}"
    );
}

#[test]
fn the_ui_global_serves_the_snapshot_taken_at_run_start() {
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
    let md = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
        # Title\n\n\
        ## Only\n\n\
        ```lua\n\
        return type(ui)\n\
        ```\n";
    assert_eq!(text_of(run_seeded(md, TEST_SEED)), "nil");
}
