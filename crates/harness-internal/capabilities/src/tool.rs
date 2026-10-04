//! The [`Tool`] trait: the implementation contract behind a
//! [`ToolDescriptor`] the Engine binds against.
//!
//! The Engine's catalog is descriptors
//! ([`ToolCatalog`](promptforge::tools::ToolCatalog)), and a
//! `ToolCall` effect names a [`ToolId`]; the Harness resolves the id in
//! its [`ToolTable`](crate::ToolTable) and calls the implementation here.
//! Some tools run locally in the Harness process (fetching and rendering a
//! web page), others proxy through the gateway so a shared credential never
//! leaves the server; both share this trait so the tool performer dispatches
//! them uniformly.

use promptforge::tools::{ToolDescriptor, ToolError, ToolId, ToolOutput};

#[cfg(test)]
#[path = "tool-tests.rs"]
mod tests;

/// A tool the Harness can dispatch during a model's tool-call loop.
///
/// # Implementing
///
/// An implementation supplies a stable identity, a wire name for the model
/// transport, a description for the model, a JSON Schema for its parameters,
/// and an async [`call`](Tool::call).
///
/// # Compatibility policy
///
/// This trait is a stable extension point that downstream crates may
/// implement. A **new required** method (one without a default body) would
/// break those implementations, so every new method ships with a default
/// implementation. Existing method signatures are stable.
///
/// # Invariants
///
/// - [`id`](Tool::id) returns the same value on every call for a given tool.
///   It is the catalog key and must be unique within a catalog. A catalog's
///   entries are the tools' [`ToolDescriptor`] values.
/// - [`wire_name`](Tool::wire_name) is the name on the model transport, not
///   the tool's identity. It is distinct from [`id`](Tool::id), and an alias
///   may replace it when the tool is advertised to a model.
/// - [`parameters_schema`](Tool::parameters_schema) returns a JSON Schema
///   `object` that describes the arguments [`call`](Tool::call) accepts.
/// - [`call`](Tool::call) is cancellation-aware and must not panic. If it
///   panics, the Harness answers its effect `Dropped` and logs the panic.
/// - [`call`](Tool::call) must mark the trust of every output correctly. Any
///   output that embeds data an attacker can influence is
///   [`ToolOutput::untrusted`].
/// - [`call`](Tool::call) must not block while polled. The Harness polls it
///   inside the run's own future, beside every other effect of the run. A
///   call that does blocking or CPU-heavy work hands that work to the Host's
///   own runtime.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// Returns the tool's stable identity.
    ///
    /// The identity is the tool's key in a catalog. It must be the same on
    /// every call and unique within any catalog that lists the tool.
    fn id(&self) -> ToolId;

    /// Returns the name the current model transport uses for the tool.
    ///
    /// This name is not the tool's identity. When the tool is advertised to a
    /// model, a prompt-local alias may replace it. It should be a non-empty
    /// token that is legal on the transport, with no `/` separator and no
    /// control characters.
    fn wire_name(&self) -> &str;

    /// Returns a one-sentence description of the tool for the model.
    fn description(&self) -> &str;

    /// Returns the JSON Schema for the tool's parameters.
    ///
    /// The schema is a JSON `object` (a map with `"type": "object"` and a
    /// `properties` map) that matches the arguments [`call`](Tool::call)
    /// accepts.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Returns whether [`call`](Tool::call) output is structured JSON rather
    /// than plain text.
    ///
    /// A structured tool's output text is one JSON value. An executor that
    /// supports structured results resumes the script with that value as
    /// data (for example, a Lua table) instead of a string. The default is
    /// `false`, meaning plain text.
    ///
    /// Structured output works only for trusted output. An untrusted result
    /// is nonce-wrapped before any parse, so the wrapped text no longer
    /// parses as JSON. The call then fails instead of letting data an
    /// attacker can influence bypass the wrapping.
    fn structured_output(&self) -> bool {
        false
    }

    /// Returns the tool's descriptor, built from its other methods.
    ///
    /// The Harness calls this when it assembles a run's catalog. The returned
    /// descriptor records no conflicts. The Harness adds the contributing
    /// capability's conflicts during assembly.
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor::new(
            self.id(),
            self.wire_name(),
            self.description(),
            self.parameters_schema(),
        )
        .structured(self.structured_output())
    }

    /// Executes the tool with the given JSON arguments and returns its output.
    ///
    /// The returned [`ToolOutput`] carries its own
    /// [`OutputTrust`](promptforge::tools::OutputTrust), so an implementation
    /// cannot forget to set trust. An
    /// [`OutputTrust::Untrusted`](promptforge::tools::OutputTrust::Untrusted)
    /// result is nonce-wrapped before it can reach model input. A failure
    /// returns a narrow [`ToolError`] whose message is safe to show the model.
    /// Implementations must not panic and should return promptly when the run
    /// is cancelled.
    ///
    /// # Errors
    /// Returns a [`ToolError`] if the arguments are unacceptable, the backend
    /// refuses, the transport fails, or the run is cancelled.
    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError>;
}
