//! W8 Task 14 (GH #383): canvy's browser half is proven by a runnable test.
//!
//! `templates/canvy/layout/canvy.test.js` is a plain-`node` suite over the edge
//! geometry and the hook. It exists so that a client-side regression is a red
//! Rust test rather than something a person notices in a browser three weeks
//! later.
//!
//! **This is the 0.12.1 lesson, kept.** In that release the canvas was unusable
//! in the browser — no `phx-hook` in the markup, so the client never ran; and
//! when it did run, `rounded(route(...))` threw, so not one edge had a path.
//! Every server-side test was green throughout, because they all asserted about
//! markup and none about the seam. The rule that came out of it: **a client path
//! is never proven over the websocket alone.** So the suite runs here, and the
//! only thing this file adds to it is the guarantee that `cargo test` executes
//! it at all.
//!
//! Skips when `node` is absent, like every other interpreter guard in this tree
//! — a missing interpreter is not a failing canvas.

fn canvy_root() -> Option<std::path::PathBuf> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/canvy");
    // R2b: where the template does not ship, this skips rather than failing on a
    // dead reference.
    for rel in [
        "layout/canvy.js",
        "layout/canvy.test.js",
        "layout/canvy.css",
    ] {
        if !root.join(rel).exists() {
            return None;
        }
    }
    Some(root)
}

#[test]
fn the_clients_own_tests_pass() {
    let Some(root) = canvy_root() else { return };
    let script = root.join("layout/canvy.test.js");
    let out = match std::process::Command::new("node").arg(&script).output() {
        Ok(o) => o,
        Err(_) => return, // no node on this host
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("all green"),
        "the canvas client's tests must pass:\n{stdout}\n{stderr}"
    );
    // The suite must actually have run something — an empty file "passes" too.
    assert!(
        stdout.matches("  ok ").count() >= 20,
        "too few client assertions ran; did the suite lose its cases?\n{stdout}"
    );
}

/// The suite reads the file the layout cell ships, not a copy of it.
///
/// A test that `require`d some other module would keep passing while the client
/// in the page drifted away from it — which is the same shape of hole the hook
/// lived in before it had any test at all.
#[test]
fn the_suite_reads_the_shipped_client() {
    let Some(root) = canvy_root() else { return };
    let suite = std::fs::read_to_string(root.join("layout/canvy.test.js")).unwrap();
    assert!(
        suite.contains("require(\"./canvy.js\")"),
        "the suite must load the shipped client file itself"
    );

    let js = std::fs::read_to_string(root.join("layout/canvy.js")).unwrap();
    // The two names the page and the client agree on.
    assert!(
        js.contains("SurfaceHooks"),
        "the hook script must fill the slot the display's shell offers"
    );
    assert!(
        js.contains("object:set"),
        "and a drag must be an object:set — the display's local lane"
    );
    // Every server-render event of the 1.x client is gone. Named here rather
    // than merely absent from the source, so re-introducing one is a red test
    // and not a quiet regression to a mechanism 2.0.0 removed on purpose.
    for retired in ["node:moved", "hive:moved", "camera:moved", "canvas:sweep"] {
        assert!(
            !js.contains(retired),
            "{retired:?} is a 1.x server-render event and must not come back"
        );
    }
}
