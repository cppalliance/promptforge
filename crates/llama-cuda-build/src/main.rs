//! Command-line driver for the CUDA `llama-server` release build.
//!
//! Runs on a Windows x86-64 machine with the CUDA Toolkit and CMake (the
//! GitHub-hosted builder installs both); a GPU is needed only for the
//! smoke check, which `--no-smoke` skips. See
//! `.github/workflows/llama-cuda-blackwell.yml` for the caller.

use std::path::PathBuf;
use std::process::ExitCode;

use llama_cuda_build::{BuildRequest, build};

const USAGE: &str = "\
llama-cuda-build - build the CUDA llama-server release zip (Windows x64)

USAGE:
    llama-cuda-build --source <folder> --tag <tag> --out <folder> [OPTIONS]

REQUIRED:
    --source <folder>   llama.cpp checkout to compile (a git checkout, not a tarball)
    --tag <tag>         llama.cpp release tag the checkout represents (for example
                        b10082); names the zip llama-server-cuda-blackwell-<tag>-win-x64.zip
    --out <folder>      output directory for the zip, its .sha256, and the manifest

OPTIONS:
    --arch <list>       comma-separated CUDA architectures (for example 120a-real);
                        defaults to the build machine's GPUs detected via nvidia-smi
    --no-smoke          skip the --list-devices smoke check, which needs a GPU
    -h, --help          print this text
";

/// Parses the command line into a [`BuildRequest`]. Every error exit
/// prints the usage text.
fn parse_args(args: &[String]) -> Result<BuildRequest, String> {
    let mut source: Option<PathBuf> = None;
    let mut tag: Option<String> = None;
    let mut out: Option<PathBuf> = None;
    let mut archs = Vec::new();
    let mut smoke = true;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = |iter: &mut std::slice::Iter<'_, String>| {
            iter.next()
                .filter(|value| !value.starts_with("--"))
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value\n\n{USAGE}"))
        };
        match arg.as_str() {
            "--source" => source = Some(PathBuf::from(value(&mut iter)?)),
            "--tag" => tag = Some(value(&mut iter)?),
            "--out" => out = Some(PathBuf::from(value(&mut iter)?)),
            "--arch" => {
                for entry in value(&mut iter)?.split(',') {
                    let entry = entry.trim();
                    if entry.is_empty()
                        || !entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                    {
                        return Err(format!(
                            "malformed --arch entry `{entry}` (expected for example 120a-real)\n\n{USAGE}"
                        ));
                    }
                    archs.push(entry.to_string());
                }
            }
            "--no-smoke" => smoke = false,
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(format!("unknown argument `{other}`\n\n{USAGE}")),
        }
    }

    Ok(BuildRequest {
        source: source.ok_or_else(|| format!("missing required --source\n\n{USAGE}"))?,
        tag: tag.ok_or_else(|| format!("missing required --tag\n\n{USAGE}"))?,
        out: out.ok_or_else(|| format!("missing required --out\n\n{USAGE}"))?,
        archs,
        smoke,
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let request = match parse_args(&args) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    match build(&request) {
        Ok(outcome) => {
            println!(
                "built {} (arch {})",
                outcome.zip.display(),
                outcome.archs.join(", ")
            );
            println!("checksum {}", outcome.checksum.display());
            println!("manifest {}", outcome.manifest.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("llama-cuda-build failed:\n{error:#}");
            ExitCode::FAILURE
        }
    }
}
