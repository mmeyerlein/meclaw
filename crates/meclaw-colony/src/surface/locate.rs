//! URL path → a surface on disk, or nothing.
//!
//! Three gates, in this order, because each one is cheaper than the next and each
//! one is a different question:
//!
//! 1. **Is the path well formed?** Segment by segment: no `.`, no `..`, no empty
//!    segment, nothing starting with `@`, nothing named `live`. The result is then
//!    checked against the root cell directory a second time — belt and braces, because a
//!    containment check that trusts its own string handling is one refactor away
//!    from being no check.
//! 2. **Is there a cell there?** Read `config.json`.
//! 3. **Does it declare a surface?** `cell.surface`, via the one parser.
//!
//! Gates 1 and 2 fail as [`LocateError::NotFound`], gate 3 as
//! [`LocateError::NoSurface`], and the HTTP layer answers both with 404. A caller
//! must not be able to tell "no such cell" from "that cell is not yours to see" —
//! the difference is exactly the information a probe wants.

use super::decl::{SurfaceDecl, parse_decl};
use std::path::{Path, PathBuf};

/// A resolved surface, with every path its callers need already derived.
#[derive(Debug, Clone)]
pub struct Located {
    /// What the cell said about being served.
    pub decl: SurfaceDecl,
    /// `<root>/<root-cell>/<path>` — the cell's own directory. The asset route joins the
    /// declared directory onto this and nothing else.
    pub cell_dir: PathBuf,
    /// The colony path, absolute and with a leading slash — the message target.
    pub cell_path: String,
}

/// Why a path is not a surface.
#[derive(Debug, Clone)]
pub enum LocateError {
    /// No directory, no `config.json`, an unreadable one, or a path that is not
    /// addressable at all.
    NotFound,
    /// A cell that never said it may be served.
    NoSurface,
    /// A declaration that exists and is wrong. The only variant that is NOT
    /// flattened into a 404: it is the operator's typo, and hiding it costs them
    /// an afternoon of looking in the wrong place.
    Malformed(String),
}

/// The one whole path segment the phoenix client owns.
///
/// It appends exactly `"/websocket"` to the URL it is handed
/// (`assets/js/phoenix/socket.js:191`), so `<cell>/live/websocket` is the
/// transport and a cell named `live` would make that ambiguous.
const RESERVED_SEGMENT: &str = "live";

/// Resolve a URL's cell path against a colony root.
///
/// `rest` is the path as it appears after `/surface/`, with no leading slash and
/// no verb suffix — the caller has already split those off.
pub fn locate(root: &Path, rest: &str) -> Result<Located, LocateError> {
    let segments = well_formed(rest)?;

    // The root cell directory, from the one resolver that knows it (GH #324).
    // Its name is the operator's choice — boot accepts any single top-level
    // directory with a `config.json` — so a literal `"main"` here served 404 for
    // every surface of every colony rooted under another name.
    let root_cell = crate::path_truth::find_root_cell_dir(root);
    let mut dir = root_cell.clone();
    for s in &segments {
        dir.push(s);
    }
    // The second opinion. `well_formed` already refused `..`, so this can only
    // fail if the segment handling above is wrong — which is precisely the
    // failure a containment check is for.
    if !dir.starts_with(&root_cell) {
        return Err(LocateError::NotFound);
    }

    let raw =
        std::fs::read_to_string(dir.join("config.json")).map_err(|_| LocateError::NotFound)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| LocateError::NotFound)?;
    let cell_block = parsed
        .get("cell")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    let decl = parse_decl(&cell_block)
        .map_err(LocateError::Malformed)?
        .ok_or(LocateError::NoSurface)?;

    Ok(Located {
        decl,
        cell_path: format!("/{}", segments.join("/")),
        cell_dir: dir,
    })
}

/// Split a URL path into segments, refusing everything that is not a plain name.
///
/// An empty path is refused too: the root is not a surface.
fn well_formed(rest: &str) -> Result<Vec<&str>, LocateError> {
    if rest.is_empty() || rest.starts_with('/') || rest.ends_with('/') {
        return Err(LocateError::NotFound);
    }
    let segments: Vec<&str> = rest.split('/').collect();
    for s in &segments {
        if s.is_empty()
            || *s == "."
            || *s == ".."
            || *s == RESERVED_SEGMENT
            || s.starts_with('@')
            || s.contains('\\')
            || s.contains('\0')
        {
            return Err(LocateError::NotFound);
        }
    }
    Ok(segments)
}
