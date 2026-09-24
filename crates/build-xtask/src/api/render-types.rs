//! Type-level rendering for [`Renderer`]: types, paths and their generic
//! arguments, bounds, generic parameters, `where` clauses, and function
//! signatures. Every resolved path is recorded as a mention on the way.

use rustdoc_types::{
    Abi, DynTrait, FunctionHeader, FunctionPointer, FunctionSignature, GenericArg, GenericArgs,
    GenericBound, GenericParamDef, GenericParamDefKind, Generics, Path, PreciseCapturingArg, Term,
    TraitBoundModifier, Type, WherePredicate,
};

use super::super::items::target;
use super::{Mention, Renderer};

impl Renderer<'_> {
    /// `: A + B`, or nothing when there are no bounds.
    pub(super) fn colon_bounds(&mut self, bounds: &[GenericBound]) -> String {
        let bounds = self.bounds(bounds);
        if bounds.is_empty() {
            bounds
        } else {
            format!(": {bounds}")
        }
    }

    /// ` = T`, or nothing when there is no type.
    pub(super) fn value(&mut self, ty: Option<&Type>) -> String {
        ty.map_or_else(String::new, |ty| format!(" = {}", self.ty(ty)))
    }

    pub(super) fn signature(&mut self, signature: &FunctionSignature, named: bool) -> String {
        let mut inputs = Vec::new();
        for (name, ty) in &signature.inputs {
            inputs.push(match (name.as_str(), ty) {
                ("self", Type::Generic(generic)) if generic == "Self" => "self".to_owned(),
                (
                    "self",
                    Type::BorrowedRef {
                        lifetime,
                        is_mutable,
                        type_,
                    },
                ) if matches!(type_.as_ref(), Type::Generic(generic) if generic == "Self") => {
                    format!(
                        "&{}{}self",
                        lifetime_prefix(lifetime.as_ref()),
                        mutable(*is_mutable)
                    )
                }
                (name, ty) if named => format!("{name}: {}", self.ty(ty)),
                (_, ty) => self.ty(ty),
            });
        }
        if signature.is_c_variadic {
            inputs.push("...".to_owned());
        }
        let output = match &signature.output {
            Some(ty) => format!(" -> {}", self.ty(ty)),
            None => String::new(),
        };
        format!("({}){output}", inputs.join(", "))
    }

    pub(super) fn ty(&mut self, ty: &Type) -> String {
        match ty {
            Type::ResolvedPath(path) => self.path(path),
            Type::DynTrait(dyn_trait) => self.dyn_trait(dyn_trait),
            Type::Generic(name) | Type::Primitive(name) => name.clone(),
            Type::FunctionPointer(pointer) => self.fn_pointer(pointer),
            Type::Tuple(types) if types.len() == 1 => format!("({},)", self.list(types)),
            Type::Tuple(types) => format!("({})", self.list(types)),
            Type::Slice(inner) => format!("[{}]", self.ty(inner)),
            Type::Array { type_, len } => format!("[{}; {len}]", self.ty(type_)),
            Type::Pat { type_, .. } => format!("{} is ..", self.ty(type_)),
            Type::ImplTrait(bounds) => format!("impl {}", self.bounds(bounds)),
            Type::Infer => "_".to_owned(),
            Type::RawPointer { is_mutable, type_ } => {
                let kind = if *is_mutable { "mut" } else { "const" };
                format!("*{kind} {}", self.ty(type_))
            }
            Type::BorrowedRef {
                lifetime,
                is_mutable,
                type_,
            } => format!(
                "&{}{}{}",
                lifetime_prefix(lifetime.as_ref()),
                mutable(*is_mutable),
                self.ty(type_)
            ),
            Type::QualifiedPath {
                name,
                args,
                self_type,
                trait_,
            } => {
                let self_type = self.ty(self_type);
                let args = args
                    .as_ref()
                    .map_or_else(String::new, |args| self.args(args));
                match trait_ {
                    Some(path) => format!("<{self_type} as {}>::{name}{args}", self.path(path)),
                    None => format!("{self_type}::{name}{args}"),
                }
            }
        }
    }

    fn list(&mut self, types: &[Type]) -> String {
        types
            .iter()
            .map(|ty| self.ty(ty))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(super) fn path(&mut self, path: &Path) -> String {
        let resolved = target(self.krate, path.id);
        let shown = match &resolved {
            Some(found) => match self.surface.facade_path(found.krate, found.path) {
                Some(facade) => facade.to_owned(),
                None => found.path.join("::"),
            },
            None => path.path.clone(),
        };
        self.mentions.push(Mention {
            context: self.context,
            target: resolved.map(|found| (found.krate.to_owned(), found.path.to_vec())),
            written: path.path.clone(),
        });
        match &path.args {
            Some(args) => format!("{shown}{}", self.args(args)),
            None => shown,
        }
    }

    fn args(&mut self, args: &GenericArgs) -> String {
        match args {
            GenericArgs::AngleBracketed { args, constraints } => {
                let mut parts: Vec<String> = args.iter().map(|arg| self.arg(arg)).collect();
                for constraint in constraints {
                    let args = constraint
                        .args
                        .as_ref()
                        .map_or_else(String::new, |args| self.args(args));
                    let binding = match &constraint.binding {
                        rustdoc_types::AssocItemConstraintKind::Equality(term) => {
                            format!(" = {}", self.term(term))
                        }
                        rustdoc_types::AssocItemConstraintKind::Constraint(bounds) => {
                            format!(": {}", self.bounds(bounds))
                        }
                    };
                    parts.push(format!("{}{args}{binding}", constraint.name));
                }
                if parts.is_empty() {
                    String::new()
                } else {
                    format!("<{}>", parts.join(", "))
                }
            }
            GenericArgs::Parenthesized { inputs, output } => {
                let inputs = self.list(inputs);
                let output = output
                    .as_ref()
                    .map_or_else(String::new, |ty| format!(" -> {}", self.ty(ty)));
                format!("({inputs}){output}")
            }
            GenericArgs::ReturnTypeNotation => "(..)".to_owned(),
        }
    }

    fn arg(&mut self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Lifetime(lifetime) => lifetime.clone(),
            GenericArg::Type(ty) => self.ty(ty),
            GenericArg::Const(constant) => constant.expr.clone(),
            GenericArg::Infer => "_".to_owned(),
        }
    }

    fn term(&mut self, term: &Term) -> String {
        match term {
            Term::Type(ty) => self.ty(ty),
            Term::Constant(constant) => constant.expr.clone(),
        }
    }

    fn bounds(&mut self, bounds: &[GenericBound]) -> String {
        let mut parts = Vec::new();
        for bound in bounds {
            parts.push(match bound {
                GenericBound::TraitBound {
                    trait_,
                    generic_params,
                    modifier,
                } => {
                    let binder = self.binder(generic_params);
                    let modifier = match modifier {
                        TraitBoundModifier::None => "",
                        TraitBoundModifier::Maybe => "?",
                        TraitBoundModifier::MaybeConst => "[const] ",
                    };
                    format!("{binder}{modifier}{}", self.path(trait_))
                }
                GenericBound::Outlives(lifetime) => lifetime.clone(),
                GenericBound::Use(args) => {
                    let args: Vec<&str> = args
                        .iter()
                        .map(|arg| match arg {
                            PreciseCapturingArg::Lifetime(name)
                            | PreciseCapturingArg::Param(name) => name.as_str(),
                        })
                        .collect();
                    format!("use<{}>", args.join(", "))
                }
            });
        }
        parts.join(" + ")
    }

    fn binder(&mut self, params: &[GenericParamDef]) -> String {
        if params.is_empty() {
            return String::new();
        }
        let params: Vec<String> = params.iter().map(|param| self.param(param)).collect();
        format!("for<{}> ", params.join(", "))
    }

    fn param(&mut self, param: &GenericParamDef) -> String {
        let name = &param.name;
        match &param.kind {
            GenericParamDefKind::Lifetime { outlives } if outlives.is_empty() => name.clone(),
            GenericParamDefKind::Lifetime { outlives } => {
                format!("{name}: {}", outlives.join(" + "))
            }
            GenericParamDefKind::Type {
                bounds, default, ..
            } => {
                let bounds = self.colon_bounds(bounds);
                let default = self.value(default.as_ref());
                format!("{name}{bounds}{default}")
            }
            GenericParamDefKind::Const { type_, default } => {
                let default = default
                    .as_ref()
                    .map_or_else(String::new, |value| format!(" = {value}"));
                format!("const {name}: {}{default}", self.ty(type_))
            }
        }
    }

    /// `<..>` over the declared parameters; the synthetic ones an
    /// argument-position `impl Trait` introduces render in the argument.
    pub(super) fn generics(&mut self, generics: &Generics) -> String {
        self.context = "bound";
        let params: Vec<String> = generics
            .params
            .iter()
            .filter(|param| {
                !matches!(
                    param.kind,
                    GenericParamDefKind::Type {
                        is_synthetic: true,
                        ..
                    }
                )
            })
            .map(|param| self.param(param))
            .collect();
        if params.is_empty() {
            String::new()
        } else {
            format!("<{}>", params.join(", "))
        }
    }

    pub(super) fn where_clause(&mut self, generics: &Generics) -> String {
        self.context = "bound";
        let mut predicates = Vec::new();
        for predicate in &generics.where_predicates {
            predicates.push(match predicate {
                WherePredicate::BoundPredicate {
                    type_,
                    bounds,
                    generic_params,
                } => {
                    let binder = self.binder(generic_params);
                    format!("{binder}{}: {}", self.ty(type_), self.bounds(bounds))
                }
                WherePredicate::LifetimePredicate { lifetime, outlives } => {
                    format!("{lifetime}: {}", outlives.join(" + "))
                }
                WherePredicate::EqPredicate { lhs, rhs } => {
                    format!("{} == {}", self.ty(lhs), self.term(rhs))
                }
            });
        }
        if predicates.is_empty() {
            String::new()
        } else {
            format!(" where {}", predicates.join(", "))
        }
    }

    fn dyn_trait(&mut self, dyn_trait: &DynTrait) -> String {
        let mut parts = Vec::new();
        for poly in &dyn_trait.traits {
            let binder = self.binder(&poly.generic_params);
            parts.push(format!("{binder}{}", self.path(&poly.trait_)));
        }
        parts.extend(dyn_trait.lifetime.clone());
        format!("dyn {}", parts.join(" + "))
    }

    fn fn_pointer(&mut self, pointer: &FunctionPointer) -> String {
        let binder = self.binder(&pointer.generic_params);
        let signature = self.signature(&pointer.sig, false);
        format!("{binder}{}fn{signature}", header(&pointer.header))
    }
}

/// `const async unsafe extern "abi" `, each part only when it applies.
pub(super) fn header(header: &FunctionHeader) -> String {
    let mut text = String::new();
    for (on, word) in [
        (header.is_const, "const "),
        (header.is_async, "async "),
        (header.is_unsafe, "unsafe "),
    ] {
        if on {
            text.push_str(word);
        }
    }
    let (abi, unwind) = match &header.abi {
        Abi::Rust => return text,
        Abi::C { unwind } => ("C", *unwind),
        Abi::Cdecl { unwind } => ("cdecl", *unwind),
        Abi::Stdcall { unwind } => ("stdcall", *unwind),
        Abi::Fastcall { unwind } => ("fastcall", *unwind),
        Abi::Aapcs { unwind } => ("aapcs", *unwind),
        Abi::Win64 { unwind } => ("win64", *unwind),
        Abi::SysV64 { unwind } => ("sysv64", *unwind),
        Abi::System { unwind } => ("system", *unwind),
        Abi::Other(name) => (name.as_str(), false),
    };
    let unwind = if unwind { "-unwind" } else { "" };
    format!("{text}extern \"{abi}{unwind}\" ")
}

fn lifetime_prefix(lifetime: Option<&String>) -> String {
    lifetime.map_or_else(String::new, |lifetime| format!("{lifetime} "))
}

fn mutable(is_mutable: bool) -> &'static str {
    if is_mutable { "mut " } else { "" }
}
