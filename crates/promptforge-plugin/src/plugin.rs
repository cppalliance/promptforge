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
/// [`Plugin::call`] and [`Plugin::ready`] return one, so the Harness can
/// hold Plugins as `dyn Plugin`. An implementation wraps its body in
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
    /// The Harness reads the list once when each run starts, after it
    /// awaits [`ready`](Plugin::ready), so a Plugin whose tools change
    /// returns its current list, and a Plugin still coming up returns what
    /// it has so far. Each tool's id sits under the name the Plugin was
    /// installed under, as in `web/fetch`.
    fn tools(&self) -> Vec<ToolDescriptor>;

    /// Resolves once the Plugin's tool list is complete, or with the
    /// reason it never will be.
    ///
    /// The Harness awaits every Plugin's `ready` together before each run
    /// reads [`tools`](Plugin::tools). A Plugin whose `ready` fails is
    /// unavailable to that run, with the error's message as the reason.
    /// The default resolves `Ok(())` at once, for a Plugin whose tools are
    /// known when `construct` returns.
    ///
    /// # Invariants
    ///
    /// - `ready` resolves `Ok` once [`tools`](Plugin::tools) is complete,
    ///   or `Err` with a reason a model can read when it never will be.
    /// - `ready` resolves within a bound the Plugin owns. The Plugin's own
    ///   task enforces that bound, and `ready` uses no runtime timer,
    ///   because it may be polled outside any particular runtime.
    /// - `ready` is cancellation-safe: a cancel drops it unresolved. After
    ///   its first resolution it answers at once unless the Plugin's state
    ///   has changed.
    ///
    /// # Errors
    /// Returns a [`ToolError`], whose message is safe to show the model,
    /// when the Plugin will never offer its tools, such as when the
    /// backend it connects to refuses it.
    fn ready(&self) -> PluginFuture<'_, Result<(), ToolError>> {
        Box::pin(async { Ok(()) })
    }

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
/// Host. A Plugin crate builds it with [`Package::new`], adding
/// [`Package::prelude`] and [`Package::needs`] only where it has them:
///
/// ```text
/// pub const PACKAGE: Package = Package::new("acme/greeter", construct)
///     .prelude(PRELUDE)
///     .needs(NEEDS);
/// ```
#[derive(Clone, Copy)]
#[non_exhaustive]
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

impl Package {
    /// Builds the label for the package `name`, with no prelude and no
    /// needs.
    #[expect(
        clippy::type_complexity,
        reason = "spelled out so a Plugin author reads the signature construct implements in one place"
    )]
    #[must_use]
    pub const fn new(
        name: &'static str,
        construct: fn(
            name: &PluginId,
            config: serde_json::Value,
            services: &HostServices,
        ) -> Result<Arc<dyn Plugin>, ToolError>,
    ) -> Package {
        Package {
            name,
            prelude: None,
            needs: &[],
            construct,
        }
    }

    /// Sets the Lua run once for each prompt that declares the Plugin.
    #[must_use]
    pub const fn prelude(self, prelude: &'static str) -> Package {
        Package {
            prelude: Some(prelude),
            ..self
        }
    }

    /// Sets the per-run services the Plugin's calls read.
    #[must_use]
    pub const fn needs(self, needs: &'static [ServiceId]) -> Package {
        Package { needs, ..self }
    }
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
