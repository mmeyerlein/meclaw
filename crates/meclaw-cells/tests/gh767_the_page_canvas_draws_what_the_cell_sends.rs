//! What arrives on `image` stands on the canvas, in PAGE pixels (GH #767).
//!
//! The wire is sixteen bytes of head and then a JPEG, and the head is
//! BIG-endian: read the other way round, a 1280-pixel page would be a canvas
//! of 2.1 billion pixels. The numbers in it are the PAGE viewport in CSS
//! pixels and not the size of the picture -- the cell caps the picture
//! (`screencast.max_*`), and a click has to be expressed in the page's own
//! numbers or it lands somewhere else by the capping factor.
//!
//! Skips when `python3` is absent or the templates do not ship (R2b).
mod support;

use std::process::Command;

use support::{COMPOSE, library_ships, repo};

/// The scene hook as the browser gets it, asked of the script itself.
fn scene() -> Option<String> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             sys.stdout.write(m.SCENE_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The part of the hook that belongs to pages: everything the wave added,
/// before the lifecycle object. The listener rules below are statements about
/// THIS part, and measuring them over the whole file would read the scene's own
/// tile handler as a breach.
fn pages_part(js: &str) -> &str {
    let from = js
        .find("// \u{2500}\u{2500} Pages")
        .expect("the hook carries a page section");
    let to = js[from..]
        .find("var hook = {")
        .expect("and it ends before the hook");
    &js[from..from + to]
}

/// The head: big-endian, four numbers, and the canvas takes two of them.
#[test]
fn the_frame_head_is_big_endian_and_the_canvas_is_the_page_viewport() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // `DataView.getUint32` is big-endian unless a second argument says
    // otherwise, and no second argument is what makes this the wire's order.
    assert!(
        part.contains("var head = new DataView(buf, 0, 16);")
            && part.contains("var w = head.getUint32(0), h = head.getUint32(4);"),
        "the head is read big-endian: {part:?}"
    );
    // Read, remembered, deliberately NOT applied: a screencast frame IS the
    // cutout already.
    assert!(
        part.contains("link.scrollX = head.getUint32(8);")
            && part.contains("link.scrollY = head.getUint32(12);"),
        "the scroll is read and kept"
    );
    assert!(
        part.contains("canvas.width = w;") && part.contains("canvas.height = h;"),
        "the bitmap of the canvas is the head's numbers, not the picture's"
    );
    // Writing `width` CLEARS the canvas, so it is written only where the shape
    // really changed -- otherwise the picture blinks between two frames.
    assert!(
        part.contains("if (canvas.width !== w || canvas.height !== h) {"),
        "the size is written only when it moved"
    );
    // And the proportions follow the head, not the first frame for ever.
    assert!(
        part.contains("pageShape(canvas, w, h);"),
        "a frame of another shape changes the frame's shape"
    );
}

/// Decoded off the main thread, closed at once, and a late frame is dropped.
#[test]
fn a_frame_is_decoded_off_thread_and_never_queued() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // Neither an `<img>` nor an object URL that would have to live until
    // `onload`: twenty frames a second would be twenty URLs a second.
    assert!(
        part.contains("root.createImageBitmap(new Blob([new Uint8Array(buf, 16)]))"),
        "the JPEG is decoded as a bitmap: {part:?}"
    );
    assert!(
        !part.contains("createObjectURL"),
        "no object URL outlives a frame"
    );
    assert!(
        part.contains("if (bmp.close) bmp.close();"),
        "the bitmap never outlives its frame"
    );
    // A queue buys latency with memory and ends up showing an old picture; the
    // cell drops a viewer that cannot keep up itself (`client_too_slow`).
    assert!(
        part.contains("if (link.busy) { if (!again) link.dropped++; return; }"),
        "a frame that arrives while the last one decodes is dropped"
    );
}

/// The event is `image`. A client that decoded a JPEG as PCM makes noise.
#[test]
fn the_binary_event_is_image_and_not_audio() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    assert!(
        part.contains("chan.on(\"image\", function (buf) { pageDraw(link, buf); });"),
        "the pictures come in on `image` (OR-G23): {part:?}"
    );
    assert!(
        !part.contains("chan.on(\"audio\""),
        "and never on the voice cell's name"
    );
}

/// `prefers-reduced-motion` does not reach into a page.
#[test]
fn frames_are_content_and_not_motion_of_the_screen() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // Somebody who wants less motion does not want the page they are reading
    // to freeze: that is a statement about the SCREEN's own movement.
    assert!(
        !part.contains("reduced()"),
        "the drawing does not ask about reduced motion: {part:?}"
    );
}

/// A canvas that lost its size to a patch gets its picture back (B-G23).
///
/// Measured in a colony on 2026-09-20: eight seconds after the one frame of a
/// standing page, `link.canvas` was still the canvas in the document, `link.w`
/// was still 1280 — and `canvas.width` was 300 and `getAttribute("width")` was
/// NULL. Writing `canvas.width` writes the content attribute, the server
/// renders the `<canvas>` without one, and the next LiveView patch reconciles
/// it away, blanking the bitmap with it. An animating page hides this because
/// its next frame sets the size again; a page that stands still — a PDF, an
/// article, a form — has no next frame and stays empty for good.
#[test]
fn the_one_frame_of_a_standing_page_survives_a_patch() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // The frame is KEPT, and only a real one is kept: a repaint must not
    // overwrite the picture it is repainting from.
    assert!(
        part.contains("if (!again) { link.last = buf; link.lastW = w; link.lastH = h; }"),
        "the last real frame is held for the repaint: {part:?}"
    );
    // A repaint is not a frame: the proof counts what the cell sent.
    assert!(
        part.contains("      if (!again) {\n        link.frames++;\n"),
        "a repaint neither counts nor is counted"
    );
    // And it never asks the cell for anything -- no keyframe, no roundtrip.
    assert!(
        part.contains("function pageRepaint(link) {")
            && part.contains("pageDraw(link, link.last, true);"),
        "the repair is the frame already in hand"
    );
    // The two moments a canvas can lose the picture: the same node patched,
    // and the node replaced.
    assert!(
        part.contains("if (link.last\n")
            && part.contains("&& (canvas.width !== link.lastW || canvas.height !== link.lastH)) {"),
        "the same node whose size a patch took away is repainted"
    );
    // Twice: the node that survived the patch, and the node it replaced.
    assert_eq!(
        part.matches("pageRepaint(link);").count(),
        2,
        "both ways a canvas loses its picture end in a repaint"
    );
    // The held frame goes when the link goes.
    assert!(
        part.contains("link.last = null;"),
        "a link that is given back holds no picture"
    );
}
