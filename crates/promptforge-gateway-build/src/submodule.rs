//! Pinned llama.cpp submodule verification, without invoking git.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// Exact commit the submodule must be checked out at (tag `b10082`).
pub const PINNED_COMMIT: &str = "fb0e6b621917488d623437349fb5361e0ac21c70";

/// Upstream repository the submodule is added from.
pub const SOURCE_URL: &str = "https://github.com/ggml-org/llama.cpp.git";

/// Resolves the submodule's git directory, following the `.git` link file
/// git writes for submodules.
///
/// # Errors
/// Returns an error when `.git` is neither a directory nor a gitdir link.
pub fn git_dir(submodule: &Path) -> anyhow::Result<PathBuf> {
    let dotgit = submodule.join(".git");
    if dotgit.is_dir() {
        return Ok(dotgit);
    }
    let text =
        std::fs::read_to_string(&dotgit).with_context(|| format!("read {}", dotgit.display()))?;
    let target = text
        .trim()
        .strip_prefix("gitdir:")
        .with_context(|| format!("{} is not a gitdir link", dotgit.display()))?
        .trim();
    Ok(submodule.join(target))
}

/// Returns the submodule's git HEAD file, for `cargo::rerun-if-changed`.
///
/// # Errors
/// Returns an error when the git directory cannot be resolved.
pub fn head_file(submodule: &Path) -> anyhow::Result<PathBuf> {
    Ok(git_dir(submodule)?.join("HEAD"))
}

/// Reads the commit the submodule is checked out at, following refs
/// (loose first, then packed).
///
/// # Errors
/// Returns an error when HEAD or the ref it names cannot be read.
pub fn head_commit(submodule: &Path) -> anyhow::Result<String> {
    let dir = git_dir(submodule)?;
    let head = std::fs::read_to_string(dir.join("HEAD")).context("read submodule HEAD")?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        let reference = reference.trim();
        let ref_file = dir.join(reference);
        if ref_file.is_file() {
            return Ok(std::fs::read_to_string(&ref_file)?.trim().to_string());
        }
        let packed =
            std::fs::read_to_string(dir.join("packed-refs")).context("read packed-refs")?;
        for line in packed.lines() {
            if let Some((sha, name)) = line.split_once(' ')
                && name.trim() == reference
            {
                return Ok(sha.to_string());
            }
        }
        anyhow::bail!("ref `{reference}` not found in loose or packed refs");
    }
    Ok(head.to_string())
}

/// Verifies the submodule is present, looks like llama.cpp, and is checked
/// out at [`PINNED_COMMIT`].
///
/// # Errors
/// Returns an error on absence, an unrecognized tree, or pin drift.
pub fn verify(submodule: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        submodule.is_dir(),
        "llama.cpp submodule is missing at {}; run \
         `git submodule update --init third_party/llama.cpp`",
        submodule.display()
    );
    anyhow::ensure!(
        submodule.join("CMakeLists.txt").is_file(),
        "{} does not look like llama.cpp (no CMakeLists.txt)",
        submodule.display()
    );
    let commit = head_commit(submodule)?;
    anyhow::ensure!(
        commit == PINNED_COMMIT,
        "llama.cpp submodule drift: expected {PINNED_COMMIT}, found {commit}; run \
         `git -C third_party/llama.cpp checkout {PINNED_COMMIT}`"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lays out a synthetic submodule: `CMakeLists.txt` plus a `.git`
    /// directory whose HEAD is `head_contents`.
    fn synthetic_submodule(head_contents: &str) -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().unwrap();
        let sub = temp.path().join("third_party/llama.cpp");
        std::fs::create_dir_all(sub.join(".git")).unwrap();
        std::fs::write(
            sub.join("CMakeLists.txt"),
            b"cmake_minimum_required(VERSION 3.14)\n",
        )
        .unwrap();
        std::fs::write(sub.join(".git/HEAD"), head_contents).unwrap();
        temp
    }

    #[test]
    fn detached_head_reads_the_commit() {
        let temp = synthetic_submodule(&format!("{PINNED_COMMIT}\n"));
        let sub = temp.path().join("third_party/llama.cpp");
        assert_eq!(head_commit(&sub).unwrap(), PINNED_COMMIT);
        verify(&sub).unwrap();
    }

    #[test]
    fn ref_heads_are_followed_loose_and_packed() {
        let temp = synthetic_submodule("ref: refs/heads/main\n");
        let sub = temp.path().join("third_party/llama.cpp");
        std::fs::create_dir_all(sub.join(".git/refs/heads")).unwrap();
        std::fs::write(
            sub.join(".git/refs/heads/main"),
            format!("{PINNED_COMMIT}\n"),
        )
        .unwrap();
        assert_eq!(head_commit(&sub).unwrap(), PINNED_COMMIT);

        let temp = synthetic_submodule("ref: refs/heads/main\n");
        let sub = temp.path().join("third_party/llama.cpp");
        std::fs::write(
            sub.join(".git/packed-refs"),
            format!("# pack\n{PINNED_COMMIT} refs/heads/main\n"),
        )
        .unwrap();
        assert_eq!(head_commit(&sub).unwrap(), PINNED_COMMIT);
    }

    #[test]
    fn gitdir_link_files_are_followed() {
        let temp = tempfile::TempDir::new().unwrap();
        let sub = temp.path().join("third_party/llama.cpp");
        let real_git = temp.path().join(".git/modules/third_party/llama.cpp");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::create_dir_all(&real_git).unwrap();
        std::fs::write(
            sub.join("CMakeLists.txt"),
            b"cmake_minimum_required(VERSION 3.14)\n",
        )
        .unwrap();
        std::fs::write(
            sub.join(".git"),
            "gitdir: ../../.git/modules/third_party/llama.cpp\n",
        )
        .unwrap();
        std::fs::write(real_git.join("HEAD"), format!("{PINNED_COMMIT}\n")).unwrap();
        assert_eq!(head_commit(&sub).unwrap(), PINNED_COMMIT);
        verify(&sub).unwrap();
    }

    #[test]
    fn absence_is_an_error() {
        let temp = tempfile::TempDir::new().unwrap();
        let err = verify(&temp.path().join("third_party/llama.cpp")).unwrap_err();
        assert!(err.to_string().contains("submodule is missing"));
    }

    #[test]
    fn drift_is_an_error_naming_both_commits() {
        let temp = synthetic_submodule("0000000000000000000000000000000000000000\n");
        let err = verify(&temp.path().join("third_party/llama.cpp")).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("drift"));
        assert!(message.contains(PINNED_COMMIT));
        assert!(message.contains("0000000000000000000000000000000000000000"));
    }
}
