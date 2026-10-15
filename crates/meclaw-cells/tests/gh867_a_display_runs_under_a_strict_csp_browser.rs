//! GH #867 — a display runs in a real browser under a strict Content-Security-Policy.
//!
//! The lock beside this one (`gh867_no_surface_page_carries_unlisted_inline_script.rs`)
//! reads the markup: no inline script that `templates/display/csp.json` does not list,
//! nothing from `blob:`. This file asks an engine. A throwaway display colony with an
//! `echo` voice cell on the mount the microphone joins is loaded by headless Chromium, and
//! the driver (`workshop/tools/display-csp-browser.mjs`) plays the proxy: every document
//! answer carries `Content-Security-Policy` built from `csp.json` plus
//! `default-src 'self'; object-src 'none'` -- `script-src 'self'` and the published
//! hashes, no `'unsafe-inline'`, no `'unsafe-eval'`, no `blob:`. The page carries the code
//! view the display shows by default (`colony-view`) beside the hostile window, so its
//! client -- the third inline script -- runs under the same policy (re-check tv-bn M8, M1).
//!
//! What must hold under it: the LiveView container connects (the boot is a file now,
//! `@client/boot.js`), the microphone hook joins and, with the mark held on Chromium's
//! fake microphone, sends frames (the capture worklet is a file now,
//! `@client/display-mic-worklet.js`), the page raises no violation at all -- counted as
//! events AND as console lines, because Chromium refuses a `blob:` worklet in the console
//! only, without an event -- and hostile text written into a window's title, a line, a list and a table caption
//! stays text (`window.__pwned` stays undefined). And, since an inline script that never
//! runs raises no violation either, each of the three registers its hook (`ColonyView`,
//! `DisplayMic`, `DisplayScene`) and each is bound to its element (re-check tv-bn M9, M2).
//!
//! Station `browser:display`, not `tests` (`scripts/gate_plan.py`, `BROWSER_LOCKS`): the
//! audio drivers of this tree have tripped under parallel browser load before (GH #763),
//! so a browser lock runs where browser locks run, and is `#[ignore]`d everywhere else.
//! No node, no browser, no template library or no python is a `SKIP`, never a red (R2b)
//! -- outside the gate. Inside it the runner sets `MECLAW_REQUIRE_BROWSERS=1` on a
//! station that books the engine (GH #1161), and a browser that is absent or does not
//! start is `NO-ENGINE ...`, exit 1, red.
//!
//! Every wait here and in the driver waits for a state or an event; time is only a HANG
//! mark that names what it waited for (AGENTS.md rule 11, welle-val2 v2-kit #1199). Until
//! then the page was read once and judged, the driver ran under a 90 s timeout, and the
//! driver gave the connect and the `hello` ten seconds each and held the mark for one: the
//! release gate of 0.64.0 read a page under load before it carried the hostile window.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::collections::BTreeSet;

use display_colony::{
    Boot, COLONY_VIEW, Driver, Site, boot_with_voice_echo, colony_view_emission, have_python,
    library_ships, repo,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::deadline::{HANG, scaled};

/// The app whose window carries the hostile text.
const APP: &str = "/alex/apps/probe";

/// The driver's run mark, in HANG marks: two of its waits can run into theirs in one run
/// and the run still reports itself (a `hello` that never comes skips the hold, frames
/// that never come skip the release), plus one for everything else. Outside it, one HANG
/// mark for node's start and the report: the browser's launch is inside the driver's own
/// mark. Both together stay under the nextest leash of this binary (default profile,
/// 8 x 30 s) with the colony's boot in front.
const RUN_HANGS: u32 = 3;

/// What the phone exit has to carry before the browser opens it: the mark, the hostile
/// window and the code view with its client.
const PAGE_CARRIES: [&str; 3] = [
    "phx-hook=\"DisplayMic\"",
    "pane-hostile",
    "phx-hook=\"ColonyView\"",
];

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

    // The code view the screen shows by default, as its layout emits it.
    colony
        .put_emission(COLONY_VIEW, &colony_view_emission())
        .await;

    // The phone exit: it takes `audio`, so the microphone hook binds and joins. The page is
    // a state the colony reaches, not a reading taken once: waited for until it carries the
    // mark, the hostile window and the code view with its client, and the HANG names what
    // the last page lacked.
    let page_lacks = async || {
        let page = colony.page("phone").await;
        PAGE_CARRIES
            .into_iter()
            .filter(|want| !page.contains(want))
            .collect::<Vec<_>>()
    };
    meclaw_testing::wait::until_some_with(
        "the phone exit carries the mark, the hostile window and the code view with its \
         client before the browser opens it",
        || async { page_lacks().await.is_empty().then_some(()) },
        || async { format!("the last page lacked {:?}", page_lacks().await) },
    )
    .await;

    let with = Driver::default()
        .script("workshop/tools/display-csp-browser.mjs")
        .arg(repo("templates/display/csp.json"));
    let driving = colony.drive(Site::Gh867CspPhone, "phone", with);
    // Under the Rust marker over the whole child (`browser_run`): the driver gets the run's
    // HANG mark and names the wait it hung in; a child past the mark plus what runs outside
    // it is ended with its browser and reported as `HANG:` with its stderr tail.
    let run = driving
        .run_marked(
            "the CSP driver on the phone",
            scaled(HANG) * RUN_HANGS,
            scaled(HANG),
        )
        .await
        .unwrap_or_else(|hung| panic!("{hung}"));
    let out = match run {
        Err(e) => {
            println!("SKIP node is not on this host: {e}");
            colony.shutdown().await;
            return;
        }
        Ok(out) => out,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    // TV-B7 (GH #1210, re-check tv-bn-drv2-fix1 § 5): a braked driver says so on
    // stderr; printed here, or a braked probe and an unbraked one read the same.
    for brake in stderr.lines().filter(|l| l.starts_with("THROTTLE ")) {
        println!("{brake}");
    }
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
    // A wait the driver ran out of is no green run: past the mark for the release frame
    // of the take, the violation count closed before the take was over, so a violation
    // the take still raises is not in it (re-check v2-kit c1 M1). Every other wait moves
    // a counter below; this one only says `HANG:`.
    let hung: Vec<&str> = stdout.lines().filter(|l| l.starts_with("HANG:")).collect();
    assert!(
        hung.is_empty(),
        "the driver ran out of a wait, the counters are no verdict: {hung:?}\n{stdout}"
    );
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

    // No violation is no proof that an inline script ran: one that is never executed
    // raises none either (re-check tv-bn M9, M2). So the positive half, per script: each
    // of the page's three inline scripts registered its hook, and each hook is bound to its
    // element (`mounted` returned) -- as sets, the order the page registers them in is not
    // the claim (TV-Q2).
    let hooks = stdout
        .lines()
        .find_map(|l| l.strip_prefix("HOOKS "))
        .unwrap_or_else(|| panic!("the driver printed no HOOKS line:\n{stdout}"));
    let hooks: Value = meclaw_core::serde_json::from_str(hooks)
        .unwrap_or_else(|e| panic!("the HOOKS line is no JSON ({e}): {hooks}"));
    let registered: BTreeSet<&str> = hooks["registered"]
        .as_array()
        .expect("registered")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mounted: BTreeSet<&str> = hooks["mounted"]
        .as_object()
        .expect("mounted")
        .iter()
        .filter(|(_, n)| n.as_u64().unwrap_or(0) >= 1)
        .map(|(k, _)| k.as_str())
        .collect();
    let want: BTreeSet<&str> = ["ColonyView", "DisplayMic", "DisplayScene"].into();
    assert!(
        want.is_subset(&registered),
        "the page registered the hook of each of its inline scripts: want {want:?}, \
         registered {registered:?} -- a script that never ran raises no violation: {stdout}"
    );
    assert!(
        want.is_subset(&mounted),
        "and each hook is bound to its element (`mounted` returned): want {want:?}, \
         mounted {mounted:?}: {stdout}"
    );

    colony.shutdown().await;
}
