//! Validated resolution policy: the thresholds that separate the outcomes.
//!
//! A [`Config`] is always valid. Its fields are private and every successful
//! construction proves the invariant: thresholds are finite and lie in the
//! supported `0.0..=1.0` policy domain, and `top_k` is nonzero. There is no
//! `validate` method because no public operation can produce an invalid value.
//!
//! A caller starts from [`Config::default`] and adjusts one threshold at a time
//! with a checked consuming setter, each of which returns [`ConfigError`] when
//! its value leaves the supported domain.
//!
//! There is no model selection. One model is compiled in, which is an
//! implementation fact rather than a caller choice.

use std::num::NonZeroUsize;

/// The default shortlist bound, three, as a nonzero count.
const DEFAULT_TOP_K: NonZeroUsize = match NonZeroUsize::new(3) {
    Some(value) => value,
    None => unreachable!(),
};

/// The model and thresholds that govern a resolution.
///
/// Every field has a justified default; a caller who has not measured their own
/// catalog should change none of them. Construction and every checked setter
/// establish the invariant, so a stored configuration is always valid.
///
/// # Examples
///
/// ```
/// use promptforge_tool_picker::Config;
///
/// let config = Config::default()
///     .with_similarity_floor(0.85)?
///     .with_top_k(5)?;
/// assert_eq!(config.top_k().get(), 5);
/// # Ok::<(), promptforge_tool_picker::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Config {
    /// The cosine similarity a candidate must reach to be considered at all.
    similarity_floor: f32,
    /// The gap between the top candidate and the runner-up required to bind.
    margin: f32,
    /// The cosine similarity at or above which two tools are treated as twins.
    duplicate_threshold: f32,
    /// Minimum score at which a lone candidate still binds.
    solo_floor: f32,
    /// How many candidates an ambiguous or duplicate outcome reports.
    top_k: NonZeroUsize,
}

impl Default for Config {
    /// The defaults, three of them measured, one provisional, and one derived.
    ///
    /// `similarity_floor` is `0.825`, the measured threshold that holds the
    /// false-bind rate at or under 5% in the hard regime of the study this
    /// engine is drawn from. `solo_floor` is `0.5`, well below the floor, so a
    /// lone reasonable paraphrase still binds. `duplicate_threshold` is `0.98`,
    /// at or above which two tools' own embeddings are near-verbatim copies.
    /// `top_k` is `3`, the shortlist length the design surfaces. `margin` is
    /// `0.05`, a provisional top-1-versus-top-2 gap to tune against a real
    /// catalog.
    fn default() -> Self {
        Self {
            similarity_floor: 0.825,
            margin: 0.05,
            duplicate_threshold: 0.98,
            solo_floor: 0.5,
            top_k: DEFAULT_TOP_K,
        }
    }
}

impl Config {
    /// Returns the similarity floor a candidate must reach to be considered.
    #[must_use]
    pub fn similarity_floor(&self) -> f32 {
        self.similarity_floor
    }

    /// Returns the margin the leader must clear the runner-up by to bind.
    #[must_use]
    pub fn margin(&self) -> f32 {
        self.margin
    }

    /// Returns the similarity at or above which two tools are twins.
    #[must_use]
    pub fn duplicate_threshold(&self) -> f32 {
        self.duplicate_threshold
    }

    /// Returns the score at which a lone candidate still binds.
    #[must_use]
    pub fn solo_floor(&self) -> f32 {
        self.solo_floor
    }

    /// Returns the shortlist bound a duplicate or ambiguous outcome reports.
    #[must_use]
    pub fn top_k(&self) -> NonZeroUsize {
        self.top_k
    }

    /// Returns this configuration with a new similarity floor.
    ///
    /// # Errors
    /// Returns [`ConfigError`] naming [`ConfigField::SimilarityFloor`] when
    /// `value` is not finite or falls outside the supported `0.0..=1.0` domain.
    #[must_use = "the checked configuration is returned, not applied in place"]
    pub fn with_similarity_floor(self, value: f32) -> Result<Self, ConfigError> {
        check_threshold(value, ConfigField::SimilarityFloor)?;
        Ok(Self {
            similarity_floor: value,
            ..self
        })
    }

    /// Returns this configuration with a new margin.
    ///
    /// # Errors
    /// Returns [`ConfigError`] naming [`ConfigField::Margin`] when `value` is
    /// not finite or falls outside the supported `0.0..=1.0` domain.
    #[must_use = "the checked configuration is returned, not applied in place"]
    pub fn with_margin(self, value: f32) -> Result<Self, ConfigError> {
        check_threshold(value, ConfigField::Margin)?;
        Ok(Self {
            margin: value,
            ..self
        })
    }

    /// Returns this configuration with a new duplicate threshold.
    ///
    /// # Errors
    /// Returns [`ConfigError`] naming [`ConfigField::DuplicateThreshold`] when
    /// `value` is not finite or falls outside the supported `0.0..=1.0` domain.
    #[must_use = "the checked configuration is returned, not applied in place"]
    pub fn with_duplicate_threshold(self, value: f32) -> Result<Self, ConfigError> {
        check_threshold(value, ConfigField::DuplicateThreshold)?;
        Ok(Self {
            duplicate_threshold: value,
            ..self
        })
    }

    /// Returns this configuration with a new solo floor.
    ///
    /// # Errors
    /// Returns [`ConfigError`] naming [`ConfigField::SoloFloor`] when `value` is
    /// not finite or falls outside the supported `0.0..=1.0` domain.
    #[must_use = "the checked configuration is returned, not applied in place"]
    pub fn with_solo_floor(self, value: f32) -> Result<Self, ConfigError> {
        check_threshold(value, ConfigField::SoloFloor)?;
        Ok(Self {
            solo_floor: value,
            ..self
        })
    }

    /// Returns this configuration with a new shortlist bound.
    ///
    /// # Errors
    /// Returns [`ConfigError`] naming [`ConfigField::TopK`] when `value` is
    /// zero.
    #[must_use = "the checked configuration is returned, not applied in place"]
    pub fn with_top_k(self, value: usize) -> Result<Self, ConfigError> {
        let top_k = NonZeroUsize::new(value).ok_or(ConfigError {
            field: ConfigField::TopK,
        })?;
        Ok(Self { top_k, ..self })
    }
}

/// Rejects a threshold outside the supported calibrated policy domain.
fn check_threshold(value: f32, field: ConfigField) -> Result<(), ConfigError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(ConfigError { field })
    }
}

/// One configuration field, named by a rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigField {
    /// The [`Config::similarity_floor`](crate::Config::similarity_floor) field.
    SimilarityFloor,
    /// The [`Config::margin`](crate::Config::margin) field.
    Margin,
    /// The [`Config::duplicate_threshold`](crate::Config::duplicate_threshold) field.
    DuplicateThreshold,
    /// The [`Config::solo_floor`](crate::Config::solo_floor) field.
    SoloFloor,
    /// The [`Config::top_k`](crate::Config::top_k) field.
    TopK,
}

/// A configuration value fell outside the supported policy domain.
///
/// Thresholds must be finite and in `0.0..=1.0`; `top_k` must be nonzero. This
/// is the supported calibrated policy domain, not the mathematical cosine range.
#[derive(Debug, thiserror::Error)]
#[error("configuration field out of the supported domain: {field:?}")]
pub struct ConfigError {
    /// The offending field.
    field: ConfigField,
}

impl ConfigError {
    /// Returns the configuration field this error rejected.
    #[must_use]
    pub fn field(&self) -> ConfigField {
        self.field
    }
}

#[cfg(feature = "serde")]
mod wire {
    use super::{Config, DEFAULT_TOP_K};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// The wire form of a configuration: every field optional and defaulted.
    ///
    /// Deserialization goes through this raw form and a checked conversion, so a
    /// successfully deserialized [`Config`] cannot hold an invalid value.
    #[derive(Serialize, Deserialize)]
    #[serde(default)]
    struct RawConfig {
        similarity_floor: f32,
        margin: f32,
        duplicate_threshold: f32,
        solo_floor: f32,
        top_k: usize,
    }

    impl Default for RawConfig {
        fn default() -> Self {
            let config = Config::default();
            Self {
                similarity_floor: config.similarity_floor,
                margin: config.margin,
                duplicate_threshold: config.duplicate_threshold,
                solo_floor: config.solo_floor,
                top_k: DEFAULT_TOP_K.get(),
            }
        }
    }

    impl Serialize for Config {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            RawConfig {
                similarity_floor: self.similarity_floor,
                margin: self.margin,
                duplicate_threshold: self.duplicate_threshold,
                solo_floor: self.solo_floor,
                top_k: self.top_k.get(),
            }
            .serialize(serializer)
        }
    }

    impl<'de> Deserialize<'de> for Config {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let raw = RawConfig::deserialize(deserializer)?;
            Config::default()
                .with_similarity_floor(raw.similarity_floor)
                .and_then(|config| config.with_margin(raw.margin))
                .and_then(|config| config.with_duplicate_threshold(raw.duplicate_threshold))
                .and_then(|config| config.with_solo_floor(raw.solo_floor))
                .and_then(|config| config.with_top_k(raw.top_k))
                .map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigField};

    fn assert_exact(actual: f32, expected: f32) {
        assert_eq!(actual.to_bits(), expected.to_bits());
    }

    #[test]
    fn defaults_are_the_documented_values() {
        let config = Config::default();
        assert_exact(config.similarity_floor(), 0.825);
        assert_exact(config.solo_floor(), 0.5);
        assert_exact(config.margin(), 0.05);
        assert_exact(config.duplicate_threshold(), 0.98);
        assert_eq!(config.top_k().get(), 3);
    }

    #[test]
    fn the_bounds_of_the_supported_domain_are_accepted() {
        let config = Config::default()
            .with_similarity_floor(0.0)
            .and_then(|config| config.with_margin(0.0))
            .and_then(|config| config.with_duplicate_threshold(1.0))
            .and_then(|config| config.with_top_k(1))
            .expect("boundary values are in the supported domain");
        assert_exact(config.similarity_floor(), 0.0);
        assert_exact(config.duplicate_threshold(), 1.0);
    }

    #[test]
    fn a_threshold_outside_the_domain_is_rejected_and_names_its_field() {
        for bad in [-0.001, 1.001, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let error = Config::default()
                .with_similarity_floor(bad)
                .expect_err("out-of-domain floor is rejected");
            assert_eq!(error.field(), ConfigField::SimilarityFloor);
            let error = Config::default()
                .with_solo_floor(bad)
                .expect_err("out-of-domain solo floor is rejected");
            assert_eq!(error.field(), ConfigField::SoloFloor);
        }
    }

    #[test]
    fn a_zero_top_k_is_rejected() {
        let error = Config::default()
            .with_top_k(0)
            .expect_err("a zero shortlist bound is rejected");
        assert_eq!(error.field(), ConfigField::TopK);
    }

    #[test]
    fn a_failed_setter_leaves_a_cloned_original_available() {
        let original = Config::default();
        let error = original.clone().with_margin(2.0);
        assert!(error.is_err());
        assert_exact(original.margin(), 0.05);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn config_round_trips_through_json_and_fills_absent_fields() {
        let config = Config::default()
            .with_similarity_floor(0.863)
            .and_then(|config| config.with_top_k(5))
            .expect("valid overrides");
        let text = serde_json::to_string(&config).expect("serialize");
        let parsed: Config = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(parsed, config);

        let partial: Config = serde_json::from_str(r#"{"top_k": 7}"#).expect("partial deserialize");
        assert_eq!(partial.top_k().get(), 7);
        assert_exact(partial.similarity_floor(), 0.825);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn checked_deserialization_rejects_an_invalid_wire_value() {
        let error = serde_json::from_str::<Config>(r#"{"similarity_floor": 2.0}"#);
        assert!(
            error.is_err(),
            "an out-of-domain wire value must be rejected"
        );
        let error = serde_json::from_str::<Config>(r#"{"top_k": 0}"#);
        assert!(error.is_err(), "a zero top_k must be rejected");
    }
}
