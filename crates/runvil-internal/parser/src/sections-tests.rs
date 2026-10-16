//! Tests for the source-position helpers.

use super::newlines_before;
use crate::Error;

#[test]
fn newlines_before_reports_broken_byte_offset_invariants() {
    for offset in [1, 4] {
        let error = newlines_before("é\n", offset)
            .expect_err("a non-boundary or out-of-range offset must return an error");
        assert!(matches!(
            error,
            Error::Internal("parser: source byte offset invariant broken")
        ));
    }
}
