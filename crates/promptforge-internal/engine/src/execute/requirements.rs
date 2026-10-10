//! The preflight report: [`Requirements`].

use promptforge_types::plugins::PluginId;

#[cfg(test)]
#[path = "requirements-tests.rs"]
mod tests;

/// A report of what the caller must still satisfy before a prompt can run.
///
/// [`Environment::prepare`](super::Environment::prepare) returns one
/// together with the prepared context. The report lists only what needs a
/// person's attention.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Requirements {
    /// The model requirements that the bound models fall short of.
    ///
    /// Each entry names the role, the check that failed, and what was
    /// required versus what the model provides. Examples are a
    /// `min_context` of 200000 against a model with a 32k context, and
    /// `thinking` against a model whose thinking mode is `Never`. Only
    /// model binding in `Environment::prepare` adds these.
    pub unmet_requirements: Vec<UnmetRequirement>,
    /// The required Plugins that the run lacks.
    ///
    /// The caller adds a Plugin the prompt declares that is not installed.
    /// The run fails until every one is satisfied.
    pub missing_required: Vec<PluginId>,
    /// The required Plugins that are installed but need a service
    /// the run lacks.
    ///
    /// There is one entry for each Plugin and missing service. The caller
    /// adds these and leaves each such Plugin's tools out of the catalog.
    /// The run fails until the application provides the service.
    pub missing_services: Vec<MissingService>,
    /// The required Plugins that are installed but cannot serve, each
    /// with the reason, such as a failure to build.
    ///
    /// The caller adds these. The run fails until the Plugin can serve.
    pub unavailable: Vec<UnavailablePlugin>,
}

impl Requirements {
    /// Returns whether the report lets the run proceed.
    ///
    /// That holds when every model requirement is met and every required
    /// Plugin is present, can serve, and has the services it needs.
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        self.unmet_requirements.is_empty()
            && self.missing_required.is_empty()
            && self.missing_services.is_empty()
            && self.unavailable.is_empty()
    }

    /// Adds the entries of `other` to this report.
    ///
    /// Use it to combine the caller's Plugin report with the report
    /// from `Environment::prepare`, so that one refusal names every gap.
    /// The merge skips an entry the report already holds.
    ///
    /// The merge drops the `missing_required` entry of a Plugin that
    /// lacks a service or is unavailable: the service or unavailable entry
    /// already names the real cause.
    pub fn merge(&mut self, other: Requirements) {
        push_new(&mut self.missing_required, other.missing_required);
        push_new(&mut self.missing_services, other.missing_services);
        push_new(&mut self.unavailable, other.unavailable);
        let (services, unavailable) = (&self.missing_services, &self.unavailable);
        self.missing_required.retain(|id| {
            !services.iter().any(|missing| missing.plugin == *id)
                && !unavailable.iter().any(|entry| entry.plugin == *id)
        });
        self.unmet_requirements.extend(other.unmet_requirements);
    }

    /// Returns the error to fail the run with, or `None` when the report
    /// is satisfied.
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
    /// Each line names what is missing or falls short, with required versus
    /// actual. The notice is written for a model to read: concise, factual,
    /// and self-contained.
    #[must_use]
    pub fn notice(&self) -> String {
        // Writing to a String is infallible, so each `write!` result is
        // discarded under the denied `unwrap_used`/`expect_used` lints.
        use std::fmt::Write as _;
        let mut notice = String::from("the environment cannot satisfy this prompt:");
        for id in &self.missing_required {
            let _ = write!(notice, "\n- missing required Plugin: {id}");
        }
        for missing in &self.missing_services {
            let _ = write!(
                notice,
                "\n- {} needs {}, and the environment provides none",
                missing.plugin, missing.service
            );
        }
        for entry in &self.unavailable {
            let _ = write!(
                notice,
                "\n- {} is unavailable: {}",
                entry.plugin, entry.reason
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

/// Appends each entry of `incoming` that `entries` does not already hold.
fn push_new<T: PartialEq>(entries: &mut Vec<T>, incoming: Vec<T>) {
    for entry in incoming {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
}

/// A required Plugin that is installed but cannot serve, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnavailablePlugin {
    /// The installed, required Plugin.
    pub plugin: PluginId,
    /// Why it cannot serve, written for the model that reads the refusal.
    pub reason: String,
}

impl UnavailablePlugin {
    /// Creates an entry stating that `plugin` cannot serve, because of
    /// `reason`.
    ///
    /// Only the caller's Plugin report holds these.
    #[must_use]
    pub fn new(plugin: PluginId, reason: impl Into<String>) -> UnavailablePlugin {
        UnavailablePlugin {
            plugin,
            reason: reason.into(),
        }
    }
}

/// A service that a required Plugin needs and the application lacks.
///
/// For example, a Plugin that asks the operator a question needs a
/// service that reaches the operator, and a batch application runs on its
/// own, so it lacks that service.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MissingService {
    /// The installed, required Plugin that needs the service.
    pub plugin: PluginId,
    /// The name of the missing service.
    ///
    /// The caller defines its own service names, so the report carries
    /// the name verbatim.
    pub service: String,
}

impl MissingService {
    /// Creates an entry stating that `plugin` needs the service named
    /// `service`, which the application lacks.
    ///
    /// Only the caller's Plugin report holds these.
    #[must_use]
    pub fn new(plugin: PluginId, service: impl Into<String>) -> MissingService {
        MissingService {
            plugin,
            service: service.into(),
        }
    }
}

/// A model requirement that the model bound to a role falls short of.
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
    /// the bound model's descriptor fails to satisfy.
    HardKeyword,
}
