//! The PE dependency closure: dumpbin discovery, import classification,
//! and CUDA runtime staging.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use crate::probe::{CommandRequest, Probe};
use crate::{deps, toolchain};

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

/// Enumerates the executable's PE import closure through dumpbin and
/// returns the external DLL names, split by who provides them.
pub(super) fn inspect_closure(
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
pub(super) fn bundle_cuda_runtimes(
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
