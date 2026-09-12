//! Cargo metadata checks for the five approved product dependency boundaries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

#[derive(serde::Deserialize)]
struct CargoMetadata {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
}

#[derive(serde::Deserialize)]
struct MetadataPackage {
    name: String,
    id: String,
    manifest_path: PathBuf,
    dependencies: Vec<MetadataDependency>,
}

#[derive(serde::Deserialize)]
struct MetadataDependency {
    path: Option<PathBuf>,
}

#[derive(Clone, Copy)]
enum PackageSet {
    Gateway,
    PromptForge,
    Workshop,
    Shared,
    GatewayOrWorkshop,
    AnyProduct,
}

impl PackageSet {
    fn contains(self, package: &str) -> bool {
        match self {
            Self::Gateway => package == "gateway" || package.starts_with("gateway-"),
            Self::PromptForge => package == "promptforge" || package.starts_with("promptforge-"),
            Self::Workshop => package == "workshop" || package.starts_with("workshop-"),
            Self::Shared => package.starts_with("shared-"),
            Self::GatewayOrWorkshop => {
                Self::Gateway.contains(package) || Self::Workshop.contains(package)
            }
            Self::AnyProduct => {
                Self::Gateway.contains(package)
                    || Self::PromptForge.contains(package)
                    || Self::Workshop.contains(package)
            }
        }
    }
}

struct DependencyRule {
    dependent: PackageSet,
    forbidden: PackageSet,
    description: &'static str,
}

const PRODUCT_DEPENDENCY_RULES: [DependencyRule; 5] = [
    DependencyRule {
        dependent: PackageSet::Gateway,
        forbidden: PackageSet::Workshop,
        description: "Gateway cannot depend on Workshop",
    },
    DependencyRule {
        dependent: PackageSet::PromptForge,
        forbidden: PackageSet::GatewayOrWorkshop,
        description: "PromptForge cannot depend on Gateway or Workshop",
    },
    DependencyRule {
        dependent: PackageSet::Gateway,
        forbidden: PackageSet::PromptForge,
        description: "Gateway cannot depend on PromptForge",
    },
    DependencyRule {
        dependent: PackageSet::Workshop,
        forbidden: PackageSet::Gateway,
        description: "Workshop cannot depend on Gateway",
    },
    DependencyRule {
        dependent: PackageSet::Shared,
        forbidden: PackageSet::AnyProduct,
        description: "Shared cannot depend on any product",
    },
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("gateway-stt is nested under the workspace crates directory"))
        .to_owned()
}

fn workspace_metadata() -> &'static CargoMetadata {
    static METADATA: OnceLock<CargoMetadata> = OnceLock::new();
    METADATA.get_or_init(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(workspace_root())
            .output()
            .unwrap_or_else(|error| panic!("cargo metadata must start: {error}"));
        assert!(
            output.status.success(),
            "cargo metadata must succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("cargo metadata must return valid JSON: {error}"))
    })
}

fn workspace_package_names_by_root(metadata: &CargoMetadata) -> BTreeMap<PathBuf, &str> {
    let members = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
        .map(|package| {
            let root = package
                .manifest_path
                .parent()
                .unwrap_or_else(|| panic!("workspace package manifest has a parent"))
                .to_owned();
            (root, package.name.as_str())
        })
        .collect()
}

fn direct_local_dependencies<'a>(
    package: &'a MetadataPackage,
    names_by_root: &BTreeMap<PathBuf, &'a str>,
) -> BTreeSet<&'a str> {
    package
        .dependencies
        .iter()
        .filter_map(|dependency| dependency.path.as_ref())
        .filter_map(|path| names_by_root.get(path).copied())
        .collect()
}

fn dependency_violations(metadata: &CargoMetadata) -> Vec<String> {
    let members = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    let names_by_root = workspace_package_names_by_root(metadata);
    let mut violations = metadata
        .packages
        .iter()
        .filter(|package| members.contains(&package.id))
        .flat_map(|package| {
            direct_local_dependencies(package, &names_by_root)
                .into_iter()
                .flat_map(move |dependency| {
                    PRODUCT_DEPENDENCY_RULES
                        .iter()
                        .filter(move |rule| {
                            rule.dependent.contains(&package.name)
                                && rule.forbidden.contains(dependency)
                        })
                        .map(move |rule| {
                            format!(
                                "{}: `{}` directly depends on `{dependency}`",
                                rule.description, package.name
                            )
                        })
                })
        })
        .collect::<Vec<_>>();
    violations.sort();
    violations
}

#[test]
fn workspace_obeys_the_five_product_dependency_rules() {
    let violations = dependency_violations(workspace_metadata());
    assert!(
        violations.is_empty(),
        "forbidden direct local dependencies:\n{}",
        violations.join("\n")
    );
}

#[test]
fn metadata_includes_renamed_target_specific_dependencies_of_every_kind() {
    let fixture = r#"
    {
      "workspace_members": ["source", "normal", "development", "build", "target"],
      "packages": [
        {
          "name": "source",
          "id": "source",
          "manifest_path": "C:/workspace/source/Cargo.toml",
          "dependencies": [
            {"name": "normal", "path": "C:/workspace/normal", "kind": null, "rename": "renamed"},
            {"name": "development", "path": "C:/workspace/development", "kind": "dev"},
            {"name": "build", "path": "C:/workspace/build", "kind": "build"},
            {"name": "target", "path": "C:/workspace/target", "kind": null, "target": "cfg(unix)"},
            {"name": "external", "path": null, "kind": null}
          ]
        },
        {
          "name": "normal", "id": "normal",
          "manifest_path": "C:/workspace/normal/Cargo.toml", "dependencies": []
        },
        {
          "name": "development", "id": "development",
          "manifest_path": "C:/workspace/development/Cargo.toml", "dependencies": []
        },
        {
          "name": "build", "id": "build",
          "manifest_path": "C:/workspace/build/Cargo.toml", "dependencies": []
        },
        {
          "name": "target", "id": "target",
          "manifest_path": "C:/workspace/target/Cargo.toml", "dependencies": []
        }
      ]
    }"#;
    let metadata: CargoMetadata =
        serde_json::from_str(fixture).expect("adversarial metadata fixture parses");
    let names = workspace_package_names_by_root(&metadata);
    let source = metadata
        .packages
        .iter()
        .find(|package| package.name == "source")
        .expect("fixture source exists");

    assert_eq!(
        direct_local_dependencies(source, &names),
        ["build", "development", "normal", "target"]
            .into_iter()
            .collect()
    );
}

#[test]
fn adversarial_metadata_triggers_each_product_dependency_rule() {
    let fixture = r#"
    {
      "workspace_members": [
        "gateway-source", "promptforge-source", "workshop-source",
        "workshop-shell-source", "shared-source",
        "gateway-target", "promptforge-target", "workshop-target"
      ],
      "packages": [
        {
          "name": "gateway-source", "id": "gateway-source",
          "manifest_path": "C:/workspace/gateway-source/Cargo.toml",
          "dependencies": [
            {"path": "C:/workspace/promptforge-target"},
            {"path": "C:/workspace/workshop-target"}
          ]
        },
        {
          "name": "promptforge-source", "id": "promptforge-source",
          "manifest_path": "C:/workspace/promptforge-source/Cargo.toml",
          "dependencies": [
            {"path": "C:/workspace/gateway-target"},
            {"path": "C:/workspace/workshop-target"}
          ]
        },
        {
          "name": "workshop", "id": "workshop-source",
          "manifest_path": "C:/workspace/workshop-source/Cargo.toml",
          "dependencies": [{"path": "C:/workspace/gateway-target"}]
        },
        {
          "name": "workshop-shell", "id": "workshop-shell-source",
          "manifest_path": "C:/workspace/workshop-shell-source/Cargo.toml",
          "dependencies": [{"path": "C:/workspace/gateway-target"}]
        },
        {
          "name": "shared-source", "id": "shared-source",
          "manifest_path": "C:/workspace/shared-source/Cargo.toml",
          "dependencies": [{"path": "C:/workspace/promptforge-target"}]
        },
        {
          "name": "gateway-target", "id": "gateway-target",
          "manifest_path": "C:/workspace/gateway-target/Cargo.toml", "dependencies": []
        },
        {
          "name": "promptforge-target", "id": "promptforge-target",
          "manifest_path": "C:/workspace/promptforge-target/Cargo.toml", "dependencies": []
        },
        {
          "name": "workshop-server", "id": "workshop-target",
          "manifest_path": "C:/workspace/workshop-target/Cargo.toml", "dependencies": []
        }
      ]
    }"#;
    let metadata: CargoMetadata =
        serde_json::from_str(fixture).expect("adversarial metadata fixture parses");
    let violations = dependency_violations(&metadata);

    for rule in PRODUCT_DEPENDENCY_RULES {
        assert!(
            violations
                .iter()
                .any(|violation| violation.starts_with(rule.description)),
            "fixture must trigger `{}`: {violations:?}",
            rule.description
        );
    }
    assert_eq!(
        violations.len(),
        7,
        "PromptForge's combined rule rejects both forbidden product families, \
         the Workshop rule is prefix-based, and Shared rejects every product"
    );
}
