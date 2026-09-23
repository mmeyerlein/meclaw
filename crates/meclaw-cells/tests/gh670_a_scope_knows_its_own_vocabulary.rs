//! GH #670 -- a scope recognises its own outdated vocabulary.
//!
//! The compose cell defines its components on the bootstrap and never again:
//! `bootstrap` is true when the tree `web` holds has no root of ours. A screen
//! that is already running would, after a template swap, get new code and keep
//! the old vocabulary. So the root carries a fingerprint of `components()` as a
//! prop, and a boot whose read finds a root with another fingerprint redefines
//! everything -- once per change of vocabulary, not per stroke. Same economy the
//! screen already has for an application's components ("the definitions only
//! travel when they changed"), now for its own language.
//!
//! Since display 2.7.0 the cell runs `resident` (GH #809): it reads the tree
//! ONCE, when a fresh child boots (and after a refused patch), and diffs every
//! later pass against what it sent itself. A template swap is exactly such a
//! fresh child, so the tests kill the cell over a tree `web` still holds and
//! wake it with a stroke -- an event that changes nothing on its own -- and read
//! the one patch its boot sends. The hive is played by `support::Screen`.
//! Skips when the templates do not ship (R2b).

mod support;

use std::process::Command;

use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, component_view, library_ships, pane, repo};

/// The fingerprint the shipped script computes (`compose.VOCAB`), the one a
/// local copy of `components()` computes the same way, and the one a copy with
/// a single changed template computes. `None` when there is no `python3` on
/// this host.
fn probe() -> Option<Value> {
    let out = Command::new("python3")
        .arg("-c")
        .arg(
            "import hashlib, importlib.util, json, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             compose = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(compose)\n\
             def fp(c):\n\
             \treturn hashlib.sha256(json.dumps(c, sort_keys=True).encode('utf-8')).hexdigest()[:12]\n\
             same = compose.components()\n\
             moved = compose.components()\n\
             moved[0]['template'] = moved[0]['template'] + ' '\n\
             print(json.dumps({'vocab': compose.VOCAB, 'same': fp(same), 'moved': fp(moved),\n\
                               'count': len(same)}))",
        )
        .arg(repo(COMPOSE))
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(meclaw_core::serde_json::from_slice(&out.stdout).expect("the probe is JSON"))
}

/// A screen with one window, booted: its first write found a fresh `web`, so its
/// one patch is the bootstrap. The bootstrap's calls come back beside it.
///
/// One stroke follows at the same moment: a write leaves its window `fresh` only
/// for its own pass, and the stroke settles it, so a later stroke -- of this cell
/// or of a fresh child -- has nothing of its own to draw.
fn booted() -> (Screen, Vec<Value>) {
    let mut screen = Screen::new(json!({
        "screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
        "default_screen": "monitor",
    }));
    let first = screen.write(
        component_view("a", "main", pane("a", json!({"title": "x"}))),
        1000,
    );
    screen.pass(json!({"kind": "stroke"}), 1000);
    (screen, first)
}

/// A fresh child over the tree `web` holds, woken by a stroke: the patch of its
/// boot. The boot's read is asserted, so an empty patch is a read that found
/// nothing to change and not a read that never happened.
fn reboot(screen: &mut Screen) -> Vec<Value> {
    screen.kill();
    let calls = screen.pass(json!({"kind": "stroke"}), 1000);
    assert_eq!(
        screen
            .hops()
            .iter()
            .filter(|h| h["route"] == "read")
            .count(),
        1,
        "a fresh child reads the tree once: {:?}",
        routes(screen)
    );
    calls
}

fn routes(screen: &Screen) -> Vec<String> {
    screen
        .hops()
        .iter()
        .map(|h| h["route"].as_str().unwrap_or("").to_string())
        .collect()
}

fn ops(calls: &[Value]) -> Vec<&str> {
    calls
        .iter()
        .map(|c| c["op"].as_str().unwrap_or(""))
        .collect()
}

fn count(calls: &[Value], op: &str) -> usize {
    calls.iter().filter(|c| c["op"] == op).count()
}

/// A page whose root is ours but carries another fingerprint is not a
/// bootstrap -- and still gets every component of the scope defined again,
/// with the root brought up to the fingerprint of the code that runs.
#[test]
fn a_swapped_scope_redefines_its_vocabulary_without_a_bootstrap() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let (mut screen, _) = booted();
    // The tree an older code left behind: the same root, another language.
    screen.web_update("display.root", json!({"vocab": "0000deadbeef"}));

    let calls = reboot(&mut screen);
    let defines = count(&calls, "component.define");
    assert_eq!(
        defines,
        probe["count"].as_u64().expect("a count") as usize,
        "every component of the scope is defined again: {:?}",
        ops(&calls)
    );
    assert_eq!(
        count(&calls, "object.create"),
        0,
        "not a bootstrap: nothing is created: {:?}",
        ops(&calls)
    );
    assert_eq!(
        count(&calls, "page.set"),
        0,
        "not a bootstrap: the page stands"
    );
    // The definitions come first -- an object that names a component may only
    // be written once the component exists -- and the root is then brought up
    // to the fingerprint of the vocabulary that was just defined.
    assert!(
        ops(&calls)[..defines]
            .iter()
            .all(|op| *op == "component.define"),
        "the definitions lead the patch: {:?}",
        ops(&calls)
    );
    let update = calls
        .iter()
        .find(|c| c["op"] == "object.update" && c["id"] == "display.root")
        .unwrap_or_else(|| panic!("the root is brought up to date: {:?}", ops(&calls)));
    assert_eq!(update["props"]["vocab"], probe["vocab"]);

    // Once per change of vocabulary: the next stroke of the same cell diffs against
    // what it sent and sends no definition again.
    let next = screen.pass(json!({"kind": "stroke"}), 1000);
    assert_eq!(
        count(&next, "component.define"),
        0,
        "the vocabulary travelled once: {:?}",
        ops(&next)
    );
}

/// The same page with the fingerprint the code computes: no definition, no
/// root update, nothing at all -- the screen is what the code would build.
#[test]
fn an_unchanged_vocabulary_sends_no_definition() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let (mut screen, first) = booted();
    assert_eq!(
        count(&first, "component.define"),
        probe["count"].as_u64().expect("a count") as usize,
        "the bootstrap defines the whole scope"
    );
    let root = first
        .iter()
        .find(|c| c["op"] == "object.create" && c["id"] == "display.root")
        .expect("the bootstrap creates the root");
    assert_eq!(
        root["props"]["vocab"], probe["vocab"],
        "the root is created with the fingerprint of the code that created it"
    );

    let calls = reboot(&mut screen);
    assert_eq!(
        count(&calls, "component.define"),
        0,
        "an unchanged vocabulary travels nowhere: {:?}",
        ops(&calls)
    );
    assert!(
        !calls
            .iter()
            .any(|c| c["op"] == "object.update" && c["id"] == "display.root"),
        "the root is not rewritten: {:?}",
        ops(&calls)
    );
    assert!(
        calls.is_empty(),
        "a screen that matches the code gets no patch at all: {:?}",
        ops(&calls)
    );
    assert!(
        !routes(&screen).iter().any(|r| r == "patch"),
        "not even an empty one: {:?}",
        routes(&screen)
    );
}

/// The fingerprint is twelve hex characters over `components()` as JSON, and
/// it moves when one template string does. Computed here over a local copy of
/// the list, never by patching the source.
#[test]
fn the_fingerprint_moves_when_a_component_does() {
    if !library_ships() {
        return;
    }
    let Some(probe) = probe() else {
        return;
    };
    let vocab = probe["vocab"].as_str().expect("VOCAB is a string");
    assert_eq!(vocab.len(), 12, "twelve characters: {vocab}");
    assert!(vocab.chars().all(|c| c.is_ascii_hexdigit()), "hex: {vocab}");
    assert_eq!(
        probe["same"], probe["vocab"],
        "the fingerprint is sha256 of components() as sorted JSON, cut to twelve"
    );
    assert_ne!(
        probe["moved"], probe["vocab"],
        "one changed template string moves the fingerprint"
    );
}
