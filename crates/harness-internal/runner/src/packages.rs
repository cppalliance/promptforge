//! The packages a Host can install: [`PackageRegistry`], holding the
//! natives from the start and each linked package the Host adds, and the
//! [`PluginKind`] that says which a package is.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Arc;

use promptforge_plugin::{
    HostServices, Package, Plugin, PluginFuture, PluginId, ToolContext, ToolDescriptor, ToolError,
    ToolOutput,
};
use serde_json::Value;

use crate::host::InstallError;

#[cfg(test)]
#[path = "packages-tests.rs"]
mod tests;

/// The packages a Host can install, by package name.
///
/// Every registry holds the natives, such as `promptforge/subagents`, from
/// the start. A Host adds the `PACKAGE` constant of each Plugin crate it
/// links, lists what the registry holds, and builds its
/// [`HostContext`](crate::HostContext) over it, which installs Plugins by
/// package name.
#[derive(Debug)]
pub struct PackageRegistry {
    entries: BTreeMap<&'static str, (Package, PluginKind, PluginId)>,
}

/// Where a package's Plugin comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginKind {
    /// Built into every registry; its behavior will live in the Engine, and
    /// today's one native, `promptforge/subagents`, has none yet.
    Native,
    /// Added by the Host with [`PackageRegistry::add`].
    Linked,
}

impl Default for PackageRegistry {
    /// A registry holding only the natives.
    fn default() -> PackageRegistry {
        let entries = NATIVE
            .into_iter()
            .filter_map(|package| {
                let default = package_name(package.name)?;
                Some((package.name, (package, PluginKind::Native, default)))
            })
            .collect();
        PackageRegistry { entries }
    }
}

impl PackageRegistry {
    /// A registry holding only the natives.
    #[must_use]
    pub fn new() -> PackageRegistry {
        PackageRegistry::default()
    }

    /// Adds `package` as a linked package, so a Host can install it by its
    /// name.
    ///
    /// # Errors
    /// Returns [`InstallError::InvalidPackage`] when the package name is
    /// not a `vendor/name` pair of valid one-segment names, and
    /// [`InstallError::DuplicatePackage`] when the registry holds the name
    /// already, as it holds each native's from the start.
    pub fn add(&mut self, package: Package) -> Result<(), InstallError> {
        let default = package_name(package.name).ok_or(InstallError::InvalidPackage {
            package: package.name,
        })?;
        match self.entries.entry(package.name) {
            Entry::Occupied(_) => Err(InstallError::DuplicatePackage {
                package: package.name,
            }),
            Entry::Vacant(slot) => {
                slot.insert((package, PluginKind::Linked, default));
                Ok(())
            }
        }
    }

    /// Every package the registry holds, natives included, with its kind,
    /// in package name order.
    pub fn list(&self) -> impl Iterator<Item = (&Package, PluginKind)> {
        self.entries
            .values()
            .map(|(package, kind, _)| (package, *kind))
    }

    /// The package named `package`, its kind, and its default local name.
    pub(crate) fn entry(&self, package: &str) -> Option<(Package, PluginKind, &PluginId)> {
        let (package, kind, default) = self.entries.get(package)?;
        Some((*package, *kind, default))
    }

    /// Whether `name` is a native's default local name, which only that
    /// native may be installed under.
    pub(crate) fn reserves(&self, name: &PluginId) -> bool {
        self.entries
            .values()
            .any(|(_, kind, default)| *kind == PluginKind::Native && default == name)
    }
}

/// The natives every registry holds from the start.
///
/// A native Plugin's behavior and tools live in the Engine. Its Plugin
/// object exists only so install, readiness, and the run snapshot treat it
/// like any other Plugin, so a native must never offer tools, carry a
/// prelude, or need services. Its `call` is unreachable, because the run
/// snapshot routes a call only to a tool in a Plugin's own list.
const NATIVE: [Package; 1] = [Package::new("promptforge/subagents", native).description(
    "Reserves the subagents name for the Engine's subagent tools, which are not built yet",
)];

/// Builds a native's [`Native`] object; see [`NATIVE`].
#[expect(
    clippy::unnecessary_wraps,
    reason = "the Package construct signature fixes the return type"
)]
fn native(
    _name: &PluginId,
    _config: Value,
    _services: &HostServices,
) -> Result<Arc<dyn Plugin>, ToolError> {
    Ok(Arc::new(Native))
}

/// The object of a native Plugin, which offers no tools; see [`NATIVE`].
struct Native;

impl Plugin for Native {
    fn tools(&self) -> Vec<ToolDescriptor> {
        Vec::new()
    }

    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        _args: Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>> {
        let message = format!(
            "{} belongs to a native Plugin, whose calls the Engine answers",
            cx.tool()
        );
        Box::pin(async move { Err(ToolError::message(message)) })
    }
}

/// The default local name for the package name `vendor/name`: its second
/// segment, when both segments are valid one-segment names.
pub(crate) fn package_name(package: &str) -> Option<PluginId> {
    let (vendor, name) = package.split_once('/')?;
    PluginId::parse(vendor).ok()?;
    PluginId::parse(name).ok()
}
