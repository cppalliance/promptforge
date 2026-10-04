//! Stage mode: builds one mdBook source tree per book under an absolute
//! output folder, `<out>/<book>/` with `book.toml`, `back-link.js`, and
//! `src/` holding the book's chapters, its rendered `index.md`, and its
//! `SUMMARY.md` side by side, plus the book's single-file export
//! `<out>/promptforge-<book>-guide.md`. Chapters sit at the top of `src/`,
//! so the built book serves them at `<book>/<chapter>.html` and the
//! overview's relative links still resolve once mdBook copies it to the
//! book root. The guide and docs trees are only read.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{
    AssembleError, BOOKS, DOCS_VAR, check_links, check_removed_workshop_stt_claims, read_chapters,
    render_export, render_index, render_summary, write_file,
};

/// Stages every book in `BOOKS` into `out`, which must be absolute, from
/// the chapters under `src/` in `docs`, the `promptforge-docs` root, and
/// the book configs and chrome in `guide`. The `[workshop.stt]` check runs
/// before anything is written; the SUMMARY link check runs on each staged
/// book. Files already in `out` are overwritten, never removed.
pub(crate) fn stage(guide: &Path, docs: Option<&Path>, out: &Path) -> Result<(), AssembleError> {
    if !out.is_absolute() {
        return Err(AssembleError(format!(
            "stage output path must be absolute, got {}",
            out.display()
        )));
    }
    let src = docs_src(docs)?;
    check_removed_workshop_stt_claims(&src)?;

    for (book, part_title) in BOOKS {
        let book_out = out.join(book);
        let book_src = book_out.join("src");
        create_dir(&book_src)?;
        copy_file(
            &guide.join("books").join(book).join("book.toml"),
            &book_out.join("book.toml"),
        )?;
        copy_file(
            &guide.join("chrome").join("back-link.js"),
            &book_out.join("back-link.js"),
        )?;

        let set_src = src.join(book);
        copy_dir(&set_src, &book_src)?;
        let chapters = read_chapters(&set_src)?;
        write_file(
            &book_src.join("index.md"),
            &render_index(part_title, &chapters),
        )?;
        let summary = render_summary(part_title, &chapters);
        write_file(&book_src.join("SUMMARY.md"), &summary)?;
        check_links(&summary, &book_src).map_err(|e| AssembleError(format!("{book} book: {e}")))?;
        write_file(
            &out.join(format!("promptforge-{book}-guide.md")),
            &render_export(part_title, &chapters, &set_src)?,
        )?;
    }
    Ok(())
}

/// The chapter folder `src/` under `docs`, or an error naming
/// `PROMPTFORGE_DOCS` when the root is unset or names no directory.
fn docs_src(docs: Option<&Path>) -> Result<PathBuf, AssembleError> {
    let Some(docs) = docs else {
        return Err(AssembleError(format!(
            "{DOCS_VAR} is not set; set it to the root of a promptforge-docs checkout"
        )));
    };
    if !docs.is_dir() {
        return Err(AssembleError(format!(
            "{DOCS_VAR} names {}, which is not a directory; set it to the root of a \
             promptforge-docs checkout",
            docs.display()
        )));
    }
    Ok(docs.join("src"))
}

/// Copies the directory `from` into `to`, recursing into subdirectories.
fn copy_dir(from: &Path, to: &Path) -> Result<(), AssembleError> {
    create_dir(to)?;
    let read_error =
        |e: std::io::Error| AssembleError(format!("cannot read {}: {e}", from.display()));
    for entry in fs::read_dir(from).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        let target = to.join(entry.file_name());
        if entry.file_type().map_err(read_error)?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies one file, overwriting `to`.
fn copy_file(from: &Path, to: &Path) -> Result<(), AssembleError> {
    fs::copy(from, to).map(drop).map_err(|e| {
        AssembleError(format!(
            "cannot copy {} to {}: {e}",
            from.display(),
            to.display()
        ))
    })
}

/// Creates a directory and its missing parents.
fn create_dir(dir: &Path) -> Result<(), AssembleError> {
    fs::create_dir_all(dir)
        .map_err(|e| AssembleError(format!("cannot create {}: {e}", dir.display())))
}

#[cfg(test)]
#[path = "stage-tests.rs"]
mod tests;
