//! GH #609 -- a second region beside the conversation, and an order that a
//! rewrite cannot move.
//!
//! Found while building an ambient application -- a clock, a weather tile and a
//! countdown, standing on a screen beside a conversation. `display@1.0.2` knew
//! exactly one region and sorted the views in it newest-first, so a view
//! rewritten every twenty seconds took the top slot on every tick: not because
//! it was important, but because it was recent. That is the right answer for a
//! card and the wrong one for anything standing.
//!
//! Two changes, and a trap between them. The order inside a region is now the
//! `ord` a view DECLARED, then its first appearance, and never the moment it
//! was last written. The second region is `aside`. The trap was that both
//! regions used to hang under the root at `ord: 0`, and the root was documented
//! as taking exactly one child -- a constraint GH #394 had already lifted in
//! the `web` cell (n+1 statics for n slots), which nobody had come back to.
//!
//! The measurement lives in `tests/fixtures/gh609_region_order_check.py`,
//! because it has to drive the SHIPPED cell through all four of its passes with
//! a store and a display on the other end. What this file adds is that
//! `cargo test` runs it -- plus the drift locks (`docs/development-rules.md`
//! § 2d) on the sentences the templates now make.
//!
//! Skips when `python3` is absent or the templates do not ship, like every
//! other interpreter guard in this tree (R2b).

use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const CONFIG: &str = "templates/display/compose/config.json";
const VIEWS: &str = "templates/display/views/config.json";
const README: &str = "templates/display/README.md";
const CHECK: &str = "crates/meclaw-cells/tests/fixtures/gh609_region_order_check.py";

fn read_json(rel: &str) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(repo(rel)).expect(rel))
        .unwrap_or_else(|e| panic!("{rel} does not parse: {e}"))
}

/// The whole loop, over the shipped bytes: two regions, and five rewrites that
/// move nothing.
#[test]
fn a_view_rewritten_five_times_keeps_the_seat_it_arrived_in() {
    for rel in [COMPOSE, CHECK] {
        if !repo(rel).exists() {
            return;
        }
    }
    let out = match std::process::Command::new("python3")
        .arg(repo(CHECK))
        .arg(repo(COMPOSE))
        .output()
    {
        Ok(o) => o,
        Err(_) => return, // no python3 on this host
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("all green"),
        "the screen's two regions and its order must hold:\n{stdout}\n{stderr}"
    );
    // The suite must actually have run something -- an empty file "passes" too.
    assert!(
        stdout.matches("  ok  ").count() >= 25,
        "too few checks ran; did the suite lose its cases?\n{stdout}"
    );
}

/// The source and the runtime copy of it are the same bytes.
///
/// A `code` cell runs `params.script_inline`; the `.py` beside it is what a
/// person reads. The checker above drives the `.py`, so this is what makes its
/// verdict a verdict about the cell.
#[test]
fn the_shipped_script_is_the_file_beside_it() {
    if !repo(COMPOSE).exists() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
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
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
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

/// The clock is gone from the ordering, and the seats are what replaced it.
#[test]
fn the_order_of_a_region_reads_no_clock() {
    if !repo(COMPOSE).exists() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    assert!(
        !src.contains("-int(r.get(\"updated_at\") or 0),"),
        "sorting a region on the last write IS GH #609; `updated_at` is a \
         `ttl_ms` question and nothing else now"
    );
    assert!(
        src.contains("def seat_of(wrapper, region, have):")
            && src.contains("def seated(views, have):"),
        "first appearance is the seat the display already holds the view at"
    );
    assert!(
        src.contains("def build(views, have=None):"),
        "the layout READS what the display holds; without it there is nothing \
         for first appearance to be remembered in"
    );
    // `updated_at` still exists -- as the expiry clock, which is what it is for.
    assert!(
        src.contains("return ttl > 0 and now - written >= ttl"),
        "`updated_at` still answers the one question it was ever right for"
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
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    assert!(
        src.contains("\"ord\","),
        "the column list the select projects has to name it too, or the plan \
         reads a row that has lost it"
    );
}

/// The README describes the mechanism it has, not the one it used to have.
///
/// A drift lock, both halves (`docs/development-rules.md` § 2d): the sentence
/// AND the code that carries it.
#[test]
fn the_readme_promises_the_order_the_cell_implements() {
    if !repo(README).exists() {
        return;
    }
    let doc = std::fs::read_to_string(repo(README)).expect("README");
    assert!(
        doc.starts_with("# `display@2.1.0`"),
        "the README names the version it describes"
    );
    assert!(
        doc.contains("| `aside` |"),
        "the second region is documented in the table a sender looks in"
    );
    assert!(
        doc.contains("**The moment a view was last written is not one of them**"),
        "the promise the cell now makes has to be the promise the README makes"
    );
    // The old sentence, in the present tense, in any of the three spellings it
    // had. "newest-first" survives ONCE, in the paragraph that retracts it.
    assert!(
        !doc.contains("newest first")
            && !doc.contains("newest view first")
            && doc.matches("newest-first").count() == 1,
        "the sentence outlived its mechanism once already -- that is exactly \
         what this lock is for"
    );
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py");
    assert!(
        src.contains("REGIONS = (\"main\", \"aside\")") && src.contains("def seated(views, have):"),
        "and the mechanism half of the lock: the regions and the seats the \
         prose names"
    );
}
