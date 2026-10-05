//! End-to-end CUDA release build: verify, compile, account, pack.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::manifest::{
    BUNDLE_FORMAT_VERSION, BundleFile, LINKAGE_POLICY, Manifest, SourceIdentity, ToolIdentity,
    sha256_hex,
};
use crate::probe::{CommandRequest, Probe, SystemProbe};
use crate::{arch, cmake, toolchain};

#[path = "bundle-closure.rs"]
mod closure;

use closure::{bundle_cuda_runtimes, inspect_closure};

/// The only target triple the tool produces: it compiles on and for a
/// Windows x86-64 machine.
const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";

/// Upstream repository a `--source` checkout comes from.
const SOURCE_URL: &str = "https://github.com/ggml-org/llama.cpp.git";

/// What one build run needs, resolved from the command line.
#[derive(Debug)]
pub struct BuildRequest {
    /// The llama.cpp checkout to compile.
    pub source: PathBuf,
    /// The llama.cpp release tag the checkout represents (for example
    /// `b10082`); names the zip.
    pub tag: String,
    /// CUDA architectures to compile for (for example `120a-real`). When
    /// empty, detected from the build machine's GPUs through `nvidia-smi`.
    pub archs: Vec<String>,
    /// Directory receiving the zip, its `.sha256`, and the manifest. The
    /// CMake build tree sits under it in `work/` and is not part of the
    /// published output.
    pub out: PathBuf,
    /// Runs the `--list-devices` smoke check after the build. Needs a GPU;
    /// the GitHub build computer has none, so the workflow passes
    /// `--no-smoke` and the self-hosted smoke job covers the GPU check.
    pub smoke: bool,
}

/// What the build produced.
#[derive(Debug)]
pub struct BuildOutcome {
    /// The release zip: `llama-server.exe`, its sibling DLLs, the CUDA
    /// runtime DLLs, and `llama-cuda-manifest.json`.
    pub zip: PathBuf,
    /// The zip's SHA-256 sidecar in `sha256sum` format.
    pub checksum: PathBuf,
    /// The canonical build manifest (also packed into the zip).
    pub manifest: PathBuf,
    /// The architectures compiled for.
    pub archs: Vec<String>,
}

/// Runs the full release build against the real environment and toolchain.
///
/// # Errors
/// Returns an error when the machine is not Windows x86-64, the checkout is
/// absent or unrecognized, the CUDA Toolkit is missing or too old, any
/// build command fails, the dependency closure is incomplete, a CUDA
/// runtime DLL cannot be found in the toolkit, or the smoke check finds no
/// CUDA device.
pub fn build(request: &BuildRequest) -> anyhow::Result<BuildOutcome> {
    let env = |name: &str| std::env::var(name).ok();
    build_with(
        &SystemProbe,
        &env,
        std::env::consts::OS,
        std::env::consts::ARCH,
        request,
    )
}

/// Runs `request` and requires exit code zero, bounding the failure output.
fn run_checked(probe: &impl Probe, request: &CommandRequest, phase: &str) -> anyhow::Result<()> {
    let output = probe
        .run(request)
        .with_context(|| format!("{phase} invocation"))?;
    anyhow::ensure!(
        output.success(),
        "{phase} failed (exit {}) running `{}`:\n{}",
        output.code,
        request.display_line(),
        output.stderr
    );
    Ok(())
}

/// Verifies the `--source` folder looks like a llama.cpp checkout and reads
/// its commit through git (a checkout always has one; a tarball download
/// fails here with instructions).
fn verify_source(probe: &impl Probe, source: &Path) -> anyhow::Result<String> {
    anyhow::ensure!(
        source.is_dir(),
        "the llama.cpp source {} is missing; pass --source pointing at a checkout",
        source.display()
    );
    anyhow::ensure!(
        source.join("CMakeLists.txt").is_file(),
        "{} does not look like llama.cpp (no CMakeLists.txt)",
        source.display()
    );
    let output = probe
        .run(&CommandRequest::new("git").args([
            "-C",
            &source.display().to_string(),
            "rev-parse",
            "HEAD",
        ]))
        .context("read the checkout's commit")?;
    anyhow::ensure!(
        output.success(),
        "git rev-parse HEAD failed in {} (exit {}): {}; --source must be a git checkout, \
         not a tarball",
        source.display(),
        output.code,
        output.stderr
    );
    Ok(output.stdout.trim().to_string())
}

/// Collects the runtime files under `stage`: `llama-server.exe` plus every
/// DLL beside it, sorted by name with hashes.
fn collect_runtime_files(stage: &Path) -> anyhow::Result<Vec<BundleFile>> {
    anyhow::ensure!(
        stage.is_dir(),
        "llama-server build produced no runtime directory at {}",
        stage.display()
    );
    let mut names = Vec::new();
    for entry in std::fs::read_dir(stage).with_context(|| format!("read {}", stage.display()))? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name == "llama-server.exe" || name.to_ascii_lowercase().ends_with(".dll") {
            names.push(name);
        }
    }
    anyhow::ensure!(
        names.iter().any(|name| name == "llama-server.exe"),
        "llama-server.exe is missing from {}",
        stage.display()
    );
    names.sort();
    let mut files = Vec::new();
    for name in names {
        let bytes = std::fs::read(stage.join(&name)).with_context(|| format!("read {name}"))?;
        files.push(BundleFile {
            size: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            name,
        });
    }
    Ok(files)
}

/// Resolved toolchain facts for one build.
struct Toolchain {
    nvcc_path: PathBuf,
    nvcc_version: String,
    toolkit_version: String,
    cmake_path: PathBuf,
    cmake_version: String,
}

/// Resolves nvcc and CMake, probes their versions, and enforces the
/// toolkit floor.
fn probe_toolchain(
    probe: &impl Probe,
    env: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<Toolchain> {
    let nvcc_path = toolchain::resolve_tool("nvcc", env)
        .context("CUDA Toolkit not found: `nvcc` is not on PATH; install CUDA >= 12.8")?;
    let nvcc_out = probe
        .run(&CommandRequest::new(&nvcc_path).args(["--version"]))
        .context("probe nvcc")?;
    anyhow::ensure!(
        nvcc_out.success(),
        "nvcc --version failed:\n{}",
        nvcc_out.stderr
    );
    let (toolkit_version, nvcc_version) = toolchain::parse_nvcc_version(&nvcc_out.stdout)
        .context("unrecognized `nvcc --version` output")?;
    toolchain::require_toolkit(&toolkit_version)?;

    let cmake_path =
        toolchain::resolve_tool("cmake", env).context("cmake is not on PATH; install CMake")?;
    let cmake_out = probe
        .run(&CommandRequest::new(&cmake_path).args(["--version"]))
        .context("probe cmake")?;
    anyhow::ensure!(
        cmake_out.success(),
        "cmake --version failed:\n{}",
        cmake_out.stderr
    );
    let cmake_version = toolchain::parse_cmake_version(&cmake_out.stdout)
        .context("unrecognized `cmake --version` output")?;

    Ok(Toolchain {
        nvcc_path,
        nvcc_version,
        toolkit_version,
        cmake_path,
        cmake_version,
    })
}

/// Runs the staged executable's device-list operation and requires at
/// least one CUDA device in its output.
fn smoke_check(probe: &impl Probe, stage: &Path) -> anyhow::Result<()> {
    let exe = stage.join("llama-server.exe");
    let smoke = probe
        .run(
            &CommandRequest::new(&exe)
                .args(["--list-devices"])
                .cwd(stage),
        )
        .context("smoke-check llama-server")?;
    anyhow::ensure!(
        smoke.success() && smoke.stdout.contains("CUDA"),
        "llama-server --list-devices reported no CUDA device (exit {}):\n{}\n{}",
        smoke.code,
        smoke.stdout,
        smoke.stderr
    );
    Ok(())
}

/// Packs the staged runtime files and the manifest into the release zip
/// and writes its SHA-256 sidecar in `sha256sum` format.
fn pack(
    out: &Path,
    tag: &str,
    stage: &Path,
    files: &[BundleFile],
    manifest_path: &Path,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let zip_name = format!("llama-server-cuda-blackwell-{tag}-win-x64.zip");
    let zip_path = out.join(&zip_name);
    let file = std::fs::File::create(&zip_path)
        .with_context(|| format!("create {}", zip_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for file in files {
        zip.start_file(&file.name, options)
            .with_context(|| format!("add {} to the zip", file.name))?;
        let mut source = std::fs::File::open(stage.join(&file.name))
            .with_context(|| format!("open {}", file.name))?;
        std::io::copy(&mut source, &mut zip).with_context(|| format!("pack {}", file.name))?;
    }
    zip.start_file("llama-cuda-manifest.json", options)
        .context("add the manifest to the zip")?;
    let mut manifest_file = std::fs::File::open(manifest_path)
        .with_context(|| format!("open {}", manifest_path.display()))?;
    std::io::copy(&mut manifest_file, &mut zip).context("pack the manifest")?;
    zip.finish().context("finish the zip")?;

    let zip_bytes =
        std::fs::read(&zip_path).with_context(|| format!("read back {}", zip_path.display()))?;
    let checksum_path = out.join(format!("{zip_name}.sha256"));
    std::fs::write(
        &checksum_path,
        format!("{}  {zip_name}\n", sha256_hex(&zip_bytes)),
    )
    .with_context(|| format!("write {}", checksum_path.display()))?;
    Ok((zip_path, checksum_path))
}

/// Full pipeline, with the command seam, environment, and machine identity
/// injected for tests.
fn build_with(
    probe: &impl Probe,
    env: &impl Fn(&str) -> Option<String>,
    host_os: &str,
    host_arch: &str,
    request: &BuildRequest,
) -> anyhow::Result<BuildOutcome> {
    anyhow::ensure!(
        host_os == "windows" && host_arch == "x86_64",
        "build-llama-cuda runs on Windows x86-64 only (found {host_os}/{host_arch})"
    );
    let commit = verify_source(probe, &request.source)?;
    let tools = probe_toolchain(probe, env)?;
    let architectures = if request.archs.is_empty() {
        arch::detect(probe)?
    } else {
        let mut archs = request.archs.clone();
        archs.sort();
        archs.dedup();
        archs
    };

    let work_dir = request.out.join("work");
    let build_dir = work_dir.join("llama-build");
    std::fs::create_dir_all(&build_dir)
        .with_context(|| format!("create {}", build_dir.display()))?;
    let (configure, build_cmd) = cmake::plan(
        &request.source,
        &build_dir,
        &tools.cmake_path,
        &architectures,
        &tools.nvcc_path,
    );
    run_checked(probe, &configure, "cmake configure")?;
    let cache = std::fs::read_to_string(build_dir.join("CMakeCache.txt"))
        .context("read CMakeCache.txt after configure")?;
    let compiler_cmake = cmake::compiler_cmake_path(&build_dir)?;
    let compiler_content = std::fs::read_to_string(&compiler_cmake)
        .with_context(|| format!("read {}", compiler_cmake.display()))?;
    let (cxx_compiler, cxx_version) = cmake::parse_compiler_cmake(&compiler_content)?;
    let identity = cmake::CacheIdentity {
        generator: cmake::parse_generator(&cache)?,
        cxx_compiler,
        cxx_version,
    };
    run_checked(probe, &build_cmd, "cmake build")?;

    let stage = build_dir.join("bin").join("Release");
    let built = collect_runtime_files(&stage)?;
    let built_names: Vec<String> = built.iter().map(|file| file.name.clone()).collect();
    let (cuda_dlls, system_dlls) = inspect_closure(probe, env, &stage, &built_names)?;
    bundle_cuda_runtimes(&tools.nvcc_path, &stage, &cuda_dlls)?;
    // Re-collect so the bundle list includes the freshly staged CUDA
    // runtime DLLs.
    let files = collect_runtime_files(&stage)?;
    if request.smoke {
        smoke_check(probe, &stage)?;
    }

    let manifest = Manifest {
        bundle_format_version: BUNDLE_FORMAT_VERSION,
        source: SourceIdentity {
            url: SOURCE_URL.to_string(),
            commit,
        },
        target_triple: TARGET_TRIPLE.to_string(),
        host_triple: TARGET_TRIPLE.to_string(),
        msvc: ToolIdentity {
            path: identity.cxx_compiler.display().to_string(),
            version: identity.cxx_version,
        },
        cmake: ToolIdentity {
            path: tools.cmake_path.display().to_string(),
            version: tools.cmake_version,
        },
        nvcc: ToolIdentity {
            path: tools.nvcc_path.display().to_string(),
            version: tools.nvcc_version,
        },
        toolkit_version: tools.toolkit_version,
        architectures: architectures.clone(),
        cmake_options: cmake::configure_options(&architectures, &tools.nvcc_path),
        linkage: LINKAGE_POLICY.to_string(),
        external_dlls: system_dlls,
        files: files.clone(),
    };
    std::fs::create_dir_all(&request.out)
        .with_context(|| format!("create {}", request.out.display()))?;
    let manifest_path = request.out.join("llama-cuda-manifest.json");
    std::fs::write(&manifest_path, manifest.render()?)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let (zip, checksum) = pack(&request.out, &request.tag, &stage, &files, &manifest_path)?;

    Ok(BuildOutcome {
        zip,
        checksum,
        manifest: manifest_path,
        archs: architectures,
    })
}

#[cfg(test)]
#[path = "bundle-tests.rs"]
mod tests;
