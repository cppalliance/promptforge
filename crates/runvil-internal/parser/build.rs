//! Runs the compile-time file-size check on this crate.

fn main() -> Result<(), build_ceiling::Violations> {
    build_ceiling::check()
}
