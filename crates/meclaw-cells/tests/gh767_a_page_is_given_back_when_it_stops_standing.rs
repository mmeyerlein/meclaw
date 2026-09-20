//! Four ways out of a page, and one door (GH #767).
//!
//! Level 0, the object gone, the hook destroyed, and the cell closing the
//! link. The first three go through the SAME function, and that is the point:
//! there is exactly one place where a topic is given back, and it hands back
//! channel, field and listeners in one order. Letting the sender go is what
//! the cell reads as "the viewer left"; the last one stops the screencast, and
//! there is no second message for it.
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

/// One door, and what it gives back in what order.
#[test]
fn there_is_exactly_one_way_out_of_a_topic() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    assert_eq!(
        part.matches(".leave()").count(),
        1,
        "a topic is given back in one place: {part:?}"
    );
    let door = {
        let from = part.find("function pageLeave(").expect("the door exists");
        let to = part[from..].find("\n  function ").expect("and it ends");
        &part[from..from + to]
    };
    for (n, needle) in [
        "delete st.pages[key];",
        "st.pageLinks--;",
        "if (link.unwire) link.unwire();",
        "parentNode.removeChild(link.field);",
        "link.chan.leave();",
    ]
    .iter()
    .enumerate()
    {
        assert!(door.contains(needle), "step {n} of the door: {door}");
    }
    // The socket may already be down; a throw here would leave the rest of the
    // sweep undone.
    assert!(
        door.contains("try { if (link.chan) link.chan.leave(); } catch (e)"),
        "the last step cannot take the sweep with it: {door}"
    );
    // The field hangs on `document.body`, outside everything LiveView clears
    // away: one per reconnect would be a leak with a clock on it.
    assert!(
        door.contains("link.field.parentNode"),
        "the hidden field is given back too: {door}"
    );
    // And the hook's own end runs the same loop over everything it holds.
    assert!(
        js.contains("pageAll(this.__scene.st);"),
        "a destroyed hook gives every topic back"
    );
}

/// A rejoin is a NEW channel, and one per closed channel.
#[test]
fn a_closed_link_is_rejoined_once_and_only_once() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // A `phx_close` on a `page:` topic is the CELL letting go -- restarted or
    // replaced -- while the window goes on standing. A screen that gave up
    // here would show a still picture that looks like a living page.
    assert!(
        part.contains("var PAGE_REJOIN_MS = 1000;"),
        "one second, once: {part:?}"
    );
    // The channel is built inside the door that opens a topic, and the rejoin
    // goes through the SAME door: `chan.join()` on a channel that has already
    // joined throws in the real client
    // (`crates/meclaw-surface/src/client/phoenix.min.js:1`, `joinedOnce`), and
    // by then the channel has taken itself off the socket in its own
    // `onClose`. The microphone builds a channel per call for the same reason.
    assert!(
        part.contains("function pageOpen(link, socket, el) {")
            && part.contains("var chan = socket.channel(\"page:\" + link.key, {"),
        "the topic is opened in one place that BUILDS the channel: {part:?}"
    );
    assert_eq!(
        part.matches("socket.channel(").count(),
        1,
        "and nowhere else: {part:?}"
    );
    assert!(
        part.matches("chan.join()").count() == 1
            && !part.contains("answer(chan)")
            && !part.contains("function pageAnswer("),
        "a channel is joined exactly once in its life: {part:?}"
    );
    assert!(
        part.contains("pageOpen(link, socket, el);") && part.contains("}, PAGE_REJOIN_MS);"),
        "the rejoin opens a new channel a second later: {part:?}"
    );
    // Only the channel that is CURRENT may ask for another one, or a close
    // arriving late from a channel already replaced would open a second
    // stream on the same topic.
    assert!(
        part.contains("if (link.gone || link.chan !== chan || !link.rejoin) return;")
            && part.contains("link.rejoin = 0;"),
        "a topic that turns the second join down IS a refusal: {part:?}"
    );
    // And the budget is given back by a join that WORKED, not by the link:
    // `rejoined = 1` once per LINK left a wall screen without a rejoin for
    // every restart after the first one.
    assert!(
        part.contains("link.rejoin = 1;") && part.contains("pageLink(link, \"up\""),
        "a working join earns the next rejoin: {part:?}"
    );
}

/// The same page on a new element keeps its channel.
#[test]
fn a_replaced_canvas_keeps_the_topic_it_had() {
    if !library_ships() {
        return;
    }
    let Some(js) = scene() else {
        return;
    };
    let part = pages_part(&js);
    // morphdom replaces the canvas on a patch. Re-joining there would cost the
    // cell a whole page, so the channel stays and the listeners and the
    // picture follow the new node.
    assert!(
        part.contains("if (link && link.canvas === canvas) {")
            && part.contains("        continue;\n      }\n"),
        "the same element is not wired twice: {part:?}"
    );
    assert!(
        part.contains("link.canvas = canvas;")
            && part.contains("link.unwire = pageWire(link, el);"),
        "a new element is re-wired without a new join"
    );
}
