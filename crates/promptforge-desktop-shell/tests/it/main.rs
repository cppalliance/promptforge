//! Caller-boundary tests: the desktop binary drives the shell through one
//! narrow entry point, and this target pins its shape from the caller's
//! side of the crate boundary.

/// Pins the exact signature of the single public entry point. Widening
/// or reshaping the boundary - a renamed function, an extra parameter, a
/// changed argument or return type - fails to compile this test, so the
/// seam the desktop binary calls cannot drift silently.
#[test]
fn run_is_the_single_narrow_entry_point() {
    let entry_point: fn(&str) -> anyhow::Result<()> = promptforge_desktop_shell::run;
    let _ = entry_point;
}
