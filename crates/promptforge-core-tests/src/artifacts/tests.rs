use std::io::{Cursor, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flate2::Compression;
use flate2::write::GzEncoder;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

use super::*;

#[derive(Debug)]
struct FakeServer {
    address: String,
    requests: Arc<AtomicUsize>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    fn new(body: &[u8]) -> Self {
        Self::with_declared_length(body.to_owned(), body.len())
    }

    fn with_declared_length(body: Vec<u8>, declared_length: usize) -> Self {
        Self::with_response(body, declared_length, Duration::ZERO)
    }

    fn delayed(body: &[u8], delay: Duration) -> Self {
        Self::with_response(body.to_owned(), body.len(), delay)
    }

    fn with_response(body: Vec<u8>, declared_length: usize, delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake artifact server");
        listener
            .set_nonblocking(true)
            .expect("make fake artifact server nonblocking");
        let address = listener
            .local_addr()
            .expect("read fake artifact server address");
        let requests = Arc::new(AtomicUsize::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = thread::spawn(move || {
            while !thread_shutdown.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if thread_shutdown.load(Ordering::Acquire) {
                            break;
                        }
                        stream
                            .set_nonblocking(false)
                            .expect("make accepted fake HTTP socket blocking");
                        thread::sleep(delay);
                        serve_one(&mut stream, &body, declared_length);
                        thread_requests.fetch_add(1, Ordering::AcqRel);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("accept fake artifact request: {error}"),
                }
            }
        });
        Self {
            address: address.to_string(),
            requests,
            shutdown,
            thread: Some(thread),
        }
    }

    fn url(&self, name: &str) -> String {
        format!("http://{}/{name}", self.address)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::Acquire)
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        let _ignored = TcpStream::connect(&self.address);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                result.expect("join fake artifact server");
            }
        }
    }
}

fn serve_one(stream: &mut TcpStream, body: &[u8], declared_length: usize) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set fake request read timeout");
    let mut request = [0_u8; 4096];
    let _count = stream.read(&mut request).expect("read fake HTTP request");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
    )
    .expect("write fake HTTP headers");
    stream.write_all(body).expect("write fake HTTP body");
}

fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher)
}

fn file_asset<'a>(name: &'a str, url: &'a str, sha256: &'a str) -> FileAsset<'a> {
    FileAsset { name, url, sha256 }
}

fn server_spec<'a>(
    archive_name: &'a str,
    url: &'a str,
    sha256: &'a str,
    kind: ArchiveKind,
    executable_name: &'a str,
) -> ServerAsset<'a> {
    server_spec_on(
        "test-platform",
        archive_name,
        url,
        sha256,
        kind,
        executable_name,
    )
}

fn server_spec_on<'a>(
    platform: &'a str,
    archive_name: &'a str,
    url: &'a str,
    sha256: &'a str,
    kind: ArchiveKind,
    executable_name: &'a str,
) -> ServerAsset<'a> {
    ServerAsset {
        os: "test",
        arch: "test",
        platform,
        archive_name,
        url,
        sha256,
        archive_kind: kind,
        executable_name,
    }
}

fn artifact_paths(artifacts: &ProvisionedArtifacts) -> (&Path, &Path) {
    (artifacts.llama_server.as_path(), artifacts.model.as_path())
}

fn zip_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, contents) in entries {
            writer
                .start_file(name, options)
                .expect("start tiny zip entry");
            writer.write_all(contents).expect("write tiny zip entry");
        }
        writer.finish().expect("finish tiny zip archive");
    }
    buffer.into_inner()
}

fn zip_symlink(name: &str, target: &str) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        writer
            .add_symlink(name, target, SimpleFileOptions::default())
            .expect("add tiny zip symlink");
        writer.finish().expect("finish tiny zip archive");
    }
    buffer.into_inner()
}

fn tar_gz_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    tar_gz_archive_with_modes(
        &entries
            .iter()
            .map(|(name, contents)| (*name, *contents, 0o755))
            .collect::<Vec<_>>(),
    )
}

fn tar_gz_archive_with_modes(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, contents, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(*mode);
        header.set_size(contents.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, name, *contents)
            .expect("append tiny tar entry");
    }
    builder
        .into_inner()
        .expect("finish tiny tar archive")
        .finish()
        .expect("finish tiny gzip stream")
}

fn tar_gz_special_entry(name: &[u8], entry_type: tar::EntryType, link: Option<&str>) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header
        .set_path("placeholder")
        .expect("set placeholder tar path");
    let bytes = header.as_mut_bytes();
    bytes[..100].fill(0);
    bytes[..name.len()].copy_from_slice(name);
    header.set_entry_type(entry_type);
    header.set_mode(0o755);
    header.set_size(0);
    if let Some(target) = link {
        header
            .set_link_name(target)
            .expect("set tiny tar link target");
    }
    header.set_cksum();
    builder
        .append(&header, io::empty())
        .expect("append special tar entry");
    builder
        .into_inner()
        .expect("finish tiny tar archive")
        .finish()
        .expect("finish tiny gzip stream")
}

const DESKTOP_PLATFORMS: [(&str, &str); 6] = [
    ("windows", "x86_64"),
    ("windows", "aarch64"),
    ("linux", "x86_64"),
    ("linux", "aarch64"),
    ("macos", "x86_64"),
    ("macos", "aarch64"),
];

#[test]
fn official_manifest_covers_supported_desktop_platforms() {
    for kind in [ModelKind::Scenario, ModelKind::Dev] {
        for (os, arch) in DESKTOP_PLATFORMS {
            let asset =
                server_asset(kind, os, arch).expect("supported platform has a manifest entry");
            assert!(
                asset
                    .url
                    .starts_with("https://github.com/ggml-org/llama.cpp/releases/download/b10082/")
            );
            assert_eq!(asset.sha256.len(), 64);
            assert!(asset.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        assert_eq!(server_assets(kind).len(), DESKTOP_PLATFORMS.len());
    }
    assert_eq!(SCENARIO_MODEL_ASSET.name, "Qwen3-0.6B-Q8_0.gguf");
    assert_eq!(
        SCENARIO_MODEL_ASSET.url,
        "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true"
    );
    assert_eq!(
        SCENARIO_MODEL_ASSET.sha256,
        "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031"
    );
}

#[test]
fn dev_manifest_pins_gpu_archives_and_model() {
    let windows = server_asset(ModelKind::Dev, "windows", "x86_64").expect("windows dev asset");
    assert_eq!(windows.platform, "windows-x86_64-vulkan");
    assert_eq!(windows.archive_name, "llama-b10082-bin-win-vulkan-x64.zip");
    assert_eq!(
        windows.sha256,
        "0a4b2e41cfb950da9a749baf8978e0626690fbead3b0ca96860785484cda5bde"
    );

    let linux = server_asset(ModelKind::Dev, "linux", "x86_64").expect("linux dev asset");
    assert_eq!(linux.platform, "linux-x86_64-vulkan");
    assert_eq!(
        linux.archive_name,
        "llama-b10082-bin-ubuntu-vulkan-x64.tar.gz"
    );
    assert_eq!(
        linux.sha256,
        "9003ea32e3d5d8a01da3e4b5d3124e0d21c63d51e112c40f5dcdef91ffaca7cc"
    );

    let linux_arm = server_asset(ModelKind::Dev, "linux", "aarch64").expect("linux arm dev asset");
    assert_eq!(linux_arm.platform, "linux-aarch64-vulkan");
    assert_eq!(
        linux_arm.archive_name,
        "llama-b10082-bin-ubuntu-vulkan-arm64.tar.gz"
    );
    assert_eq!(
        linux_arm.sha256,
        "2805902c3074f615a0105a5325ee29799500c8e29c90ccb986b59e1141df551e"
    );

    // Windows arm64 has no Vulkan build in b10082 and falls back to the CPU archive.
    assert_eq!(
        server_asset(ModelKind::Dev, "windows", "aarch64").expect("windows arm dev asset"),
        server_asset(ModelKind::Scenario, "windows", "aarch64").expect("windows arm cpu asset"),
    );
    // The macOS release tars are already Metal-enabled; both kinds share them.
    for arch in ["x86_64", "aarch64"] {
        assert_eq!(
            server_asset(ModelKind::Dev, "macos", arch).expect("macos dev asset"),
            server_asset(ModelKind::Scenario, "macos", arch).expect("macos scenario asset"),
        );
    }

    assert_eq!(DEV_MODEL_ASSET.name, "Qwen3.5-9B-Q4_K_M.gguf");
    assert_eq!(
        DEV_MODEL_ASSET.url,
        "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf"
    );
    assert_eq!(
        DEV_MODEL_ASSET.sha256,
        "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8"
    );
}

#[test]
fn model_kind_selects_distinct_pinned_assets() {
    assert_eq!(model_asset(ModelKind::Scenario), SCENARIO_MODEL_ASSET);
    assert_eq!(model_asset(ModelKind::Dev), DEV_MODEL_ASSET);
    assert_ne!(
        model_asset(ModelKind::Scenario),
        model_asset(ModelKind::Dev)
    );
    for (os, arch) in [
        ("windows", "x86_64"),
        ("linux", "x86_64"),
        ("linux", "aarch64"),
    ] {
        let scenario = server_asset(ModelKind::Scenario, os, arch).expect("scenario asset");
        let dev = server_asset(ModelKind::Dev, os, arch).expect("dev asset");
        assert_ne!(scenario.url, dev.url);
        assert_ne!(
            scenario.platform, dev.platform,
            "GPU installs need a distinct platform key"
        );
    }
}

#[test]
fn production_entrypoint_is_wired_without_running_it() {
    let entrypoint: fn(ModelKind) -> Result<ProvisionedArtifacts> = provision;
    let paths: fn(&ProvisionedArtifacts) -> (&Path, &Path) = artifact_paths;
    let _ = entrypoint;
    let _ = paths;
}

#[test]
fn unsupported_platform_names_os_and_arch() {
    for kind in [ModelKind::Scenario, ModelKind::Dev] {
        let error =
            server_asset(kind, "plan9", "mips").expect_err("unsupported platform must fail");
        assert!(matches!(
            error,
            Error::UnsupportedPlatform { ref os, ref arch } if os == "plan9" && arch == "mips"
        ));
        assert_eq!(
            error.to_string(),
            "unsupported llama-server platform `plan9/mips`"
        );
    }
}

#[test]
fn provisioner_status_defaults_to_stdout_and_with_status_overrides() {
    let cache = TempDir::new().expect("create model cache");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    assert_eq!(provisioner.status, crate::StatusStream::Stdout);

    let provisioner = provisioner.with_status(crate::StatusStream::Stderr);
    assert_eq!(provisioner.status, crate::StatusStream::Stderr);
}

#[test]
fn provision_entrypoint_provisioner_routes_status_by_kind() {
    let cache = TempDir::new().expect("create model cache");

    let dev = provisioner_for(cache.path(), ModelKind::Dev).expect("create dev provisioner");
    assert_eq!(
        dev.status,
        crate::StatusStream::Stderr,
        "dev status lines must stay off stdout, which carries only the final result"
    );

    let scenario =
        provisioner_for(cache.path(), ModelKind::Scenario).expect("create scenario provisioner");
    assert_eq!(
        scenario.status,
        crate::StatusStream::Stdout,
        "scenario status lines are part of the pinned stdout contract"
    );
}

#[test]
fn model_download_uses_part_staging_and_repairs_corruption() {
    let cache = TempDir::new().expect("create model cache");
    let body = b"tiny fake model".to_vec();
    let sha256 = digest(&body);
    let server = FakeServer::new(&body);
    let url = server.url("model.gguf");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let destination = cache.path().join("models/model.gguf");
    let staging = part_path(&destination);
    fs::create_dir_all(destination.parent().expect("model has parent"))
        .expect("create model parent");
    fs::write(&staging, b"interrupted").expect("seed interrupted download");

    let first = provisioner
        .provision_file(file_asset("model.gguf", &url, &sha256), "models")
        .expect("provision fake model");
    assert_eq!(first, destination);
    assert_eq!(fs::read(&first).expect("read cached model"), body);
    assert!(!staging.exists());
    assert_eq!(server.requests(), 1);

    provisioner
        .provision_file(file_asset("model.gguf", &url, &sha256), "models")
        .expect("reuse valid model cache");
    assert_eq!(server.requests(), 1, "cache hit must make no request");

    fs::write(&destination, b"corrupt").expect("corrupt cached model");
    provisioner
        .provision_file(file_asset("model.gguf", &url, &sha256), "models")
        .expect("repair corrupt model");
    assert_eq!(fs::read(&destination).expect("read repaired model"), body);
    assert_eq!(server.requests(), 2);
}

#[test]
fn bad_download_digest_is_rejected_without_partial_install() {
    let cache = TempDir::new().expect("create model cache");
    let server = FakeServer::new(b"wrong bytes");
    let url = server.url("model.gguf");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let expected = digest(b"expected bytes");
    let destination = cache.path().join("models/model.gguf");

    let error = provisioner
        .provision_file(file_asset("model.gguf", &url, &expected), "models")
        .expect_err("digest mismatch must fail");
    assert!(matches!(error, Error::DigestMismatch { .. }));
    assert!(!destination.exists());
    assert!(!part_path(&destination).exists());
}

#[test]
fn interrupted_response_leaves_cache_repairable() {
    let cache = TempDir::new().expect("create model cache");
    let body = b"truncated".to_vec();
    let server = FakeServer::with_declared_length(body.clone(), body.len() + 50);
    let url = server.url("model.gguf");
    let sha256 = digest(&body);
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let destination = cache.path().join("models/model.gguf");

    provisioner
        .provision_file(file_asset("model.gguf", &url, &sha256), "models")
        .expect_err("truncated response must fail");
    assert!(!destination.exists());
    assert!(!part_path(&destination).exists());

    let repair_server = FakeServer::new(&body);
    let repair_url = repair_server.url("model.gguf");
    provisioner
        .provision_file(file_asset("model.gguf", &repair_url, &sha256), "models")
        .expect("next complete response repairs cache");
    assert_eq!(fs::read(destination).expect("read repaired model"), body);
}

#[test]
fn zip_server_install_is_atomic_cached_and_self_repairing() {
    let cache = TempDir::new().expect("create model cache");
    let archive = zip_archive(&[
        ("bundle/llama-server.exe", b"tiny server"),
        ("bundle/ggml.dll", b"tiny dependency"),
    ]);
    let sha256 = digest(&archive);
    let server = FakeServer::new(&archive);
    let url = server.url("server.zip");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let spec = server_spec(
        "server.zip",
        &url,
        &sha256,
        ArchiveKind::Zip,
        "llama-server.exe",
    );
    let install = cache.path().join("llama.cpp/b10082-test-platform");
    let staging = part_path(&install);
    fs::create_dir_all(&staging).expect("seed partial install");
    fs::write(staging.join("junk"), b"partial").expect("write partial install");
    let archive_part = part_path(&cache.path().join("downloads/server.zip"));
    fs::create_dir_all(archive_part.parent().expect("archive has parent"))
        .expect("create download directory");
    fs::write(&archive_part, b"partial archive").expect("seed partial archive");

    let executable = provisioner
        .provision_server(spec)
        .expect("provision tiny zip server");
    assert_eq!(
        fs::read(&executable).expect("read installed server"),
        b"tiny server"
    );
    assert!(!staging.exists());
    assert!(!archive_part.exists());
    assert_eq!(server.requests(), 1);

    provisioner
        .provision_server(spec)
        .expect("reuse valid server install");
    assert_eq!(server.requests(), 1, "cache hit must make no request");

    fs::write(&executable, b"corrupt executable").expect("corrupt installed executable");
    let repaired = provisioner
        .provision_server(spec)
        .expect("repair install from cached archive");
    assert_eq!(
        fs::read(repaired).expect("read repaired executable"),
        b"tiny server"
    );
    assert_eq!(
        server.requests(),
        1,
        "valid archive repairs install offline"
    );

    let dependency = install.join("bundle/ggml.dll");
    fs::write(&dependency, b"corrupt dependency").expect("corrupt installed dependency");
    provisioner
        .provision_server(spec)
        .expect("repair dependency from cached archive");
    assert_eq!(
        fs::read(dependency).expect("read repaired dependency"),
        b"tiny dependency"
    );
    assert_eq!(
        server.requests(),
        1,
        "dependency repair also reuses cached archive"
    );

    fs::write(
        cache.path().join("downloads/server.zip"),
        b"corrupt archive",
    )
    .expect("corrupt cached archive");
    fs::write(&executable, b"corrupt again").expect("corrupt installed executable again");
    provisioner
        .provision_server(spec)
        .expect("repair archive and install");
    assert_eq!(server.requests(), 2);
}

#[test]
fn tar_gz_server_archive_extracts_without_external_network() {
    let cache = TempDir::new().expect("create model cache");
    let archive = tar_gz_archive(&[
        ("bundle/llama-server", b"tiny unix server"),
        ("bundle/libggml.so", b"tiny dependency"),
    ]);
    let sha256 = digest(&archive);
    let server = FakeServer::new(&archive);
    let url = server.url("server.tar.gz");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let spec = server_spec(
        "server.tar.gz",
        &url,
        &sha256,
        ArchiveKind::TarGz,
        "llama-server",
    );

    let executable = provisioner
        .provision_server(spec)
        .expect("provision tiny tar server");
    assert_eq!(
        fs::read(executable).expect("read extracted server"),
        b"tiny unix server"
    );
    assert_eq!(server.requests(), 1);
}

#[test]
fn unsafe_archive_path_is_rejected_outside_install_tree() {
    let cache = TempDir::new().expect("create model cache");
    let archive = zip_archive(&[("../escape", b"must not escape")]);
    let sha256 = digest(&archive);
    let server = FakeServer::new(&archive);
    let url = server.url("unsafe.zip");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let spec = server_spec(
        "unsafe.zip",
        &url,
        &sha256,
        ArchiveKind::Zip,
        "llama-server.exe",
    );

    let error = provisioner
        .provision_server(spec)
        .expect_err("unsafe archive must fail");
    assert!(matches!(error, Error::UnsafeArchiveEntry { .. }));
    assert!(!cache.path().join("escape").exists());
    assert!(
        !cache
            .path()
            .join("llama.cpp/b10082-test-platform.part")
            .exists()
    );
}

fn extraction_error(archive: &[u8], kind: ArchiveKind) -> Error {
    let temporary = TempDir::new().expect("create extraction directory");
    let archive_path = temporary.path().join("archive");
    let destination = temporary.path().join("install");
    fs::write(&archive_path, archive).expect("write tiny archive");
    fs::create_dir(&destination).expect("create extraction root");
    extract_archive(&archive_path, &destination, kind).expect_err("unsafe archive must fail")
}

#[test]
fn zip_rejects_portable_traversal_and_link_entries() {
    for name in ["../escape", "/absolute", "C:/drive", r"..\escape"] {
        let archive = zip_archive(&[(name, b"unsafe")]);
        assert!(
            matches!(
                extraction_error(&archive, ArchiveKind::Zip),
                Error::UnsafeArchiveEntry { .. }
            ),
            "ZIP entry `{name}` must be rejected"
        );
    }

    let archive = zip_symlink("bundle/link", "../outside");
    assert!(matches!(
        extraction_error(&archive, ArchiveKind::Zip),
        Error::UnsafeArchiveEntry { .. }
    ));
}

#[test]
fn zip_rejects_ntfs_ads_path_components() {
    let archive = zip_archive(&[("bundle/llama-server.exe:payload", b"unsafe")]);
    assert!(matches!(
        extraction_error(&archive, ArchiveKind::Zip),
        Error::UnsafeArchiveEntry { .. }
    ));
}

#[test]
fn tar_rejects_portable_traversal_and_link_entries() {
    for name in [
        b"../escape".as_slice(),
        b"/absolute".as_slice(),
        b"C:/drive".as_slice(),
        br"..\escape".as_slice(),
    ] {
        let archive = tar_gz_special_entry(name, tar::EntryType::Regular, None);
        assert!(
            matches!(
                extraction_error(&archive, ArchiveKind::TarGz),
                Error::UnsafeArchiveEntry { .. }
            ),
            "tar entry `{}` must be rejected",
            String::from_utf8_lossy(name)
        );
    }

    for entry_type in [tar::EntryType::Symlink, tar::EntryType::Link] {
        let archive = tar_gz_special_entry(b"bundle/link", entry_type, Some("../outside"));
        assert!(matches!(
            extraction_error(&archive, ArchiveKind::TarGz),
            Error::UnsafeArchiveEntry { .. }
        ));
    }
}

#[test]
fn tar_rejects_ntfs_ads_path_components() {
    let archive = tar_gz_archive(&[("bundle/llama-server:payload", b"unsafe")]);
    assert!(matches!(
        extraction_error(&archive, ArchiveKind::TarGz),
        Error::UnsafeArchiveEntry { .. }
    ));
}

#[cfg(unix)]
#[test]
fn zip_and_tar_reject_symlinked_extraction_components() {
    use std::os::unix::fs::symlink;

    let archives = [
        (
            zip_archive(&[("bundle/file", b"must stay confined")]),
            ArchiveKind::Zip,
        ),
        (
            tar_gz_archive(&[("bundle/file", b"must stay confined")]),
            ArchiveKind::TarGz,
        ),
    ];
    for (archive, kind) in archives {
        let temporary = TempDir::new().expect("create extraction directory");
        let outside = TempDir::new().expect("create outside directory");
        let archive_path = temporary.path().join("archive");
        let destination = temporary.path().join("install");
        fs::write(&archive_path, archive).expect("write tiny archive");
        fs::create_dir(&destination).expect("create extraction root");
        symlink(outside.path(), destination.join("bundle")).expect("create extraction symlink");

        let error = extract_archive(&archive_path, &destination, kind)
            .expect_err("symlinked extraction component must fail");
        assert!(matches!(error, Error::UnsafeCachePath { .. }));
        assert!(!outside.path().join("file").exists());
    }
}

#[cfg(windows)]
#[test]
fn zip_and_tar_reject_junction_extraction_components() {
    let archives = [
        (
            zip_archive(&[("bundle/file", b"must stay confined")]),
            ArchiveKind::Zip,
        ),
        (
            tar_gz_archive(&[("bundle/file", b"must stay confined")]),
            ArchiveKind::TarGz,
        ),
    ];
    for (archive, kind) in archives {
        let temporary = TempDir::new().expect("create extraction directory");
        let outside = TempDir::new().expect("create outside directory");
        let archive_path = temporary.path().join("archive");
        let destination = temporary.path().join("install");
        let junction = destination.join("bundle");
        fs::write(&archive_path, archive).expect("write tiny archive");
        fs::create_dir(&destination).expect("create extraction root");
        let status = Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(outside.path())
            .status()
            .expect("run junction creator");
        assert!(status.success(), "create extraction junction");

        let error = extract_archive(&archive_path, &destination, kind)
            .expect_err("junction extraction component must fail");
        assert!(matches!(error, Error::UnsafeCachePath { .. }));
        assert!(!outside.path().join("file").exists());
    }
}

#[test]
fn cache_paths_are_confined_before_any_request() {
    let cache = TempDir::new().expect("create model cache");
    let server = FakeServer::new(b"must not be requested");
    let url = server.url("outside");
    let sha256 = digest(b"must not be requested");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");

    for (name, directory) in [("outside", "../escape"), ("../outside", "models")] {
        let error = provisioner
            .provision_file(file_asset(name, &url, &sha256), directory)
            .expect_err("escaping cache path must fail");
        assert!(matches!(error, Error::UnsafeCachePath { .. }));
    }
    assert_eq!(server.requests(), 0);
    assert!(
        !cache
            .path()
            .parent()
            .expect("cache has parent")
            .join("escape")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn symlink_cache_component_is_rejected_without_writing_target() {
    use std::os::unix::fs::symlink;

    let cache = TempDir::new().expect("create model cache");
    let outside = TempDir::new().expect("create outside directory");
    symlink(outside.path(), cache.path().join("models")).expect("create cache symlink");
    let server = FakeServer::new(b"must not be requested");
    let url = server.url("model.gguf");
    let sha256 = digest(b"must not be requested");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");

    let error = provisioner
        .provision_file(file_asset("model.gguf", &url, &sha256), "models")
        .expect_err("symlink cache component must fail");
    assert!(matches!(error, Error::UnsafeCachePath { .. }));
    assert_eq!(server.requests(), 0);
    assert!(!outside.path().join("model.gguf").exists());
}

#[cfg(windows)]
#[test]
fn junction_cache_component_is_rejected_without_writing_target() {
    let cache = TempDir::new().expect("create model cache");
    let outside = TempDir::new().expect("create outside directory");
    let junction = cache.path().join("models");
    let status = Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(&junction)
        .arg(outside.path())
        .status()
        .expect("run junction creator");
    assert!(status.success(), "create cache junction");
    let server = FakeServer::new(b"must not be requested");
    let url = server.url("model.gguf");
    let sha256 = digest(b"must not be requested");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");

    let error = provisioner
        .provision_file(file_asset("model.gguf", &url, &sha256), "models")
        .expect_err("junction cache component must fail");
    assert!(matches!(error, Error::UnsafeCachePath { .. }));
    assert_eq!(server.requests(), 0);
    assert!(!outside.path().join("model.gguf").exists());
}

#[test]
fn concurrent_threads_share_one_artifact_download() {
    let cache = TempDir::new().expect("create model cache");
    let body = b"thread synchronized model".to_vec();
    let sha256 = digest(&body);
    let server = FakeServer::delayed(&body, Duration::from_millis(100));
    let url = server.url("model.gguf");
    let barrier = Arc::new(Barrier::new(8));
    let mut threads = Vec::new();

    for _ in 0..8 {
        let cache = cache.path().to_owned();
        let sha256 = sha256.clone();
        let url = url.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let provisioner = Provisioner::new(&cache).expect("create concurrent provisioner");
            barrier.wait();
            provisioner
                .provision_file(file_asset("model.gguf", &url, &sha256), "models")
                .expect("provision synchronized model")
        }));
    }

    for thread in threads {
        let path = thread.join().expect("join provision thread");
        assert_eq!(fs::read(path).expect("read synchronized model"), body);
    }
    assert_eq!(server.requests(), 1);
}

const PROCESS_CACHE: &str = "PROMPTFORGE_TEST_PROCESS_CACHE";
const PROCESS_URL: &str = "PROMPTFORGE_TEST_PROCESS_URL";
const PROCESS_SHA256: &str = "PROMPTFORGE_TEST_PROCESS_SHA256";
const PROCESS_ARCHIVE: &str = "PROMPTFORGE_TEST_PROCESS_ARCHIVE";

#[test]
fn cross_process_worker() {
    let (Ok(cache), Ok(url), Ok(sha256)) = (
        std::env::var(PROCESS_CACHE),
        std::env::var(PROCESS_URL),
        std::env::var(PROCESS_SHA256),
    ) else {
        return;
    };
    let provisioner = Provisioner::new(Path::new(&cache)).expect("create process provisioner");
    if std::env::var_os(PROCESS_ARCHIVE).is_some() {
        provisioner
            .provision_server(server_spec(
                "server.zip",
                &url,
                &sha256,
                ArchiveKind::Zip,
                "llama-server.exe",
            ))
            .expect("provision process synchronized server");
    } else {
        provisioner
            .provision_file(file_asset("model.gguf", &url, &sha256), "models")
            .expect("provision process synchronized model");
    }
}

#[test]
fn concurrent_processes_share_one_artifact_download() {
    let cache = TempDir::new().expect("create model cache");
    let body = b"process synchronized model".to_vec();
    let sha256 = digest(&body);
    let server = FakeServer::delayed(&body, Duration::from_millis(250));
    let url = server.url("model.gguf");
    let executable = std::env::current_exe().expect("locate current test executable");
    let mut children = Vec::new();

    for _ in 0..4 {
        children.push(
            Command::new(&executable)
                .arg("cross_process_worker")
                .env(PROCESS_CACHE, cache.path())
                .env(PROCESS_URL, &url)
                .env(PROCESS_SHA256, &sha256)
                .spawn()
                .expect("spawn artifact worker"),
        );
    }
    for mut child in children {
        assert!(
            child.wait().expect("wait for artifact worker").success(),
            "artifact worker must succeed"
        );
    }

    assert_eq!(server.requests(), 1);
    assert_eq!(
        fs::read(cache.path().join("models/model.gguf")).expect("read process synchronized model"),
        body
    );
}

#[test]
fn concurrent_threads_share_one_server_install() {
    let cache = TempDir::new().expect("create model cache");
    let archive = zip_archive(&[
        ("bundle/llama-server.exe", b"thread synchronized server"),
        ("bundle/ggml.dll", b"thread synchronized dependency"),
    ]);
    let sha256 = digest(&archive);
    let server = FakeServer::delayed(&archive, Duration::from_millis(100));
    let url = server.url("server.zip");
    let barrier = Arc::new(Barrier::new(8));
    let mut threads = Vec::new();

    for _ in 0..8 {
        let cache = cache.path().to_owned();
        let sha256 = sha256.clone();
        let url = url.clone();
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            let provisioner = Provisioner::new(&cache).expect("create concurrent provisioner");
            barrier.wait();
            provisioner
                .provision_server(server_spec(
                    "server.zip",
                    &url,
                    &sha256,
                    ArchiveKind::Zip,
                    "llama-server.exe",
                ))
                .expect("provision synchronized server")
        }));
    }

    for thread in threads {
        let path = thread.join().expect("join provision thread");
        assert_eq!(
            fs::read(path).expect("read synchronized server"),
            b"thread synchronized server"
        );
    }
    assert_eq!(server.requests(), 1);
}

#[test]
fn concurrent_processes_share_one_server_install() {
    let cache = TempDir::new().expect("create model cache");
    let archive = zip_archive(&[
        ("bundle/llama-server.exe", b"process synchronized server"),
        ("bundle/ggml.dll", b"process synchronized dependency"),
    ]);
    let sha256 = digest(&archive);
    let server = FakeServer::delayed(&archive, Duration::from_millis(250));
    let url = server.url("server.zip");
    let executable = std::env::current_exe().expect("locate current test executable");
    let mut children = Vec::new();

    for _ in 0..4 {
        children.push(
            Command::new(&executable)
                .arg("cross_process_worker")
                .env(PROCESS_CACHE, cache.path())
                .env(PROCESS_URL, &url)
                .env(PROCESS_SHA256, &sha256)
                .env(PROCESS_ARCHIVE, "1")
                .spawn()
                .expect("spawn artifact worker"),
        );
    }
    for mut child in children {
        assert!(
            child.wait().expect("wait for artifact worker").success(),
            "artifact worker must succeed"
        );
    }

    assert_eq!(server.requests(), 1);
    assert_eq!(
        fs::read(
            cache
                .path()
                .join("llama.cpp/b10082-test-platform/bundle/llama-server.exe")
        )
        .expect("read process synchronized server"),
        b"process synchronized server"
    );
}

#[cfg(unix)]
#[test]
fn executable_mode_change_invalidates_and_repairs_install() {
    use std::os::unix::fs::PermissionsExt as _;

    let cache = TempDir::new().expect("create model cache");
    let archive = tar_gz_archive_with_modes(&[
        ("bundle/llama-server", b"tiny unix server", 0o755),
        ("bundle/libggml.so", b"tiny dependency", 0o644),
    ]);
    let sha256 = digest(&archive);
    let server = FakeServer::new(&archive);
    let url = server.url("server.tar.gz");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let spec = server_spec(
        "server.tar.gz",
        &url,
        &sha256,
        ArchiveKind::TarGz,
        "llama-server",
    );
    let executable = provisioner
        .provision_server(spec)
        .expect("provision executable archive");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o644))
        .expect("remove executable mode");

    let repaired = provisioner
        .provision_server(spec)
        .expect("repair executable mode from cached archive");
    let mode = fs::metadata(repaired)
        .expect("inspect repaired executable")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
    assert_eq!(server.requests(), 1, "permission repair must stay offline");
}

/// One fake artifact pair for one model kind: a zip server archive plus a
/// model blob, each behind its own request-counting fake HTTP server.
struct FakeKindAssets {
    archive_server: FakeServer,
    model_server: FakeServer,
    executable_body: &'static [u8],
    model_body: &'static [u8],
    archive_sha256: String,
    model_sha256: String,
}

impl FakeKindAssets {
    fn new(executable_body: &'static [u8], model_body: &'static [u8]) -> Self {
        let archive = zip_archive(&[("bundle/llama-server.exe", executable_body)]);
        let archive_sha256 = digest(&archive);
        let model_sha256 = digest(model_body);
        Self {
            archive_server: FakeServer::new(&archive),
            model_server: FakeServer::new(model_body),
            executable_body,
            model_body,
            archive_sha256,
            model_sha256,
        }
    }

    fn provision(
        &self,
        provisioner: &Provisioner,
        platform: &str,
        archive_name: &str,
        model_name: &str,
    ) -> ProvisionedArtifacts {
        let archive_url = self.archive_server.url(archive_name);
        let model_url = self.model_server.url(model_name);
        provision_assets(
            provisioner,
            server_spec_on(
                platform,
                archive_name,
                &archive_url,
                &self.archive_sha256,
                ArchiveKind::Zip,
                "llama-server.exe",
            ),
            file_asset(model_name, &model_url, &self.model_sha256),
        )
        .expect("provision fake kind assets")
    }

    fn requests(&self) -> (usize, usize) {
        (self.archive_server.requests(), self.model_server.requests())
    }
}

#[test]
fn provisioning_one_kind_downloads_no_other_kind_assets() {
    let cache = TempDir::new().expect("create model cache");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let scenario = FakeKindAssets::new(b"scenario server", b"scenario model");
    let dev = FakeKindAssets::new(b"dev server", b"dev model");

    let scenario_artifacts = scenario.provision(
        &provisioner,
        "test-platform",
        "scenario.zip",
        "scenario.gguf",
    );
    assert_eq!(
        fs::read(&scenario_artifacts.llama_server).expect("read scenario server"),
        scenario.executable_body
    );
    assert_eq!(
        fs::read(&scenario_artifacts.model).expect("read scenario model"),
        scenario.model_body
    );
    assert_eq!(scenario.requests(), (1, 1));
    assert_eq!(
        dev.requests(),
        (0, 0),
        "scenario provisioning must not touch dev assets"
    );

    let dev_artifacts = dev.provision(&provisioner, "test-platform-vulkan", "dev.zip", "dev.gguf");
    assert_eq!(
        fs::read(&dev_artifacts.llama_server).expect("read dev server"),
        dev.executable_body
    );
    assert_eq!(
        fs::read(&dev_artifacts.model).expect("read dev model"),
        dev.model_body
    );
    assert_eq!(dev.requests(), (1, 1));
    assert_eq!(
        scenario.requests(),
        (1, 1),
        "dev provisioning must not touch scenario assets"
    );
}

#[test]
fn gpu_platform_key_install_coexists_with_cpu_install() {
    let cache = TempDir::new().expect("create model cache");
    let provisioner = Provisioner::new(cache.path()).expect("create provisioner");
    let cpu = FakeKindAssets::new(b"cpu server", b"scenario model");
    let gpu = FakeKindAssets::new(b"gpu server", b"dev model");

    let cpu_executable = cpu
        .provision(&provisioner, "test-platform", "cpu.zip", "scenario.gguf")
        .llama_server;
    let gpu_executable = gpu
        .provision(&provisioner, "test-platform-vulkan", "gpu.zip", "dev.gguf")
        .llama_server;

    assert!(
        cpu_executable.starts_with(cache.path().join("llama.cpp/b10082-test-platform")),
        "CPU install lives under its own platform key"
    );
    assert!(
        gpu_executable.starts_with(cache.path().join("llama.cpp/b10082-test-platform-vulkan")),
        "GPU install lives under its own platform key"
    );
    assert_eq!(
        fs::read(&cpu_executable).expect("read coexisting cpu server"),
        cpu.executable_body,
        "GPU install must not disturb the CPU install"
    );
    assert_eq!(
        fs::read(&gpu_executable).expect("read coexisting gpu server"),
        gpu.executable_body
    );

    cpu.provision(&provisioner, "test-platform", "cpu.zip", "scenario.gguf");
    gpu.provision(&provisioner, "test-platform-vulkan", "gpu.zip", "dev.gguf");
    assert_eq!(
        cpu.requests(),
        (1, 1),
        "coexisting installs stay cache hits"
    );
    assert_eq!(
        gpu.requests(),
        (1, 1),
        "coexisting installs stay cache hits"
    );
}
