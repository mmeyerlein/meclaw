//! A surface's own files, out of its own cell directory — and nothing else from it.
//!
//! # Why these are not compiled in
//!
//! The two LiveView bundles are the binary's, because they are the client half of
//! the protocol the binary speaks. A surface's **own** presentation — how its lines
//! are drawn, what its drag feels like — is the surface's, and a line that should
//! look different must not cost a release. So these come off disk, out of the
//! directory the surface declared, which travels with the template.
//!
//! # The three defences, and why the third one is the one that matters
//!
//! 1. The file name is already one plain segment: the route parser refuses a
//!    separator, a traversal and a NUL before this module is reached.
//! 2. The directory name is already one plain segment: the declaration parser
//!    refuses the same set at boot.
//! 3. The joined path is **canonicalised** and re-checked against the canonicalised
//!    asset directory. This is not belt-and-braces — a cell directory in this system
//!    is writable by cells, so a **symlink** out of it is the realistic attack, and
//!    no amount of string checking sees one.
//!
//! An unknown extension is a miss rather than `application/octet-stream`: a surface
//! that wants to serve a new file type should say so in a commit.

use meclaw_colony::surface::Located;

/// Content type by extension. A closed table on purpose.
fn content_type(file: &str) -> Option<&'static str> {
    let ext = file.rsplit_once('.')?.1;
    Some(match ext {
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "json" => "application/json; charset=utf-8",
        _ => return None,
    })
}

/// Read one declared asset. `None` is a 404, in every failure mode.
pub fn read_asset(located: &Located, file: &str) -> Option<(&'static str, Vec<u8>)> {
    let ctype = content_type(file)?;
    let dir = located.decl.assets.as_deref()?;

    // The declared directory, canonicalised. A surface that declares a directory
    // which is not there serves nothing rather than falling back to the cell dir.
    let root = located.cell_dir.join(dir).canonicalize().ok()?;
    let path = root.join(file).canonicalize().ok()?;
    if !path.starts_with(&root) {
        // A symlink pointed out of the asset directory. This is the check the
        // string rules above cannot make.
        tracing::warn!(
            surface = %located.cell_path,
            file = %file,
            "asset resolved outside its declared directory — refused"
        );
        return None;
    }
    if !path.is_file() {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    Some((ctype, bytes))
}
