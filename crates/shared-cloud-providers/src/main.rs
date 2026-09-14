//! Sheet-building binary: a thin `main` over the `shared-cloud-providers`
//! library, compiled and run by the aggregation workflow and runnable
//! locally for testing and sheet building.

fn main() -> std::process::ExitCode {
    eprintln!("shared-cloud-providers: sheet building is not yet implemented");
    std::process::ExitCode::from(2)
}
