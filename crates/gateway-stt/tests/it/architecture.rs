//! Architecture ratchets for the four-crate STT stack.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const STT_CRATES: [&str; 4] = [
    "gateway-stt",
    "gateway-stt-engine",
    "gateway-stt-backend-whisper",
    "gateway-whisper-ffi",
];

const PHASE_A_CRATES: [&str; 7] = [
    "gateway",
    "gateway-stt",
    "gateway-stt-engine",
    "gateway-stt-backend-whisper",
    "gateway-whisper-ffi",
    "shared-loopback",
    "workshop-server",
];

struct DependencyPolicy {
    crate_name: &'static str,
    final_edges: &'static [&'static str],
    temporary_edges: &'static [TemporaryEdge],
}

struct TemporaryEdge {
    dependency: &'static str,
    removal_step: &'static str,
}

struct MigrationPolicy {
    crate_name: &'static str,
    targets: &'static [MigrationPolicyTarget],
}

struct MigrationPolicyTarget {
    module: &'static str,
    target_step: &'static str,
    destination: &'static str,
}

const DEPENDENCY_POLICIES: [DependencyPolicy; 7] = [
    DependencyPolicy {
        crate_name: "gateway",
        final_edges: &[
            "gateway-config",
            "gateway-config-ui",
            "gateway-local",
            "gateway-logging",
            "gateway-routing",
            "gateway-stt",
            "gateway-web-search",
            "promptforge-core",
            "shared-loopback",
            "shared-progress",
            "shared-protocol",
            "shared-sidecar",
        ],
        temporary_edges: &[],
    },
    DependencyPolicy {
        crate_name: "gateway-stt",
        final_edges: &[
            "gateway-config",
            "gateway-local",
            "gateway-stt-backend-whisper",
            "gateway-stt-engine",
            "shared-progress",
        ],
        temporary_edges: &[TemporaryEdge {
            dependency: "workshop-server",
            removal_step: "Step 26",
        }],
    },
    DependencyPolicy {
        crate_name: "gateway-stt-engine",
        final_edges: &[],
        temporary_edges: &[],
    },
    DependencyPolicy {
        crate_name: "gateway-stt-backend-whisper",
        final_edges: &[
            "gateway-stt-engine",
            "gateway-whisper-ffi",
            "shared-progress",
        ],
        temporary_edges: &[],
    },
    DependencyPolicy {
        crate_name: "gateway-whisper-ffi",
        final_edges: &[],
        temporary_edges: &[],
    },
    DependencyPolicy {
        crate_name: "shared-loopback",
        final_edges: &[],
        temporary_edges: &[],
    },
    DependencyPolicy {
        crate_name: "workshop-server",
        final_edges: &[
            "build-ui",
            "promptforge-agent",
            "promptforge-core-support",
            "promptforge-model-client",
            "promptforge-store",
            "promptforge-tools",
            "shared-progress",
            "shared-sidecar",
        ],
        temporary_edges: &[],
    },
];

const MIGRATION_POLICIES: [MigrationPolicy; 4] = [
    MigrationPolicy {
        crate_name: "gateway-stt",
        targets: &[
            MigrationPolicyTarget {
                module: "api.rs",
                target_step: "Step 15",
                destination: "batch.rs",
            },
            MigrationPolicyTarget {
                module: "runtime.rs",
                target_step: "Step 15",
                destination: "service.rs, artifacts.rs, generation.rs, status.rs, and model.rs",
            },
            MigrationPolicyTarget {
                module: "stt.rs",
                target_step: "Step 26",
                destination: "removal after the Realtime route and Workshop relay replace the legacy socket",
            },
            MigrationPolicyTarget {
                module: "take.rs",
                target_step: "Step 14",
                destination: "independent committed-item finalization",
            },
        ],
    },
    MigrationPolicy {
        crate_name: "gateway-stt-engine",
        targets: &[
            MigrationPolicyTarget {
                module: "engine.rs",
                target_step: "Step 8",
                destination: "bounded worker dispatch",
            },
            MigrationPolicyTarget {
                module: "worker.rs",
                target_step: "Step 8",
                destination: "bounded worker command queues",
            },
        ],
    },
    MigrationPolicy {
        crate_name: "gateway-stt-backend-whisper",
        targets: &[],
    },
    MigrationPolicy {
        crate_name: "gateway-whisper-ffi",
        targets: &[],
    },
];

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CeilingsFile {
    public_root_budget: usize,
    migration_targets: BTreeMap<String, MigrationTarget>,
    modules: BTreeMap<String, usize>,
}

#[derive(Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MigrationTarget {
    target_step: String,
    destination: String,
}

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

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("gateway-stt is nested under the workspace crates directory"))
        .to_owned()
}

fn crate_root(crate_name: &str) -> PathBuf {
    workspace_root().join("crates").join(crate_name)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("{} must be readable UTF-8: {error}", path.display());
    })
}

fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn collect(directory: &Path, sources: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).unwrap_or_else(|error| {
            panic!("{} must be readable: {error}", directory.display());
        }) {
            let path = entry
                .unwrap_or_else(|error| panic!("source directory entry must be readable: {error}"))
                .path();
            if path.is_dir() {
                collect(&path, sources);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                sources.push(path);
            }
        }
    }

    let mut sources = Vec::new();
    collect(root, &mut sources);
    sources.sort();
    sources
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

fn metadata_package<'a>(metadata: &'a CargoMetadata, crate_name: &str) -> &'a MetadataPackage {
    metadata
        .packages
        .iter()
        .find(|package| package.name == crate_name)
        .unwrap_or_else(|| panic!("cargo metadata must contain workspace package {crate_name}"))
}

fn crate_workspace_edges(metadata: &CargoMetadata, crate_name: &str) -> BTreeSet<String> {
    let workspace_members = metadata.workspace_members.iter().collect::<BTreeSet<_>>();
    let package_names_by_path = metadata
        .packages
        .iter()
        .filter(|package| workspace_members.contains(&package.id))
        .map(|package| {
            let root = package
                .manifest_path
                .parent()
                .unwrap_or_else(|| panic!("workspace package manifest has a parent"))
                .to_owned();
            (root, package.name.as_str())
        })
        .collect::<BTreeMap<_, _>>();

    metadata_package(metadata, crate_name)
        .dependencies
        .iter()
        .filter_map(|dependency| {
            dependency
                .path
                .as_ref()
                .and_then(|path| package_names_by_path.get(path))
                .copied()
        })
        .filter(|dependency| *dependency != crate_name)
        .map(str::to_owned)
        .collect()
}

fn validate_dependency_policies(policies: &[DependencyPolicy]) -> Result<(), String> {
    let expected = PHASE_A_CRATES.into_iter().collect::<BTreeSet<_>>();
    let actual = policies
        .iter()
        .map(|policy| policy.crate_name)
        .collect::<BTreeSet<_>>();
    if actual.len() != policies.len() {
        return Err("dependency policies contain a duplicate crate".to_owned());
    }
    if actual != expected {
        return Err(format!(
            "dependency policies must cover the exact Phase A crates: expected {expected:?}, got {actual:?}"
        ));
    }
    Ok(())
}

#[test]
fn workspace_dependencies_match_phase_specific_allowlists() {
    let metadata = workspace_metadata();
    validate_dependency_policies(&DEPENDENCY_POLICIES).unwrap_or_else(|error| panic!("{error}"));
    for policy in &DEPENDENCY_POLICIES {
        let mut allowed = policy
            .final_edges
            .iter()
            .copied()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        for edge in policy.temporary_edges {
            assert!(
                edge.removal_step.starts_with("Step "),
                "{} -> {} must name its removal step",
                policy.crate_name,
                edge.dependency
            );
            allowed.insert(edge.dependency.to_owned());
        }
        assert_eq!(
            crate_workspace_edges(metadata, policy.crate_name),
            allowed,
            "{} workspace edges drifted from the current phase allowlist",
            policy.crate_name
        );
    }
}

#[test]
fn dependency_policy_omission_is_rejected() {
    assert!(
        validate_dependency_policies(&DEPENDENCY_POLICIES[..DEPENDENCY_POLICIES.len() - 1])
            .is_err()
    );
}

#[test]
fn metadata_edges_include_renames_local_paths_targets_and_all_kinds() {
    let fixture = r#"
    {
      "workspace_members": ["source", "normal", "development", "build", "target"],
      "packages": [
        {
          "name": "source",
          "id": "source",
          "manifest_path": "/workspace/source/Cargo.toml",
          "targets": [],
          "dependencies": [
            {"name": "normal", "path": "/workspace/normal", "kind": null, "rename": "renamed"},
            {"name": "development", "path": "/workspace/development", "kind": "dev"},
            {"name": "build", "path": "/workspace/build", "kind": "build"},
            {"name": "target", "path": "/workspace/target", "kind": null, "target": "cfg(unix)"},
            {"name": "external", "path": null, "kind": null}
          ]
        },
        {
          "name": "normal", "id": "normal",
          "manifest_path": "/workspace/normal/Cargo.toml", "targets": [], "dependencies": []
        },
        {
          "name": "development", "id": "development",
          "manifest_path": "/workspace/development/Cargo.toml", "targets": [], "dependencies": []
        },
        {
          "name": "build", "id": "build",
          "manifest_path": "/workspace/build/Cargo.toml", "targets": [], "dependencies": []
        },
        {
          "name": "target", "id": "target",
          "manifest_path": "/workspace/target/Cargo.toml", "targets": [], "dependencies": []
        }
      ]
    }"#;
    let metadata: CargoMetadata =
        serde_json::from_str(fixture).expect("adversarial metadata fixture parses");

    assert_eq!(
        crate_workspace_edges(&metadata, "source"),
        ["build", "development", "normal", "target"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
}

fn ceilings(crate_name: &str) -> CeilingsFile {
    let path = crate_root(crate_name).join("module-ceilings.toml");
    parse_ceilings(&read(&path))
        .unwrap_or_else(|error| panic!("{} must parse as TOML: {error}", path.display()))
}

fn parse_ceilings(source: &str) -> Result<CeilingsFile, toml::de::Error> {
    toml::from_str(source)
}

fn relative_source_path(src: &Path, source: &Path) -> String {
    source
        .strip_prefix(src)
        .unwrap_or_else(|_| panic!("source lives below its crate src directory"))
        .to_string_lossy()
        .replace('\\', "/")
}

fn expected_migration_targets(crate_name: &str) -> BTreeMap<String, MigrationTarget> {
    MIGRATION_POLICIES
        .iter()
        .find(|policy| policy.crate_name == crate_name)
        .unwrap_or_else(|| panic!("migration policy must cover {crate_name}"))
        .targets
        .iter()
        .map(|target| {
            (
                target.module.to_owned(),
                MigrationTarget {
                    target_step: target.target_step.to_owned(),
                    destination: target.destination.to_owned(),
                },
            )
        })
        .collect()
}

fn validate_migration_targets(crate_name: &str, config: &CeilingsFile) -> Result<(), String> {
    let expected = expected_migration_targets(crate_name);
    if config.migration_targets != expected {
        return Err(format!(
            "{crate_name} migration targets must match the exact phase policy: expected {expected:?}, got {:?}",
            config.migration_targets
        ));
    }
    for module in config.migration_targets.keys() {
        if !config.modules.contains_key(module) {
            return Err(format!(
                "{crate_name} migration target names unknown module {module}"
            ));
        }
    }
    Ok(())
}

#[test]
fn module_ceilings_cover_sources_and_name_migration_targets() {
    for crate_name in STT_CRATES {
        let src = crate_root(crate_name).join("src");
        let config = ceilings(crate_name);
        assert!(
            config.public_root_budget > 0,
            "{crate_name} public root budget must be a strict positive ceiling"
        );
        let measured = rust_sources(&src)
            .into_iter()
            .map(|source| {
                let relative = relative_source_path(&src, &source);
                let lines = read(&source).lines().count();
                (relative, lines)
            })
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            config.modules.keys().collect::<BTreeSet<_>>(),
            measured.keys().collect::<BTreeSet<_>>(),
            "{crate_name} module ceilings must list exactly its Rust source files"
        );
        for (module, lines) in measured {
            let ceiling = config.modules[&module];
            assert!(
                lines <= ceiling,
                "{crate_name}/{module} grew to {lines} lines past its exact ceiling {ceiling}"
            );
        }
        validate_migration_targets(crate_name, &config).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn missing_migration_target_is_rejected() {
    let config = CeilingsFile {
        public_root_budget: 0,
        migration_targets: BTreeMap::new(),
        modules: BTreeMap::from([("engine.rs".to_owned(), 1)]),
    };
    assert!(validate_migration_targets("gateway-stt-engine", &config).is_err());
}

#[test]
fn misspelled_migration_section_is_rejected() {
    let malformed = r#"
        public_root_budget = 2

        [migration_targtes]

        [modules]
        "lib.rs" = 1
    "#;

    assert!(parse_ceilings(malformed).is_err());
}

#[test]
fn gateway_step_15_migrations_are_pinned_to_their_destinations() {
    let expected = expected_migration_targets("gateway-stt");
    assert_eq!(
        expected["api.rs"],
        MigrationTarget {
            target_step: "Step 15".to_owned(),
            destination: "batch.rs".to_owned(),
        }
    );
    assert_eq!(
        expected["runtime.rs"],
        MigrationTarget {
            target_step: "Step 15".to_owned(),
            destination: "service.rs, artifacts.rs, generation.rs, status.rs, and model.rs"
                .to_owned(),
        }
    );
}

#[test]
fn compiler_unsafe_lints_cover_the_stt_stack() {
    let workspace: toml::Value = toml::from_str(&read(&workspace_root().join("Cargo.toml")))
        .unwrap_or_else(|error| panic!("workspace Cargo.toml must parse: {error}"));
    assert_eq!(
        workspace["workspace"]["lints"]["rust"]["unsafe_code"].as_str(),
        Some("forbid"),
        "the workspace compiler lint must forbid unsafe code"
    );

    for crate_name in [
        "gateway-stt",
        "gateway-stt-engine",
        "gateway-stt-backend-whisper",
    ] {
        let manifest: toml::Value =
            toml::from_str(&read(&crate_root(crate_name).join("Cargo.toml")))
                .unwrap_or_else(|error| panic!("{crate_name} Cargo.toml must parse: {error}"));
        assert_eq!(
            manifest["lints"]["workspace"].as_bool(),
            Some(true),
            "{crate_name} must inherit the workspace unsafe compiler lint"
        );
    }

    let ffi: toml::Value =
        toml::from_str(&read(&crate_root("gateway-whisper-ffi").join("Cargo.toml")))
            .unwrap_or_else(|error| panic!("gateway-whisper-ffi Cargo.toml must parse: {error}"));
    assert_eq!(
        ffi["lints"]["rust"]["unsafe_code"].as_str(),
        Some("deny"),
        "the FFI leaf must deny unsafe code outside its explicit expectations"
    );
}
