//! A restart reads once and draws the same screen (GH #809, acceptance 4).
//!
//! Since display 2.7.0 the curator's state is the memory of a `resident` cell, and memory
//! is a cache (`docs/cell-types.md` § code): a killed child is replaced by the pool with
//! the next message, and the new one rebuilds the screen out of the store's rows, the ONE
//! rest row and the tree `web` holds -- one select, one read. What it rebuilds has to be
//! the screen that stood, or a restart is something a person sees.
//!
//! Two restarts, because the first message after the kill decides the path through the
//! boot: a STROKE of the clock (the tick restart), and an app's WRITE on a window that
//! already stands (the review of D1 part a found that this one lost the curator's memory
//! of that window, `REBUILD-WRITE` in the scenario runner holds it for every scenario).
//!
//! **The comparison leaves out `age` and `due`, and only those** (OR-D19): `age` is
//! `fresh` only in the pass in which a window appeared, and the rebuild cannot know that
//! pass (`reconcile`); `due` names the clock's order, which the new cell places anew. Every
//! other prop of every object is compared.
//!
//! Measured where it lands: the child is killed through `/proc` (only children of this
//! test process, never by name), the `read` hops at `web` are counted, the tree is the
//! patches to `web` folded, and the order of the boot select and the write is read in the
//! store's log.

#[path = "support/display_colony.rs"]
mod display_colony;

use display_colony::{
    Boot, Colony, Driver, MESSAGES_PAGE, Named, Site, attr, boot, calls_of, have_python,
    library_ships, next_page, present, push, request_of,
};
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/note";

/// The tree with `age` and `due` taken out of every object's props (OR-D19).
fn comparable(tree: &Value) -> Value {
    let mut out = tree.clone();
    if let Some(map) = out.as_object_mut() {
        for obj in map.values_mut() {
            if let Some(props) = obj["props"].as_object_mut() {
                props.remove("age");
                props.remove("due");
            }
        }
    }
    out
}

/// The objects whose comparable form differs, by id -- what a red run prints.
fn differences(a: &Value, b: &Value) -> Vec<String> {
    let (a, b) = (comparable(a), comparable(b));
    let empty = meclaw_core::serde_json::Map::new();
    let (am, bm) = (
        a.as_object().unwrap_or(&empty),
        b.as_object().unwrap_or(&empty),
    );
    let mut ids: Vec<&String> = am.keys().chain(bm.keys()).collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .filter(|id| am.get(*id) != bm.get(*id))
        .map(|id| {
            format!(
                "{id}: before {} / after {}",
                am.get(id)
                    .map(|o| o["props"].to_string())
                    .unwrap_or_default(),
                bm.get(id)
                    .map(|o| o["props"].to_string())
                    .unwrap_or_default()
            )
        })
        .collect()
}

/// Three windows and a tap, so the rebuild has a touch, a put-away and a led window to
/// get right.
async fn the_stage() -> Colony {
    let colony = boot(Boot {
        // Long, so the only strokes are the fresh seconds this test provokes.
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
    for (view, relevance) in [("a", "0.9"), ("b", "0.8"), ("c", "0.7")] {
        colony
            .put(
                APP,
                view,
                json!({"title": view, "context": "work", "relevance": relevance,
                       "topic": format!("{view}:1"), "touched": "1"}),
            )
            .await;
    }
    let a = colony.oid(APP, "a");
    let acted = attr(&colony.tree().await, &a, "acted");
    let mut ws = colony.socket("monitor").await;
    push(&mut ws, "tap", json!({"for": a})).await;
    colony
        .wait_tree("the tap reaches a pass that draws it", |t| {
            attr(t, &a, "acted") != acted
        })
        .await;
    colony.settle().await;
    colony
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_by_the_clock_reads_once_and_draws_the_same_screen() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = the_stage().await;
    assert_eq!(colony.reads().await, 1, "one read so far, the first boot's");

    // A fresh window orders its fresh second (§ 4.34): the stroke one second after its
    // pass is the first message the killed cell gets. The kill has to land inside that
    // second, so the write is followed by a tight watch on `web`, not by a settle -- and a
    // run in which something else woke the killed cell gets a new window and a new second
    // (the precondition is retried, never the assertion).
    //
    // TV-F2 (tv-bn M7b10, 8/10 red at CPUQuota 80): the precondition is read off the boot,
    // not off a count of strokes. The boot's read answers the message that woke the cell
    // (its parent): a stroke is the clock's restart; a late reply of the pass before the
    // kill (`views`/`web` -> compose) is not, and is retried. A stroke count told neither a
    // stroke after the kill (struck while the barrier waited) from one before it, nor a
    // boot that had already happened when the reads were counted after the check.
    let mut attempt = 0;
    let (before, reads) = loop {
        attempt += 1;
        assert!(
            attempt <= 3,
            "three fresh seconds: no stroke was the first message after the kill"
        );
        let view = format!("fresh-{attempt}");
        let oid = colony.oid(APP, &view);
        // At rest: the live curator reads only on a boot, so these are the reads before
        // the kill.
        let reads = colony.reads().await;
        let strikes = colony.strikes().await.len();
        let props = json!({"title": "Fresh", "context": "work", "relevance": "0.6",
                           "topic": format!("{view}:1"), "touched": "1"});
        colony.write_view(APP, &view, props.clone()).await;
        // The kit's 10 ms poll is the tight watch; its hang mark and `HANG:` replace the
        // unscaled 30 s of this loop (re-check tv-bn M5 M1). `watch_tree`, not `wait_tree`:
        // the latter ends on a settle, and the stroke came first in 4 of 5 runs at
        // CPUQuota 80 (tv-bn M7b6 F7).
        colony
            .watch_tree(&format!("the fresh window {oid} is drawn"), |t| {
                present(t, &oid)
            })
            .await;
        assert_eq!(
            colony.kill_compose().await,
            1,
            "one resident child, the curator"
        );
        // The fresh second strikes at the latest one second after the pass: a boot follows
        // the kill, or the stroke met the live cell before it.
        colony
            .wait_until(
                "a message reaches the killed cell, or the stroke came first",
                async || colony.reads().await > reads || colony.strikes().await.len() > strikes,
            )
            .await;
        // A stroke seen before its boot's read was logged: the barrier lets the read land.
        colony.settle().await;
        if colony.reads().await == reads {
            eprintln!("809 restart: the fresh second struck before the kill, attempt {attempt}");
            // Re-check tv-bn M7b11 M7: the stroke met the live cell, so nothing woke the
            // killed one, and the next attempt's own write would boot it before that
            // attempt's kill -- a read the next attempt counts as its restart, lost
            // whatever the race. So the killed cell is woken here, by the product's way (the
            // same window written again), and its boot lands in the next attempt's count.
            // Fail closed, read off the boot as above: one read more, and a select of a boot
            // above it.
            colony.write_view(APP, &view, props).await;
            colony
                .wait_until("the rewrite wakes the killed cell", async || {
                    colony.reads().await > reads
                })
                .await;
            colony.settle().await;
            let woke = colony.boot_read(reads + 1).await;
            assert!(
                colony.reads().await == reads + 1
                    && woke
                        .as_ref()
                        .is_some_and(|(from, _, _)| !from.starts_with("<no select")),
                "precondition: the rewrite woke the killed cell, one boot; got reads {} \
                 (before {reads}), boot {:?}",
                colony.reads().await,
                woke.map(|(from, route, _)| (from, route))
            );
            continue;
        }
        let (from, route, stood) = colony
            .boot_read(reads + 1)
            .await
            .expect("the boot's read is in the log");
        if route == "in_tick" {
            // The screen that stood: the patches the killed curator sent, up to the read of
            // the new one.
            break (stood, reads);
        }
        eprintln!(
            "809 restart: the killed cell woke on {from} route {route:?}, not on the stroke, attempt {attempt}"
        );
    };
    // TV-F2 (tv-bn M7b8): the account of the run -- what the clock was told and what it
    // struck.
    eprintln!(
        "809 restart: booted on the stroke with reads {reads}, orders {:?}, strikes {:?}",
        colony.orders().await,
        colony.strikes().await
    );

    colony.settle().await;
    let after = colony.tree().await;
    let read_now = colony.reads().await;
    eprintln!("809 restart (clock): reads {read_now}, attempts {attempt}");
    assert_eq!(
        read_now,
        reads + 1,
        "the new cell read the tree ONCE (its boot)"
    );
    let diff = differences(&before, &after);
    assert!(
        diff.is_empty(),
        "the rebuilt screen differs from the one that stood (age and due aside, OR-D19):\n{}",
        diff.join("\n")
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_by_an_apps_write_reads_once_and_draws_the_same_screen() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = the_stage().await;
    // Let every fresh second strike first: the write below must be the first message.
    let settled = colony
        .wait_tree("every window has settled", |t| {
            ["a", "b", "c"]
                .iter()
                .all(|v| attr(t, &colony.oid(APP, v), "age") == json!("settled"))
        })
        .await;
    colony.settle().await;
    let before = colony.tree().await;
    assert_eq!(
        differences(&settled, &before),
        Vec::<String>::new(),
        "the stage stands still before the kill"
    );
    let reads = colony.reads().await;

    assert_eq!(
        colony.kill_compose().await,
        1,
        "one resident child, the curator"
    );
    // The same write again, on the window that stands: the app says what it said. The
    // curator's memory of `b` (its touch, its place) has to come back out of the rest row.
    // TV-A2: the baseline of an absence claim is read after quiescence (re-check tv-bn M7b3 H1).
    colony.settle().await;
    let writes = colony.writes_of(APP, "b").await;
    colony
        .write_view(
            APP,
            "b",
            json!({"title": "b", "context": "work", "relevance": "0.8",
                   "topic": "b:1", "touched": "1"}),
        )
        .await;
    colony
        .wait_until("the write wakes the killed cell and it boots", async || {
            colony.reads().await > reads && colony.writes_of(APP, "b").await > writes
        })
        .await;
    colony.settle().await;
    let after = colony.tree().await;

    assert_eq!(
        colony.reads().await,
        reads + 1,
        "the new cell read the tree ONCE (its boot)"
    );
    // The boot select went out BEFORE the write's bundle (review of D1 part a, C1): read
    // in the store's log, the select that follows the kill stands ahead of the write.
    let views = colony.to_child("views").await;
    let last_select = views.iter().rposition(|row| {
        calls_of(row).is_some_and(|calls| calls.iter().any(|c| c["operation"] == "select"))
    });
    let last_write = views.iter().rposition(|row| {
        let mark = request_of(row);
        mark["write"]["owner"] == APP && mark["write"]["view_id"] == "b"
    });
    assert!(
        last_select.is_some() && last_select < last_write,
        "the boot read the rows as they stood before the write: select at {last_select:?}, \
         write at {last_write:?}"
    );
    let diff = differences(&before, &after);
    assert!(
        diff.is_empty(),
        "the rebuilt screen differs from the one that stood (age and due aside, OR-D19):\n{}",
        diff.join("\n")
    );

    colony.shutdown().await;
}

/// Re-check tv-bn M3 M2: a `/proc` read error is no ended process. `kill_compose`
/// read `stat` with `unwrap_or_default`, so an error other than "gone" gave `""`, no
/// state, and counted as "died"; the next message then met a dying child. Gone
/// (NotFound, ESRCH) is ended; any other error and a stat without a state are red.
#[test]
fn a_proc_read_error_is_no_ended_process() {
    use std::io::{Error, ErrorKind};
    let path = "/proc/4242/stat";
    assert!(display_colony::proc_stat_dead(
        path,
        Err(Error::from(ErrorKind::NotFound))
    ));
    assert!(display_colony::proc_stat_dead(
        path,
        Err(Error::from_raw_os_error(libc::ESRCH))
    ));
    assert!(display_colony::proc_stat_dead(
        path,
        Ok("4242 (python3) Z 1 2".into())
    ));
    assert!(!display_colony::proc_stat_dead(
        path,
        Ok("4242 (py) thon) S 1 2".into())
    ));
    for (what, read) in [
        ("EACCES", Err(Error::from(ErrorKind::PermissionDenied))),
        ("EIO", Err(Error::from_raw_os_error(libc::EIO))),
        ("no state", Ok(String::new())),
    ] {
        let red = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            display_colony::proc_stat_dead(path, read)
        }));
        let msg = red
            .err()
            .and_then(|p| {
                p.downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            })
            .unwrap_or_else(|| panic!("{what}: a /proc read error counted as an answer"));
        assert!(msg.contains(path), "{what}: the red names the path: {msg}");
    }
    assert_eq!(
        display_colony::proc_read(
            "/proc/4242/cmdline",
            Err::<Vec<u8>, _>(Error::from(ErrorKind::NotFound))
        ),
        None
    );
}

/// The needles of [`no_user_of_the_display_fixture_keeps_a_deadline_of_its_own`], split so
/// the pin does not find itself: a deadline or a sleep of the test's own (TV-S3, TV-Z3).
const OWN_DEADLINE: [&str; 5] = [
    concat!("Instant", "::now"),
    concat!(".elapsed", "()"),
    concat!("time::", "timeout("),
    concat!("wait::", "within("),
    concat!("sleep", "("),
];

/// The places the needles may stand, each with its reason: the line and the line after.
const OWN_DEADLINE_ALLOWED: [(&str, &str, &str); 3] = [
    // The API stand-in answers a browser with `503` at the scaled hang mark; a handler
    // does not panic, and the browser's wait names the hang.
    (
        "support/display_colony.rs",
        concat!(
            "let Ok(Ok(reply)) = tokio::time::",
            "timeout(hang_mark(), ack_rx).await else {"
        ),
        "",
    ),
    // Re-check tv-bn M7b5 B-M2: the fixture's own quiescence barrier (`wire::Wire::settle`
    // around `sealed::barrier::settle`, the one mint of the proof since M7b13).
    // Its mark is the scaled hang mark; it keeps its own deadline instead of
    // `deadline::hang_with` because that one's last state is a SYNC closure, and the
    // colony's last state (dead letters, newest log rows) is read by asking the colony,
    // asynchronously, after the mark.
    (
        "support/display_colony.rs",
        concat!(
            "match tokio::time::",
            "timeout(hang_mark(), barrier::settle(&self.h)).await {"
        ),
        "",
    ),
    // Re-check tv-bn M7 H7: the run marker of a driver CHILD PROCESS (gh961, gh1028,
    // gh867_a_display_runs_under_a_strict_csp_browser), not a polled condition; its
    // scaling belongs to the driver class. Since M7b13 the child is the fixture's
    // (`Colony::drive` -> `Driving`), the marker the same line.
    (
        "",
        concat!(
            "let run = tokio::time::",
            "timeout(DRIVER_LIMIT, driving.output()).await;"
        ),
        "",
    ),
];

/// Every line of `src` that keeps a deadline of its own: a needle of [`OWN_DEADLINE`], or
/// `MARKER` as a word (the unscaled 30 s the fixture exported); comments do not count.
fn own_deadlines(name: &str, src: &str) -> Vec<String> {
    // A needle that starts or ends in a word character must not continue a longer word
    // there (`MARKER_EVENT`, `nosleep(` are no hits).
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let word = |line: &str, w: &str| {
        line.match_indices(w).any(|(i, _)| {
            let before = line[..i].chars().next_back();
            let after = line[i + w.len()..].chars().next();
            let open_start = !w.starts_with(ident) || before.is_none_or(|c| !ident(c));
            let open_end = !w.ends_with(ident) || after.is_none_or(|c| !ident(c));
            open_start && open_end
        })
    };
    let lines: Vec<&str> = src.lines().map(str::trim).collect();
    let mut hits = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("//") {
            continue;
        }
        let next = lines.get(i + 1).copied().unwrap_or("");
        let hit = OWN_DEADLINE.iter().any(|n| word(line, n)) || word(line, concat!("MARK", "ER"));
        let allowed = OWN_DEADLINE_ALLOWED.iter().any(|(file, l, n)| {
            (file.is_empty() || name == *file) && line == l && (n.is_empty() || next == *n)
        });
        if hit && !allowed {
            hits.push(format!("{name}:{}: {line}", i + 1));
        }
    }
    hits
}

/// Re-check tv-bn M5 M1 / M7 M1 (TV-S3, TV-Z3): no user of the display fixture, and not the
/// fixture itself, keeps a deadline of its own. Polls on a std `Instant` plus the unscaled
/// `MARKER` or a literal 30 s failed without `HANG:` ("did not hold within 30s"), so a slow
/// lane read as a red test and `MECLAW_TEST_TIME_FACTOR` moved none of them. Every wait goes
/// through `wait::until*` / `deadline::hang*`. Read at run time, so a NEW user of the
/// fixture is under the pin the day it is written.
/// The two fixtures and every test file that builds on them, as (path under `tests/`,
/// source): a user names the fixture as a module (`mod display_colony`) or builds on it
/// (`crate::display_colony`, `use crate::display_colony::presenter_colony;`).
/// Read at run time, so a NEW user is under the pins the day it is written.
fn fixture_users() -> Vec<(String, String)> {
    let tests = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut stack = vec![tests.clone()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the tests directory reads") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    let mut users = Vec::new();
    for path in files {
        let src = std::fs::read_to_string(&path).expect("a test file reads");
        let name = path
            .strip_prefix(&tests)
            .expect("under tests")
            .to_string_lossy()
            .into_owned();
        // The presenter fixture is a CHILD module of the display fixture since M7b7 and
        // names it as `super::`, so it is a user by name, not by a path in its source.
        if FIXTURES.contains(&name.as_str())
            || src.contains("mod display_colony")
            || src.contains("crate::display_colony")
        {
            users.push((name, src));
        }
    }
    users
}

/// Re-check tv-bn M5 M1 / M7 M1 (TV-S3, TV-Z3): no user of the display fixture, and not the
/// fixture itself, keeps a deadline of its own. Polls on a std `Instant` plus the unscaled
/// `MARKER` or a literal 30 s failed without `HANG:` ("did not hold within 30s"), so a slow
/// lane read as a red test and `MECLAW_TEST_TIME_FACTOR` moved none of them. Every wait goes
/// through `wait::until*` / `deadline::hang*`.
#[test]
fn no_user_of_the_display_fixture_keeps_a_deadline_of_its_own() {
    let found = fixture_users();
    let users = found.len();
    let hits: Vec<String> = found
        .iter()
        .flat_map(|(name, src)| own_deadlines(name, src))
        .collect();
    assert!(users > 30, "the fixture and its users are found: {users}");
    assert!(
        hits.is_empty(),
        "a wait with a deadline of its own, past meclaw_testing::wait:\n{}",
        hits.join("\n")
    );
}

/// The pin's needles are sharp: every form re-check tv-bn M7 named as slipping through the
/// M5 pin is found, and a comment or a longer word is not.
#[test]
fn the_deadline_pin_finds_every_form_it_names() {
    let caught = [
        concat!(
            "let deadline = std::time::Instant",
            "::now() + Duration::from_secs(30);"
        ),
        concat!(
            "tokio::time::",
            "timeout(Duration::from_secs(30), rx).await"
        ),
        concat!("tokio::time::", "timeout("),
        // the driver marker's allowance is the one line, not its prefix
        concat!(
            "let run = tokio::time::",
            "timeout(Duration::from_secs(600), driving.output()).await;"
        ),
        concat!("MARK", "ER,"),
        concat!("tokio::time::sle", "ep(BRAKE).await;"),
        concat!("sleep", "(QUIET).await;"),
        concat!("let took = started.elapsed", "();"),
        concat!(
            "meclaw_testing::wait::",
            "within(Duration::from_secs(3), f)"
        ),
    ];
    for line in caught {
        assert_eq!(own_deadlines("x.rs", line).len(), 1, "caught: {line}");
    }
    let clean = [
        concat!("// a comment may name MARK", "ER and sle", "ep("),
        "const MARKER_EVENT: &str = \"tap-probe-marker\";",
        "meclaw_testing::wait::within_hang(f).await",
        "colony.settle().await;",
    ];
    for line in clean {
        assert_eq!(
            own_deadlines("x.rs", line),
            Vec::<String>::new(),
            "clean: {line}"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The source as code: what the channel and barrier pins read
// ─────────────────────────────────────────────────────────────────────────────

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `src` with every string and char literal emptied (`""`, `' '`) and every comment
/// removed, line breaks kept. Re-check tv-bn M7b3 H2: a `)` in a message or a note at the
/// end of a line ended a wait region early, and a needle quoted in a message read as code.
fn code_only(src: &str) -> String {
    scrub(src, false)
}

/// `src` with every comment removed and its literals KEPT, line breaks kept: where a needle
/// is a path (`colony.db`, `/colony/messages`), the literal is the read (re-check tv-bn
/// M7b9 A-M1 c / M2), and a comment still says nothing.
fn code_and_literals(src: &str) -> String {
    scrub(src, true)
}

/// [`code_only`] and [`code_and_literals`]: comments out, literals emptied or kept.
fn scrub(src: &str, keep_literals: bool) -> String {
    let c: Vec<char> = src.chars().collect();
    let at = |i: usize| c.get(i).copied();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '/' && at(i + 1) == Some('/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c[i] == '/' && at(i + 1) == Some('*') {
            i += 2;
            while i < c.len() && !(c[i] == '*' && at(i + 1) == Some('/')) {
                if c[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2;
            continue;
        }
        let word_start = i == 0
            || !ident_char(c[i - 1])
            || (c[i - 1] == 'b' && (i < 2 || !ident_char(c[i - 2])));
        if c[i] == 'r' && word_start {
            let mut j = i + 1;
            let mut hashes = 0;
            while at(j) == Some('#') {
                hashes += 1;
                j += 1;
            }
            if at(j) == Some('"') {
                j += 1;
                while j < c.len() {
                    if c[j] == '"' && (0..hashes).all(|k| at(j + 1 + k) == Some('#')) {
                        j += 1 + hashes;
                        break;
                    }
                    if c[j] == '\n' && !keep_literals {
                        out.push('\n');
                    }
                    j += 1;
                }
                if keep_literals {
                    out.extend(&c[i..j.min(c.len())]);
                } else {
                    out.push_str("\"\"");
                }
                i = j;
                continue;
            }
        }
        if c[i] == '"' {
            let mut j = i + 1;
            while j < c.len() && c[j] != '"' {
                if c[j] == '\\' {
                    j += 1;
                }
                if at(j) == Some('\n') && !keep_literals {
                    out.push('\n');
                }
                j += 1;
            }
            if keep_literals {
                out.extend(&c[i..(j + 1).min(c.len())]);
            } else {
                out.push_str("\"\"");
            }
            i = j + 1;
            continue;
        }
        if c[i] == '\'' && at(i + 1) == Some('\\') {
            let mut j = i + 2;
            while j < c.len() && c[j] != '\'' {
                j += 1;
            }
            out.push_str("' '");
            i = j + 1;
            continue;
        }
        if c[i] == '\'' && at(i + 2) == Some('\'') {
            out.push_str("' '");
            i += 3;
            continue;
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// The statements of `code` (see [`code_only`]), as (first line, text joined by spaces): a
/// statement ends on a line that ends in `;`, `{` or `}` outside every `(` and `[`, so the
/// statements inside a closure argument stay part of the call that takes it.
fn statements(code: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut start = 0;
    let mut depth = 0i32;
    for (n, line) in code.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if cur.is_empty() {
            start = n + 1;
        }
        cur.push_str(line);
        cur.push(' ');
        for ch in line.chars() {
            match ch {
                '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                _ => {}
            }
        }
        let ends = line.ends_with(';')
            || line.ends_with('{')
            || line.ends_with('}')
            || line.ends_with("},");
        if depth <= 0 && ends {
            out.push((start, cur.trim_end().to_string()));
            cur.clear();
            depth = 0;
        }
    }
    if !cur.is_empty() {
        out.push((start, cur.trim_end().to_string()));
    }
    out
}

/// The name of the function `stmt` opens, if it opens one (`fn name(` / `fn name<`).
fn fn_name(stmt: &str) -> Option<&str> {
    stmt.match_indices("fn ").find_map(|(i, _)| {
        if stmt[..i].chars().next_back().is_some_and(ident_char) {
            return None;
        }
        let rest = &stmt[i + 3..];
        let end = rest.find(|c: char| !ident_char(c)).unwrap_or(rest.len());
        let next = rest[end..].chars().next();
        (end > 0 && matches!(next, Some('(') | Some('<'))).then(|| &rest[..end])
    })
}

/// How often `stmt` calls `name`: `.name(` as a method, or `name(` as a free function
/// (not the tail of a longer word, not a method of the same name).
fn calls(stmt: &str, name: &str) -> usize {
    let needle = format!("{name}(");
    stmt.match_indices(&needle)
        .filter(|(i, _)| {
            name.starts_with('.')
                || stmt[..*i]
                    .chars()
                    .next_back()
                    .is_none_or(|c| !ident_char(c))
        })
        .count()
}

fn any_calls(stmt: &str, names: &std::collections::BTreeSet<String>) -> usize {
    names.iter().map(|n| calls(stmt, n)).sum()
}

/// One function of a fixture source as the settle pin reads it.
struct ApiFn {
    /// The inline module it stands in (`wire`), or `""` at the top of the file.
    module: String,
    /// The type of the `impl` block it stands in (`Colony`, `Stage`, `Reader`), or `""`
    /// for a free function.
    owner: String,
    name: String,
    public: bool,
    /// The return type its signature names (`Value`, `Unsettled<String>`), `None` for none.
    ret: Option<String>,
    /// The raw lines above the signature up to the previous item: docs, attributes and
    /// comments -- where a `// no-settle:` mark stands.
    marks: Vec<String>,
    /// The statements of the body, read as code ([`code_only`]).
    body: Vec<String>,
    /// The whole function with its literals kept and its comments out
    /// ([`code_and_literals`]): where a path in a literal is the read (`colony.db`).
    lit: String,
}

impl ApiFn {
    fn key(&self) -> String {
        if self.owner.is_empty() {
            self.name.clone()
        } else {
            format!("{}::{}", self.owner, self.name)
        }
    }

    /// Whether a statement of the body holds one of `needles`.
    fn holds(&self, needles: &[&str]) -> bool {
        self.body
            .iter()
            .any(|s| needles.iter().any(|n| s.contains(n)))
    }

    /// Whether the function, its literals kept, holds one of `needles`.
    fn holds_literal(&self, needles: &[&str]) -> bool {
        needles.iter().any(|n| self.lit.contains(n))
    }
}

/// The type an `impl` line opens a block for: `impl Colony {` is `Colony`,
/// `impl<'a> Reader<'a> {` is `Reader`, `impl Default for Boot {` is `Boot`.
fn impl_owner(line: &str) -> Option<String> {
    let rest = line.strip_prefix("impl")?;
    let rest = match rest.chars().next() {
        Some('<') => &rest[rest.find('>')? + 1..],
        Some(' ') => rest,
        _ => return None,
    };
    let rest = rest.rsplit(" for ").next().unwrap_or(rest).trim_start();
    let end = rest.find(|c: char| !ident_char(c)).unwrap_or(rest.len());
    (end > 0).then(|| rest[..end].to_string())
}

/// The inline module a line opens (`mod wire {`, `pub(crate) mod x {`); a module from a
/// file (`pub mod presenter_colony;`) opens no block here.
fn mod_opened(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("pub(crate) ")
        .or_else(|| line.strip_prefix("pub(super) "))
        .or_else(|| line.strip_prefix("pub "))
        .unwrap_or(line);
    let name = rest.strip_prefix("mod ")?.strip_suffix('{')?.trim();
    (!name.is_empty() && name.chars().all(ident_char)).then(|| name.to_string())
}

/// The return type of a signature: what follows the `->` after the closing parenthesis
/// of its parameters, up to the body or a `where` (a `-> bool` inside
/// `impl Fn(&Value) -> bool` is a parameter's). Re-check tv-bn M7b5 A-M2: the old pin
/// computed whether a function returns and never read it.
fn signature_ret(sig: &str) -> Option<String> {
    // The parameters open after `fn`: the `(` of a `pub(super)` is none (re-check tv-bn
    // M7b9 A-M1, a `pub(super) fn … -> T` read as no return at all).
    let from = sig.find("fn ").unwrap_or(0);
    let open = from + sig[from..].find('(')?;
    let mut depth = 0i32;
    for (i, ch) in sig[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    let tail = sig[open + i + 1..].trim_start();
                    let ret = tail.strip_prefix("->")?;
                    let end = [ret.find('{'), ret.find(" where "), ret.find(';')]
                        .into_iter()
                        .flatten()
                        .min()
                        .unwrap_or(ret.len());
                    return Some(ret[..end].trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Every function of a fixture source, free or in an `impl` block, at the top of the file
/// or in an inline module, with its body read as code and the raw lines above it. Braces
/// are counted on [`code_only`], so a brace in a literal or a comment moves nothing.
fn api_fns(src: &str) -> Vec<ApiFn> {
    let code = code_only(src);
    let code: Vec<&str> = code.lines().collect();
    let lit = code_and_literals(src);
    let lit: Vec<&str> = lit.lines().collect();
    let raw: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut depth = 0i32;
    // The inline module the line stands in, and the depth of its items.
    let mut module = String::new();
    let mut base = 0i32;
    let mut owner = String::new();
    let mut last_item_end = 0usize;
    let mut i = 0;
    while i < code.len() {
        let line = code[i].trim();
        if depth == 0
            && let Some(m) = mod_opened(line)
        {
            module = m;
            base = 1;
        }
        if depth == base && line.starts_with("impl") {
            owner = impl_owner(line).unwrap_or_default();
        }
        let name = fn_name(line)
            .filter(|_| depth <= base + 1)
            .map(str::to_string);
        if let Some(name) = name {
            // The signature runs to the line that opens the body (or ends a bodiless one).
            let start = i;
            let mut sig = String::new();
            let mut j = i;
            while j < code.len() {
                sig.push_str(code[j].trim());
                sig.push(' ');
                if code[j].contains('{') || code[j].trim_end().ends_with(';') {
                    break;
                }
                j += 1;
            }
            let at = depth;
            let mut body = String::new();
            let mut k = start;
            let mut opened = false;
            while k < code.len() {
                let mut piece = String::new();
                for ch in code[k].chars() {
                    match ch {
                        '{' => {
                            depth += 1;
                            if opened || depth > at + 1 {
                                piece.push(ch);
                            }
                            opened = true;
                        }
                        '}' => {
                            depth -= 1;
                            if depth > at {
                                piece.push(ch);
                            }
                        }
                        _ if opened => piece.push(ch),
                        _ => {}
                    }
                }
                if k > j || (k == j && opened) {
                    body.push_str(&piece);
                    body.push('\n');
                }
                if opened && depth == at {
                    break;
                }
                if !opened && code[k].trim_end().ends_with(';') {
                    break;
                }
                k += 1;
            }
            let marks = raw[last_item_end.min(start)..start]
                .iter()
                .map(|l| l.trim().to_string())
                .collect();
            let sig_head = sig.trim_start();
            out.push(ApiFn {
                module: module.clone(),
                owner: if at == base {
                    String::new()
                } else {
                    owner.clone()
                },
                name,
                public: sig_head.starts_with("pub ") || sig_head.starts_with("pub("),
                ret: signature_ret(&sig),
                marks,
                body: statements(&body).into_iter().map(|(_, s)| s).collect(),
                lit: lit[start.min(lit.len())..(k + 1).min(lit.len())].join("\n"),
            });
            last_item_end = k + 1;
            i = k + 1;
            continue;
        }
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if depth == base {
            owner.clear();
        }
        if depth == 0 {
            module.clear();
            base = 0;
        }
        if line.ends_with('}') || line.ends_with(';') || line.ends_with(',') {
            last_item_end = i + 1;
        }
        i += 1;
    }
    out
}

// The settle pin over the M7b7 form (re-check tv-bn M7b5 A-M1/B-M1, the orchestrator's
// ruling "by construction means the compiler"). The colony's raw doors live in the private
// `mod wire` of the display fixture; a test reaches none of them (it does not compile --
// the probe of the M7b7 report), so a test reads only through the fixtures' functions.
// What the compiler does NOT see is the fixture side: a function of the display or the
// presenter fixture can open the wire settled (`.settled().await`, the barrier and the
// reader in one) or unsettled (`.unsettled()`), and only the second needs a reason.
//
// What carries what (OR-T11-25, re-check tv-bn M7b11 H2):
// - Guarantee, compiler + probe: `Unsettled` opens only by `because` (also in the fixture),
//   the address only against a settle proof (fixture) or a named `Site` (user), the boot
//   only with the key, the decider only sealed -- `the_sealed_core_refuses_every_way_around_it`
//   compiles every way around it in every run.
// - Guarantee, exhaustive match: the `ColonyMsg` readers ([`reads_the_colony`]); a new
//   variant does not compile until it is classified. The `ColonyHandle` readers are derived
//   fail closed (every `pub fn` is one unless pinned otherwise).
// - Guarantee, run time: a `Site` admits only its own file and one place.
// - EARLY WARNING, this scan: which fixture function settles, which opens unsettled with
//   which reason, the needles derived above, a site in two places, the local doors of the
//   users that read a colony of their own. A miss here is no read past the barrier the
//   compiler lets through; it is a fixture or a user that moved without its pin.

/// The two ways a fixture function opens the wire: settled (the barrier and the reader in
/// one) and unsettled (a reader wrapped in `Unsettled`). Split so the pin does not find
/// itself.
const OPENS_SETTLED: &str = concat!(".settl", "ed()");
const OPENS_UNSETTLED: &str = concat!(".unsettl", "ed()");

/// Opening an unsettled value, for ANY reason (re-check tv-bn M7b9 A-M1 a): a held row
/// laundered into a plain `Value` is an unsettled read of its own, and so is a report
/// handed on as a plain text (`self.trail().await.because(Why::Report)` in a new
/// `trail_text`) -- the old list knew `Watch` and `Held` only. So every `because(` is an
/// opening, whatever its argument and however it is called (`.because(`, UFCS
/// `Unsettled::because(`), and so is the private field itself (`.await.0`, `).0`): the
/// display fixture and its child `presenter_colony` both see it. Split so the pin does not
/// find itself.
const OPENS_A_VALUE: [&str; 3] = [
    concat!("becau", "se("),
    concat!(".await", ".0"),
    concat!(")", ".0"),
];

/// Whether `f` opens an unsettled value ([`OPENS_A_VALUE`]): a `because(` as a word (not
/// the tail of `my_because(`), or the field read off a call. A tuple's first field off a
/// call reads the same; name it (`let (a, _) = …`).
fn opens_a_value(f: &ApiFn) -> bool {
    f.body.iter().any(|s| {
        OPENS_A_VALUE.iter().any(|n| {
            s.match_indices(n).any(|(i, _)| {
                let tail = &s[i + n.len()..];
                let before_ok = !n.starts_with('b')
                    || s[..i].chars().next_back().is_none_or(|c| !ident_char(c));
                let after_ok =
                    n.ends_with('(') || tail.chars().next().is_none_or(|c| !ident_char(c));
                before_ok && after_ok
            })
        })
    })
}

/// The two events a fixture hands out as they are: the next message off the egress and
/// the next request the held decider receives.
const EVENT_NEEDLES: [&str; 2] = [concat!(".next_", "out("), concat!("decider.re", "cv(")];

/// A read over HTTP: the mount listener is open to anyone, the compiler guards nothing
/// there, so such a reader settles first (`page`, `status`).
///
/// A websocket or a raw socket is a read past the barrier as much as a GET (re-check tv-bn
/// M7b9 A-M2): `socket` joins through `page`, and it settles first.
const HTTP: [&str; 4] = [
    concat!("reqw", "est::"),
    concat!("connect_", "async"),
    concat!("TcpStream::", "connect"),
    concat!("hyp", "er::"),
];

/// Raw reads of the colony outside `mod wire`, DERIVED (OR-T11-25 G3, re-check tv-bn
/// M7b11 A-M5): every `ColonyMsg` variant [`reads_the_colony`] answers `true` for (an
/// exhaustive `match`: a new variant does not compile until it is classified), every
/// reader of `ColonyHandle` (its `pub fn`s minus the pinned [`NOT_HANDLE_READERS`]: a new
/// method is a reader until it is pinned otherwise), and the colony's database crate.
/// The root's guard `_td` is read as a word ([`reads_the_root`]). Since M7b7 a fixture
/// function outside the wire holds no `ColonyHandle`; only the listed doors may.
fn raw_reads() -> Vec<String> {
    static NEEDLES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NEEDLES
        .get_or_init(|| {
            let mut needles = colony_msg_readers();
            needles.extend(handle_readers().into_iter().map(|n| format!(".{n}(")));
            needles.push(concat!("rusq", "lite").to_string());
            needles
        })
        .clone()
}

// BEGIN reads_the_colony (the 809 pin parses the `=> true` arms between these marks)
/// Whether a message to the colony reads its state: every variant classified, no wildcard,
/// so a new `ColonyMsg` variant is a compile error here until it is.
fn reads_the_colony(m: &meclaw_colony::ColonyMsg) -> bool {
    use meclaw_colony::ColonyMsg as M;
    match m {
        M::ReadLiveness { .. } => true,
        M::DrainDeadLetters { .. } => true,
        M::ReadTrace { .. } => true,
        M::ReadLedger { .. } => true,
        M::ReadMessages { .. } => true,
        M::ReadGraph { .. } => true,
        M::ReadInboundEdges { .. } => true,
        M::ReadMutationsAudit { .. } => true,
        M::ReadTemplates { .. } => true,
        M::ReadDeadLetters { .. } => true,
        M::ReadRegistry { .. } => true,
        M::Settle { .. } => false,
        M::Register { .. } => false,
        M::RegisterDormant { .. } => false,
        M::AddEdge { .. } => false,
        M::SetNodeContract { .. } => false,
        M::SetRegistryProvenance { .. } => false,
        M::SetRegistryStatus { .. } => false,
        M::SetRegistryDormant { .. } => false,
        M::Route { .. } => false,
        M::Shutdown { .. } => false,
        M::ShutdownNow { .. } => false,
        M::CellDied { .. } => false,
        M::AddHiveScope { .. } => false,
        M::IoLiveness { .. } => false,
        M::WorkDone { .. } => false,
        M::ConsumeMark { .. } => false,
        M::CountAttempt { .. } => false,
        M::DeadLetterMessage { .. } => false,
        M::InitialApply { .. } => false,
        M::BeginInitialApply { .. } => false,
        M::RescanTemplates { .. } => false,
        M::KeepTemplateVersions { .. } => false,
        M::Sleep { .. } => false,
        M::MailboxRescued { .. } => false,
        M::MailboxRescuedLate(_) => false,
        M::Overflow(_) => false,
        M::Stopped { .. } => false,
        M::StopWiringRestored { .. } => false,
        M::CellDbUnusable { .. } => false,
        M::Mutation { .. } => false,
        M::MutationDoor { .. } => false,
    }
}
// END reads_the_colony

/// The `ColonyMsg` variants [`reads_the_colony`] classifies as reads, parsed from its arms
/// in this file. Fewer than eleven is a broken parse, not a smaller colony.
fn colony_msg_readers() -> Vec<String> {
    let src = include_str!("809_a_restart_reads_once_and_draws_the_same_screen.rs");
    let begin = concat!("// BEGIN reads_", "the_colony");
    let end = concat!("// END reads_", "the_colony");
    let at = src.find(begin).expect("the reader classification stands");
    let block = &src[at..at + src[at..].find(end).expect("it ends")];
    let found = msg_readers_of(block).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        found.len() >= 11,
        "the ColonyMsg readers are derived: {found:?}"
    );
    found
}

/// The `=> true` variants of the classification block, fail closed (re-check tv-bn M7b12
/// B-M1 a): every non-empty line between `match m {` and its closing `}` is exactly one
/// arm `M::<Variant>[ { .. }|(_)] => true|false,`. An or-pattern, an arm rustfmt wrapped,
/// a block body, a comment behind the comma: an unknown form, red -- never a reader
/// silently left out.
fn msg_readers_of(block: &str) -> Result<Vec<String>, String> {
    let open = "    match m {";
    let at = block
        .find(open)
        .ok_or("the classification has no `match m {`")?;
    let body = &block[at + open.len()..];
    let mut found = Vec::new();
    let mut closed = false;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if line == "    }" {
            closed = true;
            break;
        }
        let arm = line
            .strip_prefix("        M::")
            .ok_or_else(|| format!("an arm of unknown form: `{line}`"))?;
        let n = arm.find(|c: char| !ident_char(c)).unwrap_or(arm.len());
        let (name, rest) = arm.split_at(n);
        let verdict = [" { .. }", "(_)", ""]
            .iter()
            .find_map(|p| rest.strip_prefix(p)?.strip_prefix(" => "))
            .ok_or_else(|| format!("an arm of unknown form: `{line}`"))?;
        match verdict {
            "true," => found.push(name.to_string()),
            "false," => {}
            _ => return Err(format!("an arm of unknown form: `{line}`")),
        }
    }
    if !closed {
        return Err("the classification's `match` does not close".to_string());
    }
    Ok(found)
}

/// The `pub fn`s of `ColonyHandle` that do NOT read the colony, each with why: built,
/// written to, stopped, or a configuration value. Every other `pub fn` of the handle is a
/// reader (fail closed), and a name here that the handle lost is red.
const NOT_HANDLE_READERS: [(&str, &str); 24] = [
    ("new", "builds a handle"),
    ("new_with_echo", "builds a handle"),
    ("with_db_path", "builds a handle"),
    ("new_with_echo_at", "builds a handle"),
    ("new_with_factories_at", "builds a handle"),
    ("new_with_factories_and_env_at", "builds a handle"),
    ("new_with_marked_egress_at", "builds a handle"),
    (
        "new_with_factories_and_death_ack_signal_at",
        "builds a handle",
    ),
    ("new_with_blobs_at", "builds a handle"),
    (
        "blob_inline_max_bytes",
        "a configuration value of the handle, no colony state",
    ),
    ("spawn", "spawns a cell"),
    ("add_edge", "writes an edge"),
    ("send_from", "sends a message"),
    ("send", "sends a message"),
    ("sync", "waits for the colony's inbox, hands back nothing"),
    ("quiesce", "waits for rest, hands back nothing"),
    ("add_hive_scope", "writes a hive scope"),
    ("outputs_sender", "the sender cells emit on: a write door"),
    (
        "runtime",
        "the runtime a boot spawns cells with: a build door",
    ),
    ("register_spawned", "registers a cell"),
    ("register_spawned_typed", "registers a cell"),
    ("abort", "stops the colony"),
    ("join_result", "waits for the colony's end"),
    ("shutdown", "stops the colony"),
];

/// The readers of `ColonyHandle`: its `pub fn`s (`impl ColonyHandle`, read from
/// `meclaw-testing`) minus [`NOT_HANDLE_READERS`].
fn handle_readers() -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../meclaw-testing/src");
    let mut files = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("meclaw-testing reads") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&root)
                    .expect("under src")
                    .display()
                    .to_string();
                let src = std::fs::read_to_string(&path).expect("a source reads");
                files.push((rel, code_only(&src)));
            }
        }
    }
    let names = handle_fns_of(&files).unwrap_or_else(|e| panic!("{e}"));
    for (n, _) in NOT_HANDLE_READERS {
        assert!(
            names.iter().any(|m| m == n),
            "NOT_HANDLE_READERS names `{n}`, which ColonyHandle no longer has"
        );
    }
    let readers: Vec<String> = names
        .into_iter()
        .filter(|n| !NOT_HANDLE_READERS.iter().any(|(m, _)| m == n))
        .collect();
    assert!(
        readers.len() >= 6,
        "the ColonyHandle readers are derived: {readers:?}"
    );
    readers
}

/// The `impl` heads of `ColonyHandle` meclaw-testing may have, as (file, head): every other
/// one -- a second inherent block elsewhere, a `Deref`/`AsRef` that opens a type inside --
/// is red (re-check tv-bn M7b12 B-M1 b).
const HANDLE_IMPLS: [(&str, &str); 3] = [
    ("colony_handle.rs", "impl ColonyHandle {"),
    ("colony_handle.rs", "impl Default for ColonyHandle {"),
    ("colony_handle.rs", "impl Drop for ColonyHandle {"),
];

/// The struct of the handle, as (file, head), and its public fields (re-check tv-bn M7b14
/// C-M1): a `pub` field is a door like a `pub fn` -- a new one opens an inner type past
/// every needle. Any other head, a second struct, a field outside this list (`pub` or
/// `pub(…)`) or a pinned field gone is red.
const HANDLE_STRUCT: (&str, &str) = ("colony_handle.rs", "pub struct ColonyHandle {");
const HANDLE_FIELDS: [&str; 2] = ["inbox_tx", "outputs_tx"];

/// The `pub fn`s of `ColonyHandle`'s inherent block, from (file, code) of meclaw-testing,
/// fail closed (re-check tv-bn M7b12 B-M1 b): every `impl` head that names the handle is
/// one of [`HANDLE_IMPLS`], and every line of the block at indent 4 that starts with
/// `pub ` and holds `fn ` is `pub fn` or `pub async fn` -- `pub const fn`, `pub unsafe fn`
/// and the like are an unknown form, red.
fn handle_fns_of(files: &[(String, String)]) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut structs = 0;
    let mut fields = Vec::new();
    for (file, code) in files {
        let mut head: Option<String> = None;
        let mut in_impl = false;
        let mut in_struct = false;
        for line in code.lines() {
            let t = line.trim();
            let struct_head = !t.starts_with("//")
                && t.split_whitespace()
                    .collect::<Vec<_>>()
                    .windows(2)
                    .any(|w| w[0] == "struct" && names_word(w[1], "ColonyHandle"));
            if struct_head {
                if (file.as_str(), t) != HANDLE_STRUCT {
                    return Err(format!(
                        "{file}: a struct of ColonyHandle not pinned: `{t}`"
                    ));
                }
                structs += 1;
                in_struct = true;
                continue;
            }
            if in_struct {
                if line.starts_with('}') {
                    in_struct = false;
                } else if t.starts_with("pub ") || t.starts_with("pub(") {
                    let indent = line.len() - line.trim_start().len();
                    let name = t
                        .strip_prefix("pub ")
                        .filter(|_| indent == 4)
                        .and_then(|f| f.split_once(':'))
                        .map(|(n, _)| n.trim())
                        .filter(|n| HANDLE_FIELDS.contains(n))
                        .ok_or_else(|| {
                            format!("{file}: a public field of ColonyHandle not pinned: `{t}`")
                        })?;
                    fields.push(name.to_string());
                }
                continue;
            }
            if head.is_none() && (t == "impl" || t.starts_with("impl ") || t.starts_with("impl<")) {
                head = Some(String::new());
            }
            if let Some(h) = head.as_mut() {
                if !h.is_empty() {
                    h.push(' ');
                }
                h.push_str(t);
                if !t.ends_with('{') && !t.ends_with(';') {
                    continue;
                }
                let h = head.take().unwrap_or_default();
                let names_handle = h.match_indices("ColonyHandle").any(|(i, w)| {
                    !h[..i].chars().next_back().is_some_and(ident_char)
                        && !h[i + w.len()..].chars().next().is_some_and(ident_char)
                });
                if names_handle
                    && !HANDLE_IMPLS
                        .iter()
                        .any(|(f, x)| f == file && *x == h.as_str())
                {
                    return Err(format!("{file}: an impl of ColonyHandle not pinned: `{h}`"));
                }
                in_impl = file == "colony_handle.rs" && h == "impl ColonyHandle {";
                continue;
            }
            if line.starts_with('}') {
                in_impl = false;
                continue;
            }
            if !in_impl {
                continue;
            }
            let indent = line.len() - line.trim_start().len();
            if indent != 4 || !t.starts_with("pub ") || !t.contains("fn ") {
                continue;
            }
            let sig = t
                .strip_prefix("pub async fn ")
                .or_else(|| t.strip_prefix("pub fn "))
                .ok_or_else(|| format!("{file}: a `pub fn` of unknown form: `{t}`"))?;
            let n = sig.find(|c: char| !ident_char(c)).unwrap_or(sig.len());
            names.push(sig[..n].to_string());
        }
    }
    if structs != 1 {
        return Err(format!(
            "the struct ColonyHandle stands {structs} times, not once"
        ));
    }
    fields.sort();
    if fields != HANDLE_FIELDS {
        return Err(format!(
            "the public fields of ColonyHandle are {fields:?}, pinned {HANDLE_FIELDS:?}"
        ));
    }
    Ok(names)
}

/// Re-check tv-bn M7b14 B-M3: the guard of `GET /colony/messages` on hand-made replies
/// with a page of 2 -- a cursor reads on, a cut scan or a full page without a cursor is a
/// window and refused, never counted as the whole log; the door asks and judges with ONE
/// constant, the colony's own page.
#[test]
fn the_message_door_reads_every_page_and_refuses_a_window() {
    use display_colony::PageReply;
    use meclaw_colony::api_dto::{MessageLogCursor, MessageLogDto};
    let row = |i: usize| MessageLogDto {
        id: format!("m{i}"),
        trace_id: String::new(),
        parent_message_id: None,
        correlation_id: None,
        ttl: 1,
        from_path: "/".into(),
        to_path: "/".into(),
        reply_to: None,
        headers_json: "{}".into(),
        body_kind: "inline".into(),
        body_payload: None,
        created_at: i as i64,
    };
    let reply = |n: usize, cursor: bool, cut: bool| PageReply {
        entries: (0..n).map(row).collect(),
        next: cursor.then(|| MessageLogCursor {
            created_at: 1,
            id: "m1".into(),
        }),
        scan_budget: 10,
        scan_truncated: cut,
    };
    assert!(
        matches!(next_page(&reply(0, false, false), 2), Ok(None)),
        "an empty log"
    );
    assert!(
        matches!(next_page(&reply(1, false, false), 2), Ok(None)),
        "a last page"
    );
    assert!(
        matches!(next_page(&reply(2, true, false), 2), Ok(Some(c)) if c.id == "m1"),
        "a full page with a cursor reads on"
    );
    assert!(
        next_page(&reply(2, false, false), 2).is_err(),
        "a full page without a cursor is a window"
    );
    assert!(
        next_page(&reply(1, false, true), 2).is_err(),
        "a cut scan is a window"
    );
    assert!(
        next_page(&reply(2, true, true), 2).is_err(),
        "a cut scan is a window, cursor or not"
    );
    let src = include_str!("support/display_colony.rs");
    let at = src.find("    async fn get_messages(").expect("the door");
    let door = &src[at..at + src[at..].find("\n    }\n").expect("the door's end")];
    assert_eq!(
        door.matches("whole_messages(&inbox)").count(),
        1,
        "the door counts what `whole_messages` read, the whole log"
    );
    assert!(
        !names_word(door, "MessageLogFilter") && !door.contains("ReadMessages"),
        "the door reads no page of its own"
    );
    let at = src
        .find("pub(crate) async fn whole_messages(")
        .expect("the reader");
    let reader = &src[at..at + src[at..].find("\n}\n").expect("the reader's end")];
    for needle in ["limit: MESSAGES_PAGE", "next_page(&reply, MESSAGES_PAGE)"] {
        assert_eq!(
            reader.matches(needle).count(),
            1,
            "the reader names `{needle}` once"
        );
    }
    assert!(
        !reader.contains("_000"),
        "the reader names no page of its own"
    );
    let colony = include_str!("../../meclaw-colony/src/colony_dispatch.rs");
    assert!(
        colony.contains(&format!("filter.limit.clamp(1, {MESSAGES_PAGE})")),
        "MESSAGES_PAGE is the page the colony serves (`handle_read_messages`)"
    );
}

/// M7b15 find to re-check tv-bn M7b14 B-M3: the door counts the WHOLE log of a real
/// colony that holds more rows than one read serves. The colony clamps every
/// `ReadMessages` to [`MESSAGES_PAGE`] rows, so a door that reads once (the 100_000 it
/// asked for before) counted a window of 1000 as the log, and its guard never fired.
/// Every seeded row is in the door's count.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_message_door_counts_a_log_longer_than_one_read() {
    const ROWS: usize = 2 * MESSAGES_PAGE + 500;
    let h = meclaw_testing::ColonyHandle::new();
    let seeded = display_colony::seed_messages(&h, ROWS).await;
    let door = display_colony::whole_messages(&h.inbox_tx)
        .await
        .expect("the door reads the log");
    let read: std::collections::HashSet<&str> = door.iter().map(|r| r.id.as_str()).collect();
    let missing = seeded
        .iter()
        .filter(|id| !read.contains(id.as_str()))
        .count();
    assert_eq!(
        (missing, door.len() >= ROWS),
        (0, true),
        "the door counts the whole log ({} rows read of {ROWS} seeded), not one read of \
         {MESSAGES_PAGE}",
        door.len()
    );
    h.shutdown().await;
}

/// K8 class "a reader takes one page for the whole" (tv-bn M7b16, M7b17): every raw read
/// of a log the colony serves in clamped pages -- a code line that names one of the
/// [`log_reads`] (derived from the `ColonyMsg` reads with a `limit`/`filter`, not written down) -- in the tests of
/// every crate and in meclaw-testing, as (file, lines, why one read is enough there). A
/// reader of the whole log goes through `ColonyHandle::read_log` (paged) or
/// `whole_messages`; a read without a cursor says when it is full (`OnePage::short_of`).
/// A raw read anywhere else, or one more here, is red.
const RAW_LOG_READS: &[(&str, usize, &str)] = &[
    (
        "meclaw-testing/src/wait.rs",
        1,
        "`wait_for_mutation_status`: a miss in a full audit read is red",
    ),
    (
        "meclaw-colony/tests/gh_3/gh683_a_message_read_keeps_the_loop_beating.rs",
        3,
        "the read itself is the subject: it keeps the loop beating",
    ),
    (
        "meclaw-colony/tests/misc/p1_read_messages_inbox.rs",
        2,
        "the page is the subject (`ReadMessages` through the inbox)",
    ),
    (
        "meclaw-colony/tests/phase/phase_12_b_read_dead_letters.rs",
        7,
        "the page is the subject (filters, order, clamp of the DLQ read)",
    ),
    (
        "meclaw-colony/tests/phase/phase_12_b_read_mutations_audit.rs",
        2,
        "the page is the subject (the audit read)",
    ),
    (
        "meclaw-colony/tests/phase/phase_12_b_read_trace.rs",
        5,
        "the page is the subject (the trace read)",
    ),
    (
        "meclaw-testing/src/colony_handle.rs",
        2,
        "`read_log`: the paged reader every other one goes through; `mutations_audit`: the \
         one audit read, `short_of(AUDIT_PAGE)`, red at a full read",
    ),
    (
        "meclaw-cells/tests/support/display_colony.rs",
        3,
        "`whole_messages` (paged, the door), `Reader::newest(n)` (a named window), \
         `read_dead_letters` (a diagnostic that says when its read is full)",
    ),
    (
        "meclaw-cells/tests/support/web_fixture.rs",
        1,
        "`read_classes`: red when the lab outgrows one page",
    ),
    (
        "meclaw-cells/tests/store_2/phase_14b_tool_loop_store.rs",
        3,
        "the DLQ is judged empty: one read of any size decides that",
    ),
    (
        "meclaw-cells/tests/phase/phase_14c_lexical_rag.rs",
        1,
        "the DLQ is judged empty",
    ),
    (
        "meclaw-cells/tests/phase/phase_14d_multi_agent.rs",
        1,
        "the DLQ is judged empty",
    ),
];

/// Bounded reads that are no log: each is a `ColonyMsg` read with a `limit`, but its rows
/// are bounded by what the colony holds, not a record that grows with every message. Each
/// must be one of the derived reads (fail closed).
const NOT_A_LOG: &[(&str, &str)] = &[
    (
        "ReadRegistry",
        "the topology, bounded by the colony's own cells",
    ),
    (
        "ReadTemplates",
        "the template catalog, bounded by the template files on disk",
    ),
];

/// The `ColonyMsg` variants in `src` (the source of `colony.rs`) that carry a field named
/// `limit` or `filter` -- a read the colony answers in bounded pages. Derived from the
/// message enum, not from handler names (re-check tv-bn M7b17 A-M1: a name scan over
/// `handle_read_*` missed `pub(crate)`, `handle_list_*`/`handle_audit_*` and a clamp in a
/// private helper, and took the helper `handle_read_templates_from_rows` for a read).
fn bounded_reads(src: &str) -> Vec<String> {
    let Some(start) = src.find("pub enum ColonyMsg {") else {
        return Vec::new();
    };
    let body = &src[start..];
    let body = body.find("\n}\n").map_or(body, |end| &body[..end]);
    let mut out = Vec::new();
    let mut variant: Option<&str> = None;
    for line in body.lines() {
        // A variant opens at four spaces: `    Name {` (a field list) or `    Name,`/`(`.
        if let Some(rest) = line.strip_prefix("    ")
            && rest.starts_with(|c: char| c.is_ascii_uppercase())
        {
            let name_end = rest
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            variant = Some(&rest[..name_end]);
            // A one-line field list: `    Name { limit: usize },`.
            let fields = &rest[name_end..];
            if ["{ limit:", "{ filter:", ", limit:", ", filter:"]
                .iter()
                .any(|f| fields.contains(f))
            {
                out.push(rest[..name_end].to_string());
            }
            continue;
        }
        // A field at eight spaces, named exactly `limit` or `filter`.
        let field = line.strip_prefix("        ").unwrap_or("");
        if !field.starts_with(' ')
            && (field.starts_with("limit:") || field.starts_with("filter:"))
            && let Some(v) = variant
        {
            out.push(v.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The log reads the scan looks for: the bounded reads of the colony minus [`NOT_A_LOG`].
/// An empty derivation is red: the scan would look for nothing and pass.
fn log_reads() -> Vec<String> {
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../meclaw-colony/src/colony.rs"),
    )
    .expect("colony.rs reads");
    let bounded = bounded_reads(&src);
    assert!(
        !bounded.is_empty(),
        "no bounded read derived from ColonyMsg: the scan looks for nothing"
    );
    for (word, _) in NOT_A_LOG {
        assert!(
            bounded.iter().any(|c| c == word),
            "NOT_A_LOG names {word}, which is no bounded read of ColonyMsg: {bounded:?}"
        );
    }
    bounded
        .into_iter()
        .filter(|c| NOT_A_LOG.iter().all(|(w, _)| w != c))
        .collect()
}

/// The raw log reads of `files` (crate-relative path, code) as (path, count), sorted. This
/// file is the scanner and names the reads in its needles; it is left out.
fn raw_log_reads(files: &[(String, String)]) -> Vec<(String, usize)> {
    let words = log_reads();
    let mut out: Vec<(String, usize)> = files
        .iter()
        .filter(|(path, _)| !path.ends_with(file!().rsplit('/').next().unwrap_or("")))
        .map(|(path, code)| {
            let n = code
                .lines()
                .map(str::trim)
                .filter(|t| !t.starts_with("//"))
                .map(without_strings)
                .filter(|t| words.iter().any(|w| names_word(t, w)))
                .count();
            (path.clone(), n)
        })
        .filter(|(_, n)| *n > 0)
        .collect();
    out.sort();
    out
}

/// `line` with the contents of its string literals dropped (`"..."`, escapes kept inside):
/// a read is code (`ColonyMsg::ReadMessages {`), the colony's own error text quoted by an
/// oracle (`"ReadMessages SQL failed"`, gh1061 M6 M1 (b)) reads nothing. Welle-Merge R<->N
/// (NS): R's log-read-error oracles named the word only inside strings.
fn without_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_str = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
                out.push(c);
            }
        } else {
            if c == '"' {
                in_str = true;
            }
            out.push(c);
        }
    }
    out
}

/// The `.rs` files under `crates/<dir>`, as (path from `crates/`, code).
fn crate_sources(dir: &str) -> Vec<(String, String)> {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    let mut dirs = vec![crates.join(dir)];
    while let Some(d) = dirs.pop() {
        for entry in std::fs::read_dir(&d).expect("the sources read") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let rel = path
                    .strip_prefix(&crates)
                    .expect("under crates/")
                    .to_string_lossy()
                    .into_owned();
                files.push((rel, std::fs::read_to_string(&path).expect("a source reads")));
            }
        }
    }
    files
}

#[test]
fn no_reader_takes_one_page_of_the_log_for_the_whole() {
    // Every crate's tests (re-check tv-bn M7b16 A-H3), and meclaw-testing.
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut roots: Vec<String> = std::fs::read_dir(&crates)
        .expect("crates/ reads")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("tests").is_dir())
        .map(|e| format!("{}/tests", e.file_name().to_string_lossy()))
        .collect();
    roots.sort();
    assert!(
        roots.len() >= 6,
        "the scan sees the crates' tests: {roots:?}"
    );
    let mut files: Vec<(String, String)> = roots.iter().flat_map(|r| crate_sources(r)).collect();
    files.extend(crate_sources("meclaw-testing/src"));
    // A quoted word is no read, code is one -- also behind a string on the same line.
    let invented = vec![(
        "invented.rs".to_string(),
        "    \"ReadMessages SQL failed\",\n    \"\\\"ReadTrace\\\" x\",\n    \
         let t = \"a\"; tx.send(ColonyMsg::ReadTrace {\n    ColonyMsg::ReadMessages {\n"
            .to_string(),
    )];
    assert_eq!(
        raw_log_reads(&invented),
        [("invented.rs".to_string(), 2)],
        "two reads in code; the quoted error texts read nothing"
    );
    // The words are derived, not listed: the bounded log reads of the colony.
    assert_eq!(
        log_reads(),
        [
            "ReadDeadLetters",
            "ReadMessages",
            "ReadMutationsAudit",
            "ReadTrace"
        ],
        "the bounded log reads the scan looks for"
    );
    // The derivation, against invented enums: each field kind joins, a near miss does not.
    let invented = "pub enum ColonyMsg {\n    Settle {\n        ack: Tx,\n    },\n    \
                    ListWidgets {\n        /// at most 1000\n        limit: usize,\n    },\n    \
                    AuditWidgets {\n        filter: WidgetFilter,\n        reply: Tx,\n    },\n    \
                    PeekWidgets { limit: usize },\n    \
                    Register {\n        restart_limit: Option<u32>,\n        \
                    /// the limit: is documented here\n    },\n    \
                    Nested {\n        inner: Inner {\n            limit: usize,\n        },\n    },\n}\n";
    assert_eq!(
        bounded_reads(invented),
        ["AuditWidgets", "ListWidgets", "PeekWidgets"],
        "a new bounded read joins the scan by its field, whatever its name; \
         `restart_limit`, a doc line and a nested field do not"
    );
    assert!(
        bounded_reads("pub enum Other {\n    ReadTrace {\n        limit: usize,\n    },\n}\n")
            .is_empty(),
        "only ColonyMsg is read"
    );
    let pinned = |list: &[(&str, usize, &str)]| {
        let mut v: Vec<(String, usize)> =
            list.iter().map(|(f, n, _)| (f.to_string(), *n)).collect();
        v.sort();
        v
    };
    assert_eq!(
        raw_log_reads(&files),
        pinned(RAW_LOG_READS),
        "a raw read of the log or the DLQ outside the pinned ones: a page read as the whole"
    );
    // Mutants: each reader this round moved onto `read_log` (808 `patches`/`touches`,
    // gh963 `Run::log`, gh905 `latest_manifest`), and the door, with one raw read back.
    let raw = "        .send(ColonyMsg::ReadMessages {\n";
    let mut wrong = Vec::new();
    for target in [
        "meclaw-cells/tests/808_the_colony_view_stands_in_the_dock.rs",
        "meclaw-cells/tests/round_2/gh963_a_real_tool_round_reaches_the_presenter.rs",
        "meclaw-cells/tests/misc_7/gh905_a_workspace_is_materialized_and_adopted.rs",
        "meclaw-cells/tests/support/display_colony.rs",
    ] {
        let mutated: Vec<(String, String)> = files
            .iter()
            .map(|(f, c)| {
                (
                    f.clone(),
                    if f == target {
                        format!("{c}{raw}")
                    } else {
                        c.clone()
                    },
                )
            })
            .collect();
        if raw_log_reads(&mutated) == pinned(RAW_LOG_READS) {
            wrong.push(format!("a raw read back in {target} reads green"));
        }
    }
    let dlq = "    .send(meclaw_colony::ColonyMsg::ReadDeadLetters {\n".to_string();
    let mut more = files.clone();
    more.push(("meclaw-cells/tests/new_test.rs".into(), dlq));
    if raw_log_reads(&more) == pinned(RAW_LOG_READS) {
        wrong.push("a raw DLQ read in a new file reads green".into());
    }
    let audit = "        .send(ColonyMsg::ReadMutationsAudit {\n".to_string();
    let mut more = files.clone();
    more.push(("meclaw-colony/tests/new_test.rs".into(), audit));
    if raw_log_reads(&more) == pinned(RAW_LOG_READS) {
        wrong.push("a raw audit read in a colony test reads green".into());
    }
    let aliased = "        M::ReadTrace { .. } => true,\n".to_string();
    let mut more = files.clone();
    more.push(("meclaw-cells/tests/new_test.rs".into(), aliased));
    if raw_log_reads(&more) == pinned(RAW_LOG_READS) {
        wrong.push("an aliased trace read reads green".into());
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Re-check tv-bn M7b12 B-M1: both derivations are fail closed -- each form the parse does
/// not know is red, the forms as they stand are read.
#[test]
fn the_reader_derivations_refuse_an_unknown_form() {
    let block = "    match m {\n        M::ReadA { .. } => true,\n        M::Write { .. } => false,\n        M::Over(_) => false,\n    }\n";
    assert_eq!(msg_readers_of(block), Ok(vec!["ReadA".to_string()]));
    let mut wrong = Vec::new();
    for (case, arm) in [
        (
            "or-pattern",
            "        M::ReadA { .. } | M::ReadB { .. } => true,",
        ),
        (
            "wrapped arm",
            "        M::ReadB {\n            ..\n        } => true,",
        ),
        ("block body", "        M::ReadB { .. } => { true }"),
        (
            "comment behind the comma",
            "        M::ReadB { .. } => true, // b",
        ),
    ] {
        let mutant = block.replace("        M::Write { .. } => false,", arm);
        if msg_readers_of(&mutant).is_ok() {
            wrong.push(format!("the ColonyMsg derivation reads a {case}"));
        }
    }
    let strukt = "pub struct ColonyHandle {\n    pub inbox_tx: A,\n    pub outputs_tx: B,\n    join: C,\n}\n";
    let handle = format!(
        "{strukt}impl ColonyHandle {{\n    pub fn new() -> Self {{\n    }}\n    pub async fn read_log(&self) {{\n    }}\n}}\nimpl Default for ColonyHandle {{\n}}\n"
    );
    let handle = handle.as_str();
    let one = |code: &str| vec![("colony_handle.rs".to_string(), code.to_string())];
    assert_eq!(
        handle_fns_of(&one(handle)),
        Ok(vec!["new".to_string(), "read_log".to_string()])
    );
    for (case, files) in [
        (
            "`pub const fn`",
            one(&handle.replace("    pub fn new()", "    pub const fn new()")),
        ),
        (
            "`pub unsafe fn`",
            one(&handle.replace("    pub fn new()", "    pub unsafe fn new()")),
        ),
        (
            "`impl Deref for ColonyHandle`",
            one(&format!(
                "{handle}impl std::ops::Deref for ColonyHandle {{\n}}\n"
            )),
        ),
        (
            "a second inherent block in another file",
            vec![
                ("colony_handle.rs".to_string(), handle.to_string()),
                (
                    "more.rs".to_string(),
                    "impl ColonyHandle {\n    pub fn peek(&self) {\n    }\n}\n".to_string(),
                ),
            ],
        ),
        // Re-check tv-bn M7b14 C-M1: the struct's public fields are a door as well.
        (
            "a new `pub` field of the handle",
            one(&handle.replace("    join: C,", "    pub join: C,")),
        ),
        (
            "a `pub(crate)` field of the handle",
            one(&handle.replace("    join: C,", "    pub(crate) join: C,")),
        ),
        (
            "a generic struct head",
            one(&handle.replace("pub struct ColonyHandle {", "pub struct ColonyHandle<T> {")),
        ),
        (
            "a wrapped struct head",
            one(&handle.replace("pub struct ColonyHandle {", "pub struct ColonyHandle\n{")),
        ),
        (
            "a vanished pinned field",
            one(&handle.replace("    pub outputs_tx: B,\n", "")),
        ),
        ("no struct of the handle", one(&handle.replace(strukt, ""))),
        (
            "a second struct of the handle",
            vec![
                ("colony_handle.rs".to_string(), handle.to_string()),
                ("more.rs".to_string(), strukt.to_string()),
            ],
        ),
        (
            "a wrapped impl head",
            one(&format!(
                "{handle}impl AsRef<u8>\n    for ColonyHandle\n{{\n}}\n"
            )),
        ),
    ] {
        if handle_fns_of(&files).is_ok() {
            wrong.push(format!("the ColonyHandle derivation reads {case}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "a reader derivation is fail open:\n{}",
        wrong.join("\n")
    );
}

/// Whether a statement reads the colony's root guard `_td` (re-check tv-bn M7b11 A-M5):
/// `_td` as a word, except where it is declared or set (`_td:`, `_td: td`) or bound
/// (`let _td =`). `self._td.path()` and `let t = &self._td;` are reads.
fn reads_the_root(stmt: &str) -> bool {
    let word = concat!("_t", "d");
    stmt.match_indices(word).any(|(i, _)| {
        let before = stmt[..i].chars().next_back();
        let after = stmt[i + word.len()..].trim_start();
        before.is_none_or(|c| !ident_char(c))
            && !after.starts_with(|c: char| ident_char(c))
            && !after.starts_with(':')
            && !(after.starts_with('=') && !after.starts_with("=="))
    })
}

/// The colony's files under its root (re-check tv-bn M7b9 A-M1 c / M2): a path is a
/// literal, so these are read with the literals kept ([`code_and_literals`]), comments
/// out. Split so the pin does not find itself.
const RAW_READ_FILES: [&str; 2] = [concat!("colony", ".db"), concat!("cell", ".db")];

/// The owners of `mod wire`: the raw doors themselves, outside the pin (the module
/// boundary keeps them in).
const WIRE_OWNERS: [&str; 2] = ["Wire", "Reader"];

/// Every verb of `Wire`, by name, with its class and the return type its signature names
/// (`""` for none) -- re-check tv-bn M7b9 A-M1 b: the pin skips the wire's own functions,
/// and the compiler keeps TESTS off the wire, but not the parent module off a NEW verb
/// (`pub(super) async fn all(&self) -> Vec<MessageLogDto> { self.h.read_log(…) }` plus a
/// `Colony::all` was a read past every barrier, derived by nothing). So a verb that is not
/// here is red, a verb here that is gone is red, and one whose return type moved is red.
/// The classes: `build` makes the wire, `write` puts into the colony and hands nothing
/// back, `ack` puts in and hands back the colony's answer to that one message, `event`
/// hands out the thing a lock waits for, `barrier` settles (and `settled` hands out the
/// reader after it), `opening` hands out the reader wrapped in `Unsettled`, `report` reads
/// a hang's last state for its text.
const WIRE_VERBS: [(&str, &str, &str); 11] = [
    ("new", "build", "Self"),
    ("send", "write", ""),
    ("next_out", "event", "Option<Message>"),
    ("log_changes", "event", "super::LogChanges"),
    ("shutdown", "write", ""),
    ("rescan_templates", "ack", "Result<(), String>"),
    ("mutate", "ack", "MutationOutcome"),
    ("settle", "barrier", "Settled"),
    ("last_state", "report", "String"),
    ("settled", "barrier", "Reader<'_>"),
    ("unsettled", "opening", "Unsettled<Reader<'_>>"),
];

/// The only verbs that build a `Reader` (re-check tv-bn M7b9 A-M1 b): after the barrier,
/// wrapped in `Unsettled`, and for a hang's text. A `Reader(` anywhere else in the
/// fixtures -- a new verb handing out `Reader(self)`, a known one that grew one -- is red.
const READER_BUILT_BY: [&str; 3] = ["settled", "unsettled", "last_state"];

/// The readers that read ONE page on purpose (`limit:`), each with its reason -- re-check
/// tv-bn M7b12 C-M2: `Reader::log` read one page of 1000 and a full page read as "nothing
/// more", while the picks read the whole log. Every other `Reader` function reads the
/// whole (`ColonyHandle::read_log`, page by page), and these two are called by nothing but
/// `Wire::last_state`, the text of a hang -- never a verdict.
const WINDOW_READERS: [(&str, &str); 2] = [
    ("newest", "the newest rows of a hang's last state"),
    (
        "read_dead_letters",
        "the dead letters of a hang's last state",
    ),
];

/// The one raw read a wire verb makes as it stands: the log's commit counter IS the event
/// `log_changes` hands out. Any other [`raw_reads`] needle in a wire verb is red.
const WIRE_RAW: (&str, &str) = ("log_changes", concat!(".subscribe_", "log_seq("));

/// The barrier a reader over HTTP starts with (TV-A2): the fixture's `settle`, on the
/// colony itself or on the stage's colony.
const SETTLES: [&str; 4] = [
    concat!("self.settle", "().await;"),
    concat!("self.c.settle", "().await;"),
    concat!("s.c.settle", "().await;"),
    concat!("self.0.settle", "().await;"),
];

/// The kit's waits: a value read after the last of them is read at an event, not at rest.
const WAITS_ON: [&str; 7] = [
    concat!("wait::", "until"),
    concat!("deadline::", "hang"),
    concat!(".wait_", "until("),
    concat!(".wait_", "tree("),
    concat!(".watch_", "tree("),
    concat!("within_", "hang("),
    concat!(".wait_", "drawn("),
];

/// The fixture functions that may look without the barrier, each with its reason. The
/// mark `// no-settle: <reason>` above the function says so in the fixture; this list says
/// so in the pin, so a new exception is a change of both (re-check tv-bn M7b4 B-M1). One
/// that hands back a value hands it back as `Unsettled<…>`, unless it is a settling wait
/// or an event (re-check tv-bn M7b5 A-M2).
const NO_SETTLE: [(&str, &str); 16] = [
    (
        "Colony::drawn_after",
        "polls for the write's positive events; hands back the tree read at rest (a settling wait)",
    ),
    (
        "Colony::lost_write_now",
        "the account of a hang inside the failing wait; handed back as `Unsettled<String>`",
    ),
    (
        "Colony::watch_tree",
        "a tight watch for a positive event inside a window (809's fresh second); returns nothing",
    ),
    (
        "Colony::wait_tree",
        "polls for a positive event; hands back the tree read at rest (a settling wait)",
    ),
    (
        "Colony::wait_until",
        "a wait for a positive event; it returns nothing, its closure's reads are the caller's",
    ),
    (
        "Stage::tool_call",
        "a send and its positive wait under a possibly held request; returns nothing",
    ),
    (
        "Stage::out",
        "the event itself: the next message that left /alex",
    ),
    (
        "Stage::decided_on",
        "a wait for a positive event under a held request; returns nothing",
    ),
    (
        "Stage::trail",
        "the account of a red run inside failing waits; handed back as `Unsettled<String>`",
    ),
    (
        "Stage::ask",
        "the event itself: the next request the decider holds",
    ),
    (
        "Stage::journal_of",
        "polls for the row's positive event; hands back the row read at rest (a settling wait)",
    ),
    (
        "Stage::journal_of_held",
        "a turn's row while the HeldRequest argument holds the decider; handed back as `Unsettled<Value>`",
    ),
    (
        "Stage::pending_done",
        "polls for the row's positive event; hands back the row read at rest (a settling wait)",
    ),
    (
        "Stage::wait_drawn",
        "a wait for a positive event under a possibly held request; returns nothing",
    ),
    (
        "Stage::install_sample",
        "a send and its positive wait; returns nothing",
    ),
    (
        "warm",
        "a write and its positive wait (`wait_drawn`, listed); returns nothing",
    ),
];

/// The fixture functions that hand out an event itself (M7b7): a
/// message off the egress or a held request is no state read at rest, it is the thing a
/// lock waits for. A new one is listed here with its reason, or the pin is red.
const EVENTS: [(&str, &str); 3] = [
    (
        "Stage::out",
        "the next message that left /alex with the route asked for (`next_out`)",
    ),
    (
        "Stage::ask",
        "the next request the held decider receives (`recv`)",
    ),
    (
        "Colony::refusals",
        "the refusals off the egress until n are in hand (`next_out`), then every one once settled",
    ),
];

/// The doors of the fixtures that carry a raw read for someone else: the HTTP API a
/// browser asks, not a lock's read.
const DOORS: [(&str, &str); 3] = [
    (
        "seed_messages",
        "writes test data into the log of a colony of the test's own (the door's red line, tv-bn \
         M7b16), reads nothing",
    ),
    (
        "api_listener",
        "the colony's HTTP API for a browser: a request through it is the page's, not a lock's read",
    ),
    (
        "whole_messages",
        "the read behind the message route of `api_listener` (tv-bn M7b16): the page's read, paged",
    ),
];

/// The waits that hand back a value: each settles (or reads through a settling reader)
/// after its last wait, so the value is read at rest, not at the event.
const SETTLING_WAITS: [&str; 5] = [
    "Colony::drawn_after",
    "Colony::wait_tree",
    "Colony::refusals",
    "Stage::journal_of",
    "Stage::pending_done",
];

/// Whether `stmt` names `name` as a word (`r.log(`, `&r,`), not inside a longer one.
fn names_word(stmt: &str, name: &str) -> bool {
    stmt.match_indices(name).any(|(i, _)| {
        let before = stmt[..i].chars().next_back();
        let after = stmt[i + name.len()..].chars().next();
        before.is_none_or(|c| !ident_char(c)) && after.is_none_or(|c| !ident_char(c))
    })
}

/// Re-check tv-bn M7b5 H1: a reader bound settled (`let r = self.w.settled().await;`) and
/// read again after an await that is not its own -- a send, a kill, a wait: the colony may
/// have moved, and the second read is no longer at rest. Names the reader and the await.
fn read_after_move(f: &ApiFn) -> Option<(String, String)> {
    let bind_end = concat!(".settl", "ed().await;");
    let mut bound: Option<String> = None;
    let mut moved: Option<String> = None;
    for s in &f.body {
        if let Some(name) = bound.as_deref() {
            if names_word(s, name) {
                if let Some(m) = &moved {
                    return Some((name.to_string(), m.clone()));
                }
            } else if s.contains(".await") {
                moved = Some(s.clone());
            }
        }
        let flat: String = s.split_whitespace().collect();
        if flat.starts_with("let") && flat.ends_with(bind_end) {
            let rest = s.trim_start().trim_start_matches("let ").trim_start();
            let rest = rest.strip_prefix("mut ").unwrap_or(rest);
            let end = rest.find(|c: char| !ident_char(c)).unwrap_or(rest.len());
            bound = Some(rest[..end].to_string());
            moved = None;
        }
    }
    None
}

/// The settle pin over a set of fixture sources (see the note above [`OPENS_SETTLED`]).
struct SettleScan {
    hits: Vec<String>,
    /// Public fixture functions that read the colony (derived: they open the wire, hand
    /// out an event, ask over HTTP, or call one that does).
    readers: usize,
    /// The readers among them that are no listed exception and no event: settled by
    /// construction.
    settling: usize,
}

/// Whether `stmt` builds a `Reader` (`Reader(self)`, `Unsettled(Reader(self))`), not a
/// longer name ending in it.
fn builds_reader(stmt: &str) -> bool {
    let needle = concat!("Read", "er(");
    stmt.match_indices(needle)
        .any(|(i, _)| stmt[..i].chars().next_back().is_none_or(|c| !ident_char(c)))
}

/// The wire's own verbs against [`WIRE_VERBS`], every `Reader(` against
/// [`READER_BUILT_BY`], and `mod wire` holding nothing but `Wire` and `Reader` (re-check
/// tv-bn M7b9 A-M1 b).
fn wire_hits(all: &[(String, ApiFn)]) -> Vec<String> {
    let mut hits = Vec::new();
    for (file, f) in all {
        let key = f.key();
        if f.module == "wire" && !WIRE_OWNERS.contains(&f.owner.as_str()) {
            hits.push(format!(
                "{file}: {key} stands in `mod wire` but is no verb of Wire and no read of Reader"
            ));
        }
        let wire_verb = f.owner == "Wire";
        let builder = wire_verb && READER_BUILT_BY.contains(&f.name.as_str());
        if !builder && f.body.iter().any(|s| builds_reader(s)) {
            hits.push(format!(
                "{file}: {key} builds a Reader outside {READER_BUILT_BY:?}: a reader past the barrier"
            ));
        }
        // Re-check tv-bn M7b14 C-M2: `limit` as a word -- `limit: n` and the short form
        // `limit,` clippy asks for alike.
        let windowed = f.body.iter().any(|s| names_word(s, concat!("lim", "it")));
        if f.owner == "Reader" && windowed && !WINDOW_READERS.iter().any(|(n, _)| *n == f.name) {
            hits.push(format!(
                "{file}: {key} reads one page (`limit`): a window read as the whole log (C-M2)"
            ));
        }
        // C-M1 (sibling `rows_since` found in M7b14): a reader that hands out log rows
        // checks them inline itself, or hands out what such a reader of `Reader` read.
        // Re-check tv-bn M7b14 C-M2: any return type that names the row type hands out
        // rows (`Vec<…>`, `Option<…>`, a tuple), not the one spelling alone.
        let rows_out = f
            .ret
            .as_deref()
            .is_some_and(|r| names_word(r, "MessageLogDto"));
        let checked = f.body.iter().any(|s| {
            s.contains(concat!("inline_", "bodies("))
                || [".log(", ".whole_log(", ".trace("]
                    .iter()
                    .any(|r| s.contains(&format!("self{r}")))
        });
        if f.owner == "Reader"
            && rows_out
            && !checked
            && !WINDOW_READERS.iter().any(|(n, _)| *n == f.name)
        {
            hits.push(format!(
                "{file}: {key} hands out log rows unchecked for blobs: a blob body read past (C-M1)"
            ));
        }
        for (n, _) in WINDOW_READERS {
            let call = format!(".{n}(");
            let last_state = f.owner == "Wire" && f.name == "last_state";
            if !last_state && f.body.iter().any(|s| s.contains(&call)) {
                hits.push(format!(
                    "{file}: {key} calls the window reader `{n}` outside Wire::last_state: a window as a verdict"
                ));
            }
        }
        if !wire_verb {
            continue;
        }
        let Some((_, class, ret)) = WIRE_VERBS.iter().find(|(n, _, _)| *n == f.name) else {
            hits.push(format!(
                "{file}: {key} is no pinned wire verb (WIRE_VERBS): a new door to the colony past the readers"
            ));
            continue;
        };
        let has = f.ret.as_deref().unwrap_or("");
        if has != *ret {
            hits.push(format!(
                "{file}: {key} ({class}) hands back `{has}`, pinned `{ret}`"
            ));
        }
        let raw = raw_reads()
            .iter()
            .filter(|n| !(f.name == WIRE_RAW.0 && n.as_str() == WIRE_RAW.1))
            .any(|n| f.body.iter().any(|s| s.contains(n.as_str())));
        let root = f.body.iter().any(|s| reads_the_root(s));
        if raw || root || f.holds_literal(&RAW_READ_FILES) {
            hits.push(format!(
                "{file}: {key} ({class}) reads the colony raw: no read of its class"
            ));
        }
    }
    for (name, class, _) in WIRE_VERBS {
        if !all.iter().any(|(_, f)| f.owner == "Wire" && f.name == name) {
            hits.push(format!(
                "the pinned wire verb Wire::{name} ({class}) is no function of the fixtures"
            ));
        }
    }
    hits
}

fn settle_scan(fixtures: &[(&str, &str)]) -> SettleScan {
    let all: Vec<(String, ApiFn)> = fixtures
        .iter()
        .flat_map(|(file, src)| api_fns(src).into_iter().map(|f| (file.to_string(), f)))
        .collect();
    let mut hits = wire_hits(&all);
    let fns: Vec<(String, ApiFn)> = all
        .into_iter()
        .filter(|(_, f)| !(f.module == "wire" && WIRE_OWNERS.contains(&f.owner.as_str())))
        .collect();
    let door = |f: &ApiFn| DOORS.iter().any(|(k, _)| *k == f.key());
    let listed = |f: &ApiFn| NO_SETTLE.iter().any(|(k, _)| *k == f.key());
    let event = |f: &ApiFn| EVENTS.iter().any(|(k, _)| *k == f.key());
    let opens_unsettled = |f: &ApiFn| f.holds(&[OPENS_UNSETTLED]) || opens_a_value(f);
    // A reader opens the wire, opens an unsettled value, hands out an event, asks over
    // HTTP, or calls a reader, free or as a method (fixpoint).
    let mut reading: std::collections::BTreeSet<String> = fns
        .iter()
        .filter(|(_, f)| !door(f))
        .filter(|(_, f)| {
            f.holds(&[OPENS_SETTLED])
                || opens_unsettled(f)
                || f.holds(&EVENT_NEEDLES)
                || f.holds(&HTTP)
        })
        .map(|(_, f)| f.name.clone())
        .collect();
    loop {
        let more: Vec<String> = fns
            .iter()
            .filter(|(_, f)| !reading.contains(&f.name) && !door(f))
            .filter(|(_, f)| f.body.iter().any(|s| any_calls(s, &reading) > 0))
            .map(|(_, f)| f.name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        reading.extend(more);
    }
    // Re-check tv-bn M7b9 A-M1 (2): a reader settles by construction only when it opens
    // the wire settled or starts a barrier itself, or calls a reader that settles
    // (fixpoint). One that only calls exceptions -- a report, a watch, an unsettled
    // value handed on -- read through them without the barrier; before, every reader that
    // was no exception counted as settling. A settling wait rests after its last wait
    // (checked below), so a call to it settles. A function whose own line names it (an
    // unlisted opening, an unlisted event, a read over HTTP) is no exception its callers
    // read through: its hit stands at the root, not again at every caller.
    let excepted: std::collections::BTreeSet<String> = fns
        .iter()
        .filter(|(_, f)| (listed(f) || event(f)) && !SETTLING_WAITS.contains(&f.key().as_str()))
        .map(|(_, f)| f.name.clone())
        .collect();
    let settles_itself = |f: &ApiFn| {
        f.holds(&[OPENS_SETTLED]) || f.body.iter().any(|s| SETTLES.iter().any(|b| s.contains(b)))
    };
    let rooted = |f: &ApiFn| {
        !listed(f) && !event(f) && (opens_unsettled(f) || f.holds(&EVENT_NEEDLES) || f.holds(&HTTP))
    };
    let mut rests: std::collections::BTreeSet<String> = fns
        .iter()
        .filter(|(_, f)| reading.contains(&f.name))
        .filter(|(_, f)| {
            SETTLING_WAITS.contains(&f.key().as_str())
                || (!listed(f) && !event(f) && (settles_itself(f) || rooted(f)))
        })
        .map(|(_, f)| f.key())
        .collect();
    let rest_names = |rests: &std::collections::BTreeSet<String>| {
        fns.iter()
            .filter(|(_, f)| rests.contains(&f.key()) && !excepted.contains(&f.name))
            .map(|(_, f)| f.name.clone())
            .collect::<std::collections::BTreeSet<String>>()
    };
    loop {
        let names = rest_names(&rests);
        let more: Vec<String> = fns
            .iter()
            .filter(|(_, f)| reading.contains(&f.name) && !rests.contains(&f.key()))
            .filter(|(_, f)| !listed(f) && !event(f))
            .filter(|(_, f)| f.body.iter().any(|s| any_calls(s, &names) > 0))
            .map(|(_, f)| f.key())
            .collect();
        if more.is_empty() {
            break;
        }
        rests.extend(more);
    }
    // The readers that settle, by name, for the waits' check below; a name an exception
    // also carries is left out, so a call to it never counts as settling.
    let settling = rest_names(&rests);
    // The barrier as the first statement, bare or binding its proof for the address
    // (`let at_rest = self.settle().await;`, OR-T11-25 G2).
    let first_settles = |f: &ApiFn| {
        f.body.first().is_some_and(|s| {
            let s = s.trim();
            let s = s.strip_prefix("let at_rest = ").unwrap_or(s);
            SETTLES.contains(&s)
        })
    };
    let (mut readers, mut settled) = (0, 0);
    for (file, f) in &fns {
        let key = f.key();
        let marked = f.marks.iter().any(|m| m.starts_with("// no-settle: "));
        let (is_listed, is_event) = (listed(f), event(f));
        let reads = reading.contains(&f.name) && !door(f);
        // (a) An unsettled opening has its mark AND its line; a mark or a line alone is red.
        if marked && !is_listed {
            hits.push(format!(
                "{file}: {key} is marked no-settle but not listed in the pin"
            ));
        } else if is_listed && !marked {
            hits.push(format!("{file}: {key} is listed no-settle but not marked"));
        } else if !is_listed && opens_unsettled(f) {
            hits.push(format!(
                "{file}: {key} opens the wire unsettled but is neither marked no-settle nor listed (TV-A2)"
            ));
        } else if reads && !is_listed && !is_event && !rests.contains(&key) {
            hits.push(format!(
                "{file}: {key} reads through an exception without settling: neither `.settled()`, a barrier, nor a settling reader (A-M1 (2))"
            ));
        }
        // (c) An event handed out as it is, listed with its reason.
        let delivers = f.holds(&EVENT_NEEDLES);
        if delivers && !is_event {
            hits.push(format!(
                "{file}: {key} delivers the event itself but is not listed in EVENTS"
            ));
        } else if is_event && !delivers {
            hits.push(format!(
                "{file}: {key} is listed in EVENTS but delivers no event"
            ));
        }
        // (b) Re-check tv-bn M7b5 A-M2: an exception that hands back a value hands it back
        // unsettled, so the caller has to name its reason; a settling wait hands back what
        // it read at rest after its last wait.
        let wait = SETTLING_WAITS.contains(&key.as_str());
        if wait {
            let last = f
                .body
                .iter()
                .rposition(|s| WAITS_ON.iter().any(|w| s.contains(w)));
            let after = last.map_or(&f.body[..], |i| &f.body[i + 1..]);
            let at_rest = after.iter().any(|s| {
                s.contains(OPENS_SETTLED)
                    || SETTLES.iter().any(|b| s.contains(b))
                    || any_calls(s, &settling) > 0
            });
            if !at_rest {
                hits.push(format!(
                    "{file}: {key} hands back a value read at the event, not after the colony settled"
                ));
            }
        } else if is_listed
            && !is_event
            && f.ret
                .as_deref()
                .is_some_and(|r| !r.starts_with("Unsettled<"))
        {
            hits.push(format!(
                "{file}: {key} hands back a value read without the barrier, and not as `Unsettled<…>` (A-M2)"
            ));
        }
        // H1: a settled reader read again after the colony may have moved.
        if let Some((name, at)) = read_after_move(f) {
            hits.push(format!(
                "{file}: {key} reads the settled reader `{name}` after `{at}` (H1: settle again after a move)"
            ));
        }
        // A raw read outside the wire is a door's or nobody's.
        let raw = raw_reads();
        let raw: Vec<&str> = raw.iter().map(String::as_str).collect();
        let root = f.body.iter().any(|s| reads_the_root(s));
        if !door(f) && (f.holds(&raw) || root || f.holds_literal(&RAW_READ_FILES)) {
            hits.push(format!("{file}: {key} reads the colony raw, past the wire"));
        }
        if !f.public || !reads {
            continue;
        }
        readers += 1;
        if is_listed || is_event {
            continue;
        }
        settled += 1;
        // HTTP is no wire: a reader over it settles first.
        if f.holds(&HTTP) && !first_settles(f) {
            hits.push(format!(
                "{file}: {key} reads over HTTP without settling first (TV-A2)"
            ));
        }
    }
    for (k, _) in NO_SETTLE {
        if !fns.iter().any(|(_, f)| f.key() == k) {
            hits.push(format!(
                "the listed exception {k} is no function of the fixtures"
            ));
        }
    }
    for (k, _) in EVENTS {
        if !fns.iter().any(|(_, f)| f.key() == k) {
            hits.push(format!(
                "the listed event {k} is no function of the fixtures"
            ));
        }
    }
    for (k, _) in DOORS {
        if !fns.iter().any(|(_, f)| f.key() == k) {
            hits.push(format!(
                "the listed door {k} is no function of the fixtures"
            ));
        }
    }
    for k in SETTLING_WAITS {
        if !fns.iter().any(|(_, f)| f.key() == k) {
            hits.push(format!(
                "the listed wait {k} is no function of the fixtures"
            ));
        }
    }
    hits.extend(sealed_forms(fixtures));
    SettleScan {
        hits,
        readers,
        settling: settled,
    }
}

/// Where the fixtures mint a sealed value, as (needle, file, count, why). The compiler keeps
/// every other module from minting one; inside the fixture these are the places (early
/// warning: a second `Settled::new` would be a proof without a barrier).
const SEALED_MINTS: [(&str, &str, usize, &str); 5] = [
    (
        concat!("Settled::", "new("),
        "support/display_colony.rs",
        0,
        "none: only `sealed::barrier::settle` mints it, once the colony answered (M7b12 A-M1)",
    ),
    (
        concat!("Door::", "new("),
        "support/display_colony.rs",
        1,
        "`boot_inner`, with the two listeners it bound",
    ),
    (
        concat!("Key::", "new("),
        "support/display_colony.rs",
        1,
        "`boot_growing`, the fixture's own growth verb",
    ),
    (
        concat!("Key::", "new("),
        "support/presenter_colony.rs",
        1,
        "`stage`, the presenter's boot",
    ),
    (
        concat!("Sealed::", "new("),
        "support/presenter_colony.rs",
        1,
        "`stage`, the held decider",
    ),
];

/// The sealed core's forms in the fixtures (OR-T11-25 G1, re-check tv-bn M7b11 A-M1 ii):
/// `mod sealed` is private and from `sealed.rs`, no fixture defines the sealed types again,
/// the mints stand where [`SEALED_MINTS`] says, and `mod wire` holds no nested module and no
/// macro -- a function there the pin does not read (`api_fns` reads two levels) could open
/// the wire's raw fields, which a child module of `wire` sees.
fn sealed_forms(fixtures: &[(&str, &str)]) -> Vec<String> {
    let mut hits = Vec::new();
    for (file, src) in fixtures {
        let code = code_only(src);
        for twin in [
            concat!("struct ", "Unsettled"),
            concat!("enum ", "Why"),
            concat!("struct ", "Settled"),
            concat!("struct ", "Door"),
            concat!("struct ", "Key"),
        ] {
            if code.contains(twin) {
                hits.push(format!(
                    "{file}: defines `{twin}` again, past the sealed core"
                ));
            }
        }
        for (needle, at, want, _) in SEALED_MINTS {
            let n = code.matches(needle).count();
            if at == *file && n != want {
                hits.push(format!(
                    "{file}: `{needle}` stands {n} time(s), SEALED_MINTS pins {want}"
                ));
            } else if at != *file
                && !SEALED_MINTS
                    .iter()
                    .any(|(m, a, _, _)| *m == needle && a == file)
                && n > 0
            {
                hits.push(format!(
                    "{file}: mints `{needle}` {n} time(s), SEALED_MINTS pins none here"
                ));
            }
        }
        if *file == FIXTURES[0] {
            let declared = code.lines().any(|l| l.trim() == "mod sealed;");
            if !declared || !src.contains(concat!("#[path = \"sea", "led.rs\"]")) {
                hits.push(format!(
                    "{file}: `mod sealed;` from `sealed.rs`, private, is not declared"
                ));
            }
            if let Some(at) = code.find("mod wire {") {
                let body = &code[at + "mod wire {".len()..];
                let mut depth = 1i32;
                let mut end = body.len();
                for (i, c) in body.char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                for (n, line) in body[..end].lines().enumerate() {
                    let t = line.trim_start();
                    let nested = mod_opened(t).is_some()
                        || t.starts_with("mod ")
                        || t.contains(concat!("macro_", "rules!"));
                    if nested {
                        hits.push(format!(
                            "{file}: `mod wire` holds a nested module or a macro (line {} of the wire): a function the pin does not read",
                            n + 1
                        ));
                    }
                }
            } else {
                hits.push(format!("{file}: no `mod wire` stands: the pin is blind"));
            }
        }
    }
    hits
}

/// The two fixtures every lock of the display builds on, as paths under `tests/`.
const FIXTURES: [&str; 2] = ["support/display_colony.rs", "support/presenter_colony.rs"];

/// The two fixtures every lock of the display builds on, as (path under `tests/`, source).
fn the_fixtures() -> Vec<(String, String)> {
    FIXTURES
        .iter()
        .map(|p| {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join(p);
            (
                p.to_string(),
                std::fs::read_to_string(&path).expect("a fixture reads"),
            )
        })
        .collect()
}

/// What a user of the fixtures reads past its readers: the unsettled view, the egress, the
/// decider, a client of its own (HTTP, websocket, raw socket) and every raw read of the
/// colony [`raw_reads`] derives -- the `ColonyMsg` readers and every reader of
/// `ColonyHandle` on ANY receiver (re-check tv-bn M7b11 A-M5: `handle.log_since(` was no
/// hit; the display fixture's settling readers carry other names since, `mark_log` /
/// `rows_since`, so the needle needs no receiver). Split so the pin does not find itself.
///
/// **An early warning, not the guarantee** (OR-T11-25): against the fixtures none of these
/// reaches the colony -- the handle, the egress, the decider and the unsettled view are
/// sealed (the compile probe `the_sealed_core_refuses_every_way_around_it`), and the
/// colony's ADDRESS is handed out by the one door only (a named [`display_colony::Site`]
/// per call site, bound to its file at run time). They stay because a user can boot a
/// colony of its OWN and read it raw (808, listed below), and a support file other than
/// the two fixtures could hand a raw door around. Chains are joined first (`s.c.h` /
/// `.log_since(` on the next line is one read).
fn past_the_readers_needles() -> Vec<String> {
    let mut needles: Vec<String> = [
        concat!(".unsett", "led("),
        concat!(".egr", "ess"),
        concat!(".deci", "der"),
        // Clients by their crate's name, so an alias (`use reqwest as r`) and a sibling
        // crate (`hyper_util`) are hits too (re-check tv-bn M7b11 A-M3).
        concat!("reqw", "est"),
        concat!("hyp", "er"),
        concat!("connect_", "async"),
        concat!("tungstenite::", "connect"),
        concat!("TcpStream::", "connect"),
        concat!("Voice", "Client"),
    ]
    .iter()
    .map(|n| n.to_string())
    .collect();
    needles.extend(raw_reads());
    needles
}

/// The paths a user reads the colony by, past the readers (re-check tv-bn M7b9 A-M2): the
/// API's log route and the colony's database files. A path is a literal, so these are
/// read with the literals kept ([`code_and_literals`]), comments out. Split so the pin
/// does not find itself.
const PAST_THE_READERS_PATHS: [&str; 3] = [
    concat!("colony/", "messages"),
    concat!("colony", ".db"),
    concat!("cell", ".db"),
];

/// The users that may read past the readers, per (file, needle): the LOCAL DOORS that hold
/// the needle (the only functions of the file that may), the number of CALL SITES of those
/// doors and of every function that calls them (fixpoint), and the reason: they read a
/// colony of their own, or no colony state at all. Call sites, not statements (re-check
/// tv-bn M7b11 A-M4): a further `get_text(…)` or `s.tree()` behind a door is a new read
/// and moves the count, even though the door's own statements stand as they did.
const PAST_THE_READERS_ALLOWED: [(&str, &str, &[&str], usize, &str); 3] = [
    (
        "808_the_colony_view_stands_in_the_dock.rs",
        concat!(".read", "_log("),
        &["patches", "touches"],
        11,
        "boots its own colony (`h`) and reads its own log (`patches` for the tree, `touches` for \
         the colony view's stamp); it takes only helpers from the fixture",
    ),
    (
        "808_the_colony_view_stands_in_the_dock.rs",
        concat!(".drain_dead", "_letters("),
        &["wait_tree"],
        5,
        "boots its own colony (`h`) and drains its own dead letters for a hang's text",
    ),
    (
        "gh867_no_surface_page_carries_unlisted_inline_script.rs",
        concat!("reqw", "est"),
        &["get_text"],
        7,
        "`get_text` GETs files as they ship, no colony state: the page of a web cell of the test's \
         own on its own `surface_listener` (two calls); the render lock's own `web` cells \
         (`WebPage::get_route`, called by `render_canvy` and `render_web_template`, each called \
         once by the lock: five sites, m8 M9e); the voice test page and its files come from the \
         fixture (`voice_files`), the composed display pages through `page`, which settles",
    ),
];

/// This file's own reader classification ([`reads_the_colony`]) names the `ColonyMsg`
/// readers it derives; the pin reads this file without it.
fn without_the_classification(src: &str) -> String {
    let begin = concat!("// BEGIN reads_", "the_colony");
    let end = concat!("// END reads_", "the_colony");
    match (src.find(begin), src.find(end)) {
        (Some(b), Some(e)) if b < e => {
            let kept: String = src[b..e].lines().map(|_| "\n").collect();
            format!("{}{}{}", &src[..b], kept, &src[e..])
        }
        _ => src.to_string(),
    }
}

/// The local doors of `src` for `needle` (the functions whose body holds it), and the call
/// sites of the doors and of every function that calls one (fixpoint), and the needle's
/// occurrences outside every door.
fn local_doors(src: &str, needle: &str) -> (Vec<String>, usize, usize) {
    let fns = api_fns(src);
    let doors: Vec<String> = fns
        .iter()
        .filter(|f| f.holds(&[needle]))
        .map(|f| f.name.clone())
        .collect();
    let mut closure: std::collections::BTreeSet<String> = doors.iter().cloned().collect();
    loop {
        let more: Vec<String> = fns
            .iter()
            .filter(|f| !closure.contains(&f.name))
            .filter(|f| f.body.iter().any(|s| any_calls(s, &closure) > 0))
            .map(|f| f.name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        closure.extend(more);
    }
    let sites = fns
        .iter()
        .flat_map(|f| f.body.iter())
        .map(|s| any_calls(s, &closure))
        .sum();
    let inside: usize = fns
        .iter()
        .filter(|f| doors.contains(&f.name))
        .flat_map(|f| f.body.iter())
        .map(|s| s.matches(needle).count())
        .sum();
    let outside = code_only(src)
        .matches(needle)
        .count()
        .saturating_sub(inside);
    (doors, sites, outside)
}

/// Every read of `src` past the fixtures' readers, as `name:line: code`. Only the two
/// fixtures are skipped; every other support file is a user like any test (re-check tv-bn
/// M7b5 M1 c: `support/*` was skipped whole, so a helper there was invisible). A needle
/// [`PAST_THE_READERS_ALLOWED`] names for the file is no hit while its count holds; a count
/// that moved is one.
fn past_the_readers(name: &str, src: &str) -> Vec<String> {
    if FIXTURES.contains(&name) {
        return Vec::new();
    }
    let src = &without_the_classification(src);
    let needles = past_the_readers_needles();
    // A method chain broken over lines (`s.c.h` then `.log_since(…)`) is one read: join every
    // line that starts with `.` onto the chain's head and report the head's line. The paths
    // are read off the same lines of the source with its literals kept (same line breaks).
    let mut code: Vec<(usize, String)> = Vec::new();
    for (n, l) in code_only(src).lines().enumerate() {
        match code.last_mut() {
            Some((_, head)) if l.trim_start().starts_with('.') => head.push_str(l.trim()),
            _ => code.push((n, l.trim().to_string())),
        }
    }
    let lit_src = code_and_literals(src);
    let lit_lines: Vec<&str> = lit_src.lines().collect();
    let allowed: Vec<(&str, &[&str], usize)> = PAST_THE_READERS_ALLOWED
        .iter()
        .filter(|(f, _, _, _, _)| *f == name)
        .map(|(_, n, d, c, _)| (*n, *d, *c))
        .collect();
    let mut hits = Vec::new();
    for (i, (n, l)) in code.iter().enumerate() {
        let end = code.get(i + 1).map_or(lit_lines.len(), |(m, _)| *m);
        let lit = lit_lines[(*n).min(lit_lines.len())..end.min(lit_lines.len())].join(" ");
        let found: Vec<&str> = needles
            .iter()
            .map(String::as_str)
            .filter(|x| l.contains(*x))
            .chain(
                PAST_THE_READERS_PATHS
                    .iter()
                    .copied()
                    .filter(|x| lit.contains(*x)),
            )
            .collect();
        let mut past = Vec::new();
        for x in found {
            if !allowed.iter().any(|(a, _, _)| *a == x) {
                past.push(x);
            }
        }
        if !past.is_empty() {
            hits.push(format!("{name}:{}: {l} ({})", n + 1, past.join(", ")));
        }
    }
    for (needle, doors, count) in allowed {
        let (found, sites, outside) = local_doors(src, needle);
        if found.iter().map(String::as_str).ne(doors.iter().copied()) {
            hits.push(format!(
                "{name}: `{needle}` stands in the functions {found:?}, PAST_THE_READERS_ALLOWED pins the doors {doors:?}"
            ));
        }
        if outside > 0 {
            hits.push(format!(
                "{name}: `{needle}` stands {outside} time(s) outside a function the pin reads"
            ));
        }
        if sites != count {
            hits.push(format!(
                "{name}: the doors of `{needle}` are called at {sites} site(s), PAST_THE_READERS_ALLOWED pins {count}"
            ));
        }
    }
    hits
}

/// Every file [`PAST_THE_READERS_ALLOWED`] names that is no user of the fixtures: an
/// allowance for nobody would stand unseen (its count is checked only on a user).
fn allowed_without_user(users: &[(String, String)]) -> Vec<String> {
    PAST_THE_READERS_ALLOWED
        .iter()
        .filter(|(f, _, _, _, _)| !users.iter().any(|(n, _)| n.as_str() == *f))
        .map(|(f, n, _, _, _)| {
            format!("{f} `{n}` is allowed past the readers but is no user of the fixtures")
        })
        .collect()
}

/// Every place a user of the fixtures opens an unsettled value, as (file, `Why` variant,
/// reason). Re-check tv-bn M7b5 A-M2: an exception that hands back a value hands it back
/// as `Unsettled<…>`, so its caller has to name a reason -- and the reasons are pinned
/// here, so a NEW site (say, `journal_of_held` under an absence claim) is a change of the
/// test AND this list, and a site that is gone takes its line with it.
const USER_REASONS: [(&str, &str, &str); 2] = [
    (
        "gh959_an_unsure_or_late_verdict_shows_nothing.rs",
        "Held",
        "the held turn's own journal row while the test holds the decider (`journal_of_held`); \
         a positive claim about that row (`fallback == timeout`), never one about the rest",
    ),
    (
        "display/gh1210_a_gesture_reaches_the_member_exactly_once.rs",
        "Report",
        "the log writer's commit counter (`LogChanges::at`) in the text of a HANG; never a verdict",
    ),
];

/// Every `.because(…)` of the users (the two fixtures aside) that [`USER_REASONS`] does
/// not pin, and every pinned reason without its site. Read as code: a reason in a literal
/// or a comment is none.
fn user_reason_hits(users: &[(String, String)]) -> Vec<String> {
    let needle = concat!(".because", "(");
    let mut found: Vec<(String, String)> = Vec::new();
    let mut hits = Vec::new();
    for (name, src) in users {
        if FIXTURES.contains(&name.as_str()) {
            continue;
        }
        let code = code_only(src);
        for (n, line) in code.lines().enumerate() {
            for (i, _) in line.match_indices(needle) {
                let arg = &line[i + needle.len()..];
                let arg = arg.split(')').next().unwrap_or("").trim();
                let why = arg.strip_prefix("Why::").map_or(arg, |v| {
                    let end = v.find(|c: char| !ident_char(c)).unwrap_or(v.len());
                    &v[..end]
                });
                let pinned = USER_REASONS
                    .iter()
                    .any(|(f, w, _)| *f == name.as_str() && *w == why);
                if !pinned {
                    hits.push(format!(
                        "{name}:{}: opens an unsettled value for `{why}` without its line in USER_REASONS: {}",
                        n + 1,
                        line.trim()
                    ));
                }
                found.push((name.clone(), why.to_string()));
            }
        }
    }
    for (f, w, _) in USER_REASONS {
        if !found.iter().any(|(n, v)| n == f && v == w) {
            hits.push(format!("the pinned reason {f} `{w}` has no site"));
        }
    }
    hits
}

/// Re-check tv-bn M7b5 A-M1/A-M2 on the M7b7 form: every fixture function that opens the
/// wire unsettled is marked AND listed, an exception that hands back a value hands it back
/// as `Unsettled<…>` (or is a settling wait), an event is listed, and a settled reader is
/// not read again after a move. The guarantee is the compiler (the wire is private, tests
/// reach the colony only through these functions); this is the early warning for the
/// fixture side, with a floor so a scan that found no reader is blind, not clean.
#[test]
fn every_reader_of_the_fixtures_settles_before_it_reads() {
    let fixtures = the_fixtures();
    let refs: Vec<(&str, &str)> = fixtures
        .iter()
        .map(|(n, s)| (n.as_str(), s.as_str()))
        .collect();
    let scan = settle_scan(&refs);
    eprintln!(
        "settle pin: {} readers, {} settling",
        scan.readers, scan.settling
    );
    // Floors at the M7b7 count (58 readers, 43 of them settled by construction, 15 listed
    // or events), a few below so a fixture may lose a reader without a pin change; the
    // M7b5 floors (30/20 at 56/41) let half the fixture vanish unseen.
    assert!(
        scan.readers >= 50 && scan.settling >= 38,
        "the settle pin is blind: {} readers, {} settling",
        scan.readers,
        scan.settling
    );
    assert!(
        scan.hits.is_empty(),
        "a fixture function that reads past the barrier:\n{}",
        scan.hits.join("\n")
    );
}

/// The variants of the fixture's `Site` (OR-T11-25 G2), read from its `pub enum Site`.
fn site_variants(display: &str) -> Vec<String> {
    let code = code_only(display);
    let Some(at) = code.find("pub enum Site {") else {
        return Vec::new();
    };
    let body = &code[at + "pub enum Site {".len()..];
    let body = &body[..body.find('}').unwrap_or(body.len())];
    body.lines()
        .map(str::trim)
        .filter_map(|l| l.strip_suffix(','))
        .filter(|v| !v.is_empty() && v.chars().all(ident_char))
        .map(str::to_string)
        .collect()
}

/// The early warning beside the one address door (OR-T11-25 G2, re-check tv-bn M7b11
/// A-M3/A-M4): every `Site` variant stands in exactly ONE place over all users, every
/// `Site::` stands right in a site call (`.drive(Site::…`, `.voice_files(Site::…`; a site
/// bound to a name could be handed to two calls), and no user
/// renames or globs the fixture's items. The guarantee is the door: no address without a
/// site, a site's file checked at run time, a second place of one site a panic.
fn site_hits(users: &[(String, String)], display: &str) -> Vec<String> {
    let variants = site_variants(display);
    let mut hits = Vec::new();
    if variants.len() < 6 {
        hits.push(format!(
            "the sites of the fixture are not read: {variants:?}"
        ));
    }
    let mut seen: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    // This file asks with a foreign site on purpose (`the_door_refuses_a_site_of_another_file`,
    // refused at run time) and holds `Site::ALL` for `a_site_admits_only_its_own_file`.
    let pin = concat!("809_a_restart_reads_once", "_and_draws_the_same_screen.rs");
    for (name, src) in users {
        if FIXTURES.contains(&name.as_str()) || name.as_str() == pin {
            continue;
        }
        let code = code_only(&without_the_classification(src));
        for (n, line) in code.lines().enumerate() {
            if line.contains(concat!("Site ", "as"))
                || line.contains(concat!("display_colony::", "*"))
            {
                hits.push(format!(
                    "{name}:{}: renames or globs the fixture's items: {}",
                    n + 1,
                    line.trim()
                ));
            }
            for (i, _) in line.match_indices(concat!("Site", "::")) {
                if line[..i].chars().next_back().is_some_and(ident_char) {
                    continue;
                }
                let v = &line[i + "Site::".len()..];
                let v = &v[..v.find(|c: char| !ident_char(c)).unwrap_or(v.len())];
                // `Site::ALL` is the pin's own list; a user that walks every site is after
                // every address.
                if v == "ALL" {
                    hits.push(format!(
                        "{name}:{}: walks every site: {}",
                        n + 1,
                        line.trim()
                    ));
                    continue;
                }
                let head = &line[..i];
                let in_call = [".drive(", ".voice_files("]
                    .iter()
                    .any(|c| head.ends_with(c));
                if !in_call {
                    hits.push(format!(
                        "{name}:{}: `Site::{v}` stands outside an address call: {}",
                        n + 1,
                        line.trim()
                    ));
                }
                seen.entry(v.to_string())
                    .or_default()
                    .push(format!("{name}:{}", n + 1));
            }
        }
    }
    for v in &variants {
        match seen.get(v).map(Vec::len).unwrap_or(0) {
            1 => {}
            0 => hits.push(format!("the site `{v}` has no place in a user")),
            k => hits.push(format!(
                "the site `{v}` stands in {k} places: {:?}",
                seen[v]
            )),
        }
    }
    for v in seen.keys() {
        if !variants.contains(v) {
            hits.push(format!("`Site::{v}` is no site of the fixture"));
        }
    }
    hits
}

/// Re-check tv-bn M7b4 A-M1 (third form) / M7b5 A-M2: a user that reads the colony past the
/// readers (the log or egress off a handle) is past the barrier, too, and every reason a
/// user gives to open an unsettled value is pinned with its site.
#[test]
fn no_user_of_the_fixtures_reads_the_colony_past_their_readers() {
    let users = fixture_users();
    let hits: Vec<String> = users
        .iter()
        .flat_map(|(n, s)| past_the_readers(n, s))
        .collect();
    assert!(users.len() > 30, "the users are found: {}", users.len());
    let hits: Vec<String> = hits
        .into_iter()
        .chain(allowed_without_user(&users))
        .collect();
    assert!(
        hits.is_empty(),
        "a read of the colony past the fixtures' readers:\n{}",
        hits.join("\n")
    );
    let reasons = user_reason_hits(&users);
    assert!(
        reasons.is_empty(),
        "an unsettled value opened by a user without its pinned reason:\n{}",
        reasons.join("\n")
    );
    let display = &the_fixtures()[0].1;
    let sites = site_hits(&users, display);
    assert!(
        sites.is_empty(),
        "the colony's address past its one door:\n{}",
        sites.join("\n")
    );
}

/// The export marker, probed and never read (`gh80_shipped_conditions_are_guarded`):
/// `plans/` is an export-forbidden prefix, so only the private tree has it.
const PRIVATE_ONLY_MARKER: &str = "plans";

/// M7b14 (browser-station-lock, `scripts/gate_plan.py`): a driver site's unit names its
/// driver script itself -- the gate plans a browser lock by the driver it names, and the
/// shared fixture names none -- and it is the script `Colony::drive` starts for the site
/// (`workshop/tools/display-<name>-browser.mjs`). A unit naming another script panics in
/// `drive` at run time; this reads it without a browser.
#[test]
fn every_driver_site_names_the_script_the_fixture_starts() {
    let display = &the_fixtures()[0].1;
    let mut wrong = Vec::new();
    let mut drivers = 0;
    for site in Site::ALL {
        let head = format!("Site::{site:?} => Some((Launch::");
        let Some(at) = display.find(&head) else {
            continue;
        };
        let rest = &display[at + head.len()..];
        let name = rest.split('"').nth(1).unwrap_or_default();
        drivers += 1;
        let script = format!("workshop/tools/display-{name}-browser.mjs");
        let unit = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join(site.file());
        let src = std::fs::read_to_string(&unit).unwrap_or_default();
        if !src.contains(&format!("\"{script}\"")) || !src.contains(".script(") {
            wrong.push(format!(
                "{site:?}: {} does not name `{script}`",
                site.file()
            ));
        }
        // The script lives under `workshop/`, which only the private tree carries: there it
        // exists, or `drive` has nothing to start; the public export names it and stops here.
        if !display_colony::repo(&script).is_file()
            && display_colony::repo(PRIVATE_ONLY_MARKER).is_dir()
        {
            wrong.push(format!("{site:?}: `{script}` is not in this tree"));
        }
    }
    assert_eq!(drivers, 5, "five driver sites, the voice site starts none");
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// OR-T11-25 G2: a site admits its own file and no other (re-check tv-bn M7b11 A-M3: a new
/// way to the address through `colony.url(Site::X, …)` from another file panics at run
/// time), names a reason, and `Site::ALL` is every variant the fixture declares.
#[test]
fn a_site_admits_only_its_own_file() {
    let display = &the_fixtures()[0].1;
    let variants = site_variants(display);
    let all: Vec<String> = Site::ALL.iter().map(|s| format!("{s:?}")).collect();
    assert_eq!(all, variants, "Site::ALL is every site, in order");
    let here = concat!(
        "crates/meclaw-cells/tests/809_a_restart_reads_once",
        "_and_draws_the_same_screen.rs"
    );
    for site in Site::ALL {
        let own = format!("crates/meclaw-cells/tests/{}", site.file());
        let line = site.line();
        assert!(site.admits(&own, line).is_ok(), "{site:?} admits {own}");
        assert!(
            site.admits(site.file(), line).is_ok(),
            "{site:?} admits its bare file"
        );
        assert!(
            site.admits(here, line).is_err(),
            "{site:?} admits a foreign file"
        );
        let suffix = format!("crates/meclaw-cells/tests/x_{}", site.file());
        assert!(
            site.admits(&suffix, line).is_err(),
            "{site:?} admits {suffix}"
        );
        // Re-check tv-bn M7b12 A-M2: a second place of the site in its own file -- another
        // test, another process under nextest -- is refused by its line.
        assert!(
            site.admits(&own, line + 1).is_err(),
            "{site:?} admits a second line of its file"
        );
        assert!(!site.reason().trim().is_empty(), "{site:?} names a reason");
        // The register is the source: the line holds the site's call, so a site whose test
        // returns early on this host (no browser) still stands where the register says.
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join(site.file()),
        )
        .expect("the site's file reads");
        let at = src
            .lines()
            .nth(line as usize - 1)
            .unwrap_or_default()
            .to_string();
        assert!(
            at.contains(&format!("Site::{site:?}")),
            "{site:?} stands at {}:{line} by the register, which holds: {at}",
            site.file()
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The channel pin
// ─────────────────────────────────────────────────────────────────────────────

/// The waits a channel poll hides in (TV-S1), split so the pin does not find itself.
const POLLED_WAITS: [&str; 3] = [
    concat!("within_", "hang("),
    concat!("wait::", "until"),
    concat!(".wait_", "until("),
];

/// Every `try_recv` of `src` inside the closure of a kit wait: a channel polled at the
/// poll interval where the event is the channel's own `recv` (TV-S1). The region of a wait
/// runs from its call to the line where its parentheses close, read as code (literals and
/// comments out, [`code_only`]). The second value counts the regions read.
fn polled_channels(name: &str, src: &str) -> (Vec<String>, usize) {
    let needle = concat!("try_", "recv(");
    let code = code_only(src);
    let lines: Vec<&str> = code.lines().map(str::trim).collect();
    let mut hits = Vec::new();
    let mut regions = 0;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(at) = POLLED_WAITS.iter().filter_map(|w| line.find(w)).min() else {
            i += 1;
            continue;
        };
        regions += 1;
        let mut depth = 0i32;
        let mut opened = false;
        let mut j = i;
        let mut rest = &line[at..];
        loop {
            if rest.contains(needle) {
                hits.push(format!("{name}:{}: {}", j + 1, lines[j]));
            }
            for c in rest.chars() {
                match c {
                    '(' => {
                        depth += 1;
                        opened = true;
                    }
                    ')' => depth -= 1,
                    _ => {}
                }
            }
            if (opened && depth <= 0) || j + 1 == lines.len() {
                break;
            }
            j += 1;
            rest = lines[j];
        }
        i = j + 1;
    }
    (hits, regions)
}

/// Re-check tv-bn M7b M1 (TV-S1): no user of the display fixture waits for a message by
/// polling its channel. `presenter_colony.rs` `out` and `ask` took `try_recv` under
/// `within_hang`, so the wait woke on the poll interval instead of the message, and an
/// empty channel was indistinguishable from a slow one; the event is `recv` under
/// `deadline::hang_with` (as `Colony::refusals`). Re-check tv-bn M7b3 M1: with a floor --
/// a scan that found no wait region at all is blind, not clean.
#[test]
fn no_user_of_the_display_fixture_polls_a_channel_in_a_wait() {
    let users = fixture_users();
    let (mut hits, mut regions) = (Vec::new(), 0);
    for (name, src) in &users {
        let (h, r) = polled_channels(name, src);
        hits.extend(h);
        regions += r;
    }
    eprintln!("channel pin: {} users, {regions} wait regions", users.len());
    assert!(
        users.len() > 30,
        "the fixture and its users are found: {}",
        users.len()
    );
    assert!(regions >= 20, "the wait regions are found: {regions}");
    assert!(
        hits.is_empty(),
        "a channel polled inside a wait, past its recv:\n{}",
        hits.join("\n")
    );
}

/// The channel pin is sharp: a poll inside a wait is found, a `)` in a literal or a comment
/// does not close its region early, the event form and quoted needles are no hits.
#[test]
fn the_channel_pin_finds_what_it_names() {
    let polled = concat!(
        "let got = meclaw_testing::wait::within_",
        "hang(|| async {\n",
        "    match decider.borrow_mut().try_",
        "recv() {\n",
        "        Ok(held) => Some(held),\n",
        "        Err(_) => None,\n",
        "    }\n",
        "})\n",
        ".await;\n",
        "while let Ok(m) = colony.egress.try_",
        "recv() {}\n"
    );
    assert_eq!(polled_channels("x.rs", polled).0.len(), 1, "{polled}");
    // H2: a `)` in a literal or a comment no longer closes the region early.
    let quoted = concat!(
        "meclaw_testing::wait::within_",
        "hang(|| async { // )\n",
        "    eprintln!(\")\");\n",
        "    rx.try_",
        "recv().ok()\n",
        "}).await;\n"
    );
    assert_eq!(polled_channels("x.rs", quoted).0.len(), 1, "{quoted}");
    let event = concat!(
        "let got = async move { decider.recv().await };\n",
        "meclaw_testing::deadline::hang_with(\"asked\", got, || 0).await;\n",
        "while let Ok(m) = colony.egress.try_",
        "recv() {}\n",
        "// within_",
        "hang(|| async { rx.try_",
        "recv() })\n",
        "let note = \"within_",
        "hang(|| rx.try_",
        "recv())\";\n"
    );
    assert_eq!(polled_channels("x.rs", event).0, Vec::<String>::new());
    assert_eq!(
        polled_channels("x.rs", "").1,
        0,
        "an empty source has no region"
    );
}

/// `src` with the one occurrence of `from` replaced by `to`; a mutant that does not apply
/// is a broken test, not a green one.
fn mutate(src: &str, from: &str, to: &str) -> String {
    assert_eq!(
        src.matches(from).count(),
        1,
        "the mutant applies once: {from}"
    );
    src.replacen(from, to, 1)
}

/// The settle pin over a mutated display fixture and the presenter fixture as it stands.
fn settle_hits(display: &str, presenter: &str) -> Vec<String> {
    settle_scan(&[
        ("support/display_colony.rs", display),
        ("support/presenter_colony.rs", presenter),
    ])
    .hits
}

/// `src` with the first wire opening of the function `head` turned unsettled:
/// `.settled()` and its `.await` (on the same line or the next) become
/// `.unsettled().because(Why::Watch)`. A head that does not open the wire settled is a
/// broken test, not a green one.
fn unsettle(src: &str, head: &str) -> String {
    let settled = concat!(".settl", "ed()");
    let at = src
        .find(head)
        .unwrap_or_else(|| panic!("the reader stands: {head}"));
    let open = at
        + src[at..]
            .find(settled)
            .unwrap_or_else(|| panic!("{head} opens the wire settled"));
    let rest = &src[open + settled.len()..];
    let gap = rest.len() - rest.trim_start().len();
    assert!(
        rest[gap..].starts_with(".await"),
        "{head}: the settled open is awaited"
    );
    format!(
        "{}{}{}",
        &src[..open],
        concat!(".unsettl", "ed().because(Why::Watch)"),
        &rest[gap + ".await".len()..]
    )
}

/// `src` without the text from `from` up to and including `to` (both standing once).
fn cut(src: &str, from: &str, to: &str) -> String {
    assert_eq!(src.matches(from).count(), 1, "the cut applies once: {from}");
    let start = src.find(from).expect("the cut starts");
    let end = start + src[start..].find(to).expect("the cut ends") + to.len();
    format!("{}{}", &src[..start], &src[end..])
}

/// One case of a mutant table: `hits` has `want` lines and, when `want > 0`, one of them
/// names `needle`. A wrong case is collected, not asserted, so a red run names every
/// mutant the pin misses at once.
fn expect_hits(wrong: &mut Vec<String>, case: &str, hits: &[String], want: usize, needle: &str) {
    let named = want == 0 || hits.iter().any(|h| h.contains(needle));
    if hits.len() != want || !named {
        wrong.push(format!(
            "{case}: want {want} hit(s) naming `{needle}`, got {}: {hits:?}",
            hits.len()
        ));
    }
}

/// The settle pin is sharp (re-check tv-bn M7b5 A-M1/A-M2/H1, on the M7b7 form): the
/// compiler keeps tests off the wire, and the pin names every way the FIXTURE side could
/// still read past the barrier -- an unsettled open without its mark and its line, a
/// mark or a line without the other, a vanished entry, an exception that hands back a
/// plain value (F5: `watch_tree -> Value`; `journal_of_held` without `Unsettled`), an
/// event that is not listed, a settled reader read again after the colony may have moved
/// -- and a needle in a literal or a comment is no hit. The user side: a reason to open an
/// unsettled value that is not pinned is red, and so is a pinned reason without its site.
#[test]
fn the_settle_pin_finds_what_it_names() {
    let fixtures = the_fixtures();
    let (display, presenter) = (fixtures[0].1.as_str(), fixtures[1].1.as_str());
    let mut wrong = Vec::new();
    expect_hits(
        &mut wrong,
        "the fixtures as they stand",
        &settle_hits(display, presenter),
        0,
        "",
    );

    // A reader that opens the wire unsettled instead of settled: the barrier is gone, so
    // the opening needs its mark AND its line in the pin (A-M1, the reader carries it).
    for reader in [
        "writes_taken",
        "strikes",
        "reads",
        "first_drawn",
        "judge_questions",
    ] {
        let mutant = unsettle(display, &format!("    pub async fn {reader}(&self"));
        expect_hits(
            &mut wrong,
            &format!("unsettled {reader}"),
            &settle_hits(&mutant, presenter),
            1,
            &format!("Colony::{reader} "),
        );
    }
    for reader in ["drawn", "ids"] {
        let mutant = unsettle(presenter, &format!("    pub async fn {reader}(&self"));
        expect_hits(
            &mut wrong,
            &format!("unsettled Stage::{reader}"),
            &settle_hits(display, &mutant),
            1,
            &format!("Stage::{reader} "),
        );
    }
    // A new function, public or a private helper behind a public one, that opens the wire
    // unsettled: derived, never listed.
    for (case, new) in [
        (
            "a new unsettled reader",
            concat!(
                "    pub async fn fresh(&self) -> usize {\n",
                "        self.w.unsettl",
                "ed().because(Why::Watch).log(None).await.len()\n",
                "    }\n"
            ),
        ),
        (
            "a private unsettled helper",
            concat!(
                "    async fn fresh(&self) -> usize {\n",
                "        self.w.unsettl",
                "ed().because(Why::Report).log(None).await.len()\n",
                "    }\n",
                "    pub async fn fresher(&self) -> usize {\n",
                "        self.fresh().await\n",
                "    }\n"
            ),
        ),
    ] {
        let mutant = mutate(
            display,
            "    /// How many questions reached the judge cell.\n",
            &format!("{new}\n    /// How many questions reached the judge cell.\n"),
        );
        expect_hits(
            &mut wrong,
            case,
            &settle_hits(&mutant, presenter),
            1,
            "Colony::fresh ",
        );
    }
    // A held row laundered into a plain value: opening an unsettled value for Watch or Held
    // is an unsettled read of its own and needs the mark and the line.
    let launder = mutate(
        presenter,
        "    /// Whether the patch log at `web` ever created an object whose id carries `needle`.\n",
        concat!(
            "    pub async fn row(&self, held: &HeldRequest) -> Value {\n",
            "        self.journal_of_held(held, \"t\").await.because",
            "(Why::Held)\n",
            "    }\n",
            "    /// Whether the patch log at `web` ever created an object whose id carries `needle`.\n"
        ),
    );
    expect_hits(
        &mut wrong,
        "a held row laundered into a plain value",
        &settle_hits(display, &launder),
        1,
        "Stage::row ",
    );
    // A mark without its line, a line without its mark, a listed name that is gone.
    let marked = mutate(
        presenter,
        "    pub async fn drawn(&self, needle: &str) -> bool {\n",
        concat!(
            "    // no-settle: the request is held by ask()\n",
            "    pub async fn drawn(&self, needle: &str) -> bool {\n"
        ),
    );
    expect_hits(
        &mut wrong,
        "a mark without its line",
        &settle_hits(display, &marked),
        1,
        "Stage::drawn is marked no-settle but not listed",
    );
    let unmarked = mutate(
        presenter,
        "    // no-settle: the event itself: the next message that left /alex\n",
        "",
    );
    expect_hits(
        &mut wrong,
        "a line without its mark",
        &settle_hits(display, &unmarked),
        1,
        "Stage::out is listed no-settle but not marked",
    );
    let gone = cut(
        presenter,
        "// no-settle: a write and its positive wait; returns nothing\npub async fn warm(",
        "    s.wait_drawn(\"warm\").await;\n}\n",
    );
    expect_hits(
        &mut wrong,
        "a listed name that is gone",
        &settle_hits(display, &gone),
        1,
        "warm is no function",
    );
    // A-M2: an exception that hands back a plain value. F5 (`watch_tree -> Value`, the
    // pattern M7b6 removed) and the held row without its wrapper.
    let f5 = mutate(
        display,
        "    pub async fn watch_tree(&self, why: &str, want: impl Fn(&Value) -> bool) {\n",
        "    pub async fn watch_tree(&self, why: &str, want: impl Fn(&Value) -> bool) -> Value {\n",
    );
    expect_hits(
        &mut wrong,
        "F5 watch_tree -> Value",
        &settle_hits(&f5, presenter),
        1,
        "Colony::watch_tree hands back",
    );
    let bare = mutate(
        presenter,
        "turn_id: &str) -> Unsettled<Value> {\n",
        "turn_id: &str) -> Value {\n",
    );
    expect_hits(
        &mut wrong,
        "journal_of_held -> Value",
        &settle_hits(display, &bare),
        1,
        "Stage::journal_of_held hands back",
    );
    // A wait that hands back what it saw at the event, not at rest.
    let at_event = mutate(
        display,
        "        .await;\n        self.tree().await\n    }\n\n    /// Wait until `want` holds of whatever",
        "        .await;\n        Value::Null\n    }\n\n    /// Wait until `want` holds of whatever",
    );
    expect_hits(
        &mut wrong,
        "a settling wait that hands back the event",
        &settle_hits(&at_event, presenter),
        1,
        "Colony::wait_tree hands back",
    );
    // An event the pin does not list: the next message off the egress, handed out raw.
    let event = mutate(
        display,
        "    /// How many questions reached the judge cell.\n",
        concat!(
            "    pub async fn next(&self) -> Option<Message> {\n",
            "        self.w.next_",
            "out().await\n",
            "    }\n",
            "    /// How many questions reached the judge cell.\n"
        ),
    );
    expect_hits(
        &mut wrong,
        "an unlisted event",
        &settle_hits(&event, presenter),
        1,
        "Colony::next delivers the event",
    );
    // H1: a settled reader read again after the colony may have moved.
    let moved = mutate(
        display,
        "        let r = self.w.settled().await;\n        self.account_of_write(",
        "        let r = self.w.settled().await;\n        self.kill_compose().await;\n        self.account_of_write(",
    );
    expect_hits(
        &mut wrong,
        "a settled reader read after a move",
        &settle_hits(&moved, presenter),
        1,
        "Colony::lost_write reads the settled reader `r` after",
    );
    // A brace or a needle in a literal or a comment moves nothing.
    let quoted = mutate(
        display,
        "    /// How many questions reached the judge cell.\n",
        concat!(
            "    pub fn quoted(&self) -> &'static str {\n",
            "        // { .unsettl",
            "ed() .next_",
            "out( decider.re",
            "cv( ReadMessages\n",
            "        \"} .unsettl",
            "ed() .next_",
            "out( .because",
            "(Why::Held) ReadMessages\"\n",
            "    }\n",
            "    /// How many questions reached the judge cell.\n"
        ),
    );
    expect_hits(
        &mut wrong,
        "needles in a literal or a comment",
        &settle_hits(&quoted, presenter),
        0,
        "",
    );
    // Re-check tv-bn M7b9 A-M1 (a): an unsettled value opened for ANY reason is an opening
    // -- a report handed on as a plain text, a hang's account, the commit counter, the
    // field read off the call, the UFCS form -- and needs the mark and the line.
    for (case, fixture, new, needle) in [
        (
            "a report laundered into a text (Stage)",
            "presenter",
            concat!(
                "    pub async fn trail_text(&self) -> String {\n",
                "        self.trail().await.becau",
                "se(Why::Report)\n",
                "    }\n"
            ),
            "Stage::trail_text ",
        ),
        (
            "a report laundered by UFCS",
            "presenter",
            concat!(
                "    pub async fn trail_text(&self) -> String {\n",
                "        Unsettled::becau",
                "se(self.trail().await, Why::Report)\n",
                "    }\n"
            ),
            "Stage::trail_text ",
        ),
        (
            "a report opened by its field",
            "presenter",
            concat!(
                "    pub async fn trail_text(&self) -> String {\n",
                "        self.trail().await",
                ".0\n",
                "    }\n"
            ),
            "Stage::trail_text ",
        ),
        (
            "a hang's account laundered into a text (Colony)",
            "display",
            concat!(
                "    pub async fn lost_text(&self, owner: &str, view_id: &str) -> String {\n",
                "        self.lost_write_now(\"w\", owner, view_id, \"t\")\n",
                "            .await\n",
                "            .becau",
                "se(Why::Report)\n",
                "    }\n"
            ),
            "Colony::lost_text ",
        ),
        (
            "the commit counter laundered into a number",
            "display",
            concat!(
                "    pub fn commits(&self) -> u64 {\n",
                "        self.log_changes().at().becau",
                "se(Why::Report)\n",
                "    }\n"
            ),
            "Colony::commits ",
        ),
    ] {
        let (d, p) = if fixture == "display" {
            (
                mutate(
                    display,
                    "    /// How many questions reached the judge cell.\n",
                    &format!("{new}\n    /// How many questions reached the judge cell.\n"),
                ),
                presenter.to_string(),
            )
        } else {
            (
                display.to_string(),
                mutate(
                    presenter,
                    "    /// Whether the patch log at `web` ever created an object whose id carries `needle`.\n",
                    &format!(
                        "{new}    /// Whether the patch log at `web` ever created an object whose id carries `needle`.\n"
                    ),
                ),
            )
        };
        expect_hits(&mut wrong, case, &settle_hits(&d, &p), 1, needle);
    }
    // A-M1 (2): a reader that only calls exceptions settles nowhere; one that settles after
    // them (a settling reader) is no hit.
    for (case, new, want, needle) in [
        (
            "a reader through a watch only",
            concat!(
                "    pub async fn glance(&self) {\n",
                "        self.watch_tree(\"x\", |_| true).await;\n",
                "    }\n"
            ),
            1,
            "Colony::glance reads through an exception without settling",
        ),
        (
            "a reader that settles after the watch",
            concat!(
                "    pub async fn glance(&self) -> Value {\n",
                "        self.watch_tree(\"x\", |_| true).await;\n",
                "        self.tree().await\n",
                "    }\n"
            ),
            0,
            "",
        ),
    ] {
        let mutant = mutate(
            display,
            "    /// How many questions reached the judge cell.\n",
            &format!("{new}\n    /// How many questions reached the judge cell.\n"),
        );
        expect_hits(
            &mut wrong,
            case,
            &settle_hits(&mutant, presenter),
            want,
            needle,
        );
    }
    let handed_on = mutate(
        presenter,
        "    /// Whether the patch log at `web` ever created an object whose id carries `needle`.\n",
        concat!(
            "    pub async fn trail_again(&self) -> Unsettled<String> {\n",
            "        self.trail().await\n",
            "    }\n",
            "    /// Whether the patch log at `web` ever created an object whose id carries `needle`.\n"
        ),
    );
    expect_hits(
        &mut wrong,
        "an unsettled value handed on unlisted",
        &settle_hits(display, &handed_on),
        1,
        "Stage::trail_again reads through an exception without settling",
    );
    // A-M1 (b): the wire's verbs are pinned. A new verb (and the `Colony` reader over it),
    // a verb that hands out a reader, a known verb that grows a reader, a return type or a
    // raw read, a verb that is gone, a function of `mod wire` that is no verb.
    let all = mutate(
        &mutate(
            display,
            "        /// A reader WITHOUT the barrier, opened only with a named [`super::Why`].\n",
            concat!(
                "        pub(super) async fn all(&self) -> Vec<MessageLogDto> {\n",
                "            self.h.read_",
                "log(MessageLogFilter::default()).await\n",
                "        }\n",
                "        /// A reader WITHOUT the barrier, opened only with a named [`super::Why`].\n"
            ),
        ),
        "    /// How many questions reached the judge cell.\n",
        concat!(
            "    pub async fn all(&self) -> Vec<MessageLogDto> {\n",
            "        self.w.all().await\n",
            "    }\n",
            "    /// How many questions reached the judge cell.\n"
        ),
    );
    expect_hits(
        &mut wrong,
        "a new wire verb and its reader",
        &settle_hits(&all, presenter),
        1,
        "Wire::all is no pinned wire verb",
    );
    for (case, from, to, want, needle) in [
        (
            "a new verb that hands out a reader",
            "        /// A reader WITHOUT the barrier, opened only with a named [`super::Why`].\n",
            concat!(
                "        pub(super) fn raw(&self) -> Reader<'_> {\n",
                "            Read",
                "er(self)\n",
                "        }\n",
                "        /// A reader WITHOUT the barrier, opened only with a named [`super::Why`].\n"
            ),
            2,
            "Wire::raw",
        ),
        (
            "a known verb that builds a reader",
            "            super::LogChanges(self.h.subscribe_log_seq())\n",
            concat!(
                "            let _r = Read",
                "er(self);\n",
                "            super::LogChanges(self.h.subscribe_log_seq())\n"
            ),
            1,
            "Wire::log_changes builds a Reader",
        ),
        (
            "a verb whose return type moved",
            "        pub(super) async fn send(&self, msg: Message) {\n",
            "        pub(super) async fn send(&self, msg: Message) -> usize {\n",
            1,
            "Wire::send (write) hands back",
        ),
        (
            "a verb that reads raw",
            "            self.h.send(msg).await;\n",
            concat!(
                "            self.h.send(msg).await;\n",
                "            let _ = self.h.read_",
                "log(MessageLogFilter::default()).await;\n"
            ),
            1,
            "Wire::send (write) reads the colony raw",
        ),
        (
            "a function of the wire that is no verb",
            "    /// The raw doors. Every field is private, so only this module touches them.\n",
            concat!(
                "    pub(super) async fn take(w: &Wire) -> Option<Message> {\n",
                "        w.egress.lock().await.recv().await\n",
                "    }\n",
                "    /// The raw doors. Every field is private, so only this module touches them.\n"
            ),
            1,
            "take stands in `mod wire`",
        ),
    ] {
        let mutant = mutate(display, from, to);
        expect_hits(
            &mut wrong,
            case,
            &settle_hits(&mutant, presenter),
            want,
            needle,
        );
    }
    let verb_gone = cut(
        display,
        "        /// Shut the colony down.\n",
        "        }\n",
    );
    expect_hits(
        &mut wrong,
        "a wire verb that is gone",
        &settle_hits(&verb_gone, presenter),
        1,
        "Wire::shutdown (write) is no function",
    );
    // A-M1 (c): the colony's database past the wire, by the crate, the root's guard, or
    // the file's name in a literal.
    for (case, new) in [
        (
            "rusqlite on the database under the root",
            concat!(
                "    pub fn rows_raw(&self) -> bool {\n",
                "        rusq",
                "lite::Connection::open(self._t",
                "d.path().join(\"colony",
                ".db\")).is_ok()\n",
                "    }\n"
            ),
        ),
        (
            "rusqlite alone",
            concat!(
                "    pub fn rows_raw(&self) -> bool {\n",
                "        rusq",
                "lite::Connection::open_in_memory().is_ok()\n",
                "    }\n"
            ),
        ),
        (
            "the root's guard read",
            concat!(
                "    pub fn rows_raw(&self) -> bool {\n",
                "        self._t",
                "d.path().exists()\n",
                "    }\n"
            ),
        ),
        (
            "the colony's database by name",
            concat!(
                "    pub fn rows_raw(&self) -> bool {\n",
                "        std::path::Path::new(\"colony",
                ".db\").exists()\n",
                "    }\n"
            ),
        ),
        (
            "a cell's database by name",
            concat!(
                "    pub fn rows_raw(&self) -> bool {\n",
                "        std::path::Path::new(\"web/cell",
                ".db\").exists()\n",
                "    }\n"
            ),
        ),
    ] {
        let mutant = mutate(
            display,
            "    /// How many questions reached the judge cell.\n",
            &format!("{new}\n    /// How many questions reached the judge cell.\n"),
        );
        expect_hits(
            &mut wrong,
            case,
            &settle_hits(&mutant, presenter),
            1,
            "Colony::rows_raw reads the colony raw",
        );
    }
    // Blind: no fixture, no reader.
    let empty = settle_scan(&[]);
    if (empty.readers, empty.settling) != (0, 0)
        || !empty.hits.iter().all(|h| h.contains("no function"))
    {
        wrong.push(format!(
            "an empty scan: {} readers, {} settling, {:?}",
            empty.readers, empty.settling, empty.hits
        ));
    }

    // The user side, past the readers: the forms the call pin missed are no hits (the
    // reader carries the barrier), every way past the readers is one -- also from a support
    // file that is not one of the two fixtures (re-check tv-bn M7b5 M1 c).
    let fine = concat!(
        "async fn t(colony: &Colony, s: &Stage) {\n",
        "    colony.wait_tree(\"drawn\", |t| ok(t)).await;\n",
        "    let writes = colony.writes_taken().await;\n",
        "    assert!(writes.is_empty());\n",
        "    assert!(Colony::strikes(colony).await.is_empty());\n",
        "    assert!(!stage_routes(s).await.iter().any(|r| r == \"view\"));\n",
        "    let mark = colony.mark_log().await;\n",
        "    let rows = colony\n        .rows_since(&mark, filter)\n        .await;\n",
        "    let note = \"colony.egress.try_recv() and s.c.unsettled()\";\n",
        "}\n"
    );
    expect_hits(
        &mut wrong,
        "a user through the readers",
        &past_the_readers("x.rs", fine),
        0,
        "",
    );
    for past in [
        "let t = s.c.unsettled().tree().await;",
        "let rows = colony.h.log_since(&mark, filter).await;",
        "let mark = colony.h.log_mark().await;",
        "let rows = s.c.h\n        .log_since(&mark, filter)\n        .await;",
        "let h = &colony.h;\nlet rows = h.log_since(&mark, filter).await;",
        "while let Ok(m) = colony.egress.try_recv() {}",
        "let dlq = colony.h.drain_dead_letters().await;",
        "colony.h.inbox_tx.send(ColonyMsg::ReadMessages { filter, ack }).await;",
    ] {
        expect_hits(&mut wrong, past, &past_the_readers("x.rs", past), 1, "x.rs");
    }
    expect_hits(
        &mut wrong,
        "a support file that is no fixture",
        &past_the_readers(
            "support/new.rs",
            "let rows = colony.h.log_since(&mark, filter).await;",
        ),
        1,
        "support/new.rs",
    );
    expect_hits(
        &mut wrong,
        "the fixtures are the readers",
        &past_the_readers(
            "support/presenter_colony.rs",
            "s.c.unsettled().tree().await;",
        ),
        0,
        "",
    );

    // The user side, the reasons (A-M2): every `.because(Why::…)` of a user is pinned with
    // its reason, and every pinned reason has its site.
    let users = fixture_users();
    expect_hits(
        &mut wrong,
        "the users as they stand",
        &user_reason_hits(&users),
        0,
        "",
    );
    // Re-check tv-bn M7b9 A-M2: a user past `page`/`status`/`socket` and their barrier --
    // the API's log route, the mount, a socket of its own -- and the colony's database.
    for past in [
        concat!(
            "let r = reqw",
            "est::get(format!(\"{}/colony/",
            "messages\", colony.api_url())).await;"
        ),
        concat!(
            "let page = reqw",
            "est::get(colony.url(\"monitor\")).await;"
        ),
        concat!(
            "let (ws, _) = tokio_tungstenite::connect_",
            "async(colony.url(\"monitor\")).await.unwrap();"
        ),
        concat!(
            "let s = tokio::net::TcpStream::",
            "connect(colony.api_url()).await;"
        ),
        concat!("let c = hyp", "er::Client::new();"),
        concat!("let c = rusq", "lite::Connection::open(dir.join(\"x\"));"),
        concat!("let b = std::fs::read(dir.join(\"colony", ".db\"));"),
        concat!("let b = std::fs::read(dir.join(\"web/cell", ".db\"));"),
        concat!(
            "let url = format!(\"{}/colony/",
            "messages\", colony.api_url());"
        ),
    ] {
        expect_hits(&mut wrong, past, &past_the_readers("x.rs", past), 1, "x.rs");
    }
    expect_hits(
        &mut wrong,
        "a path in a comment",
        &past_the_readers(
            "x.rs",
            concat!(
                "// GET /colony/",
                "messages, colony",
                ".db\nlet n = colony.log_len().await;\n"
            ),
        ),
        0,
        "",
    );
    // The allowances are per needle and counted: gh867 GETs static files, so a further
    // GET is red by its count, and a log GET or a database read there is red by its needle.
    let gh867 = "gh867_no_surface_page_carries_unlisted_inline_script.rs";
    let src867 = users
        .iter()
        .find(|(n, _)| n == gh867)
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    expect_hits(
        &mut wrong,
        "gh867 as it stands",
        &past_the_readers(gh867, &src867),
        0,
        "",
    );
    for (case, add, want, needle) in [
        (
            "gh867 with a further GET",
            concat!(
                "async fn y() {\n    let _ = reqw",
                "est::get(\"http://x\").await;\n}\n"
            ),
            1,
            "PAST_THE_READERS_ALLOWED pins the doors",
        ),
        (
            "gh867 with a log GET",
            concat!(
                "async fn y(colony: &Colony) {\n    let _ = reqw",
                "est::get(format!(\"{}/colony/",
                "messages\", colony.api_url())).await;\n}\n"
            ),
            2,
            concat!("colony/", "messages"),
        ),
        (
            "gh867 with a database read",
            concat!(
                "fn y() {\n    let _ = std::fs::read(\"colony",
                ".db\");\n}\n"
            ),
            1,
            concat!("colony", ".db"),
        ),
    ] {
        let mutant = format!("{src867}\n{add}");
        expect_hits(
            &mut wrong,
            case,
            &past_the_readers(gh867, &mutant),
            want,
            needle,
        );
    }
    let without_867: Vec<(String, String)> =
        users.iter().filter(|(n, _)| n != gh867).cloned().collect();
    expect_hits(
        &mut wrong,
        "an allowance for a file that is no user",
        &allowed_without_user(&without_867),
        1,
        "gh867_no_surface",
    );
    let mut more = users.clone();
    more.push((
        "gh_new.rs".to_string(),
        concat!(
            "async fn t(s: &Stage, held: &HeldRequest) {\n",
            "    let row = s.journal_of_held(held, \"t\").await.because",
            "(Why::Held);\n",
            "    assert!(row.is_null());\n",
            "}\n"
        )
        .to_string(),
    ));
    expect_hits(
        &mut wrong,
        "a new reason without its line",
        &user_reason_hits(&more),
        1,
        "gh_new.rs",
    );
    let fewer: Vec<(String, String)> = users
        .iter()
        .filter(|(n, _)| !n.starts_with("gh959_an_unsure"))
        .cloned()
        .collect();
    expect_hits(
        &mut wrong,
        "a pinned reason without its site",
        &user_reason_hits(&fewer),
        1,
        "has no site",
    );
    let mut quoted_users = users.clone();
    quoted_users.push((
        "gh_note.rs".to_string(),
        concat!(
            "// s.journal_of_held(held, \"t\").await.because",
            "(Why::Held)\n",
            "let note = \".because",
            "(Why::Held)\";\n"
        )
        .to_string(),
    ));
    expect_hits(
        &mut wrong,
        "a reason in a literal or a comment",
        &user_reason_hits(&quoted_users),
        0,
        "",
    );

    // OR-T11-25, re-check tv-bn M7b11 A-M1 ii: a nested module or a macro in `mod wire`
    // builds functions the pin does not read; a second mint of the settle proof.
    let wire_head = "mod wire {\n";
    for (case, add) in [
        ("a module in the wire", "    mod more {}\n"),
        (
            "a macro in the wire",
            concat!("    macro_", "rules! verb { () => {}; }\n"),
        ),
    ] {
        let mutant = mutate(display, wire_head, &format!("{wire_head}{add}"));
        expect_hits(
            &mut wrong,
            case,
            &settle_hits(&mutant, presenter),
            1,
            "holds a nested module or a macro",
        );
    }
    let page_head = "    pub async fn page(&self, screen: &str) -> String {\n        let at_rest = self.settle().await;";
    let forged = mutate(
        display,
        page_head,
        "    pub async fn page(&self, screen: &str) -> String {\n        let at_rest = sealed::Settled::new();",
    );
    expect_hits(
        &mut wrong,
        "a settle proof without the barrier",
        &settle_hits(&forged, presenter),
        2,
        "SEALED_MINTS pins 0",
    );
    // M7b12 C-M2 (M7b14): one horizon -- `Reader::log` back on one page of 1000, and a
    // window reader called for a verdict.
    let log_head =
        "        pub(super) async fn log(&self, to_prefix: Option<&str>) -> Vec<MessageLogDto> {\n";
    let one_page = mutate(
        display,
        log_head,
        &format!(
            "{log_head}            let _page = MessageLogFilter {{ {}: 1000, ..Default::default() }};\n",
            concat!("lim", "it")
        ),
    );
    expect_hits(
        &mut wrong,
        "the log read on one page",
        &settle_hits(&one_page, presenter),
        1,
        "reads one page",
    );
    // Re-check tv-bn M7b14 C-M2: the short form clippy asks for, `limit` as a word.
    let short = mutate(
        display,
        log_head,
        &format!(
            "{log_head}            let {l} = 1000;\n            let _page = MessageLogFilter {{ {l}, ..Default::default() }};\n",
            l = concat!("lim", "it")
        ),
    );
    expect_hits(
        &mut wrong,
        "the log read on one page in the short form",
        &settle_hits(&short, presenter),
        1,
        "reads one page",
    );
    let verdict = mutate(
        display,
        log_head,
        &format!("{log_head}            let _rows = self.newest(1000).await;\n"),
    );
    expect_hits(
        &mut wrong,
        "a window reader for a verdict",
        &settle_hits(&verdict, presenter),
        1,
        "calls the window reader `newest`",
    );
    let since = "            let rows = self.0.h.log_since(mark, filter).await;\n";
    let unchecked = mutate(
        display,
        &format!(
            "{since}            super::{}(&rows);\n",
            concat!("inline_", "bodies")
        ),
        since,
    );
    expect_hits(
        &mut wrong,
        "log rows handed out unchecked",
        &settle_hits(&unchecked, presenter),
        1,
        "Reader::rows_since hands out log rows unchecked",
    );
    // Re-check tv-bn M7b14 C-M2: a row type other than `Vec<…>` hands out rows as well.
    let since_ret = "            filter: MessageLogFilter,\n        ) -> Vec<MessageLogDto> {\n            // A log reader like `log`";
    let optional = mutate(
        &unchecked,
        since_ret,
        &since_ret.replace("Vec<MessageLogDto>", "Option<MessageLogDto>"),
    );
    expect_hits(
        &mut wrong,
        "one log row handed out unchecked",
        &settle_hits(&optional, presenter),
        1,
        "Reader::rows_since hands out log rows unchecked",
    );
    // A-M5: the raw reads are derived -- a `ColonyMsg` reader the old list missed, in the
    // boot that holds the handle; the root's guard bound to a name.
    let boot_line = "    let (api_port, api) = api_listener(h.inbox_tx.clone()).await;\n";
    let graph = mutate(
        display,
        boot_line,
        &format!(
            "{boot_line}    let _ = h.inbox_tx.send(ColonyMsg::Read{}{{ ack: todo!() }});\n",
            "Graph "
        ),
    );
    expect_hits(
        &mut wrong,
        "ReadGraph in the boot",
        &settle_hits(&graph, presenter),
        1,
        "boot_inner reads the colony raw",
    );
    let tree_head = "    pub async fn tree(&self) -> Value {\n";
    let root = mutate(
        display,
        tree_head,
        &format!(
            "{tree_head}        let t = &self.{};\n        let _ = t;\n",
            concat!("_t", "d")
        ),
    );
    expect_hits(
        &mut wrong,
        "the root's guard bound to a name",
        &settle_hits(&root, presenter),
        1,
        "Colony::tree reads the colony raw",
    );
    // A-M5 and A-M3, the user side: a handle reader on any receiver, a client by an alias
    // or a sibling crate, a sync websocket, the kit's voice client.
    for past in [
        concat!("let rows = handle.log_", "since(&mark, f).await;"),
        concat!("let n = colony_handle.read_", "log(None).await;"),
        concat!("let p = h.tempdir_", "path();"),
        concat!("use reqw", "est as r;"),
        concat!(
            "let c = hyp",
            "er_util::client::legacy::Client::builder(x);"
        ),
        concat!("let ws = tokio_tungstenite::tungstenite::", "connect(url);"),
        concat!(
            "let v = meclaw_testing::voice_client::Voice",
            "Client::connect(url).await;"
        ),
    ] {
        expect_hits(&mut wrong, past, &past_the_readers("x.rs", past), 1, "x.rs");
    }
    // A-M5 in 808: a reader the old list missed, beside the pinned ones.
    let s808 = "808_the_colony_view_stands_in_the_dock.rs";
    let src808 = users
        .iter()
        .find(|(n, _)| n == s808)
        .map(|(_, s)| s.clone())
        .unwrap_or_default();
    expect_hits(
        &mut wrong,
        "808 as it stands",
        &past_the_readers(s808, &src808),
        0,
        "",
    );
    for (case, add, needle) in [
        (
            "808 reads its dead letters raw",
            concat!(
                "async fn y(h: &ColonyHandle) {\n    let _ = h.inbox_tx.send(ColonyMsg::Read",
                "DeadLetters { ack });\n}\n"
            ),
            concat!("Read", "DeadLetters"),
        ),
        // A-M4: a further read behind a local door moves the count of its call sites.
        (
            "808 reads the tree once more",
            "async fn y(s: &Stage) {\n    let _ = s.tree().await;\n}\n",
            "called at 12 site(s)",
        ),
    ] {
        let mutant = format!("{src808}\n{add}");
        expect_hits(
            &mut wrong,
            case,
            &past_the_readers(s808, &mutant),
            1,
            needle,
        );
    }
    let get_more = concat!(
        "async fn z(colony: &Colony) {\n    let _ = get_text(\"/screen/\").await;\n",
        "    let _ = colony.voice_files(Site::Gh867VoiceFiles).await;\n}\n"
    );
    let more867 = format!("{src867}\n{get_more}");
    expect_hits(
        &mut wrong,
        "gh867 GETs the composed screen through `get_text`",
        &past_the_readers(gh867, &more867),
        1,
        "called at 8 site(s)",
    );
    // OR-T11-25 G2: the address through the one door -- a second place of a site, a site
    // bound to a name, a renamed site, a site the fixture does not have.
    let with = |name: &str, src: &str| {
        let mut u: Vec<(String, String)> =
            users.iter().filter(|(n, _)| n != name).cloned().collect();
        u.push((name.to_string(), src.to_string()));
        u
    };
    expect_hits(
        &mut wrong,
        "the sites as they stand",
        &site_hits(&users, display),
        0,
        "",
    );
    expect_hits(
        &mut wrong,
        "a second place of the voice files' site",
        &site_hits(&with(gh867, &more867), display),
        1,
        "Gh867VoiceFiles` stands in 2 places",
    );
    for (case, src, want, needle) in [
        (
            "a site bound to a name",
            "fn t(c: &Colony) {\n    let at = Site::Gh767Monitor;\n    let _ = c.drive(at, \"m\", Driver::default());\n}\n",
            2,
            "stands outside an address call",
        ),
        (
            "a renamed site",
            concat!("use display_colony::{Site ", "as S};\n"),
            1,
            "renames or globs",
        ),
        (
            "a site the fixture does not have",
            "fn t(c: &Colony) {\n    let _ = c.drive(Site::Elsewhere, \"m\", Driver::default());\n}\n",
            1,
            "is no site of the fixture",
        ),
        (
            "a user that walks every site",
            "fn t(c: &Colony) {\n    for s in Site::ALL {}\n}\n",
            1,
            "walks every site",
        ),
    ] {
        expect_hits(
            &mut wrong,
            case,
            &site_hits(&with("gh_new.rs", src), display),
            want,
            needle,
        );
    }

    assert!(
        wrong.is_empty(),
        "the settle pin misses what it names:\n{}",
        wrong.join("\n")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// The sealed core, compiled (OR-T11-25 G1/G5, re-check tv-bn M7b11 A-M1 i, A-M2, A-M3)
// ─────────────────────────────────────────────────────────────────────────────

/// The probe crate around `support/sealed.rs`: the module `dc` stands where the display
/// fixture stands (the parent of `sealed`, with a child-visible door, a decider and an
/// unsettled reader of its own), `{fixture}` is code of the fixture itself, `{user}` code
/// of a test outside it -- the form of every user, the harness sibling of
/// `super::display_colony` and a `#[path] mod dc` of its own alike (re-check M7b11 A-M2).
const PROBE_CRATE: &str = r##"#![allow(dead_code)]
mod dc {
    #[path = "{sealed}"]
    pub mod sealed;
    pub use sealed::{Unsettled, Why};
    #[derive(Debug, Clone, Copy)]
    pub enum Site { Here }
    impl sealed::NamedSeal for Site {}
    impl sealed::Named for Site {
        fn file(self) -> &'static str { "probe.rs" }
        fn line(self) -> u32 { 1 }
        fn reason(self) -> &'static str { "the probe's own site" }
    }
    pub struct Colony { door: sealed::Door, decider: sealed::Sealed<u8> }
    impl Colony {
        pub fn boot() -> Self {
            let _ = sealed::Key::new();
            Colony { door: sealed::Door::new(1, 2), decider: sealed::Sealed::new(0) }
        }
        pub fn trail(&self) -> Unsettled<String> { Unsettled::new(String::new()) }
        pub fn page(&self) -> u16 { self.door.settled(sealed::barrier::settle()).port }
        #[track_caller]
        pub fn url(&self, at: Site) -> u16 { self.door.hand(at).api_port }
        pub fn ask(&mut self) -> u8 { *self.decider.open_mut() + *self.decider.open() }
    }
    pub fn door_of(c: &Colony) -> &sealed::Door { &c.door }
    pub fn decider_of(c: &Colony) -> &sealed::Sealed<u8> { &c.decider }
    {fixture}
}
pub fn user(c: &mut dc::Colony) -> String {
    let _ = (c.page(), c.url(dc::Site::Here), c.ask());
    c.trail().because(dc::Why::Report)
}
{user}
"##;

/// Every way around the sealed core, as (name, side, code, the error the compiler must
/// answer with). Side `f` is the fixture itself, `u` a test. Each was measured with
/// rustc 2024 edition; a form that compiles, or fails for another reason, is red.
///
/// On 8bb25f709 the fixture side held `Unsettled`'s field and the ports in its own module
/// (`pub port`, a field of `Unsettled` private to `display_colony`), so every `f` row and
/// `u_new`, `u_door_*`, `u_settled_new`, `u_key_new`, `u_sealed_*` compiled there (the
/// probe against that shape is red, M7b12 report).
const PROBE_CASES: [(&str, char, &str, &str); 22] = [
    // A-M1 i: the fixture opens an unsettled value past `because`.
    (
        "fx_field",
        'f',
        "fn m(c: &Colony) -> String { let u = c.trail(); u.0 }",
        "E0616",
    ),
    (
        "fx_let",
        'f',
        "fn m(c: &Colony) -> String { let Unsettled(t) = c.trail(); t }",
        "E0532",
    ),
    (
        "fx_match",
        'f',
        "fn m(c: &Colony) -> String { match c.trail() { Unsettled(t) => t } }",
        "E0532",
    ),
    (
        "fx_unsettled_lit",
        'f',
        "fn m() -> Unsettled<u8> { Unsettled(1) }",
        "E0423",
    ),
    // A-M3: the fixture reads the address without the settle proof, or forges one.
    (
        "fx_port",
        'f',
        "fn m(c: &Colony) -> u16 { c.door.port }",
        "E0616",
    ),
    (
        "fx_settled_lit",
        'f',
        "fn m() -> sealed::Settled { sealed::Settled(()) }",
        "E0603",
    ),
    // M7b12 A-M1: the fixture mints the settle proof past the barrier.
    (
        "fx_settled_new",
        'f',
        "fn m() -> sealed::Settled { sealed::Settled::new() }",
        "E0624",
    ),
    (
        "fx_door_lit",
        'f',
        "fn m() -> sealed::Door { sealed::Door { port: 1, api_port: 2 } }",
        "E0451",
    ),
    // A-M2 (a user outside the fixture, in any of its forms) and the user side of A-M1/A-M3.
    (
        "u_field",
        'u',
        "fn m(c: &dc::Colony) -> String { c.trail().0 }",
        "E0616",
    ),
    (
        "u_let",
        'u',
        "fn m(c: &dc::Colony) -> String { let dc::Unsettled(t) = c.trail(); t }",
        "E0532",
    ),
    (
        "u_lit",
        'u',
        "fn m() -> dc::Unsettled<u8> { dc::Unsettled(1) }",
        "E0423",
    ),
    (
        "u_new",
        'u',
        "fn m() -> dc::Unsettled<u8> { dc::Unsettled::new(1) }",
        "E0624",
    ),
    (
        "u_settled_new",
        'u',
        "fn m() -> dc::sealed::Settled { dc::sealed::Settled::new() }",
        "E0624",
    ),
    (
        "u_key_new",
        'u',
        "fn m() -> dc::sealed::Key { dc::sealed::Key::new() }",
        "E0624",
    ),
    (
        "u_door_new",
        'u',
        "fn m() -> dc::sealed::Door { dc::sealed::Door::new(1, 2) }",
        "E0624",
    ),
    (
        "u_door_port",
        'u',
        "fn m(c: &dc::Colony) -> u16 { dc::door_of(c).port }",
        "E0616",
    ),
    (
        "u_door_settled",
        'u',
        "fn m(c: &dc::Colony, s: dc::sealed::Settled) -> u16 { dc::door_of(c).settled(s).port }",
        "E0624",
    ),
    (
        "u_door_hand",
        'u',
        "fn m(c: &dc::Colony) -> u16 { dc::door_of(c).hand(dc::Site::Here).port }",
        "E0624",
    ),
    (
        "u_sealed_open",
        'u',
        "fn m(c: &dc::Colony) -> u8 { *dc::decider_of(c).open() }",
        "E0624",
    ),
    (
        "u_sealed_field",
        'u',
        "fn m(c: &dc::Colony) -> u8 { dc::decider_of(c).0 }",
        "E0616",
    ),
    // M7b12 B-M2: a site of a test's own -- `Named` is sealed, against the real trait.
    (
        "u_own_site",
        'u',
        "#[derive(Debug, Clone, Copy)] struct Mine; \
         impl dc::sealed::Named for Mine { fn file(self) -> &'static str { \"x.rs\" } \
         fn line(self) -> u32 { 1 } fn reason(self) -> &'static str { \"mine\" } }",
        "E0277",
    ),
    (
        "u_seal",
        'u',
        "#[derive(Debug, Clone, Copy)] struct Mine; impl dc::sealed::NamedSeal for Mine {}",
        "E0603",
    ),
];

/// The compiler that built this test: `rustc` beside the `cargo` of the build, else the
/// one on the PATH.
fn probe_rustc() -> std::path::PathBuf {
    let beside = std::path::Path::new(env!("CARGO")).with_file_name("rustc");
    if beside.is_file() {
        beside
    } else {
        std::path::PathBuf::from("rustc")
    }
}

/// The workspace's edition (`edition = "…"` of the root manifest).
fn probe_edition() -> String {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let manifest = std::fs::read_to_string(&root).expect("the workspace manifest reads");
    manifest
        .lines()
        .find_map(|l| {
            let l = l.trim();
            let v = l.strip_prefix("edition")?.trim_start().strip_prefix('=')?;
            Some(v.trim().trim_matches('"').to_string())
        })
        .expect("the workspace names its edition")
}

/// Compile one probe of `sealed` with `fixture`/`user` code; the error codes it failed
/// with (empty: it compiled), or the whole output when rustc did not run as a compiler.
fn compile_probe(
    dir: &std::path::Path,
    sealed: &std::path::Path,
    name: &str,
    fixture: &str,
    user: &str,
) -> Result<std::collections::BTreeSet<String>, String> {
    let src = PROBE_CRATE
        .replace("{sealed}", &sealed.display().to_string())
        .replace("{fixture}", fixture)
        .replace("{user}", user);
    let file = dir.join(format!("{name}.rs"));
    std::fs::write(&file, src).expect("the probe writes");
    let out = std::process::Command::new(probe_rustc())
        .arg("--edition")
        .arg(probe_edition())
        .args([
            "--cfg",
            "sealed_probe",
            "--crate-type",
            "lib",
            "--error-format",
            "short",
            "--crate-name",
        ])
        .arg(name)
        .arg("--out-dir")
        .arg(dir)
        .arg(&file)
        .output()
        .map_err(|e| format!("rustc does not run: {e}"))?;
    let err = String::from_utf8_lossy(&out.stderr);
    let codes: std::collections::BTreeSet<String> = err
        .match_indices("error[E")
        .map(|(i, _)| err[i + 6..i + 11].to_string())
        .collect();
    if out.status.success() {
        Ok(codes)
    } else if codes.is_empty() {
        Err(format!(
            "{name}: rustc failed without an error code:\n{err}"
        ))
    } else {
        Ok(codes)
    }
}

/// Every probe of `sealed` that does not hold: the positive control must compile, every
/// case must fail with exactly its code.
fn sealed_probe(sealed: &std::path::Path) -> Vec<String> {
    let dir = tempfile::tempdir().expect("a probe directory");
    let mut wrong = Vec::new();
    match compile_probe(dir.path(), sealed, "positive", "", "") {
        Ok(codes) if codes.is_empty() => {}
        Ok(codes) => wrong.push(format!(
            "the probe is broken: the positive control fails with {codes:?}"
        )),
        Err(e) => wrong.push(format!("the probe is broken: {e}")),
    }
    for (name, side, code, want) in PROBE_CASES {
        let (fixture, user) = if side == 'f' { (code, "") } else { ("", code) };
        match compile_probe(dir.path(), sealed, name, fixture, user) {
            Ok(codes) if codes.len() == 1 && codes.contains(want) => {}
            Ok(codes) if codes.is_empty() => wrong.push(format!(
                "{name}: compiles, the sealed core lets it through: {code}"
            )),
            Ok(codes) => wrong.push(format!("{name}: fails with {codes:?}, not {want}: {code}")),
            Err(e) => wrong.push(e),
        }
    }
    wrong
}

/// OR-T11-25 G1/G5: the guarantee is the compiler. In every run of this binary the sealed
/// core is compiled with a positive control and every way around it (re-check tv-bn M7b11
/// A-M1 i, A-M2, A-M3); this replaces the `compile_fail` block of the fixture, which never
/// ran (integration-test doctests do not).
#[test]
fn the_sealed_core_refuses_every_way_around_it() {
    let sealed = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/sealed.rs");
    let wrong = sealed_probe(&sealed);
    assert!(
        wrong.is_empty(),
        "the sealed core of the display fixture:\n{}",
        wrong.join("\n")
    );
}

/// OR-T11-25 G2 at run time (re-check tv-bn M7b11 A-M3): the one door hands the colony's
/// address to a site only in the site's own file. This file asking with gh767's site is a
/// new way to the address from the wrong place: refused where it asks (`#[track_caller]`
/// binds the site to the caller), before any address leaves the fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_door_refuses_a_site_of_another_file() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = boot(Boot::default()).await;
    let asked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        colony.drive(Site::Gh767Monitor, "monitor", Driver::default())
    }));
    let refused = asked.expect_err("the door hands gh767's address to 809");
    let text = refused
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        text.contains("is the site of gh767_the_drivers_tap_reaches_the_curator_browser.rs")
            && text.contains("809_a_restart_reads_once_and_draws_the_same_screen.rs"),
        "the refusal names the site's file and the caller's: {text}"
    );
    colony.shutdown().await;
}

/// OR-T11-25 G3 (re-check tv-bn M7b11 A-M5): the raw reads are derived, not listed -- the
/// `ColonyMsg` readers from the exhaustive classification (all eleven `Read*`/`Drain*`,
/// never a write like `Settle`), the `ColonyHandle` readers from its `pub fn`s minus the
/// pinned non-readers (the four the old list missed among them).
#[test]
fn the_raw_reads_are_derived_from_the_colony() {
    let (ack, _rx) = tokio::sync::oneshot::channel();
    assert!(!reads_the_colony(&meclaw_colony::ColonyMsg::Settle { ack }));
    let msgs = colony_msg_readers();
    for v in [
        concat!("Read", "Graph"),
        concat!("Read", "DeadLetters"),
        concat!("Read", "Registry"),
        concat!("Drain", "DeadLetters"),
    ] {
        assert!(msgs.iter().any(|m| m == v), "{v} is a reader: {msgs:?}");
    }
    assert!(!msgs.iter().any(|m| m == "Settle"), "Settle is no reader");
    let handle = handle_readers();
    for r in [
        "log_since",
        "log_mark",
        "drain_dead_letters",
        "tempdir_path",
        "read_log",
    ] {
        assert!(
            handle.iter().any(|h| h == r),
            "{r} is a handle reader: {handle:?}"
        );
    }
    assert!(!handle.iter().any(|h| h == "send"), "send is no reader");
}
