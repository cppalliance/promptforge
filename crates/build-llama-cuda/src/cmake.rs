//! CMake configure and build command plans for the pinned llama.cpp tree.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::probe::CommandRequest;

/// Identity facts recovered from a generated CMake build tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheIdentity {
    /// Generator CMake selected (for example `Visual Studio 18 2026`).
    pub generator: String,
    /// C++ compiler executable CMake resolved (the MSVC `cl.exe`).
    pub cxx_compiler: PathBuf,
    /// C++ compiler version string CMake recorded.
    pub cxx_version: String,
}

/// Parses the generator out of `CMakeCache.txt`.
///
/// # Errors
/// Returns an error when `CMAKE_GENERATOR` is missing.
pub fn parse_generator(cache: &str) -> anyhow::Result<String> {
    cache
        .lines()
        .find_map(|line| line.trim().strip_prefix("CMAKE_GENERATOR:INTERNAL"))
        .and_then(|rest| {
            rest.split_once('=')
                .map(|(_, value)| value.trim().to_string())
        })
        .context("CMakeCache.txt lacks CMAKE_GENERATOR")
}

/// Parses the C++ compiler identity out of `CMakeCXXCompiler.cmake`.
///
/// The compiler identity comes from this file rather than `CMakeCache.txt`
/// because the Visual Studio generators never write `CMAKE_CXX_COMPILER` or
/// `CMAKE_CXX_COMPILER_VERSION` cache entries: with those generators the
/// toolset fixes the compiler, so only the per-language compiler file under
/// `CMakeFiles/<version>/` records what was resolved.
///
/// # Errors
/// Returns an error when either `set(...)` entry is missing.
pub fn parse_compiler_cmake(content: &str) -> anyhow::Result<(PathBuf, String)> {
    let entry = |key: &str| {
        content.lines().find_map(|line| {
            line.trim()
                .strip_prefix(&format!("set({key} \""))
                .and_then(|rest| rest.strip_suffix("\")"))
                .map(str::to_string)
        })
    };
    let compiler =
        entry("CMAKE_CXX_COMPILER").context("CMakeCXXCompiler.cmake lacks CMAKE_CXX_COMPILER")?;
    let version = entry("CMAKE_CXX_COMPILER_VERSION")
        .context("CMakeCXXCompiler.cmake lacks CMAKE_CXX_COMPILER_VERSION")?;
    Ok((PathBuf::from(compiler), version))
}

/// Locates `CMakeFiles/<version>/CMakeCXXCompiler.cmake` under `build_dir`.
///
/// # Errors
/// Returns an error when `CMakeFiles` is unreadable or no configured
/// compiler file exists.
pub fn compiler_cmake_path(build_dir: &Path) -> anyhow::Result<PathBuf> {
    let cmake_files = build_dir.join("CMakeFiles");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&cmake_files)
        .with_context(|| format!("read {}", cmake_files.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let candidate = dir.join("CMakeCXXCompiler.cmake");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "no CMakeFiles/<version>/CMakeCXXCompiler.cmake under {}",
        build_dir.display()
    )
}

/// The full material CMake option set for the bundle configure step, in
/// canonical (sorted) `-DKEY=VALUE` form.
///
/// Project libraries are static, CUDA is on, upstream tests, examples, the
/// app, the web UI, OpenSSL, LLGuidance, and OpenMP are off, and the
/// architecture list is exactly the detected set. `GGML_CUDA_FA_ALL_QUANTS`
/// is on because the gateway launches `llama-server` with mixed KV cache
/// types (`--cache-type-k q8_0 --cache-type-v q4_0`), and the CUDA flash
/// attention kernel rejects mixed K/V quant types unless every quant
/// combination is compiled in; without it FLASH_ATTN_EXT falls back to the
/// CPU backend on every layer. `LLAMA_BUILD_TOOLS` stays
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
        "-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string(),
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

/// Builds the configure and build invocations compiling `source` into
/// `build_dir` as a Release `llama-server`.
#[must_use]
pub fn plan(
    source: &Path,
    build_dir: &Path,
    cmake: &Path,
    archs: &[String],
    nvcc: &Path,
) -> (CommandRequest, CommandRequest) {
    let mut configure_args = vec![
        "-S".to_string(),
        source.display().to_string(),
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
    fn parses_generator_from_cache() {
        let cache = "# comment\n\
                     CMAKE_GENERATOR:INTERNAL=Visual Studio 18 2026\n\
                     CMAKE_GENERATOR_INSTANCE:INTERNAL=C:/VS18\n";
        assert_eq!(parse_generator(cache).unwrap(), "Visual Studio 18 2026");
    }

    #[test]
    fn visual_studio_cache_carries_no_compiler_entries() {
        // The Visual Studio generators fix the compiler through the toolset,
        // so their caches omit CMAKE_CXX_COMPILER; the generator parse must
        // not depend on those entries.
        let cache = "CMAKE_GENERATOR:INTERNAL=Visual Studio 18 2026\n";
        assert_eq!(parse_generator(cache).unwrap(), "Visual Studio 18 2026");
    }

    #[test]
    fn missing_generator_is_an_error() {
        assert!(parse_generator("").is_err());
    }

    #[test]
    fn parses_compiler_cmake_identity() {
        let content = "set(CMAKE_CXX_COMPILER \"C:/VS/VC/Tools/MSVC/14.51/bin/Hostx64/x64/cl.exe\")\n\
                       set(CMAKE_CXX_COMPILER_ID \"MSVC\")\n\
                       set(CMAKE_CXX_COMPILER_VERSION \"19.51.36256.0\")\n";
        let (compiler, version) = parse_compiler_cmake(content).unwrap();
        assert_eq!(
            compiler,
            PathBuf::from("C:/VS/VC/Tools/MSVC/14.51/bin/Hostx64/x64/cl.exe")
        );
        assert_eq!(version, "19.51.36256.0");
    }

    #[test]
    fn missing_compiler_cmake_entries_are_errors() {
        assert!(parse_compiler_cmake("").is_err());
        assert!(
            parse_compiler_cmake("set(CMAKE_CXX_COMPILER \"cl.exe\")\n").is_err(),
            "version alone missing must fail"
        );
    }

    #[test]
    fn locates_compiler_cmake_under_versioned_dir() {
        let temp = tempfile::TempDir::new().unwrap();
        let build_dir = temp.path();
        assert!(compiler_cmake_path(build_dir).is_err());
        let versioned = build_dir.join("CMakeFiles/4.4.2");
        std::fs::create_dir_all(&versioned).unwrap();
        std::fs::write(versioned.join("CMakeCXXCompiler.cmake"), b"").unwrap();
        assert_eq!(
            compiler_cmake_path(build_dir).unwrap(),
            versioned.join("CMakeCXXCompiler.cmake")
        );
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
            "-DGGML_CUDA_FA=ON",
            "-DGGML_CUDA_FA_ALL_QUANTS=ON",
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
    fn configure_options_enable_all_flash_attention_quants() {
        // The gateway runs llama-server with mixed KV cache types
        // (--cache-type-k q8_0 --cache-type-v q4_0); without
        // GGML_CUDA_FA_ALL_QUANTS the CUDA flash attention kernel rejects
        // the combination and FLASH_ATTN_EXT silently falls back to the CPU
        // backend on every layer.
        let options = configure_options(
            &["120a-real".to_string()],
            Path::new("C:/CUDA/bin/nvcc.exe"),
        );
        assert!(
            options.contains(&"-DGGML_CUDA_FA_ALL_QUANTS=ON".to_string()),
            "dropping GGML_CUDA_FA_ALL_QUANTS returns flash attention to CPU fallback"
        );
    }

    #[test]
    fn plan_emits_exact_invocations() {
        let (configure, build) = plan(
            Path::new("ws/llama.cpp"),
            Path::new("out/llama-build"),
            Path::new("cmake"),
            &["120a-real".to_string()],
            Path::new("nvcc"),
        );
        assert_eq!(configure.program, PathBuf::from("cmake"));
        assert_eq!(
            configure.args[0..4],
            ["-S", "ws/llama.cpp", "-B", "out/llama-build"]
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
