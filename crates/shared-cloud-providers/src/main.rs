//! Sheet-building binary: a thin `main` over the `shared-cloud-providers`
//! library, compiled and run by the aggregation workflow and runnable
//! locally for testing and sheet building.
//!
//! Reads each provider's API key from the environment variable named by
//! its descriptor, downloads the previous release's `models.json` when
//! `MODELS_SHEET_PREVIOUS_URL` is set (an unset URL or a failed download
//! is tolerated: the build proceeds with no previous sheet), and writes
//! the merged sheet as pretty-printed JSON to the output path named by
//! the first argument, defaulting to `./models.json`.

use std::process::ExitCode;
use std::time::Duration;

/// Environment variable carrying the previous release's sheet URL.
const PREVIOUS_SHEET_URL_ENV: &str = "MODELS_SHEET_PREVIOUS_URL";

#[tokio::main]
async fn main() -> ExitCode {
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

/// Build the sheet and write it to the output path, returning the path.
async fn run() -> Result<String, Box<dyn std::error::Error>> {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "models.json".to_owned());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    let previous = previous_sheet(&client).await;
    let keys = |provider: &shared_cloud_providers::Provider| std::env::var(provider.key_env).ok();
    let sheet = shared_cloud_providers::build_sheet(&client, previous, &keys).await;
    let mut json = serde_json::to_string_pretty(&sheet)?;
    json.push('\n');
    std::fs::write(&output, json)?;
    Ok(output)
}

/// Download the previous release's sheet when its URL is configured.
/// Both an unset URL (first run) and a failed download (the release may
/// not exist yet) are tolerated by building with no previous sheet.
async fn previous_sheet(client: &reqwest::Client) -> Option<shared_gateway_api::Sheet> {
    let url = match std::env::var(PREVIOUS_SHEET_URL_ENV) {
        Ok(url) if !url.is_empty() => url,
        _ => return None,
    };
    match shared_cloud_providers::fetch_sheet(client, &url).await {
        Ok(sheet) => Some(sheet),
        Err(err) => {
            eprintln!(
                "shared-cloud-providers: previous sheet at {url} unavailable ({err}); building without it"
            );
            None
        }
    }
}
