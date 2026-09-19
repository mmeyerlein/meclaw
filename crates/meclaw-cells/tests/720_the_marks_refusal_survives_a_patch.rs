//! GH #720 -- what the mark says about a refusal outlives the next patch.
//!
//! Measured on the fresh instance after #719 (18.09.2026, Chromium, the monitor
//! profile served over plain HTTP on a LAN address, so the browser refuses the
//! microphone): a 900 ms press dims the mark and sends the hold
//! (`data-phase="error"`, opacity 0.5, `holdsRefused=1` at t=460 ms) -- and
//! 100 ms later `data-phase` is empty again and the mark is bright
//! (t=562 ms). The pass that the same hold started rendered `#display-os`
//! again, and the server's value for that attribute is, and always was, the
//! empty string. Nobody reads a refusal in a tenth of a second, so § 5.4 ("the
//! mark says the refusal in its live region and dims") held for nobody, and the
//! proof B-05 skipped for the same reason.
//!
//! The fix is not a field in the screen's state. § 3.2 allows exactly one
//! browser state with meaning -- whether the dock is open -- and the refusal is
//! not asking to become the second one. What the mark is doing (listening,
//! sending, speaking, refused) has never been the screen's state: it is client
//! state of the same kind as the listening glow (§ 6.8), true about THIS page's
//! voice channel and about no other output of the same member, and a microphone
//! this browser could not open is nothing the curator has an opinion about. So
//! the hook keeps its own phase and its own live-region text and writes both
//! back in `updated()` after the morph has taken them.
//!
//! What ends the refusal is a gesture or a device, never a clock: the next
//! press starts the mark's story over, and a microphone that opens after all
//! clears it on the spot. A timeout would be a quantity outside the sheet
//! (§ 2), and a refused CHANNEL (the socket's word) still outlives both, which
//! is why `clearMark` restores that one instead of blanking the mark.
//!
//! This is a file-text lock. The behaviour is measured in the browser by B-05,
//! which since this find reads the mark AFTER a pass, not only before one.
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

/// The two things a patch wipes are written into the hook's own state as well.
#[test]
fn the_hook_keeps_the_phase_and_the_line_it_wrote() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        !os.contains("function phase(p) { el.setAttribute(\\\"data-phase\\\", p); }"),
        "`phase()` still writes only the attribute -- that is the whole find of \
         GH #720: the next morph puts the server's empty value back"
    );
    assert!(
        os.contains("function phase(p) { st.phase = p; el.setAttribute(\\\"data-phase\\\", p); }"),
        "the mark's phase is not kept in the hook's own state, so nothing can \
         write it back after a patch"
    );
    assert!(
        os.contains("function say(t) { st.said = t;"),
        "§ 5.4: the mark SAYS the refusal in its live region -- a line only the \
         DOM remembers is gone with the same patch as the phase"
    );
    assert!(
        os.contains("phase: \\\"\\\", said: \\\"\\\""),
        "the two fields start empty on `window.__displayMic`, where a proof \
         driving a real browser reads them (B-05)"
    );
}

/// And writes them back once the morph has been through.
#[test]
fn updated_puts_the_mark_back_after_the_morph() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    let start = os
        .find("    updated: function () {")
        .expect("the OS hook has no `updated()` -- then no patch is ever answered (GH #720)");
    let body = &os[start..start + 200.min(os.len() - start)];
    assert!(
        body.contains("this.__mic"),
        "`updated()` reaches for no handle of its own: {body}"
    );
    assert!(
        body.contains("repaint()"),
        "`updated()` does not repaint the mark: {body}"
    );

    let rp = os
        .find("function repaint() {")
        .expect("no `repaint` -- there is no one place the mark is put back");
    let paint = &os[rp..rp + 600.min(os.len() - rp)];
    assert!(
        paint.contains("st.phase") && paint.contains("setAttribute(\\\"data-phase\\\""),
        "the repaint does not restore `data-phase`, which is what the sheet \
         dims on: {paint}"
    );
    assert!(
        paint.contains("st.said") && paint.contains("textContent"),
        "the repaint does not restore the live region (§ 5.4): {paint}"
    );
    assert!(
        paint.contains("!== st.phase") && paint.contains("!== st.said"),
        "the repaint writes unconditionally: re-writing the same text into an \
         `aria-live` region announces it a second time: {paint}"
    );
    assert!(
        paint.contains("querySelector('[data-role=\\\"state\\\"]')"),
        "the live region is remembered instead of re-queried -- morphdom may \
         have replaced the span the mount held: {paint}"
    );
    assert!(
        os.contains("this.__mic = { st: st, repaint: repaint };"),
        "the handle is not taken in `mounted()`, so `updated()` finds nothing"
    );
    assert!(
        os.contains("this.__mic = null;"),
        "`destroyed()` leaves the handle behind: a re-mounted hook would \
         repaint a dead element"
    );
}

/// § 3.2: the refusal stays in the browser and says nothing to the colony.
#[test]
fn the_refusal_is_never_pushed_to_the_screens_state() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    let rp = os.find("function repaint() {").expect("no `repaint`");
    let paint = &os[rp..rp + 600.min(os.len() - rp)];
    assert!(
        !paint.contains("pushEvent"),
        "§ 3.2: the only browser state with meaning is whether the dock is \
         open. A refused microphone is client state like the listening glow \
         (§ 6.8) -- pushing it would ask the curator for an opinion about a \
         device it cannot see: {paint}"
    );
    let os_py = read(COMPOSE);
    assert!(
        os_py.contains("data-phase=\"\" data-unseen=\"{{unseen}}\""),
        "the server still renders the mark with an empty phase, and that is the \
         point: if the colony ever rendered a phase, § 3.2 would have a second \
         exception"
    );
}

/// A gesture or a device ends the refusal. Never a clock.
#[test]
fn a_new_press_and_a_granted_microphone_clear_the_mark() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        os.contains(
            "function clearMark() { phase(refused ? \\\"error\\\" : \\\"\\\"); \
             say(refused ? refusal : idleText()); }"
        ),
        "no `clearMark` that puts the mark back to what is still TRUE about \
         this screen -- a refused channel is the socket's word and outlives the \
         gesture, a refused device does not"
    );
    assert!(
        os.matches("clearMark();").count() >= 2,
        "`clearMark` is defined and never called: the dim would stand until the \
         page is reloaded"
    );
    let down = os.find("async function down(e) {").expect("`down` is gone");
    let end = os[down..]
        .find("// \u{a7} 6.4: on an output without audio")
        .expect("`down` no longer reads the profile");
    let head = &os[down..down + end];
    assert!(
        head.contains("clearMark();"),
        "a new press does not start the mark's story over: {head}"
    );
    // GH #704: everything a tap needs is recorded before anything about the
    // channel is read, and `clearMark` reads it.
    let kind = head
        .find("byPointer = !!(e && e.pointerId")
        .expect("`down` no longer records what kind of press this is");
    assert!(
        head.find("clearMark();").unwrap() > kind,
        "the mark is repainted before the press is recorded (GH #704): {head}"
    );
    let mic = os
        .find("stream = await navigator.mediaDevices.getUserMedia")
        .expect("`openMic` no longer asks for the device");
    let granted = &os[mic..mic + 900.min(os.len() - mic)];
    assert!(
        granted.contains("clearMark();"),
        "a microphone that opened after all leaves the mark dimmed for ever -- \
         nothing else ever takes the refusal off: {granted}"
    );
    assert!(
        !os.contains("setTimeout(clearMark"),
        "a refusal that fades on a clock is a quantity outside the sheet (§ 2)"
    );
}

/// B-05 reads the mark after a pass, not only before one.
#[test]
fn the_browser_proof_measures_across_a_pass() {
    if !library_ships() {
        return;
    }
    let driver = read("workshop/tools/display-layout-browser.mjs");
    let start = driver
        .find("async function B05(ctx) {")
        .expect("B-05 is gone");
    let end = driver[start..]
        .find("\n/**")
        .unwrap_or(driver.len() - start);
    let b05 = &driver[start..start + end];
    assert!(
        b05.contains("after_patch"),
        "B-05 still reads the mark once and calls that § 5.4: the refusal it \
         measured was gone 100 ms later (GH #720): {b05}"
    );
    assert!(
        b05.contains("holdsRefused"),
        "B-05 cannot tell a refused DEVICE from a refused CHANNEL, and since \
         OR-H0.15 the first one does push a hold: {b05}"
    );
    assert!(
        b05.contains("isSecureContext"),
        "B-05 never produces the refusal it is about: on a secure origin with a \
         microphone there is nothing to measure, and on an insecure one a hold \
         IS the refusal: {b05}"
    );
}
