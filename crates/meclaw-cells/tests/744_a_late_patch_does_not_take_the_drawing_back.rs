//! GH #744 — a patch from a pass that started before the tap does not take the
//! optimistic drawing back (display-hive.md § 5.7, Decision 18.09.).
//!
//! § 5.7 promises that the pass never disagrees with what the client drew. It
//! keeps that promise about the CONTENT and said nothing about the ORDER, and
//! the order is what a person feels. Every event costs a full read pass, so a
//! finger that taps while a pass is still in the air is answered first by THAT
//! pass — which renders the state from before the tap and writes it over the
//! drawing (`crates/meclaw-cells/src/web/cell.rs`: what a bundle carries is
//! written over the DOM).
//!
//! Measured on the candidate colony, the owner's own session
//! (`plans/welle-h3-2026-09-18/messungen/B-klickserie.md` § 3, 16:38:16–16:38:20):
//! five taps on the chat tile in four seconds, every one of them processed by the
//! curator, and the person saw a window that flashed open and closed again each
//! time. It looked exactly like a tile that does nothing, and only a reload
//! ended it.
//!
//! What is locked here: the client keeps its drawing for a tapped window until a
//! patch arrives whose state was computed after the tap, and it reads that off
//! the curator's own stamp `data-acted` — the last moment a touch or a put-away
//! moved this window (`max(since, dismissed_at)`), which is the browser-visible
//! version of what § 5.2 and § 4.9 write. No clock of the browser's own enters
//! the comparison: the stamp is the SERVER's, read out of the DOM at the moment
//! the finger lands, so a skewed device clock cannot release the hold early.
//!
//! GH #765 (way A) did not retire this, and the difference is worth one paragraph.
//! Way A stops a pass whose state write the store REFUSED from drawing at all — the
//! flicker of a state that never was. What is left is a pass that LANDED: measured on the
//! twin on 19.09., in a six-run series without this hold, one of the twenty-two taps that
//! reached the screen was answered first by the pass of the clock's own STROKE — the
//! order the screen placed itself, coming due, not the `in_tick` lane the first draft of
//! this paragraph named. It had started 66 ms before the finger and wrote its row with
//! `rows_affected 1`, so its drawing was the truth of that instant — and it rendered the
//! state before the tap for 90 ms until the tap's own patch arrived. No ordering of the
//! writes reaches that case.
//! The hold does, and that is why it stays.
//!
//! The source half of this claim is `708_the_client_draws_the_tap_before_the_pass.rs`;
//! it is needles in a string and a one-line mutant keeps every one of them
//! standing. This half runs the hook in a real engine and delivers two late
//! frames.
//!
//! Skips when the templates, the driver, node or a browser are missing (R2b) —
//! this tree installs nothing for a test.

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const DRIVER: &str = "workshop/tools/display-scene-browser.mjs";

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// The two hooks' sources (the scene's, the mark's), as the browser gets them. Asking
/// `compose.py` for them is the only way to be sure the bytes under test are the bytes
/// that ship.
fn scene_js(to: &std::path::Path, os: &std::path::Path) -> Option<()> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             open(sys.argv[2], 'w').write(m.SCENE_CLIENT_JS)\n\
             open(sys.argv[3], 'w').write(m.OS_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .arg(to)
        .arg(os)
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(())
}

/// One `key=value` of the driver's line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|p| p.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

/// The driver's `SCENE` line, or `None` where the tree, node or a browser is missing
/// (R2b; under `MECLAW_REQUIRE_BROWSERS=1` a skip is red).
fn scene_line() -> Option<String> {
    if !library_ships() || !repo(DRIVER).is_file() {
        return None;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let js = td.path().join("scene.js");
    let os = td.path().join("os.js");
    scene_js(&js, &os)?;
    // The shipped sheet: the driver takes its rules for the mark's line, so whether a
    // lost take's word is SEEN is read off the real sheet (re-check tv-bn M10k M2).
    let out = std::process::Command::new("node")
        .arg(repo(DRIVER))
        .arg(&js)
        .arg(&os)
        .arg(repo("templates/display/compose/display-dna.css"))
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        // GH #1161 (OR-TB.S3.39): the gate's station booked this engine and runs with
        // `MECLAW_REQUIRE_BROWSERS=1`; there a skipped driver is a red test, never a pass.
        assert!(
            std::env::var("MECLAW_REQUIRE_BROWSERS").as_deref() != Ok("1"),
            "a skip under MECLAW_REQUIRE_BROWSERS=1 is red:\n{stderr}"
        );
        println!("{stderr}");
        return None;
    }
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("SCENE "))
        .unwrap_or_else(|| panic!("the driver printed no SCENE line:\n{stdout}\n{stderr}"));
    Some(line.to_string())
}

/// The tapped canvas window and the modal it co-closes, in a real engine, with
/// two late frames delivered by hand.
///
/// The driver prints `tap=<before>/<drawn>/<held>/<reopened>/<released>`, each
/// reading the two `data-level` values in that order (timer, chat):
///
/// ```text
/// built              tap=32/00/00/02/12 restored=2
/// before the fix     tap=32/00/12/12/12 restored=0
/// before the co-close guard  tap=32/00/00/00/12 restored=2
/// ```
///
/// `held` and `reopened` are the two cases. `before` and `drawn` say only that the
/// page is the one this measures and that § 5.7's first half still works;
/// `released` says the hold ends, which is what keeps a drawing from outliving the
/// pass that answers it.
#[test]
fn a_patch_of_an_older_pass_leaves_the_drawing_standing() {
    let Some(line) = scene_line() else {
        return;
    };
    let line = line.as_str();
    let tap = field(line, "tap=");
    let steps: Vec<&str> = tap.split('/').collect();
    assert_eq!(steps.len(), 5, "tap= is five readings: {line}");
    assert_eq!(
        steps[0], "32",
        "the page this measures starts with the canvas window on the urgent level \
         (the driver delivers one before this block) and the modal over it: {line}"
    );
    assert_eq!(
        steps[1], "00",
        "the client draws the tap itself -- the window is put away and the modal \
         co-closed in the same frame (§ 5.2, § 5.3): {line}"
    );
    assert_eq!(
        steps[2], "00",
        "a patch whose state was computed BEFORE the tap took the drawing back. \
         § 5.7 (Decision 18.09.): the client keeps its drawing until a patch \
         carries a state computed after the tap -- this is the flash the owner \
         read as `the tile does nothing` (B-klickserie § 3, e25 16:38:16): {line}"
    );
    assert_eq!(
        steps[3], "02",
        "a patch that moves the CO-CLOSED window's own stamp shut it again. § 5.7 \
         runs in both directions, and the state is one for all outputs (§ 3.1): a \
         chat opened by a hold (§ 5.4 -- it never passes through the tile handler) \
         or a tap on the phone reaches this browser only as a patch, and a list \
         drawn seconds ago may not close it. The tapped window is still held here, \
         because ITS stamp has not moved: {line}"
    );
    assert_eq!(
        steps[4], "12",
        "and a patch stamped AFTER the tap stands again, whatever it says -- a \
         hold that never ends is a client that stopped listening: {line}"
    );
    assert_eq!(
        field(line, "restored="),
        "2",
        "two patches were drawn over: a `held` that is right because nothing \
         patched at all proves nothing: {line}"
    );
}

/// welle-testing tv-bn M10b: every tap leaves the screen (display-hive.md § 2: "a tap
/// ... reaches the curator as the event `tap`"; § 5.8: ten taps on one tile are ten
/// steps). The vendored LiveView's own click path drops a `phx-click` on an element
/// whose last push is unanswered (`data-phx-ref-src`, "noop if we are in the middle of
/// awaiting an ack for this el already"); measured on build03 under `--load 12`, the
/// colony heard five of B-07's ten taps (M10). The hook sends each tap itself, so ten
/// taps on a tile with an open ack are ten pushes and ten drawings:
/// `ack=<sent>/<drawn>`.
#[test]
fn a_tap_behind_an_open_ack_is_sent_and_drawn() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "ack="),
        "10/10",
        "ten taps on a tile whose last push waits for its answer: every one is sent \
         and drawn (§ 2, § 5.8) -- a tap the screen keeps to itself is a tap the \
         curator never counts: {line}"
    );
}

/// welle-testing tv-bn M10b, re-check M10 M1.1c: a press that never became a tap (a
/// swipe in the dock on a phone: `pointerdown`, then `pointercancel`) is neither sent
/// nor drawn. Drawn on `pointerdown`, it was a window that moved for a tap LiveView
/// never sent. `cancel=<sent>/<drawn>/<level>`: the level is the one before, or `moved`.
#[test]
fn a_cancelled_press_is_neither_sent_nor_drawn() {
    let Some(line) = scene_line() else {
        return;
    };
    let cancel = field(&line, "cancel=");
    assert!(
        cancel.starts_with("0/0/") && !cancel.ends_with("/moved"),
        "a press the browser cancelled moved a window or reached the socket: {line}"
    );
}

/// welle-testing tv-bn M10b, K7 12.10., derived from display-hive.md § 3.2 ("no state in
/// the browser carries meaning"): a tap while the link is down is not sent later and not
/// drawn -- a tap that lands seconds later at the reconnect is an intent the person no
/// longer has -- and the screen says it has no link. `offline=<sent>/<drawn>/<mark>/
/// <mark after the link is back>`.
#[test]
fn an_unlinked_tap_is_neither_sent_nor_drawn_and_the_screen_says_so() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "offline="),
        "0/0/true/none",
        "a tap without a link was sent, drawn, or left the screen silent -- or the \
         screen still says `no link` once the link is back (K7, § 3.2): {line}"
    );
}

/// OR-T10-10 (re-check tv-bn M10d A-M1): K7 holds for EVERY input, not only the tile.
/// Enter in the input line while the link is down sends nothing and leaves the sentence
/// standing; the link coming back sends nothing either (no resend nobody asked for); the
/// person's own Enter sends it and the field empties.
/// `line=<sent>/<field>/<sent after the link is back>/<field>/<sent after Enter>/<field>`.
#[test]
fn an_unlinked_line_keeps_its_sentence_and_goes_out_only_on_the_persons_enter() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "line="),
        "0/kept/0/kept/1/empty",
        "Enter without a link sent the line, emptied it (the sentence is lost), or the \
         link coming back sent it by itself (K7, OR-T10-10): {line}"
    );
}

/// The same for the app's buttons and option chips: a click without a link reaches
/// nobody, a click once the link is back does. `buttons=<sent>/<sent after the link is back>`.
#[test]
fn an_unlinked_button_reaches_nobody() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "buttons="),
        "0/1",
        "a button or chip without a link reached LiveView, or one with the link back did \
         not (K7, OR-T10-10): {line}"
    );
}

/// And the hold on the mark: held past the threshold without a link it pushes no `hold`,
/// and the mark says the link is gone -- its phase `offline` and the word `offline`,
/// out of the same table as `listening…`. The link back gives the mark what it said
/// before (this page has no socket, so `error`).
/// `hold=<holds pushed>/<phase>/<said>/<phase after the link is back>`.
#[test]
fn an_unlinked_hold_reaches_nobody_and_the_mark_says_so() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "hold="),
        "0/offline/offline/error",
        "a hold without a link was pushed, said `listening…`, or the mark kept quiet about \
         the link (K7, OR-T10-10, § 5.4): {line}"
    );
}

/// K7 on a streamed page (OR-T10-10 with K8 (a); re-check tv-bn M10e A-M1): its inputs
/// ride a Phoenix channel, and Phoenix keeps what is pushed without a link and sends it
/// on the rejoin by itself. The driver runs the vendored client on a socket whose link
/// it drops and brings back, and counts every frame of each kind between the drop and a
/// marker of the person's own sent after the rejoin -- what Phoenix flushed lands in
/// front of it. `page=<controls>/<pointer>/<wheel>/<touch>/<key>/<text>/<marker>`; one
/// lock per kind of input, and each lock has its control of the same kind WITH the link
/// (`pointer.wheel.touch.key.text`, each `1`): a kind the page does not wire, or a page
/// without its key field, makes a `0` there and is red, never a silent `0` behind it
/// (re-check tv-bn M10g B-M1).
#[test]
fn an_unlinked_page_takes_no_input_of_any_kind() {
    let Some(line) = scene_line() else {
        return;
    };
    let page: Vec<&str> = field(&line, "page=").split('/').collect();
    assert_eq!(page.len(), 7, "page=<7 counts>: {line}");
    assert_eq!(
        page[0], "1.1.1.1.1",
        "the controls: one pointer, wheel, touch, key and text input with the link each reach \
         the page once -- a 0 is a kind the probe never stimulated: {line}"
    );
    for (i, kind) in ["pointer", "wheel", "touch", "key", "text"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            page[i + 1],
            "0",
            "a {kind} input without a link went out, then or after the rejoin (K8 a): {line}"
        );
    }
    assert_eq!(
        page[6], "1",
        "the person's own input after the rejoin goes out: {line}"
    );
}

/// OR-T10-24 on a streamed page: LiveView stays linked and only the page's channel
/// fails. That is the same state as no link (K7, K8 (a)): the page says `down`, and
/// nothing is sent -- not then, not while Phoenix joins again, not flushed after the join
/// -- with LiveView's word still `up`, so `canPush` is the only gate the pin passes
/// (re-check tv-bn M10g A-M1, B-M2). `pchan=<link>/<pointer>/<wheel>/<touch>/<key>/<text>/
/// <link back>/<marker>`.
#[test]
fn a_page_whose_channel_is_not_joined_takes_no_input_and_says_so() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "pchan="),
        "down/0/0/0/0/0/up/1",
        "a page whose channel failed kept its colour, sent an input then or after the join, \
         or never came back (OR-T10-24, K8 a): {line}"
    );
}

/// OR-T10-24 on the mark: LiveView stays linked and only the voice channel fails while a
/// take runs. The take ends here and says `not sent` (WORDS), its audio, release and a
/// cancel do not go out; a press while Phoenix joins the channel again is refused on the
/// screen (`offline`) and sends no hold, then or after the join -- `canPush` on its own
/// (re-check tv-bn M10g A-M1, B-M2). A tap while it joins leaves the mark `offline` too:
/// the press puts back what is true about the link, not LiveView's word alone (re-check
/// tv-bn M10h M4). `chan=<audio with the channel>/<phase>/<said>/<release>/<cancel>/
/// <audio>/<hold>/<phase after a tap while joining>/<phase on a press while joining>/
/// <said>/<the next hold>`.
#[test]
fn a_take_whose_channel_is_not_joined_ends_here_and_nothing_goes_out() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "chan="),
        "1/error/not_sent_\u{2014}_say_it_again/0/0/0/0/offline/offline/offline/1",
        "a take across a failed channel went on, sent something, or a press or a tap while \
         the channel joins again did not say `offline` (OR-T10-24, K8 a): {line}"
    );
}

/// `canPush` false WITHOUT the channel's error before it (re-check tv-bn M10h M1): the
/// socket's CLOSING window -- `isConnected()` false, `onclose` not yet fired -- and the
/// cell's own `close` of the call. No `lose()` ran before, so the refusal at each call
/// site is the only thing between the take and Phoenix's buffers (`sendBuffer`,
/// `pushBuffer`), which flush on the reconnect: (audio) a worklet frame of a running take
/// (`sendAudio`), (release) the release at the end of the drain (`frame`), (drain) a press
/// inside the drain window, whose `down()` ends the drain with a refused release (re-check
/// tv-bn M10i M1), (close) the call closed under a take (`onClose`). Each ends the take
/// with `not sent` and nothing of it goes out, then or after the reconnect. `push=` per
/// case `<phase>/<said>/<release>/<audio>` up to the next hold; the drain case reads
/// `<phase>/<said>/<drains ended>` at the press and `<phase>/<said>` again at the press's
/// threshold -- the press that lost the take does not take its `not sent` back.
#[test]
fn a_take_refused_at_each_push_without_a_channel_error_ends_with_not_sent() {
    let Some(line) = scene_line() else {
        return;
    };
    let unsent = "not_sent_\u{2014}_say_it_again";
    assert_eq!(
        field(&line, "push="),
        format!(
            "error/{unsent}/0/0/error/{unsent}/0/0/error/{unsent}/1/error/{unsent}/0/0/\
             error/{unsent}/0/0"
        ),
        "a push refused without the channel's error let the take run on silently, or sent \
         its audio or release later (K7, K8 a on OR-T10-10/OR-T10-24): {line}"
    );
}

/// OR-T10-15 and K8 on it: a take running when the link drops ends on the screen. Its
/// audio, its release and a cancel are not kept for later -- not by the hook and not by
/// Phoenix's buffers -- and the mark says the take was NOT sent, out of WORDS, until the
/// person holds again. `take=<audio with the link>/<phase>/<said>/<release>/<cancel>/
/// <audio without the link>/<said with the link back>/<the next hold>`.
#[test]
fn a_take_the_link_drops_ends_here_and_says_it_was_not_sent() {
    let Some(line) = scene_line() else {
        return;
    };
    assert_eq!(
        field(&line, "take="),
        "1/error/not_sent_\u{2014}_say_it_again/0/0/0/not_sent_\u{2014}_say_it_again/1",
        "a take across a lost link sent something later, or the mark kept quiet that it was \
         lost (OR-T10-15, K8 a): {line}"
    );
}

/// K7 on what writes the mark's word OUTSIDE a press (re-check tv-bn M10j M1): after a
/// take was lost, the cell's `error` frame, the rejoin's `timeout` and a refused rejoin --
/// and behind the last two the next join's `ok` and the cell's `hello`, the first frame
/// after every join (`gh836_hello_waits_until_the_handler_took_the_session`) -- leave
/// `not sent` standing. Only a press spends it. So do the cell's `turn`, `speak_start`
/// and `speak_end` -- the answer to an earlier take arriving while this one is lost --
/// which write the phase alone: the phase stays `error` and the line is seen behind
/// each (re-check tv-bn M10m M1). `kept=<phase>/<said>` at the `error` frame,
/// `<phase>/<said>/<seen>` behind each of `turn`, `speak_start`, `speak_end`,
/// `<phase>/<said>` at the timeout, `<said>` behind the next `hello`,
/// `<phase>/<said>` at the refusal, `<said>` behind the next `hello`.
#[test]
fn a_lost_take_keeps_its_word_through_every_writer_but_a_press() {
    let Some(line) = scene_line() else {
        return;
    };
    let unsent = "not_sent_\u{2014}_say_it_again";
    assert_eq!(
        field(&line, "kept="),
        format!(
            "error/{unsent}/error/{unsent}/seen/error/{unsent}/seen/error/{unsent}/seen/\
             error/{unsent}/{unsent}/error/{unsent}/{unsent}"
        ),
        "a frame, a timeout, a refusal or a rejoin's hello took the `not sent` of a lost take \
         back without a press, or a phase-only frame (`turn`, `speak_start`, `speak_end`) took \
         its line out of sight (K7, OR-T10-15/-24): {line}"
    );
}

/// The mark's line has ONE writer, and it is the one that keeps a lost take's word
/// (re-check tv-bn M10j M1): `say` is the only assignment to the line and to `st.said`
/// besides `repaint`, which puts `st.said` back after a morph. A second writer would be
/// one that forgets `not sent`.
#[test]
fn the_marks_line_has_one_writer_and_it_keeps_not_sent() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py ships");
    let os = src
        .split("OS_CLIENT_JS = (")
        .nth(1)
        .expect("the mark's script")
        .split("\n)\n")
        .next()
        .unwrap();
    assert!(
        os.contains("function say(t) { if (lost) t = WORDS.unsent; st.said = t; if (state) state.textContent = t; }"),
        "`say` writes the line and keeps a lost take's word: {os}"
    );
    assert_eq!(
        os.matches("textContent = ").count(),
        2,
        "the line is written by `say` and put back by `repaint`, nowhere else"
    );
    assert!(
        os.contains("s.textContent = st.said;"),
        "`repaint` puts back what `say` wrote"
    );
    assert_eq!(
        os.matches("st.said = ").count(),
        1,
        "`st.said` is written by `say` alone"
    );
    assert!(
        os.contains("lost = false;\\n\"\n    \"        holding = true;"),
        "the take a press begins spends the word of the one it lost, or `listening` would \
         read `not sent` while it runs (re-check tv-bn M10j H2)"
    );
    for other in [
        "innerText",
        "nodeValue",
        "innerHTML",
        "outerHTML",
        "insertAdjacent",
        "replaceChildren",
        "replaceWith",
        "append(",
        "prepend(",
        "appendChild",
        "insertBefore",
        "createTextNode",
        ".data =",
        "textContent\\\"",
    ] {
        assert!(!os.contains(other), "no other way to the line: {other}");
    }
    let tight: String = os.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(
        tight.matches("textContent=").count() - tight.matches("textContent==").count()
            + tight.matches("textContent+=").count(),
        2,
        "`say` and `repaint` write the line, in any spelling nowhere else"
    );
    assert_eq!(
        tight.matches("st.said=").count() - tight.matches("st.said==").count(),
        1,
        "`st.said` is written by `say` alone, in any spelling"
    );
}

/// The mark's phase has ONE writer in the client, and it is the one that keeps a lost
/// take's word SEEN (re-check tv-bn M10m M1): the sheet draws the line only in `error`,
/// so a write of `data-phase` past `phase()` -- a frame handler that sets `speaking`
/// itself -- would keep `not sent` in the line and take it out of sight. The attribute
/// has three writers (re-check tv-bn M10n M1): `phase()`, the only write of `st.phase`;
/// the server's render, `data-phase=""` in `OS_TEMPLATE`, which LiveView's morph writes
/// on every pass and every join's render; and `repaint`, which `updated` calls in the
/// SAME task as that morph -- synchronously, unconditionally -- to put `phase()`'s value
/// back. A timer or a condition in `updated` leaves the server's `""` standing for a
/// paint or for good. The frames that write only the phase go through `phase()`; the
/// press the morph also takes back (`aria-pressed="false"`) is put back by `repaint` too.
#[test]
fn the_marks_phase_has_one_writer_and_it_keeps_a_lost_word_seen() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py ships");
    let os = src
        .split("OS_CLIENT_JS = (")
        .nth(1)
        .expect("the mark's script")
        .split("\n)\n")
        .next()
        .unwrap();
    assert!(
        os.contains(
            "function phase(p) { if (lost) p = \\\"error\\\"; st.phase = p; el.setAttribute(\\\"data-phase\\\", p); }"
        ),
        "`phase` writes the phase and keeps a lost take's `error`: {os}"
    );
    assert!(
        os.contains(
            "if (el.getAttribute(\\\"data-phase\\\") !== st.phase) el.setAttribute(\\\"data-phase\\\", st.phase);"
        ),
        "`repaint` puts back what `phase` wrote"
    );
    assert!(
        os.contains(
            "if (b && b.getAttribute(\\\"aria-pressed\\\") !== pr) b.setAttribute(\\\"aria-pressed\\\", pr);"
        ) && os.contains("pr = holding ? \\\"true\\\" : \\\"false\\\";"),
        "`repaint` puts back the press of a running take that a pass's morph took"
    );
    // The third writer: the server's render, morphed in on every pass and every join.
    assert!(
        src.contains("data-phase=\"\" data-unseen=\"{{unseen}}\""),
        "the server renders the mark's phase empty -- the morph's value"
    );
    // ... and healed in the same task, by `updated`, word for word.
    assert!(
        os.contains(
            "    updated: function () {\\n\"\n    \"      if (this.__mic) this.__mic.repaint();\\n\"\n    \"    },\\n\""
        ),
        "`updated` repaints the mark synchronously and always -- a timer or a condition \
         leaves the morph's `\"\"` standing: {os}"
    );
    let tight: String = os.chars().filter(|c| !c.is_whitespace()).collect();
    assert_eq!(
        tight.matches("setAttribute(\\\"data-phase\\\"").count(),
        2,
        "`phase` and `repaint` write the phase, nowhere else"
    );
    assert_eq!(
        tight.matches("setAttribute(").count(),
        tight.matches("setAttribute(\\\"").count(),
        "every attribute write names its attribute -- none can hide `data-phase`"
    );
    assert_eq!(
        tight.matches("st.phase=").count() - tight.matches("st.phase==").count(),
        1,
        "`st.phase` is written by `phase` alone"
    );
    assert_eq!(
        os.matches("dataset").count(),
        os.matches("el.dataset.mount ||").count(),
        "`dataset` is read for the mount, never written"
    );
    for other in [
        "removeAttribute",
        "toggleAttribute",
        "setAttributeNode",
        "setAttributeNS",
        ".attributes",
        "'data-phase'",
        "setAttribute.",
        "Reflect",
        "\\\"data-\\\" +",
    ] {
        assert!(!os.contains(other), "no other way to the phase: {other}");
    }
    for frame in [
        "else if (f.type === \\\"turn\\\") { st.turns++; st.text = f.text; phase(\\\"\\\"); }",
        "else if (f.type === \\\"speak_start\\\") { playAt = 0; phase(\\\"speaking\\\"); }",
        "else if (f.type === \\\"speak_end\\\") { st.speakEnd++; phase(\\\"\\\"); }",
    ] {
        assert!(
            os.contains(frame),
            "a frame that writes only the phase goes through `phase`: {frame}"
        );
    }
}

/// The same at run time, on the line ON THE PAGE (re-check tv-bn M10k M1/M2): across
/// `take=`, `chan=`, `push=`, `kept=` and `spent=` every change of the live line comes
/// from `say` or from `repaint` -- the line's `textContent` setter is watched, and a
/// change no such call accounts for (`innerHTML`, `insertAdjacentHTML`, a text node's
/// `data`, a write from anywhere else) is a violation. And after a take was lost, until
/// the word is spent and outside a running take, the line reads `not sent` AND is seen:
/// box and clip computed from the shipped sheet's two rules for the line -- a seeing
/// person gets the word, not only a screen reader (K7); at no other time does it read
/// `not sent`. The word is spent by a press or by the take a press begins
/// (`aria-pressed` "true"), and the state is judged at every change of the line and on
/// both sides of every press, release and cancel -- BEFORE a press spends the word, so
/// a case whose last callback releases and presses the next one in the same task is
/// judged where it ended (re-check tv-bn M10l M1). Eight presses of the chain find a
/// lost take's word standing: the one after `take=`'s rejoin, the one while the channel
/// joins again (`chan=`), the markers behind `push=` audio, release, drain and close,
/// the `timeout` take behind `error`, the `spent` take behind `timeout`. `writes=<changes>/<violations>/<rules of the sheet>/
/// <first violation>/<presses that found the word>/<changes of the phase>`. The phase is
/// watched the same way (re-check tv-bn M10m M1): every change of `data-phase` comes from
/// `phase()` or `repaint` at the element's `setAttribute`, or from LiveView's morph inside
/// a patch, and then only the server's `""` (re-check tv-bn M10n M1); one from anywhere
/// else, or one no such call accounts for (`dataset`, `removeAttribute`), is a violation,
/// and so is a patch that leaves the phase, the line or the press other than it found
/// them (`morph=`).
#[test]
fn a_lost_takes_word_is_written_by_say_alone_and_seen() {
    let Some(line) = scene_line() else {
        return;
    };
    let writes = field(&line, "writes=");
    let parts: Vec<&str> = writes.split('/').collect();
    assert_eq!(
        parts.len(),
        6,
        "writes=<changes>/<violations>/<rules>/<first>/<presses that found the word>/\
         <changes of the phase>: {line}"
    );
    assert!(
        parts[5].parse::<u32>().unwrap_or(0) > 0,
        "the watch saw no change of the phase at all: {line}"
    );
    assert!(
        parts[0].parse::<u32>().unwrap_or(0) > 0,
        "the watch saw no change of the line at all: {line}"
    );
    assert_eq!(
        parts[2], "2",
        "the shipped sheet's two rules for the line were not on the page: {line}"
    );
    assert_eq!(
        &parts[1..5],
        &["0", "2", "none", "8"],
        "the line changed past `say`, the phase past `phase()`/`repaint`, a lost take's \
         `not sent` was overwritten or not seen before the press that spends it, `not sent` \
         stood without a lost take, or a seam of the chain was not judged (K7, \
         OR-T10-15/-24): {line}"
    );
}

/// The third writer of the mark, exercised (re-check tv-bn M10n M1): the server renders
/// the mark with `data-phase=""`, an empty line and `aria-pressed="false"`, and LiveView
/// morphs the hook's element back to that on every pass and on every join's render -- on
/// a rejoin BEFORE `reconnected` -- and calls `updated` in the same task. The driver
/// stands that patch in wherever the chain has one: the rejoins of `take=`, of `push=`'s
/// audio, release and drain cases (the voice channel rides on LiveView's own socket, so
/// its drop is LiveView's too), of `kept=`'s `error` frame, before `timeout` and before
/// the refusal; the pass of the cell's `turn`; a pass while the `spent` take is held.
/// Behind each, once the task is over -- never between morph and `updated`, no paint
/// lies there -- what the hook drew stands: a lost take's word in `error` and seen, the
/// held take `listening` (and pressed, `writes=`). `morph=<phase>/<said>/<seen>` behind
/// each patch, then `<patches>/<healed>`: a patch whose morph took something and whose
/// `updated` put all of it back is healed.
#[test]
fn a_patch_puts_back_what_the_mark_drew_in_the_same_task() {
    let Some(line) = scene_line() else {
        return;
    };
    let lost = "error/not_sent_\u{2014}_say_it_again/seen";
    assert_eq!(
        field(&line, "morph="),
        format!(
            "{lost}/{lost}/{lost}/{lost}/{lost}/{lost}/{lost}/listening/listening\u{2026}/unseen/{lost}/9/9"
        ),
        "a patch (the server's `data-phase=\"\"`, empty line, `aria-pressed=\"false\"`, then \
         `updated`) left the mark other than it found it at the end of its task, or a patch \
         of the chain was not delivered (K7, GH #720): {line}"
    );
}

/// The other direction (re-check tv-bn M10j H2, M10k M1), without a clock: a press
/// inside the drain window loses the take before it -- the voice channel cannot push
/// for that moment -- and the channel can push again before the press's threshold.
/// The press's own take reads `listening…`, and the idle word once it ends; a `begin`
/// that kept the lost word would read `not sent` over a take that IS sent.
/// `spent=<phase>/<said>/<lost> at the press, <phase>/<said> at its hold, <phase>/<said>
/// after its release`.
#[test]
fn the_take_a_press_begins_spends_the_word_of_the_one_it_lost() {
    let Some(line) = scene_line() else {
        return;
    };
    let unsent = "not_sent_\u{2014}_say_it_again";
    assert_eq!(
        field(&line, "spent="),
        format!("error/{unsent}/1/listening/listening\u{2026}/sending/stt/tts"),
        "a take begun after a lost one read `not sent`, or the lost one was not lost or \
         not said (K7): {line}"
    );
}

/// K8 on OR-T10-10 (b): the mark's word for a lost link stands in the same table as
/// `listening…`, in the same language -- no second source for what the mark says.
#[test]
fn the_marks_words_have_one_source() {
    if !library_ships() {
        return;
    }
    let src = std::fs::read_to_string(repo(COMPOSE)).expect("compose.py ships");
    let os = src
        .split("OS_CLIENT_JS = (")
        .nth(1)
        .expect("the mark's script")
        .split("\n)\n")
        .next()
        .unwrap();
    let table = os
        .split("var WORDS = {")
        .nth(1)
        .expect("the mark's words in one table")
        .split('}')
        .next()
        .unwrap();
    assert!(
        table.contains("listening: \\\"listening\\\\u2026\\\"")
            && table.contains("offline: \\\"offline\\\"")
            && table.contains("unsent: \\\"not sent \\\\u2014 say it again\\\""),
        "`listening…`, `offline` and `not sent` side by side, in English: {table}"
    );
    assert!(
        os.contains("say(WORDS.listening)")
            && os.contains("say(WORDS.offline)")
            && !os.contains("say(\\\"listening")
            && os.contains("t = WORDS.unsent;")
            && !os.contains("say(\\\"offline")
            && !os.contains("say(\\\"not sent")
            && !os.contains("say(\\\"closed"),
        "the mark says its words out of the table and nowhere as a literal"
    );
}

/// The mark of K7 is the sheet's, and it stays inside § 6.11 ("no part of a tile has an
/// opacity below 0.88"): everything that takes a finger or a key -- tiles, app buttons,
/// option chips, the input line, the mark -- loses its colour, not its legibility, while
/// the screen has no link (OR-T10-10).
#[test]
fn the_sheet_says_the_screen_has_no_link() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo("templates/display/compose/display-dna.css"))
        .expect("the sheet ships");
    let (head, rest) = sheet
        .split_once(" { filter: grayscale(1)")
        .and_then(|(h, r)| {
            h.rsplit_once("*/")
                .map(|(_, h)| (h.to_string(), r.to_string()))
        })
        .expect("a rule for the inputs of a screen without a link");
    let rule = rest.split('}').next().unwrap();
    for sel in [
        "[data-offline=\"true\"] [phx-click]",
        "[data-offline=\"true\"] [phx-keyup]",
        "[data-offline=\"true\"] .display-os-mark",
        "[data-offline=\"true\"] .display-browser-view",
    ] {
        assert!(
            head.contains(sel),
            "every input says `no link`, not only the tile: {sel} missing in {head}"
        );
    }
    assert!(
        !rule.contains("opacity"),
        "the inputs say `no link` by colour, never by an opacity under § 6.11's floor: {rule}"
    );
    assert!(
        sheet.contains(".display-os[data-phase=\"offline\"] .display-os-mark"),
        "the mark wears its own `offline` phase"
    );
}

/// One source for the mark's server markup (re-check tv-bn M10o M1, OR-T12-6): a
/// stand-in that wrote the mark by hand drifted from `OS_TEMPLATE` without a sound, and
/// the morph that patches the hook's element back to it then exercised the copy, not the
/// server. Every browser stand-in that puts the mark on its page takes it from
/// `workshop/tools/lib/os-template.mjs`, which reads `OS_TEMPLATE` out of `compose.py`
/// and fails loud on a placeholder it does not fill. No `.mjs` under `workshop/tools`
/// (scanned, so a fourth driver is read too -- re-check tv-bn M10p M2) carries the
/// mark's element as a literal, and the three known stand-ins import and call the
/// library. A tree that ships the tools but not the library is red, never skipped: a
/// revert that drops the library brings the hand copies back (re-check tv-bn M10p M1).
#[test]
fn every_stand_in_takes_the_mark_from_the_server_template() {
    const LIB: &str = "workshop/tools/lib/os-template.mjs";
    // The export switch hangs on the tools, not on the claim: no tools, nothing to read.
    if !library_ships() || !repo("workshop/tools").is_dir() {
        return;
    }
    assert!(
        repo(LIB).is_file(),
        "{LIB} is gone while workshop/tools ships -- the stand-ins have no one source \
         for the mark (a revert of the library brings the hand copies back)"
    );
    let lib = std::fs::read_to_string(repo(LIB)).unwrap_or_else(|e| panic!("{LIB}: {e}"));
    assert!(
        lib.contains("sys.stdout.write(m.OS_TEMPLATE)")
            && lib.contains("templates/display/compose/compose.py"),
        "the stand-ins' mark is not read out of compose.py's `OS_TEMPLATE`: {lib}"
    );
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
                .path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n != "node_modules") {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|x| x == "mjs") {
                out.push(path);
            }
        }
    }
    let mut sources = Vec::new();
    walk(&repo("workshop/tools"), &mut sources);
    sources.sort();
    let lib_path = repo(LIB).canonicalize().unwrap();
    let mut takers = Vec::new();
    for path in &sources {
        if path.canonicalize().unwrap() == lib_path {
            continue;
        }
        // Read as source, never run.
        let src =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for copy in ["class=\"display-os\"", "class=\"display-os-mark\""] {
            assert!(
                !src.contains(copy),
                "{} writes the mark by hand ({copy}) -- a copy drifts from `OS_TEMPLATE` \
                 without a sound",
                path.display()
            );
        }
        if src.contains("osServerMarkup(") {
            assert!(
                src.contains("import { osServerMarkup } from \"")
                    && src.contains("lib/os-template.mjs\";"),
                "{} calls `osServerMarkup` without importing it from the library",
                path.display()
            );
            takers.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    for stand_in in [
        "display-scene-browser.mjs",
        "display-os-browser.mjs",
        "display-browser-lab.mjs",
    ] {
        assert!(
            takers.iter().any(|t| t == stand_in),
            "{stand_in} does not take the mark from the server template (takers: {takers:?})"
        );
    }
}
