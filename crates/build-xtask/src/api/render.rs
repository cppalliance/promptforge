//! Renders visited items as one listing line each, and records every path
//! the rendering names as a mention, so the closure check sees exactly
//! what the listing shows: signatures, fields, generic bounds,
//! supertraits, both sides of every trait impl, and associated types.
//!
//! A path the facade re-exports renders as its facade path; any other
//! renders as its definition path (`alloc::string::String`), which keeps
//! the listing independent of how the source happened to spell it.

use rustdoc_types::{
    Attribute, Crate, Function, Generics, Impl, Item, ItemEnum, StructKind, VariantKind,
};

use super::items::Surface;
use super::walk::{Role, Visit};
use types::header;

/// One path a rendering named.
#[derive(Debug)]
pub(crate) struct Mention {
    /// Where on the item it appeared (`signature`, `bound`, ...).
    pub(crate) context: &'static str,
    /// The defining crate and definition path, when rustdoc resolved it.
    pub(crate) target: Option<(String, Vec<String>)>,
    /// The path as the source wrote it.
    pub(crate) written: String,
}

/// Renders items whose ids resolve in one crate's JSON.
#[derive(Debug)]
pub(crate) struct Renderer<'a> {
    krate: &'a Crate,
    surface: &'a Surface,
    context: &'static str,
    pub(crate) mentions: Vec<Mention>,
}

impl<'a> Renderer<'a> {
    pub(crate) fn new(krate: &'a Crate, surface: &'a Surface) -> Self {
        Renderer {
            krate,
            surface,
            context: "signature",
            mentions: Vec::new(),
        }
    }

    /// The listing line for `visit`, or `None` for an item the listing
    /// leaves to its impl line (trait impl methods, and inherent impl
    /// headers without generics). Mentions are recorded either way.
    pub(crate) fn line(&mut self, visit: &Visit<'_>) -> Option<String> {
        let label = visit.label.as_str();
        match (visit.role, &visit.item.inner) {
            (Role::Impl | Role::ImplItem, _) => self.impl_line(visit),
            (role, ItemEnum::Function(function)) => {
                let line = self.function(label, function);
                let provided = role == Role::TraitItem && function.has_body;
                Some(if provided {
                    format!("{line} {{ .. }}")
                } else {
                    line
                })
            }
            (_, ItemEnum::Struct(structure)) => Some(self.data(
                "struct",
                visit.item,
                label,
                &structure.generics,
                struct_shape(&structure.kind),
            )),
            (_, ItemEnum::Union(union)) => Some(self.data(
                "union",
                visit.item,
                label,
                &union.generics,
                braced(union.has_stripped_fields),
            )),
            (_, ItemEnum::Enum(enumeration)) => {
                Some(self.data("enum", visit.item, label, &enumeration.generics, ""))
            }
            (_, ItemEnum::StructField(ty)) => {
                self.context = "field";
                Some(format!("pub {label}: {}", self.ty(ty)))
            }
            (_, ItemEnum::Variant(variant)) => {
                let exhaustiveness = non_exhaustive(visit.item);
                let shape = variant_shape(&variant.kind);
                Some(match &variant.discriminant {
                    Some(discriminant) => {
                        format!("{exhaustiveness}pub {label} = {}{shape}", discriminant.expr)
                    }
                    None => format!("{exhaustiveness}pub {label}{shape}"),
                })
            }
            (_, ItemEnum::Trait(definition)) => {
                let generics = self.generics(&definition.generics);
                self.context = "supertrait";
                let supertraits = self.colon_bounds(&definition.bounds);
                let where_clause = self.where_clause(&definition.generics);
                let unsafety = if definition.is_unsafe { "unsafe " } else { "" };
                let auto = if definition.is_auto { "auto " } else { "" };
                Some(format!(
                    "pub {unsafety}{auto}trait {label}{generics}{supertraits}{where_clause}"
                ))
            }
            (
                _,
                ItemEnum::AssocType {
                    generics,
                    bounds,
                    type_,
                    ..
                },
            ) => {
                let params = self.generics(generics);
                self.context = "associated type";
                let bounds = self.colon_bounds(bounds);
                let value = self.value(type_.as_ref());
                let where_clause = self.where_clause(generics);
                Some(format!(
                    "pub type {label}{params}{bounds}{value}{where_clause}"
                ))
            }
            (_, ItemEnum::AssocConst { type_, .. } | ItemEnum::Constant { type_, .. }) => {
                self.context = "type";
                Some(format!("pub const {label}: {}", self.ty(type_)))
            }
            (_, ItemEnum::Static(item)) => {
                self.context = "type";
                let mutability = if item.is_mutable { "mut " } else { "" };
                Some(format!(
                    "pub static {mutability}{label}: {}",
                    self.ty(&item.type_)
                ))
            }
            (_, ItemEnum::TypeAlias(alias)) => {
                let generics = self.generics(&alias.generics);
                self.context = "aliased type";
                let ty = self.ty(&alias.type_);
                let where_clause = self.where_clause(&alias.generics);
                Some(format!("pub type {label}{generics} = {ty}{where_clause}"))
            }
            (_, ItemEnum::Macro(_) | ItemEnum::ProcMacro(_)) => Some(format!("pub macro {label}!")),
            (_, other) => Some(format!("pub {:?} {label}", other.item_kind())),
        }
    }

    /// The line for an impl block or a trait impl's associated item.
    fn impl_line(&mut self, visit: &Visit<'_>) -> Option<String> {
        let name = visit.item.name.as_deref().unwrap_or("_");
        match &visit.item.inner {
            ItemEnum::Impl(block) => {
                let header = self.impl_header(block);
                let generic = !block.generics.params.is_empty()
                    || !block.generics.where_predicates.is_empty();
                (block.trait_.is_some() || generic).then_some(header)
            }
            ItemEnum::Function(function) => {
                self.function(&visit.label, function);
                None
            }
            ItemEnum::AssocType { type_, .. } => {
                self.context = "associated type";
                let value = self.value(type_.as_ref());
                Some(format!("{} {{ type {name}{value} }}", visit.parent))
            }
            ItemEnum::AssocConst { type_, .. } => {
                self.context = "associated const";
                let ty = self.ty(type_);
                Some(format!("{} {{ const {name}: {ty} }}", visit.parent))
            }
            _ => None,
        }
    }

    /// `impl<..> Trait<..> for Type where ..`, or `impl<..> Type` for an
    /// inherent impl.
    pub(crate) fn impl_header(&mut self, block: &Impl) -> String {
        let generics = self.generics(&block.generics);
        let where_clause = self.where_clause(&block.generics);
        let unsafety = if block.is_unsafe { "unsafe " } else { "" };
        let trait_part = match &block.trait_ {
            Some(path) => {
                self.context = "implemented trait";
                let negative = if block.is_negative { "!" } else { "" };
                format!("{negative}{} for ", self.path(path))
            }
            None => String::new(),
        };
        self.context = "impl self type";
        let self_type = self.ty(&block.for_);
        format!("{unsafety}impl{generics} {trait_part}{self_type}{where_clause}")
    }

    fn data(
        &mut self,
        keyword: &str,
        item: &Item,
        label: &str,
        generics: &Generics,
        shape: &str,
    ) -> String {
        let params = self.generics(generics);
        let where_clause = self.where_clause(generics);
        let exhaustiveness = non_exhaustive(item);
        format!("{exhaustiveness}pub {keyword} {label}{params}{where_clause}{shape}")
    }

    fn function(&mut self, label: &str, function: &Function) -> String {
        let generics = self.generics(&function.generics);
        let where_clause = self.where_clause(&function.generics);
        self.context = "signature";
        let signature = self.signature(&function.sig, true);
        let header = header(&function.header);
        format!("pub {header}fn {label}{generics}{signature}{where_clause}")
    }
}

/// The `#[non_exhaustive] ` prefix, when the item carries the attribute.
/// A variant's own attributes carry it; an enum's say nothing about its
/// variants.
fn non_exhaustive(item: &Item) -> &'static str {
    if item
        .attrs
        .iter()
        .any(|attr| matches!(attr, Attribute::NonExhaustive))
    {
        "#[non_exhaustive] "
    } else {
        ""
    }
}

/// What a struct line ends with: nothing but `;` for a unit struct, the
/// tuple or braced form otherwise.
fn struct_shape(kind: &StructKind) -> &'static str {
    match kind {
        StructKind::Unit => ";",
        StructKind::Tuple(fields) => tuple(fields.iter().any(Option::is_none)),
        StructKind::Plain {
            has_stripped_fields,
            ..
        } => braced(*has_stripped_fields),
    }
}

/// What a variant line ends with; a unit variant ends with nothing, so
/// its line is the path alone.
fn variant_shape(kind: &VariantKind) -> &'static str {
    match kind {
        VariantKind::Plain => "",
        VariantKind::Tuple(fields) => tuple(fields.iter().any(Option::is_none)),
        VariantKind::Struct {
            has_stripped_fields,
            ..
        } => braced(*has_stripped_fields),
    }
}

/// A tuple kind's shape, naming hidden fields in place of `..`.
fn tuple(hidden: bool) -> &'static str {
    if hidden {
        "(/* private fields */)"
    } else {
        "(..)"
    }
}

/// A braced kind's shape, naming hidden fields in place of `..`.
fn braced(hidden: bool) -> &'static str {
    if hidden {
        " { /* private fields */ }"
    } else {
        " { .. }"
    }
}

#[path = "render-types.rs"]
mod types;
