//! Capability activation: the harness-side step that turns a prompt's
//! declared capabilities into the run's [`ToolCatalog`] and the
//! implementations behind it.
//!
//! Before a run is prepared, the host resolves the prompt's declarations
//! against its [`CapabilityRegistry`], checks the present capabilities for
//! co-activation conflicts, activates each survivor with the run's
//! [`RunServices`], and assembles the contributions into two things: the
//! [`ToolCatalog`] of descriptors [`Environment::prepare`] fills slots
//! against, and the [`ToolTable`] of implementations the host's tool
//! performer resolves a `ToolCall` effect's id in. The engine sees only the
//! first.
//!
//! [`Environment::prepare`]: promptforge::Environment::prepare

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use promptforge::Prompt;
use promptforge::capabilities::CapabilityId;
use promptforge::tools::{ToolCatalog, ToolDescriptor, ToolId};
use promptforge::{CapabilityConflict, Requirements};

use crate::capability::{Capability, Contribution, RunServices};
use crate::registry::CapabilityRegistry;
use crate::tool::Tool;

/// The implementations behind a run's catalog, keyed by stable identity.
///
/// Held by the host, never by the engine: a `ToolCall` effect names a
/// [`ToolId`], and the host's performer resolves it here.
#[derive(Clone, Default)]
pub struct ToolTable {
    tools: BTreeMap<ToolId, Arc<dyn Tool>>,
}

impl ToolTable {
    /// Builds an empty table.
    #[must_use]
    pub fn new() -> ToolTable {
        ToolTable::default()
    }

    /// Adds `tool` under its own identity; a repeated identity keeps the
    /// first implementation.
    pub fn insert(&mut self, tool: Arc<dyn Tool>) {
        self.tools.entry(tool.id()).or_insert(tool);
    }

    /// Returns the implementation registered under `id`.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<Arc<dyn Tool>> {
        self.tools.get(id).map(Arc::clone)
    }

    /// Returns whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl fmt::Debug for ToolTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolTable")
            .field("ids", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// What activating a prompt's declared capabilities produced.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Activation {
    /// The activated capabilities' contributed tools as descriptors, in
    /// declaration order: what the host hands to
    /// [`Environment::tools`](promptforge::Environment::tools).
    pub catalog: ToolCatalog,
    /// The implementations behind the catalog: what the host's tool
    /// performer resolves against.
    pub tools: ToolTable,
    /// What activation could not satisfy: the required capabilities that
    /// are absent or failed to activate, and the co-activation conflicts.
    /// Merged into the prepare report through
    /// [`Requirements::merge`] so one refusal names every gap.
    pub requirements: Requirements,
}

/// Resolves and activates the capabilities `prompt` declares against
/// `registry`, assembling the run's catalog and implementation table.
///
/// Declared capabilities resolve against the registry in declaration order.
/// A missing required capability lands in
/// [`Requirements::missing_required`]; an absent optional capability is
/// skipped with a log line. Present capabilities are checked for
/// co-activation conflicts (bashkit vs terminal: two filesystem realities,
/// and a context gets one or the other, never both); a conflicting pair
/// activates neither member and lands in [`Requirements::conflicts`]
/// naming both. Each remaining capability is activated with `services`
/// (the run's VFS and cancellation handle); an activation failure is logged
/// and the capability contributes nothing - and when the failed capability
/// is required, it also lands in [`Requirements::missing_required`], since
/// the run cannot have what the prompt declared.
///
/// The activated contributions are assembled into the catalog in
/// declaration order, with tool prefix-containment enforced at assembly: a
/// contributed tool whose id escapes its capability's id, repeats an
/// earlier contribution, or has a transport-illegal wire name is
/// rejected - logged and never admitted. Every admitted descriptor
/// includes its capability's declared conflicts for the record.
///
/// # Panics
/// Panics only if `prompt` declares a capability id that is not a valid
/// 2-segment id, which the parser refuses before a [`Prompt`] exists.
#[must_use]
pub fn activate(
    registry: Option<&CapabilityRegistry>,
    prompt: &Prompt,
    services: &RunServices,
) -> Activation {
    let mut requirements = Requirements::default();
    // Resolve the declarations against the registry, preserving
    // declaration order.
    let mut present: Vec<(CapabilityId, Arc<dyn Capability>, bool)> = Vec::new();
    for declaration in prompt.frontmatter().capabilities() {
        #[expect(
            clippy::expect_used,
            reason = "the parser validated the declared id's arity and charset at parse time, so a parse failure here is a defect, not a prompt error"
        )]
        let id = CapabilityId::parse(&declaration.id().to_string())
            .expect("a parsed capability declaration names a valid capability id");
        let capability = registry.and_then(|registry| registry.get(&id));
        let Some(capability) = capability else {
            if declaration.is_optional() {
                tracing::info!(capability = %id, "optional capability absent; skipped");
            } else {
                requirements.missing_required.push(id);
            }
            continue;
        };
        present.push((id, Arc::clone(capability), declaration.is_optional()));
    }
    // Co-activation conflicts are declared by the capabilities themselves;
    // the check is symmetric, so only one member of a pair needs to name
    // the other. A conflicting pair activates neither member and fails
    // preparation naming both.
    let mut conflicted = vec![false; present.len()];
    for (i, (first_id, first, _)) in present.iter().enumerate() {
        for (j, (second_id, second, _)) in present.iter().enumerate().skip(i + 1) {
            if first.conflicts().contains(second_id) || second.conflicts().contains(first_id) {
                tracing::warn!(
                    first = %first_id,
                    second = %second_id,
                    "conflicting capabilities declared; neither activates"
                );
                requirements
                    .conflicts
                    .push(CapabilityConflict::new(first_id.clone(), second_id.clone()));
                conflicted[i] = true;
                conflicted[j] = true;
            }
        }
    }
    let mut activated: Vec<(CapabilityId, Vec<CapabilityId>, Contribution)> = Vec::new();
    for ((id, capability, optional), is_conflicted) in
        present.iter().zip(conflicted.iter().copied())
    {
        if is_conflicted {
            continue;
        }
        match capability.create(services) {
            Ok(contribution) => {
                tracing::info!(capability = %id, "capability activated");
                activated.push((id.clone(), capability.conflicts().to_vec(), contribution));
            }
            Err(error) => {
                tracing::warn!(
                    capability = %id,
                    %error,
                    "capability activation failed; it contributes nothing to the run"
                );
                // A required capability that cannot activate leaves the run
                // without something the prompt declared: report it like an
                // absent one so the run fails until satisfied.
                if !*optional {
                    requirements.missing_required.push(id.clone());
                }
            }
        }
    }
    let (catalog, tools) = assemble(&activated);
    Activation {
        catalog,
        tools,
        requirements,
    }
}

/// Assembles the run's catalog and implementation table from the activated
/// capabilities' contributions in declaration order.
///
/// Containment is total and enforced here: every contributed tool's id must
/// sit under its contributing capability's full id (`namespace/pack/name`
/// for a `namespace/pack` capability). A violating tool - like a repeated
/// id or a transport-illegal wire name - is rejected at assembly: logged
/// and never admitted.
fn assemble(
    activated: &[(CapabilityId, Vec<CapabilityId>, Contribution)],
) -> (ToolCatalog, ToolTable) {
    let mut descriptors: Vec<ToolDescriptor> = Vec::new();
    let mut table = ToolTable::new();
    let mut seen = std::collections::BTreeSet::new();
    for (capability, conflicts, contribution) in activated {
        for tool in &contribution.tools {
            let id = tool.id();
            if !capability.contains(&id) {
                tracing::warn!(
                    capability = %capability,
                    tool = %id,
                    "contributed tool id escapes its capability's id; rejected at assembly"
                );
                continue;
            }
            if !seen.insert(id.clone()) {
                tracing::warn!(
                    capability = %capability,
                    tool = %id,
                    "contributed tool id repeats an earlier contribution; rejected at assembly"
                );
                continue;
            }
            let descriptor: ToolDescriptor = tool.descriptor().with_conflicts(conflicts.clone());
            // The catalog is the transport boundary: validate the wire name
            // per tool so one bad tool costs only itself.
            if let Err(error) = ToolCatalog::new(std::slice::from_ref(&descriptor)) {
                tracing::warn!(
                    capability = %capability,
                    tool = %id,
                    %error,
                    "contributed tool failed catalog validation; rejected at assembly"
                );
                continue;
            }
            descriptors.push(descriptor);
            table.insert(Arc::clone(tool));
        }
    }
    let catalog = match ToolCatalog::new(&descriptors) {
        Ok(catalog) => catalog,
        Err(error) => {
            // Every accepted descriptor passed containment, uniqueness, and
            // wire-name validation above, so this build cannot fail; the
            // arm is defensive.
            tracing::warn!(%error, "catalog assembly failed after per-tool validation");
            ToolCatalog::default()
        }
    };
    (catalog, table)
}
