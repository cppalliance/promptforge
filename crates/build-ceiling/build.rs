//! Runs the compile-time file-size check on this crate by including its own
//! source, because a crate cannot be its own build-dependency.

#[expect(
    unreachable_pub,
    reason = "the library's public items are private to this build script"
)]
#[path = "src/lib.rs"]
mod build_ceiling;

fn main() -> Result<(), build_ceiling::Violations> {
    build_ceiling::check()
}
