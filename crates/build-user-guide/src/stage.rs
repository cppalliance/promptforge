//! Stage mode: builds one mdBook source tree per book under an absolute
//! output folder, `<out>/<book>/` with `book.toml`, `back-link.js`, and
//! `src/` holding the book's set folders, each set's `index.md`, and the
//! book's `SUMMARY.md`. The checked-in guide tree is only read.

use std::fs;
use std::path::Path;

use crate::{
    AssembleError, BOOKS, check_links, check_removed_workshop_stt_claims, read_chapters,
    render_index, render_summary, write_file,
};

/// Stages every book in `BOOKS` from `guide` into `out`, which must be
/// absolute. The `[workshop.stt]` check runs before anything is written;
/// the SUMMARY link check runs on each staged book. Files already in `out`
/// are overwritten, never removed.
pub(crate) fn stage(guide: &Path, out: &Path) -> Result<(), AssembleError> {
    if !out.is_absolute() {
        return Err(AssembleError(format!(
            "stage output path must be absolute, got {}",
            out.display()
        )));
    }
    let src = guide.join("src");
    check_removed_workshop_stt_claims(&src)?;

    for (book, sets) in BOOKS {
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

        let mut parts = Vec::new();
        for (set, part_title) in *sets {
            let set_src = src.join(set);
            let set_out = book_src.join(set);
            copy_dir(&set_src, &set_out)?;
            let chapters = read_chapters(&set_src)?;
            write_file(
                &set_out.join("index.md"),
                &render_index(part_title, &chapters),
            )?;
            parts.push((*set, *part_title, chapters));
        }
        let summary = render_summary(&parts);
        write_file(&book_src.join("SUMMARY.md"), &summary)?;
        check_links(&summary, &book_src).map_err(|e| AssembleError(format!("{book} book: {e}")))?;
    }
    Ok(())
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
