//! Cursor pagination of the four `*/list` results (`mcp.page-size`).
//!
//! A cursor is `v1.<offset>.<fingerprint>`: an offset into the caller's
//! visible list plus a hash of that list's keys and the caller's subject.
//! Any change to what the caller sees (a group toggled, a private member
//! added, other scopes) changes the fingerprint, and the stale cursor is
//! refused with `-32602` so the client restarts from the first page — the
//! `list_changed` notification already tells it to.
//!
//! The fingerprint is content-based, not a session generation: it needs no
//! session (works under `mcp.stateless`) and only invalidates the family
//! that changed. Cursors are not secrets — a forged offset can only page
//! the caller's own visible list.

use std::hash::{DefaultHasher, Hash, Hasher};

use rmcp::model::{ErrorCode, ErrorData, Prompt, Resource, ResourceTemplate, Tool};

/// The key a list entry is identified by in the fingerprint.
pub(crate) trait ListKey {
    fn list_key(&self) -> &str;
}

impl ListKey for Tool {
    fn list_key(&self) -> &str {
        &self.name
    }
}

impl ListKey for Resource {
    fn list_key(&self) -> &str {
        &self.uri
    }
}

impl ListKey for ResourceTemplate {
    fn list_key(&self) -> &str {
        &self.uri_template
    }
}

impl ListKey for Prompt {
    fn list_key(&self) -> &str {
        &self.name
    }
}

const VERSION: &str = "v1";

/// One page of `list` starting at `cursor`, plus the cursor of the next page.
///
/// `page_size = None` serves the whole list and issues no cursor; a cursor
/// sent anyway is refused (this server never issued it).
pub(crate) fn paginate<W: ListKey>(
    mut list: Vec<W>,
    cursor: Option<&str>,
    page_size: Option<usize>,
    subject: Option<&str>,
) -> Result<(Vec<W>, Option<String>), ErrorData> {
    let Some(size) = page_size else {
        return match cursor {
            None => Ok((list, None)),
            Some(_) => Err(invalid_cursor()),
        };
    };
    let fingerprint = fingerprint(&list, subject);
    let offset = match cursor {
        None => 0,
        Some(cursor) => decode(cursor, fingerprint, list.len())?,
    };
    let end = offset.saturating_add(size).min(list.len());
    let next = (end < list.len()).then(|| encode(end, fingerprint));
    list.truncate(end);
    list.drain(..offset);
    Ok((list, next))
}

fn fingerprint<W: ListKey>(list: &[W], subject: Option<&str>) -> u64 {
    // `DefaultHasher::new()` uses fixed keys: stable across requests and
    // workers of one build, which is all a cursor needs.
    let mut hasher = DefaultHasher::new();
    subject.hash(&mut hasher);
    list.len().hash(&mut hasher);
    for entry in list {
        entry.list_key().hash(&mut hasher);
    }
    hasher.finish()
}

fn encode(offset: usize, fingerprint: u64) -> String {
    format!("{VERSION}.{offset}.{fingerprint:016x}")
}

fn decode(cursor: &str, fingerprint: u64, len: usize) -> Result<usize, ErrorData> {
    let mut parts = cursor.split('.');
    let (Some(VERSION), Some(offset), Some(fp), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid_cursor());
    };
    let offset: usize = offset.parse().map_err(|_| invalid_cursor())?;
    let fp = u64::from_str_radix(fp, 16).map_err(|_| invalid_cursor())?;
    // An issued cursor always points inside the list (`end < len`).
    if fp != fingerprint || offset == 0 || offset >= len {
        return Err(invalid_cursor());
    }
    Ok(offset)
}

fn invalid_cursor() -> ErrorData {
    ErrorData::new(
        ErrorCode::INVALID_PARAMS,
        "invalid cursor: the list changed or the cursor was not issued by this server — \
         restart from the first page",
        None,
    )
}
