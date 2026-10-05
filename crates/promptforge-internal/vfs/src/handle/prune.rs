//! The ledger's prune: dropping the claims of ended scopes, collapsing
//! the epochs every live identity has ordered, and resetting the
//! threshold.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::claims::{ClaimsTables, Epoch, PRUNE_AT, RegionClaims};
use super::scope::{ScopeId, View};
use crate::path::VfsPath;
use crate::traits::ExecId;

/// Prunes the tables: drops every claim of an ended or dead scope,
/// collapses - outside the pattern regions, which accumulate for the
/// life of a run - each live scope's epochs that happen before every one
/// of its live identities, and removes every region left empty. The next
/// prune waits for the tables to double what survived. Runs under the
/// tables lock.
pub(super) fn prune(tables: &mut ClaimsTables) {
    prune_dead_scopes(tables);
    prune_ordered_epochs(tables);
    let occupied = |_: &VfsPath, region: &mut RegionClaims| {
        region.write.is_some() || !region.reads.is_empty() || !region.created.is_empty()
    };
    tables.paths.retain(occupied);
    tables.children.retain(occupied);
    tables.patterns.retain(occupied);
    tables.subtrees.retain(occupied);
    tables.ancestors.retain(occupied);
    tables.entries = tables.paths.len()
        + tables.children.len()
        + tables.patterns.len()
        + tables.subtrees.len()
        + tables.ancestors.len();
    tables.prune_at = PRUNE_AT.max(2 * tables.entries);
}

/// Drops every claim of an ended or dead scope and the registry entries
/// for those scopes: an ended scope's claims are ignored anyway, so they
/// can go at once.
fn prune_dead_scopes(tables: &mut ClaimsTables) {
    let mut dead: HashSet<ScopeId> = HashSet::new();
    for (&scope_id, weak) in &tables.scopes {
        if weak.upgrade().is_none_or(|scope| scope.ended()) {
            dead.insert(scope_id);
        }
    }
    if dead.is_empty() {
        return;
    }
    let retain = |region: &mut RegionClaims| {
        region.write = region
            .write
            .filter(|(scope_id, _)| !dead.contains(scope_id));
        region
            .reads
            .retain(|_, (scope_id, _)| !dead.contains(scope_id));
        region
            .created
            .retain(|_, (scope_id, _)| !dead.contains(scope_id));
    };
    for region in tables.paths.values_mut() {
        retain(region);
    }
    for region in tables.children.values_mut() {
        retain(region);
    }
    for region in tables.patterns.values_mut() {
        retain(region);
    }
    for region in tables.subtrees.values_mut() {
        retain(region);
    }
    for region in tables.ancestors.values_mut() {
        retain(region);
    }
    tables.scopes.retain(|scope_id, _| !dead.contains(scope_id));
}

/// Collapses, per region, each live scope's epochs that happen before
/// every one of its live identities into one clock-0 epoch. No identity
/// of the scope, present or forked later, can race with a clock-0 epoch,
/// but every other live scope still conflicts with it, including one
/// that registers after the prune, so the region's claim is never lost
/// while its scope lives. An identity whose tool call has ended claims
/// nothing again, so it is not waited for. Pattern regions are exempt:
/// they accumulate for the life of a run.
fn prune_ordered_epochs(tables: &mut ClaimsTables) {
    // One view per live scope of its live identities' clocks, then
    // each epoch checked against it.
    let mut live_views: HashMap<ScopeId, Vec<View>> = HashMap::new();
    for (&scope_id, weak) in &tables.scopes {
        let Some(scope) = weak.upgrade() else {
            continue;
        };
        if scope.ended() {
            continue;
        }
        let inner = scope.lock();
        let views: Vec<View> = inner
            .identities
            .iter()
            .filter(|(_, identity)| identity.refs > 0 && !identity.ended)
            .map(|(&id, identity)| View {
                id,
                own: identity.own,
                seen: Arc::clone(&identity.seen),
                forked: identity.forked.clone(),
            })
            .collect();
        if !views.is_empty() {
            live_views.insert(scope_id, views);
        }
    }
    let ordered_before_everyone = |scope_id: ScopeId, epoch: Epoch| {
        live_views.get(&scope_id).is_some_and(|views| {
            views
                .iter()
                .all(|view| epoch.clock <= view.seen_clock(epoch.id))
        })
    };
    let prune_region = |region: &mut RegionClaims| {
        if let Some((scope_id, epoch)) = &mut region.write
            && ordered_before_everyone(*scope_id, *epoch)
        {
            epoch.clock = 0;
        }
        collapse_ordered(&mut region.reads, &ordered_before_everyone);
        collapse_ordered(&mut region.created, &ordered_before_everyone);
    };
    for region in tables.paths.values_mut() {
        prune_region(region);
    }
    for region in tables.children.values_mut() {
        prune_region(region);
    }
    for region in tables.subtrees.values_mut() {
        prune_region(region);
    }
    for region in tables.ancestors.values_mut() {
        prune_region(region);
    }
}

/// Replaces each scope's `ordered` epochs in one per-identity epoch map
/// with a single clock-0 epoch under the identity with the highest
/// clock among them, which the conflict message names. Clocks are
/// per-identity counters, so that identity is not necessarily the most
/// recent.
fn collapse_ordered(
    epochs: &mut HashMap<ExecId, (ScopeId, u64)>,
    ordered: &impl Fn(ScopeId, Epoch) -> bool,
) {
    let mut kept: HashMap<ScopeId, Epoch> = HashMap::new();
    epochs.retain(|&id, &mut (scope_id, clock)| {
        let epoch = Epoch { id, clock };
        if !ordered(scope_id, epoch) {
            return true;
        }
        let latest = kept.entry(scope_id).or_insert(epoch);
        if epoch.clock > latest.clock {
            *latest = epoch;
        }
        false
    });
    for (scope_id, epoch) in kept {
        epochs.insert(epoch.id, (scope_id, 0));
    }
}
