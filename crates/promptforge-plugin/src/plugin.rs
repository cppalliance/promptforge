//! The [`Plugin`] trait and the [`Package`] label a Plugin crate exports.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use promptforge_types::plugins::PluginId;
use promptforge_types::tools::{ToolDescriptor, ToolError, ToolOutput};

use crate::{HostServices, ServiceId, ToolContext};

/// A boxed future that may borrow for `'a`, the same type as
/// `futures::future::BoxFuture`.
///
/// [`Plugin::call`] returns one, so the Harness can hold Plugins as
/// `dyn Plugin`. An implementation wraps its body in
/// `Box::pin(async move { ... })`.
pub type PluginFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One Plugin object: the tools it offers and the calls it performs.
///
/// The Host builds one object per installed Plugin, through
/// [`Package::construct`], and every run shares it, so an implementation
/// is `Send + Sync`. Cleanup, such as stopping a child process, goes in
/// the implementation's `Drop`, which runs when the Host drops the context
/// holding the Plugin. There is no shutdown hook.
pub trait Plugin: Send + Sync {
    /// Returns the tools the Plugin offers now.
    ///
    /// The Harness reads the list once when each run starts, so a Plugin
    /// whose tools change returns its current list, and a Plugin still
    /// coming up returns what it has so far. Each tool's id sits under the
    /// name the Plugin was installed under, as in `web/fetch`.
    fn tools(&self) -> Vec<ToolDescriptor>;

    /// Performs one tool call.
    ///
    /// `cx` names the called tool and lends the call's filesystem access,
    /// its origin, and the run's services. `args` is the JSON the model or
    /// the script passed.
    ///
    /// # Invariants
    ///
    /// - `call` must not block while polled. The Harness polls every
    ///   effect of a run on the same task, so blocking or CPU-heavy work
    ///   goes to the Host's runtime, without the access. Operations
    ///   through [`ToolContext::access`] run inside the call; on real
    ///   directories they block the poll briefly, as `store.*` does.
    /// - `call` must not panic. If it does, the Harness answers its effect
    ///   `Dropped` and logs the panic.
    /// - `call` marks the trust of every output correctly. Output that
    ///   embeds data an attacker can influence is
    ///   [`ToolOutput::untrusted`].
    /// - `call` is cancellation-aware. The Harness drops the returned
    ///   future on a stop or a cancel, unless the tool's descriptor sets
    ///   `survives_stop`, in which case only a cancel drops it.
    ///
    /// # Errors
    /// Returns a [`ToolError`], whose message is safe to show the model,
    /// when the call rejects the arguments, its backend refuses, its
    /// transport fails, or the run is cancelled.
    fn call<'a>(
        &'a self,
        cx: ToolContext<'a>,
        args: serde_json::Value,
    ) -> PluginFuture<'a, Result<ToolOutput, ToolError>>;
}

/// A Plugin crate's label, exported as a constant such as
/// `plugin_web::PACKAGE`.
///
/// It is fixed data the Host reads before anything is built. Passing it to
/// the Harness's install is also what links the Plugin crate into the
/// Host.
#[derive(Clone, Copy)]
pub struct Package {
    /// The package name, `vendor/name` with two segments, such as
    /// `promptforge/web`. A Host installs the Plugin under the second
    /// segment unless it picks another name.
    pub name: &'static str,
    /// Lua run once for each prompt that declares the Plugin, as a chunk
    /// whose `...` is the name the Plugin was installed under; `None` when
    /// the Plugin has no prelude.
    pub prelude: Option<&'static str>,
    /// The per-run services the Plugin's calls read. A run without every
    /// one of them cannot use the Plugin.
    pub needs: &'static [ServiceId],
    /// Builds the one Plugin object every run shares. The Host calls it
    /// once, at install, with the name it chose, its JSON configuration
    /// for the Plugin, and the Host-wide services.
    ///
    /// A failure, such as a missing Host-wide service, leaves the Plugin
    /// unavailable, and its message appears in the refusal notice of each
    /// run whose prompt declares the Plugin.
    #[expect(
        clippy::type_complexity,
        reason = "spelled out so a Plugin author reads the signature construct implements in one place"
    )]
    pub construct: fn(
        name: &PluginId,
        config: serde_json::Value,
        services: &HostServices,
    ) -> Result<Arc<dyn Plugin>, ToolError>,
}

impl fmt::Debug for Package {
    /// Shows the name, whether a prelude exists, and the needs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Package")
            .field("name", &self.name)
            .field("prelude", &self.prelude.is_some())
            .field("needs", &self.needs)
            .finish_non_exhaustive()
    }
}
