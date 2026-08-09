//! Validated decision policy for tool resolution.

use std::num::NonZeroUsize;

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{ConfigError, ConfigErrorRepr};

/// The validated thresholds that govern resolution.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize))]
#[non_exhaustive]
pub struct Config {
    similarity_floor: f32,
    margin: f32,
    duplicate_threshold: f32,
    solo_floor: f32,
    top_k: NonZeroUsize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            similarity_floor: 0.825,
            solo_floor: 0.5,
            margin: 0.05,
            duplicate_threshold: 0.98,
            top_k: NonZeroUsize::new(3).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

impl Config {
    /// Returns the calibrated similarity floor.
    #[must_use]
    pub fn similarity_floor(&self) -> f32 {
        self.similarity_floor
    }
    /// Returns the binding margin.
    #[must_use]
    pub fn margin(&self) -> f32 {
        self.margin
    }
    /// Returns the near-duplicate threshold.
    #[must_use]
    pub fn duplicate_threshold(&self) -> f32 {
        self.duplicate_threshold
    }
    /// Returns the solo-candidate floor.
    #[must_use]
    pub fn solo_floor(&self) -> f32 {
        self.solo_floor
    }
    /// Returns the nonzero result-group limit.
    #[must_use]
    pub fn top_k(&self) -> NonZeroUsize {
        self.top_k
    }

    /// Returns this configuration with a checked similarity floor.
    ///
    /// # Errors
    /// Returns an error when `value` is non-finite or outside `0.0..=1.0`.
    pub fn with_similarity_floor(mut self, value: f32) -> Result<Self, ConfigError> {
        self.similarity_floor = threshold(ConfigField::SimilarityFloor, value)?;
        Ok(self)
    }
    /// Returns this configuration with a checked margin.
    ///
    /// # Errors
    /// Returns an error when `value` is non-finite or outside `0.0..=1.0`.
    pub fn with_margin(mut self, value: f32) -> Result<Self, ConfigError> {
        self.margin = threshold(ConfigField::Margin, value)?;
        Ok(self)
    }
    /// Returns this configuration with a checked duplicate threshold.
    ///
    /// # Errors
    /// Returns an error when `value` is non-finite or outside `0.0..=1.0`.
    pub fn with_duplicate_threshold(mut self, value: f32) -> Result<Self, ConfigError> {
        self.duplicate_threshold = threshold(ConfigField::DuplicateThreshold, value)?;
        Ok(self)
    }
    /// Returns this configuration with a checked solo floor.
    ///
    /// # Errors
    /// Returns an error when `value` is non-finite or outside `0.0..=1.0`.
    pub fn with_solo_floor(mut self, value: f32) -> Result<Self, ConfigError> {
        self.solo_floor = threshold(ConfigField::SoloFloor, value)?;
        Ok(self)
    }
    /// Returns this configuration with a checked result-group limit.
    ///
    /// # Errors
    /// Returns an error when `value` is zero.
    pub fn with_top_k(mut self, value: usize) -> Result<Self, ConfigError> {
        self.top_k = NonZeroUsize::new(value).ok_or(ConfigError(ConfigErrorRepr::ZeroTopK))?;
        Ok(self)
    }
}

fn threshold(field: ConfigField, value: f32) -> Result<f32, ConfigError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(ConfigError(ConfigErrorRepr::Threshold { field, value }))
    }
}

/// Identifies a configuration field rejected by a checked setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigField {
    /// The strict relevance floor.
    SimilarityFloor,
    /// The leader-to-runner-up margin.
    Margin,
    /// The tool-pair duplicate threshold.
    DuplicateThreshold,
    /// The solo-candidate relevance floor.
    SoloFloor,
    /// The result-group limit.
    TopK,
}

#[cfg(feature = "serde")]
#[derive(Deserialize)]
#[serde(default)]
struct RawConfig {
    similarity_floor: f32,
    margin: f32,
    duplicate_threshold: f32,
    solo_floor: f32,
    top_k: usize,
}

#[cfg(feature = "serde")]
impl Default for RawConfig {
    fn default() -> Self {
        let value = Config::default();
        Self {
            similarity_floor: value.similarity_floor(),
            margin: value.margin(),
            duplicate_threshold: value.duplicate_threshold(),
            solo_floor: value.solo_floor(),
            top_k: value.top_k().get(),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for Config {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawConfig::deserialize(deserializer)?;
        Config::default()
            .with_similarity_floor(raw.similarity_floor)
            .and_then(|c| c.with_margin(raw.margin))
            .and_then(|c| c.with_duplicate_threshold(raw.duplicate_threshold))
            .and_then(|c| c.with_solo_floor(raw.solo_floor))
            .and_then(|c| c.with_top_k(raw.top_k))
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(all(test, not(test)))]
mod tests {
    use super::{Config, ModelId};
    use crate::error::Error;

    /// Asserts a float is exactly the documented literal, bit for bit.
    ///
    /// Compared as bits rather than with `==`: a default is a literal the
    /// documentation quotes, so the honest assertion is exact equality, and
    /// comparing the bit patterns says so without an epsilon that would let a
    /// changed default slip through.
    fn assert_exact(actual: f32, expected: f32) {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn defaults_are_the_documented_values() {
        let config = Config::default();
        assert_eq!(config.model_id, ModelId::BgeSmallEnV15);
        assert_exact(config.similarity_floor, 0.825);
        assert_exact(config.solo_floor, 0.5);
        assert_exact(config.margin, 0.05);
        assert_exact(config.duplicate_threshold, 0.98);
        assert_eq!(config.top_k, 3);
    }

    #[test]
    fn defaults_validate() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn the_bounds_of_the_cosine_range_are_accepted() {
        let config = Config {
            similarity_floor: 0.0,
            margin: 0.0,
            duplicate_threshold: 1.0,
            top_k: 1,
            ..Config::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn a_threshold_outside_the_cosine_range_is_rejected() {
        for bad in [-0.001, 1.001, f32::NAN, f32::INFINITY] {
            for (field, config) in [
                (
                    "similarity_floor",
                    Config {
                        similarity_floor: bad,
                        ..Config::default()
                    },
                ),
                (
                    "solo_floor",
                    Config {
                        solo_floor: bad,
                        ..Config::default()
                    },
                ),
                (
                    "margin",
                    Config {
                        margin: bad,
                        ..Config::default()
                    },
                ),
                (
                    "duplicate_threshold",
                    Config {
                        duplicate_threshold: bad,
                        ..Config::default()
                    },
                ),
            ] {
                match config.validate() {
                    Err(Error::ThresholdOutOfRange { field: named, .. }) => {
                        assert_eq!(named, field, "the error must name the offending field");
                    }
                    other => panic!("{field} = {bad} should be out of range, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_zero_length_shortlist_is_rejected() {
        let config = Config {
            top_k: 0,
            ..Config::default()
        };
        assert!(matches!(config.validate(), Err(Error::EmptyShortlist)));
    }

    #[test]
    fn the_duplicate_threshold_is_not_ordered_against_the_floor() {
        // The two measure different things - one tool against another, and a
        // need against a tool - so neither bounds the other and every
        // arrangement of them is a configuration the engine can carry out.
        for duplicate_threshold in [0.85, 0.9, 0.95] {
            let config = Config {
                similarity_floor: 0.9,
                duplicate_threshold,
                ..Config::default()
            };
            assert!(
                config.validate().is_ok(),
                "duplicate_threshold {duplicate_threshold} against a floor of 0.9"
            );
        }
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = Config {
            similarity_floor: 0.863,
            top_k: 5,
            ..Config::default()
        };
        let text = serde_json::to_string(&config).unwrap();
        let parsed: Config = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn absent_json_fields_take_their_defaults() {
        let parsed: Config = serde_json::from_str(r#"{"top_k": 7}"#).unwrap();
        assert_eq!(parsed.top_k, 7);
        assert_eq!(
            Config {
                top_k: 7,
                ..Config::default()
            },
            parsed
        );

        let parsed: Config = serde_json::from_str(r#"{"solo_floor": 0.6}"#).unwrap();
        assert_exact(parsed.solo_floor, 0.6);
        assert_eq!(
            Config {
                solo_floor: 0.6,
                ..Config::default()
            },
            parsed
        );
    }

    #[test]
    fn the_model_id_serializes_as_its_huggingface_name() {
        let text = serde_json::to_string(&ModelId::BgeSmallEnV15).unwrap();
        assert_eq!(text, r#""bge-small-en-v1.5""#);
        assert_eq!(ModelId::BgeSmallEnV15.to_string(), "bge-small-en-v1.5");
        assert_eq!(ModelId::default(), ModelId::BgeSmallEnV15);
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn every_threshold_boundary_and_non_finite_value_is_checked() {
        for value in [0.0, 1.0] {
            assert!(Config::default().with_similarity_floor(value).is_ok());
            assert!(Config::default().with_margin(value).is_ok());
            assert!(Config::default().with_duplicate_threshold(value).is_ok());
            assert!(Config::default().with_solo_floor(value).is_ok());
        }
        for value in [
            -f32::EPSILON,
            1.0 + f32::EPSILON,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ] {
            assert!(Config::default().with_similarity_floor(value).is_err());
            assert!(Config::default().with_margin(value).is_err());
            assert!(Config::default().with_duplicate_threshold(value).is_err());
            assert!(Config::default().with_solo_floor(value).is_err());
        }
        assert_eq!(
            Config::default().with_top_k(0).unwrap_err().field(),
            ConfigField::TopK
        );
        assert_eq!(Config::default().with_top_k(1).unwrap().top_k().get(), 1);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserialization_defaults_missing_fields_and_rejects_invalid_states() {
        let parsed: Config = serde_json::from_str(r#"{"top_k":7}"#).unwrap();
        assert_eq!(parsed.top_k().get(), 7);
        assert!(serde_json::from_str::<Config>(r#"{"top_k":0}"#).is_err());
        assert!(serde_json::from_str::<Config>(r#"{"margin":"bad"}"#).is_err());
        assert!(serde_json::from_str::<Config>(r#"{"solo_floor":-1.0}"#).is_err());
    }
}
