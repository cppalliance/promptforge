//! Tests that the shim chunk names stay pointed at the shim files.

use super::{
    FANOUT_CHUNK_NAME, FANOUT_SOURCE, SHIM_CHUNK_NAME, SHIM_SOURCE, TASKS_CHUNK_NAME, TASKS_SOURCE,
};
use crate::tests::assert_chunk_name_resolves;

#[test]
fn the_shim_chunk_name_resolves_to_the_shim_file() {
    assert_chunk_name_resolves("SHIM_CHUNK_NAME", SHIM_CHUNK_NAME, SHIM_SOURCE);
}

#[test]
fn the_tasks_chunk_name_resolves_to_the_tasks_file() {
    assert_chunk_name_resolves("TASKS_CHUNK_NAME", TASKS_CHUNK_NAME, TASKS_SOURCE);
}

#[test]
fn the_fanout_chunk_name_resolves_to_the_fanout_file() {
    assert_chunk_name_resolves("FANOUT_CHUNK_NAME", FANOUT_CHUNK_NAME, FANOUT_SOURCE);
}
