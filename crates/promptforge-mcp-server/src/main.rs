//! The `promptforge-mcp-server` binary:
//! `promptforge-mcp-server serve [--stdio] <prompts.toml>`.
//!
//! Boot either produces a complete catalog or the process refuses to serve.
//! Every fault the resolution pass found is printed before the nonzero exit, so
//! an operator fixes them in one pass rather than one restart each.

use std::process::ExitCode;

use promptforge_mcp_server::{ServerArgs, run};

/// What the process prints when the arguments are not the two shapes it takes.
const USAGE: &str = "usage: promptforge-mcp-server serve [--stdio] <prompts.toml>";

/// Entry point. A thin shell: it parses the command line, routes logging off
/// standard output when the wire owns it, hands the rest to
/// [`promptforge_mcp_server::run`], and renders any boot failure. The runtime
/// is built inside `run` rather than by an attribute macro, matching the
/// gateway, so a future service wrapper can construct it the same way.
fn main() -> ExitCode {
    let Some(args) = ServerArgs::parse(std::env::args_os().skip(1)) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    // On stdio the protocol owns standard output, so a log line written there
    // would corrupt the wire. Everything else logs to stdout.
    if args.stdio() {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stdout)
            .init();
    }
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", report(&error));
            ExitCode::FAILURE
        }
    }
}

/// Renders a boot failure and every cause under it, one line per link.
///
/// `Display` on a boxed error prints the outermost message alone, which would
/// reach the operator as `bind 127.0.0.1:9310` with the I/O reason - "address
/// already in use" - discarded, and the same for an unreadable configuration.
fn report(error: &(dyn std::error::Error + 'static)) -> String {
    let mut out = format!("error: {error}");
    let mut cause = error.source();
    while let Some(source) = cause {
        out.push_str("\n  caused by: ");
        out.push_str(&source.to_string());
        cause = source.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use promptforge_mcp_server::Config;
    use tempfile::TempDir;

    use super::report;

    #[test]
    fn a_report_carries_every_cause_under_the_outermost_message() {
        // A temporary directory gives a path that is guaranteed absent and
        // independent of the working directory, so the test is deterministic
        // wherever it runs rather than resting on a file not existing beside it.
        let dir = TempDir::new().expect("create a temporary directory");
        let missing = dir.path().join("no-such-prompts-file.toml");
        let error = Config::load(&missing).expect_err("a missing configuration is refused");
        let text = report(&error);
        let mut lines = text.lines();
        let first = lines.next().expect("the outermost message is printed");
        assert!(first.starts_with("error: read config "), "{text}");
        assert!(
            first.contains("no-such-prompts-file.toml"),
            "the failing path is named: {text}"
        );
        let cause = lines.next().expect("the I/O reason is printed under it");
        assert!(cause.starts_with("  caused by: "), "{text}");
        assert!(cause.len() > "  caused by: ".len(), "{text}");
    }
}
