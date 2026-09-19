//! GH #708 -- the mark binds only what the profile names (§ 6.4), and the hold
//! carries no field (§ 5.6, S-088).
//!
//! "`inputs` gates what is bound and shown: `tap_bound`, `hold_bound` and
//! `input_line` after step 2 … without `audio` a press on a bound mark longer
//! than the threshold is a press (§ 5.5), the client records nothing, sends
//! nothing to the channel `voice`, sends no event `hold`, and the mark has no
//! listening state" (§ 6.4). And: "That an output of type `tv` has `inputs: []`
//! is the door: a television is an output device; hold and press come from the
//! phone or the laptop" (the maintainer's ruling of 14.09.).
//!
//! Until this wave the hook asked nothing: a long press on a television opened
//! the microphone, sent audio to the channel `voice` and pushed an event at the
//! curator -- on a screen with no way to press it at all.
//!
//! The event's name and shape: "`tap {for: <oid>}` comes from the tile,
//! `hold {topic: chat}` from the mark (the model's head)" (§ 5.6) -- and S-088
//! decides what that means on the wire: the hold carries NO field, the topic is
//! the description's shorthand for the window step 3 acts on. The names `tile`
//! and `touch` are struck (§ 2).
//!
//! The browser proofs are B-13 (no hover on a tv), B-14 (600 ms on an output
//! without audio is a press) and B-19 (the listening state exists only where a
//! hold does), strand H5.
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
fn the_hook_reads_the_profile_before_it_binds_anything() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        os.contains("data-inputs"),
        "the mark's hook never asks what this output can take (§ 6.4)"
    );
    for counter in ["st.bound =", "st.audio ="] {
        assert!(
            os.contains(counter),
            "`{counter}` is missing: what the hook bound is what a browser \
             proof reads (B-13, B-14, B-19)"
        );
    }
    let gate = os.find("if (!finger)").expect(
        "no gate on `finger` -- § 6.4: without `pointer` and without `touch` nothing is bound",
    );
    let bind = os
        .find("btn.addEventListener(")
        .expect("the hook binds nothing at all any more");
    assert!(
        gate < bind,
        "the hook binds its listeners before it asks whether this output has a \
         finger -- a television would answer a press that cannot happen (§ 6.4)"
    );
}

#[test]
fn without_audio_a_long_press_is_a_press() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        os.matches("if (!audio)").count() >= 2,
        "the audio gate stands in one place at most: the ring (`warm`) and the \
         press (`down`) both have to ask, or a television records (§ 6.4)"
    );
    assert!(
        os.contains("!audio || held < HOLD_MS"),
        "a press longer than the threshold on an output without audio still \
         means the dock (§ 6.4, § 5.5): without this it means nothing at all, \
         and the mark reads as a dead control"
    );
    assert_eq!(
        os.matches("phase(\\\"listening\\\")").count(),
        1,
        "the listening state is written somewhere else as well -- § 6.8: the \
         glow exists only where a hold exists"
    );
}

#[test]
fn the_hold_is_called_hold_and_carries_no_field() {
    if !library_ships() {
        return;
    }
    let os = block("OS_CLIENT_JS");
    assert!(
        os.contains("pushEvent(\\\"hold\\\", {})"),
        "the mark pushes something other than `hold {{}}` -- § 5.6 names the \
         event, S-088 says it carries no field: a `topic` a client sends with \
         it is ignored, so sending one is a promise about the wrong thing"
    );
    assert!(
        !os.contains("pushEvent(\\\"touch\\\""),
        "the struck name `touch` is still pushed (§ 2, § 5.6: it is called \
         `hold`; `touch` is the curator's term for what the pass does with it)"
    );
}
