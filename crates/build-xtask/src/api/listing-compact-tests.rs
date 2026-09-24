//! The listing's compact impl lines, rule by rule: markers are left out,
//! auto traits and derive-style impls each share one line per type, and
//! an impl bounding a type parameter, like every other impl, keeps its
//! own line. The fixture checks all four against real rustdoc JSON.

use rustdoc_types::{GenericBound, GenericParamDef, Id, Path, TraitBoundModifier, Type};

use super::super::super::fixture::workspace;
use super::super::super::report;
use super::*;

fn trait_path(args: Option<GenericArgs>) -> Path {
    Path {
        path: String::new(),
        id: Id(0),
        args: args.map(Box::new),
    }
}

fn generics(params: Vec<GenericParamDef>, where_predicates: Vec<WherePredicate>) -> Generics {
    Generics {
        params,
        where_predicates,
    }
}

/// A trait impl passing the trait `args`, over `generics`.
fn impl_of(args: Option<GenericArgs>, generics: Generics, negative: bool) -> Impl {
    Impl {
        is_unsafe: false,
        generics,
        provided_trait_methods: Vec::new(),
        trait_: Some(trait_path(args)),
        for_: Type::Infer,
        items: Vec::new(),
        is_negative: negative,
        is_synthetic: false,
        blanket_impl: None,
    }
}

fn plain() -> Impl {
    impl_of(None, generics(Vec::new(), Vec::new()), false)
}

fn negative() -> Impl {
    impl_of(None, generics(Vec::new(), Vec::new()), true)
}

fn trait_bound() -> GenericBound {
    GenericBound::TraitBound {
        trait_: trait_path(None),
        generic_params: Vec::new(),
        modifier: TraitBoundModifier::None,
    }
}

fn type_param(bounds: Vec<GenericBound>) -> GenericParamDef {
    GenericParamDef {
        name: "T".to_owned(),
        kind: GenericParamDefKind::Type {
            bounds,
            default: None,
            is_synthetic: false,
        },
    }
}

fn lifetime_param(name: &str) -> GenericParamDef {
    GenericParamDef {
        name: name.to_owned(),
        kind: GenericParamDefKind::Lifetime {
            outlives: Vec::new(),
        },
    }
}

/// `impl<T: Bound> Trait for Type<T>`.
fn bounded_inline() -> Impl {
    impl_of(
        None,
        generics(vec![type_param(vec![trait_bound()])], Vec::new()),
        false,
    )
}

/// `impl<T> Trait for Type<T> where T: Bound`.
fn bounded_in_where() -> Impl {
    let predicate = WherePredicate::BoundPredicate {
        type_: Type::Generic("T".to_owned()),
        bounds: vec![trait_bound()],
        generic_params: Vec::new(),
    };
    impl_of(
        None,
        generics(vec![type_param(Vec::new())], vec![predicate]),
        false,
    )
}

#[test]
fn marker_impls_are_left_out_even_when_bounded() {
    for name in ["StructuralPartialEq", "TrivialClone", "UnsafeUnpin"] {
        for block in [plain(), bounded_inline(), bounded_in_where()] {
            assert_eq!(
                placement(Some(("core", name)), &block),
                Placement::Omitted,
                "{name}"
            );
        }
    }
}

#[test]
fn an_auto_trait_impl_joins_the_auto_line_marked_when_negative() {
    for name in [
        "Freeze",
        "RefUnwindSafe",
        "Send",
        "Sync",
        "Unpin",
        "UnwindSafe",
    ] {
        assert_eq!(
            placement(Some(("core", name)), &plain()),
            Placement::Auto { name, lacks: false }
        );
        assert_eq!(
            placement(Some(("core", name)), &negative()),
            Placement::Auto { name, lacks: true }
        );
    }
}

#[test]
fn the_auto_line_names_its_traits_alphabetically_with_a_bang_on_each_one_lacking() {
    let mut shared = Shared::default();
    for (name, lacks) in [
        ("UnwindSafe", false),
        ("Sync", true),
        ("Freeze", false),
        ("Send", true),
        ("Unpin", false),
        ("RefUnwindSafe", false),
    ] {
        shared
            .auto
            .entry("promptforge::Local")
            .or_default()
            .insert(name, lacks);
    }
    assert_eq!(
        shared.lines(),
        ["auto promptforge::Local: Freeze, RefUnwindSafe, !Send, !Sync, Unpin, UnwindSafe"]
    );
}

#[test]
fn a_derive_style_impl_joins_the_derives_line() {
    for name in [
        "Clone",
        "Copy",
        "Debug",
        "Default",
        "Eq",
        "Hash",
        "Ord",
        "PartialEq",
        "PartialOrd",
    ] {
        assert_eq!(
            placement(Some(("core", name)), &plain()),
            Placement::Derives(name)
        );
    }
    for krate in ["serde", "serde_core"] {
        for name in ["Deserialize", "Serialize"] {
            assert_eq!(
                placement(Some((krate, name)), &plain()),
                Placement::Derives(name),
                "{krate}"
            );
        }
    }
    let deserialize = impl_of(
        Some(GenericArgs::AngleBracketed {
            args: vec![GenericArg::Lifetime("'de".to_owned())],
            constraints: Vec::new(),
        }),
        generics(vec![lifetime_param("'de")], Vec::new()),
        false,
    );
    assert_eq!(
        placement(Some(("serde_core", "Deserialize")), &deserialize),
        Placement::Derives("Deserialize"),
        "a lifetime is no type parameter"
    );
}

#[test]
fn the_derives_line_names_its_traits_alphabetically() {
    let mut shared = Shared::default();
    for name in [
        "Serialize",
        "PartialOrd",
        "Clone",
        "Deserialize",
        "PartialEq",
        "Default",
        "Copy",
        "Ord",
        "Debug",
        "Hash",
        "Eq",
    ] {
        shared
            .derives
            .entry("promptforge::Plain")
            .or_default()
            .insert(name);
    }
    assert_eq!(
        shared.lines(),
        [
            "derives promptforge::Plain: Clone, Copy, Debug, Default, Deserialize, Eq, Hash, \
             Ord, PartialEq, PartialOrd, Serialize"
        ]
    );
}

#[test]
fn an_impl_bounding_a_type_parameter_keeps_its_own_line() {
    for block in [bounded_inline(), bounded_in_where()] {
        for name in ["Clone", "Send"] {
            assert_eq!(
                placement(Some(("core", name)), &block),
                Placement::Own,
                "{name}"
            );
        }
    }
    let unbounded = impl_of(
        None,
        generics(
            vec![type_param(Vec::new()), lifetime_param("'a")],
            vec![WherePredicate::LifetimePredicate {
                lifetime: "'a".to_owned(),
                outlives: vec!["'static".to_owned()],
            }],
        ),
        false,
    );
    assert_eq!(
        placement(Some(("core", "Send")), &unbounded),
        Placement::Auto {
            name: "Send",
            lacks: false
        },
        "an unbounded parameter and a lifetime bound bound no type parameter"
    );
}

#[test]
fn every_other_impl_keeps_its_own_line() {
    for name in ["Display", "Error", "From"] {
        assert_eq!(
            placement(Some(("core", name)), &plain()),
            Placement::Own,
            "{name}"
        );
    }
    let partial_eq_str = impl_of(
        Some(GenericArgs::AngleBracketed {
            args: vec![GenericArg::Type(Type::Primitive("str".to_owned()))],
            constraints: Vec::new(),
        }),
        generics(Vec::new(), Vec::new()),
        false,
    );
    assert_eq!(
        placement(Some(("core", "PartialEq")), &partial_eq_str),
        Placement::Own,
        "an impl relating the type to another is no derive"
    );
    assert_eq!(
        placement(Some(("core", "Clone")), &negative()),
        Placement::Own
    );
    for name in ["Clone", "Send", "TrivialClone", "UnsafeUnpin"] {
        assert_eq!(
            placement(Some(("promptforge_inner", name)), &plain()),
            Placement::Own,
            "a same-named trait outside core: {name}"
        );
    }
    assert_eq!(placement(None, &plain()), Placement::Own);
}

const INNER: &str = "//! Inner.\n\n\
    use std::fmt;\n\n\
    /// Plain data.\n\
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]\n\
    pub struct Plain;\n\n\
    impl fmt::Display for Plain {\n    \
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {\n        \
            f.write_str(\"plain\")\n    \
        }\n\
    }\n\n\
    impl PartialEq<str> for Plain {\n    \
        fn eq(&self, _other: &str) -> bool {\n        \
            false\n    \
        }\n\
    }\n\n\
    impl serde::Serialize for Plain {}\n\n\
    /// Bound to one thread.\n\
    pub struct Local(*const ());\n\n\
    /// Holds anything.\n\
    #[derive(Clone)]\n\
    pub struct Wrapper<T>(pub T);\n";

const FACADE: &str = "//! Facade.\npub use promptforge_inner::Local;\n\
    pub use promptforge_inner::Plain;\npub use promptforge_inner::Wrapper;\n";

#[test]
#[ignore = "needs the pinned nightly"]
fn the_listing_drops_markers_and_collapses_auto_and_derive_style_impls() {
    let root = workspace(INNER, FACADE);
    let listing = report(root.path()).expect("the fixture documents").listing;
    let wrapper_auto = ["Freeze", "Send", "Sync", "Unpin"].map(|name| {
        format!(
            "impl<T> core::marker::{name} for promptforge::Wrapper<T> where T: core::marker::{name}"
        )
    });
    let wrapper_unwind = ["RefUnwindSafe", "UnwindSafe"].map(|name| {
        format!(
            "impl<T> core::panic::unwind_safe::{name} for promptforge::Wrapper<T> where T: \
             core::panic::unwind_safe::{name}"
        )
    });
    let mut expected: Vec<String> = [
        "auto promptforge::Local: Freeze, RefUnwindSafe, !Send, !Sync, Unpin, UnwindSafe",
        "auto promptforge::Plain: Freeze, RefUnwindSafe, Send, Sync, Unpin, UnwindSafe",
        "derives promptforge::Plain: Clone, Copy, Debug, Default, Eq, PartialEq, Serialize",
        "impl core::cmp::PartialEq<str> for promptforge::Plain",
        "impl core::fmt::Display for promptforge::Plain",
        "impl<T: core::clone::Clone> core::clone::Clone for promptforge::Wrapper<T>",
        "pub promptforge::Wrapper::0: T",
        "pub struct promptforge::Local",
        "pub struct promptforge::Plain",
        "pub struct promptforge::Wrapper<T>",
    ]
    .map(str::to_owned)
    .into_iter()
    .chain(wrapper_auto)
    .chain(wrapper_unwind)
    .collect();
    expected.sort();
    assert_eq!(listing, expected);
}
