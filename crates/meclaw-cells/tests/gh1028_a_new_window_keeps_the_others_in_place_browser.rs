//! GH #1028 — a new window that ranks first keeps the others in place.
//!
//! Measured on the display: whenever a window opened AHEAD of the windows already standing,
//! a MutationObserver saw every canvas window section removed and inserted again on that
//! one patch, and the verdict reached the DOM at p50 ≈ 1 s under load (≈ 650 ms quiet).
//! The cause is the client's keyed morph: a node inserted ahead of keyed siblings makes it
//! re-append every sibling behind it. The screen now keeps windows in the DOM in the order
//! they arrived and writes each window's place on the canvas as `--rank`, which the sheet
//! reads as the grid's `order`.
//!
//! What this file holds, in Chromium AND WebKit, measured by
//! `workshop/tools/display-keep-browser.mjs` at the receiver: after three windows stand, a
//! fourth of high relevance opens; the three held window elements are still the connected
//! elements their ids resolve to, none of them was removed or added by any patch (proved
//! against a sentinel write behind it, not by waiting), and the new window still stands
//! first in reading order.
//!
//! Station `browser:display` (`scripts/gate_plan.py`, `BROWSER_LOCKS`), `#[ignore]`d
//! everywhere else. No node, no playwright, no engine, no template library or no python
//! is a `SKIP`, never a red (R2b).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::path::Path;
use std::time::Duration;

use display_colony::{Boot, SCREEN, boot, have_python, library_ships, repo};
use meclaw_core::serde_json::{Value, json};

const DRIVER: &str = "workshop/tools/display-keep-browser.mjs";
/// The laboratory WebKit runs in on this host, sourced for every run except a Chromium
/// one (the driver's default engine is WebKit). Sourced for Chromium too, a host without
/// the laboratory read its `SKIP` line as the Chromium run's own, and the lock proved
/// nothing there (XB-M review I-1: build02 skipped every Chromium run).
const WKENV: &str = "workshop/tools/wkenv.sh";
/// `sh -c` with `$1` the laboratory, the rest the driver and its arguments.
const WITH_LAB: &str =
    "case \" $* \" in *\" --engine chromium \"*) ;; *) . \"$1\" ;; esac; shift; exec node \"$@\"";
/// How long one engine's run may take, all in.
const DRIVER_LIMIT: Duration = Duration::from_secs(180);

/// A window with one card. Its pane's DOM id is its `pane_id`.
fn card(view: &str, title: &str, relevance: &str) -> Value {
    json!({
        "component": "display-pane", "key": "c.win",
        "props": {"pane_id": format!("pane-{view}"), "title": title, "context": "work",
                  "relevance": relevance, "topic": format!("{view}:1"), "touched": "2"},
        "children": [{"component": "display-card", "key": "c.card",
                      "props": {"kicker": "keep", "title": view, "value": "1"}}]
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

/// The steps of one engine's run. The views carry the engine's name, so the windows of
/// the run before (withdrawn, maybe still leaving) never share an id with this one's.
fn write_steps(dir: &Path, engine: &str) {
    let v = |x: &str| format!("keep-{x}-{engine}");
    let steps = [
        ("1-a", in_view(&v("a"), card(&v("a"), &v("a"), "0.8"))),
        ("2-b", in_view(&v("b"), card(&v("b"), &v("b"), "0.85"))),
        ("3-c", in_view(&v("c"), card(&v("c"), &v("c"), "0.9"))),
        (
            "4-new",
            in_view(&v("new"), card(&v("new"), &v("new"), "0.99")),
        ),
        (
            "5-sentinel",
            in_view(
                &v("new"),
                card(&v("new"), &format!("seen {engine}"), "0.99"),
            ),
        ),
        ("6-withdraw-a", in_withdraw(&v("a"))),
        ("7-withdraw-b", in_withdraw(&v("b"))),
        ("8-withdraw-c", in_withdraw(&v("c"))),
        ("9-withdraw-new", in_withdraw(&v("new"))),
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
        Err(_) => panic!("{engine}: the keep driver did not finish within {DRIVER_LIMIT:?}"),
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
async fn a_new_window_ahead_keeps_the_others_in_place() {
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 in this tree");
        return;
    }
    if !repo(DRIVER).is_file() || !repo(WKENV).is_file() {
        println!("SKIP the driver or its laboratory does not ship in this tree");
        return;
    }
    // A long linger and fade, as in gh961: the windows must stay open on the canvas
    // while the run measures them, not decay to `ambient` (`display: none`).
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
        let dir = steps.path().join(engine);
        write_steps(&dir, engine);
        let Some(v) = drive(engine, &url, &api, &dir).await else {
            continue;
        };
        assert_eq!(
            n(&v, "same_nodes"),
            3,
            "{engine}: the three standing windows keep their nodes: {v}"
        );
        assert_eq!(
            n(&v, "touched"),
            0,
            "{engine}: no patch removed or re-inserted a standing window: {v}"
        );
        assert_eq!(
            v["new_first"],
            json!(true),
            "{engine}: the new window still stands first in reading order: {v}"
        );
    }

    colony.shutdown().await;
}
