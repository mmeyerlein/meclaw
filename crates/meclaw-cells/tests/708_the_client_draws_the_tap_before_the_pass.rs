//! GH #708 -- the client draws the tap before the pass (§ 5.7), and draws only
//! the end state of a series (§ 5.8).
//!
//! "Immediately visible. Not in the model (rendering): the client draws the
//! effect of a tap by sentences 1-3 optimistically (window open or closed, tile
//! pressed, a co-closed modal closed); the curator's pass confirms it. Because
//! the finger stands above the score (§ 4.19), the pass never disagrees; a
//! deviation is a defect -- Q-20 is the pass the client is checked against"
//! (§ 5.7). And § 5.8: "the client draws only the end state of the series, no
//! window flashes".
//!
//! Until this wave the client drew the pressed TILE and nothing else: the
//! window opened when the patch of the next pass arrived, which is one round
//! trip through the colony -- on a phone the finger had long left the glass.
//!
//! What the three sentences mean for the DOM, and what this file pins:
//!
//!   * a tap on the tile of an open window puts it away (§ 5.2): level 0,
//!     rung `ambient`, the tile no longer `open`;
//!   * a tap on the tile of a window that is not open opens it (§ 5.1): level 1
//!     on the canvas ladder, level 2 on the modal one, rung `focus`;
//!   * either way, a tap on a CANVAS window co-closes the open modal (§ 5.3 --
//!     the rule reads the tapped window's `layer`, also on put-away);
//!   * and while the client's own drawing stands, the FLIP animation keeps its
//!     hands off, or ten taps in a second are ten flights (§ 5.8).
//!
//! The browser proofs are B-06 (the DOM changes before the patch, and the end
//! state matches the pass) and B-07 (ten taps, at most one visible change),
//! strand H5.
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
fn the_scene_draws_the_window_and_not_only_the_tile() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        scene.contains("function optimistic("),
        "the scene hook has no optimistic drawing at all -- § 5.7: the client \
         draws the effect of a tap, the pass confirms it"
    );
    assert!(
        scene.contains("getAttribute(\\\"data-for\\\")"),
        "the drawing never looks up the WINDOW its tile stands for: the two \
         halves of one object carry the same name (§ 6.10)"
    );
    for written in [
        "setAttribute(\\\"data-level\\\"",
        "setAttribute(\\\"data-rung\\\"",
        "setAttribute(\\\"data-open\\\"",
    ] {
        assert!(
            scene.contains(written),
            "the drawing does not write `{written}` -- the client draws the \
             same words the curator does, or the confirming patch is a second \
             movement (§ 5.7)"
        );
    }
    assert!(
        scene.contains("st.optimistic++"),
        "nothing counts the drawings: B-06 reads the counter to prove the DOM \
         moved BEFORE the patch"
    );
}

#[test]
fn a_tap_on_a_canvas_window_co_closes_the_modal() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        scene.contains("data-layer\\\") === \\\"canvas\\\""),
        "the drawing does not read the tapped window's `layer` -- § 5.3: the \
         modal-closing rule reads the LAYER and not the rung, and it applies on \
         put-away as well (S-077, Decision 16.09.)"
    );
    assert!(
        scene.contains("[data-level=\\\"2\\\"]"),
        "nothing looks for the open modal to close with it (§ 5.3)"
    );
}

#[test]
fn the_flip_keeps_its_hands_off_while_the_client_draws() {
    if !library_ships() {
        return;
    }
    let scene = block("SCENE_CLIENT_JS");
    let flip = scene
        .find("function flip(")
        .expect("the FLIP movement is gone");
    let guard = scene
        .find("dataset.optimistic === \\\"1\\\"")
        .expect("nothing tells the movement that the client is drawing (§ 5.8)");
    assert!(
        guard > flip,
        "the guard stands outside `flip()`: § 5.8 asks that a series of taps \
         draws its END state, and an animation per patch is the flashing it \
         forbids"
    );
}

/// welle-tb tb-zcol B: B-06 proves "before the pass" by a POSITIVE signal, the
/// client's own drawing counter moving inside the tap's task, never by a
/// wall-clock assumption. M-INT-F-W (build04, 24 threads under load): B-06 and
/// the co-close read 110 ms for a synchronous handler -- the host's scheduler,
/// not the client -- and the colony line went red while the isolation was 10/10
/// green. No patch can land inside one synchronous task, so a counter that
/// moved by exactly one there is the drawing before the pass.
#[test]
fn b06_reads_the_drawing_counter_and_no_wall_clock() {
    const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
    if !repo(DRIVER).is_file() {
        eprintln!("SKIP: {DRIVER} is not in this tree");
        return;
    }
    let driver = read(DRIVER);
    for (name, end) in [
        ("async function B06(ctx) {", "async function coClose(ctx) {"),
        ("async function coClose(ctx) {", "async function B07(ctx) {"),
    ] {
        let start = driver
            .find(name)
            .unwrap_or_else(|| panic!("{name} is gone"));
        let stop = driver[start..]
            .find(end)
            .unwrap_or_else(|| panic!("{end} does not follow {name}"));
        let body = &driver[start..start + stop];
        // Since welle-testing tv-bn M10b through the shared tap (`tileTap`), which reads
        // the counter in the tap's own task and hands its delta back as `drawings`.
        assert!(
            body.contains("__displayScene.optimistic")
                || (body.contains("tileTap(") && body.contains(".drawings")),
            "{name} never reads the client's drawing counter -- the positive \
             signal that the DOM moved before the patch (§ 5.7)"
        );
        assert!(
            !body.contains(">= 50"),
            "{name} still judges a synchronous handler by the wall clock: under \
             load that measures the host's scheduler, not the client"
        );
    }
}

/// The browser driver's text between two markers: a proof body (`async function B24(ctx) {`
/// up to `async function B25(ctx) {`) or a helper of the run (re-check tv-bn-layout B).
fn driver_span(src: &str, from: &str, to: &str) -> String {
    let start = src.find(from).unwrap_or_else(|| panic!("{from} is gone"));
    let stop = src[start..]
        .find(to)
        .unwrap_or_else(|| panic!("{to} does not follow {from}"));
    src[start..start + stop].to_string()
}

/// welle-testing tv-bn-layout B, M1 (TV-Z3, TV-K5): the browser run's marker is >= 3 x
/// the loaded P95 of a run and scales with the lane, and the harness kill over it leaves
/// room for what runs outside it. 690 s for a colony line was 2.1 x its P95 (272-330 s
/// under `--load 12`, C1), unscaled, and 30 s under the 720 s kill: L6 ran into the kill
/// 3 of 3 and lost the name of the proof it hung in.
#[test]
fn the_run_marker_sits_under_the_leash_with_room() {
    const WAIT: &str = "workshop/tools/wait.mjs";
    const NEXTEST: &str = ".config/nextest.toml";
    if !repo(WAIT).is_file() || !repo(NEXTEST).is_file() {
        eprintln!("SKIP: {WAIT} or {NEXTEST} is not in this tree");
        return;
    }
    let wait = read(WAIT);
    let marker = |kind: &str| -> u64 {
        let key = format!("{kind}: Math.round(");
        let at = wait.find(&key).unwrap_or_else(|| {
            panic!("RUN_HANG_MS.{kind} is not scaled with the lane (TIME_FACTOR)")
        });
        let rest = &wait[at + key.len()..];
        let num: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '_')
            .filter(|c| *c != '_')
            .collect();
        let close = rest.find(')').expect("Math.round( closes");
        assert!(
            rest[..close].contains("* TIME_FACTOR"),
            "RUN_HANG_MS.{kind} is not multiplied by TIME_FACTOR"
        );
        num.parse::<u64>().expect("a marker in ms") / 1000
    };
    let toml = read(NEXTEST);
    let leash = |binary: &str| -> u64 {
        // The override whose next line is the leash (the binary is named in other
        // overrides too: its test group, its output).
        let line = toml
            .match_indices(&format!("filter = 'binary_id(=meclaw-cells::{binary})'"))
            .filter_map(|(at, _)| toml[at..].lines().nth(1))
            .find(|l| l.starts_with("slow-timeout"))
            .unwrap_or_else(|| panic!("no leash for {binary}"));
        let n: String = line
            .split("terminate-after = ")
            .nth(1)
            .expect("terminate-after")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        assert!(line.contains("period = \"60s\""), "{binary}: {line}");
        60 * n.parse::<u64>().expect("a count")
    };
    let (colony, sheet) = (marker("colony"), marker("sheet"));
    assert!(
        colony >= 1020,
        "the colony marker {colony} s is under 3 x the loaded P95 (340 s)"
    );
    let colony_leash = leash("710_the_colony_holds_in_both_engines_browser");
    let sheet_leash = leash("710_the_sheet_holds_in_both_engines_browser");
    assert!(
        colony_leash >= colony + 180,
        "the colony kill {colony_leash} s leaves under 180 s over the marker {colony} s \
         (boot, launch and close run outside it)"
    );
    assert!(
        sheet_leash >= sheet + 90,
        "the sheet kill {sheet_leash} s leaves under 90 s over the marker {sheet} s"
    );
    // Rust leads the two numbers (orchestrator decision tv-bn-lb.2, re-check M1-R): the
    // constants in `deadline` go to the driver as `MECLAW_WEBKIT_RUN_MS`, `wait.mjs` keeps
    // the same ones for a run without the tests. The child runs under the run's marker
    // plus what runs outside it, then gets SIGTERM and a HANG mark to end, then SIGKILL,
    // a HANG mark for the tree to be gone (re-check B fix2, M3) and a FLOOR mark for the
    // streams to close (H1). At factor 1 all of that ends before the leash even when the
    // test spent a whole boot mark and a whole HANG (stage; the page parts, themselves
    // under a HANG marker, M2) before the child started, with 30 s to spare.
    // Above factor 1 the leash, which knows no factor, comes first: whoever introduces a
    // factor > 1 raises the leash in the same strand (`.config/nextest.toml`).
    use meclaw_testing::deadline::{
        BOOT_HANG, BROWSER_RUN_COLONY, BROWSER_RUN_OUTSIDE, BROWSER_RUN_SHEET, FLOOR, HANG,
    };
    use std::time::Duration;
    assert_eq!(
        BROWSER_RUN_COLONY.as_secs(),
        colony,
        "Rust and wait.mjs disagree on the colony marker"
    );
    assert_eq!(
        BROWSER_RUN_SHEET.as_secs(),
        sheet,
        "Rust and wait.mjs disagree on the sheet marker"
    );
    let spare = Duration::from_secs(30);
    // After the marker: SIGTERM mark, survivors mark, streams mark.
    let after = HANG + HANG + FLOOR;
    let colony_end = BOOT_HANG + HANG + BROWSER_RUN_COLONY + BROWSER_RUN_OUTSIDE + after + spare;
    let sheet_end = HANG + BROWSER_RUN_SHEET + BROWSER_RUN_OUTSIDE + after + spare;
    // Welle-Merge R<->N (NS, table N-K11; OR-T18-7): under a CPU quota the marks run
    // scaled by the quota profile's factor `QUOTA_TIME_FACTOR` (F = 3, as every quota run
    // and the profile's own leash), so its leash holds the factor-1 end at that factor.
    let quota_leash = |binary: &str| -> u64 {
        let pat =
            format!("[[profile.quota.overrides]]\nfilter = 'binary_id(=meclaw-cells::{binary})'\n");
        let at = toml
            .find(&pat)
            .unwrap_or_else(|| panic!("no quota leash for {binary}"));
        let line = toml[at + pat.len()..].lines().next().unwrap_or_default();
        assert!(
            line.starts_with("slow-timeout") && line.contains("period = \"60s\""),
            "{binary}: {line}"
        );
        let n: String = line
            .split("terminate-after = ")
            .nth(1)
            .expect("terminate-after")
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        60 * n.parse::<u64>().expect("a count")
    };
    for (binary, end) in [
        ("710_the_colony_holds_in_both_engines_browser", colony_end),
        ("710_the_sheet_holds_in_both_engines_browser", sheet_end),
    ] {
        let want =
            meclaw_testing::deadline::scaled_by(end, meclaw_testing::deadline::QUOTA_TIME_FACTOR);
        let got = quota_leash(binary);
        assert!(
            got >= want.as_secs(),
            "{binary}: the quota kill {got} s comes before the marker scaled by QUOTA_TIME_FACTOR \
             has ended the child ({} s)",
            want.as_secs()
        );
    }
    assert!(
        colony_leash >= colony_end.as_secs(),
        "the colony kill {colony_leash} s comes before the Rust marker has ended the child \
         ({} s: boot, stage, run, outside, SIGTERM, survivors, streams, spare)",
        colony_end.as_secs()
    );
    assert!(
        sheet_leash >= sheet_end.as_secs(),
        "the sheet kill {sheet_leash} s comes before the Rust marker has ended the child \
         ({} s: page parts, run, outside, SIGTERM, survivors, streams, spare)",
        sheet_end.as_secs()
    );
}

/// re-check tv-bn-layout B fix2, M2 (TV-Z3; no blocking sync I/O in async): the sheet's
/// `compose.py` runs under the child marker too, never under a bare `.output()` -- the
/// leash budgets the page parts at a HANG mark, and this is what holds them to it.
#[test]
fn the_sheet_page_parts_run_under_the_child_marker() {
    let sheet = read("crates/meclaw-cells/tests/710_the_sheet_holds_in_both_engines_browser.rs");
    let parts = driver_span(&sheet, "fn page_parts(", "\n}\n");
    assert!(
        parts.contains("browser_run::run_marked(") && !parts.contains(".output()"),
        "compose.py runs without the Rust marker: {parts}"
    );
    // re-check B fix3 H3 (bundle N): the marker the leash budgets, not just any marker.
    assert!(
        parts.contains("meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG),")
            && parts.contains("std::time::Duration::ZERO,"),
        "compose.py runs under another marker than the HANG mark the leash budgets: {parts}"
    );
}

/// Both browser tests run their driver under the child marker, with their own number and
/// the outside part scaled with the lane (re-check B fix2, M1: TV-Z3).
#[test]
fn both_browser_tests_run_their_driver_under_the_child_marker() {
    let marker = read("crates/meclaw-cells/tests/support/browser_run.rs");
    let outside = driver_span(&marker, "pub fn outside()", "\n}\n");
    assert!(
        outside.contains("scaled(BROWSER_RUN_OUTSIDE)"),
        "the part of the child marker outside the run does not scale with the lane: {outside}"
    );
    let fixture = read("crates/meclaw-cells/tests/support/display_colony.rs");
    let driving = driver_span(&fixture, "impl Driving {", "\n}\n");
    let driving_marks =
        driving.contains("browser_run::run_marked(&mut self.0, what, run, outside)");
    for (file, run) in [
        (
            "crates/meclaw-cells/tests/710_the_colony_holds_in_both_engines_browser.rs",
            "browser_run::colony_run()",
        ),
        (
            "crates/meclaw-cells/tests/710_the_sheet_holds_in_both_engines_browser.rs",
            "browser_run::sheet_run()",
        ),
    ] {
        let src = read(file);
        let span = driver_span(&src, "fn drive(", "\n}\n");
        // Since M7b13 the colony line's driver is the fixture's child (`Colony::drive`,
        // re-check tv-bn M7b12 A-M3): the test calls `Driving::run_marked`, which must
        // hand the child to the same marker -- nothing else counts as marked.
        let marked = span.contains("browser_run::run_marked(")
            || (span.contains(".run_marked(") && driving_marks);
        assert!(
            marked && span.contains(run) && span.contains("browser_run::outside()"),
            "{file}: the driver's child runs without the Rust marker {run}"
        );
        assert!(
            !span.contains(".output()"),
            "{file}: the driver's child still runs under a bare `.output()`"
        );
    }
}

/// welle-testing tv-bn-layout B, M2/M3 (TV-B1, TV-B3): what a proof needs from a fresh
/// page is a MOUNTED client, waited for without a catch -- never frames (a page that
/// draws few, on a lane that stalls them: 71 s / 270 s for 78 frame waits, C1) and never
/// a swallowed mount (a socket that never mounts left every proof a skip and the line
/// green). Pinned on the run's start, the exit page, the sibling, B-26, B-28 and B-33.
#[test]
fn the_run_waits_on_a_mounted_client_and_not_on_frames() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let src = read(DRIVER_B);
    let spans = [
        (
            "run start",
            driver_span(&src, "const lab = laboratory({", "const api = live"),
        ),
        (
            "exit page",
            driver_span(&src, "exitPage: async (name) => {", "fresh: null,"),
        ),
        (
            "sibling",
            driver_span(
                &src,
                "sibling: async (name) => {",
                "/** Views onto the screen",
            ),
        ),
        (
            "B-26",
            driver_span(
                &src,
                "async function B26(ctx) {",
                "async function B27(ctx) {",
            ),
        ),
        (
            "B-28",
            driver_span(
                &src,
                "async function B28(ctx) {",
                "async function B29(ctx) {",
            ),
        ),
        (
            "B-33",
            driver_span(
                &src,
                "async function B33(ctx) {",
                "async function B34(ctx) {",
            ),
        ),
    ];
    for (name, body) in &spans {
        for banned in [
            "drawn2(",
            "frame1(",
            "requestAnimationFrame",
            "waitForSelector(",
        ] {
            assert!(!body.contains(banned), "{name} still waits with `{banned}`");
        }
        assert!(
            !body.contains(".catch(() => {})"),
            "{name} swallows a failure with `.catch(() => {{}})`"
        );
    }
    for (name, body) in &spans[..3] {
        assert!(
            body.contains("mounted("),
            "{name} does not wait for the mounted client"
        );
    }
    assert!(
        !src.contains("const SHELL_MARKER_MS = 30000"),
        "the shell marker is a literal, not the scaled HANG marker (TV-Z3)"
    );
}

/// welle-testing tv-bn-layout B, M4: B-26 opens the conversation with a whole tap --
/// `pointerdown` alone is drawn by the client and never heard by `phx-click`, so no pass
/// confirmed it and the next pass took the window back mid-typing -- and waits for the
/// pass that answered THAT tap before it reads the level: the tile carries the tap's own
/// `phx-value-seq` and the wait is `tapAnswered` on `data-tap-seq` (bundle N, merge
/// condition 1 of re-check B fix1). A grown `data-acted` was any touch of the window, and
/// after the merge of branch A `passAfter` was no function of the driver at all.
#[test]
fn b26_opens_the_conversation_with_a_whole_tap() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let body = driver_span(
        &read(DRIVER_B),
        "async function B26(ctx) {",
        "async function B27(ctx) {",
    );
    assert!(
        !body.contains(".dispatchEvent(\"pointerdown\""),
        "B-26 still taps with a bare `pointerdown` the colony never hears (TV-B3)"
    );
    // Since welle-testing tv-bn M10b the whole tap -- `click` and `phx-value-seq`
    // included -- is the shared `tileTap` (pinned in
    // `every_tile_tap_of_the_layout_driver_proves_it_was_sent`).
    assert!(
        body.contains("tileTap(ctx.page"),
        "B-26's tap is no whole tap"
    );
    assert!(
        body.contains("tapAnswered(ctx.page, tapped.for, seq,"),
        "B-26 does not bind its wait to its own tap's seq (merge condition 1, A-M1)"
    );
    assert!(
        !body.contains("passAfter("),
        "B-26 still waits for any bigger `data-acted` (merge condition 1)"
    );
}

/// welle-testing tv-bn-layout B, M5 (TV-Z1/Z2/Z10): B-24's bound is exact on the page's
/// clock in the driver's hand (`page.clock`, ten seconds, at most six tones), never a
/// `Date.now()` delta between two readings of a live page.
#[test]
fn b24_counts_tones_on_the_page_clock() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let body = driver_span(
        &read(DRIVER_B),
        "async function B24(ctx) {",
        "async function B25(ctx) {",
    );
    assert!(
        !body.contains("at: Date.now()") && !body.contains("now.at - from.at"),
        "B-24 still stamps its readings with the page's wall clock"
    );
    assert!(
        body.contains("clock.install("),
        "B-24 does not install the page clock"
    );
    assert!(
        body.contains("clock.runFor("),
        "B-24 does not run the page clock"
    );
    // GH #1209 (M11): the bound's stretch is read from the INSTALLED page clock in the same
    // task as the tone count, and no `pauseAt` aims at an absolute time the running clock
    // overtakes under load ("Cannot fast-forward to the past").
    assert!(
        body.contains("page_at: performance.now()") && body.contains("now.page_at - from.page_at"),
        "B-24 does not read its stretch from the page's own clock"
    );
    // Re-check M11 (M1): bound to the member, not to the text `clock.pauseAt(` -- a line
    // break before the dot, `?.`, an alias or `{ pauseAt } = page.clock` reach the same call.
    assert!(
        !body.contains(".pauseAt") && !body.contains("{ pauseAt"),
        "B-24 aims the page clock at an absolute time again"
    );
    assert!(
        body.contains("rang no tone"),
        "B-24 has no positive half: a running context that rang nothing passes"
    );
}

/// welle-testing tv-bn-layout B, M6 (TV-Z3): a HANG names the last state the page showed.
/// Without a reader of its own, `until` reads the predicate itself -- `last state: null`
/// was the message of every wait that passed none.
#[test]
fn a_hang_names_the_last_state_it_saw() {
    const WAIT: &str = "workshop/tools/wait.mjs";
    if !repo(WAIT).is_file() {
        eprintln!("SKIP: {WAIT} is not in this tree");
        return;
    }
    let wait = read(WAIT);
    let body = driver_span(&wait, "export async function until(", "\n}\n");
    assert!(
        body.contains("state || (() => target.evaluate(pred, arg))"),
        "`until` on a Playwright page has no default reading for its HANG line"
    );
    assert!(
        body.contains("Math.max(deadline, MIN_WAIT_MS)"),
        "`until` takes a positive wait under 10 s x factor (TV-Z3)"
    );
    assert!(
        body.contains("hang(what"),
        "one CDP ask is unbounded: a CDP that never answers ends at the run marker"
    );
}

const DRIVER_B: &str = "workshop/tools/display-layout-browser.mjs";

/// welle-tb tb-zcol B, welle-tv tv-s1-browser (VORGABEN TV-B4): the colony line's
/// two shots are evidence, and evidence never judges. M-INT-F-W: L3
/// (chromium/phone) died in its first `page.screenshot` (30 s under load,
/// isolation 10/10 green) and B-06's tap was never measured. The driver asks for
/// a shot only after two animation frames (the positive signal that the page
/// draws, bounded by the HANG marker of `wait.mjs`) and never ends the line
/// early. Two frames that never come make the line red after every proof ran
/// (`frames_failed`, post-xhigh-8 B3); the shot itself (`stillShot`) carries CSS
/// pixels, no animations, no caret and its own `SHOT_HANG_MS`, and a shot that
/// does not come back is an annotation by name in the receipt (`shots_not_taken`)
/// and judges nothing -- TV-B4, declared in re-check tv-bn-drv2-fix1 M (it
/// replaced post-xhigh-8 M2).
#[test]
fn the_colony_lines_shots_wait_for_a_frame_and_never_judge() {
    const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
    const LAB: &str = "workshop/tools/display-browser-lab.mjs";
    if !repo(DRIVER).is_file() || !repo(LAB).is_file() {
        eprintln!("SKIP: {DRIVER} or {LAB} is not in this tree");
        return;
    }
    let driver = read(DRIVER);
    let start = driver
        .find("async function evidence(name) {")
        .expect("the evidence helper is gone");
    let body = &driver[start..];
    let body = &body[..body.find("\n}\n").expect("evidence is one function")];
    assert!(
        body.contains("requestAnimationFrame(() => requestAnimationFrame("),
        "a shot is asked for before the page has drawn a frame"
    );
    assert!(
        body.contains("frames_failed"),
        "two frames that never come leave no mark on the line"
    );
    assert!(
        body.contains("const shot = await lab.shot(name);")
            && body.contains("if (!shot.taken)")
            && body.contains("report.shots_not_taken[name] = shot.why;"),
        "a shot that does not come back is no annotation in the receipt"
    );
    assert!(
        body.contains("await hang(") && !body.contains("setTimeout("),
        "the two frames before a shot are not bounded by the HANG marker"
    );
    assert!(
        driver.contains("const framesFailed = Object.keys(report.frames_failed || {}).length > 0;")
            && driver.contains("await emit(framesFailed ||"),
        "a compositor that never drew leaves the line green"
    );
    assert!(
        !driver.contains("shots_failed") && !driver.contains("shotsFailed"),
        "a missing evidence shot still judges the line (TV-B4: an annotation)"
    );
    assert!(
        !driver.contains("await lab.shot(report.run"),
        "a shot outside `evidence` can still end the line"
    );
    let lab = read(LAB);
    let start = lab
        .find("export async function stillShot(page, opts) {")
        .expect("the lab's still shot is gone");
    let still = &lab[start..];
    let still = &still[..still.find("\n}\n").expect("stillShot is one function")];
    for option in [
        "scale: \"css\"",
        "animations: \"disabled\"",
        "caret: \"hide\"",
        "timeout: SHOT_HANG_MS",
    ] {
        assert!(
            still.contains(option),
            "the evidence shot lacks `{option}` (TV-B4)"
        );
    }
    let start = lab.find("shot: (name) =>").expect("the lab's shot is gone");
    let shot = &lab[start..];
    let shot = &shot[..shot
        .find("\n    async finish(")
        .expect("shot is one member")];
    assert!(
        shot.contains("stillShot(page,")
            && shot.contains("() => ({ taken: true })")
            && shot.contains("return { taken: false, why };"),
        "a shot that does not come back rejects instead of answering its status"
    );
}

/// welle-testing tv-bn-layout C (re-check tv-bn-layout C-M2, VORGABEN TV-B4): a shot
/// that did not come back is an annotation in EVERY driver, not only in the colony line.
/// Since `lab.shot` answers `{ taken, why }` instead of throwing, a driver that writes
/// `await lab.shot(...)` and drops the answer loses the picture without a trace -- map,
/// blocks and page did exactly that. The lab says the reason itself (`SHOT <name> not
/// taken`, printed as a `LOG` line by `finish`), and each driver keeps the answer and
/// names the missing picture beside its verdict, never inside it.
#[test]
fn every_driver_keeps_the_answer_of_an_evidence_shot() {
    const LAB: &str = "workshop/tools/display-browser-lab.mjs";
    const DRIVERS: [&str; 4] = [
        "workshop/tools/display-map-browser.mjs",
        "workshop/tools/display-blocks-browser.mjs",
        "workshop/tools/display-page-browser.mjs",
        "workshop/tools/display-layout-browser.mjs",
    ];
    if !repo(LAB).is_file() || DRIVERS.iter().any(|d| !repo(d).is_file()) {
        eprintln!("SKIP: the drivers are not in this tree");
        return;
    }
    let lab = read(LAB);
    assert!(
        lab.contains("noise.push(`SHOT ${name} not taken: ${why}`);")
            && lab.contains("noise.map((n) => `LOG ${n}\\n`)"),
        "the lab no longer says a missing shot beside the run's output"
    );
    let tools = repo("workshop/tools");
    for entry in std::fs::read_dir(&tools).expect("the tools directory is a directory") {
        let path = entry.expect("a directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("mjs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a driver is text");
        for (n, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") || code.starts_with('*') || !code.contains("lab.shot(") {
                continue;
            }
            assert!(
                code.contains("= await lab.shot("),
                "{}:{}: the answer of an evidence shot is dropped (TV-B4: a missing \
                 picture is an annotation): {code}",
                path.display(),
                n + 1
            );
        }
    }
    // map and blocks: the two drivers that report their shot beside a `checks` verdict.
    for driver in &DRIVERS[..2] {
        let text = read(driver);
        assert!(
            text.contains("const shotsNotTaken = shot.taken ? {} : { [shotName]: shot.why };")
                && text.contains("shots_not_taken: shotsNotTaken"),
            "{driver} does not name a missing shot in its report"
        );
        let verdict = text
            .find("const ok = Object.values(checks)")
            .expect("the verdict over the checks");
        assert!(
            !text[verdict..].contains("shot.taken &&")
                && !text[verdict..].contains("&& shot.taken"),
            "{driver} judges by its evidence shot"
        );
    }
    let page = read("workshop/tools/display-page-browser.mjs");
    assert!(
        page.contains("return shot.taken ? \"\" : ` shot_not_taken=${name}`;")
            && page
                .matches("const shotField = await evidence(lab, ")
                .count()
                == 3,
        "a page mode drops a missing shot instead of naming it on its line"
    );
}

/// welle-testing tv-bn-layout C2 (VORGABEN TV-B1, TV-K5): B-07's ten taps are paced by
/// the page answering each one -- the client's drawing counter moved by exactly one in
/// the tap's own task -- and never by the page's animation frames. Five frames between
/// two taps (fifty in the series) were 105-132 s of a colony line under `--load 12` on
/// build03 (C1, `report.secs`): a page that draws few frames, and a loaded lane that
/// stalls them, turned a one-second series into two minutes and pushed the line past the
/// third of its run marker. Each tap is its own round trip into the page, so a patch can
/// land between two taps the way it does between a person's.
#[test]
fn b07_paces_its_series_on_the_drawing_and_not_on_frames() {
    const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
    if !repo(DRIVER).is_file() {
        eprintln!("SKIP: {DRIVER} is not in this tree");
        return;
    }
    let driver = read(DRIVER);
    let start = driver
        .find("async function B07(ctx) {")
        .expect("B-07 is gone");
    let stop = driver[start..]
        .find("async function B08(ctx) {")
        .expect("B-08 does not follow B-07");
    let body = &driver[start..start + stop];
    assert!(
        !body.contains("requestAnimationFrame"),
        "B-07 still paces its series on the page's frames: under load they stall, \
         and fifty of them were two minutes of a colony line (TV-K5)"
    );
    // welle-testing tv-bn M10b: every tap of the series is ONE call of the shared tap
    // (`tileTap`), which reads the client's drawing counter (§ 5.7) and its send counter
    // in the tap's own task.
    assert!(
        body.contains("tileTap("),
        "B-07 taps outside the shared tap that proves each tap was drawn and sent"
    );
    let helper = driver
        .split("async function tileTap(page, {")
        .nth(1)
        .expect("the shared tap is gone")
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(
        helper.contains(".optimistic")
            && helper.contains(".sent")
            && helper.contains("phx-value-seq"),
        "the shared tap no longer reads the drawing and the send counter: {helper}"
    );
    assert!(
        body.contains("did not draw every tap"),
        "a tap the client never drew still counts as one of the ten"
    );
    // The series taps at the page's own pace and never waits for LiveView's answer
    // (re-check M10, M1.3): a person does not wait for an ack, and the case in which the
    // screen lost taps (five of ten under `--load 12`, M10) is exactly the tap before
    // the answer. M10's wait on `data-phx-ref-src` stepped around it.
    assert!(
        !body.contains("data-phx-ref-src"),
        "B-07 waits for LiveView's answer between taps -- it no longer drives the case \
         in which taps were lost"
    );
    assert!(
        body.contains("did not send every tap") && body.contains("heard all ten taps"),
        "B-07 proves each tap was sent and all ten reached the colony (§ 5.8)"
    );
    assert!(
        body.contains("did not end where ten toggles end"),
        "B-07 checks the end state of ten toggles (§ 5.8)"
    );
}

/// welle-testing tv-bn M10b (re-check M10, M2): every tile tap a layout proof dispatches
/// goes through the shared tap, which proves it was sent (and `tapArrived`, that it was
/// heard). Six places tapped on their own -- one tap, no proof, or a loop whose second
/// tap covered a first one LiveView had dropped. The other `click` in the driver is the
/// TV-A1 marker (`logMarker`, a fresh element that carries no ref and is no tile).
#[test]
fn every_tile_tap_of_the_layout_driver_proves_it_was_sent() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let driver = read(DRIVER_B);
    let clicks: Vec<usize> = driver
        .match_indices("new MouseEvent(\"click\"")
        .map(|(i, _)| i)
        .collect();
    let owner = |at: usize| {
        driver[..at]
            .rfind("\nasync function ")
            .map(|f| driver[f + 1..].split('(').next().unwrap_or("").to_string())
            .unwrap_or_default()
    };
    let owners: Vec<String> = clicks.iter().map(|&i| owner(i)).collect();
    assert_eq!(
        owners,
        vec![
            "async function tileTap".to_string(),
            "async function logMarker".to_string()
        ],
        "a `click` outside the shared tap and the TV-A1 marker taps a tile without proof"
    );
    for site in [
        "async function B06(",
        "async function coClose(",
        "async function B07(",
        "async function B23(",
        "async function B26(",
    ] {
        let body = driver
            .split(site)
            .nth(1)
            .expect(site)
            .split("\nasync function ")
            .next()
            .unwrap();
        assert!(
            body.contains("tileTap(") && body.contains("notSent("),
            "{site} taps without proving the tap was sent"
        );
    }
}

/// welle-testing tv-bn-layout C3 (re-check tv-bn-layout C fix1 H4, VORGABEN TV-B1, TV-S2,
/// TV-Z3): the page driver's tap -- the driver of the gh767 tap proof -- waits on the
/// page and never on the clock. Before, it waited 15 s for the scene hook (a literal,
/// no lane factor, no `HANG:` line), tried the tile three times 800 ms apart and slept
/// 1200 ms before its evidence shot; a loaded lane stretched none of that and a slow
/// patch read as `tapped=0`. Now the hook and the tile are `until` with the HANG marker
/// (the tap is dispatched in the same page task that finds the tile), and the shot
/// waits for two of the page's frames under `hang`. The verdict stays in gh767's
/// `wait_tree`; the shot is evidence.
#[test]
fn the_page_drivers_tap_waits_on_the_page_and_never_on_the_clock() {
    const DRIVER: &str = "workshop/tools/display-page-browser.mjs";
    if !repo(DRIVER).is_file() {
        eprintln!("SKIP: {DRIVER} is not in this tree");
        return;
    }
    let driver = read(DRIVER);
    let mut found = Vec::new();
    for (n, line) in driver.lines().enumerate() {
        let code = line.trim_start();
        if code.starts_with("//") || code.starts_with('*') || code.starts_with("/**") {
            continue;
        }
        for (needle, why) in [
            ("sleep(", "a sleep in the driver (TV-S2)"),
            (
                "timeout: 15_000",
                "a fixed mark without the lane factor (TV-Z3)",
            ),
            (
                "timeout: 20_000",
                "a fixed mark without the lane factor (TV-Z3)",
            ),
            (
                ".catch(() => {})",
                "an expiry that leaves no `HANG:` line (TV-Z3)",
            ),
        ] {
            if code.contains(needle) {
                found.push(format!("{DRIVER}:{}: {why}: {code}", n + 1));
            }
        }
    }
    let start = driver
        .find("async function runTap(lab, page) {")
        .expect("the tap mode is gone");
    let body = &driver[start..];
    let body = &body[..body.find("\n}\n").expect("runTap is one function")];
    for (needle, why) in [
        (
            "await until(page, \"the scene hook never mounted",
            "the hook is not awaited with the HANG marker",
        ),
        (
            "hit = await until(",
            "the tile is not tapped by a wait on the page",
        ),
        (
            "requestAnimationFrame(() => requestAnimationFrame(",
            "the evidence shot does not wait for the page's frames",
        ),
        (
            "await hang(\"two frames after the tap",
            "the two frames are not bounded by the HANG marker",
        ),
    ] {
        if !body.contains(needle) {
            found.push(format!("runTap: {why}"));
        }
    }
    // C4 (re-check tv-bn-layout C fix2 H-a/H-c): `waitForSelector` waited for a VISIBLE
    // window by default; the C3 predicate `!!document.querySelector(sel)` only for an
    // attached one, and `found.boundingBox()` needs layout -- an attached canvas without
    // a box reads `clicked=0`. The wait asks for the box, and the driver's `until` is
    // `wait.mjs`'s (only a timeout is `HANG:`, with the page's last state).
    let start = driver
        .find("async function runPage(lab, page) {")
        .expect("the page mode is gone");
    let page_body = &driver[start..];
    let page_body = &page_body[..page_body.find("\n}\n").expect("runPage is one function")];
    // C5 (re-check C fix3 M1): the needle is the predicate line itself -- the state
    // reader below it carries `getClientRects().length > 0` too, so a predicate reset
    // to `return !!c;` kept the bare needle green. Exactly once (re-check C fix4 H1,
    // bundle N): a second wait with the same predicate would let a mutant at one of
    // the two survive again, so a second copy is red as well (fail-closed).
    if page_body
        .matches("return !!c && c.getClientRects().length > 0;")
        .count()
        != 1
    {
        found.push(
            "runPage: the window wait asks for an attached canvas, not a laid-out one".into(),
        );
    }
    if !driver.contains("return waitUntil(page, pred, arg, { desc: what, polling, state });") {
        found.push(
            "the driver's `until` is not `wait.mjs`'s (every error reads `HANG:`, no state)".into(),
        );
    }
    assert!(
        found.is_empty(),
        "the page driver's tap waits on the clock:\n{}",
        found.join("\n")
    );
}

/// welle-tv tv-bn-layout A (re-check A-M1..M4): the layout proofs B-01..B-07 and B-14
/// wait on events bound to what they did, and a HANG is red. B-06, the co-close and
/// B-07 wait on THEIR tap's seq (the co-close on the modal it judges); B-07 and the
/// co-close turn a HANG into `bad`; B-03 waits on its own landing with a HANG message
/// and on an instance on a patch counter, never an in-page `setTimeout`; B-05 waits on
/// a PATCH (`patches`), not on scene beats that the 1-s interval also makes; B-01 and
/// B-14 read "no hold reached the colony" only behind a marker over the same socket.
#[test]
fn the_layout_proofs_wait_on_their_own_events() {
    const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
    if !repo(DRIVER).is_file() || !library_ships() {
        eprintln!("SKIP: {DRIVER} is not in this tree");
        return;
    }
    let driver = read(DRIVER);
    let body = |name: &str, end: &str| -> String {
        let start = driver
            .find(name)
            .unwrap_or_else(|| panic!("{name} is gone"));
        let stop = driver[start..]
            .find(end)
            .unwrap_or_else(|| panic!("{end} does not follow {name}"));
        driver[start..start + stop].to_string()
    };
    let b06 = body("async function B06(ctx) {", "async function coClose(ctx) {");
    let co = body("async function coClose(ctx) {", "async function B07(ctx) {");
    let b07 = body("async function B07(ctx) {", "async function B08(ctx) {");
    for (name, b) in [("B-06", &b06), ("coClose", &co), ("B-07", &b07)] {
        assert!(
            (b.contains("phx-value-seq") || b.contains("tileTap(")) && b.contains("tapAnswered("),
            "{name} does not bind its wait to its own tap's seq (A-M1)"
        );
        assert!(
            !b.contains("passAfter("),
            "{name} still waits for any bigger `data-acted` (A-M1)"
        );
    }
    assert!(
        co.contains("tapAnswered(ctx.page, found.modal"),
        "the co-close waits on another window than the modal it judges (A-M1)"
    );
    // Bundle N M3 (re-check A fix1 H4): the co-close's HANG way back keeps the presses
    // it gathered in B-06's value, not only `co.why`.
    assert!(
        b06.contains("co_presses: co.presses"),
        "B-06 drops the co-close's presses on its HANG way back (A fix1 H4)"
    );
    assert!(
        b07.contains("if (unanswered) return bad("),
        "B-07 annotates a HANG and judges on (A-M2)"
    );
    assert!(
        !co.contains(".catch(() => {})"),
        "the co-close swallows a HANG message (A-M2, H1)"
    );
    let b03 = body("async function B03(ctx) {", "async function B04(ctx) {");
    assert!(
        !b03.contains("setTimeout(") && !b03.contains("ctx.landed("),
        "B-03 still waits on an in-page timer or a landing that says only `false` (A-M2/A-M3)"
    );
    assert!(
        b03.contains("if (unlanded) return bad("),
        "B-03's landing HANG is not red (A-M2)"
    );
    let b05 = body("async function B05(ctx) {", "async function B06(ctx) {");
    assert!(
        !b05.contains("sceneBeats(") && b05.contains("patchAfter("),
        "B-05 still counts scene beats the 1-s interval makes (A-M3)"
    );
    for (name, end) in [
        ("async function B01(ctx) {", "async function B02(ctx) {"),
        ("async function B14(ctx) {", "async function standsStill("),
    ] {
        let b = body(name, end);
        let marker = b.find("logMarker(").unwrap_or(usize::MAX);
        let read = b
            .rfind("ctx.log(\"hold\")")
            .expect("the hold count is gone");
        assert!(
            marker < read,
            "{name} reads `no hold` without a marker over the same path (A-M4, TV-A1)"
        );
    }
    // Bundle N M3 (re-check A fix1 H1): the marker waits on ITS OWN line -- a `for` no
    // other tap carries (`view.marker.<seq>`), counted by the fixture under `&for=` --
    // and on a count that grew (`>`), never on "any tap count other than before".
    let marker = body(
        "async function logMarker(ctx, desc) {",
        "const MINUTE_HANG_MS",
    );
    assert!(
        marker.contains("\"view.marker.\" + nextTapSeq()")
            && marker.contains("ctx.log(\"tap\", oid)")
            && marker.contains("> was")
            && !marker.contains("!== was"),
        "the TV-A1 marker waits on any tap, not on its own line (A fix1 H1)"
    );
    assert!(
        driver.contains("\"&for=\" + encodeURIComponent(forOid)"),
        "`ctx.log` cannot ask for the taps of one window (A fix1 H1)"
    );
    assert!(
        read("crates/meclaw-cells/tests/support/display_colony.rs")
            .contains("body_of(e)[\"event\"][\"value\"][\"for\"]"),
        "the fixture's log cannot count the taps of one window (A fix1 H1)"
    );
    // The marker and the hold count read GESTURES by name: the `web` cell sends every
    // gesture on the route `event`, so a count by route read 0 whatever the finger did
    // (measured A': `route:tap=0` 30 s after the marker's tap, all twenty lines).
    assert!(
        driver.contains("\"?event=\" + gesture") && !driver.contains("\"?route=\" + route"),
        "`ctx.log` counts a gesture by a route no gesture travels on (A-M4)"
    );
    assert!(
        read("crates/meclaw-cells/tests/support/display_colony.rs")
            .contains("body_of(e)[\"event\"][\"name\"]"),
        "the fixture's log does not count gestures by their name (A-M4)"
    );
    let scene = block("SCENE_CLIENT_JS");
    assert!(
        scene.contains("this.__scene.st.patches = (this.__scene.st.patches || 0) + 1;"),
        "the scene hook counts no patches: `ticks` mixes them with the 1-s interval"
    );
}

/// welle-testing tv-bn M10e (re-check M10d B-M1, B-M2): a tap's ARRIVAL is judged as
/// exactly one (§ 5.8: each tap is one step -- a tap delivered twice flips its window
/// back, and ten taps delivered twice keep the same parity), and a missing counted log
/// never reads as "arrived": on a colony it is red, in the sheet it is named in
/// `partial` (fail-closed, never silent). Waiting on `>= 1` is fine; the verdict after
/// the wait is `=== 1`.
#[test]
fn a_taps_arrival_is_exactly_once_and_never_silent() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let driver = read(DRIVER_B);
    let arrived = driver
        .split("async function tapArrived(ctx, seq, desc, value) {")
        .nth(1)
        .expect("`tapArrived` takes the value it annotates")
        .split("\n}\n")
        .next()
        .unwrap();
    assert!(
        !arrived.contains("return undefined"),
        "a missing counted log still passes as `undefined`: {arrived}"
    );
    assert!(
        arrived.contains("!== 1") && arrived.contains("times"),
        "the arrival is waited for on `>= 1` and judged on exactly one: {arrived}"
    );
    assert!(
        arrived.contains("arrival unproven: no counted log"),
        "a missing counted log names itself: {arrived}"
    );
    let start = driver
        .find("async function B07(ctx) {")
        .expect("B-07 is gone");
    let stop = driver[start..]
        .find("async function B08(ctx) {")
        .expect("B-08 follows B-07");
    let b07 = &driver[start..start + stop];
    assert!(
        !b07.contains("h >= 1).length") && !b07.contains("else {\n      heard = null;"),
        "B-07 counts a tap heard twice as heard, or drops the arrival silently: {b07}"
    );
    assert!(
        b07.contains("h === 1") && b07.contains("arrival unproven: no counted log"),
        "B-07 judges each of the ten on exactly one and names a missing log"
    );
    // M10e re-check B-M1: the `=== 1` is read BEHIND a marker over the same socket
    // (TV-A1), never on the reading that ended the wait -- a second delivery still on its
    // way is not in that one.
    let (wait, verdict) = arrived
        .split_once("logMarker(ctx")
        .expect("`tapArrived` puts a marker between the wait and the verdict");
    assert!(
        wait.contains(">= 1")
            && verdict.contains("const times = await heard()")
            && verdict.contains("!== 1"),
        "`tapArrived` waits, THEN the marker, THEN reads `times` for `=== 1`: {arrived}"
    );
    let (b07_wait, b07_verdict) = b07
        .split_once("logMarker(ctx")
        .expect("B-07 puts a marker between the wait and the verdict");
    assert!(
        b07_wait.contains("h >= 1")
            && b07_verdict.contains("heard = await hearing();")
            && b07_verdict.contains("every((h) => h === 1)"),
        "B-07 reads the ten again behind the marker and judges that reading: {b07}"
    );
    let start = driver
        .find("async function B26(ctx) {")
        .expect("B-26 is gone");
    let stop = driver[start..].find("\n}\n").expect("B-26 ends");
    let b26 = &driver[start..start + stop];
    assert!(
        !b26.contains("after === before")
            && b26.contains("enterOnce(ctx, enterBefore, shape.event)"),
        "B-26 counts the Enter's own event, not `any newer row`: {b26}"
    );
    let once = driver
        .split("async function enterOnce(ctx, enterBefore, event) {")
        .nth(1)
        .expect("the Enter's arrival is judged in one place")
        .split("\n}\n")
        .next()
        .unwrap();
    let (_, enter_verdict) = once
        .split_once("logMarker(ctx")
        .expect("the Enter's count is read behind a marker");
    assert!(
        enter_verdict.contains("times !== 1"),
        "the Enter arrived exactly once, read behind the marker: {once}"
    );
}

/// `stampLift`'s definition down to its option objects (re-check N, M7): the stamp holds
/// only while both listeners capture on the window, ahead of every listener of the page.
/// Without `capture: true` on the `pointerup` the page's own handler hears the lift first
/// and reads the event's natural stamp -- 300 ms on a quiet host, green, and #969 back
/// under load.
const STAMP_LIFT: &str = "window.addEventListener(\"pointerdown\", (down) => {
      window.addEventListener(\"pointerup\", (up) => {
        Object.defineProperty(up, \"timeStamp\", { value: down.timeStamp + gap });
      }, { capture: true, once: true });
    }, { capture: true, once: true });";

/// What in B-33's body makes the gesture's length a wall-clock reading (OR-T12-26, #969):
/// a sleep between down and up, a click with a delay, a stamp taken by a handler, and a
/// "delivered" length that left its category waved through as a partial pass. And what
/// the deterministic form needs: stamped events and the constants guard. The lift's stamp
/// is needled where it is USED, not only where `stampLift` is defined (re-check N, M2):
/// without the two calls in front of the stalls the stamp is the event's own, 300 ms on a
/// quiet host and green, and only load turns it red -- #969 back as a load flake.
fn b33_clock(body: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for (banned, why) in [
        ("waitForTimeout(", "a sleep between down and up"),
        ("{ delay:", "a click whose length is a sleep"),
        ("= performance.now(); }", "a stamp taken by a handler"),
        ("drifted", "a drifted length waved through"),
        (
            "left its category",
            "a drifted length waved through as partial",
        ),
    ] {
        if body.contains(banned) {
            found.push(why);
        }
    }
    for (needed, why) in [
        (
            "Object.defineProperty(down, \"timeStamp\"",
            "the tap's down is not stamped",
        ),
        (STAMP_LIFT, "a stalled lift is not stamped"),
        (
            "await stampLift(SHORT);\n  const busy = await page.evaluate(",
            "the stalled lift is not stamped before the stall",
        ),
        (
            "await stampLift(SHORT);\n  const queued = await page.evaluate(",
            "the queued lift is not stamped before the stall",
        ),
        (
            "SHORT < WANT && WANT < LONG && STALL > WANT",
            "no guard on the press lengths",
        ),
    ] {
        if !body.contains(needed) {
            found.push(why);
        }
    }
    found
}

/// OR-T12-26 (#969, the form of M10r): B-33 makes every gesture with its length and the
/// page classifies it by the stamps; nothing measured on a clock is a verdict, and no
/// gesture can drift into a silent partial.
#[test]
fn b33_judges_the_gesture_by_its_stamps_not_the_clock() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let src = read(DRIVER_B);
    let body = driver_span(
        &src,
        "async function B33(ctx) {",
        "async function B34(ctx) {",
    );
    let found = b33_clock(&body);
    assert!(found.is_empty(), "B-33 still times its gestures: {found:?}");
}

/// The lock bites: the form before OR-T12-26, word for word in its load-bearing lines.
#[test]
fn the_b33_clock_lock_bites() {
    let old = r#"
    m.addEventListener("pointerdown", () => { window.__b33.down = performance.now(); }, true);
      await page.waitForTimeout(ms);
      await page.click(MARK, { delay: ms });
  const drifted = [];
    value.partial = "the delivered press left its category under load (" + drifted.join(", ")
"#;
    assert_eq!(b33_clock(old).len(), 5 + 5, "{:?}", b33_clock(old));
    let unstamped = "SHORT < WANT && WANT < LONG && STALL > WANT Object.defineProperty(down, \"timeStamp\"\
                     await stampLift(SHORT);\n  const busy = await page.evaluate(\
                     await stampLift(SHORT);\n  const queued = await page.evaluate(";
    assert_eq!(b33_clock(unstamped), vec!["a stalled lift is not stamped"]);
}

/// The lock bites on the driver itself (re-check N, M2): the needles are the real lines,
/// so the clean body is clean, and each way the stamp or the guard can go still bites.
#[test]
fn the_b33_stamp_is_needled_where_it_is_used() {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return;
    }
    let src = read(DRIVER_B);
    let body = driver_span(
        &src,
        "async function B33(ctx) {",
        "async function B34(ctx) {",
    );
    assert!(b33_clock(&body).is_empty(), "{:?}", b33_clock(&body));
    // The stamp's phase (re-check N, M7): each way a listener of `stampLift` stops
    // capturing on the window ahead of the page is red, the `pointerup` above all.
    assert_eq!(
        body.matches(STAMP_LIFT).count(),
        1,
        "B-33 defines stampLift once"
    );
    let options = "{ capture: true, once: true }";
    let inner = STAMP_LIFT.find(options).expect("the pointerup's options");
    let outer = STAMP_LIFT
        .rfind(options)
        .expect("the pointerdown's options");
    assert!(
        inner < outer,
        "the pointerup listener stands inside the pointerdown one"
    );
    let at = |off: usize, with: &str| {
        format!(
            "{}{with}{}",
            &STAMP_LIFT[..off],
            &STAMP_LIFT[off + options.len()..]
        )
    };
    for (what, phaseless) in [
        ("the lift without capture", at(inner, "{ once: true }")),
        (
            "the lift with capture off",
            at(inner, "{ capture: false, once: true }"),
        ),
        ("the press without capture", at(outer, "{ once: true }")),
        (
            "the lift heard on the document",
            STAMP_LIFT.replacen(
                "window.addEventListener(\"pointerup\"",
                "document.addEventListener(\"pointerup\"",
                1,
            ),
        ),
    ] {
        assert_ne!(phaseless, STAMP_LIFT, "{what}");
        let bitten = body.replacen(STAMP_LIFT, &phaseless, 1);
        assert_eq!(
            b33_clock(&bitten),
            vec!["a stalled lift is not stamped"],
            "{what}"
        );
    }
    let calls = "  await stampLift(SHORT);\n";
    assert_eq!(
        body.matches(calls).count(),
        2,
        "B-33 stamps both stalled lifts"
    );
    // Without the two calls: the definition alone stands, and the lifts go unstamped.
    let uncalled = body.replace(calls, "");
    assert_eq!(
        b33_clock(&uncalled),
        vec![
            "the stalled lift is not stamped before the stall",
            "the queued lift is not stamped before the stall",
        ]
    );
    // The stamp installed behind the stall it is meant for: too late, and red.
    let behind = body.replacen(calls, "", 1).replacen(
        "  const stalled = await settled(late, \"stalled\");\n",
        "  await stampLift(SHORT);\n  const stalled = await settled(late, \"stalled\");\n",
        1,
    );
    assert_ne!(
        behind,
        body.replacen(calls, "", 1),
        "the stall is followed by its settle"
    );
    assert_eq!(
        b33_clock(&behind),
        vec!["the stalled lift is not stamped before the stall"]
    );
    // The guard without the stall's own clause: a stall under the threshold proves nothing.
    let unguarded = body.replacen(" && STALL > WANT", "", 1);
    assert_ne!(unguarded, body, "the guard names the stall");
    assert_eq!(b33_clock(&unguarded), vec!["no guard on the press lengths"]);
}

/// FN3 (GH #1205, the form of OR-T12-26): the finger's tap carries its own length.
/// The lift of every tap is stamped with its down's stamp by listeners capturing on the
/// window, ahead of every listener of the page -- WebKit stamps each event of a tap
/// when it dispatches it, and a page stalled between the two (TV-F7, build02 at 80 %
/// CPU) read the tap as a hold.
const FINGER_LIFT: &str = "    const down = (d) => {
      lift = (up) => {
        Object.defineProperty(up, \"timeStamp\", { value: d.timeStamp });
        w.__fingerLifts = (w.__fingerLifts || 0) + 1;
      };
      w.addEventListener(\"pointerup\", lift, { capture: true, once: true });
    };
    w.addEventListener(\"pointerdown\", down, { capture: true, once: true });";

/// B-36's wait ends on a HOLD as well as on a tap (N-R4, FN4): the hook's `holds`, counted
/// at the lift of a press past its threshold. `touches` alone is a hold the channel was
/// sent, and a refused channel sends none, so a wait for taps and touches alone ran a
/// real hold into the HANG marker before its verdict came.
const B36_HOLD_WAIT: &str = "      return (m.taps || 0) > w.taps || (m.touches || 0) > w.touches
        || (m.holds || 0) > w.holds;
    }, was, {";

/// And a hold is named the moment it is counted, before anything of the dock is read.
const B36_HOLD_VERDICT: &str = "    if (counted.touches > 0 || counted.holds > 0) {
      value.legs.push({ from, counted });
      return bad({ ...value, why: \"a press of B-36 was held, not tapped\" });";

/// What the finger's tap and B-36 need so that B-36 is a statement about the dock and
/// not about the host's load: the lift stamped before the tap is sent, and B-36 asking
/// the hook that each of its presses was counted as one tap (the positive signal) --
/// and naming a hold at once instead of waiting for a tap that never comes.
fn finger_tap_clock(tap: &str, b36: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    match (tap.find(FINGER_LIFT), tap.find("page.tap(selector")) {
        (Some(lift), Some(sent)) if lift < sent => {}
        (Some(_), Some(_)) => found.push("the lift is stamped after the tap was sent"),
        _ => found.push("the finger's lift is not stamped with its down's stamp"),
    }
    for (needed, why) in [
        (
            "value.legs.push({ from, counted, ",
            "B-36 does not record how its presses were counted",
        ),
        (
            "if (leg.counted.taps !== 1 || leg.counted.touches !== 0) {\n      return bad(",
            "B-36 does not ask that each press was one tap",
        ),
        (
            B36_HOLD_WAIT,
            "B-36 waits for a tap only: a held press runs into HANG",
        ),
        (B36_HOLD_VERDICT, "B-36 does not name a held press"),
    ] {
        if !b36.contains(needed) {
            found.push(why);
        }
    }
    found
}

fn finger_tap_spans() -> Option<(String, String)> {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return None;
    }
    let src = read(DRIVER_B);
    Some((
        driver_span(
            &src,
            "async function fingerTap(selector) {",
            "const ctx = {",
        ),
        driver_span(&src, "async function B36(ctx) {", "const CHECKS = {"),
    ))
}

/// FN3 (GH #1205): the finger's tap is classified by the length it was sent with, and
/// B-36 judges a dock only behind a press the hook counted as one tap.
#[test]
fn the_finger_tap_carries_its_length_and_b36_asks_for_the_tap() {
    let Some((tap, b36)) = finger_tap_spans() else {
        return;
    };
    let found = finger_tap_clock(&tap, &b36);
    assert!(found.is_empty(), "{found:?}");
    assert_eq!(
        tap.matches(FINGER_LIFT).count(),
        1,
        "one lift stamp per tap"
    );
}

/// The lock bites on the driver itself: each way the stamp or the positive signal can
/// go is exactly one finding.
#[test]
fn the_finger_tap_lock_bites() {
    let Some((tap, b36)) = finger_tap_spans() else {
        return;
    };
    let one = |t: &str, b: &str| finger_tap_clock(t, b);
    // The lift stamped a hold's length after its down (the tap made a hold).
    let late = tap.replacen("{ value: d.timeStamp }", "{ value: d.timeStamp + 350 }", 1);
    assert_ne!(late, tap);
    assert_eq!(
        one(&late, &b36),
        vec!["the finger's lift is not stamped with its down's stamp"]
    );
    // The lift heard after the page's own listeners (no capture).
    let phaseless = tap.replacen(
        "w.addEventListener(\"pointerup\", lift, { capture: true, once: true });",
        "w.addEventListener(\"pointerup\", lift, { once: true });",
        1,
    );
    assert_ne!(phaseless, tap);
    assert_eq!(
        one(&phaseless, &b36),
        vec!["the finger's lift is not stamped with its down's stamp"]
    );
    // No stamp at all: the tap as it was before FN3.
    let unstamped = tap.replacen(FINGER_LIFT, "", 1);
    assert_eq!(
        one(&unstamped, &b36),
        vec!["the finger's lift is not stamped with its down's stamp"]
    );
    // The stamp armed only after the tap is on its way.
    let behind = format!("{unstamped}\n{FINGER_LIFT}");
    assert!(behind.find(FINGER_LIFT) > behind.find("page.tap(selector"));
    assert_eq!(
        one(&behind, &b36),
        vec!["the lift is stamped after the tap was sent"]
    );
    // B-36 without its positive half: the check gone, or the count not recorded.
    let unchecked = b36.replacen(
        "if (leg.counted.taps !== 1 || leg.counted.touches !== 0) {\n      return bad(",
        "if (false) {\n      return bad(",
        1,
    );
    assert_ne!(unchecked, b36);
    assert_eq!(
        one(&tap, &unchecked),
        vec!["B-36 does not ask that each press was one tap"]
    );
    let holds_too = b36.replacen(" || leg.counted.touches !== 0", "", 1);
    assert_ne!(holds_too, b36);
    assert_eq!(
        one(&tap, &holds_too),
        vec!["B-36 does not ask that each press was one tap"]
    );
    let unrecorded = b36.replacen(
        "value.legs.push({ from, counted, ",
        "value.legs.push({ from, ",
        1,
    );
    assert_ne!(unrecorded, b36);
    assert_eq!(
        one(&tap, &unrecorded),
        vec!["B-36 does not record how its presses were counted"]
    );
    // N-R4: the wait without the hold, and the hold left unnamed.
    for (cut, with, why) in [
        (
            "\n        || (m.holds || 0) > w.holds",
            "",
            "B-36 waits for a tap only: a held press runs into HANG",
        ),
        (
            " || (m.touches || 0) > w.touches",
            "",
            "B-36 waits for a tap only: a held press runs into HANG",
        ),
        (
            "if (counted.touches > 0 || counted.holds > 0) {",
            "if (false) {",
            "B-36 does not name a held press",
        ),
        (
            " || counted.holds > 0",
            "",
            "B-36 does not name a held press",
        ),
    ] {
        let cut_b36 = b36.replacen(cut, with, 1);
        assert_ne!(cut_b36, b36, "{cut}");
        assert_eq!(one(&tap, &cut_b36), vec![why], "{cut}");
    }
}

/// The hook's half of N-R4: a pointer press that lifts past its threshold is counted as
/// a hold (`holds`) at its lift, like `taps` -- on the refused, the unlinked and the
/// device-refused threshold as much as on a take.
const HOOK_HOLD_UNSPOKEN: &str =
    "if (was && byPointer && (!audio || held < HOLD_MS)) tapDock();\\n\"";
const HOOK_HOLD_UNSPOKEN_ELSE: &str = "else if (was && byPointer) st.holds++;\\n\"";
const HOOK_HOLD_TAKE: &str =
    "if (was && byPointer) st.holds++;\\n\"\n    \"        holding = false;";

fn hook_hold_clock(client: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    match (
        client.find(HOOK_HOLD_UNSPOKEN),
        client.find(HOOK_HOLD_UNSPOKEN_ELSE),
    ) {
        (Some(tap), Some(hold)) if tap < hold && !client[tap..hold].contains("return;") => {}
        _ => found.push("a press lifted past its threshold without a take is not counted"),
    }
    if !client.contains(HOOK_HOLD_TAKE) {
        found.push("a take's lift is not counted as a hold");
    }
    if !client.contains("holdsUnlinked: 0, holds: 0,") {
        found.push("the hook does not start `holds` at zero");
    }
    found
}

#[test]
fn the_hook_counts_a_hold_at_its_lift() {
    if !library_ships() {
        eprintln!("SKIP: the template library does not ship in this tree");
        return;
    }
    let client = block("OS_CLIENT_JS");
    let found = hook_hold_clock(&client);
    assert!(found.is_empty(), "{found:?}");
    // The bite: each half gone is exactly its finding.
    for (cut, with, why) in [
        (
            HOOK_HOLD_UNSPOKEN_ELSE,
            "\\n\"",
            "a press lifted past its threshold without a take is not counted",
        ),
        (
            HOOK_HOLD_TAKE,
            "\\n\"\n    \"        holding = false;",
            "a take's lift is not counted as a hold",
        ),
        (
            "holdsUnlinked: 0, holds: 0,",
            "holdsUnlinked: 0,",
            "the hook does not start `holds` at zero",
        ),
    ] {
        let cut_client = client.replacen(cut, with, 1);
        assert_ne!(cut_client, client, "{cut}");
        assert_eq!(hook_hold_clock(&cut_client), vec![why], "{cut}");
    }
}

const SWITCH_STATE: &str = "state: trail ? () => switchState(page, pred, trail) : null";
const TRAIL_THEN_LAND: &str =
    "const trail = switchTrail(page);\n    const res = await page.goto(base + path, ";
const TRAIL_THEN_LEAVE: &str = "const trail = switchTrail(page);\n    await page.goto(here, ";

/// What a HANG of B-15's switch waits needs to say where the switch stood: every wait
/// carries a trail armed before its `goto` (the switch replaces itself while `goto`
/// still waits), and `standsStill` hands it to the HANG line.
fn switch_trail_clock(still: &str, b15: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    if !still.contains(SWITCH_STATE) {
        found.push("the switch's HANG line does not read its trail");
    }
    if b15.matches(TRAIL_THEN_LAND).count() != 1 || b15.matches(TRAIL_THEN_LEAVE).count() != 1 {
        found.push("a switch trail is not armed before its goto");
    }
    let waits = b15.matches("await standsStill(page, ").count();
    let traced = b15.matches("await standsStill(page, path, trail);").count()
        + b15
            .matches("await standsStill(page, \"/\", trail);")
            .count();
    if waits != 2 || traced != waits {
        found.push("a switch wait of B-15 runs without its trail");
    }
    found
}

fn switch_trail_spans() -> Option<(String, String)> {
    if !repo(DRIVER_B).is_file() {
        eprintln!("SKIP: {DRIVER_B} is not in this tree");
        return None;
    }
    let src = read(DRIVER_B);
    Some((
        driver_span(&src, "async function standsStill(", "/** § 6.5:"),
        driver_span(
            &src,
            "async function B15(ctx) {",
            "async function B16(ctx) {",
        ),
    ))
}

/// FN5 (NM4 f7w, B-15 `HANG: the page at / left the switch` under 80 %): the switch's
/// way out -- request, response, commit and `domcontentloaded` of each main-frame
/// navigation, and the document the page stands in -- is in the HANG line. A reading,
/// never a verdict.
#[test]
fn b15_names_where_the_switch_stood_when_it_hangs() {
    let Some((still, b15)) = switch_trail_spans() else {
        return;
    };
    let found = switch_trail_clock(&still, &b15);
    assert!(found.is_empty(), "{found:?}");
}

/// The lock bites on the driver itself: each way the trail can go is one finding.
#[test]
fn the_switch_trail_lock_bites() {
    let Some((still, b15)) = switch_trail_spans() else {
        return;
    };
    let unread = still.replacen(SWITCH_STATE, "state: null", 1);
    assert_ne!(unread, still);
    assert_eq!(
        switch_trail_clock(&unread, &b15),
        vec!["the switch's HANG line does not read its trail"]
    );
    // Armed after the goto: the switch may have left before anyone listened.
    let late = b15.replacen(
        TRAIL_THEN_LAND,
        "const res = await page.goto(base + path, ",
        1,
    );
    assert_ne!(late, b15);
    assert_eq!(
        switch_trail_clock(&still, &late),
        vec!["a switch trail is not armed before its goto"]
    );
    for (from, to) in [
        (
            "await standsStill(page, path, trail);",
            "await standsStill(page, path);",
        ),
        (
            "await standsStill(page, \"/\", trail);",
            "await standsStill(page, \"/\");",
        ),
    ] {
        let bare = b15.replacen(from, to, 1);
        assert_ne!(bare, b15);
        assert_eq!(
            switch_trail_clock(&still, &bare),
            vec!["a switch wait of B-15 runs without its trail"]
        );
    }
}
