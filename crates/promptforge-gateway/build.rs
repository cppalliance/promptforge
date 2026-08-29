//! Build script: compiles and embeds the CUDA llama.cpp bundle when the
//! `llama-cuda` feature is enabled on a native Windows x86-64 build, and
//! no-ops otherwise. All output stays under `OUT_DIR`.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
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
