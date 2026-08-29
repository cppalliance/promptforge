//! PE dependency-closure accounting for the built runtime tree.
//!
//! The bundle ships every llama.cpp/GGML runtime file the build emits.
//! Windows system DLLs and declared CUDA Toolkit DLLs stay external: the
//! runtime host must carry the same compatible CUDA Toolkit.

/// Classification of one imported DLL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DllClass {
    /// Windows system or MSVC runtime DLL: external, provided by the OS.
    System,
    /// CUDA Toolkit runtime DLL: external, provided by the installed toolkit.
    CudaToolkit,
    /// Anything else: must be present in the bundle.
    Bundled,
}

/// Windows system and MSVC runtime DLLs that stay external, lowercase.
const SYSTEM_DLLS: &[&str] = &[
    "advapi32.dll",
    "bcrypt.dll",
    "bcryptprimitives.dll",
    "cfgmgr32.dll",
    "comdlg32.dll",
    "crypt32.dll",
    "dbghelp.dll",
    "gdi32.dll",
    "imm32.dll",
    "kernel32.dll",
    "msvcp140.dll",
    "msvcp140_1.dll",
    "msvcp140_2.dll",
    "msvcrt.dll",
    "ntdll.dll",
    "ole32.dll",
    "oleaut32.dll",
    "psapi.dll",
    "rpcrt4.dll",
    "sechost.dll",
    "setupapi.dll",
    "shell32.dll",
    "shlwapi.dll",
    "user32.dll",
    "userenv.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
    "version.dll",
    "winmm.dll",
    "wldap32.dll",
    "ws2_32.dll",
];

/// CUDA Toolkit runtime DLL prefixes that stay external, lowercase.
const CUDA_PREFIXES: &[&str] = &[
    "cublas",
    "cudart",
    "cudnn",
    "cufft",
    "cupti",
    "curand",
    "cusolver",
    "cusparse",
    "nvjitlink",
    "npp",
    "nvrtc",
    "nvtx",
];

/// Classifies one imported DLL name, case-insensitively.
#[must_use]
pub fn classify(dll: &str) -> DllClass {
    let lower = dll.to_ascii_lowercase();
    if lower.starts_with("api-ms-win-")
        || lower.starts_with("ext-ms-win-")
        || SYSTEM_DLLS.contains(&lower.as_str())
    {
        return DllClass::System;
    }
    if CUDA_PREFIXES.iter().any(|prefix| lower.starts_with(prefix)) {
        return DllClass::CudaToolkit;
    }
    DllClass::Bundled
}

/// Parses `dumpbin /dependents` output into imported DLL names.
#[must_use]
pub fn parse_dumpbin_dependents(output: &str) -> Vec<String> {
    let mut dlls = Vec::new();
    let mut in_deps = false;
    for line in output.lines() {
        if line.contains("Image has the following dependencies:") {
            in_deps = true;
            continue;
        }
        if !in_deps {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !dlls.is_empty() {
                break;
            }
            continue;
        }
        if line.starts_with(char::is_whitespace) && trimmed.to_ascii_lowercase().ends_with(".dll") {
            dlls.push(trimmed.to_string());
        } else if !line.starts_with(char::is_whitespace) {
            break;
        }
    }
    dlls
}

/// Splits the import closure into the external DLL names the runtime host
/// must provide, requiring every bundle-classified import to be present in
/// `bundled`.
///
/// # Errors
/// Returns an error when an import is neither a known system/CUDA DLL nor
/// present in the bundle.
pub fn external_closure(imports: &[String], bundled: &[String]) -> anyhow::Result<Vec<String>> {
    let mut external = Vec::new();
    for dll in imports {
        match classify(dll) {
            DllClass::System | DllClass::CudaToolkit => external.push(dll.clone()),
            DllClass::Bundled => anyhow::ensure!(
                bundled.iter().any(|name| name.eq_ignore_ascii_case(dll)),
                "imported DLL `{dll}` is neither a known system/CUDA DLL nor present \
                 in the bundle; the dependency closure is incomplete"
            ),
        }
    }
    external.sort();
    external.dedup();
    Ok(external)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMPBIN_OUTPUT: &str = "Microsoft (R) COFF/PE Dumper Version 14.44\n\
                                  \n\
                                  Dump of file llama-server.exe\n\
                                  \n\
                                  File Type: EXECUTABLE IMAGE\n\
                                  \n\
                                  \x20 Image has the following dependencies:\n\
                                  \n\
                                  \x20   cublas64_13.dll\n\
                                  \x20   cublasLt64_13.dll\n\
                                  \x20   KERNEL32.dll\n\
                                  \x20   MSVCP140.dll\n\
                                  \n\
                                  \x20 Summary\n";

    #[test]
    fn parses_dumpbin_dependents() {
        assert_eq!(
            parse_dumpbin_dependents(DUMPBIN_OUTPUT),
            vec![
                "cublas64_13.dll",
                "cublasLt64_13.dll",
                "KERNEL32.dll",
                "MSVCP140.dll"
            ]
        );
    }

    #[test]
    fn parses_empty_dependencies() {
        assert!(parse_dumpbin_dependents("File Type: EXECUTABLE IMAGE\n").is_empty());
    }

    #[test]
    fn classifies_system_dlls() {
        assert_eq!(classify("KERNEL32.dll"), DllClass::System);
        assert_eq!(classify("vcruntime140.dll"), DllClass::System);
        assert_eq!(
            classify("api-ms-win-core-file-l1-1-0.dll"),
            DllClass::System
        );
    }

    #[test]
    fn classifies_cuda_toolkit_dlls() {
        assert_eq!(classify("cudart64_13.dll"), DllClass::CudaToolkit);
        assert_eq!(classify("cublasLt64_13.dll"), DllClass::CudaToolkit);
        assert_eq!(classify("nvrtc64_130_0.dll"), DllClass::CudaToolkit);
    }

    #[test]
    fn classifies_everything_else_as_bundled() {
        assert_eq!(classify("ggml-cuda.dll"), DllClass::Bundled);
        assert_eq!(classify("llama.dll"), DllClass::Bundled);
    }

    #[test]
    fn closure_keeps_system_and_cuda_external() {
        let imports: Vec<String> = ["KERNEL32.dll", "cublas64_13.dll"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let external = external_closure(&imports, &[]).unwrap();
        assert_eq!(external, vec!["KERNEL32.dll", "cublas64_13.dll"]);
    }

    #[test]
    fn closure_accepts_bundled_dlls_present_in_the_tree() {
        let imports: Vec<String> = ["ggml-cuda.dll"].iter().map(|s| (*s).to_string()).collect();
        let bundled: Vec<String> = ["llama-server.exe", "ggml-cuda.dll"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        assert!(external_closure(&imports, &bundled).unwrap().is_empty());
    }

    #[test]
    fn closure_rejects_unbundled_unknown_dlls() {
        let imports: Vec<String> = ["mystery.dll"].iter().map(|s| (*s).to_string()).collect();
        let err = external_closure(&imports, &[]).unwrap_err();
        assert!(err.to_string().contains("mystery.dll"));
    }
}
