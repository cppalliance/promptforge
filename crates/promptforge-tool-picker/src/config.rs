//! Configuration for resolution: the model and the decision thresholds.
//!
//! The configuration selects the embedding model and the thresholds that
//! separate the four outcomes from one another.
//!
//! There is no path to model weights here, and there never will be: the weights
//! are compiled into the binary, so a path would name a file the engine does
//! not read.
//!
//! Three of the four thresholds have defaults measured on bge-small-en-v1.5;
//! each is justified where it is declared. Those measurements are transcribed
//! into this file rather than looked up, because the engine is self-contained
//! and reads nothing outside itself.
//!
//! A [`Config`] is plain data with public fields, so a caller adjusts one
//! threshold with struct-update syntax over [`Config::default`] and leaves the
//! rest at their justified values. Because the fields are public, nothing
//! enforces their consistency at construction; [`Config::validate`] is the
//! single place that checks them, and building an engine calls it. Validation
//! lives in a method rather than a constructor for exactly this reason: a
//! constructor that took five arguments would be checked once and then bypassed
//! by the next field assignment, while a method can be re-run on a value the
//! caller has since edited or deserialized.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The embedding model the engine uses.
///
/// Only one model can be selected, because only one model's weights are
/// compiled into the binary; offering a choice the binary cannot satisfy would
/// turn a build-time fact into a runtime failure.
///
/// The type is an `#[non_exhaustive]` enum rather than a string or a unit
/// struct so that embedding a second model later adds a variant instead of
/// reshaping the configuration. A caller matching on it must already carry a
/// wildcard arm, so that addition is not a breaking change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ModelId {
    /// `BAAI/bge-small-en-v1.5`: a 384-dimension English sentence encoder.
    ///
    /// Its pooling is CLS-token pooling followed by L2 normalization, which is
    /// the model's own convention and not interchangeable with mean pooling.
    #[default]
    #[serde(rename = "bge-small-en-v1.5")]
    BgeSmallEnV15,
}

impl ModelId {
    /// The model's name, spelled as it is on HuggingFace and in serialized form.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ModelId::BgeSmallEnV15 => "bge-small-en-v1.5",
        }
    }
}

impl std::fmt::Display for ModelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The model and thresholds that govern a resolution.
///
/// Every field has a default that [`Config::default`] documents and justifies;
/// a caller who has not measured their own catalog should change none of them.
/// A configuration is only trustworthy once [`Config::validate`] has accepted
/// it - see the module documentation for why that check is a method.
///
/// In JSON every field is optional and an absent one takes its default, so a
/// caller can override a single threshold without restating the others.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The embedding model to use.
    pub model_id: ModelId,
    /// The cosine similarity a candidate must reach to be considered at all.
    ///
    /// Below it, the engine abstains rather than guessing.
    pub similarity_floor: f32,
    /// The gap between the top candidate and the runner-up required to bind.
    ///
    /// Two candidates closer together than this are a tie, not a winner.
    pub margin: f32,
    /// The cosine similarity at or above which two tools are treated as twins.
    ///
    /// Measured between the two tools' *own* embeddings, not between their
    /// scores for any need: twin-ness is a property of the pair and the same
    /// whatever is being asked of them. It is unrelated to
    /// `similarity_floor`, which measures a need against a tool.
    pub duplicate_threshold: f32,
    /// How many candidates a shortlist carries.
    pub top_k: usize,
}

impl Default for Config {
    /// The defaults, three of them measured and one of them provisional.
    ///
    /// `model_id` is [`ModelId::BgeSmallEnV15`], the only model there is.
    ///
    /// `similarity_floor` is `0.825`: the measured threshold that holds the
    /// false-bind rate at or under 5% in the hard regime of the study this
    /// engine is drawn from. A stricter 1% budget corresponds to `0.863` and a
    /// looser 10% budget to `0.805`, so the floor is the dial that trades
    /// coverage against wrong bindings - raise it and the engine binds less
    /// often but is wrong less often when it does.
    ///
    /// `duplicate_threshold` is `0.98`: at or above that cosine similarity
    /// between two tools' own embeddings, the pair was found to be twins
    /// rather than neighbours. Roughly 11% of a broad catalog has such a twin,
    /// which at that similarity means a near-verbatim republication of the
    /// same tool, overwhelmingly across servers.
    ///
    /// `top_k` is `3`: on the realistic band the correct tool was in the top 3
    /// about 90% of the time while top-1 was around 76%. That gap is why the
    /// design surfaces a shortlist instead of forcing a single guess.
    ///
    /// `margin` is `0.05`, and unlike the other three it is **not** a measured
    /// value. It is a starting point for the top-1-versus-top-2 gap required to
    /// bind confidently, to be tuned against a real catalog in use.
    fn default() -> Self {
        Self {
            model_id: ModelId::BgeSmallEnV15,
            similarity_floor: 0.825,
            margin: 0.05,
            duplicate_threshold: 0.98,
            top_k: 3,
        }
    }
}

impl Config {
    /// Checks that the configuration describes a decision the engine can make.
    ///
    /// The check is a method rather than a constructor because the fields are
    /// public: a value can be edited or deserialized after it was last checked,
    /// so the check has to be re-runnable. Building an engine validates its
    /// configuration, so a caller who only ever passes a [`Config`] to the
    /// engine never has to call this directly.
    ///
    /// Nonsense is rejected here rather than absorbed: a threshold outside the
    /// cosine range or a shortlist of length zero would not fail loudly during
    /// resolution, it would quietly turn every need into the same answer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ThresholdOutOfRange`] if `similarity_floor`, `margin`,
    /// or `duplicate_threshold` is NaN or falls outside `0.0..=1.0`, and
    /// [`Error::EmptyShortlist`] if `top_k` is zero.
    ///
    /// No relation between the thresholds is checked, because none holds.
    /// `duplicate_threshold` measures one tool against another and
    /// `similarity_floor` measures a need against a tool, so neither bounds
    /// the other and a `duplicate_threshold` below the floor is a perfectly
    /// coherent configuration.
    pub fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("similarity_floor", self.similarity_floor),
            ("margin", self.margin),
            ("duplicate_threshold", self.duplicate_threshold),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(Error::ThresholdOutOfRange { field, value });
            }
        }

        if self.top_k == 0 {
            return Err(Error::EmptyShortlist);
        }

        Ok(())
    }
}

#[cfg(test)]
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
    }

    #[test]
    fn the_model_id_serializes_as_its_huggingface_name() {
        let text = serde_json::to_string(&ModelId::BgeSmallEnV15).unwrap();
        assert_eq!(text, r#""bge-small-en-v1.5""#);
        assert_eq!(ModelId::BgeSmallEnV15.to_string(), "bge-small-en-v1.5");
        assert_eq!(ModelId::default(), ModelId::BgeSmallEnV15);
    }
}
