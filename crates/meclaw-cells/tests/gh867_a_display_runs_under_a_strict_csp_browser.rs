//! GH #867 — a display runs in a real browser under a strict Content-Security-Policy.
//!
//! The lock beside this one (`gh867_no_surface_page_carries_unlisted_inline_script.rs`)
//! reads the markup: no inline script that `templates/display/csp.json` does not list,
//! nothing from `blob:`. This file asks an engine. A throwaway display colony with an
//! `echo` voice cell on the mount the microphone joins is loaded by headless Chromium, and
//! the driver (`workshop/tools/display-csp-browser.mjs`) plays the proxy: every document
//! answer carries `Content-Security-Policy` built from `csp.json` plus
//! `default-src 'self'; object-src 'none'` -- `script-src 'self'` and the two published
//! hashes, no `'unsafe-inline'`, no `'unsafe-eval'`, no `blob:`.
//!
//! What must hold under it: the LiveView container connects (the boot is a file now,
//! `@client/boot.js`), the microphone hook joins and, with the mark held for a second on
//! Chromium's fake microphone, sends frames (the capture worklet is a file now,
//! `@client/display-mic-worklet.js`), the page raises no violation at all -- counted as
//! events AND as console lines, because Chromium refuses a `blob:` worklet in the console
//! only, without an event -- and hostile text written into a window's title, a line, a list and a table caption
//! stays text (`window.__pwned` stays undefined).
//!
//! Station `browser:display`, not `tests` (`scripts/gate_plan.py`, `BROWSER_LOCKS`): the
//! audio drivers of this tree have tripped under parallel browser load before (GH #763),
//! so a browser lock runs where browser locks run, and is `#[ignore]`d everywhere else.
//! No node, no browser, no template library or no python is a `SKIP`, never a red (R2b).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, boot_with_voice_echo, have_python, library_ships, repo};
use meclaw_core::serde_json::json;

/// The app whose window carries the hostile text.
const APP: &str = "/alex/apps/probe";
/// How long the browser driver may take, all in.
const DRIVER_LIMIT: Duration = Duration::from_secs(90);

/// The payloads of #868's component test that a TEXT prop takes: each one is markup, a
/// script, an event handler, a URL scheme or a template tag if it is not escaped. What
/// an `html` prop does with them is #868's to settle, with its sanitiser; here they go
/// only where the screen promises text, and the promise is measured under the policy.
/// The raw props get [`HOSTILE_MARKUP`] (GH #868).
const HOSTILE: [&str; 6] = [
    "'><script>window.__pwned=1</script>",
    "<img src=x onerror=window.__pwned=1>",
    "\"><svg onload=window.__pwned=1>",
    "javascript:window.__pwned=1",
    "[[\"exec\",{\"attr\":\"onclick\"}]]",
    "{{&x}}{{children}}",
];

/// Markup for the four raw props an application fills (GH #868): each one carries a
/// script, an event handler, a script URL or a frame the display's allowlist has to drop
/// before the page is drawn. Without the allowlist every one of them is a violation of
/// the policy (inline script, inline handler) or a `javascript:` navigation waiting for a
/// click; with it they leave table cells, shapes and text, and the counts stay at zero.
const HOSTILE_MARKUP: [&str; 5] = [
    "<tr><th>head</th></tr><script>window.__pwned=1</script>",
    "<tr><td><img src=x onerror=window.__pwned=1>cell</td></tr></tbody></table><script>window.__pwned=1</script>",
    "<a href=\"javascript:window.__pwned=1\">link</a><svg onload=window.__pwned=1></svg>",
    "<svg viewBox=\"0 0 10 10\" role=\"img\"><rect onclick=\"window.__pwned=1\" width=\"5\" height=\"5\"/></svg><iframe src=\"javascript:window.__pwned=1\"></iframe>",
    "<p>body</p><script>window.__pwned=1</script><a href=\"https://example.org\" onmouseover=\"window.__pwned=1\">l</a>",
];

/// One number out of the driver's `CSP …` line.
fn counter(line: &str, key: &str) -> u64 {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "browser lock: runs in the station browser:display (scripts/gate_plan.py BROWSER_LOCKS)"]
async fn a_display_runs_under_a_strict_csp() {
    if !library_ships() || !have_python() {
        println!("SKIP no template library or no python3 in this tree");
        return;
    }
    let driver = repo("workshop/tools/display-csp-browser.mjs");
    if !driver.is_file() {
        println!("SKIP the driver does not ship in this tree");
        return;
    }
    let colony = boot_with_voice_echo(Boot::default()).await;

    // One window with the hostile text in every text slot a view has: the title, a line,
    // a list with its items, a table's caption -- and hostile markup in the four raw
    // props an application fills (GH #868): a table's head and rows, a media card's and a
    // chart's figure, a document's body.
    let [a, b, c, d, e, f] = HOSTILE;
    let [head, rows, media, chart, body] = HOSTILE_MARKUP;
    colony
        .put_content(
            APP,
            "hostile",
            json!({
                "component": "display-pane", "key": "c.win",
                "props": {"pane_id": "pane-hostile", "title": a, "context": "system",
                          "relevance": "0.9", "topic": "hostile:1", "touched": "1"},
                "children": [
                    {"component": "display-text", "key": "c.line", "props": {"body": b}},
                    {"component": "display-list", "key": "c.list", "props": {"title": c},
                     "children": [
                        {"component": "display-item", "key": "c.i1", "props": {"k": d, "v": e}}
                     ]},
                    {"component": "display-table", "key": "c.table",
                     "props": {"caption": f, "head": head, "rows": rows}},
                    {"component": "display-media", "key": "c.media", "props": {"figure": media}},
                    {"component": "display-chart", "key": "c.chart", "props": {"figure": chart}},
                    {"component": "display-document", "key": "c.doc", "props": {"body": body}}
                ]
            }),
        )
        .await;

    // The phone exit: it takes `audio`, so the microphone hook binds and joins.
    let url = colony.url("phone");
    let page = colony.page("phone").await;
    assert!(
        page.contains("phx-hook=\"DisplayMic\"") && page.contains("pane-hostile"),
        "the phone exit carries the mark and the hostile window before the browser opens it"
    );

    let run = tokio::time::timeout(
        DRIVER_LIMIT,
        tokio::process::Command::new("node")
            .arg(&driver)
            .arg(&url)
            .arg(repo("templates/display/csp.json"))
            .kill_on_drop(true)
            .output(),
    )
    .await;
    let out = match run {
        Err(_) => panic!("the browser driver did not finish within {DRIVER_LIMIT:?}"),
        Ok(Err(e)) => {
            println!("SKIP node is not on this host: {e}");
            colony.shutdown().await;
            return;
        }
        Ok(Ok(out)) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    match out.status.code() {
        Some(3) => {
            println!("{}", stderr.trim());
            colony.shutdown().await;
            return;
        }
        Some(0) => {}
        other => panic!("the browser driver failed ({other:?}):\n{stdout}\n{stderr}"),
    }
    // The whole output, so the violations are on record whichever way the verdict goes.
    println!("{}", stdout.trim_end());
    let line = stdout
        .lines()
        .find(|l| l.starts_with("CSP "))
        .unwrap_or_else(|| panic!("the driver printed no counters:\n{stdout}\n{stderr}"))
        .to_string();

    assert_eq!(
        counter(&line, "connected="),
        1,
        "the LiveView container connected under the policy -- the boot ran: {stdout}"
    );
    assert_eq!(
        counter(&line, "worklet="),
        1,
        "the held mark sent frames -- the capture worklet loaded and ran: {stdout}"
    );
    assert_eq!(
        counter(&line, "pwned="),
        0,
        "the hostile text stayed text: {stdout}"
    );
    assert_eq!(
        counter(&line, "violations="),
        0,
        "and the page raised no violation of the published policy: {stdout}"
    );

    colony.shutdown().await;
}
