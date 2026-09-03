//! End-to-end CUDA release build: verify, compile, account, pack.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::manifest::{
    BUNDLE_FORMAT_VERSION, BundleFile, LINKAGE_POLICY, Manifest, SourceIdentity, ToolIdentity,
    sha256_hex,
};
use crate::probe::{CommandRequest, Probe, SystemProbe};
use crate::{arch, cmake, deps, toolchain};

/// The only target triple the tool produces: it compiles on and for a
/// Windows x86-64 machine.
pub(crate) const TARGET_TRIPLE: &str = "x86_64-pc-windows-msvc";

/// Upstream repository a `--source` checkout comes from.
pub(crate) const SOURCE_URL: &str = "https://github.com/ggml-org/llama.cpp.git";

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
    /// CMake build tree lives under it in `work/` and is not part of the
    /// published output.
    pub out: PathBuf,
    /// Run the `--list-devices` smoke check after the build. Needs a GPU;
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
/// Returns an error when the host is not Windows x86-64, the checkout is
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

/// Locates `dumpbin.exe` through `vswhere`, returning the tool and the
/// directory the child needs on `PATH` for its own DLLs.
fn locate_dumpbin(
    probe: &impl Probe,
    env: &impl Fn(&str) -> Option<String>,
) -> anyhow::Result<(PathBuf, PathBuf)> {
    let vswhere = toolchain::vswhere_path(env)
        .context("vswhere.exe not found; a Visual Studio C++ workload is required")?;
    let request = CommandRequest::new(&vswhere).args([
        "-latest",
        "-products",
        "*",
        "-requires",
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-find",
        "VC\\Tools\\MSVC\\*\\bin\\Hostx64\\x64\\dumpbin.exe",
    ]);
    let output = probe.run(&request).context("locate dumpbin")?;
    anyhow::ensure!(
        output.success(),
        "vswhere failed (exit {}):\n{}",
        output.code,
        output.stderr
    );
    let dumpbin = output
        .stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .context("vswhere found no dumpbin.exe")?;
    let dumpbin = PathBuf::from(dumpbin);
    let dir = dumpbin
        .parent()
        .context("dumpbin path has no parent")?
        .to_path_buf();
    Ok((dumpbin, dir))
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

/// Enumerates the executable's PE import closure through dumpbin and
/// returns the external DLL names, split by who provides them.
fn inspect_closure(
    probe: &impl Probe,
    env: &impl Fn(&str) -> Option<String>,
    stage: &Path,
    bundled_names: &[String],
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let (dumpbin, dumpbin_dir) = locate_dumpbin(probe, env)?;
    let exe = stage.join("llama-server.exe");
    let deps_out = probe
        .run(
            &CommandRequest::new(&dumpbin)
                .args(["/dependents", &exe.display().to_string()])
                .path_prefix(&dumpbin_dir),
        )
        .context("inspect PE imports")?;
    anyhow::ensure!(
        deps_out.success(),
        "dumpbin failed (exit {}):\n{}",
        deps_out.code,
        deps_out.stderr
    );
    let imports = deps::parse_dumpbin_dependents(&deps_out.stdout);
    let mut cuda = Vec::new();
    let mut system = Vec::new();
    for dll in imports {
        match deps::classify(&dll) {
            deps::DllClass::CudaToolkit => cuda.push(dll),
            deps::DllClass::System => system.push(dll),
            deps::DllClass::Bundled => anyhow::ensure!(
                bundled_names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(&dll)),
                "imported DLL `{dll}` is neither a known system/CUDA DLL nor present \
                 in the bundle; the dependency closure is incomplete"
            ),
        }
    }
    cuda.sort();
    cuda.dedup();
    system.sort();
    system.dedup();
    Ok((cuda, system))
}

/// Copies each imported CUDA runtime DLL from the toolkit into the staging
/// directory, so the zip is self-contained and the end user needs only the
/// NVIDIA driver. The runtime directory is `<root>/bin/x64` on CUDA 13
/// (which moved the Windows runtime DLLs out of `bin`) or `<root>/bin` on
/// CUDA 12, probed in that order.
fn bundle_cuda_runtimes(
    nvcc_path: &Path,
    stage: &Path,
    cuda_dlls: &[String],
) -> anyhow::Result<()> {
    if cuda_dlls.is_empty() {
        return Ok(());
    }
    let toolkit_root = nvcc_path
        .parent()
        .and_then(Path::parent)
        .context("nvcc path has no toolkit root")?;
    let candidates = [
        toolkit_root.join("bin").join("x64"),
        toolkit_root.join("bin"),
    ];
    for dll in cuda_dlls {
        let source = candidates
            .iter()
            .map(|dir| dir.join(dll))
            .find(|candidate| candidate.is_file())
            .with_context(|| {
                format!(
                    "imported CUDA runtime DLL `{dll}` not found under {} or {}; \
                     the zip must ship it",
                    candidates[0].display(),
                    candidates[1].display()
                )
            })?;
        std::fs::copy(&source, stage.join(dll))
            .with_context(|| format!("stage {}", source.display()))?;
    }
    Ok(())
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

/// Full pipeline, with the command seam, environment, and host identity
/// injected for tests.
pub(crate) fn build_with(
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
mod tests {
    use super::*;
    use crate::probe::fake::{FakeProbe, fail, ok};

    const NVCC_OUTPUT: &str = "nvcc: NVIDIA (R) Cuda compiler driver\n\
                               Cuda compilation tools, release 13.3, V13.3.73\n";
    const COMMIT: &str = "fb0e6b621917488d623437349fb5361e0ac21c70";
    // Visual Studio generators fix the compiler through the toolset, so a
    // real cache carries the generator but no CMAKE_CXX_COMPILER entries.
    const CACHE: &str = "CMAKE_GENERATOR:INTERNAL=Visual Studio 18 2026\n";
    const COMPILER_CMAKE: &str = "set(CMAKE_CXX_COMPILER \"C:/VS/VC/Tools/MSVC/14.51/bin/Hostx64/x64/cl.exe\")\n\
         set(CMAKE_CXX_COMPILER_VERSION \"19.51.36256.0\")\n";
    const DUMPBIN_OUTPUT: &str = "Dump of file llama-server.exe\n\
                                  \n\
                                  \x20 Image has the following dependencies:\n\
                                  \n\
                                  \x20   cublas64_13.dll\n\
                                  \x20   KERNEL32.dll\n\
                                  \n\
                                  \x20 Summary\n";

    /// A synthetic Windows host: a llama.cpp checkout, an output directory
    /// pre-seeded with the tree a real cmake build would emit, and a tool
    /// directory holding fake `nvcc.exe`/`cmake.exe` plus the CUDA runtime
    /// DLL the closure names.
    struct SyntheticHost {
        _temp: tempfile::TempDir,
        source: PathBuf,
        out: PathBuf,
        tools: PathBuf,
        dumpbin: PathBuf,
        program_files_x86: PathBuf,
    }

    impl SyntheticHost {
        fn new() -> Self {
            let temp = tempfile::TempDir::new().unwrap();
            let root = temp.path();
            let source = root.join("llama.cpp");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(
                source.join("CMakeLists.txt"),
                b"cmake_minimum_required(VERSION 3.14)\n",
            )
            .unwrap();

            let out = root.join("out");
            let stage = out.join("work/llama-build/bin/Release");
            std::fs::create_dir_all(&stage).unwrap();
            std::fs::write(stage.join("llama-server.exe"), b"synthetic-exe").unwrap();
            std::fs::write(stage.join("ggml-cuda.dll"), b"synthetic-dll").unwrap();
            std::fs::write(out.join("work/llama-build/CMakeCache.txt"), CACHE).unwrap();
            let compiler_dir = out.join("work/llama-build/CMakeFiles/4.4.2");
            std::fs::create_dir_all(&compiler_dir).unwrap();
            std::fs::write(compiler_dir.join("CMakeCXXCompiler.cmake"), COMPILER_CMAKE).unwrap();

            // nvcc resolves to <tools>/bin/nvcc.exe, so the toolkit root is
            // <tools> and the CUDA 13 runtime directory is <tools>/bin/x64.
            let tools = root.join("tools");
            std::fs::create_dir_all(tools.join("bin/x64")).unwrap();
            std::fs::write(tools.join("bin/nvcc.exe"), b"").unwrap();
            std::fs::write(tools.join("bin/cmake.exe"), b"").unwrap();
            std::fs::write(tools.join("bin/x64/cublas64_13.dll"), b"synthetic-cudart").unwrap();

            let dumpbin_dir = root.join("vs/VC/Tools/MSVC/14.44/bin/Hostx64/x64");
            std::fs::create_dir_all(&dumpbin_dir).unwrap();
            let dumpbin = dumpbin_dir.join("dumpbin.exe");
            std::fs::write(&dumpbin, b"").unwrap();
            let program_files_x86 = root.join("pf");
            std::fs::create_dir_all(program_files_x86.join("Microsoft Visual Studio/Installer"))
                .unwrap();
            std::fs::write(
                program_files_x86.join("Microsoft Visual Studio/Installer/vswhere.exe"),
                b"",
            )
            .unwrap();

            Self {
                _temp: temp,
                source,
                out,
                tools,
                dumpbin,
                program_files_x86,
            }
        }

        fn env(&self) -> impl Fn(&str) -> Option<String> + '_ {
            move |name| match name {
                "PATH" => Some(self.tools.join("bin").display().to_string()),
                "PATHEXT" => Some(".exe".to_string()),
                "ProgramFiles(x86)" => Some(self.program_files_x86.display().to_string()),
                _ => None,
            }
        }

        fn probe(&self) -> FakeProbe {
            FakeProbe::default()
                .on("nvcc.exe --version", ok(NVCC_OUTPUT))
                .on("cmake.exe --version", ok("cmake version 4.4.2\n"))
                .on("rev-parse", ok(&format!("{COMMIT}\n")))
                .on("nvidia-smi", ok("12.0\n"))
                .on("--build", ok(""))
                .on("-S", ok(""))
                .on("vswhere", ok(&format!("{}\n", self.dumpbin.display())))
                .on("dumpbin", ok(DUMPBIN_OUTPUT))
                .on(
                    "llama-server.exe",
                    ok("ggml_cuda_init: found 1 CUDA devices\nDevice 0: NVIDIA RTX PRO 6000\n"),
                )
        }

        fn request(&self) -> BuildRequest {
            BuildRequest {
                source: self.source.clone(),
                tag: "b10082".to_string(),
                archs: Vec::new(),
                out: self.out.clone(),
                smoke: true,
            }
        }

        fn build(&self) -> anyhow::Result<BuildOutcome> {
            build_with(
                &self.probe(),
                &self.env(),
                "windows",
                "x86_64",
                &self.request(),
            )
        }
    }

    #[test]
    fn non_windows_host_is_rejected() {
        let host = SyntheticHost::new();
        let err = build_with(
            &host.probe(),
            &host.env(),
            "linux",
            "x86_64",
            &host.request(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("Windows x86-64 only"));
    }

    #[test]
    fn missing_source_is_an_error() {
        let host = SyntheticHost::new();
        let mut request = host.request();
        request.source = host.source.join("absent");
        let err =
            build_with(&host.probe(), &host.env(), "windows", "x86_64", &request).unwrap_err();
        assert!(format!("{err:#}").contains("is missing"));
    }

    #[test]
    fn unrecognized_source_is_an_error() {
        let temp = tempfile::TempDir::new().unwrap();
        let host = SyntheticHost::new();
        let mut request = host.request();
        request.source = temp.path().to_path_buf();
        let err =
            build_with(&host.probe(), &host.env(), "windows", "x86_64", &request).unwrap_err();
        assert!(format!("{err:#}").contains("does not look like llama.cpp"));
    }

    #[test]
    fn non_checkout_source_is_an_error() {
        let host = SyntheticHost::new();
        let probe = FakeProbe::default().on("rev-parse", fail(128, "not a git repository"));
        let err =
            build_with(&probe, &host.env(), "windows", "x86_64", &host.request()).unwrap_err();
        assert!(format!("{err:#}").contains("must be a git checkout"));
    }

    #[test]
    fn missing_cuda_toolkit_fails_the_build() {
        let temp = tempfile::TempDir::new().unwrap();
        let host = SyntheticHost::new();
        let empty = temp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let env = |name: &str| match name {
            "PATH" => Some(empty.display().to_string()),
            _ => host.env()(name),
        };
        let err =
            build_with(&host.probe(), &env, "windows", "x86_64", &host.request()).unwrap_err();
        assert!(format!("{err:#}").contains("CUDA Toolkit not found"));
    }

    #[test]
    fn cmake_failure_reports_bounded_stderr() {
        let host = SyntheticHost::new();
        let probe = FakeProbe::default()
            .on("nvcc.exe --version", ok(NVCC_OUTPUT))
            .on("cmake.exe --version", ok("cmake version 4.4.2\n"))
            .on("rev-parse", ok(&format!("{COMMIT}\n")))
            .on("nvidia-smi", ok("12.0\n"))
            .on("-S", fail(1, &"ninja: error\n".repeat(10_000)));
        let err =
            build_with(&probe, &host.env(), "windows", "x86_64", &host.request()).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("cmake configure failed (exit 1)"));
        assert!(message.len() < crate::probe::OUTPUT_LIMIT + 4096);
    }

    #[test]
    fn missing_compiler_identity_fails_the_build() {
        let host = SyntheticHost::new();
        std::fs::remove_file(
            host.out
                .join("work/llama-build/CMakeFiles/4.4.2/CMakeCXXCompiler.cmake"),
        )
        .unwrap();
        let err = host.build().unwrap_err();
        assert!(format!("{err:#}").contains("CMakeCXXCompiler.cmake"));
    }

    #[test]
    fn smoke_check_requires_a_cuda_device() {
        let host = SyntheticHost::new();
        let probe = FakeProbe::default()
            .on("nvcc.exe --version", ok(NVCC_OUTPUT))
            .on("cmake.exe --version", ok("cmake version 4.4.2\n"))
            .on("rev-parse", ok(&format!("{COMMIT}\n")))
            .on("nvidia-smi", ok("12.0\n"))
            .on("--build", ok(""))
            .on("-S", ok(""))
            .on("vswhere", ok("C:/VS/dumpbin.exe\n"))
            .on("dumpbin", ok(DUMPBIN_OUTPUT))
            .on("llama-server.exe", ok("no devices found\n"));
        let err =
            build_with(&probe, &host.env(), "windows", "x86_64", &host.request()).unwrap_err();
        assert!(format!("{err:#}").contains("no CUDA device"));
    }

    #[test]
    fn missing_cuda_runtime_dll_fails_the_build() {
        let host = SyntheticHost::new();
        std::fs::remove_file(host.tools.join("bin/x64/cublas64_13.dll")).unwrap();
        let err = host.build().unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("cublas64_13.dll"), "{message}");
        assert!(message.contains("the zip must ship it"), "{message}");
    }

    #[test]
    fn no_smoke_never_runs_the_server() {
        let host = SyntheticHost::new();
        let mut request = host.request();
        request.smoke = false;
        let probe = host.probe();
        build_with(&probe, &host.env(), "windows", "x86_64", &request).unwrap();
        assert!(
            !probe
                .invocations()
                .iter()
                .any(|line| line.contains("--list-devices"))
        );
    }

    #[test]
    fn explicit_archs_skip_nvidia_smi() {
        let host = SyntheticHost::new();
        let mut request = host.request();
        request.archs = vec![
            "89-real".to_string(),
            "120a-real".to_string(),
            "89-real".to_string(),
        ];
        let probe = host.probe();
        let outcome = build_with(&probe, &host.env(), "windows", "x86_64", &request).unwrap();
        assert_eq!(outcome.archs, vec!["120a-real", "89-real"]);
        assert!(
            !probe
                .invocations()
                .iter()
                .any(|line| line.contains("nvidia-smi"))
        );
    }

    #[test]
    fn full_synthetic_build_produces_manifest_zip_and_checksum() {
        let host = SyntheticHost::new();
        let probe = host.probe();
        let outcome =
            build_with(&probe, &host.env(), "windows", "x86_64", &host.request()).unwrap();
        assert_eq!(outcome.archs, vec!["120a-real"]);

        let manifest_text = std::fs::read_to_string(&outcome.manifest).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(&manifest_text).unwrap();
        assert_eq!(manifest["bundle_format_version"], 2);
        assert_eq!(manifest["source"]["commit"], COMMIT);
        assert_eq!(manifest["target_triple"], "x86_64-pc-windows-msvc");
        assert_eq!(manifest["toolkit_version"], "13.3");
        assert_eq!(manifest["architectures"], serde_json::json!(["120a-real"]));
        assert_eq!(manifest["linkage"], crate::manifest::LINKAGE_POLICY);
        // cublas64_13.dll is bundled now; only the system DLL stays external.
        assert_eq!(
            manifest["external_dlls"],
            serde_json::json!(["KERNEL32.dll"])
        );
        assert_eq!(manifest["msvc"]["version"], "19.51.36256.0");
        assert_eq!(
            manifest["msvc"]["path"],
            "C:/VS/VC/Tools/MSVC/14.51/bin/Hostx64/x64/cl.exe"
        );
        let files = manifest["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0]["name"], "cublas64_13.dll");
        assert_eq!(files[0]["sha256"], sha256_hex(b"synthetic-cudart"));
        assert_eq!(files[1]["name"], "ggml-cuda.dll");
        assert_eq!(files[2]["name"], "llama-server.exe");
        assert_eq!(files[2]["sha256"], sha256_hex(b"synthetic-exe"));

        assert_eq!(
            outcome.zip.file_name().unwrap(),
            "llama-server-cuda-blackwell-b10082-win-x64.zip"
        );
        let zip_file = std::fs::File::open(&outcome.zip).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "cublas64_13.dll",
                "ggml-cuda.dll",
                "llama-cuda-manifest.json",
                "llama-server.exe"
            ]
        );

        let checksum = std::fs::read_to_string(&outcome.checksum).unwrap();
        let expected = format!(
            "{}  llama-server-cuda-blackwell-b10082-win-x64.zip\n",
            sha256_hex(&std::fs::read(&outcome.zip).unwrap())
        );
        assert_eq!(checksum, expected);

        let invocations = probe.invocations();
        assert!(
            invocations
                .iter()
                .any(|line| line.contains("--list-devices"))
        );
        assert!(invocations.iter().any(|line| line.contains("/dependents")));
    }
}
