//! The package registry and installs by name: a new registry holds the
//! native `promptforge/subagents` and lists every package in name order,
//! `add` refuses a name that is not a vendor/name pair and a name the
//! registry holds already, and install refuses a package the registry
//! does not hold, keeps the native's name to the native, and refuses the
//! native's second install as a taken name.

use harness_runner::{HostContext, InstallError, PackageRegistry, PluginKind};
use promptforge_plugin::{HostServices, PluginId, ServiceId, ServiceKey};
use serde_json::Value;

use crate::prepare::echo_package;
use crate::support::registry;

/// The native's package name.
const SUBAGENTS: &str = "promptforge/subagents";

/// The test-only service a listed package needs.
const GREETING: ServiceKey<str> = ServiceKey::new("tests/greeting");

const NEEDS: &[ServiceId] = &[GREETING.id()];

const NO_NEEDS: &[ServiceId] = &[];

fn name(text: &str) -> PluginId {
    PluginId::parse(text).unwrap()
}

#[test]
fn a_new_registry_holds_the_native_and_lists_every_package_in_name_order() {
    let mut packages = PackageRegistry::new();
    packages
        .add(echo_package("tests/zeta", None).needs(NEEDS))
        .unwrap();
    packages.add(echo_package("acme/alpha", None)).unwrap();
    let listed: Vec<(&str, PluginKind, &[ServiceId])> = packages
        .list()
        .map(|(package, kind)| (package.name, kind, package.needs))
        .collect();
    assert_eq!(
        listed,
        [
            ("acme/alpha", PluginKind::Linked, NO_NEEDS),
            (SUBAGENTS, PluginKind::Native, NO_NEEDS),
            ("tests/zeta", PluginKind::Linked, NEEDS),
        ],
        "a package added with needs lists those needs"
    );
    let (native, _kind) = packages.list().nth(1).unwrap();
    assert!(
        !native.description.is_empty(),
        "a Host listing packages can say what the native does"
    );
}

#[test]
fn add_refuses_a_package_name_that_is_not_a_vendor_name_pair() {
    for bad in [
        "web",
        "acme/web/extra",
        "/web",
        "acme/",
        "Acme/web",
        "acme/Web",
    ] {
        assert_eq!(
            PackageRegistry::new().add(echo_package(bad, None)),
            Err(InstallError::InvalidPackage { package: bad }),
            "{bad}"
        );
    }
}

#[test]
fn add_refuses_a_package_name_the_registry_holds_already_the_natives_included() {
    let mut packages = registry(&[echo_package("tests/tools", None)]);
    for taken in ["tests/tools", SUBAGENTS] {
        assert_eq!(
            packages.add(echo_package(taken, None)),
            Err(InstallError::DuplicatePackage { package: taken }),
            "{taken}"
        );
    }
    assert!(
        packages
            .list()
            .any(|(package, kind)| package.name == SUBAGENTS && kind == PluginKind::Native),
        "the refused add left the native in place"
    );
}

#[test]
fn install_refuses_a_package_the_registry_does_not_hold() {
    let mut host = HostContext::new(
        registry(&[echo_package("tests/tools", None)]),
        HostServices::new(),
    );
    for missing in ["tests/other", "promptforge/tasks"] {
        assert_eq!(
            host.install(missing, None, Value::Null),
            Err(InstallError::UnknownPackage {
                package: missing.to_owned()
            }),
            "{missing}"
        );
    }
    let message = host
        .install("tests/other", None, Value::Null)
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("tests/other") && message.contains("PackageRegistry::add"),
        "the refusal names the package and how to add it: {message}"
    );
}

#[test]
fn install_keeps_the_natives_name_to_the_native_alone() {
    let mut host = HostContext::new(
        registry(&[
            echo_package("tests/tools", None),
            echo_package("acme/subagents", None),
        ]),
        HostServices::new(),
    );
    assert_eq!(
        host.install(SUBAGENTS, Some(name("helpers")), Value::Null),
        Err(InstallError::ReservedName {
            package: SUBAGENTS,
            name: name("helpers"),
        }),
        "the native keeps its own name"
    );
    assert_eq!(
        host.install("tests/tools", Some(name("subagents")), Value::Null),
        Err(InstallError::ReservedName {
            package: "tests/tools",
            name: name("subagents"),
        }),
        "a linked package may not pick the native's name"
    );
    let refused = host.install("acme/subagents", None, Value::Null);
    assert_eq!(
        refused,
        Err(InstallError::ReservedName {
            package: "acme/subagents",
            name: name("subagents"),
        }),
        "a linked package may not take the native's name by default"
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "Plugin package `acme/subagents` cannot be installed as subagents: \
         a native Plugin keeps its own name, and no other package may take it"
    );
    assert_eq!(
        host.install(SUBAGENTS, Some(name("subagents")), Value::Null),
        Ok(name("subagents")),
        "the native installs under its own name"
    );
}

#[test]
fn installing_the_native_twice_is_a_taken_name() {
    let mut host = HostContext::new(PackageRegistry::new(), HostServices::new());
    assert_eq!(
        host.install(SUBAGENTS, None, Value::Null),
        Ok(name("subagents"))
    );
    assert_eq!(
        host.install(SUBAGENTS, None, Value::Null),
        Err(InstallError::NameTaken {
            name: name("subagents"),
            existing: name("subagents"),
        })
    );
}
