//! HF metadata sidecar tests.

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
fn rendered_sidecar_records_a_format_version() {
    // SIDECAR-005: the on-disk format is versioned.
    let rendered = render_sidecar(&sample_meta());
    assert!(
        rendered.contains(&format!("version: {SIDECAR_VERSION}")),
        "sidecar should record its format version"
    );
}

#[test]
fn unknown_version_is_rejected_not_misparsed() {
    // SIDECAR-005: an unknown/newer version keeps only the frontmatter and
    // refuses to trust a body it does not understand.
    let rendered = render_sidecar(&sample_meta());
    let bumped = rendered.replacen(&format!("version: {SIDECAR_VERSION}"), "version: 999", 1);
    let parsed = parse_sidecar(&bumped);
    assert_eq!(parsed.source, sample_meta().source);
    assert_eq!(parsed.fetched, sample_meta().fetched);
    assert!(
        parsed.chat_template.is_none(),
        "unknown version body rejected"
    );
    assert!(parsed.card.is_none());
}

#[test]
fn v1_legacy_sidecar_still_parses() {
    // A v1 markdown sidecar round-trips its template.
    let v1 = "---\nversion: 1\nsource: https://hf/x\n---\n\n## chat_template\n\n```jinja\n{{ bos }}\n```\n";
    let meta = parse_sidecar(v1);
    assert_eq!(meta.source.as_deref(), Some("https://hf/x"));
    assert_eq!(meta.chat_template.as_deref(), Some("{{ bos }}"));
}

#[test]
fn v2_round_trips_template_with_embedded_fences_and_headings() {
    // SIDECAR-005: the v2 JSON body round-trips a template that itself
    // contains ``` fences or `##` headings losslessly.
    let hostile = SidecarMeta {
        source: Some("https://huggingface.co/x/y/resolve/main/m.gguf".to_owned()),
        fetched: Some("2026-08-10T00:00:00Z".to_owned()),
        chat_template: Some("## not a heading\n```\nembedded fence\n```\n{{ content }}".to_owned()),
        card: Some("card with\n## heading and ``` fence".to_owned()),
    };
    let rendered = render_sidecar(&hostile);
    assert_eq!(parse_sidecar(&rendered), hostile);
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
    let url = "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf";
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
