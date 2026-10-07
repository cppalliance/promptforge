//! Includes the file-size check's source instead of depending on
//! `build-ceiling`, because Plugin crates may name no `build-*` crate.

#[expect(
    unreachable_pub,
    reason = "the check's public items are private to this build script"
)]
#[path = "../build-ceiling/src/lib.rs"]
mod build_ceiling;

fn main() -> Result<(), build_ceiling::Violations> {
    build_ceiling::check()
}
