//! Welle F -- the ring behind the threshold (E-3, OR-F25).
//!
//! The 250 ms threshold is only affordable because nothing is thrown away
//! while it runs. From the first touch on the mark the worklet's frames go
//! into a ring of 2 s (100 frames of 20 ms), and the moment the hold goes out
//! they follow it, in order, ahead of the live frames -- the cell queues them
//! behind the `hold` that frames them. Without this, the first quarter second
//! of every take is lost, which is where the word that matters is said. The
//! report it answers is E-3: on a device, pressing the mark and speaking at
//! once lost the speech, and the speaker had to wait a few seconds first.
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

fn script() -> String {
    std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"")
}

#[test]
fn the_ring_is_two_seconds_and_drops_from_the_front() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        src.contains("var PRE_MAX = 100;"),
        "100 frames of 20 ms is 2 s -- longer than any setup measured on the device"
    );
    assert!(
        src.contains("if (pre.length > PRE_MAX) pre.shift();"),
        "the oldest frame falls out rather than the ring growing"
    );
}

#[test]
fn the_worklet_port_has_three_branches_now() {
    if !library_ships() {
        return;
    }
    let src = script();
    let port = src
        .split("worklet.port.onmessage = function (e) {")
        .nth(1)
        .expect("the port handler")
        .split("src.connect(worklet)")
        .next()
        .unwrap();
    assert!(port.contains("if (holding)"), "holding: {port}");
    assert!(port.contains("else if (draining)"), "draining: {port}");
    assert!(
        port.contains("else if (armed)"),
        "and armed -- kept, not sent: {port}"
    );
}

#[test]
fn what_was_kept_follows_the_hold_and_the_ring_is_then_empty() {
    if !library_ships() {
        return;
    }
    let src = script();
    let begin = src
        .split("function begin() {")
        .nth(1)
        .expect("the hook has a `begin`")
        .split("function up(e)")
        .next()
        .unwrap();
    let hold = begin.find("frame({ type: \"hold\" })").expect("the hold");
    let flush = begin
        .find("while (pre.length) sendAudio(pre.shift());")
        .expect("the flush");
    assert!(
        hold < flush,
        "the kept frames go AFTER the hold that frames them: {begin}"
    );
    assert!(
        begin.contains("st.prebuffered = pre.length;") && begin.contains("disarm();"),
        "and the ring is measured and then emptied: {begin}"
    );
}

#[test]
fn the_ring_starts_at_the_touch_and_a_tap_takes_it_with_it() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        src.contains("el.addEventListener(\"pointerdown\", warm, true)"),
        "the capture phase on the whole mark: the device is asked for while the finger is going down"
    );
    let up = src
        .split("function up(e) {")
        .nth(1)
        .expect("the hook has an `up`")
        .split("holding = false;")
        .next()
        .unwrap();
    assert!(
        up.contains("disarm();"),
        "a touch that never became a hold takes its ring with it: {up}"
    );
    // And a hold that DID happen disarms too. `begin()` disarms on the way in,
    // the tap branch above disarms on the way out -- if the hold branch did
    // not, the ring would keep filling between takes, `down()` would skip its
    // `arm()` because `armed` is still true, and the next hold would carry up
    // to 2 s of room tone from before the last release in front of it, plus a
    // `setup_ms` that never happened.
    let held = src
        .split("function up(e) {")
        .nth(1)
        .expect("the hook has an `up`")
        .split("holding = false;")
        .nth(1)
        .expect("the hold branch")
        .split("function typing(e)")
        .next()
        .unwrap();
    assert!(
        held.contains("disarm();"),
        "the hold branch gives the ring back as well: {held}"
    );
}

#[test]
fn the_ring_is_armed_once_and_the_microphone_is_opened_once() {
    if !library_ships() {
        return;
    }
    let src = script();
    // A press with nothing in front of it -- the space key, or a driver that
    // calls the handle -- has had no touch to arm the ring, so it arms here.
    let down = src
        .split("async function down(e) {")
        .nth(1)
        .expect("the hook has a `down`")
        .split("function begin()")
        .next()
        .unwrap();
    assert!(
        down.contains("if (!armed) arm();"),
        "a press with no touch in front of it starts the ring itself: {down}"
    );
    // One microphone at a time: a touch and the press that follows it are two
    // callers, and two `getUserMedia` in flight build two graphs of which one
    // is never heard from again.
    let mic = src
        .split("function mic() {")
        .nth(1)
        .expect("the hook has a `mic`")
        .split("function warm()")
        .next()
        .unwrap();
    assert!(
        mic.contains("if (micWait) return micWait;") && mic.contains("micWait = null;"),
        "the second caller waits on the first promise instead of opening a second graph: {mic}"
    );
    // And the capture listener is given back with the same `true` it was
    // registered with, or a re-mount leaves a ring arming behind it.
    let teardown = src
        .split("this.__displayMicTeardown = function () {")
        .nth(1)
        .expect("the teardown")
        .split("destroyed:")
        .next()
        .unwrap();
    assert!(
        teardown.contains("el.removeEventListener(\"pointerdown\", warm, true);"),
        "the arming listener is given back in the capture phase it lives in: {teardown}"
    );
}

/// welle-tv tv-bn-layout (VORGABEN TV-B1): the probes that warm the mark (B-04, B-29,
/// the mic driver's touch before the press) wait until the ring HOLDS frames, never a
/// fixed 600/400 ms. The signal is ONE field of the hook, `ringed`: what the ring holds
/// right now -- set where a frame is kept, emptied with the ring on arm and disarm.
/// Test observability only: nothing in the hook reads it back, so the ring behaves
/// exactly as before.
#[test]
fn the_ring_counts_what_it_keeps_and_the_probes_wait_on_it() {
    if !library_ships() {
        return;
    }
    let src = script();
    // The hook's state literal (the line that also starts `holdsRefused`) carries the
    // counter at 0 -- read on that line, not as a neighbour pair: m10 (K7/K8) put
    // `holdsUnlinked`/`takesLost` between the two, and the pair-pin went red (S2).
    let state = src
        .lines()
        .find(|l| l.contains("holdsRefused: 0,"))
        .expect("the mic hook's state literal");
    assert!(
        state.contains(" ringed: 0,"),
        "the mic hook's state starts the ring counter at 0: {state}"
    );
    let kept = "else if (armed) { pre.push(e.data); if (pre.length > PRE_MAX) pre.shift(); st.ringed = pre.length; }";
    assert_eq!(
        src.matches(kept).count(),
        1,
        "the counter is what the ring holds after a frame is kept, written once, where it is kept"
    );
    assert!(
        src.contains(
            "function arm() { armed = true; armedAt = Date.now(); pre.length = 0; st.ringed = 0; }"
        ) && src.contains(
            "function disarm() { armed = false; armedAt = 0; pre.length = 0; st.ringed = 0; }"
        ),
        "the counter empties with the ring, on arm and on disarm"
    );
    // Nothing in the code reads `ringed` back: every mention outside a comment is
    // one of the writes or the initial 0. Comments may name it -- the scene hook's
    // `patches` and the door's `tapseq` cite it as the model of test observability
    // (6b193a806, b9ac3ef84) -- so the pin judges what the code does, not how often
    // the word occurs (a whole-file count went 5 -> 7 on two comments, tv-bn M8).
    let in_code: Vec<&str> = src
        .lines()
        .filter(|l| l.contains("ringed"))
        .map(|l| {
            if l.trim_start().starts_with('#') {
                ""
            } else {
                l.split("// ").next().unwrap_or(l)
            }
        })
        .filter(|code| code.contains("ringed"))
        .collect();
    let writes_and_init = in_code
        .iter()
        .map(|code| code.matches("st.ringed = ").count() + code.matches("ringed: 0").count())
        .sum::<usize>();
    let mentions = in_code
        .iter()
        .map(|code| code.matches("ringed").count())
        .sum::<usize>();
    assert!(
        writes_and_init == 4 && mentions == writes_and_init,
        "`ringed` is written (kept, arm, disarm) and initialised, never read by the code: {in_code:#?}"
    );
    assert!(
        src.contains("// `ringed`: what the ring holds right now"),
        "the mic hook says what the field is for, in its comment"
    );
    const DRIVER: &str = "workshop/tools/display-layout-browser.mjs";
    if !repo(DRIVER).is_file() {
        eprintln!("SKIP: {DRIVER} is not in this tree");
        return;
    }
    let driver = std::fs::read_to_string(repo(DRIVER)).expect("driver");
    for (name, end) in [
        ("async function B04(ctx) {", "\nasync function "),
        ("async function B29(ctx) {", "\nasync function "),
    ] {
        let start = driver
            .find(name)
            .unwrap_or_else(|| panic!("{name} is gone"));
        let rest = &driver[start + name.len()..];
        let body = &rest[..rest.find(end).unwrap_or(rest.len())];
        assert!(
            body.contains("ringFills("),
            "{name} waits on the ring holding a frame (`ringed`)"
        );
        assert!(
            !body.contains("waitForTimeout("),
            "{name} still sleeps on the driver's clock"
        );
    }
}
