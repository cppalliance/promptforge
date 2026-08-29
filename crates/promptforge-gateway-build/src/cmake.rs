//! CMake configure and build command plans for the pinned llama.cpp tree.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::probe::CommandRequest;

/// Identity facts recovered from a generated `CMakeCache.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheIdentity {
    /// Generator CMake selected (for example `Visual Studio 17 2022`).
    pub generator: String,
    /// C++ compiler executable CMake resolved (the MSVC `cl.exe`).
    pub cxx_compiler: PathBuf,
    /// C++ compiler version string CMake recorded.
    pub cxx_version: String,
}

/// Parses the generator and C++ compiler identity out of `CMakeCache.txt`.
///
/// # Errors
/// Returns an error when any of the three entries is missing.
pub fn parse_cache(cache: &str) -> anyhow::Result<CacheIdentity> {
    let entry = |key: &str| {
        cache
            .lines()
            .find_map(|line| line.trim().strip_prefix(key))
            .and_then(|rest| {
                rest.split_once('=')
                    .map(|(_, value)| value.trim().to_string())
            })
    };
    let generator =
        entry("CMAKE_GENERATOR:INTERNAL").context("CMakeCache.txt lacks CMAKE_GENERATOR")?;
    let cxx_compiler =
        entry("CMAKE_CXX_COMPILER:FILEPATH").context("CMakeCache.txt lacks CMAKE_CXX_COMPILER")?;
    let cxx_version = entry("CMAKE_CXX_COMPILER_VERSION:STRING")
        .context("CMakeCache.txt lacks CMAKE_CXX_COMPILER_VERSION")?;
    Ok(CacheIdentity {
        generator,
        cxx_compiler: PathBuf::from(cxx_compiler),
        cxx_version,
    })
}

/// The full material CMake option set for the bundle configure step, in
/// canonical (sorted) `-DKEY=VALUE` form.
///
/// Project libraries are static, CUDA is on, upstream tests, examples, the
/// app, the web UI, OpenSSL, LLGuidance, and OpenMP are off, and the
/// architecture list is exactly the detected set. `LLAMA_BUILD_TOOLS` stays
/// on because the pin only defines the `llama-server` target from the tools
/// tree; the build step compiles that target alone, so unrelated upstream
/// programs are configured but never built. `LLAMA_USE_PREBUILT_UI` is off
/// because its default would download assets from the network at build time.
#[must_use]
pub fn configure_options(archs: &[String], nvcc: &Path) -> Vec<String> {
    let mut options = vec![
        format!("-DCMAKE_CUDA_ARCHITECTURES={}", archs.join(";")),
        format!("-DCMAKE_CUDA_COMPILER={}", nvcc.display()),
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        "-DBUILD_SHARED_LIBS=OFF".to_string(),
        "-DGGML_BACKEND_DL=OFF".to_string(),
        "-DGGML_CCACHE=OFF".to_string(),
        "-DGGML_CUDA=ON".to_string(),
        "-DGGML_CUDA_FA=ON".to_string(),
        "-DGGML_CUDA_GRAPHS=ON".to_string(),
        "-DGGML_CUDA_NCCL=OFF".to_string(),
        "-DGGML_LTO=OFF".to_string(),
        "-DGGML_NATIVE=OFF".to_string(),
        "-DGGML_OPENMP=OFF".to_string(),
        "-DGGML_STATIC=ON".to_string(),
        "-DLLAMA_ALL_WARNINGS=OFF".to_string(),
        "-DLLAMA_BUILD_APP=OFF".to_string(),
        "-DLLAMA_BUILD_COMMON=ON".to_string(),
        "-DLLAMA_BUILD_EXAMPLES=OFF".to_string(),
        "-DLLAMA_BUILD_SERVER=ON".to_string(),
        "-DLLAMA_BUILD_TESTS=OFF".to_string(),
        "-DLLAMA_BUILD_TOOLS=ON".to_string(),
        "-DLLAMA_BUILD_UI=OFF".to_string(),
        "-DLLAMA_FATAL_WARNINGS=OFF".to_string(),
        "-DLLAMA_LLGUIDANCE=OFF".to_string(),
        "-DLLAMA_OPENSSL=OFF".to_string(),
        "-DLLAMA_USE_PREBUILT_UI=OFF".to_string(),
    ];
    options.sort();
    options
}

/// Builds the configure and build invocations compiling `submodule` into
/// `build_dir` as a Release `llama-server`.
#[must_use]
pub fn plan(
    submodule: &Path,
    build_dir: &Path,
    cmake: &Path,
    archs: &[String],
    nvcc: &Path,
) -> (CommandRequest, CommandRequest) {
    let mut configure_args = vec![
        "-S".to_string(),
        submodule.display().to_string(),
        "-B".to_string(),
        build_dir.display().to_string(),
    ];
    configure_args.extend(configure_options(archs, nvcc));
    let configure = CommandRequest::new(cmake).args(configure_args);
    let build = CommandRequest::new(cmake).args([
        "--build".to_string(),
        build_dir.display().to_string(),
        "--config".to_string(),
        "Release".to_string(),
        "--target".to_string(),
        "llama-server".to_string(),
        "--parallel".to_string(),
    ]);
    (configure, build)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cache_identity() {
        let cache = "# comment\n\
                     CMAKE_GENERATOR:INTERNAL=Visual Studio 17 2022\n\
                     CMAKE_CXX_COMPILER:FILEPATH=C:/VS/VC/Tools/MSVC/14.44/bin/Hostx64/x64/cl.exe\n\
                     CMAKE_CXX_COMPILER_VERSION:STRING=19.44.35219.0\n";
        let identity = parse_cache(cache).unwrap();
        assert_eq!(identity.generator, "Visual Studio 17 2022");
        assert_eq!(
            identity.cxx_compiler,
            PathBuf::from("C:/VS/VC/Tools/MSVC/14.44/bin/Hostx64/x64/cl.exe")
        );
        assert_eq!(identity.cxx_version, "19.44.35219.0");
    }

    #[test]
    fn missing_cache_entries_are_errors() {
        assert!(parse_cache("").is_err());
    }

    #[test]
    fn configure_options_are_canonical_and_complete() {
        let options = configure_options(
            &["120a-real".to_string()],
            Path::new("C:/CUDA/bin/nvcc.exe"),
        );
        let mut sorted = options.clone();
        sorted.sort();
        assert_eq!(
            options, sorted,
            "option list must be emitted in canonical order"
        );
        for required in [
            "-DGGML_CUDA=ON",
            "-DGGML_STATIC=ON",
            "-DGGML_NATIVE=OFF",
            "-DGGML_BACKEND_DL=OFF",
            "-DGGML_OPENMP=OFF",
            "-DBUILD_SHARED_LIBS=OFF",
            "-DLLAMA_BUILD_SERVER=ON",
            "-DLLAMA_BUILD_TESTS=OFF",
            "-DLLAMA_BUILD_EXAMPLES=OFF",
            "-DLLAMA_BUILD_TOOLS=ON",
            "-DLLAMA_BUILD_APP=OFF",
            "-DLLAMA_BUILD_UI=OFF",
            "-DLLAMA_USE_PREBUILT_UI=OFF",
            "-DLLAMA_OPENSSL=OFF",
            "-DLLAMA_LLGUIDANCE=OFF",
            "-DCMAKE_BUILD_TYPE=Release",
            "-DCMAKE_CUDA_ARCHITECTURES=120a-real",
            "-DCMAKE_CUDA_COMPILER=C:/CUDA/bin/nvcc.exe",
        ] {
            assert!(
                options.contains(&required.to_string()),
                "missing {required}"
            );
        }
    }

    #[test]
    fn plan_emits_exact_invocations() {
        let (configure, build) = plan(
            Path::new("ws/third_party/llama.cpp"),
            Path::new("out/llama-build"),
            Path::new("cmake"),
            &["120a-real".to_string()],
            Path::new("nvcc"),
        );
        assert_eq!(configure.program, PathBuf::from("cmake"));
        assert_eq!(
            configure.args[0..4],
            ["-S", "ws/third_party/llama.cpp", "-B", "out/llama-build"]
        );
        assert!(configure.args.contains(&"-DGGML_CUDA=ON".to_string()));
        assert_eq!(
            build.args,
            [
                "--build",
                "out/llama-build",
                "--config",
                "Release",
                "--target",
                "llama-server",
                "--parallel"
            ]
        );
    }
}
