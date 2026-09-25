//! `build-xtask` - workspace automation for the PromptForge repository.
//!
//! ## Invariants
//!
//! - Tier: tooling; depends on no workspace crates. The tidy-style
//!   architecture checks run as tests (`cargo test -p build-xtask`);
//!   `cargo xtask tidy` prints the same report on demand.
//! - `cargo xtask api` reads rustdoc JSON, so it runs only on the nightly
//!   pinned in `api/toolchain.rs`; its fixtures that build rustdoc JSON are
//!   `#[ignore]`d on stable.
//! - Every file in this crate stays under 500 lines; split first, then edit.

mod api;
mod doc_hidden;
mod engine_deps;
mod engine_guards;
mod facade_shape;
mod harness_bans;
mod manifest;
mod new_crate;
mod product;
mod retired_symbols;
mod site;
mod test_support_leak;
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
        Some("api") => api::run(root, &args[2..]),
        Some("site") => site::run(root, &args[2..]),
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
    eprintln!(
        "usage: cargo xtask new-crate <workshop-name> | cargo xtask tidy | \
         cargo +<pinned nightly> xtask api [--check | --bless] | \
         cargo xtask site [--books-only]"
    );
    ExitCode::from(2)
}
