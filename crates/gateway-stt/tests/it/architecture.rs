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

const PUBLIC_ROOT_COUNTS: [(&str, usize); 4] = [
    ("gateway-stt", 6),
    ("gateway-stt-engine", 7),
    ("gateway-stt-backend-whisper", 2),
    ("gateway-whisper-ffi", 6),
];

const TEST_FIXTURE_PUBLIC_ROOT_COUNTS: [(&str, usize); 2] =
    [("gateway-stt", 7), ("gateway-stt-engine", 8)];

const LEGACY_WORKSHOP_UI_SPEECH_SEAMS: [&str; 6] = [
    "setupLegacyStt",
    "sttCapability",
    "interface StreamFrame",
    "interface InterimFrame",
    "interface FinalFrame",
    "pcm-capture",
];

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
            "gateway-stt-engine",
            "gateway-web-search",
            "promptforge-core",
            "shared-loopback",
            "shared-progress",
            "shared-protocol",
            "shared-sidecar",
        ],
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
    },
    DependencyPolicy {
        crate_name: "gateway-stt-engine",
        final_edges: &[],
    },
    DependencyPolicy {
        crate_name: "gateway-stt-backend-whisper",
        final_edges: &[
            "gateway-stt-engine",
            "gateway-whisper-ffi",
            "shared-progress",
        ],
    },
    DependencyPolicy {
        crate_name: "gateway-whisper-ffi",
        final_edges: &[],
    },
    DependencyPolicy {
        crate_name: "shared-loopback",
        final_edges: &[],
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
    },
];

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CeilingsFile {
    public_root_count: usize,
    test_fixture_public_root_count: Option<usize>,
    modules: BTreeMap<String, usize>,
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
            "dependency policies must cover the exact final crates: expected {expected:?}, got \
             {actual:?}"
        ));
    }
    Ok(())
}

fn dependency_drift_message(crate_name: &str) -> String {
    format!("{crate_name} workspace edges drifted from the exact final allowlist")
}

#[test]
fn workspace_dependencies_match_exact_final_allowlists() {
    let metadata = workspace_metadata();
    validate_dependency_policies(&DEPENDENCY_POLICIES).unwrap_or_else(|error| panic!("{error}"));
    for policy in &DEPENDENCY_POLICIES {
        let allowed = policy
            .final_edges
            .iter()
            .copied()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            crate_workspace_edges(metadata, policy.crate_name),
            allowed,
            "{}",
            dependency_drift_message(policy.crate_name)
        );
    }
}

#[test]
fn dependency_drift_diagnostic_names_the_final_invariant() {
    assert_eq!(
        dependency_drift_message("gateway-stt"),
        "gateway-stt workspace edges drifted from the exact final allowlist"
    );
}

#[test]
fn dependency_policy_omission_is_rejected() {
    assert!(
        validate_dependency_policies(&DEPENDENCY_POLICIES[..DEPENDENCY_POLICIES.len() - 1])
            .is_err()
    );
}

#[test]
fn dependency_policy_is_final_without_workshop_back_edges() {
    let workshop = DEPENDENCY_POLICIES
        .iter()
        .find(|policy| policy.crate_name == "workshop-server")
        .unwrap_or_else(|| panic!("final policy contains the Workshop dependency policy"));
    assert!(
        workshop.final_edges.contains(&"shared-loopback"),
        "final policy retains the Workshop dependency on shared-loopback"
    );
    assert!(
        !DEPENDENCY_POLICIES
            .iter()
            .find(|policy| policy.crate_name == "gateway-stt")
            .unwrap_or_else(|| panic!("final policy contains gateway-stt"))
            .final_edges
            .contains(&"workshop-server"),
        "final policy forbids the Gateway STT to Workshop dependency"
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

fn expected_public_root_count(crate_name: &str) -> usize {
    PUBLIC_ROOT_COUNTS
        .iter()
        .find_map(|(name, count)| (*name == crate_name).then_some(*count))
        .unwrap_or_else(|| panic!("public-root policy must cover {crate_name}"))
}

fn expected_test_fixture_public_root_count(crate_name: &str) -> Option<usize> {
    TEST_FIXTURE_PUBLIC_ROOT_COUNTS
        .iter()
        .find_map(|(name, count)| (*name == crate_name).then_some(*count))
}

fn validate_module_ceiling(lines: usize, ceiling: usize) -> Result<(), String> {
    if lines != ceiling {
        return Err(format!(
            "measured {lines} physical lines but the exact ceiling is {ceiling}"
        ));
    }
    if lines > 500 {
        return Err(format!(
            "final module has {lines} physical lines above the 500-line limit"
        ));
    }
    Ok(())
}

#[test]
fn final_module_ceilings_cover_every_source() {
    for crate_name in STT_CRATES {
        let src = crate_root(crate_name).join("src");
        let config = ceilings(crate_name);
        assert_eq!(
            config.public_root_count,
            expected_public_root_count(crate_name),
            "{crate_name} public root count drifted from the exact final policy"
        );
        assert_eq!(
            config.test_fixture_public_root_count,
            expected_test_fixture_public_root_count(crate_name),
            "{crate_name} test-fixtures public root count drifted from the exact final policy"
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
            validate_module_ceiling(lines, ceiling).unwrap_or_else(|error| {
                panic!("{crate_name}/{module} violates its source policy: {error}")
            });
        }
    }
}

#[test]
fn exact_module_ceiling_policy_rejects_spare_growth_and_every_oversize() {
    assert!(validate_module_ceiling(499, 500).is_err());
    assert!(validate_module_ceiling(501, 501).is_err());
    assert!(validate_module_ceiling(500, 500).is_ok());
}

#[test]
fn stale_migration_section_is_rejected() {
    let malformed = r#"
        public_root_count = 2

        [migration_targets]

        [modules]
        "lib.rs" = 1
    "#;

    assert!(parse_ceilings(malformed).is_err());
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

fn blank_rust_non_code(masked: &mut [u8], start: usize, end: usize) {
    for byte in &mut masked[start..end] {
        if !matches!(*byte, b'\n' | b'\r') {
            *byte = b' ';
        }
    }
}

fn rust_raw_string_end(source: &[u8], start: usize) -> Option<usize> {
    let mut cursor = start;
    if source.get(cursor) == Some(&b'b') {
        cursor += 1;
    }
    if source.get(cursor) != Some(&b'r') {
        return None;
    }
    cursor += 1;
    let hashes_start = cursor;
    while source.get(cursor) == Some(&b'#') {
        cursor += 1;
    }
    let hashes = cursor - hashes_start;
    if source.get(cursor) != Some(&b'"') {
        return None;
    }
    cursor += 1;
    while cursor < source.len() {
        if source[cursor] == b'"'
            && source.get(cursor + 1..cursor + 1 + hashes)
                == Some(&source[hashes_start..hashes_start + hashes])
        {
            return Some(cursor + 1 + hashes);
        }
        cursor += 1;
    }
    Some(source.len())
}

fn rust_quoted_end(source: &[u8], start: usize, quote: u8) -> usize {
    let mut cursor = start + 1;
    while cursor < source.len() {
        match source[cursor] {
            b'\\' => cursor = (cursor + 2).min(source.len()),
            byte if byte == quote => return cursor + 1,
            _ => cursor += 1,
        }
    }
    source.len()
}

fn rust_char_literal_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = start + 1;
    match *bytes.get(cursor)? {
        b'\\' => {
            cursor += 1;
            match *bytes.get(cursor)? {
                b'x' => cursor += 3,
                b'u' if bytes.get(cursor + 1) == Some(&b'{') => {
                    cursor += 2;
                    while bytes.get(cursor) != Some(&b'}') {
                        cursor += 1;
                        if cursor >= bytes.len() {
                            return None;
                        }
                    }
                    cursor += 1;
                }
                _ => cursor += 1,
            }
        }
        _ => {
            cursor += source[cursor..].chars().next()?.len_utf8();
        }
    }
    (bytes.get(cursor) == Some(&b'\'')).then_some(cursor + 1)
}

fn mask_rust_non_code(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut masked = bytes.to_vec();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes.get(cursor..cursor + 2) == Some(b"//") {
            let start = cursor;
            cursor += 2;
            while !matches!(bytes.get(cursor), None | Some(b'\n')) {
                cursor += 1;
            }
            blank_rust_non_code(&mut masked, start, cursor);
        } else if bytes.get(cursor..cursor + 2) == Some(b"/*") {
            let start = cursor;
            cursor += 2;
            let mut depth = 1_usize;
            while cursor < bytes.len() && depth > 0 {
                if bytes.get(cursor..cursor + 2) == Some(b"/*") {
                    depth += 1;
                    cursor += 2;
                } else if bytes.get(cursor..cursor + 2) == Some(b"*/") {
                    depth -= 1;
                    cursor += 2;
                } else {
                    cursor += 1;
                }
            }
            blank_rust_non_code(&mut masked, start, cursor);
        } else if let Some(end) = rust_raw_string_end(bytes, cursor) {
            blank_rust_non_code(&mut masked, cursor, end);
            cursor = end;
        } else if bytes[cursor] == b'"' {
            let end = rust_quoted_end(bytes, cursor, b'"');
            blank_rust_non_code(&mut masked, cursor, end);
            cursor = end;
        } else if bytes[cursor] == b'\'' {
            if let Some(end) = rust_char_literal_end(source, cursor) {
                blank_rust_non_code(&mut masked, cursor, end);
                cursor = end;
            } else {
                cursor += 1;
            }
        } else {
            cursor += 1;
        }
    }
    String::from_utf8(masked).unwrap_or_else(|error| panic!("masking must preserve UTF-8: {error}"))
}

fn compact_rust_code(source: &str) -> String {
    mask_rust_non_code(source)
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

fn production_rust_code(source: &str) -> String {
    let mut code = compact_rust_code(source);
    if let Some(tests) = code.rfind("#[cfg(test)]modtests{") {
        code.truncate(tests);
    }
    code
}

fn matching_delimiter(source: &str, open: usize, opening: u8, closing: u8) -> Option<usize> {
    let mut depth = 0_usize;
    for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
        if *byte == opening {
            depth += 1;
        } else if *byte == closing {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(open + offset);
            }
        }
    }
    None
}

fn braced_body_after<'a>(source: &'a str, marker: &str) -> Result<&'a str, String> {
    let marker = source
        .find(marker)
        .ok_or_else(|| format!("missing `{marker}`"))?;
    let open = source[marker..]
        .find('{')
        .map(|offset| marker + offset)
        .ok_or_else(|| format!("`{marker}` has no body"))?;
    let close = matching_delimiter(source, open, b'{', b'}')
        .ok_or_else(|| format!("`{marker}` has an unbalanced body"))?;
    Ok(&source[open + 1..close])
}

fn split_top_level(source: &str, delimiter: u8) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut round = 0_usize;
    let mut square = 0_usize;
    let mut curly = 0_usize;
    let mut angle = 0_usize;
    for (index, byte) in source.bytes().enumerate() {
        match byte {
            b'(' => round += 1,
            b')' => round = round.saturating_sub(1),
            b'[' => square += 1,
            b']' => square = square.saturating_sub(1),
            b'{' => curly += 1,
            b'}' => curly = curly.saturating_sub(1),
            b'<' => angle += 1,
            b'>' => angle = angle.saturating_sub(1),
            byte if byte == delimiter && round == 0 && square == 0 && curly == 0 && angle == 0 => {
                parts.push(&source[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if start < source.len() {
        parts.push(&source[start..]);
    }
    parts
}

fn strip_attributes(mut field: &str) -> Result<&str, String> {
    while field.starts_with("#[") {
        let close = matching_delimiter(field, 1, b'[', b']')
            .ok_or_else(|| format!("unbalanced field attribute in `{field}`"))?;
        field = &field[close + 1..];
    }
    Ok(field)
}

fn struct_field_types(source: &str, name: &str) -> Result<BTreeSet<String>, String> {
    let body = braced_body_after(source, &format!("struct{name}"))?;
    split_top_level(body, b',')
        .into_iter()
        .filter(|field| !field.is_empty())
        .map(|field| {
            let field = strip_attributes(field)?;
            let colon = field
                .find(':')
                .ok_or_else(|| format!("`{name}` field `{field}` has no type"))?;
            Ok(field[colon + 1..].to_owned())
        })
        .collect()
}

const TRANSACTION_RESOURCE_TYPES: [&str; 13] = [
    "AppState",
    "ProfileName",
    "ProgressTree",
    "SwitchTarget",
    "StopSet",
    "PreparedPersistence",
    "PriorRuntimeSnapshot",
    "StagedTarget",
    "RuntimeReplacement",
    "CancellationToken",
    "Routing",
    "StartReport",
    "GatewayError",
];

fn require_exact_resources(source: &str, owner: &str, expected: &[&str]) -> Result<(), String> {
    let fields = struct_field_types(source, owner)?;
    let actual = fields
        .into_iter()
        .filter(|field| TRANSACTION_RESOURCE_TYPES.contains(&field.as_str()))
        .collect::<BTreeSet<_>>();
    let expected = expected
        .iter()
        .copied()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{owner} transaction ownership must be exactly {expected:?}, got {actual:?}"
        ))
    }
}

fn method_body<'a>(source: &'a str, owner: &str, signature: &str) -> Result<&'a str, String> {
    let implementation = braced_body_after(source, &format!("impl{owner}"))?;
    braced_body_after(implementation, signature)
}

fn require_order(source: &str, markers: &[&str], invariant: &str) -> Result<(), String> {
    let mut cursor = 0;
    for marker in markers {
        let position = source[cursor..]
            .find(marker)
            .ok_or_else(|| format!("{invariant} must retain ordered `{marker}`"))?;
        cursor += position + marker.len();
    }
    Ok(())
}

fn is_single_awaited_call(body: &str, callee: &str) -> bool {
    let prefix = format!("{callee}(");
    if !body.starts_with(&prefix) {
        return false;
    }
    let open = prefix.len() - 1;
    matching_delimiter(body, open, b'(', b')')
        .is_some_and(|close| body.get(close + 1..) == Some(".await"))
}

fn validate_transaction_phase_ownership(profile: &str) -> Result<(), String> {
    for (owner, resources) in [
        (
            "PreparedPhase",
            &[
                "AppState",
                "ProfileName",
                "ProgressTree",
                "SwitchTarget",
                "StopSet",
                "PreparedPersistence",
                "CancellationToken",
            ][..],
        ),
        (
            "CutoverPhase",
            &[
                "AppState",
                "ProfileName",
                "ProgressTree",
                "SwitchTarget",
                "PreparedPersistence",
                "PriorRuntimeSnapshot",
                "CancellationToken",
            ],
        ),
        (
            "CutoverOwner",
            &[
                "AppState",
                "ProfileName",
                "StagedTarget",
                "PreparedPersistence",
                "PriorRuntimeSnapshot",
                "CancellationToken",
            ],
        ),
        (
            "StagedPhase",
            &[
                "AppState",
                "ProfileName",
                "StagedTarget",
                "RuntimeReplacement",
                "PreparedPersistence",
                "PriorRuntimeSnapshot",
                "CancellationToken",
            ],
        ),
        (
            "CommitTail",
            &[
                "AppState",
                "ProfileName",
                "StagedTarget",
                "RuntimeReplacement",
                "PriorRuntimeSnapshot",
                "CancellationToken",
            ],
        ),
        (
            "PublicationPhase",
            &[
                "AppState",
                "ProfileName",
                "StagedTarget",
                "RuntimeReplacement",
                "CancellationToken",
                "Routing",
            ],
        ),
    ] {
        require_exact_resources(profile, owner, resources)?;
    }
    Ok(())
}

fn validate_terminal_ownership(profile: &str) -> Result<(), String> {
    for (owner, resources) in [
        ("CommittedPhase", &["StartReport"][..]),
        ("RolledBackPhase", &["GatewayError"]),
        ("IndeterminatePhase", &["GatewayError"]),
        (
            "RollbackOwner",
            &[
                "AppState",
                "PriorRuntimeSnapshot",
                "CancellationToken",
                "GatewayError",
            ],
        ),
    ] {
        require_exact_resources(profile, owner, resources)?;
    }
    let terminal = braced_body_after(profile, "enumTerminalPhase")?;
    if terminal
        != "Committed(CommittedPhase),RolledBack(RolledBackPhase),Indeterminate(IndeterminatePhase),"
    {
        return Err(format!(
            "TerminalPhase must exactly own committed, rolled-back, and indeterminate outcomes, \
             got `{terminal}`"
        ));
    }
    Ok(())
}

fn validate_preparation_and_staging(profile: &str) -> Result<(), String> {
    let cutover = method_body(
        profile,
        "PreparedPhase",
        "asyncfncut_over(self)->Result<CutoverPhase,TerminalPhase>",
    )?;
    require_order(
        cutover,
        &[
            "capture_runtime_snapshot(&self.state).await",
            "cut_over(&self.state,&self.target,&self.tree,self.stop,&self.token,).await",
            "self.roll_back(prior,error).await",
            "CutoverPhase{",
        ],
        "prepared-to-cutover transition",
    )?;

    let stage = method_body(
        profile,
        "CutoverPhase",
        "asyncfnstage(self)->Result<StagedPhase,TerminalPhase>",
    )?;
    require_order(
        stage,
        &[
            "ifself.token.is_cancelled(){",
            "letSome(deadline)=",
            "letowner=CutoverOwner{",
            "spawn_runtimes(",
            "letstaged=owner.into_staged(replacement);",
            "ifstaged.token.is_cancelled(){",
            "staged.roll_back_after_stage(switch_cancelled).await",
        ],
        "cutover-to-staged cancellation and ownership",
    )?;

    let prepare = braced_body_after(profile, "pub(super)asyncfnprepare(")?;
    require_order(
        prepare,
        &[
            "iftoken.is_cancelled(){",
            "prepare_target(",
            "iftoken.is_cancelled(){",
            "download_artifacts(",
            "iftoken.is_cancelled(){",
            "PreparedPersistence::prepare(",
            "iftoken.is_cancelled(){",
            "Ok(PreparedPhase{",
        ],
        "preparation cancellation and persistence ownership",
    )
}

fn validate_commit_and_publication(profile: &str) -> Result<(), String> {
    let commit = method_body(profile, "StagedPhase", "asyncfncommit(self)->TerminalPhase")?;
    require_order(
        commit,
        &[
            "ifself.token.is_cancelled(){",
            "letpublication_state=state.clone();",
            "()=self.token.cancelled()=>",
            "ifself.token.is_cancelled(){",
            "letStagedPhase{",
            "matchpersistence.commit().await{",
            "PersistenceCommitError::Determinate(error)",
            "tail.into_rollback(error)",
            "PersistenceCommitError::Indeterminate(error)",
            "tail.into_indeterminate(",
            "letpublication=tail.into_publication(routing);",
            "publication.publish().await",
        ],
        "persistence-before-publication and commit cancellation",
    )?;
    if commit.matches("into_publication(").count() != 1 {
        return Err(
            "persistence-before-publication requires one consuming publication transition"
                .to_owned(),
        );
    }

    method_body(
        profile,
        "CommitTail",
        "fninto_publication(self,routing:Routing)->PublicationPhase",
    )?;
    method_body(
        profile,
        "PublicationPhase",
        "asyncfnpublish(self)->TerminalPhase",
    )?;
    method_body(
        profile,
        "TerminalPhase",
        "fnfinish(self)->Result<StartReport,GatewayError>",
    )?;
    Ok(())
}

fn validate_rollback_reconstruction(profile: &str, generation: &str) -> Result<(), String> {
    let restore = braced_body_after(profile, "asyncfnrestore_runtime_snapshot(")?;
    require_order(
        restore,
        &[
            "LocalRuntime::start(",
            "state.live.write().await",
            "live.routing=prior.routing",
            "live.config=prior.config",
            "live.profile_name=prior.profile_name",
            "live.model_allowlist=prior.model_allowlist",
            "live.loading=prior.loading",
        ],
        "prior runtime reconstruction before republication",
    )?;

    let rollback = method_body(
        profile,
        "RollbackOwner",
        "asyncfnfinish(self)->TerminalPhase",
    )?;
    require_order(
        rollback,
        &[
            "restore_runtime_snapshot(&self.state,self.prior).await",
            "request_fatal_shutdown(",
        ],
        "rollback reconstruction failure escalation",
    )?;
    let runtime_rollback = braced_body_after(profile, "fnrollback_runtime(")?;
    if !runtime_rollback.contains("abort_replacement(replacement.speech)") {
        return Err("staged rollback must reconstruct the retired speech generation".to_owned());
    }

    let replacement = struct_field_types(generation, "SpeechReplacement")?;
    for owned in [
        "Weak<Shared>",
        "Option<Generation>",
        "Option<GenerationSpec>",
        "ReplacementPermit",
    ] {
        if !replacement.contains(owned) {
            return Err(format!(
                "SpeechReplacement must own restartable rollback resource `{owned}`"
            ));
        }
    }
    let speech_rollback = method_body(
        generation,
        "SpeechReplacement",
        "fnrollback(&mutself)->Result<(),SpeechError>",
    )?;
    if !speech_rollback.contains("restore_generation(&owner,&self.permit,&rollback)") {
        return Err("speech rollback must reconstruct its retired generation".to_owned());
    }
    let speech_drop = method_body(generation, "DropforSpeechReplacement", "fndrop(&mutself)")?;
    if !speech_drop.contains("self.rollback()") {
        return Err("dropped speech replacement must roll back its owned generation".to_owned());
    }
    Ok(())
}

fn validate_fatal_indeterminate_shutdown(profile: &str) -> Result<(), String> {
    let fatal = braced_body_after(profile, "pub(super)fnrequest_fatal_shutdown(")?;
    require_order(
        fatal,
        &[
            "token.cancel();",
            "state.shutdown.fire();",
            "state.speech.shutdown();",
            "GatewayError::switch_failed(",
        ],
        "fatal indeterminate shutdown",
    )?;

    let mut cursor = 0;
    let mut constructors = 0;
    while let Some(relative) = profile[cursor..].find("IndeterminatePhase{") {
        let start = cursor + relative;
        let open = start + "IndeterminatePhase".len();
        cursor = open + 1;
        if profile[..start].ends_with("struct") {
            continue;
        }
        let close = matching_delimiter(profile, open, b'{', b'}')
            .ok_or_else(|| "indeterminate phase construction must be balanced".to_owned())?;
        if !profile[open + 1..close].starts_with("error:request_fatal_shutdown(") {
            return Err(
                "every indeterminate outcome must be constructed through fatal shutdown".to_owned(),
            );
        }
        constructors += 1;
    }
    if constructors == 0 {
        return Err("transaction must construct fatal indeterminate outcomes".to_owned());
    }
    Ok(())
}

fn validate_root_transaction_delegation(root: &str) -> Result<(), String> {
    let root_delegate = braced_body_after(root, "asyncfnrun_switch_with_config(")?;
    if !is_single_awaited_call(root_delegate, "profile_switch::run") {
        return Err(
            "Gateway root must delegate profile switching as one awaited transaction call"
                .to_owned(),
        );
    }
    Ok(())
}

fn validate_profile_replacement_architecture(
    profile_switch: &str,
    gateway_root: &str,
    speech_generation: &str,
) -> Result<(), String> {
    let profile = production_rust_code(profile_switch);
    let root = production_rust_code(gateway_root);
    let generation = production_rust_code(speech_generation);
    validate_transaction_phase_ownership(&profile)?;
    validate_terminal_ownership(&profile)?;
    validate_preparation_and_staging(&profile)?;
    validate_commit_and_publication(&profile)?;
    validate_rollback_reconstruction(&profile, &generation)?;
    validate_fatal_indeterminate_shutdown(&profile)?;
    validate_root_transaction_delegation(&root)
}

#[test]
fn profile_replacement_policy_requires_restartable_rollback_and_fatal_shutdown() {
    let generation = read(&crate_root("gateway-stt").join("src/generation.rs"));
    let gateway = read(&crate_root("gateway").join("src/lib.rs"));
    let profile_switch = read(&crate_root("gateway").join("src/profile_switch.rs"));

    validate_profile_replacement_architecture(&profile_switch, &gateway, &generation)
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn profile_replacement_architecture_rejects_adversarial_mutations() {
    let generation = read(&crate_root("gateway-stt").join("src/generation.rs"));
    let gateway = read(&crate_root("gateway").join("src/lib.rs"));
    let profile_switch = read(&crate_root("gateway").join("src/profile_switch.rs"));
    let mutation = |source: &str, from: &str, to: &str| {
        assert!(
            source.contains(from),
            "mutation fixture must contain `{from}`"
        );
        source.replacen(from, to, 1)
    };

    let ownership = mutation(
        &profile_switch,
        "    persistence: PreparedPersistence,\n",
        "    persistence: Arc<PreparedPersistence>,\n",
    );
    assert!(
        validate_profile_replacement_architecture(&ownership, &gateway, &generation)
            .is_err_and(|error| error.contains("PreparedPhase transaction ownership"))
    );

    let early_publication = mutation(
        &profile_switch,
        "        match persistence.commit().await {",
        "        let _premature = tail.into_publication(routing);\n\
         match persistence.commit().await {",
    );
    assert!(
        validate_profile_replacement_architecture(&early_publication, &gateway, &generation)
            .is_err_and(|error| error.contains("persistence-before-publication"))
    );

    let cancelled_boundary = mutation(
        &profile_switch,
        "        if self.token.is_cancelled() {\n            let error = switch_cancelled(&self.name);",
        "        if false {\n            let error = switch_cancelled(&self.name);",
    );
    assert!(
        validate_profile_replacement_architecture(&cancelled_boundary, &gateway, &generation)
            .is_err_and(|error| error.contains("cutover-to-staged cancellation"))
    );

    let nonfatal = mutation(
        &profile_switch,
        "    state.shutdown.fire();",
        "    // state.shutdown.fire();",
    );
    assert!(
        validate_profile_replacement_architecture(&nonfatal, &gateway, &generation)
            .is_err_and(|error| error.contains("fatal indeterminate shutdown"))
    );

    let unowned_terminal = mutation(
        &profile_switch,
        "    Indeterminate(IndeterminatePhase),",
        "    Indeterminate(GatewayError),",
    );
    assert!(
        validate_profile_replacement_architecture(&unowned_terminal, &gateway, &generation)
            .is_err_and(|error| error.contains("TerminalPhase must exactly own"))
    );

    let root_orchestration = mutation(
        &gateway,
        "    profile_switch::run(&state, name, tree, candidate, persistence, token).await",
        "    let _duplicate_owner = &state;\n\
         profile_switch::run(&state, name, tree, candidate, persistence, token).await",
    );
    assert!(
        validate_profile_replacement_architecture(
            &profile_switch,
            &root_orchestration,
            &generation
        )
        .is_err_and(|error| error.contains("Gateway root must delegate"))
    );

    let dropped_reconstruction = mutation(
        &generation,
        "restore_generation(&owner, &self.permit, &rollback)",
        "Ok(())",
    );
    assert!(
        validate_profile_replacement_architecture(
            &profile_switch,
            &gateway,
            &dropped_reconstruction
        )
        .is_err_and(|error| error.contains("speech rollback must reconstruct"))
    );
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
