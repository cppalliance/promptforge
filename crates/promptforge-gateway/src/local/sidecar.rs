//! HF metadata sidecar files written beside cached GGUFs.
//!
//! When a GGUF is provisioned from Hugging Face, we fetch lightweight metadata
//! (tokenizer `chat_template`, optional model card excerpt) and write a single
//! markdown file next to the GGUF with the same stem:
//!
//! ```text
//! models/gemma-3-27b-it-q4_0.gguf
//! models/gemma-3-27b-it-q4_0.md    <-- sidecar
//! ```
//!
//! The sidecar is YAML frontmatter plus optional fenced content. The gateway
//! reads it back as supplementary [`DialectEvidence`] when `/props` is thin.

use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

/// On-disk sidecar format version, emitted in the frontmatter (SIDECAR-005).
const SIDECAR_VERSION: u32 = 1;
/// Byte ceiling for a local sidecar read (SIDECAR-003).
const MAX_SIDECAR_BYTES: u64 = 1024 * 1024;
/// Byte ceiling for a remote `tokenizer_config.json` read (SIDECAR-002).
const MAX_TOKENIZER_BYTES: u64 = 8 * 1024 * 1024;

/// A typed failure while fetching remote sidecar metadata (SIDECAR-006).
///
/// The caller deliberately downgrades this to a debug log and proceeds without
/// a sidecar, rather than swallowing the distinction between "no template" and
/// "fetch failed".
#[derive(Debug, thiserror::Error)]
pub(crate) enum SidecarError {
    /// The source URL was not a recognized Hugging Face `resolve` URL.
    #[error("unsupported Hugging Face source URL")]
    UnsupportedUrl,
    /// The HTTP request failed at the transport layer.
    #[error("fetch tokenizer config")]
    Request(#[source] reqwest::Error),
    /// The endpoint returned a non-success status.
    #[error("fetch tokenizer config returned {status}")]
    Status {
        /// The rendered status.
        status: String,
    },
    /// Reading the (bounded) response body failed.
    #[error("read tokenizer config body")]
    Body(#[source] io::Error),
    /// The response body was not valid JSON.
    #[error("decode tokenizer config JSON")]
    Decode(#[source] serde_json::Error),
}

/// Metadata extracted from the sidecar markdown file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SidecarMeta {
    /// HF source URL that produced this sidecar.
    pub source: Option<String>,
    /// ISO-8601 timestamp of when the metadata was fetched.
    pub fetched: Option<String>,
    /// Raw Jinja chat template string from the tokenizer config.
    pub chat_template: Option<String>,
    /// Short model card excerpt, if available.
    pub card: Option<String>,
}

/// Returns the sidecar `.md` path for a given GGUF path.
pub(crate) fn sidecar_path(gguf: &Path) -> PathBuf {
    gguf.with_extension("md")
}

/// Reads and parses the sidecar file next to `gguf`, if it exists.
///
/// Returns `None` when the sidecar does not exist. Returns an error only on
/// genuine I/O failures (permissions, corrupt read).
pub(crate) fn read_sidecar(gguf: &Path) -> Result<Option<SidecarMeta>, io::Error> {
    let path = sidecar_path(gguf);
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // Bounded read (SIDECAR-003): a corrupt or oversized sidecar cannot exhaust
    // memory. Truncation only degrades best-effort metadata.
    let mut text = String::new();
    file.take(MAX_SIDECAR_BYTES).read_to_string(&mut text)?;
    Ok(Some(parse_sidecar(&text)))
}

/// Writes the sidecar `.md` beside `gguf` atomically.
///
/// The content is written to a temp file in the same directory and renamed into
/// place, so a reader never observes a half-written sidecar (SIDECAR-004).
pub(crate) fn write_sidecar(gguf: &Path, meta: &SidecarMeta) -> Result<(), io::Error> {
    let path = sidecar_path(gguf);
    let content = render_sidecar(meta);
    let mut temp = path.clone();
    let mut name = temp.file_name().unwrap_or_default().to_owned();
    name.push(".tmp");
    temp.set_file_name(name);

    let mut file = File::create(&temp)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    match fs::rename(&temp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ignored = fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// Renders the sidecar markdown string from metadata.
fn render_sidecar(meta: &SidecarMeta) -> String {
    let mut out = String::with_capacity(512);
    out.push_str("---\n");
    out.push_str("version: ");
    out.push_str(&SIDECAR_VERSION.to_string());
    out.push('\n');
    if let Some(source) = &meta.source {
        out.push_str("source: ");
        out.push_str(source);
        out.push('\n');
    }
    if let Some(fetched) = &meta.fetched {
        out.push_str("fetched: ");
        out.push_str(fetched);
        out.push('\n');
    }
    out.push_str("---\n");

    if let Some(template) = &meta.chat_template {
        out.push_str("\n## chat_template\n\n```jinja\n");
        out.push_str(template);
        if !template.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n");
    }

    if let Some(card) = &meta.card {
        out.push_str("\n## card\n\n");
        out.push_str(card);
        if !card.ends_with('\n') {
            out.push('\n');
        }
    }

    out
}

/// Parses a sidecar markdown string into [`SidecarMeta`].
fn parse_sidecar(text: &str) -> SidecarMeta {
    let mut meta = SidecarMeta::default();

    let Some(rest) = text.strip_prefix("---\n") else {
        return meta;
    };
    let Some(fm_end) = rest.find("\n---\n") else {
        return meta;
    };
    let frontmatter = &rest[..fm_end];
    let body = &rest[fm_end + 5..]; // skip "\n---\n"

    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("source: ") {
            meta.source = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("fetched: ") {
            meta.fetched = Some(value.to_owned());
        }
    }

    meta.chat_template = extract_fenced_block(body, "## chat_template", "jinja");
    meta.card = extract_section_text(body, "## card");

    meta
}

/// Extracts the content of a fenced code block following a heading.
fn extract_fenced_block(body: &str, heading: &str, lang: &str) -> Option<String> {
    let heading_pos = body.find(heading)?;
    let after_heading = &body[heading_pos + heading.len()..];
    let fence_open = format!("```{lang}\n");
    let fence_start = after_heading.find(&fence_open)?;
    let content_start = fence_start + fence_open.len();
    let remaining = &after_heading[content_start..];
    let fence_end = remaining.find("\n```")?;
    let content = &remaining[..fence_end];
    Some(content.to_owned())
}

/// Extracts plain text following a heading, up to the next heading or EOF.
fn extract_section_text(body: &str, heading: &str) -> Option<String> {
    let heading_pos = body.find(heading)?;
    let after_heading = &body[heading_pos + heading.len()..];
    // Skip the heading line's trailing newline(s)
    let trimmed = after_heading.trim_start_matches('\n');
    if trimmed.is_empty() {
        return None;
    }
    // Take until the next heading or EOF
    let end = trimmed.find("\n## ").unwrap_or(trimmed.len());
    let text = trimmed[..end].trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// Fetches HF tokenizer config for `chat_template` from a HF URL.
///
/// Attempts to resolve the repo/revision from the download URL and fetch
/// `tokenizer_config.json` (read with a byte cap, SIDECAR-002). `Ok(None)`
/// means the response was valid but carried no usable template.
///
/// # Errors
/// Returns a [`SidecarError`] the caller can log and deliberately downgrade
/// (SIDECAR-006) when the URL is unsupported, the request fails, the status is
/// non-success, or the body cannot be read or decoded.
pub(crate) fn fetch_hf_chat_template(
    client: &reqwest::blocking::Client,
    source_url: &str,
    bearer: Option<&str>,
) -> Result<Option<String>, SidecarError> {
    let (repo, revision) = parse_hf_url(source_url).ok_or(SidecarError::UnsupportedUrl)?;
    let api_url = format!("https://huggingface.co/{repo}/raw/{revision}/tokenizer_config.json");
    let mut request = client.get(&api_url);
    if let Some(token) = bearer {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(SidecarError::Request)?;
    if !response.status().is_success() {
        return Err(SidecarError::Status {
            status: response.status().to_string(),
        });
    }
    // Bounded body read: a hostile or mistaken endpoint cannot stream unbounded
    // bytes into memory.
    let mut buf = Vec::new();
    response
        .take(MAX_TOKENIZER_BYTES)
        .read_to_end(&mut buf)
        .map_err(SidecarError::Body)?;
    let json: serde_json::Value = serde_json::from_slice(&buf).map_err(SidecarError::Decode)?;
    // chat_template can be a string or an array of objects with "template" fields.
    let template = match &json["chat_template"] {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => arr
            .iter()
            .find(|entry| entry.get("name").and_then(|n| n.as_str()) == Some("default"))
            .or_else(|| arr.first())
            .and_then(|entry| entry.get("template"))
            .and_then(|t| t.as_str())
            .map(String::from),
        _ => None,
    };
    Ok(template)
}

/// Parses a HF download URL into `(repo, revision)`.
///
/// Expects patterns like:
/// `https://huggingface.co/{org}/{model}/resolve/{rev}/{file}`
fn parse_hf_url(url: &str) -> Option<(String, String)> {
    let path = url.strip_prefix("https://huggingface.co/")?;
    // Remove query string
    let path = path.split('?').next().unwrap_or(path);
    let parts: Vec<&str> = path.splitn(5, '/').collect();
    // parts: [org, model, "resolve", revision, filename]
    if parts.len() >= 5 && parts[2] == "resolve" {
        Some((format!("{}/{}", parts[0], parts[1]), parts[3].to_owned()))
    } else {
        None
    }
}

/// Returns the current UTC timestamp as an ISO-8601 string suitable for the
/// `fetched` frontmatter field, formatted from [`std::time::SystemTime`] with no
/// external crate or subprocess.
pub(crate) fn utc_now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    format_unix_utc(secs)
}

/// Format a Unix timestamp (seconds since 1970-01-01 UTC) as `YYYY-MM-DDThh:mm:ssZ`.
///
/// Uses Howard Hinnant's days-to-civil algorithm; valid for all dates at or
/// after the Unix epoch.
fn format_unix_utc(secs: u64) -> String {
    let days = secs / 86_400;
    let second_of_day = secs % 86_400;
    let (hour, minute, second) = (
        second_of_day / 3_600,
        (second_of_day % 3_600) / 60,
        second_of_day % 60,
    );

    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    fn sample_meta() -> SidecarMeta {
        SidecarMeta {
            source: Some("https://huggingface.co/google/gemma-3-27b-it-qat-q4_0-gguf/resolve/main/gemma-3-27b-it-q4_0.gguf".to_owned()),
            fetched: Some("2026-08-08T12:00:00Z".to_owned()),
            chat_template: Some("{{ bos_token }}{% for message in messages %}<start_of_turn>{{ message['role'] }}\n{{ message['content'] }}<end_of_turn>\n{% endfor %}".to_owned()),
            card: Some("Gemma 3 27B instruction-tuned model.".to_owned()),
        }
    }

    #[test]
    fn formats_unix_epoch_boundaries() {
        assert_eq!(super::format_unix_utc(0), "1970-01-01T00:00:00Z");
        // 2021-01-01T00:00:00Z == 1609459200
        assert_eq!(
            super::format_unix_utc(1_609_459_200),
            "2021-01-01T00:00:00Z"
        );
        // 2000-02-29T12:34:56Z (leap day) == 951827696
        assert_eq!(super::format_unix_utc(951_827_696), "2000-02-29T12:34:56Z");
    }

    #[test]
    fn sidecar_path_replaces_extension() {
        let gguf = PathBuf::from("/cache/models/gemma-3-27b-it-q4_0.gguf");
        assert_eq!(
            sidecar_path(&gguf),
            PathBuf::from("/cache/models/gemma-3-27b-it-q4_0.md")
        );
    }

    #[test]
    fn round_trip_sidecar() {
        let meta = sample_meta();
        let rendered = render_sidecar(&meta);
        let parsed = parse_sidecar(&rendered);
        assert_eq!(parsed, meta);
    }

    #[test]
    fn rendered_sidecar_carries_a_format_version() {
        // SIDECAR-005: the on-disk format is versioned, and an unknown/newer
        // version line is tolerated on read (forward-compatible round trip).
        let rendered = render_sidecar(&sample_meta());
        assert!(
            rendered.contains(&format!("version: {SIDECAR_VERSION}")),
            "sidecar should record its format version"
        );
        let bumped = rendered.replacen(&format!("version: {SIDECAR_VERSION}"), "version: 999", 1);
        assert_eq!(parse_sidecar(&bumped), sample_meta());
    }

    #[test]
    fn write_and_read_sidecar_file() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("model.gguf");
        fs::write(&gguf, b"fake-gguf").expect("write gguf");

        let meta = sample_meta();
        write_sidecar(&gguf, &meta).expect("write sidecar");

        let read_back = read_sidecar(&gguf).expect("read").expect("should exist");
        assert_eq!(read_back, meta);
    }

    #[test]
    fn read_sidecar_returns_none_when_missing() {
        let dir = TempDir::new().expect("tempdir");
        let gguf = dir.path().join("absent.gguf");
        let result = read_sidecar(&gguf).expect("no io error");
        assert!(result.is_none());
    }

    #[test]
    fn parse_sidecar_minimal() {
        let text = "---\nsource: https://example.com/model.gguf\n---\n";
        let meta = parse_sidecar(text);
        assert_eq!(
            meta.source.as_deref(),
            Some("https://example.com/model.gguf")
        );
        assert!(meta.chat_template.is_none());
        assert!(meta.card.is_none());
    }

    #[test]
    fn parse_sidecar_no_frontmatter() {
        let meta = parse_sidecar("just some text");
        assert_eq!(meta, SidecarMeta::default());
    }

    #[test]
    fn parse_hf_url_extracts_repo_and_revision() {
        let url =
            "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf";
        let (repo, rev) = parse_hf_url(url).expect("should parse");
        assert_eq!(repo, "unsloth/Qwen3.5-9B-GGUF");
        assert_eq!(rev, "main");
    }

    #[test]
    fn parse_hf_url_with_query_string() {
        let url = "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true";
        let (repo, rev) = parse_hf_url(url).expect("should parse");
        assert_eq!(repo, "Qwen/Qwen3-0.6B-GGUF");
        assert_eq!(rev, "main");
    }

    #[test]
    fn parse_hf_url_rejects_non_hf() {
        assert!(parse_hf_url("https://example.com/foo/bar.gguf").is_none());
    }

    #[test]
    fn render_sidecar_without_optional_fields() {
        let meta = SidecarMeta {
            source: Some("https://example.com/model.gguf".to_owned()),
            fetched: Some("2026-01-01T00:00:00Z".to_owned()),
            chat_template: None,
            card: None,
        };
        let rendered = render_sidecar(&meta);
        assert!(rendered.contains("source: https://example.com/model.gguf"));
        assert!(!rendered.contains("## chat_template"));
        assert!(!rendered.contains("## card"));
    }
}
