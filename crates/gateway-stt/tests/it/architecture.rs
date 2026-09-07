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

const PUBLIC_ROOT_BUDGETS: [(&str, usize); 4] = [
    ("gateway-stt", 6),
    ("gateway-stt-engine", 7),
    ("gateway-stt-backend-whisper", 2),
    ("gateway-whisper-ffi", 6),
];

const LEGACY_WORKSHOP_UI_SPEECH_SEAMS: [&str; 6] = [
    "setupLegacyStt",
    "sttCapability",
    "interface StreamFrame",
    "interface InterimFrame",
    "interface FinalFrame",
    "pcm-capture",
];

const DEPENDENCY_PHASE: &str = "Phase C";
const DEPENDENCY_POLICY_CRATES: [&str; 7] = [
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
        temporary_edges: &[],
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
            "shared-loopback",
            "shared-progress",
            "shared-sidecar",
        ],
        temporary_edges: &[],
    },
];

const MIGRATION_POLICIES: [MigrationPolicy; 4] = [
    MigrationPolicy {
        crate_name: "gateway-stt",
        targets: &[],
    },
    MigrationPolicy {
        crate_name: "gateway-stt-engine",
        targets: &[],
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

fn forbidden_legacy_speech_seams<'a>(source: &str, forbidden: &'a [&'a str]) -> Vec<&'a str> {
    forbidden
        .iter()
        .copied()
        .filter(|symbol| source.contains(symbol))
        .collect()
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
    let expected = DEPENDENCY_POLICY_CRATES
        .into_iter()
        .collect::<BTreeSet<_>>();
    let actual = policies
        .iter()
        .map(|policy| policy.crate_name)
        .collect::<BTreeSet<_>>();
    if actual.len() != policies.len() {
        return Err("dependency policies contain a duplicate crate".to_owned());
    }
    if actual != expected {
        return Err(format!(
            "dependency policies must cover the exact {DEPENDENCY_PHASE} crates: expected \
             {expected:?}, got {actual:?}"
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
fn dependency_policy_has_advanced_to_phase_c() {
    assert_eq!(DEPENDENCY_PHASE, "Phase C");
    let workshop = DEPENDENCY_POLICIES
        .iter()
        .find(|policy| policy.crate_name == "workshop-server")
        .unwrap_or_else(|| panic!("Phase C contains the Workshop dependency policy"));
    assert!(
        workshop.final_edges.contains(&"shared-loopback"),
        "Phase C retains the Workshop dependency on shared-loopback"
    );
    assert!(
        DEPENDENCY_POLICIES
            .iter()
            .all(|policy| policy.temporary_edges.is_empty()),
        "Phase C has no temporary dependency exceptions"
    );
}

#[test]
fn legacy_speech_seams_are_absent_from_production_sources() {
    let gateway_stt = rust_sources(&crate_root("gateway-stt").join("src"))
        .into_iter()
        .map(|path| read(&path))
        .collect::<String>();
    for forbidden in [
        "mod stt;",
        "crate::stt::",
        "workshop_server",
        "workshop_status",
        "x-promptforge-workshop-status",
        "workshop_routes",
    ] {
        assert!(
            !gateway_stt.contains(forbidden),
            "gateway-stt production sources must not contain legacy seam `{forbidden}`"
        );
    }

    let workshop = rust_sources(&crate_root("workshop-server").join("src"))
        .into_iter()
        .map(|path| read(&path))
        .collect::<String>();
    for forbidden in [
        "pub(crate) mod stt;",
        "routes::stt",
        "GatewaySttSocket",
        "connect_stt",
        "workshop_status",
        "x-promptforge-workshop-status",
        "spawn_with_routes",
    ] {
        assert!(
            !workshop.contains(forbidden),
            "Workshop production sources must not contain legacy seam `{forbidden}`"
        );
    }

    let workshop_ui = [
        read(&crate_root("workshop-server").join("ui/src/ui/stt.ts")),
        read(&crate_root("workshop-server").join("ui/src/services/protocol.ts")),
        read(&crate_root("workshop-server").join("ui/pcm-worklet.js")),
    ]
    .concat();
    let forbidden = forbidden_legacy_speech_seams(&workshop_ui, &LEGACY_WORKSHOP_UI_SPEECH_SEAMS);
    assert!(
        forbidden.is_empty(),
        "Workshop UI production sources must not contain legacy seams: {forbidden:?}"
    );
}

#[test]
fn legacy_workshop_ui_gate_rejects_all_legacy_forms_without_current_capture_false_positives() {
    for forbidden in LEGACY_WORKSHOP_UI_SPEECH_SEAMS {
        assert_eq!(
            forbidden_legacy_speech_seams(forbidden, &LEGACY_WORKSHOP_UI_SPEECH_SEAMS),
            [forbidden],
            "the zero-symbol gate must reject legacy production symbol `{forbidden}`"
        );
    }

    let adversarial_pcm_forms = [
        r#"registerProcessor("pcm-capture", Processor);"#,
        r#"new AudioWorkletNode(context, "pcm-capture");"#,
        "`pcm-capture requires a 24 kHz AudioContext`",
    ];
    for source in adversarial_pcm_forms {
        assert_eq!(
            forbidden_legacy_speech_seams(source, &LEGACY_WORKSHOP_UI_SPEECH_SEAMS),
            ["pcm-capture"],
            "the zero-symbol gate must reject the processor ID in every production context"
        );
    }

    let retained_capture = [
        "pcm16-capture",
        "Pcm16CaptureProcessor",
        "RealtimeTranscriptionService",
        "SpeechCaptureService",
        "AudioWorkletNode",
        "getUserMedia",
    ]
    .join("\n");
    assert!(
        forbidden_legacy_speech_seams(&retained_capture, &LEGACY_WORKSHOP_UI_SPEECH_SEAMS)
            .is_empty(),
        "the zero-symbol gate must retain current Realtime and browser capture behavior"
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

fn expected_public_root_budget(crate_name: &str) -> usize {
    PUBLIC_ROOT_BUDGETS
        .iter()
        .find_map(|(name, budget)| (*name == crate_name).then_some(*budget))
        .unwrap_or_else(|| panic!("public-root policy must cover {crate_name}"))
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

fn validate_module_ceiling(
    lines: usize,
    ceiling: usize,
    settled_limit: Option<usize>,
) -> Result<(), String> {
    if lines != ceiling {
        return Err(format!(
            "measured {lines} physical lines but the exact ceiling is {ceiling}"
        ));
    }
    if let Some(limit) = settled_limit
        && lines > limit
    {
        return Err(format!(
            "settled module has {lines} physical lines above the {limit}-line limit"
        ));
    }
    Ok(())
}

#[test]
fn module_ceilings_cover_sources_and_name_migration_targets() {
    for crate_name in STT_CRATES {
        let src = crate_root(crate_name).join("src");
        let config = ceilings(crate_name);
        assert_eq!(
            config.public_root_budget,
            expected_public_root_budget(crate_name),
            "{crate_name} public root budget drifted from the exact phase policy"
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
            let settled_limit = (crate_name == "gateway-stt"
                && !config.migration_targets.contains_key(&module))
            .then_some(500);
            validate_module_ceiling(lines, ceiling, settled_limit).unwrap_or_else(|error| {
                panic!("{crate_name}/{module} violates its source policy: {error}")
            });
        }
        validate_migration_targets(crate_name, &config).unwrap_or_else(|error| panic!("{error}"));
    }
}

#[test]
fn exact_module_ceiling_policy_rejects_spare_growth_and_settled_oversize() {
    assert!(validate_module_ceiling(499, 500, Some(500)).is_err());
    assert!(validate_module_ceiling(501, 501, Some(500)).is_err());
    assert!(validate_module_ceiling(500, 500, Some(500)).is_ok());
    assert!(
        validate_module_ceiling(501, 501, None).is_ok(),
        "a named migration may retain an exact temporary oversize"
    );
}

#[test]
fn completed_engine_migration_targets_are_removed() {
    let config = CeilingsFile {
        public_root_budget: 7,
        migration_targets: BTreeMap::new(),
        modules: BTreeMap::from([("engine.rs".to_owned(), 1), ("worker.rs".to_owned(), 1)]),
    };
    assert!(validate_migration_targets("gateway-stt-engine", &config).is_ok());
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
fn completed_step_39_migrations_are_removed() {
    let expected = expected_migration_targets("gateway-stt");
    assert!(!expected.contains_key("api.rs"));
    assert!(!expected.contains_key("runtime.rs"));
    assert!(expected.is_empty());
}

const REFCOUNT_INTROSPECTION_OWNERS: [&str; 3] = ["Arc", "Rc", "Weak"];
const REFCOUNT_INTROSPECTION_METHODS: [&str; 11] = [
    "decrement_strong_count",
    "get_mut",
    "get_mut_unchecked",
    "increment_strong_count",
    "into_inner",
    "is_unique",
    "make_mut",
    "strong_count",
    "try_unwrap",
    "unwrap_or_clone",
    "weak_count",
];

fn calls_associated_method(source: &str, owner: &str, method: &str) -> bool {
    let compact = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let marker = format!("{owner}::");
    let mut remainder = compact.as_str();
    while let Some(position) = remainder.find(&marker) {
        let mut candidate = &remainder[position + marker.len()..];
        if let Some(generic) = candidate.strip_prefix('<') {
            let mut depth = 1_usize;
            let mut end = None;
            for (index, character) in generic.char_indices() {
                match character {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(index + character.len_utf8());
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end else {
                return false;
            };
            let Some(after_generic) = generic[end..].strip_prefix("::") else {
                return false;
            };
            candidate = after_generic;
        }
        if candidate.starts_with(&format!("{method}(")) {
            return true;
        }
        remainder = &remainder[position + marker.len()..];
    }
    false
}

fn refcount_introspection(source: &str) -> Option<(&'static str, &'static str)> {
    REFCOUNT_INTROSPECTION_OWNERS
        .into_iter()
        .flat_map(|owner| {
            REFCOUNT_INTROSPECTION_METHODS
                .into_iter()
                .map(move |method| (owner, method))
        })
        .find(|(owner, method)| calls_associated_method(source, owner, method))
}

#[test]
fn every_reference_count_introspection_form_is_rejected() {
    for owner in REFCOUNT_INTROSPECTION_OWNERS {
        for method in REFCOUNT_INTROSPECTION_METHODS {
            let direct = format!("let _ = {owner}::{method}(&value);");
            assert_eq!(refcount_introspection(&direct), Some((owner, method)));
            let generic = format!("let _ = {owner}::<Vec<u8>>::{method}(&value);");
            assert_eq!(refcount_introspection(&generic), Some((owner, method)));
        }
    }
    assert_eq!(refcount_introspection("Arc::clone(&value)"), None);
    assert_eq!(refcount_introspection("Arc::ptr_eq(&left, &right)"), None);
    assert_eq!(refcount_introspection("Weak::upgrade(&owner)"), None);
}

#[test]
fn generation_quiescence_uses_explicit_ownership_without_item_transfer() {
    let generation = read(&crate_root("gateway-stt").join("src/generation.rs"));
    let replacement = read(&crate_root("gateway-stt").join("src/replacement.rs"));
    for source in rust_sources(&crate_root("gateway-stt").join("src")) {
        let contents = read(&source);
        assert!(
            refcount_introspection(&contents).is_none(),
            "{} must not infer lifecycle ownership from reference counts",
            source.display()
        );
    }
    for policy in [
        "requests: usize",
        "jobs: usize",
        "struct SessionEpoch",
        "struct ReplacementCoordinator",
    ] {
        assert!(
            replacement.contains(policy),
            "replacement policy must retain {policy}"
        );
    }
    assert!(
        !generation.contains("CommittedItem") && !replacement.contains("CommittedItem"),
        "Realtime sessions retain committed-item failure ownership"
    );
}

#[test]
fn realtime_retirement_is_registry_owned_event_driven_and_keeps_state_pure() {
    let registry = read(&crate_root("gateway-stt").join("src/realtime/registry.rs"));
    let state = registry
        .split_once("struct RegistryState {")
        .and_then(|(_, rest)| rest.split_once('}'))
        .map_or_else(
            || panic!("Realtime registry must retain explicit state"),
            |(body, _)| body,
        );

    assert!(state.contains("active: usize"));
    for runtime_type in ["JoinHandle", "Notify", "AtomicUsize"] {
        assert!(
            !state.contains(runtime_type),
            "pure registry accounting must not contain {runtime_type}"
        );
    }
    for policy in [
        "tokio::spawn(async move",
        "task.abort();",
        "task.await",
        "join_retired_tasks(finalization_tasks)",
        "error.is_cancelled()",
        "record_retired_task_failures",
        "self.release_admission();",
        "self.cleanup.emit();",
    ] {
        assert!(
            registry.contains(policy),
            "registry-owned retirement must retain {policy}"
        );
    }
    for polling in ["noop_waker", "poll_join", "reap_retired"] {
        assert!(
            !registry.contains(polling),
            "registry retirement must not use scheduler polling through {polling}"
        );
    }
    let Some(release_position) = registry.find("self.release_admission();") else {
        panic!("registry retirement must release admission");
    };
    let Some(notification_position) = registry.find("self.cleanup.emit();") else {
        panic!("registry retirement must emit cleanup notification");
    };
    assert!(
        release_position < notification_position,
        "admission must release before cleanup notification"
    );

    let session_tests = read(&crate_root("gateway-stt").join("tests/it/realtime_session.rs"));
    let retirement_test = session_tests
        .split_once("async fn dropping_session_retains_admission_until_interim_cleanup_joins()")
        .and_then(|(_, rest)| rest.split_once("#[tokio::test]"))
        .map_or_else(
            || panic!("Realtime retirement regression must remain focused"),
            |(body, _)| body,
        );
    for evidence in ["cleanup_notified()", "tokio::time::timeout"] {
        assert!(
            retirement_test.contains(evidence),
            "Realtime retirement regression must retain {evidence}"
        );
    }
    assert!(
        !retirement_test.contains("wait_until(")
            && !retirement_test.contains("tokio::task::yield_now"),
        "Realtime retirement verification must not count scheduler yields"
    );

    for regression in [
        "cleanup_event_count()",
        "dropping_session_retains_admission_until_finalization_cleanup_joins",
        "retired_task_join_failures_are_preserved",
    ] {
        assert!(
            session_tests.contains(regression),
            "Realtime retirement regression must retain {regression}"
        );
    }
}

#[test]
fn profile_replacement_policy_requires_restartable_rollback_and_fatal_shutdown() {
    let generation = read(&crate_root("gateway-stt").join("src/generation.rs"));
    let gateway = read(&crate_root("gateway").join("src/lib.rs"));
    let persistence = read(&crate_root("gateway").join("src/config_write.rs"));

    for policy in [
        "rollback: Option<GenerationSpec>",
        "restore_generation",
        "impl Drop for SpeechReplacement",
    ] {
        assert!(
            generation.contains(policy),
            "speech replacement must retain {policy}"
        );
    }
    for policy in [
        "PreparedPersistence::prepare",
        "PersistenceCommitError::Determinate",
        "PersistenceCommitError::Indeterminate",
        "PROFILE_STAGE_TIMEOUT",
        "state.shutdown.fire()",
    ] {
        assert!(
            gateway.contains(policy),
            "Gateway transaction policy must retain {policy}"
        );
    }
    for policy in ["file.sync_all()", "std::fs::rename", "sync_parent"] {
        assert!(
            persistence.contains(policy),
            "profile persistence must retain {policy}"
        );
    }
}

#[test]
fn gateway_speech_discovery_uses_only_generic_facade_facts() {
    let gateway_root = crate_root("gateway").join("src");
    let gateway = read(&gateway_root.join("lib.rs"));
    let section = |start, end| {
        gateway
            .split_once(start)
            .and_then(|(_, rest)| rest.split_once(end))
            .map_or_else(
                || panic!("Gateway source must retain `{start}` before `{end}`"),
                |(body, _)| body,
            )
    };
    let model_info = read(&gateway_root.join("model_info.rs"));
    let system = read(&gateway_root.join("system.rs"));
    let discovery = [
        section("async fn list_models(", "#[derive(Debug, Deserialize)]"),
        section("async fn admin_status(", "async fn admin_queue_cancel("),
        model_info.as_str(),
        system.as_str(),
    ]
    .concat();
    let compact = discovery
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();

    for required in ["speech.status()", "speech.models()"] {
        assert!(
            compact.contains(required),
            "Gateway speech discovery must use facade fact `{required}`"
        );
    }
    for forbidden in [
        "stt_models()",
        "SttRole",
        "WorkshopSttConfig",
        "workshop_status",
    ] {
        assert!(
            !compact.contains(forbidden),
            "Gateway speech discovery must not depend on `{forbidden}`"
        );
    }
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
