//! Verification of the prebuilt config UI artifact (`ui/dist/` plus its
//! `manifest.json`) that release builds embed. Shared between `build.rs`
//! and the crate's test suite through `#[path]` includes, so the release
//! gate and its tests run the same code. The input-hash algorithm is
//! mirrored exactly in `ui/manifest.mjs`: sha256 over the byte-sorted,
//! ui-relative forward-slash paths of every build input, feeding path
//! bytes, a `0x00`, the content bytes, and a `0x00` per file.

use std::fs;
use std::path::Path;

use sha2::Digest;

/// Manifest schema version; bump when the fields change. Mirrored in
/// `ui/manifest.mjs`.
pub(crate) const MANIFEST_VERSION: u32 = 1;

/// Static UI files copied verbatim into `ui/dist/`. Mirrored in
/// `ui/build.mjs`.
pub(crate) const STATIC_FILES: &[&str] = &["index.html", "icons/promptforge-icon-1.png"];

/// Build scripts and manifests whose contents change the bundle without
/// touching `src/`. Mirrored in `ui/manifest.mjs`.
const BUILD_INPUTS: &[&str] = &[
    "build.mjs",
    "manifest.mjs",
    "package.json",
    "package-lock.json",
    "tsconfig.json",
];

/// The dist-relative names `crate::routes` serves; a packaged artifact
/// that lacks one would 404 in release only.
const REQUIRED_SERVED: &[&str] = &["app.js", "icons/promptforge-icon-1.png", "index.html"];

const INSTRUCTIONS: &str = "\
Release builds embed the verified UI artifact in ui/dist/. The build already
tried to produce the artifact with `node build.mjs --package`; to produce it
by hand and see the full packaging output:

    cd crates/promptforge-gateway-config-ui/ui
    npm ci              # once per checkout
    npm run package

Debug builds (`cargo build` without `--release`) build the UI in place and
need no artifact.";

/// Lowercase hex digits for digest encoding.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Verifies the artifact under `ui/dist/`: manifest present and current,
/// inputs unchanged since packaging, minified, and every served file
/// present and non-empty. The error names the reason and prints the
/// recovery instructions.
pub(crate) fn verify(ui_dir: &Path) -> Result<(), String> {
    verify_inner(ui_dir).map_err(|reason| {
        format!("the config UI artifact at ui/dist/ cannot be embedded: {reason}\n\n{INSTRUCTIONS}")
    })
}

fn verify_inner(ui_dir: &Path) -> Result<(), String> {
    let dist_dir = ui_dir.join("dist");
    let text = fs::read_to_string(dist_dir.join("manifest.json"))
        .map_err(|_| "dist/manifest.json is absent".to_string())?;
    let manifest: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("dist/manifest.json is not valid JSON: {error}"))?;

    let version = manifest.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(MANIFEST_VERSION)) {
        return Err(format!(
            "dist/manifest.json has version {version:?}, expected {MANIFEST_VERSION}"
        ));
    }
    if manifest
        .get("minified")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Err(
            "the artifact is not minified; only `npm run package` output may be embedded"
                .to_string(),
        );
    }

    let recorded = manifest
        .get("inputHash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "dist/manifest.json has no inputHash string".to_string())?;
    let actual = compute_input_hash(ui_dir)?;
    if recorded != actual {
        return Err("the UI sources changed after the artifact was packaged".to_string());
    }

    let files: Vec<&str> = manifest
        .get("files")
        .and_then(serde_json::Value::as_array)
        .map(|array| array.iter().filter_map(serde_json::Value::as_str).collect())
        .ok_or_else(|| "dist/manifest.json has no files list".to_string())?;
    for served in REQUIRED_SERVED {
        if !files.contains(served) {
            return Err(format!(
                "the artifact is missing {served}, which the server routes serve"
            ));
        }
    }
    for file in files {
        if file.contains("..") || Path::new(file).is_absolute() {
            return Err(format!(
                "dist/manifest.json lists {file}, which escapes ui/dist/"
            ));
        }
        let length = fs::metadata(dist_dir.join(file))
            .map_err(|_| format!("the artifact is missing {file} on disk"))?
            .len();
        if length == 0 {
            return Err(format!("the artifact's {file} is empty"));
        }
    }
    Ok(())
}

/// Hashes every input the bundle depends on: `src/**`, the static files,
/// and the build scripts and manifests. Any change to any of them
/// invalidates a packaged artifact.
pub(crate) fn compute_input_hash(ui_dir: &Path) -> Result<String, String> {
    let mut inputs = Vec::new();
    collect_files(&ui_dir.join("src"), ui_dir, &mut inputs)?;
    inputs.extend(
        STATIC_FILES
            .iter()
            .chain(BUILD_INPUTS)
            .map(|file| (*file).to_string()),
    );
    inputs.sort();
    let mut hasher = sha2::Sha256::new();
    for relative in inputs {
        let content = fs::read(ui_dir.join(&relative))
            .map_err(|error| format!("read ui/{relative} for the input hash: {error}"))?;
        hasher.update(relative.as_bytes());
        hasher.update([0u8]);
        hasher.update(content);
        hasher.update([0u8]);
    }
    let bytes = hasher.finalize();
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(char::from(HEX[usize::from(byte >> 4)]));
        hex.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    Ok(hex)
}

/// Collects every file under `dir` as `ui_dir`-relative forward-slash
/// paths.
fn collect_files(dir: &Path, ui_dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|error| format!("read {}: {error}", dir.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("list {}: {error}", dir.display()))?
            .path();
        if path.is_dir() {
            collect_files(&path, ui_dir, out)?;
        } else {
            let relative = path
                .strip_prefix(ui_dir)
                .map_err(|_| format!("{} escapes {}", path.display(), ui_dir.display()))?;
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a minimal but complete ui/ tree plus a packaged dist/ whose
    /// manifest matches the inputs, and returns the ui dir.
    fn fixture_ui() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let ui_dir = temp.path().join("ui");
        let write = |relative: &str, content: &str| {
            let path = ui_dir.join(relative);
            fs::create_dir_all(path.parent().expect("fixture paths have parents"))
                .expect("fixture dirs");
            fs::write(&path, content).expect("fixture file");
        };
        write("src/main.ts", "console.log(1);\n");
        for file in STATIC_FILES {
            write(file, "static\n");
        }
        for file in BUILD_INPUTS {
            write(file, "build input\n");
        }
        for served in REQUIRED_SERVED {
            write(&format!("dist/{served}"), "bundled\n");
        }
        let manifest = format!(
            "{{\n  \"version\": {},\n  \"minified\": true,\n  \"inputHash\": \"{}\",\n  \"files\": {:?}\n}}\n",
            MANIFEST_VERSION,
            compute_input_hash(&ui_dir).expect("input hash"),
            REQUIRED_SERVED,
        );
        write("dist/manifest.json", &manifest);
        (temp, ui_dir)
    }

    #[test]
    fn fresh_artifact_passes_verification() {
        let (_temp, ui_dir) = fixture_ui();
        verify(&ui_dir).expect("a freshly packaged artifact verifies");
    }

    #[test]
    fn missing_manifest_fails_with_build_instructions() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let error = verify(temp.path()).expect_err("no artifact must fail");
        assert!(error.contains("dist/manifest.json is absent"), "{error}");
        assert!(error.contains("npm run package"), "{error}");
    }

    #[test]
    fn unparseable_manifest_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        fs::write(ui_dir.join("dist/manifest.json"), "{ not json").expect("corrupt manifest");
        let error = verify(&ui_dir).expect_err("a corrupt manifest must fail");
        assert!(error.contains("not valid JSON"), "{error}");
    }

    #[test]
    fn wrong_manifest_version_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        let text = fs::read_to_string(ui_dir.join("dist/manifest.json")).expect("manifest");
        fs::write(
            ui_dir.join("dist/manifest.json"),
            text.replace(
                &format!("\"version\": {MANIFEST_VERSION},"),
                "\"version\": 999,",
            ),
        )
        .expect("rewrite manifest");
        let error = verify(&ui_dir).expect_err("a schema version mismatch must fail");
        assert!(
            error.contains(&format!("expected {MANIFEST_VERSION}")),
            "{error}"
        );
    }

    #[test]
    fn missing_input_hash_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        let text = fs::read_to_string(ui_dir.join("dist/manifest.json")).expect("manifest");
        fs::write(
            ui_dir.join("dist/manifest.json"),
            text.replace("inputHash", "wrongKey"),
        )
        .expect("rewrite manifest");
        let error = verify(&ui_dir).expect_err("a manifest without an inputHash must fail");
        assert!(error.contains("no inputHash"), "{error}");
    }

    #[test]
    fn missing_files_list_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        let text = fs::read_to_string(ui_dir.join("dist/manifest.json")).expect("manifest");
        fs::write(
            ui_dir.join("dist/manifest.json"),
            text.replace("\"files\"", "\"entries\""),
        )
        .expect("rewrite manifest");
        let error = verify(&ui_dir).expect_err("a manifest without a files list must fail");
        assert!(error.contains("no files list"), "{error}");
    }

    #[test]
    fn stale_input_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        fs::write(ui_dir.join("src/main.ts"), "console.log(2);\n").expect("edit source");
        let error = verify(&ui_dir).expect_err("a source edit must fail");
        assert!(error.contains("sources changed"), "{error}");
    }

    #[test]
    fn unminified_artifact_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        let text = fs::read_to_string(ui_dir.join("dist/manifest.json")).expect("manifest");
        fs::write(
            ui_dir.join("dist/manifest.json"),
            text.replace("true", "false"),
        )
        .expect("rewrite manifest");
        let error = verify(&ui_dir).expect_err("an unminified artifact must fail");
        assert!(error.contains("not minified"), "{error}");
    }

    #[test]
    fn missing_served_file_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        fs::remove_file(ui_dir.join("dist/app.js")).expect("remove served file");
        let error = verify(&ui_dir).expect_err("a missing served file must fail");
        assert!(error.contains("app.js"), "{error}");
    }

    #[test]
    fn empty_served_file_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        fs::write(ui_dir.join("dist/app.js"), "").expect("empty the bundle");
        let error = verify(&ui_dir).expect_err("an empty bundle must fail");
        assert!(error.contains("app.js is empty"), "{error}");
    }

    #[test]
    fn escaping_manifest_entry_fails_verification() {
        let (_temp, ui_dir) = fixture_ui();
        let text = fs::read_to_string(ui_dir.join("dist/manifest.json")).expect("manifest");
        fs::write(
            ui_dir.join("dist/manifest.json"),
            text.replace("\"app.js\",", "\"app.js\", \"../escape.txt\","),
        )
        .expect("rewrite manifest");
        let error = verify(&ui_dir).expect_err("an entry escaping dist must fail");
        assert!(error.contains("../escape.txt"), "{error}");
    }
}
