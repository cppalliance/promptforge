//! The preflight report: [`Requirements`].

use promptforge_api_types::capabilities::CapabilityId;

/// The preflight report: what the caller must still satisfy before the
/// prompt can run.
///
/// [`Environment::prepare`](super::Environment::prepare) returns one
/// alongside the enriched context. The report lists only what needs human
/// attention: a skipped optional capability is a log line at prepare, not
/// a report field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Requirements {
    /// The model requirements the filled bindings do not satisfy: the
    /// role, which check, and required versus actual (a `min_context` of
    /// 200000 against a 32k model; `thinking` against a Never model).
    /// Populated by the model fill; capability activation adds none.
    pub unmet_requirements: Vec<UnmetRequirement>,
    /// The required capabilities the run cannot have: reported by
    /// activation when absent from the host's registry or failed to
    /// activate, and by prepare when an exact tool slot names a
    /// capability that contributed nothing to the catalog. The run fails
    /// until every one is satisfied.
    pub missing_required: Vec<CapabilityId>,
    /// The declared co-activation conflicts: pairs of present
    /// capabilities that cannot activate in one run (bashkit vs
    /// terminal - two filesystem realities, and a context gets one or
    /// the other, never both). Neither member of a conflicting pair
    /// activates; the run fails until the prompt declares one or the
    /// other. Reported by activation, never by prepare.
    pub conflicts: Vec<CapabilityConflict>,
}

impl Requirements {
    /// Returns whether nothing blocks the run: no unmet model
    /// requirements, no missing required capabilities, and no
    /// co-activation conflicts.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.unmet_requirements.is_empty()
            && self.missing_required.is_empty()
            && self.conflicts.is_empty()
    }

    /// Folds `other` into this report: the host merges what activation
    /// could not satisfy into what prepare could not, so one refusal names
    /// every gap. A capability already reported missing is not repeated.
    pub fn merge(&mut self, other: Requirements) {
        for id in other.missing_required {
            if !self.missing_required.contains(&id) {
                self.missing_required.push(id);
            }
        }
        self.conflicts.extend(other.conflicts);
        self.unmet_requirements.extend(other.unmet_requirements);
    }

    /// The refusal a host fails the run with when the report is
    /// unsatisfied: a [`RunError`](super::RunError) of kind
    /// [`RequirementsUnmet`](super::RunErrorKind::RequirementsUnmet)
    /// carrying the [`notice`](Requirements::notice), or `None` when
    /// nothing blocks the run. The host checks this after merging
    /// activation's report into prepare's, before building the run.
    #[must_use]
    pub fn refusal(&self) -> Option<super::RunError> {
        (!self.is_satisfied()).then(|| {
            super::RunError::from(crate::Error::RequirementsUnmet {
                notice: self.notice(),
            })
        })
    }

    /// The refusal notice a host fails the run with when the report is
    /// unsatisfied.
    ///
    /// Written to be read by a model - concise, factual, self-contained -
    /// because it may arrive as tool output when the prompt runs as a
    /// sub-run tool. Each line names what is missing or unmet, with
    /// required versus actual.
    #[must_use]
    pub fn notice(&self) -> String {
        // Writing to a String is infallible; the `let _` mirrors the
        // crate's established pattern (subst.rs) under the denied
        // `unwrap_used`/`expect_used` lints.
        use std::fmt::Write as _;
        let mut notice = String::from("the environment cannot satisfy this prompt:");
        for id in &self.missing_required {
            let _ = write!(notice, "\n- missing required capability: {id}");
        }
        for conflict in &self.conflicts {
            let _ = write!(
                notice,
                "\n- conflicting capabilities: {} and {} cannot be activated \
                 together; declare one or the other",
                conflict.first, conflict.second
            );
        }
        for unmet in &self.unmet_requirements {
            let line = match unmet.check {
                RequirementCheck::ContextMinimum => format!(
                    "role '{}': requires a context of at least {} tokens; \
                     the current model provides {}",
                    unmet.role, unmet.required, unmet.actual
                ),
                RequirementCheck::HardKeyword => format!(
                    "role '{}': requires '{}'; \
                     the current model's thinking capability is {}",
                    unmet.role, unmet.required, unmet.actual
                ),
            };
            let _ = write!(notice, "\n- {line}");
        }
        notice
    }
}

/// One declared co-activation conflict: two present capabilities that
/// cannot activate in one run, named in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CapabilityConflict {
    /// The earlier-declared capability.
    pub first: CapabilityId,
    /// The later-declared capability.
    pub second: CapabilityId,
}

impl CapabilityConflict {
    /// Records one conflicting pair in declaration order: `first` was
    /// declared before `second`. The host's activation reports these; the
    /// engine's prepare never does.
    #[must_use]
    pub fn new(first: CapabilityId, second: CapabilityId) -> CapabilityConflict {
        CapabilityConflict { first, second }
    }
}

/// One failed model requirement: the role, which check failed, and what
/// the prompt required versus what the filled model provides.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnmetRequirement {
    /// The declared role label whose requirement failed.
    pub role: String,
    /// Which requirement check failed.
    pub check: RequirementCheck,
    /// What the prompt required (a context minimum of `200000`; the
    /// `thinking` keyword).
    pub required: String,
    /// What the filled model provides (a context of `32000`; a `Never`
    /// thinking capability).
    pub actual: String,
}

/// Which model requirement check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequirementCheck {
    /// The role's context minimum exceeds the filled model's context.
    ContextMinimum,
    /// A hard keyword (`thinking`, `no-thinking`) the filled model's
    /// descriptor does not satisfy.
    HardKeyword,
}
