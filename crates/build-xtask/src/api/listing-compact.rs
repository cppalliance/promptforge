//! Where the listing places each trait impl: left out, on its type's
//! shared `auto` or `derives` line, or on a line of its own.

use std::collections::{BTreeMap, BTreeSet};

use rustdoc_types::{
    Crate, GenericArg, GenericArgs, GenericParamDefKind, Generics, Impl, WherePredicate,
};

use super::super::items::target;

/// The `core` markers whose impls the listing leaves out: no host can
/// depend on them, and the nightly-only `TrivialClone` and `UnsafeUnpin`
/// would churn whenever the pinned nightly moves.
const OMITTED: [&str; 3] = ["StructuralPartialEq", "TrivialClone", "UnsafeUnpin"];

/// The `core` auto traits a type's `auto` line names.
const AUTO: [&str; 6] = [
    "Freeze",
    "RefUnwindSafe",
    "Send",
    "Sync",
    "Unpin",
    "UnwindSafe",
];

/// The `core` traits a type's `derives` line names.
const CORE_DERIVES: [&str; 9] = [
    "Clone",
    "Copy",
    "Debug",
    "Default",
    "Eq",
    "Hash",
    "Ord",
    "PartialEq",
    "PartialOrd",
];

/// The serde traits a type's `derives` line names. `serde` defines them in
/// `serde_core` and re-exports them, and rustdoc names the defining crate.
const SERDE_DERIVES: [&str; 2] = ["Deserialize", "Serialize"];

/// Where the listing shows one visited item.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Placement<'a> {
    /// Nowhere.
    Omitted,
    /// On its type's `auto` line, with `!` when the type `lacks` it.
    Auto { name: &'a str, lacks: bool },
    /// On its type's `derives` line.
    Derives(&'a str),
    /// On a line of its own.
    Own,
}

/// The defining crate and name of the trait `block` implements, or `None`
/// for an inherent impl or a trait rustdoc could not resolve.
pub(super) fn trait_name<'a>(krate: &'a Crate, block: &Impl) -> Option<(&'a str, &'a str)> {
    let found = target(krate, block.trait_.as_ref()?.id)?;
    Some((found.krate, found.path.last()?.as_str()))
}

/// Where the listing shows `block`, an impl of the trait `trait_` names
/// by its defining crate and name.
pub(super) fn placement<'a>(trait_: Option<(&str, &'a str)>, block: &Impl) -> Placement<'a> {
    let Some((krate, name)) = trait_ else {
        return Placement::Own;
    };
    let core = krate == "core";
    if core && OMITTED.contains(&name) {
        return Placement::Omitted;
    }
    let trait_args = block.trait_.as_ref().and_then(|path| path.args.as_deref());
    if bounds_a_type_parameter(&block.generics) || passes_a_type(trait_args) {
        return Placement::Own;
    }
    if core && AUTO.contains(&name) {
        return Placement::Auto {
            name,
            lacks: block.is_negative,
        };
    }
    let derive = (core && CORE_DERIVES.contains(&name))
        || (matches!(krate, "serde" | "serde_core") && SERDE_DERIVES.contains(&name));
    if derive && !block.is_negative {
        Placement::Derives(name)
    } else {
        Placement::Own
    }
}

/// Whether `generics` bound a type parameter, inline or in a `where`
/// clause, so the impl holds only for some of the type's arguments.
fn bounds_a_type_parameter(generics: &Generics) -> bool {
    let inline = generics.params.iter().any(|param| {
        matches!(&param.kind, GenericParamDefKind::Type { bounds, .. } if !bounds.is_empty())
    });
    inline
        || generics
            .where_predicates
            .iter()
            .any(|predicate| !matches!(predicate, WherePredicate::LifetimePredicate { .. }))
}

/// Whether a trait's `args` pass it anything but lifetimes, as
/// `PartialEq<str>` does and `Deserialize<'de>` does not: such an impl
/// relates the type to another, which a derive never does.
fn passes_a_type(args: Option<&GenericArgs>) -> bool {
    match args {
        None => false,
        Some(GenericArgs::AngleBracketed { args, constraints }) => {
            !constraints.is_empty()
                || args
                    .iter()
                    .any(|arg| !matches!(arg, GenericArg::Lifetime(_)))
        }
        Some(_) => true,
    }
}

/// The impls that share their type's `auto` and `derives` lines, keyed by
/// the type's facade path.
#[derive(Debug, Default)]
pub(super) struct Shared<'a> {
    /// Each type's auto traits, each with whether the type lacks it.
    pub(super) auto: BTreeMap<&'a str, BTreeMap<&'a str, bool>>,
    pub(super) derives: BTreeMap<&'a str, BTreeSet<&'a str>>,
}

impl Shared<'_> {
    /// One `auto` line and one `derives` line per type that has any, each
    /// naming its traits in alphabetical order.
    pub(super) fn lines(&self) -> Vec<String> {
        let auto = self.auto.iter().map(|(owner, traits)| {
            let traits: Vec<String> = traits
                .iter()
                .map(|(name, lacks)| format!("{}{name}", if *lacks { "!" } else { "" }))
                .collect();
            format!("auto {owner}: {}", traits.join(", "))
        });
        let derives = self.derives.iter().map(|(owner, traits)| {
            let traits: Vec<&str> = traits.iter().copied().collect();
            format!("derives {owner}: {}", traits.join(", "))
        });
        auto.chain(derives).collect()
    }
}

#[cfg(test)]
#[path = "listing-compact-tests.rs"]
mod tests;
