//! The Host's installed Plugins: [`HostContext`], built once at startup and
//! shared by every run, and the snapshot each run takes of it.
//!
//! A Host installs each Plugin crate's [`Package`] under a local name, and
//! install calls the package's `construct` once to build the one Plugin
//! object every run shares. A construct failure is not an install error:
//! the Plugin is stored as unavailable with the failure as its reason, and
//! every run that needs it is refused naming that reason. Before each run,
//! the Harness's preparation waits for every built Plugin to be ready, and
//! a Plugin whose `ready` fails is unavailable to that run the same way.
//! Then it takes a snapshot of every installed Plugin with the run's own
//! services: the catalog the Engine fills slots against, the preludes of
//! the Plugins the prompt declares, the requirements the snapshot cannot
//! meet, and the performer that sends each tool call to the Plugin its id
//! names.

use std::collections::BTreeMap;
use std::fmt;
use std::pin::pin;
use std::sync::Arc;

use futures_util::future::{Either, join_all, select};
use promptforge::cancel::CancelHandle;
use promptforge::{Environment, Prompt, Requirements};
use promptforge_plugin::{HostServices, Package, Plugin, PluginId};

#[path = "host-run.rs"]
mod run;

pub(crate) use run::HostRunContext;

#[cfg(test)]
#[path = "host-tests.rs"]
mod tests;

/// Every Plugin a Host installed, and the Host-wide services their
/// `construct` functions read.
///
/// A Host builds one at startup, installs its Plugins, and shares it with
/// every Harness behind an `Arc`. Dropping it drops the Plugin objects,
/// which is where a Plugin cleans up.
pub struct HostContext {
    services: HostServices,
    installed: Vec<Installed>,
}

/// One installed Plugin: its local name, its label, and the built object
/// or the reason building it failed.
struct Installed {
    name: PluginId,
    package: Package,
    plugin: Result<Arc<dyn Plugin>, String>,
}

impl HostContext {
    /// A context with no Plugins, whose installs hand `services` to each
    /// package's `construct`.
    ///
    /// `services` are Host-wide, such as a search provider. They reach
    /// only `construct`; a tool call reads the run's own services instead.
    #[must_use]
    pub fn new(services: HostServices) -> HostContext {
        HostContext {
            services,
            installed: Vec::new(),
        }
    }

    /// Builds the Plugin with `package.construct` and adds it under `name`,
    /// or under the package name's second segment when `name` is `None`,
    /// and returns the name it used.
    ///
    /// `construct` runs once, here, with the name, `config`, and the
    /// Host-wide services. A `construct` failure is not an error here: the
    /// Plugin is stored as unavailable, with the failure as the reason a
    /// run that needs it is refused.
    ///
    /// # Errors
    /// Returns [`InstallError::InvalidPackage`] when the package name is
    /// not a `vendor/name` pair of valid one-segment names, and
    /// [`InstallError::NameTaken`] when the name is installed already or
    /// differs from an installed name only by `-`, `_`, or `.`.
    pub fn install(
        &mut self,
        package: Package,
        name: Option<PluginId>,
        config: serde_json::Value,
    ) -> Result<PluginId, InstallError> {
        let default = package_name(package.name).ok_or(InstallError::InvalidPackage {
            package: package.name,
        })?;
        let name = name.unwrap_or(default);
        let twin = normalize(&name);
        if let Some(existing) = self
            .installed
            .iter()
            .find(|installed| normalize(&installed.name) == twin)
        {
            return Err(InstallError::NameTaken {
                name,
                existing: existing.name.clone(),
            });
        }
        let plugin = (package.construct)(&name, config, &self.services).map_err(|error| {
            tracing::warn!(
                plugin = %name,
                package = package.name,
                %error,
                "Plugin construct failed; it is installed as unavailable"
            );
            error.to_string()
        });
        self.installed.push(Installed {
            name: name.clone(),
            package,
            plugin,
        });
        Ok(name)
    }

    /// Waits until every built Plugin's `ready` resolves, polling them
    /// together, and returns the reason of each one that will not be
    /// ready, by name.
    ///
    /// A cancel on `cancel` ends the wait with no failures, so each Plugin
    /// is snapshotted as it stands, and a run cancelled here ends
    /// cancelled whatever the snapshot holds.
    pub(crate) async fn wait_ready(&self, cancel: &CancelHandle) -> BTreeMap<PluginId, String> {
        let waits = self.installed.iter().filter_map(|installed| {
            let plugin = installed.plugin.as_ref().ok()?;
            Some(async move { (&installed.name, plugin.ready().await) })
        });
        let joined = pin!(join_all(waits));
        // The cancel arm is polled first, so a run cancelled already does
        // not wait.
        match select(cancel.cancelled(), joined).await {
            Either::Left(((), _)) => BTreeMap::new(),
            Either::Right((results, _)) => results
                .into_iter()
                .filter_map(|(name, result)| {
                    let error = result.err()?;
                    tracing::warn!(
                        plugin = %name,
                        %error,
                        "Plugin will not be ready; this run treats it as unavailable"
                    );
                    Some((name.clone(), error.to_string()))
                })
                .collect(),
        }
    }

    /// Takes one run's snapshot of every installed Plugin with `services`,
    /// the run's own services, and builds what the Engine needs for
    /// `prompt`.
    ///
    /// `failures` holds the reason of each Plugin whose `ready` failed
    /// before this run, which the run treats as unavailable. The
    /// environment's catalog holds every usable Plugin's tools, declared
    /// or not, and its preludes are the declared, usable Plugins', in
    /// declaration order. The requirements cover every Plugin the prompt
    /// declares or names in a tool slot. The returned context performs the
    /// run's tool calls.
    pub(crate) fn begin_run(
        &self,
        services: HostServices,
        failures: BTreeMap<PluginId, String>,
        prompt: &Prompt,
    ) -> (HostRunContext, Environment, Requirements) {
        let run = HostRunContext::snapshot(
            self.installed
                .iter()
                .map(|installed| (&installed.name, installed.package, &installed.plugin)),
            services,
            failures,
        );
        let env = Environment::new()
            .tools(run.catalog())
            .preludes(run.preludes(prompt));
        let requirements = run.requirements(prompt);
        (run, env, requirements)
    }
}

impl fmt::Debug for HostContext {
    /// Shows the services and each installed name with whether it built.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let installed: Vec<(&PluginId, bool)> = self
            .installed
            .iter()
            .map(|installed| (&installed.name, installed.plugin.is_ok()))
            .collect();
        f.debug_struct("HostContext")
            .field("services", &self.services)
            .field("installed", &installed)
            .finish()
    }
}

/// A mistake [`HostContext::install`] refuses outright, so the Host's
/// author sees it at startup.
///
/// A Plugin whose `construct` fails is not one of these; it is installed
/// as unavailable instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum InstallError {
    /// The package name is not `vendor/name`, or a segment of it is not a
    /// valid one-segment name.
    #[error("Plugin package name `{package}` is not a vendor/name pair")]
    InvalidPackage {
        /// The package name as the label spells it.
        package: &'static str,
    },
    /// The name is installed already (`existing` equals `name`), or it
    /// differs from the installed `existing` only by `-`, `_`, or `.`, as
    /// `user-input` and `user_input` do, which a model reading the tool
    /// list cannot tell apart.
    #[error(
        "Plugin name {name} is taken by the installed {existing}; install it under another name"
    )]
    NameTaken {
        /// The name the install asked for.
        name: PluginId,
        /// The installed name it collides with.
        existing: PluginId,
    },
}

/// The default local name for the package name `vendor/name`: its second
/// segment, when both segments are valid one-segment names.
fn package_name(package: &str) -> Option<PluginId> {
    let (vendor, name) = package.split_once('/')?;
    PluginId::parse(vendor).ok()?;
    PluginId::parse(name).ok()
}

/// A name with each `-`, `_`, and `.` mapped to `-`, so two names that
/// differ only by that punctuation compare equal. Names are lowercase
/// only, so case needs no handling.
fn normalize(name: &PluginId) -> String {
    name.to_string()
        .chars()
        .map(|c| if matches!(c, '-' | '_' | '.') { '-' } else { c })
        .collect()
}
