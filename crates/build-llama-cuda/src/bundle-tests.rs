//! Unit tests for the end-to-end release build on a synthetic machine.

use super::*;
use crate::probe::fake::{FakeProbe, fail, ok};

const NVCC_OUTPUT: &str = "nvcc: NVIDIA (R) Cuda compiler driver\n\
                           Cuda compilation tools, release 13.3, V13.3.73\n";
const COMMIT: &str = "fb0e6b621917488d623437349fb5361e0ac21c70";
// Visual Studio generators fix the compiler through the toolset, so a
// real cache names the generator but omits CMAKE_CXX_COMPILER entries.
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

/// A synthetic Windows machine: a llama.cpp checkout, an output directory
/// pre-seeded with the tree a real cmake build would emit, and a tool
/// directory holding fake `nvcc.exe`/`cmake.exe` plus the CUDA runtime
/// DLL the closure names.
struct SyntheticMachine {
    _temp: tempfile::TempDir,
    source: PathBuf,
    out: PathBuf,
    tools: PathBuf,
    dumpbin: PathBuf,
    program_files_x86: PathBuf,
}

impl SyntheticMachine {
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
fn non_windows_machine_is_rejected() {
    let machine = SyntheticMachine::new();
    let err = build_with(
        &machine.probe(),
        &machine.env(),
        "linux",
        "x86_64",
        &machine.request(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("Windows x86-64 only"));
}

#[test]
fn missing_source_is_an_error() {
    let machine = SyntheticMachine::new();
    let mut request = machine.request();
    request.source = machine.source.join("absent");
    let err = build_with(
        &machine.probe(),
        &machine.env(),
        "windows",
        "x86_64",
        &request,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("is missing"));
}

#[test]
fn unrecognized_source_is_an_error() {
    let temp = tempfile::TempDir::new().unwrap();
    let machine = SyntheticMachine::new();
    let mut request = machine.request();
    request.source = temp.path().to_path_buf();
    let err = build_with(
        &machine.probe(),
        &machine.env(),
        "windows",
        "x86_64",
        &request,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("does not look like llama.cpp"));
}

#[test]
fn non_checkout_source_is_an_error() {
    let machine = SyntheticMachine::new();
    let probe = FakeProbe::default().on("rev-parse", fail(128, "not a git repository"));
    let err = build_with(
        &probe,
        &machine.env(),
        "windows",
        "x86_64",
        &machine.request(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("must be a git checkout"));
}

#[test]
fn missing_cuda_toolkit_fails_the_build() {
    let temp = tempfile::TempDir::new().unwrap();
    let machine = SyntheticMachine::new();
    let empty = temp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let env = |name: &str| match name {
        "PATH" => Some(empty.display().to_string()),
        _ => machine.env()(name),
    };
    let err = build_with(
        &machine.probe(),
        &env,
        "windows",
        "x86_64",
        &machine.request(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("CUDA Toolkit not found"));
}

#[test]
fn cmake_failure_reports_bounded_stderr() {
    let machine = SyntheticMachine::new();
    let probe = FakeProbe::default()
        .on("nvcc.exe --version", ok(NVCC_OUTPUT))
        .on("cmake.exe --version", ok("cmake version 4.4.2\n"))
        .on("rev-parse", ok(&format!("{COMMIT}\n")))
        .on("nvidia-smi", ok("12.0\n"))
        .on("-S", fail(1, &"ninja: error\n".repeat(10_000)));
    let err = build_with(
        &probe,
        &machine.env(),
        "windows",
        "x86_64",
        &machine.request(),
    )
    .unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("cmake configure failed (exit 1)"));
    assert!(message.len() < crate::probe::OUTPUT_LIMIT + 4096);
}

#[test]
fn missing_compiler_identity_fails_the_build() {
    let machine = SyntheticMachine::new();
    std::fs::remove_file(
        machine
            .out
            .join("work/llama-build/CMakeFiles/4.4.2/CMakeCXXCompiler.cmake"),
    )
    .unwrap();
    let err = machine.build().unwrap_err();
    assert!(format!("{err:#}").contains("CMakeCXXCompiler.cmake"));
}

#[test]
fn smoke_check_requires_a_cuda_device() {
    let machine = SyntheticMachine::new();
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
    let err = build_with(
        &probe,
        &machine.env(),
        "windows",
        "x86_64",
        &machine.request(),
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("no CUDA device"));
}

#[test]
fn missing_cuda_runtime_dll_fails_the_build() {
    let machine = SyntheticMachine::new();
    std::fs::remove_file(machine.tools.join("bin/x64/cublas64_13.dll")).unwrap();
    let err = machine.build().unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("cublas64_13.dll"), "{message}");
    assert!(message.contains("the zip must ship it"), "{message}");
}

#[test]
fn no_smoke_never_runs_the_server() {
    let machine = SyntheticMachine::new();
    let mut request = machine.request();
    request.smoke = false;
    let probe = machine.probe();
    build_with(&probe, &machine.env(), "windows", "x86_64", &request).unwrap();
    assert!(
        !probe
            .invocations()
            .iter()
            .any(|line| line.contains("--list-devices"))
    );
}

#[test]
fn explicit_archs_skip_nvidia_smi() {
    let machine = SyntheticMachine::new();
    let mut request = machine.request();
    request.archs = vec![
        "89-real".to_string(),
        "120a-real".to_string(),
        "89-real".to_string(),
    ];
    let probe = machine.probe();
    let outcome = build_with(&probe, &machine.env(), "windows", "x86_64", &request).unwrap();
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
    let machine = SyntheticMachine::new();
    let probe = machine.probe();
    let outcome = build_with(
        &probe,
        &machine.env(),
        "windows",
        "x86_64",
        &machine.request(),
    )
    .unwrap();
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
