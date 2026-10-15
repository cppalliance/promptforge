//! Tests for the run seed and the values derived from it: the derivation's
//! known answer, the nonce and run id it yields, the seed's serde form, and
//! its redacted `Debug`.

use std::fmt::Write as _;

use super::*;
use crate::test_support::{Seeded, assert_wire};
use crate::untrusted::GuardNonce;

/// The seed every draw in these tests starts from; a failure names it.
const SEED: u64 = 0x5EED;

/// `bytes` as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("a String takes every write");
        out
    })
}

#[test]
fn derive_matches_its_known_answer() {
    // SHA-256 over `promptforge.run-uid.v1` then the bytes 0x00..=0x1f,
    // computed once outside this crate.
    let seed = RunSeed::from_bytes(std::array::from_fn(|i| {
        u8::try_from(i).expect("an index below 32 fits a byte")
    }));
    let expected = "6f2f4dd98da459ed84496ff9f6fa48886f76f8b0325d58888661b7b45394d8e1";
    assert_eq!(hex(&derive(b"promptforge.run-uid.v1", &seed)), expected);
    assert_eq!(
        RunUid::from_seed(&seed).to_string(),
        expected,
        "the run id is derive under its own tag"
    );
}

#[test]
fn the_nonce_and_run_id_differ_from_each_other_and_the_seed_and_repeat() {
    let mut rng = Seeded::new(SEED);
    let seed = rng.run_seed();
    let nonce = GuardNonce::from_seed(&seed).to_string();
    let uid = RunUid::from_seed(&seed).to_string();
    let bytes = hex(seed.as_bytes());
    rng.check(
        !uid.starts_with(&nonce),
        format_args!("the nonce {nonce} is the run id's prefix {uid}"),
    );
    rng.check(uid != bytes, "the run id is the seed's bytes");
    rng.check(
        !bytes.starts_with(&nonce),
        format_args!("the nonce {nonce} is the seed's prefix"),
    );
    rng.check_eq(
        &GuardNonce::from_seed(&seed).to_string(),
        &nonce,
        "one seed, one nonce",
    );
    rng.check_eq(
        &RunUid::from_seed(&seed),
        &RunUid::from_seed(&seed),
        "one seed, one run id",
    );
}

#[test]
fn a_run_seed_round_trips_through_serde_as_64_hex_characters() {
    let mut rng = Seeded::new(SEED);
    let seed = rng.run_seed();
    let rendered = hex(seed.as_bytes());
    rng.check_eq(&seed.to_string(), &rendered, "Display is the serde form");
    assert_wire(&seed, &format!("\"{rendered}\""));

    let short = serde_json::from_str::<RunSeed>("\"abc\"")
        .expect_err("three characters are not a seed")
        .to_string();
    assert!(
        short.contains("64 lowercase hex characters") && short.contains("found 3"),
        "the refusal names the required form and the actual length: {short}"
    );
    let upper = serde_json::from_str::<RunSeed>(&format!("\"A{}\"", &rendered[1..]))
        .expect_err("an uppercase digit is not a seed")
        .to_string();
    assert!(
        upper.contains("64 lowercase hex characters") && upper.contains("'A'"),
        "the refusal names the required form and the actual character: {upper}"
    );
}

#[test]
fn a_run_seeds_debug_shows_no_seed_bytes() {
    let seed = RunSeed::from_bytes([0xab; 32]);
    assert_eq!(format!("{seed:?}"), "RunSeed(..)");
    assert_eq!(format!("{seed:#?}"), "RunSeed(..)");
}
