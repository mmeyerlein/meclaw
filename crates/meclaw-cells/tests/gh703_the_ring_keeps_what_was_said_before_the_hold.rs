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
        .split("function up()")
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
        .split("function up() {")
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
        .split("function up() {")
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
