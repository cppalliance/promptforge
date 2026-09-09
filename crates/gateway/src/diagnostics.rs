//! The `diagnostics` subcommand's report: formatted JSON naming the state
//! directory, the config path, the log files, and the gateway discovery file,
//! plus whether a gateway is running right now.
//!
//! The report is read-only by contract: it never initializes logging,
//! never rotates a log, never parses configuration, and never mutates the
//! state directory - a stale gateway discovery file reads as not-running and
//! stays on disk for the next launch to clean. It never carries the
//! bearer key, environment values, config contents, or log contents.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Builds the diagnostics report as formatted JSON.
///
/// `explicit_config` is the CLI- or environment-resolved config path when
/// one was given; without it the report names the path boot discovery
/// would use (the profile location when nothing exists yet).
#[must_use]
pub fn diagnostics_json(explicit_config: Option<PathBuf>) -> String {
    let run_dir = shared_sidecar::default_run_dir();
    let state_dir = run_dir
        .as_deref()
        .and_then(Path::parent)
        .map(Path::to_path_buf);
    let config_path = crate::boot::discover_for_report(explicit_config);
    let running = run_dir.as_deref().is_some_and(shared_sidecar::is_running);
    render(
        state_dir.as_deref(),
        config_path.as_deref(),
        run_dir.as_deref(),
        running,
    )
}

/// A JSON string literal for `text`, with every escape handled.
fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| unreachable!("serializing a string cannot fail"))
}

/// A path rendered as a JSON string, or `null` when the location could
/// not be determined at all.
fn json_path(path: Option<&Path>) -> String {
    path.map_or_else(
        || "null".to_string(),
        |path| json_string(&path.to_string_lossy()),
    )
}

/// One `{ "path": ..., "exists": ... }` entry.
fn path_entry(path: Option<&Path>) -> String {
    format!(
        "{{ \"path\": {}, \"exists\": {} }}",
        json_path(path),
        path.is_some_and(Path::is_file)
    )
}

/// Renders the report in the contract's shape and key order. Pure apart
/// from the `exists` stat calls, so tests drive it with fixture
/// directories.
fn render(
    state_dir: Option<&Path>,
    config_path: Option<&Path>,
    run_dir: Option<&Path>,
    running: bool,
) -> String {
    let discovery_file = run_dir.map(shared_sidecar::gateway_discovery_file_path);
    let mut out = String::new();
    // Writing to a String is infallible, so each writeln's Result is
    // dropped on purpose.
    let _ = writeln!(out, "{{");
    let _ = writeln!(out, "  \"state_dir\": {},", json_path(state_dir));
    let _ = writeln!(out, "  \"config\": {},", path_entry(config_path));
    let _ = writeln!(out, "  \"logs\": {{");
    let current = state_dir.map(|dir| gateway_logging::LogConfig::new(dir).log_path());
    let _ = writeln!(out, "    \"current\": {},", path_entry(current.as_deref()));
    if let Some(state_dir) = state_dir {
        let retained = gateway_logging::LogConfig::new(state_dir).retained_log_paths();
        let _ = writeln!(out, "    \"retained\": [");
        for (index, path) in retained.iter().enumerate() {
            let comma = if index + 1 == retained.len() { "" } else { "," };
            let _ = writeln!(out, "      {}{comma}", path_entry(Some(path)));
        }
        let _ = writeln!(out, "    ]");
    } else {
        let _ = writeln!(out, "    \"retained\": []");
    }
    let _ = writeln!(out, "  }},");
    let _ = writeln!(
        out,
        "  \"connection_file\": {},",
        path_entry(discovery_file.as_deref())
    );
    let _ = writeln!(out, "  \"running\": {running},");
    let _ = writeln!(
        out,
        "  \"version\": {}",
        json_string(env!("CARGO_PKG_VERSION"))
    );
    let _ = writeln!(out, "}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the report back as JSON for shape assertions.
    fn parse(rendered: &str) -> serde_json::Value {
        serde_json::from_str(rendered).expect("the report is valid JSON")
    }

    #[test]
    fn the_report_matches_the_contract_shape() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let state_dir = temp.path().join("state");
        let run_dir = state_dir.join("run");
        std::fs::create_dir_all(state_dir.join("logs")).expect("logs dir");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        std::fs::write(state_dir.join("logs/gateway.log"), "current").expect("seed log");
        std::fs::write(state_dir.join("logs/gateway.log.1"), "previous").expect("seed rotation");
        let config = state_dir.join("gateway.toml");
        std::fs::write(&config, "config-version = 2\n").expect("seed config");

        let rendered = render(Some(&state_dir), Some(&config), Some(&run_dir), false);
        let report = parse(&rendered);

        // The exact key set, in the contract's order: serde_json sorts
        // parsed objects, so the order assertion runs on the raw text.
        let mut at = 0;
        for key in [
            "\"state_dir\"",
            "\"config\"",
            "\"logs\"",
            "\"current\"",
            "\"retained\"",
            "\"connection_file\"",
            "\"running\"",
            "\"version\"",
        ] {
            let found = rendered[at..]
                .find(key)
                .unwrap_or_else(|| panic!("{key} appears after position {at}: {rendered}"));
            at += found + key.len();
        }
        assert_eq!(
            report["state_dir"].as_str().expect("a string"),
            state_dir.to_string_lossy()
        );
        assert_eq!(
            report["config"]["path"].as_str(),
            Some(&*config.to_string_lossy())
        );
        assert_eq!(report["config"]["exists"], true);
        assert!(
            report["logs"]["current"]["path"]
                .as_str()
                .expect("a string")
                .ends_with("gateway.log")
        );
        assert_eq!(report["logs"]["current"]["exists"], true);
        let retained = report["logs"]["retained"].as_array().expect("an array");
        assert_eq!(
            retained.len(),
            5,
            "five retained slots, one per kept previous run"
        );
        assert_eq!(retained[0]["exists"], true, "the seeded .1 exists");
        assert_eq!(retained[4]["exists"], false, ".5 was never written");
        assert!(
            retained[0]["path"]
                .as_str()
                .expect("a string")
                .ends_with("gateway.log.1")
        );
        assert!(
            report["connection_file"]["path"]
                .as_str()
                .expect("a string")
                .ends_with("gateway.json")
        );
        assert_eq!(report["connection_file"]["exists"], false);
        assert_eq!(report["running"], false);
        assert_eq!(report["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn the_report_carries_no_secret_material() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let state_dir = temp.path().join("state");
        let run_dir = state_dir.join("run");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        // A live-looking gateway discovery file with a bearer key: the report
        // names the file but never reads its contents into the output.
        shared_sidecar::GatewayDiscoveryFile {
            port: 8081,
            api_key: "the-bearer-key".to_owned(),
            pid: 4242,
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-05T12:00:00Z".to_owned(),
        }
        .write_to(&run_dir)
        .expect("write the gateway discovery file");

        let rendered = render(Some(&state_dir), None, Some(&run_dir), false);
        assert!(
            !rendered.contains("the-bearer-key"),
            "the report never carries the bearer key: {rendered}"
        );
        assert_eq!(parse(&rendered)["connection_file"]["exists"], true);
    }

    #[test]
    fn the_report_mutates_nothing() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let state_dir = temp.path().join("state");
        let run_dir = state_dir.join("run");
        std::fs::create_dir_all(state_dir.join("logs")).expect("logs dir");
        std::fs::create_dir_all(&run_dir).expect("run dir");
        std::fs::write(state_dir.join("logs/gateway.log"), "the running log").expect("seed log");
        std::fs::write(run_dir.join("gateway.json"), b"not json").expect("a stale file");

        let before = std::fs::read_to_string(state_dir.join("logs/gateway.log")).expect("read");
        render(Some(&state_dir), None, Some(&run_dir), false);

        assert_eq!(
            std::fs::read_to_string(state_dir.join("logs/gateway.log")).expect("read"),
            before,
            "the current log is untouched"
        );
        assert!(
            !state_dir.join("logs/gateway.log.1").exists(),
            "no rotation happened"
        );
        assert!(
            run_dir.join("gateway.json").exists(),
            "a stale gateway discovery file is left for the next launch to clean"
        );
    }

    #[test]
    fn an_unlocatable_state_dir_renders_null_paths() {
        let report = parse(&render(None, None, None, false));
        assert!(report["state_dir"].is_null());
        assert!(report["config"]["path"].is_null());
        assert_eq!(report["config"]["exists"], false);
        assert!(report["logs"]["current"]["path"].is_null());
        assert_eq!(
            report["logs"]["retained"]
                .as_array()
                .expect("an array")
                .len(),
            0,
            "no state dir, no retained list"
        );
        assert!(report["connection_file"]["path"].is_null());
        assert_eq!(report["running"], false);
    }

    #[test]
    fn windows_path_separators_survive_json_escaping() {
        let report = parse(&render(
            Some(Path::new("C:\\Users\\v\\.promptforge")),
            None,
            None,
            false,
        ));
        assert_eq!(
            report["state_dir"].as_str(),
            Some("C:\\Users\\v\\.promptforge"),
            "backslashes round-trip through the JSON escaping"
        );
    }
}
