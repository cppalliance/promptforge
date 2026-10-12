//! The loader chunk's name: a live path to the file it embeds.

use super::{REQUIRE_CHUNK_NAME, REQUIRE_SOURCE};
use crate::tests::assert_chunk_name_resolves;

#[test]
fn the_require_chunk_name_resolves_to_the_loader_file() {
    assert_chunk_name_resolves("REQUIRE_CHUNK_NAME", REQUIRE_CHUNK_NAME, REQUIRE_SOURCE);
}
