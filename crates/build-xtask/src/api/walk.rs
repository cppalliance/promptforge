//! Enumerates every item the surface exposes: each re-exported item, its
//! fields, variants, inherent associated items, trait items, and the trait
//! impls rustdoc shows on its page, with their associated items.
//!
//! A type's impls come from its own `impls` list (which carries the auto
//! trait and blanket impls rustdoc synthesizes) and from every internal
//! crate's impl blocks whose self type is that item, because an impl in a
//! downstream internal crate shows on the facade's page too; a trait's
//! come from its `implementations` and every impl of it the same way.
//! Blanket impls of traits outside the internal crates (`From<T> for T`,
//! `Borrow<T>`, and every dependency's) say nothing about the facade and
//! are skipped.

use std::collections::{HashMap, HashSet};

use rustdoc_types::{Crate, Id, Item, ItemEnum, StructKind, Type, VariantKind};

use super::items::{Surface, target};
use super::load::Loaded;
use super::render::Renderer;

/// Where a visited item sits on the surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// A re-exported item.
    Item,
    /// A struct, union, or variant field.
    Field,
    /// An enum variant.
    Variant,
    /// An associated item of an inherent impl.
    Member,
    /// An associated item of a surface trait.
    TraitItem,
    /// An impl block, inherent or trait.
    Impl,
    /// An associated item of a trait impl.
    ImplItem,
}

/// One visited item.
#[derive(Debug)]
pub(crate) struct Visit<'a> {
    /// The label of what holds it: the owning type's facade path, or for
    /// a trait impl's associated item, the impl's header.
    pub(crate) parent: String,
    /// How findings and the listing name it.
    pub(crate) label: String,
    pub(crate) role: Role,
    pub(crate) item: &'a Item,
    /// The JSON the item's ids resolve in, and that crate's name.
    pub(crate) krate: &'a Crate,
    pub(crate) crate_name: &'a str,
}

type Key = (String, Vec<String>);
type Located<'a> = (&'a str, &'a Crate, Id);

/// Every item the surface exposes, each impl once.
pub(crate) fn visits<'a>(loaded: &'a Loaded, surface: &'a Surface) -> Vec<Visit<'a>> {
    let mut walker = Walker {
        loaded,
        surface,
        by_type: HashMap::new(),
        by_trait: HashMap::new(),
        seen: HashSet::new(),
        visits: Vec::new(),
    };
    walker.index_impls();
    for entry in &surface.entries {
        // `Surface::resolve` records an entry only for a crate in
        // `loaded.internal` and an id in that crate's index, refusing any
        // other re-export, so neither arm below skips an entry.
        let Some((crate_name, krate)) = loaded.internal.get_key_value(&entry.krate) else {
            continue;
        };
        let Some(item) = krate.index.get(&entry.id) else {
            continue;
        };
        walker.entry(crate_name, krate, item, &entry.label);
    }
    walker.visits
}

struct Walker<'a> {
    loaded: &'a Loaded,
    surface: &'a Surface,
    by_type: HashMap<Key, Vec<Located<'a>>>,
    by_trait: HashMap<Key, Vec<Located<'a>>>,
    seen: HashSet<(&'a str, Id)>,
    visits: Vec<Visit<'a>>,
}

impl<'a> Walker<'a> {
    /// Indexes every written impl block in every internal crate by the
    /// item its self type names and by its trait.
    fn index_impls(&mut self) {
        for (name, krate) in &self.loaded.internal {
            for (id, item) in &krate.index {
                let ItemEnum::Impl(block) = &item.inner else {
                    continue;
                };
                if block.is_synthetic || block.blanket_impl.is_some() {
                    continue;
                }
                let located = (name.as_str(), krate, *id);
                if let Type::ResolvedPath(path) = &block.for_
                    && let Some(key) = key(krate, path.id)
                {
                    self.by_type.entry(key).or_default().push(located);
                }
                if let Some(key) = block.trait_.as_ref().and_then(|path| key(krate, path.id)) {
                    self.by_trait.entry(key).or_default().push(located);
                }
            }
        }
    }

    fn push(
        &mut self,
        parent: &str,
        label: String,
        role: Role,
        item: &'a Item,
        at: (&'a str, &'a Crate),
    ) {
        self.visits.push(Visit {
            parent: parent.to_owned(),
            label,
            role,
            item,
            krate: at.1,
            crate_name: at.0,
        });
    }

    fn entry(&mut self, crate_name: &'a str, krate: &'a Crate, item: &'a Item, label: &str) {
        let at = (crate_name, krate);
        self.push(label, label.to_owned(), Role::Item, item, at);
        let own = key(krate, item.id);
        match &item.inner {
            ItemEnum::Struct(structure) => {
                match &structure.kind {
                    StructKind::Plain { fields, .. } => {
                        self.fields(fields.iter().copied(), label, at);
                    }
                    StructKind::Tuple(fields) => {
                        self.fields(fields.iter().flatten().copied(), label, at);
                    }
                    StructKind::Unit => {}
                }
                self.impls(&structure.impls, own.as_ref(), label, at);
            }
            ItemEnum::Union(union) => {
                self.fields(union.fields.iter().copied(), label, at);
                self.impls(&union.impls, own.as_ref(), label, at);
            }
            ItemEnum::Enum(enumeration) => {
                for id in &enumeration.variants {
                    self.variant(*id, label, at);
                }
                self.impls(&enumeration.impls, own.as_ref(), label, at);
            }
            ItemEnum::Trait(definition) => {
                for id in &definition.items {
                    if let Some(member) = krate.index.get(id) {
                        let name = member.name.as_deref().unwrap_or("_");
                        self.push(
                            label,
                            format!("{label}::{name}"),
                            Role::TraitItem,
                            member,
                            at,
                        );
                    }
                }
                let written = own.and_then(|own| self.by_trait.get(&own).cloned());
                for &id in &definition.implementations {
                    self.impl_block((crate_name, krate, id), label);
                }
                for located in written.into_iter().flatten() {
                    self.impl_block(located, label);
                }
            }
            _ => {}
        }
    }

    fn fields(&mut self, ids: impl Iterator<Item = Id>, owner: &str, at: (&'a str, &'a Crate)) {
        for id in ids {
            if let Some(field) = at.1.index.get(&id) {
                let name = field.name.as_deref().unwrap_or("_");
                self.push(owner, format!("{owner}::{name}"), Role::Field, field, at);
            }
        }
    }

    fn variant(&mut self, id: Id, owner: &str, at: (&'a str, &'a Crate)) {
        let Some(variant) = at.1.index.get(&id) else {
            return;
        };
        let label = format!("{owner}::{}", variant.name.as_deref().unwrap_or("_"));
        self.push(owner, label.clone(), Role::Variant, variant, at);
        if let ItemEnum::Variant(inner) = &variant.inner {
            match &inner.kind {
                VariantKind::Plain => {}
                VariantKind::Tuple(fields) => {
                    self.fields(fields.iter().flatten().copied(), &label, at);
                }
                VariantKind::Struct { fields, .. } => {
                    self.fields(fields.iter().copied(), &label, at);
                }
            }
        }
    }

    fn impls(&mut self, own: &[Id], key: Option<&Key>, owner: &str, at: (&'a str, &'a Crate)) {
        let written = key.and_then(|key| self.by_type.get(key).cloned());
        for &id in own {
            self.impl_block((at.0, at.1, id), owner);
        }
        for located in written.into_iter().flatten() {
            self.impl_block(located, owner);
        }
    }

    fn impl_block(&mut self, (crate_name, krate, id): Located<'a>, owner: &str) {
        let Some(item) = krate.index.get(&id) else {
            return;
        };
        let ItemEnum::Impl(block) = &item.inner else {
            return;
        };
        if !self.seen.insert((crate_name, id)) {
            return;
        }
        if block.blanket_impl.is_some() {
            let internal = block
                .trait_
                .as_ref()
                .and_then(|path| target(krate, path.id))
                .is_some_and(|trait_| self.surface.internal.contains(trait_.krate));
            if !internal {
                return;
            }
        }
        let at = (crate_name, krate);
        let header = Renderer::new(krate, self.surface).impl_header(block);
        self.push(owner, header.clone(), Role::Impl, item, at);
        for child in &block.items {
            let Some(member) = krate.index.get(child) else {
                continue;
            };
            let name = member.name.as_deref().unwrap_or("_");
            if block.trait_.is_none() {
                self.push(owner, format!("{owner}::{name}"), Role::Member, member, at);
            } else {
                let label = format!("{header} {{ {} {name} }}", keyword(&member.inner));
                self.push(&header, label, Role::ImplItem, member, at);
            }
        }
    }
}

/// The keyword an associated item is declared with.
fn keyword(inner: &ItemEnum) -> &'static str {
    match inner {
        ItemEnum::Function(_) => "fn",
        ItemEnum::AssocConst { .. } => "const",
        _ => "type",
    }
}

/// The cross-crate key of the item `id` names in `krate`'s JSON.
fn key(krate: &Crate, id: Id) -> Option<Key> {
    let target = target(krate, id)?;
    Some((target.krate.to_owned(), target.path.to_vec()))
}
