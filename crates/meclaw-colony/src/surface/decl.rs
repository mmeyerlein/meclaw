//! `cell.surface` — the one parser.
//!
//! Two callers: the central `cell`-block parse in [`crate::config`], so a broken
//! declaration is a boot error next to every other config error, and the HTTP
//! route in `meclaw-api`, so the route and the boot cannot disagree about what a
//! cell offers.
//!
//! It sits in the `cell` block rather than in `params` because the spec says what
//! that block is for: the `cell` key list is closed and describes how the
//! **colony** runs the cell (`docs/config.md`). Serving a cell over HTTP is the
//! colony operating it. `params` is parsed per cell type against a closed key
//! list, so putting it there would mean adding it to every type that ever wants
//! a surface — which is the definition of a special case.

use serde_json::Value;

/// A cell's statement that it may be served over HTTP.
///
/// Deliberately three fields, all presentational, none of them about content.
/// The cell at the URL is the cell that renders; there is nothing else to name.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceDecl {
    /// The page's `<title>`. Empty is untidy, not broken.
    #[serde(default)]
    pub title: String,
    /// One directory under this cell's own directory whose files are served at
    /// `/surface/<cell>/@asset/<file>`. One plain segment: it is joined onto a
    /// path, so every spelling that could leave the cell directory is refused
    /// here rather than at the join. `None` means the surface has no files of
    /// its own and uses only what the binary ships.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<String>,
    /// Shown in the dead render while the socket connects, so a slow first paint
    /// says what it is waiting for instead of showing nothing.
    #[serde(default)]
    pub boot_hint: String,
}

/// Parse `cell.surface` out of a cell block.
///
/// `Ok(None)` is the ordinary answer: no key, no surface, and every cell written
/// before this feature keeps its exact meaning. `Err` is a declaration that
/// exists and is wrong, which must be loud at boot rather than silent until
/// somebody opens a URL.
pub fn parse_decl(cell_block: &Value) -> Result<Option<SurfaceDecl>, String> {
    let Some(raw) = cell_block.get("surface") else {
        return Ok(None);
    };
    if !raw.is_object() {
        return Err("cell.surface must be an object".into());
    }
    let decl: SurfaceDecl =
        serde_json::from_value(raw.clone()).map_err(|e| format!("cell.surface: {e}"))?;
    if let Some(a) = decl.assets.as_deref() {
        plain_segment(a)?;
    }
    Ok(Some(decl))
}

/// One path segment, plainly spelled.
///
/// `@` opens a verb in the HTTP path; a backslash is a separator on Windows and
/// would make containment platform-dependent; `.`, `..` and `/` are the
/// traversal spellings; a NUL byte truncates a path in the layer below. All
/// refused where the value is written down, because a containment check that runs
/// only at the join is one refactor away from not running.
fn plain_segment(s: &str) -> Result<(), String> {
    if s.is_empty()
        || s.contains('/')
        || s.contains('\\')
        || s.contains('\0')
        || s.starts_with('@')
        || s == "."
        || s == ".."
    {
        return Err(format!(
            "cell.surface.assets {s:?} must be one plain directory name \
             (no '/', no '\\', no leading '@', not '.' or '..')"
        ));
    }
    Ok(())
}
