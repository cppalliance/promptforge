//! Sheet-building binary: a thin `main` over the `gateway-cloud-providers`
//! library, compiled and run by the aggregation workflow and runnable
//! locally for testing and sheet building.
//!
//! Reads each provider's API key from the environment variable named by
//! its descriptor (after loading operator secrets from
//! `~/.promptforge/cloud-provider-secrets.env` when present), downloads the previous release's `models.json` when
//! `MODELS_SHEET_PREVIOUS_URL` is set (an unset URL or an HTTP 404 means
//! first run: the build proceeds with no previous sheet; any other
//! download failure is fatal, since silently losing history would demote
//! every slice to `unavailable`), and writes the merged sheet as
//! pretty-printed JSON to the output path named by the first argument,
//! defaulting to `~/.promptforge/cloud-provider-models.json`.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use gateway_api::Sheet;

/// Environment variable carrying the previous release's sheet URL.
const PREVIOUS_SHEET_URL_ENV: &str = "MODELS_SHEET_PREVIOUS_URL";

/// The sheet's default output filename.
const DEFAULT_OUTPUT_NAME: &str = "cloud-provider-models.json";

#[tokio::main]
async fn main() -> ExitCode {
    load_secrets();
    match run().await {
        Ok(path) => {
            eprintln!("shared-cloud-providers: wrote {path}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("shared-cloud-providers: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the operator's home directory, mirroring the ART-009
/// convention (`USERPROFILE` on Windows, `HOME` otherwise) rather than
/// importing `gateway-local`, which would pull the local-inference
/// stack into this thin sheet-building binary.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    home.filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// Load operator secrets from `<home>/.promptforge/cloud-provider-secrets.env`,
/// overriding the process environment so local runs need no exported keys.
///
/// A missing file or unresolvable home earns a stderr note and a
/// malformed file a stderr warning; the run continues with the
/// environment either way.
fn load_secrets() {
    let Some(home) = home_dir() else {
        eprintln!(
            "shared-cloud-providers: note: home directory unresolved; skipping the secrets file"
        );
        return;
    };
    let path = home.join(".promptforge").join("cloud-provider-secrets.env");
    match dotenvy::from_path_override(&path) {
        Ok(()) => {}
        Err(dotenvy::Error::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "shared-cloud-providers: note: no secrets file at {}; using the environment",
                path.display()
            );
        }
        Err(err) => {
            eprintln!(
                "shared-cloud-providers: warning: secrets file at {} not loaded: {err}",
                path.display()
            );
        }
    }
}

/// The default output path: `cloud-provider-models.json` in the profile
/// directory `<home>/.promptforge`, created when absent. An unresolvable
/// home or an uncreatable profile directory falls back to the current
/// directory with a stderr note.
fn default_output() -> String {
    let Some(home) = home_dir() else {
        eprintln!(
            "shared-cloud-providers: note: home directory unresolved; writing ./{DEFAULT_OUTPUT_NAME}"
        );
        return DEFAULT_OUTPUT_NAME.to_owned();
    };
    let dir = home.join(".promptforge");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "shared-cloud-providers: warning: profile directory {} not created: {err}; writing ./{DEFAULT_OUTPUT_NAME}",
            dir.display()
        );
        return DEFAULT_OUTPUT_NAME.to_owned();
    }
    dir.join(DEFAULT_OUTPUT_NAME).to_string_lossy().into_owned()
}

/// Build the sheet and write it to the output path, returning the path.
async fn run() -> Result<String, Box<dyn std::error::Error>> {
    let output = std::env::args().nth(1).unwrap_or_else(default_output);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let url = std::env::var(PREVIOUS_SHEET_URL_ENV).ok();
    let previous = match previous_sheet(&client, url.as_deref()).await? {
        PreviousSheet::FirstRun => None,
        PreviousSheet::Fetched(sheet) => Some(sheet),
    };
    let keys = |provider: &gateway_cloud_providers::Provider| {
        provider
            .key_env
            .and_then(|key_env| std::env::var(key_env).ok())
    };
    let sheet = gateway_cloud_providers::build_sheet(&client, previous, &keys).await;
    let mut json = serde_json::to_string_pretty(&sheet)?;
    json.push('\n');
    std::fs::write(&output, json)?;
    Ok(output)
}

/// The outcome of resolving the previous release's sheet.
enum PreviousSheet {
    /// No URL configured, or the release does not exist yet (HTTP 404):
    /// build without history.
    FirstRun,
    /// The previous release's sheet, fetched and parsed.
    Fetched(Sheet),
}

/// Resolve the previous release's sheet. An unset URL and an HTTP 404
/// both mean first run; any other failure - transport error, non-404
/// non-success status, unparseable body - is fatal, since silently
/// losing history would demote every slice to `unavailable`.
async fn previous_sheet(
    client: &reqwest::Client,
    url: Option<&str>,
) -> Result<PreviousSheet, Box<dyn std::error::Error>> {
    let Some(url) = url.filter(|url| !url.is_empty()) else {
        return Ok(PreviousSheet::FirstRun);
    };
    match gateway_cloud_providers::fetch_sheet(client, url).await {
        Ok(sheet) => Ok(PreviousSheet::Fetched(sheet)),
        Err(gateway_cloud_providers::FetchError::NotFound { .. }) => {
            eprintln!(
                "shared-cloud-providers: no previous release at {url} (HTTP 404); building without history"
            );
            Ok(PreviousSheet::FirstRun)
        }
        Err(err) => Err(format!("previous sheet at {url}: {err}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve one HTTP response with `status` carrying `body`, returning
    /// the URL to request.
    fn serve_once(status: &'static str, body: &'static str) -> String {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        let addr = listener.local_addr().expect("fixture server addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept fixture client");
            // Read the request first: replying before the client finishes
            // sending is an HTTP protocol error. A short read timeout bounds
            // the capture without a sleep; once the client awaits the
            // response, the next read simply times out.
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(200)));
            let mut buf = [0_u8; 4096];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture response");
        });
        format!("http://{addr}/models.json")
    }

    #[tokio::test]
    async fn previous_sheet_without_url_is_first_run() {
        let client = reqwest::Client::new();
        let outcome = previous_sheet(&client, None)
            .await
            .expect("an unset URL is not an error");
        assert!(
            matches!(outcome, PreviousSheet::FirstRun),
            "an unset URL must mean first run"
        );
        let outcome = previous_sheet(&client, Some(""))
            .await
            .expect("an empty URL is not an error");
        assert!(
            matches!(outcome, PreviousSheet::FirstRun),
            "an empty URL must mean first run"
        );
    }

    #[tokio::test]
    async fn previous_sheet_treats_404_as_first_run() {
        let url = serve_once("404 Not Found", "not found");
        let client = reqwest::Client::new();
        let outcome = previous_sheet(&client, Some(&url))
            .await
            .expect("a 404 previous release is not an error");
        assert!(
            matches!(outcome, PreviousSheet::FirstRun),
            "a 404 must mean first run: the release does not exist yet"
        );
    }

    #[tokio::test]
    async fn previous_sheet_propagates_transport_error() {
        let client = reqwest::Client::new();
        let result = previous_sheet(&client, Some("http://127.0.0.1:1/models.json")).await;
        let Err(err) = result else {
            panic!("an unreachable previous-sheet URL must be fatal");
        };
        assert!(
            err.to_string().contains("http://127.0.0.1:1/models.json"),
            "the error must name the URL: {err}"
        );
    }

    #[tokio::test]
    async fn previous_sheet_propagates_500() {
        let url = serve_once("500 Internal Server Error", "boom");
        let client = reqwest::Client::new();
        let result = previous_sheet(&client, Some(&url)).await;
        let Err(err) = result else {
            panic!("a 500 previous-sheet response must be fatal");
        };
        assert!(
            err.to_string().contains(url.as_str()),
            "the error must name the URL: {err}"
        );
    }

    #[tokio::test]
    async fn previous_sheet_propagates_unparseable_200() {
        let url = serve_once("200 OK", "this is not a sheet");
        let client = reqwest::Client::new();
        let result = previous_sheet(&client, Some(&url)).await;
        let Err(err) = result else {
            panic!("a 200 with an unparseable body must be fatal");
        };
        assert!(
            err.to_string().contains(url.as_str()),
            "the error must name the URL: {err}"
        );
    }
}
