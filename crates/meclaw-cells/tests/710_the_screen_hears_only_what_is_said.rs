//! The web seam: what the screen hears, and what it never hears (befund 04 § E.1).
//!
//! The two R-scenarios of the description live here, and they are R because they say
//! "nothing happens": **Q-07** -- a press on the OS mark is no event, the pass sees
//! nothing -- and **S-062** -- a hold on an output without `audio`, a press on the tv,
//! and the switch: no event reaches the pass. The absence of a message cannot be measured
//! where `compose.py` runs alone, because a message that is never made is only missing
//! where messages are made. So this file boots a colony, serves the real page, opens the
//! page's OWN LiveView socket and counts the colony's `message_log`.
//!
//! Beside them ride the K-scenarios of the same seam as anchors: **Q-11** and **Q-17**
//! (the door takes the profiles, `inputs` gates what is bound, a `screens` map without
//! `phone` passes when `default_screen` names an entry), **S-079** (`dock_default` and
//! `dock_max` per exit), **Q-14** and **S-059** (every output shows the same windows with
//! the same rung and the same level -- the state has ONE value of each), **S-053** (a tap
//! for a window the state does not hold is absorbed).
//!
//! One colony, several cases (befund 04 § E.1).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, boot, have_python, library_ships, next_text, push};
use meclaw_core::serde_json::{Value, json};

/// The app whose window stands on the screen for the whole file.
const APP: &str = "/alex/apps/note";

/// How long the colony must stay quiet before a count is taken. A settle window, never a
/// semantic discriminator: what is asserted afterwards is the same at 5 ms or 500.
const QUIET: Duration = Duration::from_millis(400);

/// The props of one rendered root, out of the last patch bundle that named it.
fn root_props(patches: &[Vec<Value>], prefix: &str) -> Value {
    let id = format!("{prefix}display.root");
    for bundle in patches.iter().rev() {
        for call in bundle.iter().rev() {
            if call["id"] == id.as_str()
                && (call["op"] == "object.create" || call["op"] == "object.update")
            {
                return call["props"].clone();
            }
        }
    }
    panic!("no patch ever wrote {id}");
}

/// The props of the window `oid` as one output drew it.
fn window_props(patches: &[Vec<Value>], prefix: &str, oid: &str) -> Value {
    let id = format!("{prefix}{oid}/c.win");
    for bundle in patches.iter().rev() {
        for call in bundle.iter().rev() {
            if call["id"] == id.as_str()
                && (call["op"] == "object.create" || call["op"] == "object.update")
            {
                return call["props"].clone();
            }
        }
    }
    panic!("no patch ever wrote {id}");
}

/// Every route the passes published, sorted and deduplicated.
fn routes(patches: &[Vec<Value>]) -> Vec<String> {
    let mut out: Vec<String> = patches
        .iter()
        .flatten()
        .filter(|c| c["op"] == "page.set")
        .filter_map(|c| c["route"].as_str().map(str::to_string))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_screen_hears_only_what_is_said() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // A long linger and a long fade: this file is about the web seam, and a screen whose
    // clock strikes twice a second while a page is being read would put the clock's
    // messages into every count below. The clock's own seam is the file beside this one.
    let mut colony = boot(Boot {
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
    let oid = colony.oid(APP, "note");

    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "system", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    colony.settle(QUIET).await;

    // ── Q-17, § 6.5: the switch and three outputs, and nothing else ───────────────────
    let patches = colony.patches().await;
    assert_eq!(
        routes(&patches),
        vec!["/", "/monitor", "/phone", "/tv"],
        "three outputs and the switch at the mount root"
    );
    for screen in ["", "monitor", "phone", "tv"] {
        assert_eq!(
            colony.status(screen).await,
            200,
            "/{}/{screen} is served",
            display_colony::MOUNT
        );
    }
    assert_eq!(
        colony.status("nosuch").await,
        404,
        "a name without an entry in `screens` is a 404, and no switch (§ 6.5)"
    );
    assert_eq!(
        root_props(&patches, "")["switch"],
        "1",
        "the mount root is the switch and only it"
    );
    for prefix in ["monitor.", "phone.", "tv."] {
        assert_eq!(
            root_props(&patches, prefix)["switch"],
            "",
            "{prefix} is an output, not the switch"
        );
    }

    // ── Q-11, S-079, § 6.1/§ 6.4: the profile is the door, and it gates the bindings ──
    let tv = root_props(&patches, "tv.");
    let monitor = root_props(&patches, "monitor.");
    let phone = root_props(&patches, "phone.");
    assert_eq!(
        (tv["exit"].clone(), tv["inputs"].clone(), tv["tap"].clone()),
        (json!("tv"), json!(""), json!(false)),
        "a television is an output device: `inputs: []`, so nothing is bound (Q-11, § 6.4)"
    );
    assert_eq!(tv["input_line"], json!(false), "and no input line either");
    assert_eq!(
        (tv["dock"].clone(), tv["dock_max"].clone()),
        (json!("shown"), json!(7)),
        "the tv's own dials (S-079)"
    );
    assert_eq!(
        (
            monitor["inputs"].clone(),
            monitor["tap"].clone(),
            monitor["input_line"].clone()
        ),
        (json!("pointer keyboard"), json!(true), json!(true)),
        "a monitor has a finger and a keyboard (Q-11, § 7.3)"
    );
    assert_eq!(
        (monitor["dock"].clone(), monitor["dock_max"].clone()),
        (json!("shown"), json!(8))
    );
    assert_eq!(
        (
            phone["inputs"].clone(),
            phone["dock"].clone(),
            phone["dock_max"].clone()
        ),
        (json!("audio touch"), json!("hidden"), json!(5)),
        "the phone ships its dock closed and carries the ear (S-079, § 6.4)"
    );

    // ── Q-14, S-059, § 6.2: one state, many exits ────────────────────────────────────
    let here = window_props(&patches, "", &oid);
    for prefix in ["monitor.", "phone.", "tv."] {
        let there = window_props(&patches, prefix, &oid);
        for key in ["rung", "level", "score", "since", "age", "layer"] {
            assert_eq!(
                there[key], here[key],
                "{key} is systemwide: {prefix} disagrees with the switch's rendering \
                 ({there} vs {here})"
            );
        }
    }

    // ── Q-07, § 5.5: a press on the mark is no event ─────────────────────────────────
    // The mark is a plain button. It carries NO `phx-click` and no `phx-value-*`: there is
    // no wire a press could travel on, and the hold-versus-press decision is the client's
    // own (a token, 250 ms). What the page does bind is the hook, and the hook pushes
    // `hold` -- which is why the contrast below means something.
    let page = colony.page("monitor").await;
    let at = page
        .find("class=\"display-os-mark\"")
        .expect("the monitor page carries the OS mark");
    let open = page[..at].rfind('<').expect("the button tag opens");
    let mark_tag = &page[open..open + page[open..].find('>').expect("the button tag closes")];
    assert!(
        !mark_tag.contains("phx-click") && !mark_tag.contains("phx-value"),
        "a press has no wire at all: {mark_tag}"
    );
    assert!(
        page.contains("pushEvent(\"hold\", {})"),
        "and the hook pushes the hold under its own name, with nothing in it (S-088)"
    );

    // The page's own socket, open and idle. An event is the ONE place a gesture becomes a
    // message (§ 4.1), so what is counted is what reached the pass -- not the whole log,
    // which the screen's own clock also writes to.
    let mut ws = colony.socket("monitor").await;
    colony.settle(QUIET).await;
    assert!(
        colony.events_into_pass().await.is_empty(),
        "an open page that nobody touches sends nothing: {:?}",
        colony.events_into_pass().await
    );

    // The same socket, the gesture that DOES have a wire: one hold, one event.
    push(&mut ws, "hold", json!({})).await;
    let _ = next_text(&mut ws).await; // the reply to the push
    colony.settle(QUIET).await;
    let seen = colony.events_into_pass().await;
    assert_eq!(
        seen,
        vec![("hold".to_string(), json!({}))],
        "a hold is an event and reaches the pass -- and it carries NOTHING (S-088)"
    );

    // ── S-062, § 6.4: the three gates of the client ──────────────────────────────────
    // The tv: nothing is bound, so neither a press, nor a hold, nor a tap has a wire.
    let tv_page = colony.page("tv").await;
    assert!(
        tv_page.contains("data-inputs=\"\""),
        "the tv output says it takes nothing (§ 6.4)"
    );
    assert!(
        !tv_page.contains("phx-click=\"tap\""),
        "and no tile on it shouts a name nobody opens"
    );
    // The monitor: a finger, but no ear. `hold_bound` is `tap_bound && audio` (§ 6.4),
    // and the client reads that gate off the very attribute the profile wrote, so a
    // monitor's mark records nothing and sends no `hold` -- whatever a finger does on it.
    assert!(
        page.contains("data-inputs=\"pointer keyboard\""),
        "the monitor has a finger and a keyboard and no ear"
    );
    assert!(
        page.contains("var audio = inputs.indexOf(\"audio\") > -1;"),
        "and the gate is read off that attribute, in the shipped client (§ 6.4)"
    );
    assert!(
        page.contains("phx-click=\"tap\""),
        "and its tiles do take the finger (Q-11)"
    );
    // The switch: it leads somewhere, so nothing on it is ever touched. The server draws
    // it out of `default_screen` and marks it, and the shipped client returns early on
    // that mark -- no listener, no socket, no voice channel for the half second before
    // `location.replace` (§ 6.5).
    let switch = colony.page("").await;
    assert!(
        switch.contains("data-switch=\"1\""),
        "the mount root says it is the switch (§ 6.5)"
    );
    assert!(
        page.contains("data-switch=\"\""),
        "and an explicit output says it is not"
    );
    assert!(
        switch.contains("getAttribute(\"data-switch\") === \"1\""),
        "the client asks the same question before it binds anything (§ 6.5)"
    );

    // ── S-053, § 6.12: a tap for a window the state does not hold is absorbed ────────
    push(&mut ws, "tap", json!({"for": "view.nobody.nothing"})).await;
    let _ = next_text(&mut ws).await;
    colony.settle(QUIET).await;
    assert_eq!(
        colony.events_into_pass().await.len(),
        2,
        "the tap did reach the hive -- what follows is about what it did NOT do"
    );
    let left = drain_egress(&mut colony).await;
    assert!(
        left.is_empty(),
        "a tap the state cannot place is absorbed by step 3, never sent out as an \
         application event (S-053): {left:?}"
    );

    // ── § 4.16, Decision 16.09.: `judge: off` means the hive has no judge to ask ─────
    // This colony ships the shipped default. Windows arrived, rungs changed, strokes
    // struck -- and not one question left for the judge cell.
    assert_eq!(
        colony.judge_questions().await,
        0,
        "with `judge: off` no verdict can arrive, so nothing is asked"
    );

    colony.shutdown().await;
}

/// Every `event` the screen sent out of the hive, drained.
async fn drain_egress(colony: &mut display_colony::Colony) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(m) = colony.egress.try_recv() {
        if m.headers.hop.get("route").and_then(Value::as_str) == Some("event") {
            out.push(json!({"owner": m.headers.hop.get("owner")}));
        }
    }
    out
}
