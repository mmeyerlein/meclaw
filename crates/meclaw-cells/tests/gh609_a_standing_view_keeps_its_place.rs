//! GH #609 -- the order of the screen reads no clock.
//!
//! Found while building an ambient application -- a clock, a weather tile and a
//! countdown, standing on a screen beside a conversation. `display@1.0.2` knew
//! exactly one region and sorted the views in it newest-first, so a view
//! rewritten every twenty seconds took the top slot on every tick: not because
//! it was important, but because it was recent. That is the right answer for a
//! card and the wrong one for anything standing.
//!
//! The theme outlived its first mechanism. Display 2.5.0 rebuilt the curator on
//! the reference model of display-hive.md, and the seat-by-first-appearance the
//! original fix introduced (`seated`, `seat_of`) is gone with it. What holds,
//! and what this file pins, is the sentence underneath: **the moment a view was
//! last written orders nothing.** Today three orders exist and none of them
//! reads a wall clock --
//!
//! * the rows the store hands the pass, by region, then the `ord` a view
//!   DECLARED, then owner and view id (`pass_views`);
//! * the open canvas windows, by lead, score, the younger `since` and the id
//!   (`canvas_order`, § 6.3), written onto the wrapper as a negative `ord`;
//! * the dock, by seat and rank (`step9_dock` -> `state["dock_order"]`, § 4.28).
//!
//! `since` is a TOUCH, not a write: a rewrite that changes no own prop moves it
//! not at all (§ 4.8 b), which is the case GH #609 was opened for. The last test
//! drives the shipped cell over five such rewrites and watches the wrapper's
//! `ord`.
//!
//! Skips when `python3` is absent or the templates do not ship, like every
//! other interpreter guard in this tree (R2b).

mod support;

use meclaw_core::serde_json::{Value, json};
use support::{COMPOSE, Screen, component_view, library_ships, pane, repo, window_id};

const CONFIG: &str = "templates/display/compose/config.json";
const VIEWS: &str = "templates/display/views/config.json";
const README: &str = "templates/display/README.md";

fn read_json(rel: &str) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(repo(rel)).expect(rel))
        .unwrap_or_else(|e| panic!("{rel} does not parse: {e}"))
}

fn source() -> String {
    std::fs::read_to_string(repo(COMPOSE)).expect("compose.py")
}

/// One `def` of the script, from its header to the next one at column zero --
/// so a claim about an ordering is asked of the function that orders, and not
/// of the whole file, where any other function could be answering for it.
fn body_of<'a>(src: &'a str, header: &str) -> &'a str {
    let start = src
        .find(header)
        .unwrap_or_else(|| panic!("`{header}` is not in compose.py"));
    let rest = &src[start + header.len()..];
    let end = rest.find("\ndef ").map(|i| i + 1).unwrap_or(rest.len());
    &rest[..end]
}

/// The source and the runtime copy of it are the same bytes.
///
/// A `code` cell runs `params.script_inline`; the `.py` beside it is what a
/// person reads. Every test here drives the `.py`, so this is what makes their
/// verdict a verdict about the cell.
#[test]
fn the_shipped_script_is_the_file_beside_it() {
    if !repo(COMPOSE).exists() {
        return;
    }
    let src = source();
    let cfg = read_json(CONFIG);
    let inline = cfg["params"]["script_inline"]
        .as_str()
        .expect("compose declares script_inline");
    assert_eq!(inline, src, "script_inline has drifted from compose.py");
}

/// The two regions are declared once, in the order they stand.
#[test]
fn the_regions_are_a_closed_list_with_main_first() {
    if !repo(COMPOSE).exists() {
        return;
    }
    let src = source();
    assert!(
        src.contains(r#"REGIONS = ("main", "aside")"#),
        "`main` first, because it is the DEFAULT: a view that names no region \
         has to land where it landed before there was a second one"
    );
    assert!(
        src.contains("\"ord\": i * ORD_STEP,"),
        "the regions' own `ord` comes from the declaration order -- two \
         regions at 0 were the second half of GH #609"
    );
}

/// The `views` table holds the band a view asked for, or the order is a guess.
#[test]
fn the_table_carries_the_declared_ord() {
    if !repo(VIEWS).exists() {
        return;
    }
    let cfg = read_json(VIEWS);
    assert_eq!(
        cfg["params"]["schema"]["views"]["ord"], "int",
        "a declared `ord` that is not stored is a band that survives exactly \
         one write"
    );
    assert_eq!(
        cfg["params"]["schema"]["views"]["region"], "text",
        "and the region it stands in beside it"
    );
    let src = source();
    assert!(
        src.contains("\"ord\","),
        "the column list the select projects has to name it too, or the plan \
         reads a row that has lost it"
    );
}

/// No order of the screen reads a clock -- and the README says so.
///
/// A drift lock, both halves (`docs/development-rules.md` § 2d). The prose half
/// is the retraction the README carries under "What no longer holds": *the
/// canvas is ordered by what the windows mean*. The mechanism half is that the
/// three orderings name only meaning, and that `updated_at` -- the column that
/// once sorted the screen -- is written by the cell and read by nothing in it.
#[test]
fn no_order_of_the_screen_reads_a_clock() {
    if !repo(COMPOSE).exists() || !repo(README).exists() {
        return;
    }
    let doc = std::fs::read_to_string(repo(README)).expect("README");
    assert!(
        doc.contains("the canvas is ordered by what the windows mean"),
        "the promise the cell makes has to be the promise the README makes"
    );
    let src = source();

    // The canvas, § 6.3: lead, score, the younger `since`, the id. `since` is
    // the moment of the last TOUCH; a rewrite that touches nothing leaves it.
    let canvas = body_of(&src, "def canvas_order(state):");
    for key in ["\"rung\"", "\"score\"", "\"since\""] {
        assert!(
            canvas.contains(key),
            "the canvas order reads {key}:{canvas}"
        );
    }
    assert!(
        !canvas.contains("updated_at") && !canvas.contains("written_at"),
        "sorting the canvas on the last write IS GH #609:{canvas}"
    );

    // The dock, § 4.28: seats by `seat_ord`, the rest by rank.
    let dock = body_of(&src, "def step9_dock(state, now):");
    for key in ["seat_ord", "\"rank\""] {
        assert!(dock.contains(key), "the dock order reads {key}");
    }
    assert!(
        !dock.contains("updated_at") && !dock.contains("written_at"),
        "the dock does not sort on the last write either"
    );

    // The rows the cell holds (GH #809: in memory, `sorted_rows`): a deterministic order
    // for the pass, and nothing more. What a person SEES is the pass's word, so the `ord` a SENDER asked for is not
    // read here either -- § 2 says a seat is "not the first-appearance order of views
    // (`ord`)", and § 10 retires "canvas order by first appearance" in favour of § 6.3.
    let rows = body_of(&src, "def sorted_rows(rows):");
    assert!(
        rows.contains("REGION_INDEX.get(str(x.get(\"region\") or REGIONS[0]), 0),")
            && rows.contains("str(x.get(\"owner\") or \"\"),"),
        "the row order names the region and identity:{rows}"
    );
    assert!(
        !rows.contains("declared_ord") && !src.contains("def declared_ord("),
        "the band a view declared orders nothing any more (§ 2 Seat, § 10)"
    );

    // And the windows that are NOT open share one place, because they have no place on
    // the canvas to be ordered in (§ 6.3): only what is open is ordered, and it is
    // ordered by meaning. A second numbering beside it would move every window of a
    // region whenever one arrives whose id sorts ahead, for a difference nobody sees.
    let tree = body_of(
        &src,
        "def objects_from_state(state, rows, now, name, have=None):",
    );
    assert!(
        tree.contains("canvas_order(state)") && tree.contains("\"ord\": 0,"),
        "the tree orders the open canvas windows and nothing else:{tree}"
    );

    // And the strongest reading of the sentence: nothing ORDERS on `updated_at`. The
    // column is read in exactly two places, and neither is an order:
    //
    //   * the reconciliation of OR-H0.9 asks a row WHEN the store wrote it, so a write it
    //     has to replay at the boot lands at its own moment and `ttl_ms` and the decay
    //     count from there -- the column used as a clock, which is what it is;
    //   * and a write the door took is passed at the moment it was stamped
    //     (`accept_row`), the same clock for the same reason.
    //
    // (The third reader, the state row's version check of GH #744, left with the state
    // row in display 2.7.0, GH #809.) A rank is what GH #609 was, and that is still
    // nowhere.
    let readers = [
        "def reconcile(state, rows, event, now):",
        "def _written_at(value, fallback):",
        "def accept_row(owner, row, withdraw):",
    ];
    let inside: usize = readers
        .iter()
        .map(|header| {
            let body = body_of(&src, header);
            body.matches("get(\"updated_at\")").count() + body.matches("[\"updated_at\"]").count()
        })
        .sum();
    let total =
        src.matches("get(\"updated_at\")").count() + src.matches("[\"updated_at\"]").count();
    assert_eq!(
        total, inside,
        "`updated_at` is read outside the reconciliation -- that is the bug this file is \
         named after"
    );
    assert!(inside > 0, "and the two readers are still the two readers");
    for header in [
        "def canvas_order(state):",
        "def step9_dock(state, now):",
        "def sorted_rows(rows):",
        "def objects_from_state(state, rows, now, name, have=None):",
    ] {
        let body = body_of(&src, header);
        assert!(
            !body.contains("updated_at") && !body.contains("written_at"),
            "{header} orders on a clock:{body}"
        );
    }
}

fn params() -> Value {
    json!({"screens": {"monitor": {"display_type": "monitor", "inputs": ["pointer"]}},
           "default_screen": "monitor"})
}

/// A standing view, as an ambient application writes it: same props every time.
fn standing(view_id: &str) -> Value {
    component_view(
        view_id,
        "main",
        pane(
            view_id,
            json!({"title": view_id, "context": "ambient", "relevance": "0.9",
                   "topic": format!("standing:{view_id}")}),
        ),
    )
}

fn ord_of(screen: &Screen, id: &str) -> i64 {
    screen
        .held
        .as_array()
        .expect("the display holds a list")
        .iter()
        .find(|o| o["id"] == id)
        .unwrap_or_else(|| panic!("{id} is held"))["ord"]
        .as_i64()
        .expect("an `ord` is a number")
}

/// The case of GH #609, over the shipped cell: a view rewritten five times
/// keeps its place.
///
/// Two standing windows, written in the same moment, so nothing but the id
/// separates them in `canvas_order`. Then the second one is written again five
/// times with the props it already has -- which is no touch (§ 4.8 b), so
/// `since` stands, so the order stands. Under `display@1.0.2` each of those
/// five writes would have taken the top place.
#[test]
fn a_view_rewritten_five_times_keeps_the_place_it_stands_in() {
    if !library_ships() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut screen = Screen::new(params());
    screen.write(standing("a"), 100_000);
    screen.write(standing("b"), 100_000);
    // § 4.35: a fresh window takes no focus in the pass it appears in.
    screen.pass(json!({"kind": "stroke"}), 101_000);

    let (a, b) = (window_id("alex", "a"), window_id("alex", "b"));
    let before = (ord_of(&screen, &a), ord_of(&screen, &b));
    assert!(
        before.0 < before.1,
        "the two stand in id order, the leading one first: {before:?}"
    );

    for (i, now) in [102_000u64, 103_000, 104_000, 105_000, 106_000]
        .into_iter()
        .enumerate()
    {
        let calls = screen.write(standing("b"), now);
        assert!(
            !calls.iter().any(|c| c["op"] == "object.move"),
            "rewrite {i} moved something: {calls:?}"
        );
        assert_eq!(
            (ord_of(&screen, &a), ord_of(&screen, &b)),
            before,
            "rewrite {i} moved the screen"
        );
        assert_eq!(
            screen.curator(&b, "since"),
            json!(100_000),
            "rewrite {i} is no touch, so `since` stands (§ 4.8 b)"
        );
    }

    // ...and the rewrite did land: the window is still there, still open.
    assert!(screen.holds(&format!("{b}/c.b")), "the window stands");
    assert_eq!(screen.curator(&b, "open"), json!(true));
}
