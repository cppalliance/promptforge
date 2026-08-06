//! The `promptforge-mcp-server` binary:
//! `promptforge-mcp-server serve [--stdio] <prompts.toml>`.
//!
//! Boot either produces a complete catalog or the process refuses to serve.
//! Every fault the resolution pass found is printed before the nonzero exit, so
//! an operator fixes them in one pass rather than one restart each.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use promptforge_mcp_server::{
    Catalog, CatalogHandle, Config, OnBroken, PreparedTools, Retrieval, Watcher, serve_http,
    serve_stdio,
};

/// What the process prints when the arguments are not the two shapes it takes.
const USAGE: &str = "usage: promptforge-mcp-server serve [--stdio] <prompts.toml>";

/// Entry point. The runtime is built inside `main` rather than by an attribute
/// macro, matching the gateway, so a future service wrapper can construct it
/// the same way.
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(invocation) = Invocation::parse(&arguments) else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    // On stdio the protocol owns standard output, so a log line written there
    // would corrupt the wire. Everything else logs to stdout.
    if invocation.stdio {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stdout)
            .init();
    }
    match run(&invocation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", report(error.as_ref()));
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

/// One command line, parsed.
struct Invocation {
    /// Serve over standard input and output rather than binding a port.
    stdio: bool,
    /// The configuration file to load.
    config: String,
}

impl Invocation {
    /// Parses `serve <config>` or `serve --stdio <config>`, and nothing else.
    fn parse(arguments: &[String]) -> Option<Invocation> {
        let mut rest = arguments.iter().map(String::as_str);
        if rest.next()? != "serve" {
            return None;
        }
        let first = rest.next()?;
        let (stdio, config) = if first == "--stdio" {
            (true, rest.next()?)
        } else {
            (false, first)
        };
        if rest.next().is_some() {
            return None;
        }
        Some(Invocation {
            stdio,
            config: config.to_string(),
        })
    }
}

/// Loads the configuration, resolves the catalog, and serves the chosen
/// transport until the process is stopped.
fn run(invocation: &Invocation) -> Result<(), Box<dyn std::error::Error>> {
    let source = Path::new(&invocation.config);
    let config = Config::load(source)?;
    // Boot refuses an incomplete catalog: a service that starts with nine of
    // ten prompts is one whose catalog silently disagrees with its own
    // configuration, and a client sees only a missing tool.
    let catalog = Catalog::resolve(&config, OnBroken::Reject)?;
    // Tool capability binding is synchronous and model-backed. Prepare its
    // complete live registry and picker once before the async executor exists,
    // then share the immutable result across every run.
    let tools = Arc::new(PreparedTools::new(&config.gateway)?);
    // Prepare the optional prompt-retrieval index before the runtime for the
    // same blocking-CPU reason. Unlike the required execution picker above, a
    // failed retrieval index costs `need_prompt` and nothing else.
    let retrieval = Arc::new(Retrieval::start(&catalog));
    let config = Arc::new(config);
    let catalog = Arc::new(CatalogHandle::new(catalog));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let stdio = invocation.stdio;
    runtime.block_on(async move {
        // Started inside the runtime, because the debounce window is a task, and
        // held for as long as the transport serves: dropping the guard stops the
        // watches.
        let _watcher = Watcher::start(
            source,
            Arc::clone(&config),
            Arc::clone(&catalog),
            Arc::clone(&retrieval),
        )?;
        if stdio {
            serve_stdio(config, catalog, retrieval, tools).await?;
        } else {
            serve_http(config, catalog, retrieval, tools).await?;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Invocation::parse` over borrowed arguments, which is how a test spells
    /// a command line.
    fn parse(arguments: &[&str]) -> Option<Invocation> {
        let owned: Vec<String> = arguments.iter().map(|a| (*a).to_string()).collect();
        Invocation::parse(&owned)
    }

    #[test]
    fn parses_the_http_shape() {
        let invocation = parse(&["serve", "prompts.toml"]).expect("serve <config> is accepted");
        assert!(!invocation.stdio);
        assert_eq!(invocation.config, "prompts.toml");
    }

    #[test]
    fn parses_the_stdio_shape() {
        let invocation =
            parse(&["serve", "--stdio", "prompts.toml"]).expect("serve --stdio <config>");
        assert!(invocation.stdio);
        assert_eq!(invocation.config, "prompts.toml");
    }

    #[test]
    fn rejects_a_flag_after_the_config() {
        // The position is part of the shape: `--stdio` is the second argument or
        // it is not the flag, since a configuration may legitimately be named
        // anything.
        assert!(parse(&["serve", "prompts.toml", "--stdio"]).is_none());
    }

    #[test]
    fn rejects_a_trailing_extra_argument() {
        assert!(parse(&["serve", "prompts.toml", "extra"]).is_none());
        assert!(parse(&["serve", "--stdio", "prompts.toml", "extra"]).is_none());
    }

    #[test]
    fn rejects_a_missing_config_and_a_missing_subcommand() {
        assert!(parse(&[]).is_none());
        assert!(parse(&["serve"]).is_none());
        assert!(parse(&["serve", "--stdio"]).is_none());
        assert!(parse(&["run", "prompts.toml"]).is_none());
        assert!(parse(&["--stdio", "prompts.toml"]).is_none());
    }

    #[test]
    fn a_report_carries_every_cause_under_the_outermost_message() {
        let error = Config::load(Path::new("no-such-prompts-file.toml"))
            .expect_err("a missing configuration is refused");
        let text = report(&error);
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("error: read config no-such-prompts-file.toml")
        );
        let cause = lines.next().expect("the I/O reason is printed under it");
        assert!(cause.starts_with("  caused by: "), "{text}");
        assert!(cause.len() > "  caused by: ".len(), "{text}");
    }
}
