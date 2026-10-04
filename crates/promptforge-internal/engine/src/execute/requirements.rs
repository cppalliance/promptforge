//! The preflight report: [`Requirements`].

use promptforge_types::capabilities::CapabilityId;

#[cfg(test)]
#[path = "requirements-tests.rs"]
mod tests;

/// A report of what the caller must still satisfy before a prompt can run.
///
/// [`Environment::prepare`](super::Environment::prepare) returns one
/// together with the prepared context. The report lists only what needs a
/// person's attention. An optional capability that is skipped is logged,
/// not reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Requirements {
    /// The model requirements that the bound models do not meet.
    ///
    /// Each entry names the role, the check that failed, and what was
    /// required versus what the model provides. Examples are a
    /// `min_context` of 200000 against a model with a 32k context, and
    /// `thinking` against a model whose thinking mode is `Never`. Only
    /// model binding in `Environment::prepare` adds these. Capability
    /// activation adds none.
    pub unmet_requirements: Vec<UnmetRequirement>,
    /// The required capabilities that the run cannot have.
    ///
    /// Capability activation adds a capability that is not registered or
    /// that fails to activate. `Environment::prepare` adds the capability
    /// of an exact tool slot when that capability contributed nothing to
    /// the tool catalog. The run fails until every one is satisfied.
    pub missing_required: Vec<CapabilityId>,
    /// The required capabilities that are registered but need a service
    /// the application does not provide.
    ///
    /// There is one entry for each capability and missing service.
    /// Capability activation adds these, and it does not activate such a
    /// capability. The run fails until the application provides the
    /// service or the prompt declares the capability optional.
    pub missing_services: Vec<MissingService>,
    /// The declared conflicts: pairs of registered capabilities that
    /// cannot activate in the same run.
    ///
    /// For example, `bashkit` and `terminal` each give the run its own
    /// view of the filesystem, so a run gets one or the other, never both.
    /// Neither member of a conflicting pair activates. The run fails until
    /// the prompt declares only one of them. Capability activation adds
    /// these, and `Environment::prepare` never does.
    pub conflicts: Vec<CapabilityConflict>,
}

impl Requirements {
    /// Returns whether nothing in the report blocks the run.
    ///
    /// That holds when every model requirement is met, every required
    /// capability is present and has the services it needs, and no pair of
    /// capabilities conflicts.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.unmet_requirements.is_empty()
            && self.missing_required.is_empty()
            && self.missing_services.is_empty()
            && self.conflicts.is_empty()
    }

    /// Adds the entries of `other` to this report.
    ///
    /// Use it to combine the capability activation report with the report
    /// from `Environment::prepare`, so that one refusal names every gap. A
    /// capability already reported missing, or a service already reported
    /// missing for the same capability, is not added again.
    ///
    /// A capability that lacks a service is not also reported as missing.
    /// Such a `missing_required` entry can come only from the tool slot
    /// check in `Environment::prepare`. That check finds an exact slot
    /// whose capability contributed nothing, because activation skipped
    /// the capability for the missing service. The service entry already
    /// names the real cause.
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
        self.missing_required.retain(|id| {
            !self
                .missing_services
                .iter()
                .any(|missing| missing.capability == *id)
        });
        self.conflicts.extend(other.conflicts);
        self.unmet_requirements.extend(other.unmet_requirements);
    }

    /// Returns the error to fail the run with, or `None` when nothing
    /// blocks the run.
    ///
    /// The error is a [`RunError`](super::RunError) of kind
    /// [`RequirementsUnmet`](super::RunErrorKind::RequirementsUnmet) that
    /// reports the [`notice`](Requirements::notice). The caller checks it
    /// after merging every report into one, and before building the run.
    #[must_use]
    pub fn refusal(&self) -> Option<super::RunError> {
        (!self.is_satisfied()).then(|| {
            super::RunError::from(crate::Error::RequirementsUnmet {
                notice: self.notice(),
            })
        })
    }

    /// Returns the refusal notice, which explains what blocks the run.
    ///
    /// Each line names what is missing or unmet, with required versus
    /// actual. The notice can arrive as tool output when the prompt runs as
    /// a tool of another run, so it is written for a model to read:
    /// concise, factual, and self-contained.
    #[must_use]
    pub fn notice(&self) -> String {
        // Writing to a String is infallible, so each `write!` result is
        // discarded under the denied `unwrap_used`/`expect_used` lints.
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

/// A declared conflict between two registered capabilities that cannot
/// activate in the same run.
///
/// The pair is named in the order the prompt declares them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CapabilityConflict {
    /// The earlier-declared capability.
    pub first: CapabilityId,
    /// The later-declared capability.
    pub second: CapabilityId,
}

impl CapabilityConflict {
    /// Creates a conflict between `first` and `second`, where the prompt
    /// declares `first` before `second`.
    ///
    /// Only capability activation reports these.
    #[must_use]
    pub fn new(first: CapabilityId, second: CapabilityId) -> CapabilityConflict {
        CapabilityConflict { first, second }
    }
}

/// A service that a required capability needs and the application does
/// not provide.
///
/// For example, a capability that asks the operator a question needs a
/// service that reaches the operator, and a batch application has nobody
/// to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MissingService {
    /// The registered, required capability that needs the service.
    pub capability: CapabilityId,
    /// The name of the missing service, as the caller gave it.
    ///
    /// The caller defines its own service names, so the report carries
    /// the name unchanged.
    pub service: String,
}

impl MissingService {
    /// Creates an entry stating that `capability` needs the service named
    /// `service`, which the application does not provide.
    ///
    /// Only capability activation reports these.
    #[must_use]
    pub fn new(capability: CapabilityId, service: impl Into<String>) -> MissingService {
        MissingService {
            capability,
            service: service.into(),
        }
    }
}

/// A model requirement that the model bound to a role does not meet.
///
/// It names the role, the check that failed, and what the prompt required
/// versus what the bound model provides.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnmetRequirement {
    /// The declared role label whose requirement failed.
    pub role: String,
    /// Which requirement check failed.
    pub check: RequirementCheck,
    /// What the prompt required, such as a context minimum of `200000` or
    /// the `thinking` keyword.
    pub required: String,
    /// What the bound model provides, such as a context of `32000` or a
    /// thinking mode of `Never`.
    pub actual: String,
}

/// Which model requirement check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequirementCheck {
    /// The role's context minimum exceeds the context of the model bound
    /// to the role.
    ContextMinimum,
    /// The role declares a hard keyword, `thinking` or `no-thinking`, that
    /// the bound model's descriptor does not satisfy.
    HardKeyword,
}
