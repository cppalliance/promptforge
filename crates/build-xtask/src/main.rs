//! `build-xtask` - workspace automation for the PromptForge repository.
//!
//! ## Invariants
//!
//! - Tier: tooling; depends on no workspace crates. The tidy-style
//!   architecture checks run as tests (`cargo test -p build-xtask`);
//!   `cargo xtask tidy` prints the same report on demand.
//! - Every file in this crate stays under 500 lines; split first, then edit.

// Fixture-tested now; Step 39 of the sans-I/O engine plan wires the guard
// into `tidy`, at which point the allowance goes.
#[cfg_attr(not(test), allow(dead_code))]
mod engine_deps;
mod manifest;
mod new_crate;
mod product;
mod tidy;

use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) else {
        eprintln!("error: cannot locate workspace root from CARGO_MANIFEST_DIR");
        return ExitCode::FAILURE;
    };
    match args.get(1).map(String::as_str) {
        Some("new-crate") => match args.get(2) {
            Some(name) => match new_crate::scaffold(root, name) {
                Ok(dir) => {
                    println!("scaffolded {}", dir.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error:#}");
                    ExitCode::FAILURE
                }
            },
            None => usage(),
        },
        Some("tidy") => {
            let violations = tidy::all_violations(root);
            if violations.is_empty() {
                println!("tidy: no violations");
                ExitCode::SUCCESS
            } else {
                for violation in &violations {
                    eprintln!("tidy: {violation}");
                }
                ExitCode::FAILURE
            }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: cargo xtask new-crate <workshop-name> | cargo xtask tidy");
    ExitCode::from(2)
}
