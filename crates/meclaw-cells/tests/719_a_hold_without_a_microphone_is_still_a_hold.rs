//! GH #719 -- a long press with a refused microphone is still a hold.
//!
//! Measured on the fresh instance (18.09.2026, Chromium and WebKit, the monitor
//! profile with `audio`, served over plain HTTP on a LAN address so the browser
//! refuses the microphone): a 900 ms press on the OS mark did NOTHING. No
//! `hold` reached the pass, the dock did not toggle, `__displayMic` stayed
//! 0/0/0/0/0, `data-phase` stayed empty, the mark kept opacity 1. Only the live
//! region said "microphone needs https or localhost".
//!
//! Two rules were broken at once. `display-hive.md` § 5.4 counts a page that is
//! not served over HTTPS among the refused voice channels and asks that "the
//! mark says the refusal in its live region and dims" -- it said it and did not
//! dim, because `openMic()` returns `false` without ever going through the
//! refusal path. And § 6.4 is no help here: the reading "a long press is a
//! press" hangs on the PROFILE, not on the device, so an `audio` output
//! correctly refuses the dock and the press therefore meant nothing at all.
//!
//! Ruling OR-H0.15: **the hold is the event, the audio is best effort.** With a
//! refused microphone on an `audio` profile the client still sends
//! `pushEvent("hold", {})` -- the chat opens, and on an output with a keyboard
//! the input line (§ 7.3) carries the conversation instead of the voice. The
//! mark says the refusal (`openMic` already did), takes `data-phase="error"` so
//! the sheet dims it through `--mark-dim`, and counts `holdsRefused`. The
//! `refused` latch is deliberately NOT set: it is the socket's word, and
//! latching a device refusal would mean a microphone granted a minute later
//! needed a reload.
//!
//! This is a file-text lock. The behaviour itself is measured in the browser by
//! B-05 (the mark dims on `data-phase="error"`), B-14 and B-33.
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

/// The refused microphone leaves the press with something to do.
#[test]
fn a_refused_microphone_no_longer_ends_the_press_in_silence() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        !os.contains("if (!worklet && !(await mic())) return;"),
        "the microphone branch of `down()` still returns in silence -- that is \
         the whole find of GH #719: no hold, no press, no dim, no counter"
    );
    assert!(
        os.contains("if (!worklet && !(await mic())) {"),
        "the microphone branch is no longer the one-line return, and it is not \
         the block that replaced it either -- rewrite this lock with it"
    );
}

/// What that branch does: the refusal, the dim and the hold.
#[test]
fn the_refusal_dims_the_mark_counts_and_still_sends_the_hold() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    let start = os
        .find("function refuseMic(")
        .expect("no `refuseMic` -- the refused-device path has no end of its own (OR-H0.15)");
    let body = &os[start..start + 900.min(os.len() - start)];

    assert!(
        body.contains("phase(\\\"error\\\")"),
        "§ 5.4: the mark DIMS on a refused voice channel, and the sheet dims \
         on `data-phase=\"error\"`: {body}"
    );
    assert!(
        body.contains("st.holdsRefused++"),
        "a refusal that counts nothing cannot be measured from the outside: \
         {body}"
    );
    assert!(
        body.contains("pushEvent(\\\"hold\\\", {})"),
        "OR-H0.15: the hold is the event and the audio is best effort -- the \
         chat opens whether or not there is a microphone: {body}"
    );
    assert!(
        !body.contains("refused = true"),
        "the `refused` latch is the socket's word. Latching a device refusal \
         makes it sticky, and a microphone granted a minute later would need a \
         page reload: {body}"
    );
}

/// The press under the threshold stays a press, and the dock stays its answer.
#[test]
fn a_short_press_without_a_microphone_is_still_the_dock() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        os.contains("if (was && byPointer && (!audio || held < HOLD_MS)) tapDock();"),
        "§ 5.5/§ 6.4: a press under the threshold means the dock, and a \
         refused microphone does not change that"
    );
    // The refusal runs off the same threshold clock as everything else on this
    // mark, or a tap on a screen with no microphone would open the chat.
    assert!(
        os.contains("setTimeout(refuseMic,"),
        "the refused-device path does not wait for the threshold: a 120 ms tap \
         would become a hold"
    );
}

/// The sheet's half of § 5.4, unchanged: dimming is opacity, through the token.
#[test]
fn the_sheet_still_dims_the_mark_through_its_own_token() {
    if !library_ships() {
        return;
    }
    let src = read(COMPOSE);
    assert!(
        src.contains(
            ".display-columns[data-inputs~=\"audio\"] .display-os[data-phase=\"error\"] \
             .display-os-mark {\n  opacity: var(--mark-dim);\n}"
        ),
        "the dim rule the client's `phase(\"error\")` reaches for is gone or \
         moved -- B-05 measures exactly this"
    );
}
