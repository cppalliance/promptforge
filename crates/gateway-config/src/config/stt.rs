//! Speech-to-text catalog entries and the digest-pinned recommended pair.

use serde::{Deserialize, Serialize};

/// The engine slot a speech-to-text model fills.
///
/// # Examples
/// ```
/// use gateway_config::SttRole;
///
/// assert_ne!(SttRole::Interim, SttRole::Final);
/// ```
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
///
/// # Examples
/// ```
/// use gateway_config::Config;
///
/// let config = Config::from_toml_str(
///     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
///      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
/// )?;
/// assert_eq!(config.catalog_stt_models()[0].name(), "speech");
/// # Ok::<(), gateway_config::ConfigError>(())
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SttModelConfig {
    /// Catalog name referenced by `[[profile]].models`.
    pub(super) name: String,
    /// Engine slot this model fills.
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
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    /// #      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
    /// # )?;
    /// assert_eq!(config.catalog_stt_models()[0].name(), "speech");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the engine slot this model fills.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::{Config, SttRole};
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    /// #      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
    /// # )?;
    /// assert_eq!(config.catalog_stt_models()[0].role(), SttRole::Interim);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub const fn role(&self) -> SttRole {
        self.role
    }

    /// Returns the artifact source.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    /// #      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
    /// # )?;
    /// assert_eq!(config.catalog_stt_models()[0].source(), "/speech.bin");
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the optional lowercase hexadecimal SHA-256 pin.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    /// #      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
    /// # )?;
    /// assert_eq!(config.catalog_stt_models()[0].sha256(), None);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    /// Returns the estimated VRAM use in gibibytes.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    /// #      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
    /// # )?;
    /// assert_eq!(config.catalog_stt_models()[0].vram_gb(), 1.0);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
    #[must_use]
    pub fn vram_gb(&self) -> f64 {
        self.vram_gb
    }

    /// Returns the optional local dominion binding.
    ///
    /// # Examples
    /// ```
    /// # use gateway_config::Config;
    /// # let config = Config::from_toml_str(
    /// #     "config-version = 2\n[server]\nbind = \"127.0.0.1:8080\"\napi_key = \"secret\"\n\
    /// #      [[stt_model]]\nname = \"speech\"\nrole = \"interim\"\nsource = \"/speech.bin\"\nvram_gb = 1.0\n",
    /// # )?;
    /// assert_eq!(config.catalog_stt_models()[0].dominion(), None);
    /// # Ok::<(), gateway_config::ConfigError>(())
    /// ```
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
///
/// # Examples
/// ```
/// use gateway_config::RECOMMENDED_STT_MODELS;
///
/// let model = RECOMMENDED_STT_MODELS[0];
/// assert!(model.source().starts_with("https://"));
/// ```
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
    ///
    /// # Examples
    /// ```
    /// use gateway_config::RECOMMENDED_STT_MODELS;
    ///
    /// assert_eq!(RECOMMENDED_STT_MODELS[0].name(), "whisper-base-en");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// Returns the recommended engine role.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::{RECOMMENDED_STT_MODELS, SttRole};
    ///
    /// assert_eq!(RECOMMENDED_STT_MODELS[0].role(), SttRole::Interim);
    /// ```
    #[must_use]
    pub const fn role(self) -> SttRole {
        self.role
    }

    /// Returns the canonical whisper.cpp download URL.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::RECOMMENDED_STT_MODELS;
    ///
    /// assert!(RECOMMENDED_STT_MODELS[0].source().contains("whisper.cpp"));
    /// ```
    #[must_use]
    pub const fn source(self) -> &'static str {
        self.source
    }

    /// Returns the verified lowercase hexadecimal SHA-256 pin.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::RECOMMENDED_STT_MODELS;
    ///
    /// assert_eq!(RECOMMENDED_STT_MODELS[0].sha256().len(), 64);
    /// ```
    #[must_use]
    pub const fn sha256(self) -> &'static str {
        self.sha256
    }

    /// Returns the conservative VRAM estimate in gibibytes.
    ///
    /// # Examples
    /// ```
    /// use gateway_config::RECOMMENDED_STT_MODELS;
    ///
    /// assert!(RECOMMENDED_STT_MODELS[0].vram_gb() > 0.0);
    /// ```
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
///
/// # Examples
/// ```
/// use gateway_config::{RECOMMENDED_STT_MODELS, SttRole};
///
/// assert_eq!(RECOMMENDED_STT_MODELS.len(), 2);
/// assert_eq!(RECOMMENDED_STT_MODELS[1].role(), SttRole::Final);
/// ```
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
