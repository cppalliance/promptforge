//! Validation of the profile checklists and selection of the active profile.

use std::collections::HashSet;

use super::super::{Config, SttRole};
use crate::error::ConfigError;
use crate::profile::ProfileName;

impl Config {
    fn validate_profile_vram(
        &self,
        profile_name: &str,
        selected: &HashSet<&str>,
    ) -> Result<(), ConfigError> {
        for dominion in &self.dominions {
            let Some(budget) = dominion.vram_gb else {
                continue;
            };
            let mut total = 0.0;
            for model in &self.catalog_local_models {
                if !selected.contains(model.name.as_str()) {
                    continue;
                }
                let Some(bound) = &model.dominion else {
                    continue;
                };
                if bound != &dominion.id {
                    continue;
                }
                let Some(estimate) = model.vram_gb else {
                    return Err(ConfigError::Validation(format!(
                        "profile {profile_name} selects local_model {} without vram_gb, \
                         but dominion {} has a vram_gb budget",
                        model.name, dominion.id,
                    )));
                };
                total += estimate;
            }
            for model in &self.catalog_stt_models {
                if selected.contains(model.name.as_str())
                    && model.dominion.as_deref() == Some(dominion.id.as_str())
                {
                    total += model.vram_gb;
                }
            }
            let budget = f64::from(budget);
            if total > budget {
                return Err(ConfigError::Validation(format!(
                    "profile {profile_name} exceeds dominion {} vram_gb budget {budget} \
                     by {} (selected local and STT models sum to {total})",
                    dominion.id,
                    total - budget,
                )));
            }
        }
        Ok(())
    }

    pub(super) fn validate_profiles(&self) -> Result<(), ConfigError> {
        // A profile gates only what the gateway must spawn or load itself.
        // Remote models cost nothing to serve, so they are always routed and
        // a profile that lists one is told so rather than silently accepted.
        let selectable: HashSet<&str> = self
            .catalog_local_models
            .iter()
            .map(|model| model.name.as_str())
            .chain(
                self.catalog_stt_models
                    .iter()
                    .map(|model| model.name.as_str()),
            )
            .collect();
        let remote: HashSet<&str> = self
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect();
        let mut profile_names = HashSet::new();
        for profile in &self.profiles {
            ProfileName::parse(&profile.name).map_err(|error| {
                ConfigError::Validation(format!(
                    "profile name {:?} is invalid: {error}",
                    profile.name
                ))
            })?;
            if !profile_names.insert(profile.name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "duplicate profile name {}",
                    profile.name
                )));
            }

            let mut selected = HashSet::new();
            let mut interim = None;
            let mut final_model = None;
            for name in &profile.models {
                if !selectable.contains(name.as_str()) {
                    if remote.contains(name.as_str()) {
                        return Err(ConfigError::Validation(format!(
                            "profile {:?} lists remote model {name:?}; a profile selects only \
                             [[local_model]] and [[stt_model]] entries, remote models are \
                             always served",
                            profile.name
                        )));
                    }
                    return Err(ConfigError::Validation(format!(
                        "profile {} names undefined catalog model {name}",
                        profile.name
                    )));
                }
                if !selected.insert(name.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "profile {} lists duplicate model {name}",
                        profile.name
                    )));
                }
                let Some(stt_model) = self
                    .catalog_stt_models
                    .iter()
                    .find(|model| model.name == *name)
                else {
                    continue;
                };
                let slot = match stt_model.role {
                    SttRole::Interim => &mut interim,
                    SttRole::Final => &mut final_model,
                };
                if let Some(first) = slot.replace(name.as_str()) {
                    return Err(ConfigError::Validation(format!(
                        "profile {} selects more than one {:?} STT model: {first} and {name}",
                        profile.name, stt_model.role
                    )));
                }
            }
            if interim.is_none()
                && let Some(final_name) = final_model
            {
                return Err(ConfigError::Validation(format!(
                    "profile {} selects final STT model {final_name} without an interim model; \
                     add one interim STT model or remove the final model",
                    profile.name
                )));
            }
            self.validate_profile_vram(&profile.name, &selected)?;
        }
        Ok(())
    }

    /// Selects `name`, or no profile at all, narrowing only the local and
    /// speech-to-text sets. The remote routing table is never touched.
    ///
    /// Any stale state-file name recorded by an earlier load is cleared: a
    /// fresh selection supersedes it, and `Config::load` records the stale
    /// name only after this call when the state file wins.
    pub(crate) fn activate_profile(
        &mut self,
        name: Option<&ProfileName>,
    ) -> Result<(), ConfigError> {
        self.stale_state_selection = None;
        let Some(name) = name else {
            self.local_models = Vec::new();
            self.stt_models = Vec::new();
            self.active_profile = None;
            return Ok(());
        };
        let Some(index) = self
            .profiles
            .iter()
            .position(|profile| profile.name == name.as_str())
        else {
            return Err(ConfigError::Validation(format!(
                "active profile {} is not defined (defined profiles: {})",
                name,
                self.defined_profile_names()
            )));
        };
        let selected: HashSet<&str> = self.profiles[index]
            .models
            .iter()
            .map(String::as_str)
            .collect();
        self.local_models = self
            .catalog_local_models
            .iter()
            .filter(|model| selected.contains(model.name.as_str()))
            .cloned()
            .collect();
        self.stt_models = self
            .catalog_stt_models
            .iter()
            .filter(|model| selected.contains(model.name.as_str()))
            .cloned()
            .collect();
        self.active_profile = Some(index);
        Ok(())
    }

    fn defined_profile_names(&self) -> String {
        if self.profiles.is_empty() {
            return "<none>".to_owned();
        }
        self.profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}
