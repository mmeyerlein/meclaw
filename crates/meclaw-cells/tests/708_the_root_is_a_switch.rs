//! GH #708 -- `/<mount>/` is a switch (§ 6.5).
//!
//! "The phone output is called `phone` and lies at `/<mount>/phone` … `/<mount>/`
//! is a **switch**: the root page checks once, in the client,
//! `(pointer: coarse) and (max-width: 600px)` and redirects to `/<mount>/phone`,
//! otherwise to `default_screen`. Explicit URLs (`/<mount>/tv`, `/<mount>/monitor`,
//! `/<mount>/phone`) override the switch; a name without an entry in `screens`
//! is a 404, no switch. When `screens.phone` is missing, the switch leads to
//! `default_screen`. The profile stays a server matter" (§ 6.5).
//!
//! Until this wave `/<mount>/` was a PAGE: the server rendered the default
//! output's tree there and nothing ever asked what the device is. A phone that
//! opened the bare address got the monitor's rendering, and the only way to the
//! phone's was to know its name and type it.
//!
//! Two halves, and this file pins the client's: the root page carries
//! `data-switch="1"` (the server's half, H1) and the client leaves at once. An
//! explicit exit carries no `data-switch`, so nothing there ever moves -- which
//! is what "explicit URLs override the switch" means when the check lives in
//! the browser.
//!
//! The browser proof is B-15 (all four cases of § 6.5), strand H5.
//!
//! Skips when the templates do not ship (R2b).

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn block(name: &str) -> String {
    let src = read(COMPOSE);
    let open = format!("{name} = (");
    let start = src.find(&open).unwrap_or_else(|| panic!("{name} is gone"));
    let end = src[start..]
        .find("\n)")
        .unwrap_or_else(|| panic!("{name} is not one parenthesised block"));
    src[start..start + end].to_string()
}

#[test]
fn the_root_carries_what_the_switch_needs() {
    if !library_ships() {
        return;
    }
    let shell = block("SHELL_TEMPLATE");
    for attr in [
        "data-switch=\"{{switch}}\"",
        "data-default=\"{{default_screen}}\"",
        "data-screens=\"{{screens}}\"",
    ] {
        assert!(
            shell.contains(attr),
            "the shell does not carry `{attr}` -- the switch decides in the \
             browser (§ 6.5) and reads all three off the root: whether this \
             page is the switch, where it leads, and which exits exist"
        );
    }
    let compose = read(COMPOSE);
    for prop in ["\"switch\": \"text\"", "\"default_screen\": \"text\""] {
        assert!(
            compose.contains(prop),
            "the root component does not declare {prop}: a prop no schema \
             knows is a prop no patch carries"
        );
    }
}

#[test]
fn the_switch_asks_the_device_and_leaves() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    let fun = scene
        .find("function switchExit(")
        .expect("there is no switch in the client (§ 6.5)");
    assert!(
        scene.contains("(pointer: coarse) and (max-width: 600px)"),
        "the switch asks something other than the query § 6.5 names"
    );
    assert!(
        scene.contains("location.replace("),
        "the switch does not REPLACE the entry: a redirect that pushes leaves \
         the switch in the history, and `back` walks into it again"
    );
    assert!(
        scene.contains("getAttribute(\\\"data-switch\\\") !== \\\"1\\\""),
        "nothing tells the switch that this page is one: an explicit exit \
         carries no `data-switch` and must never move (§ 6.5)"
    );
    assert!(
        scene.contains("indexOf(\\\"phone\\\") > -1"),
        "the switch leads to `phone` without asking whether that exit exists \
         -- § 6.5: when `screens.phone` is missing it leads to `default_screen`"
    );
    let call = scene
        .find("if (switchExit(")
        .expect("the switch is defined and never run");
    let register = scene
        .find("root.SurfaceHooks = ")
        .expect("the scene hook registers nothing any more");
    assert!(
        fun < call && call < register,
        "the switch runs after the hooks are registered: a page that is about \
         to leave should not first boot a socket and a curation pass"
    );
}

/// The page that is about to replace itself boots nothing (§ 6.5). The scene
/// hook returns before it registers -- and the MARK is a script of its own, so
/// it has to ask again: without this, the switch page bound its listeners,
/// opened the socket and joined `voice:<call>` for the half second before
/// `location.replace`, which is the round trip the early return exists against.
#[test]
fn the_mark_does_not_boot_on_the_switch_page() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    let gate = os
        .find("data-switch\\\") === \\\"1\\\"")
        .expect("the mark's hook never asks whether this page is the switch (§ 6.5)");
    let bind = os
        .find("btn.addEventListener(")
        .expect("the hook binds nothing at all any more");
    let join = os
        .find("if (audio && !refused) join();")
        .expect("the join is gone");
    assert!(
        gate < bind && gate < join,
        "the mark binds and joins before it asks whether this page is the \
         switch -- a socket and a voice channel for a page that is leaving"
    );
}
