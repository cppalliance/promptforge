//! The preflight report: [`Requirements`].

use promptforge_types::capabilities::CapabilityId;

#[cfg(test)]
#[path = "requirements-tests.rs"]
mod tests;

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
    /// The required capabilities that are present but need a host
    /// service this host does not provide: one entry per capability and
    /// missing service. Reported by activation, which does not activate
    /// such a capability; the run fails until the host provides the
    /// service or the prompt declares the capability optional.
    pub missing_services: Vec<MissingService>,
    /// The declared co-activation conflicts: pairs of present
    /// capabilities that cannot activate in one run (bashkit vs
    /// terminal - two filesystem realities, and a context gets one or
    /// the other, never both). Neither member of a conflicting pair
    /// activates; the run fails until the prompt declares one or the
    /// other. Reported by activation, never by prepare.
    pub conflicts: Vec<CapabilityConflict>,
}

impl Requirements {
    /// Returns whether the report is satisfied: every model requirement
    /// is met, every required capability is present and has the host
    /// services it needs, and no pair of capabilities conflicts.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.unmet_requirements.is_empty()
            && self.missing_required.is_empty()
            && self.missing_services.is_empty()
            && self.conflicts.is_empty()
    }

    /// Folds `other` into this report: the host merges what activation
    /// could not satisfy into what prepare could not, so one refusal names
    /// every gap. A capability already reported missing, or a service
    /// already reported missing for the same capability, is not repeated.
    pub fn merge(&mut self, other: Requirements) {
        for id in other.missing_required {
            if !self.missing_required.contains(&id) {
                self.missing_required.push(id);
            }
        }
        for missing in other.missing_services {
            if !self.missing_services.contains(&missing) {
                self.missing_services.push(missing);
            }
        }
        self.conflicts.extend(other.conflicts);
        self.unmet_requirements.extend(other.unmet_requirements);
    }

    /// The refusal a host fails the run with when the report is
    /// unsatisfied: a [`RunError`](super::RunError) of kind
    /// [`RequirementsUnmet`](super::RunErrorKind::RequirementsUnmet)
    /// reporting the [`notice`](Requirements::notice), or `None` when
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
        for missing in &self.missing_services {
            let _ = write!(
                notice,
                "\n- {} needs {}, and this host provides none",
                missing.capability, missing.service
            );
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

/// One host service a required capability needs and the host does not
/// provide, such as a capability that asks the operator on a batch host
/// with nobody to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MissingService {
    /// The present, required capability that needs the service.
    pub capability: CapabilityId,
    /// The service's model-readable name, such as "an input broker". The
    /// host owns its service vocabulary, so the report carries the name
    /// the host gave it.
    pub service: String,
}

impl MissingService {
    /// Records that `capability` needs the host service named `service`
    /// and the host does not provide it. The host's activation reports
    /// these; the engine's prepare never does.
    #[must_use]
    pub fn new(capability: CapabilityId, service: impl Into<String>) -> MissingService {
        MissingService {
            capability,
            service: service.into(),
        }
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
