//! Paginated listing and capability-retrieval rendering.
//!
//! Both `tools/list` and `list_prompts` serve a large catalog one bounded page
//! at a time rather than materializing it whole, so one call's response size
//! does not grow with the catalog and concurrent callers cannot turn its size
//! into a memory spike. `need_prompt` renders its shortlist in the same shape
//! as the listing, so a caller reads one thing whichever way it found a prompt.

use rmcp::model::{CallToolResult, ContentBlock, ErrorData};
use serde_json::{Value, json};

use crate::catalog::Catalog;
#[cfg(feature = "picker")]
use crate::retrieval::Shortlist;

#[cfg(feature = "picker")]
use super::reply::text_error;

/// The most items one page carries, for `tools/list` and for `list_prompts`
/// alike.
///
/// A large catalog is served one bounded page at a time rather than
/// materialized whole, so one call's response size does not grow with the
/// catalog and concurrent callers cannot turn its size into a memory spike.
pub(super) const PAGE_LIMIT: usize = 100;

/// What a caller of `need_prompt` is told when the retrieval index is not
/// loaded: that this one tool cannot answer, and where an answer still is. The
/// tool was advertised, so the caller did nothing wrong and a protocol fault
/// would be blaming it for the server's own state.
///
/// Only a `picker` build publishes `need_prompt`, so only it needs the message.
#[cfg(feature = "picker")]
const RETRIEVAL_UNAVAILABLE: &str = "need_prompt cannot answer: this server's retrieval index is not loaded. Call list_prompts and choose a prompt from the catalog instead.";

/// The offset a pagination cursor names, or zero when there is none.
///
/// # Errors
/// Returns `-32602` when a cursor is present but is not one this server issues -
/// a non-negative integer offset - since only a client that fabricated or
/// corrupted it could get that wrong.
pub(super) fn page_start(cursor: Option<&str>) -> Result<usize, ErrorData> {
    match cursor {
        None => Ok(0),
        Some(cursor) => cursor.parse::<usize>().map_err(|_| {
            ErrorData::invalid_params(
                format!("cursor {cursor:?} is not a valid pagination cursor"),
                None,
            )
        }),
    }
}

/// One bounded page of the listing every enabled prompt appears in, healthy or
/// broken, from the offset `cursor` names.
///
/// A large catalog is paged rather than materialized whole: one call renders at
/// most [`PAGE_LIMIT`] entries and, when more remain, carries a `next_cursor` to
/// read the page after this one. The text block is compact rather than a second
/// pretty copy of the same page, so the response does not carry the listing
/// twice over.
///
/// # Errors
/// Returns `-32602` when `cursor` is present but not one this server issued, and
/// `-32603` if the page cannot be serialized.
pub(super) fn list_prompts_result(
    catalog: &Catalog,
    cursor: &str,
) -> Result<CallToolResult, ErrorData> {
    let entries = catalog.entries();
    let start = page_start((!cursor.is_empty()).then_some(cursor))?;
    let end = start.saturating_add(PAGE_LIMIT).min(entries.len());
    let prompts: Vec<Value> = entries
        .get(start..end)
        .unwrap_or(&[])
        .iter()
        .map(|entry| {
            let mut obj = json!({
                "name": entry.name(),
                "description": entry.description(),
                "problem": entry.problem(),
            });
            // `obj` is a `json!` object literal, so it is always an object.
            if let Some(prompt) = entry.prompt()
                && let Some(object) = obj.as_object_mut()
            {
                let fm = prompt.frontmatter();
                if let Some(input) = fm.input() {
                    object.insert(
                        "input".to_owned(),
                        json!({ "path": input.path(), "description": input.description() }),
                    );
                }
                if let Some(output) = fm.output() {
                    object.insert(
                        "output".to_owned(),
                        json!({ "path": output.path(), "description": output.description() }),
                    );
                }
            }
            obj
        })
        .collect();
    let mut structured = json!({ "prompts": prompts });
    if end < entries.len()
        && let Some(object) = structured.as_object_mut()
    {
        object.insert("next_cursor".to_owned(), Value::String(end.to_string()));
    }
    let text = serde_json::to_string(&structured)
        .map_err(|e| ErrorData::internal_error(format!("render the prompt listing: {e}"), None))?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    Ok(result)
}

/// The candidates a capability retrieved, shaped like the listing so a caller
/// reads one thing whichever way it found a prompt.
///
/// An empty shortlist is a success, not an error: "no prompt is close to this"
/// is an answer, and the catalog is one `list_prompts` call away.
///
/// # Errors
/// Returns `-32603` when the engine could not embed the capability, which is
/// nothing the caller can correct, and when the candidates cannot be serialized.
///
/// Only a `picker` build publishes `need_prompt`, so only it needs this.
#[cfg(feature = "picker")]
pub(crate) fn need_prompt_result(shortlist: &Shortlist) -> Result<CallToolResult, ErrorData> {
    let candidates = match shortlist {
        Shortlist::Candidates(candidates) => candidates,
        Shortlist::Unavailable => return Ok(text_error(RETRIEVAL_UNAVAILABLE.to_owned())),
        Shortlist::Failed(detail) => {
            return Err(ErrorData::internal_error(
                format!("rank prompts for the capability: {detail}"),
                None,
            ));
        }
    };
    let structured = json!({ "prompts": candidates });
    let text = serde_json::to_string_pretty(&structured)
        .map_err(|e| ErrorData::internal_error(format!("render the candidates: {e}"), None))?;
    let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
    result.structured_content = Some(structured);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;

    use super::page_start;

    #[test]
    fn a_cursor_is_an_offset_absent_is_the_first_page_and_garbage_is_a_client_bug() {
        assert_eq!(page_start(None).expect("no cursor is the first page"), 0);
        assert_eq!(
            page_start(Some("100")).expect("a valid cursor is its offset"),
            100
        );
        let error = page_start(Some("not-a-number"))
            .expect_err("a cursor this server never issued is the client's bug");
        assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    }
}
