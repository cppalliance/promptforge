//! The landing page: a title and one table, with a row per rustdoc site and
//! per staged book, so a new site or book adds its row with no edit here.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use super::RUSTDOC_SITES;

const TITLE: &str = "PromptForge documentation";

/// The landing page: one row per entry in `RUSTDOC_SITES`, in order, then
/// one per book in `books`, each read from the `title` and `description`
/// under `[book]` in its `book.toml` staged under `staged`.
pub(super) fn page(staged: &Path, books: &[OsString]) -> Result<String, String> {
    let mut rows = String::new();
    for (dir, krate, covers) in RUSTDOC_SITES {
        push_row(&mut rows, dir, krate, "API reference", covers);
    }
    for book in books {
        let book = book.to_string_lossy();
        let (title, description) = book_text(&staged.join(&*book).join("book.toml"))?;
        push_row(&mut rows, &book, &title, "Guide", &description);
    }
    Ok(format!(
        "<!DOCTYPE html>\n\
         <html lang=\"en\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{TITLE}</title>\n\
         <link rel=\"stylesheet\" href=\"style.css\">\n\
         </head>\n\
         <body>\n\
         <h1>{TITLE}</h1>\n\
         <table>\n\
         <tr><th>Documentation</th><th>Kind</th><th>Covers</th></tr>\n\
         {rows}\
         </table>\n\
         </body>\n\
         </html>\n"
    ))
}

/// Appends the row linking to `<dir>/index.html` under the site root.
fn push_row(rows: &mut String, dir: &str, name: &str, kind: &str, covers: &str) {
    let _ = writeln!(
        rows,
        "<tr><td><a href=\"{}/index.html\">{}</a></td><td>{kind}</td><td>{}</td></tr>",
        escape(dir),
        escape(name),
        escape(covers)
    );
}

/// The non-empty `title` and `description` under `[book]` in the
/// `book.toml` at `path`.
fn book_text(path: &Path) -> Result<(String, String), String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("site: cannot read {}: {error}", path.display()))?;
    let config: toml::Value = toml::from_str(&text)
        .map_err(|error| format!("site: cannot parse {}: {error}", path.display()))?;
    let field = |key: &str| {
        config
            .get("book")
            .and_then(|book| book.get(key))
            .and_then(toml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                format!(
                    "site: {} needs a non-empty `{key}` under `[book]` for its landing row",
                    path.display()
                )
            })
    };
    Ok((field("title")?, field("description")?))
}

/// `text` with the HTML special characters escaped.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
