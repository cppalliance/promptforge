//! Speech-to-text catalog entries and the digest-pinned recommended pair.

use serde::{Deserialize, Deserializer, Serialize};

/// Default sliding-window length for interim transcription, in seconds.
const DEFAULT_STT_WINDOW_SECONDS: u64 = 15;

/// Default interval between interim transcriptions, in milliseconds.
const DEFAULT_STT_INTERVAL_MS: u64 = 500;

/// The canonical `[stt]` pipeline tuning section.
///
/// Model sources and roles are defined in global `[[stt_model]]` catalog
/// entries and profiles enable them through membership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct SttPipelineConfig {
    /// Seconds of trailing audio each interim pass transcribes.
    window_seconds: u64,
    /// Milliseconds between interim passes while a take is recording.
    interval_ms: u64,
    /// Domain terms whisper is biased toward. Empty disables biasing.
    vocabulary: Vec<String>,
    /// Which whisper runtime build to download. Defaults to `auto`.
    #[serde(default, skip_serializing_if = "WhisperBackend::is_auto")]
    whisper_backend: WhisperBackend,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct RawSttPipelineConfig {
    window_seconds: u64,
    interval_ms: u64,
    vocabulary: Vec<String>,
    #[serde(default, skip_serializing_if = "WhisperBackend::is_auto")]
    whisper_backend: WhisperBackend,
}

impl Default for RawSttPipelineConfig {
    fn default() -> Self {
        Self {
            window_seconds: DEFAULT_STT_WINDOW_SECONDS,
            interval_ms: DEFAULT_STT_INTERVAL_MS,
            vocabulary: Vec::new(),
            whisper_backend: WhisperBackend::Auto,
        }
    }
}

impl Default for SttPipelineConfig {
    fn default() -> Self {
        Self {
            window_seconds: DEFAULT_STT_WINDOW_SECONDS,
            interval_ms: DEFAULT_STT_INTERVAL_MS,
            vocabulary: Vec::new(),
            whisper_backend: WhisperBackend::Auto,
        }
    }
}

impl TryFrom<RawSttPipelineConfig> for SttPipelineConfig {
    type Error = &'static str;

    fn try_from(raw: RawSttPipelineConfig) -> Result<Self, Self::Error> {
        if raw.window_seconds == 0 {
            return Err("stt.window_seconds must be at least 1");
        }
        if raw.interval_ms == 0 {
            return Err("stt.interval_ms must be at least 1");
        }
        let seconds =
            usize::try_from(raw.window_seconds).map_err(|_| "stt.window_seconds is too large")?;
        seconds
            .checked_mul(16_000)
            .ok_or("stt.window_seconds is too large")?;
        Ok(Self {
            window_seconds: raw.window_seconds,
            interval_ms: raw.interval_ms,
            vocabulary: raw.vocabulary,
            whisper_backend: raw.whisper_backend,
        })
    }
}

impl From<&SttPipelineConfig> for RawSttPipelineConfig {
    fn from(config: &SttPipelineConfig) -> Self {
        Self {
            window_seconds: config.window_seconds,
            interval_ms: config.interval_ms,
            vocabulary: config.vocabulary.clone(),
            whisper_backend: config.whisper_backend,
        }
    }
}

impl<'de> Deserialize<'de> for SttPipelineConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawSttPipelineConfig::deserialize(deserializer)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

impl SttPipelineConfig {
    /// Returns the seconds of trailing audio each interim pass transcribes.
    #[must_use]
    pub fn window_seconds(&self) -> u64 {
        self.window_seconds
    }

    /// Returns the milliseconds between interim passes while a take is recording.
    #[must_use]
    pub fn interval_ms(&self) -> u64 {
        self.interval_ms
    }

    /// Returns the domain terms whisper is biased toward.
    #[must_use]
    pub fn vocabulary(&self) -> &[String] {
        &self.vocabulary
    }

    /// Returns the configured whisper runtime build selection
    /// (`whisper_backend`, default `auto`). Consulted only on Windows x86-64
    /// and Linux x86-64.
    #[must_use]
    pub fn whisper_backend(&self) -> WhisperBackend {
        self.whisper_backend
    }
}

/// The whisper runtime build the gateway downloads for speech-to-text on
/// Windows x86-64 and Linux x86-64. Every other platform has at most one
/// build, so this setting is consulted on those two only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum WhisperBackend {
    /// Lets the gateway choose the build for the machine it runs on.
    #[default]
    Auto,
    /// The CPU-only whisper.cpp build.
    Cpu,
    /// The whisper.cpp CUDA build, which needs an NVIDIA GPU.
    Cuda,
}

impl WhisperBackend {
    /// True for the default (`auto`), so serialization can omit it.
    #[must_use]
    pub fn is_auto(&self) -> bool {
        *self == WhisperBackend::Auto
    }
}

/// The speech engine slot a speech-to-text model fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SttRole {
    /// Low-latency model used while a take is still recording.
    Interim,
    /// Higher-accuracy model used to crystallize completed audio.
    Final,
}

/// One speech-to-text model declared as `[[stt_model]]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SttModelConfig {
    /// Catalog name referenced by `[[profile]].models`.
    pub(super) name: String,
    /// Speech engine slot this model fills.
    pub(super) role: SttRole,
    /// HTTPS download URL or operator-controlled local path.
    pub(super) source: String,
    /// Optional lowercase hexadecimal SHA-256 integrity pin.
    #[serde(default)]
    pub(super) sha256: Option<String>,
    /// Estimated VRAM use in gibibytes.
    pub(super) vram_gb: f64,
    /// Optional local dominion that accounts for this model's VRAM.
    #[serde(default)]
    pub(super) dominion: Option<String>,
}

impl SttModelConfig {
    /// Returns the catalog name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the speech engine slot this model fills.
    #[must_use]
    pub const fn role(&self) -> SttRole {
        self.role
    }

    /// Returns the artifact source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the optional lowercase hexadecimal SHA-256 pin.
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// Returns the estimated VRAM use in gibibytes.
    #[must_use]
    pub fn vram_gb(&self) -> f64 {
        self.vram_gb
    }

    /// Returns the optional local dominion binding.
    #[must_use]
    pub fn dominion(&self) -> Option<&str> {
        self.dominion.as_deref()
    }
}

/// One built-in speech-to-text model recommendation.
///
/// Recommended entries are immutable catalog seeds for the Config UI's
/// restore action. Both entries use canonical whisper.cpp URLs and SHA-256
/// digests captured from Hugging Face LFS metadata.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct RecommendedSttModel {
    name: &'static str,
    role: SttRole,
    source: &'static str,
    sha256: &'static str,
    vram_gb: f64,
}

impl RecommendedSttModel {
    /// Returns the recommended catalog name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Returns the recommended speech engine role.
    #[must_use]
    pub const fn role(self) -> SttRole {
        self.role
    }

    /// Returns the canonical whisper.cpp download URL.
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }

    /// Returns the verified lowercase hexadecimal SHA-256 pin.
    #[must_use]
    pub const fn sha256(self) -> &'static str {
        self.sha256
    }

    /// Returns the conservative VRAM estimate in gibibytes.
    #[must_use]
    pub const fn vram_gb(self) -> f64 {
        self.vram_gb
    }
}

/// Digest-pinned CPU-friendly whisper.cpp pair restored by the Config UI.
///
/// `base.en` supplies responsive interim results and `small.en` supplies the
/// more accurate final pass. The estimates include headroom above the model
/// files' resident-memory footprints.
pub const RECOMMENDED_STT_MODELS: [RecommendedSttModel; 2] = [
    RecommendedSttModel {
        name: "whisper-base-en",
        role: SttRole::Interim,
        source: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
        vram_gb: 1.0,
    },
    RecommendedSttModel {
        name: "whisper-small-en",
        role: SttRole::Final,
        source: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
        sha256: "c6138d6d58ecc8322097e0f987c32f1be8bb0a18532a3f88f734d1bbf9c41e5d",
        vram_gb: 2.0,
    },
];

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Read as _;

    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn public_deserialization_rejects_invalid_pipeline_bounds() {
        for json in [
            r#"{"window_seconds":0}"#,
            r#"{"interval_ms":0}"#,
            r#"{"window_seconds":18446744073709551615}"#,
        ] {
            assert!(
                serde_json::from_str::<SttPipelineConfig>(json).is_err(),
                "invalid public STT pipeline input must fail: {json}"
            );
        }

        assert!(
            toml::from_str::<SttPipelineConfig>("window_seconds = 0").is_err(),
            "format-specific TOML deserialization must use the same validation boundary"
        );
    }

    #[test]
    fn public_deserialization_applies_valid_defaults() {
        let config: SttPipelineConfig =
            serde_json::from_str("{}").expect("default STT pipeline is valid");
        assert_eq!(config.window_seconds(), DEFAULT_STT_WINDOW_SECONDS);
        assert_eq!(config.interval_ms(), DEFAULT_STT_INTERVAL_MS);
        assert!(config.vocabulary().is_empty());
        assert_eq!(config.whisper_backend(), WhisperBackend::Auto);
        assert_eq!(
            config,
            SttPipelineConfig::default(),
            "an absent [stt] section falls back to the same defaults"
        );

        let json = serde_json::to_value(&config).expect("STT pipeline serializes");
        assert!(
            json.get("whisper_backend").is_none(),
            "the default `auto` backend is omitted: {json}"
        );
    }

    #[test]
    fn public_serialization_writes_a_non_default_whisper_backend() {
        let config: SttPipelineConfig = serde_json::from_str(r#"{"whisper_backend":"cuda"}"#)
            .expect("a CUDA whisper backend is valid");
        assert_eq!(config.whisper_backend(), WhisperBackend::Cuda);

        let json = serde_json::to_value(&config).expect("STT pipeline serializes");
        assert_eq!(json["whisper_backend"], "cuda", "{json}");
    }

    #[test]
    fn recommended_pair_is_complete_and_digest_pinned() {
        assert_eq!(RECOMMENDED_STT_MODELS.len(), 2);
        assert_eq!(RECOMMENDED_STT_MODELS[0].role(), SttRole::Interim);
        assert_eq!(RECOMMENDED_STT_MODELS[1].role(), SttRole::Final);
        for model in RECOMMENDED_STT_MODELS {
            assert!(
                model
                    .source()
                    .starts_with("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-")
            );
            assert_eq!(model.sha256().len(), 64);
            assert!(
                model
                    .sha256()
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            );
            assert!(model.vram_gb().is_normal());
        }
    }

    #[test]
    fn recommended_pair_is_whisper_base_en_then_small_en() {
        assert_eq!(RECOMMENDED_STT_MODELS[0].name(), "whisper-base-en");
        assert_eq!(RECOMMENDED_STT_MODELS[1].name(), "whisper-small-en");
    }

    #[test]
    fn recommended_vram_estimates_are_positive() {
        for model in RECOMMENDED_STT_MODELS {
            assert!(model.vram_gb() > 0.0, "{} estimate", model.name());
        }
    }

    #[test]
    #[ignore = "downloads large live artifacts to detect upstream URL or digest drift"]
    fn recommended_pair_live_urls_match_pins() {
        for model in RECOMMENDED_STT_MODELS {
            let mut response = reqwest::blocking::get(model.source())
                .expect("recommended STT URL responds")
                .error_for_status()
                .expect("recommended STT URL returns success");
            let mut hasher = Sha256::new();
            let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
            loop {
                let count = response
                    .read(&mut buffer)
                    .expect("recommended STT artifact downloads");
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
            }
            let digest = hasher.finalize();
            let mut actual = String::with_capacity(64);
            for byte in digest {
                write!(&mut actual, "{byte:02x}").expect("writing to String is infallible");
            }
            assert_eq!(actual, model.sha256(), "digest drift for {}", model.name());
        }
    }
}
