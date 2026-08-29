use std::sync::Barrier;
use std::thread;

use tempfile::TempDir;

use super::*;
use crate::testsupport::hex_sha256;

const TOOLKIT_VERSION: &str = "13.3";
const TOOLKIT_DLL: &str = "cublas64_13.dll";
const SYSTEM_DLL: &str = "KERNEL32.dll";

/// A synthetic host: a cache root, a fake CUDA Toolkit with `cublas64_13.dll`
/// in `bin`, and a fake `System32` with `KERNEL32.dll`.
struct SyntheticHost {
    _temp: TempDir,
    cache: PathBuf,
    toolkit_root: PathBuf,
    system_root: PathBuf,
}

impl SyntheticHost {
    fn new() -> Self {
        let temp = TempDir::new().expect("tempdir");
        let cache = temp.path().join("cache");
        fs::create_dir(&cache).expect("cache dir");
        let toolkit_root = temp.path().join("cuda");
        let bin = toolkit_root.join("bin");
        fs::create_dir_all(&bin).expect("toolkit bin");
        fs::write(bin.join(TOOLKIT_DLL), b"fake-cublas").expect("toolkit dll");
        let system_root = temp.path().join("windows");
        let system32 = system_root.join("System32");
        fs::create_dir_all(&system32).expect("system32");
        fs::write(system32.join(SYSTEM_DLL), b"fake-kernel32").expect("system dll");
        Self {
            _temp: temp,
            cache,
            toolkit_root,
            system_root,
        }
    }

    fn env(&self) -> impl Fn(&str) -> Option<OsString> + '_ {
        move |name| match name {
            "CUDA_PATH_V13_3" => Some(self.toolkit_root.as_os_str().to_owned()),
            "SystemRoot" => Some(self.system_root.as_os_str().to_owned()),
            _ => None,
        }
    }

    fn install(&self) -> PathBuf {
        self.cache.join("llama.cpp").join(install_dir_name())
    }
}

fn bundle_files() -> Vec<(&'static str, &'static [u8])> {
    vec![
        ("ggml-cuda.dll", b"synthetic-ggml-cuda"),
        (SERVER_EXECUTABLE, b"synthetic-llama-server"),
    ]
}

/// Renders a canonical-shaped manifest for `files`, with the target, toolkit
/// version, linkage, and external DLL list of a real CUDA build.
fn manifest_json(files: &[(&str, &[u8])]) -> String {
    manifest_json_with(files, BUNDLE_TARGET, TOOLKIT_VERSION, EXPECTED_LINKAGE, 1)
}

fn manifest_json_with(
    files: &[(&str, &[u8])],
    target: &str,
    toolkit: &str,
    linkage: &str,
    format_version: u32,
) -> String {
    let entries: Vec<serde_json::Value> = files
        .iter()
        .map(|(name, bytes)| {
            serde_json::json!({
                "name": name,
                "sha256": hex_sha256(bytes),
                "size": bytes.len(),
            })
        })
        .collect();
    let manifest = serde_json::json!({
        "bundle_format_version": format_version,
        "source": {
            "url": "https://github.com/ggml-org/llama.cpp.git",
            "commit": "fb0e6b621917488d623437349fb5361e0ac21c70",
        },
        "target_triple": target,
        "host_triple": target,
        "toolkit_version": toolkit,
        "linkage": linkage,
        "external_dlls": [SYSTEM_DLL, TOOLKIT_DLL],
        "files": entries,
    });
    format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).expect("render manifest")
    )
}

fn payload<'a>(manifest: &'a str, files: &'a [(&'a str, &'a [u8])]) -> BundlePayload<'a> {
    BundlePayload { manifest, files }
}

fn stage(
    host: &SyntheticHost,
    manifest: &str,
    files: &[(&str, &[u8])],
) -> Result<StagedCudaBundle> {
    stage_bundle(&host.cache, &payload(manifest, files), &host.env())
}

#[test]
fn stages_and_publishes_a_valid_bundle() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);
    let staged = stage(&host, &manifest, &files).expect("stage bundle");

    let install = host.install();
    assert_eq!(staged.executable, install.join(SERVER_EXECUTABLE));
    assert_eq!(
        staged.path_prefix,
        vec![install.clone(), host.toolkit_root.join("bin")]
    );
    assert_eq!(
        fs::read(install.join(SERVER_EXECUTABLE)).expect("read staged exe"),
        b"synthetic-llama-server"
    );
    assert_eq!(
        fs::read(install.join("ggml-cuda.dll")).expect("read staged dll"),
        b"synthetic-ggml-cuda"
    );
    assert!(install.join(INSTALL_MARKER).is_file());
    assert!(!part_path(&install).exists());
}

#[test]
fn cache_hit_returns_without_restaging() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);
    let first = stage(&host, &manifest, &files).expect("first stage");

    // A sentinel at the staging path proves the second call never restaged:
    // restaging begins by removing the `.part` sibling.
    let sentinel = part_path(&host.install());
    fs::write(&sentinel, b"sentinel").expect("plant sentinel");
    let marker_before = fs::read(host.install().join(INSTALL_MARKER)).expect("read marker");

    let second = stage(&host, &manifest, &files).expect("cache hit");
    assert_eq!(first, second);
    assert_eq!(fs::read(&sentinel).expect("sentinel survives"), b"sentinel");
    assert_eq!(
        fs::read(host.install().join(INSTALL_MARKER)).expect("marker"),
        marker_before
    );
}

#[test]
fn tampered_payload_digest_is_rejected_before_any_staging() {
    let host = SyntheticHost::new();
    let manifest = manifest_json(&bundle_files());
    let tampered: Vec<(&str, &[u8])> = vec![
        ("ggml-cuda.dll", b"synthetic-ggml-cuda"),
        // Same length as the real bytes, so the digest check (not the size
        // check) is what fires.
        (SERVER_EXECUTABLE, b"synthetic-llama-SERVER"),
    ];
    let error = stage(&host, &manifest, &tampered).expect_err("tampering must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::DigestMismatch { ref name, .. }) if name == SERVER_EXECUTABLE
        ),
        "unexpected error: {error}"
    );
    assert!(!host.install().exists());
    assert!(!part_path(&host.install()).exists());
}

#[test]
fn target_mismatch_is_rejected() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json_with(
        &files,
        "aarch64-pc-windows-msvc",
        TOOLKIT_VERSION,
        EXPECTED_LINKAGE,
        1,
    );
    let error = stage(&host, &manifest, &files).expect_err("wrong target must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::TargetMismatch { .. })
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn manifest_schema_violations_are_rejected() {
    let host = SyntheticHost::new();
    let files = bundle_files();

    // Missing required field: the JSON does not decode into the schema.
    let incomplete = serde_json::json!({ "bundle_format_version": 1 }).to_string();
    let error = stage(&host, &incomplete, &files).expect_err("incomplete manifest must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::ManifestDecode(_))
        ),
        "unexpected error: {error}"
    );

    let future = manifest_json_with(&files, BUNDLE_TARGET, TOOLKIT_VERSION, EXPECTED_LINKAGE, 2);
    let error = stage(&host, &future, &files).expect_err("future format must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::UnsupportedFormat { found: 2 })
        ),
        "unexpected error: {error}"
    );

    let dynamic = manifest_json_with(&files, BUNDLE_TARGET, TOOLKIT_VERSION, "dynamic", 1);
    let error = stage(&host, &dynamic, &files).expect_err("wrong linkage must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::UnexpectedLinkage { .. })
        ),
        "unexpected error: {error}"
    );

    let no_server: Vec<(&str, &[u8])> = vec![("ggml-cuda.dll", b"synthetic-ggml-cuda")];
    let manifest = manifest_json(&no_server);
    let error = stage(&host, &manifest, &no_server).expect_err("missing executable must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::MissingExecutable)
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn unsafe_or_malformed_manifest_entries_are_rejected() {
    let host = SyntheticHost::new();

    // A multi-component name would escape the flat staging directory.
    let traversal: Vec<(&str, &[u8])> = vec![
        ("sub/evil.dll", b"evil"),
        (SERVER_EXECUTABLE, b"synthetic-llama-server"),
    ];
    let manifest = manifest_json(&traversal);
    let error = stage(&host, &manifest, &traversal).expect_err("traversal name must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::UnsafeFileName { .. })
        ),
        "unexpected error: {error}"
    );

    // A digest that is not 64 lowercase hex characters never reaches a compare.
    let files = bundle_files();
    let mut manifest: serde_json::Value =
        serde_json::from_str(&manifest_json(&files)).expect("parse manifest");
    manifest["files"][0]["sha256"] = serde_json::json!("not-hex");
    let manifest = manifest.to_string();
    let error = stage(&host, &manifest, &files).expect_err("malformed digest must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::MalformedDigest { .. })
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn payload_manifest_mismatches_are_rejected() {
    let host = SyntheticHost::new();
    let files = bundle_files();

    // Manifest lists a file the payload does not carry.
    let mut listed = bundle_files();
    listed.push(("extra.dll", b"extra"));
    let manifest = manifest_json(&listed);
    let error = stage(&host, &manifest, &files).expect_err("missing payload file must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::MissingFile { ref name }) if name == "extra.dll"
        ),
        "unexpected error: {error}"
    );

    // Payload carries a file the manifest does not list.
    let manifest = manifest_json(&files);
    let error = stage(&host, &manifest, &listed).expect_err("unlisted payload file must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::UnlistedFile { ref name }) if name == "extra.dll"
        ),
        "unexpected error: {error}"
    );

    // Manifest size disagrees with the payload bytes.
    let mut sized: serde_json::Value =
        serde_json::from_str(&manifest_json(&files)).expect("parse manifest");
    sized["files"][0]["size"] = serde_json::json!(1);
    let sized = sized.to_string();
    let error = stage(&host, &sized, &files).expect_err("size mismatch must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::SizeMismatch { .. })
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn missing_toolkit_dependency_is_rejected() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);

    // The declared CUDA DLL is absent from the toolkit runtime directory.
    fs::remove_file(host.toolkit_root.join("bin").join(TOOLKIT_DLL)).expect("remove toolkit dll");
    let error = stage(&host, &manifest, &files).expect_err("missing dll must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::MissingToolkitDependency { ref dll, .. }) if dll == TOOLKIT_DLL
        ),
        "unexpected error: {error}"
    );
    assert!(!host.install().exists());
}

#[test]
fn missing_toolkit_runtime_directory_is_rejected() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);
    let env = |name: &str| -> Option<OsString> {
        // No CUDA_PATH_V13_3 and no CUDA_PATH: only the system root resolves.
        match name {
            "SystemRoot" => Some(host.system_root.as_os_str().to_owned()),
            _ => None,
        }
    };
    let error = stage_bundle(&host.cache, &payload(&manifest, &files), &env)
        .expect_err("unresolvable toolkit must fail");
    assert!(
        matches!(
            error,
            LocalError::CudaBundle(BundleError::ToolkitNotFound { .. })
        ),
        "unexpected error: {error}"
    );
}

#[test]
fn versioned_toolkit_variable_wins_over_generic_cuda_path() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);
    let env = |name: &str| -> Option<OsString> {
        match name {
            // The generic variable points at a root with no `bin`; the
            // versioned one must still win.
            "CUDA_PATH" => Some(OsString::from("Z:/nonexistent-cuda")),
            _ => host.env()(name),
        }
    };
    let staged = stage_bundle(&host.cache, &payload(&manifest, &files), &env).expect("stage");
    assert_eq!(staged.path_prefix[1], host.toolkit_root.join("bin"));
}

#[test]
fn interrupted_staging_and_partial_install_are_replaced() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);

    // A crashed earlier run left a partial staging directory and a partial
    // install with no marker.
    let install = host.install();
    let staging = part_path(&install);
    fs::create_dir_all(&staging).expect("staging dir");
    fs::write(staging.join("leftover.dll"), b"junk").expect("leftover");
    fs::create_dir_all(&install).expect("install dir");
    fs::write(install.join("stale.exe"), b"stale").expect("stale file");

    let staged = stage(&host, &manifest, &files).expect("restage");
    assert_eq!(staged.executable, install.join(SERVER_EXECUTABLE));
    assert!(!staging.exists());
    assert!(!install.join("stale.exe").exists());
    assert_eq!(
        fs::read(install.join(SERVER_EXECUTABLE)).expect("staged exe"),
        b"synthetic-llama-server"
    );
    assert!(install.join(INSTALL_MARKER).is_file());
}

#[test]
fn drifted_installation_tree_is_restaged() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);
    stage(&host, &manifest, &files).expect("first stage");

    // In-place corruption breaks the recorded tree digest, forcing a restage.
    fs::write(host.install().join("ggml-cuda.dll"), b"corrupted").expect("corrupt dll");
    let staged = stage(&host, &manifest, &files).expect("restage after drift");
    assert_eq!(
        fs::read(staged.executable).expect("staged exe"),
        b"synthetic-llama-server"
    );
    assert_eq!(
        fs::read(host.install().join("ggml-cuda.dll")).expect("restored dll"),
        b"synthetic-ggml-cuda"
    );
}

#[test]
fn concurrent_publication_yields_one_valid_installation() {
    let host = SyntheticHost::new();
    let files = bundle_files();
    let manifest = manifest_json(&files);
    let barrier = Barrier::new(2);

    let results: Vec<Result<StagedCudaBundle>> = thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let host = &host;
                let manifest = &manifest;
                let files = &files;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    stage_bundle(&host.cache, &payload(manifest, files), &host.env())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("publisher thread"))
            .collect()
    });

    let first = results[0].as_ref().expect("first publisher");
    let second = results[1].as_ref().expect("second publisher");
    assert_eq!(first, second);
    assert_eq!(
        fs::read(host.install().join(SERVER_EXECUTABLE)).expect("staged exe"),
        b"synthetic-llama-server"
    );
    // The loser's view is the winner's published tree: one more call is a
    // pure cache hit, proving the marker and tree digest agree.
    stage(&host, &manifest, &files).expect("post-race cache hit");
}
