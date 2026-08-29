//! Build script: compiles and embeds the CUDA llama.cpp bundle when the
//! `llama-cuda` feature is enabled on a native Windows x86-64 build, and
//! no-ops otherwise. All output stays under `OUT_DIR`.
//!
//! Emits the `llama_cuda_embedded` cfg exactly when the generated
//! `llama_cuda_bundle` module will exist, so runtime code gates on one name
//! instead of repeating the feature/target triple. The target comes from
//! Cargo's environment variables, never host cfgs, so a cross-compile does
//! not claim an embedded bundle it did not produce.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(llama_cuda_embedded)");
    let target_is_windows_x86_64 = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64");
    if cfg!(feature = "llama-cuda") && target_is_windows_x86_64 {
        println!("cargo::rustc-cfg=llama_cuda_embedded");
    }
    #[cfg(feature = "llama-cuda")]
    match promptforge_gateway_build::build() {
        Ok(report) => {
            for path in report.rerun_if_changed {
                println!("cargo::rerun-if-changed={}", path.display());
            }
        }
        Err(err) => {
            eprintln!("promptforge-gateway: llama-cuda bundle build failed:\n{err:?}");
            std::process::exit(1);
        }
    }
}
