//! GH #961 — motion by target state: blocks glide in, many marks move in one pass.
//!
//! The screen draws states, never frames. A block the catalogue marks `block: true`
//! glides in when it is inserted into the DOM (a CSS animation on mount, `--enter-ms`),
//! and a `display-mark` in a `display-field` moves when its target changes: the template
//! writes `--x`/`--y` out of the two typed int props, the sheet turns them into a
//! `transform` with `transition: transform var(--move-ms)`, and the device animates.
//! Nothing travels the wire but the new target (display-hive § 1.5: a diff of hundreds
//! of positions for one gesture is a defect).
//!
//! What this file holds, in Chromium AND WebKit (R-23-10), measured by
//! `workshop/tools/display-motion-browser.mjs` at the receiver -- the page's own socket
//! and its DOM, never the emission:
//!
//! - (a) a card written while the page stands has a running enter animation; an update
//!   of the same card keeps the node and animates nothing again; under
//!   `prefers-reduced-motion: reduce` a fresh card has no animation at all;
//! - (b) one write that gives 200 marks new targets arrives as exactly ONE `diff` frame
//!   that moves a mark (proved against a sentinel write behind it, not by waiting), every
//!   one of the 200 runs a `transform` transition, none is replaced, and after the
//!   transitions are finished every mark stands on its target within 1 px;
//! - (c) a write that changes only a nested field's target moves the mark inside it
//!   with the field and changes no mark on the wire.
//!
//! Station `browser:display` (`scripts/gate_plan.py`, `BROWSER_LOCKS`), `#[ignore]`d
//! everywhere else. No node, no playwright, no engine, no template library or no python
//! is a `SKIP`, never a red (R2b).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::path::Path;
use std::time::Duration;

use display_colony::{Boot, PROBE, SCREEN, boot, have_python, library_ships, repo};
use meclaw_core::serde_json::{Value, json};

const DRIVER: &str = "workshop/tools/display-motion-browser.mjs";
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium
/// one (the driver's default engine is WebKit). Sourced for Chromium too, a host without
/// the laboratory read its `SKIP` line as the Chromium run's own, and the lock proved
/// nothing there (XB-M review I-1: build02 skipped every Chromium run).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// `sh -c` with `$1` the laboratory, the rest the driver and its arguments.
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";
/// The window that carries the field.
const VIEW: &str = "motion";
/// How many marks one write moves.
const MARKS: i64 = 200;
/// The field's coordinate space, both ways.
const SPACE: i64 = 1000;
/// How long one engine's run may take, all in.
const DRIVER_LIMIT: Duration = Duration::from_secs(180);

/// Mark `i`'s target in round `a` (`true`) or round `b`: a grid of 20 x 10, and the
/// second round is the first mirrored through the centre -- so every mark moves, and no
/// two marks share a target.
fn target(i: i64, a: bool) -> (i64, i64) {
    let (x, y) = ((i % 20) * 50 + 25, (i / 20) * 100 + 50);
    if a { (x, y) } else { (SPACE - x, SPACE - y) }
}

/// The window with the field: 200 marks at round `a`/`b` and a nested 100 x 100 field
/// at `nest`, with one mark in its middle.
fn field(title: &str, a: bool, nest: (i64, i64)) -> Value {
    let mut children: Vec<Value> = (0..MARKS)
        .map(|i| {
            let (x, y) = target(i, a);
            json!({"component": "display-mark", "key": format!("c.m{i}"),
                   "props": {"x": x, "y": y, "glyph": "*", "label": format!("m{i}")}})
        })
        .collect();
    children.push(json!({
        "component": "display-field", "key": "c.nest",
        "props": {"w": 100, "h": 100, "x": nest.0, "y": nest.1},
        "children": [{"component": "display-mark", "key": "c.inner",
                      "props": {"x": 50, "y": 50, "glyph": "*", "label": "inner"}}]
    }));
    json!({
        "component": "display-pane", "key": "c.win",
        "props": {"pane_id": "pane-motion", "title": title, "context": "work",
                  "relevance": "0.9", "topic": "motion:1", "touched": "1"},
        "children": [{"component": "display-field", "key": "c.field",
                      "props": {"w": SPACE, "h": SPACE}, "children": children}]
    })
}

/// A window with one card.
fn card(view: &str, title: &str, value: &str) -> Value {
    json!({
        "component": "display-pane", "key": "c.win",
        "props": {"pane_id": format!("pane-{view}"), "title": view, "context": "work",
                  "relevance": "0.8", "topic": format!("{view}:1"), "touched": "2"},
        "children": [{"component": "display-card", "key": "c.card",
                      "props": {"kicker": "motion", "title": title, "value": value}}]
    })
}

/// One `POST /messages` body: a write at the screen's door, the shape `put_content` sends.
fn in_view(view: &str, content: Value) -> Value {
    json!({"target": SCREEN, "hop": {"route": "in_view"},
           "body": {"view_id": view, "region": "main", "kind": "component",
                    "content": content, "components": [], "ttl_ms": 0, "messages": []}})
}

fn in_withdraw(view: &str) -> Value {
    json!({"target": SCREEN, "hop": {"route": "in_withdraw"},
           "body": {"view_id": view, "messages": []}})
}

/// The steps of one engine's run, in the order the driver sends them.
fn write_steps(dir: &Path, engine: &str) {
    let enter = format!("enter-{engine}");
    let still = format!("still-{engine}");
    let steps = [
        (
            "1-card",
            in_view(&enter, card(&enter, &format!("card {engine}"), "1")),
        ),
        (
            "2-card-update",
            in_view(&enter, card(&enter, &format!("card {engine}"), "2")),
        ),
        ("3-card-withdraw", in_withdraw(&enter)),
        (
            "4-reduced-card",
            in_view(&still, card(&still, &format!("still {engine}"), "1")),
        ),
        ("5-reduced-withdraw", in_withdraw(&still)),
        ("6-move", in_view(VIEW, field("motion", false, (100, 100)))),
        (
            "7-sentinel",
            in_view(VIEW, field(&format!("moved {engine}"), false, (100, 100))),
        ),
        (
            "8-nest",
            in_view(VIEW, field(&format!("moved {engine}"), false, (700, 600))),
        ),
        (
            "9-nest-sentinel",
            in_view(VIEW, field(&format!("nested {engine}"), false, (700, 600))),
        ),
    ];
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("the steps directory");
    for (name, body) in steps {
        std::fs::write(dir.join(format!("{name}.json")), body.to_string()).expect("a step");
    }
}

/// One engine's report, or `None` when this host cannot measure.
async fn drive(engine: &str, url: &str, api: &str, dir: &Path) -> Option<Value> {
    let run = tokio::time::timeout(
        DRIVER_LIMIT,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(WITH_LAB)
            .arg("sh")
            .arg(repo(WKENV))
            .arg(repo(DRIVER))
            .arg(url)
            .arg(dir)
            .arg("--engine")
            .arg(engine)
            .env("MECLAW_DISPLAY_API", api)
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let out = match run {
        Err(_) => panic!("{engine}: the motion driver did not finish within {DRIVER_LIMIT:?}"),
        Ok(Err(e)) => {
            println!("SKIP neither node nor a shell for it on this host: {e}");
            return None;
        }
        Ok(Ok(out)) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        println!("SKIP {engine}: {}", stderr.trim());
        return None;
    }
    let line = stdout
        .lines()
        .find(|l| l.starts_with('{'))
        .unwrap_or_else(|| {
            panic!(
                "{engine}: no report ({:?}):\n{stdout}\n{stderr}",
                out.status.code()
            )
        });
    println!("{engine}: {line}");
    let report: Value = meclaw_core::serde_json::from_str(line).expect("the report is JSON");
    assert!(
        report["error"].is_null(),
        "{engine}: the run itself broke: {report}\n{stderr}"
    );
    Some(report["values"].clone())
}

fn n(v: &Value, key: &str) -> i64 {
    v[key]
        .as_i64()
        .unwrap_or_else(|| panic!("{key} is not a number in {v}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "browser lock: runs in the station browser:display (scripts/gate_plan.py BROWSER_LOCKS)"]
async fn blocks_glide_in_and_many_marks_move_in_one_pass() {
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 in this tree");
        return;
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the driver or its laboratory does not ship in this tree");
        return;
    }
    // A long linger and fade: with the helper's defaults (2 s + 3 s, made for locks that
    // wait FOR the fade) the field's window decayed below the bar while the enter pages
    // ran, fell to `ambient` and was `display: none` -- measured on a lane, every mark at
    // a zero rect, no transition, and the pane re-inserted when its rung changed. This lock
    // measures motion inside a window that stays open, not the curator's fade.
    let colony = boot(Boot {
        linger_ms: 600_000,
        fade_ms: 600_000,
        ..Boot::default()
    })
    .await;
    let url = colony.url("monitor");
    let api = colony.api_url();
    let steps = tempfile::tempdir().expect("a steps directory");

    for engine in ["chromium", "webkit"] {
        // Every engine starts from the first round, written through the door and waited
        // for at `web`: what the browser opens is what the pass drew.
        let tree = colony
            .put_content(PROBE, VIEW, field("motion", true, (100, 100)))
            .await;
        assert!(
            tree.to_string().contains("display-mark"),
            "the field stands in the tree before the browser opens it: {tree}"
        );
        let dir = steps.path().join(engine);
        write_steps(&dir, engine);
        let Some(v) = drive(engine, &url, &api, &dir).await else {
            continue;
        };

        // (a) Enter on mount, not on update, not under reduced motion.
        assert!(
            n(&v, "enter") >= 1,
            "{engine}: an inserted card glides in: {v}"
        );
        assert_eq!(
            v["same_node"],
            json!(true),
            "{engine}: an update keeps the card's node: {v}"
        );
        assert_eq!(
            n(&v, "reanimated"),
            0,
            "{engine}: an update does not glide in again: {v}"
        );
        assert_eq!(
            n(&v, "reduced"),
            0,
            "{engine}: reduced motion runs no animation: {v}"
        );

        // (b) One write, 200 new targets: one frame, 200 transitions, 200 on target.
        assert_eq!(
            n(&v, "marks"),
            MARKS,
            "{engine}: the field carries every mark: {v}"
        );
        assert_eq!(
            n(&v, "mark_frames"),
            1,
            "{engine}: the 200 new targets arrive in ONE diff frame: {v}"
        );
        // ... and that frame is the pass's only one: no second, empty diff beside it
        // (B.3, review D1 M1). Clock ticks change their text and do not count here.
        assert_eq!(
            n(&v, "empty_frames"),
            0,
            "{engine}: the write sent no empty diff beside the one that moved: {v}"
        );
        assert_eq!(
            n(&v, "moved"),
            MARKS,
            "{engine}: every mark got its new target: {v}"
        );
        assert_eq!(
            n(&v, "replaced"),
            0,
            "{engine}: the patch moves marks, never re-inserts: {v}"
        );
        assert_eq!(
            n(&v, "transitions"),
            MARKS,
            "{engine}: every mark runs a transform transition right after the frame: {v}"
        );
        assert_eq!(
            n(&v, "on_target"),
            MARKS,
            "{engine}: every mark ends on its target within 1 px: {v}"
        );

        // (c) The nested field moves its mark; no mark changes on the wire.
        assert_eq!(
            n(&v, "nest_marks"),
            0,
            "{engine}: no mark changed for a field's move: {v}"
        );
        assert_eq!(
            n(&v, "nest_fields"),
            1,
            "{engine}: one prop pair, on the nested field: {v}"
        );
        assert_eq!(
            n(&v, "nest_empty_frames"),
            0,
            "{engine}: the field's move sent no empty diff either: {v}"
        );
        assert_eq!(
            v["nest_transition"],
            json!(true),
            "{engine}: the nested field glides: {v}"
        );
        assert_eq!(
            v["nest_on_target"],
            json!(true),
            "{engine}: the inner mark is on target: {v}"
        );
        let got = &v["nest_delta_px"];
        let want = &v["nest_expect_px"];
        for k in 0..2 {
            let (g, w) = (
                got[k].as_f64().unwrap_or(f64::NAN),
                want[k].as_f64().unwrap_or(0.0),
            );
            assert!(
                (g - w).abs() <= 1.0,
                "{engine}: the inner mark moved with its field ({g} vs {w} px): {v}"
            );
        }
    }

    colony.shutdown().await;
}
