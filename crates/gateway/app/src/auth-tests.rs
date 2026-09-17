//! Tests for the shared secret comparison.
use super::secret_eq;

#[test]
fn equal_secrets_match() {
    assert!(secret_eq(b"s3cret-token", b"s3cret-token"));
}

#[test]
fn unequal_secrets_do_not_match() {
    assert!(!secret_eq(b"s3cret-token", b"wrong-token"));
    assert!(!secret_eq(b"", b"nonempty"));
    assert!(!secret_eq(b"short", b"a-much-longer-token"));
}

#[test]
fn empty_matches_empty() {
    assert!(secret_eq(b"", b""));
}
