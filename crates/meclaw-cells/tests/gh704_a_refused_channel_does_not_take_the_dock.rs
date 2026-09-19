//! Welle F -- a refused voice channel does not take the dock with it (R-23-4).
//!
//! The OS mark carries two gestures on one button. A short press toggles the
//! dock, and that is pure presentation: a client-side attribute on `<html>`,
//! nothing pushed, no semantics for the colony (OR-F5). A long press opens the
//! microphone, and only THAT needs a `voice` cell at the other end.
//!
//! The wave's proof run measured the two tied together: a refused join set the
//! `disabled` property on the button, and a disabled control dispatches no
//! pointer events at all -- so the dock toggle died with the voice channel.
//! On a screen with a voice cell it is invisible; after a restart of that cell
//! the mark stays dead until the page is reloaded, and on the phone the dock
//! is the only way to the tiles (`data-dock="hidden"` is the phone's profile
//! default).
//!
//! Two halves, and the second is the one that counts:
//!
//! * the source says the refusal is a flag of the hook's own and never the
//!   button's `disabled` property, and that neither way into a tap reads it;
//! * a real browser presses the mark, with input the ENGINE dispatches, after
//!   a join that was refused. A guard inside a handler proves nothing here:
//!   the question is whether the handler is called at all.
//!
//! Skips when the templates do not ship (R2b), and when the host has no
//! browser to drive (the driver says `SKIP` and leaves with 3).

use std::process::Command;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

const COMPOSE: &str = "templates/display/compose/compose.py";
const SHEET: &str = "templates/display/compose/display-dna.css";
const DRIVER: &str = "workshop/tools/display-os-browser.mjs";

/// The sheet without its comments -- a `{` inside one would split a rule in
/// the wrong place, and a word inside one would answer a question about code.
fn uncommented(sheet: &str) -> String {
    let mut out = String::with_capacity(sheet.len());
    let mut rest = sheet;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start..].find("*/") {
            Some(end) => rest = &rest[start + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Every rule of the sheet as `(selector list, declarations)`.
fn rules(sheet: &str) -> Vec<(String, String)> {
    let plain = uncommented(sheet);
    let mut out = Vec::new();
    let mut rest = plain.as_str();
    while let Some(open) = rest.find('{') {
        let (head, tail) = rest.split_at(open);
        let tail = &tail[1..];
        let Some(close) = tail.find('}') else { break };
        let (decls, after) = tail.split_at(close);
        out.push((
            head.rsplit('}').next().unwrap_or(head).trim().to_string(),
            decls.to_string(),
        ));
        rest = &after[1..];
    }
    out
}

/// The declarations of the ONE rule with exactly this selector list.
fn rule_named(sheet: &str, selector: &str) -> String {
    let wanted: String = selector.split_whitespace().collect::<Vec<_>>().join(" ");
    rules(sheet)
        .into_iter()
        .find(|(sel, _)| sel.split_whitespace().collect::<Vec<_>>().join(" ") == wanted)
        .unwrap_or_else(|| panic!("the sheet has no rule `{wanted}`"))
        .1
}

fn library_ships() -> bool {
    repo("templates/display/template.json").is_file()
}

/// `compose.py` with the escaping of the Python string undone.
fn script() -> String {
    std::fs::read_to_string(repo(COMPOSE))
        .expect("compose.py")
        .replace("\\\"", "\"")
}

/// The slice of the hook between two markers. Measured rather than guessed at
/// a closing brace: the source is a Python string whose every line carries the
/// literal `\n"`, so a real newline never stands where the JavaScript has one.
fn between<'a>(src: &'a str, head: &str, end: &str) -> &'a str {
    let after = src
        .split(head)
        .nth(1)
        .unwrap_or_else(|| panic!("the hook has no {head}"));
    let slice = after
        .split(end)
        .next()
        .unwrap_or_else(|| panic!("nothing follows {head} up to {end}"));
    assert!(
        slice.len() < after.len(),
        "{end} does not follow {head} -- the slice is the rest of the file"
    );
    slice
}

#[test]
fn the_refusal_is_a_flag_of_the_hook_and_never_the_buttons_own_property() {
    if !library_ships() {
        return;
    }
    let src = script();
    assert!(
        !src.contains("btn.disabled"),
        "a disabled button dispatches no pointer events, so no guard inside \
         the hook can give the dock toggle back (R-23-4)"
    );
    assert!(
        src.contains("refused = true"),
        "the refusal lives in a flag the hook owns"
    );
    // Nor as `aria-disabled`: the mark still answers a finger, and a control
    // that answers must not tell assistive technology that it does not. What
    // says the refusal is the live region and the phase the sheet dims by.
    assert!(
        !src.contains("aria-disabled\", \"true\""),
        "a control that still answers is not announced as unavailable"
    );
    let refuse_from = between(&src, "function refuseFrom(why) {", "\\n\"");
    assert!(
        refuse_from.contains("say(why)") && refuse_from.contains("phase(\"error\")"),
        "the refusal is said and shown, in one place: {refuse_from}"
    );
    // The same question one layer lower: no socket at all. Leaving `mounted`
    // there would take every listener below it -- the two that carry the dock
    // -- so it is answered the same way.
    assert!(
        src.contains("if (!socket) refuseFrom("),
        "a missing socket refuses the speech half instead of leaving the mount"
    );
    assert!(
        !src.contains("if (!socket) { say"),
        "and it no longer returns out of `mounted`"
    );
    // The click way into a tap, up to the switch it performs.
    let click = between(&src, "function onClick() {", "tapDock();");
    for forbidden in ["disabled", "refused"] {
        assert!(
            !click.contains(forbidden),
            "a click asks nothing about the voice channel: {forbidden} in {click}"
        );
    }
    // The pointer way, up to the line that records what kind of press this is.
    // Everything a tap needs is set BEFORE anything about the channel is read;
    // that ordering is the whole repair.
    let press = between(&src, "async function down(e) {", "byPointer = ");
    for forbidden in ["disabled", "refused"] {
        assert!(
            !press.contains(forbidden),
            "the press is recorded before the channel is asked about: \
             {forbidden} in {press}"
        );
    }
    // And the hold does ask, visibly: a press that is long enough and cannot
    // speak says so rather than dying quietly.
    let refuse = between(&src, "function refuse() {", "// The take itself");
    assert!(
        refuse.contains("phase(\"error\")") && refuse.contains("say(refusal)"),
        "a hold with no channel refuses in the open: {refuse}"
    );
    assert!(
        src.contains("holdTimer = setTimeout(refuse, HOLD_MS)"),
        "and it refuses at the same threshold a take would have begun at"
    );
}

/// § 5.4: "a refused voice channel never disables the press: the mark says the
/// refusal in its live region and dims." The hook's half is the test above;
/// this is the sheet's, and it is about the word DIMS. Until strand H5 the refused
/// phase only changed the mark's COLOUR -- the third ink instead of the second
/// -- which reads as another mark and not as a mark that cannot do its work
/// (B-05, F-2; H2 waved the finding away as rendering taste, and the sentence
/// stands in the description).
#[test]
fn the_refused_mark_dims_rather_than_only_changing_colour() {
    if !library_ships() {
        return;
    }
    let sheet = std::fs::read_to_string(repo(SHEET)).expect("the sheet ships");
    assert!(
        sheet.contains("--mark-dim: 0.5;"),
        "the dimming is a quantity under a name (§ 2 `Token`) and the same one \
         an output without `audio` already spends (§ 6.4)"
    );
    let dim = rule_named(
        &sheet,
        ".display-columns[data-inputs~=\"audio\"] .display-os[data-phase=\"error\"] \
         .display-os-mark",
    );
    assert!(
        dim.contains("opacity: var(--mark-dim)"),
        "a refused mark DIMS: opacity, not a second colour (§ 5.4, B-05 \
         measures `opacity < 1`). Found: {dim}"
    );
    // And the dimming is the only thing that changes: § 5.4 in the same breath
    // forbids taking the press away, so no rule may put the button out of a
    // finger's reach.
    for (selector, decls) in rules(&sheet) {
        if !selector.contains("data-phase=\"error\"") {
            continue;
        }
        // One thing this phase draws is not a control: the caption that says
        // the refusal (OR-H2.1, GH #722). It stands BESIDE the mark, on its
        // leading side -- B-34 measures that its box reaches neither over the
        // button nor over a tile -- and it must refuse the pointer for the very
        // reason this loop exists: the press belongs to the button and nothing
        // may take it. So it is asked for the opposite, not waved through.
        if selector.trim_end().ends_with(".display-os-state") {
            assert!(
                decls.contains("pointer-events: none"),
                "`{selector}` hangs over the mark's upper edge and takes a \
                 pointer: it would swallow the press that ends the refusal \
                 (§ 5.4, R-23-4): {decls}"
            );
            assert!(
                !decls.contains("display: none") && !decls.contains("clip-path: inset"),
                "`{selector}` is the line § 5.4 asks the mark to SAY, and it is \
                 hidden again (GH #722): {decls}"
            );
            continue;
        }
        assert!(
            !decls.contains("pointer-events: none") && !decls.contains("display: none"),
            "`{selector}` takes the press away from a mark that was only \
             refused a microphone (§ 5.4, R-23-4): {decls}"
        );
    }
    // One value, two rules: the output without `audio` reads the same token.
    let quiet = rule_named(
        &sheet,
        ".display-columns:not([data-inputs~=\"audio\"]) .display-os-mark",
    );
    assert!(
        quiet.contains("opacity: var(--mark-dim)"),
        "the older dimming spends the token rather than a number of its own \
         (§ 2 `Token`). Found: {quiet}"
    );
}

/// One `key=value` out of the driver's line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(key))
        .unwrap_or_else(|| panic!("{key} is not in {line:?}"))
}

#[test]
fn the_mark_still_opens_the_dock_after_the_channel_said_no() {
    if !library_ships() || !repo(DRIVER).is_file() {
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    // Asking `compose.py` is the only way to be sure the bytes under test are
    // the bytes that ship.
    let made = Command::new("python3")
        .arg("-c")
        .arg(
            "import importlib.util, sys\n\
             spec = importlib.util.spec_from_file_location('compose', sys.argv[1])\n\
             m = importlib.util.module_from_spec(spec)\n\
             spec.loader.exec_module(m)\n\
             open(sys.argv[2] + '/os.js', 'w').write(m.OS_CLIENT_JS)",
        )
        .arg(repo(COMPOSE))
        .arg(td.path())
        .output();
    let made = match made {
        Ok(out) => out,
        Err(_) => return,
    };
    assert!(
        made.status.success(),
        "compose.py did not answer:\n{}",
        String::from_utf8_lossy(&made.stderr)
    );
    let out = match Command::new("node")
        .arg(repo(DRIVER))
        .arg(td.path().join("os.js"))
        .output()
    {
        Ok(out) => out,
        Err(_) => return,
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if out.status.code() == Some(3) || stderr.lines().any(|l| l.starts_with("SKIP ")) {
        println!("{stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the driver failed:\n{stdout}\n{stderr}"
    );
    let line = stdout
        .lines()
        .find(|l| l.starts_with("OSMARK "))
        .unwrap_or_else(|| panic!("the driver printed no line:\n{stdout}\n{stderr}"));
    println!("{line}");
    assert_eq!(
        field(line, "disabled="),
        "false",
        "the button is never the place the refusal is kept: {line}"
    );
    assert_eq!(
        field(line, "tap="),
        "1",
        "a short press opens the dock with no voice channel anywhere (R-23-4): {line}"
    );
    assert_eq!(
        field(line, "taps="),
        "1",
        "and it switches exactly once: {line}"
    );
    assert_eq!(
        field(line, "hold="),
        "1",
        "a long press is speech and leaves the dock where the tap put it: {line}"
    );
    assert_eq!(
        field(line, "holds="),
        "0",
        "a hold is no tap, refused or not: {line}"
    );
    assert_eq!(
        field(line, "frames="),
        "0",
        "and nothing was pushed at a channel that refused: {line}"
    );
    assert_eq!(
        field(line, "touches="),
        "0",
        "a take that never began is no touch on the chat either (R-23-5): {line}"
    );
    assert_eq!(
        field(line, "phase="),
        "error",
        "the hold refuses in the open rather than dying quietly: {line}"
    );
    assert_ne!(
        field(line, "said="),
        "\"\"",
        "and it says what happened: {line}"
    );
    // `phase` and `said` are already set when the JOIN is refused, long before
    // any press -- so on their own they would stay green with the refusal at
    // the threshold deleted. This counter is the hook's own, written nowhere
    // but in `refuse()`, and it is what makes that half able to go red.
    assert_eq!(
        field(line, "refused="),
        "1",
        "the hold really reached the refusal at the threshold: {line}"
    );
    assert_eq!(
        field(line, "aria="),
        "null",
        "a mark that answers a finger is not announced as unavailable: {line}"
    );
    // The space key is the desk's way of speaking and carries no dock, but it
    // must not die quietly either.
    assert_eq!(
        field(line, "keydock="),
        field(line, "hold="),
        "the space key switches no dock: {line}"
    );
    assert_eq!(
        field(line, "keyrefused="),
        "2",
        "and a held space key refuses in the open, as a finger does: {line}"
    );
    // One layer lower: a page with no `SurfaceSocket` at all. Until this repair
    // the hook left `mounted` there and wired no listener below that line.
    assert_eq!(
        field(line, "nosocket="),
        "1",
        "the dock opens even where there is no socket to speak on: {line}"
    );
}
