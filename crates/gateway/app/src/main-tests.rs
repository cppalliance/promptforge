//! Tests for the binary's log filter, command-line parsing, and fatal-error logging.

use super::*;

#[test]
fn the_default_filter_keeps_gateway_info_and_quiets_whisper_cpp() {
    let filter = tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER);
    assert_eq!(
        filter.max_level_hint(),
        Some(tracing::level_filters::LevelFilter::INFO),
        "gateway crates log at info and nothing enables debug"
    );
    assert!(
        DEFAULT_LOG_FILTER.contains("whisper_cpp=warn"),
        "the noisy STT dependency stays at warn: {DEFAULT_LOG_FILTER}"
    );
    assert!(
        !DEFAULT_LOG_FILTER.contains("debug") && !DEFAULT_LOG_FILTER.contains("trace"),
        "the default never enables debug or trace: {DEFAULT_LOG_FILTER}"
    );
}

/// A fatal returned error is logged once with its complete source
/// chain, and the queue drains to disk before the process exits.
#[test]
fn a_fatal_error_is_logged_once_with_its_full_chain_then_drained() {
    #[derive(Debug)]
    struct Chain(&'static str, Option<Box<Chain>>);

    impl std::fmt::Display for Chain {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Chain {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1
                .as_deref()
                .map(|cause| cause as &dyn std::error::Error)
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = LogRuntime::start(LogConfig::new(temp.path().join("state")))
        .expect("start the log pipeline");
    let log_path = runtime.path().to_path_buf();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(runtime.writer())
        .finish();
    let error = Chain(
        "serve the gateway",
        Some(Box::new(Chain(
            "bind 127.0.0.1:8081",
            Some(Box::new(Chain("address already in use", None))),
        ))),
    );
    tracing::subscriber::with_default(subscriber, || log_error_chain(&error));
    // The logger shuts down last, which is what drains the chain.
    runtime.shutdown().expect("the queue drains before exit");

    let log = std::fs::read_to_string(&log_path).expect("read the log");
    for link in [
        "error: serve the gateway",
        "caused by: bind 127.0.0.1:8081",
        "caused by: address already in use",
    ] {
        assert_eq!(
            log.matches(link).count(),
            1,
            "each chain link lands exactly once: {link}\n{log}"
        );
    }
}
