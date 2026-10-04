//! GH #996: inserting at the line after the last one appends.
//!
//! Models append to a file with `file_insert` and `line` set to the number
//! of lines plus one (`line: 7` on a six-line file), mostly with `where:
//! after` or no `where` at all (`after` is the default). The measured runs
//! lost one tool call per append on an `out_of_range` whose only hint was
//! `lines: 6` and the text "out of range" -- across every model tried.
//!
//! The contract: line N+1 inserts at the end of the file, `before` and
//! `after` alike, exactly as `after` line N does (one version, byte for
//! byte). That holds for an empty file (N = 0: `after` line 0 or 1, or
//! `before` line 1) and for a file whose last line has no line end (the new
//! text starts on a line of its own). Only N+1: any other line outside the file stays
//! `out_of_range`; the answer carries `lines: N` and a message naming the
//! ways to append. The tool description says how to append, in both copies
//! of the menu. The append at N+1 is the append at N in every other respect
//! too: a `\r\n` file gets `\r\n` ends, and a moved base merges (or is
//! refused as `base_moved`) the same way. `line` sent as the string `"7"` is
//! refused as `bad_request`, as `"6"` is. Every assertion is made on the one
//! `tool_result` the model receives.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const F: &str = "fh-0a0b0c0d0e61";
/// Six lines, the last one ended.
const OPS: &str = "\"\"\"Arithmetic helpers.\"\"\"\n\n\ndef add(a, b):\n    \"\"\"Return the sum of a and b.\"\"\"\n    return a + b\n";
/// What a model sends to append: a blank line, then the function.
const MUL: &str = "\ndef mul(a, b):\n    return a * b\n";

/// `./derive` as a recorder (the pattern of gh908): a write's derive job is
/// another issue's work and calls a model and an embedder.
const DERIVE_RECORDER: &str = "import sys, json\n\
doc = json.load(sys.stdin)\n\
sys.stdout.write('[]')\n";

fn space(text: &str) -> (Space, String) {
    let mut s = Space::with(
        "/m/files",
        &[("derive", "script_inline", json!(DERIVE_RECORDER))],
    );
    let v = s.seed_text(F, "/calc/ops.py", &[text], &[]).remove(0);
    (s, v)
}

/// `<n>:<h4>|<text>` for every line of `src`, computed here.
fn formatted(src: &str) -> String {
    src.lines()
        .enumerate()
        .map(|(i, l)| format!("{}:{}|{}", i + 1, h4(l), l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One tool call on the space's `in_tool` lane from the reasoning core; the
/// parsed text of the ONE `tool_result` that leaves.
fn call(s: &mut Space, name: &str, id: &str, args: Value) -> Value {
    let before = s.out.len();
    s.lane(
        "in_tool",
        json!({"tool_caller": "cogny"}),
        json!({"tool_name": name, "tool_call_id": id}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                             "text": args.to_string()}]}),
    );
    let mine: Vec<Msg> = s.out[before..]
        .iter()
        .filter(|m| m.route() == "tool_result")
        .cloned()
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "{name}: exactly one tool_result, got {mine:?}; stderr {:?}",
        s.stderr
    );
    let msgs = mine[0].messages();
    assert_eq!(msgs.len(), 1);
    let text = msgs[0]["text"].as_str().unwrap();
    meclaw_core::serde_json::from_str(text).expect("the result is JSON")
}

/// The whole head, read back through `file_read`: (version token, text).
fn head(s: &mut Space, id: &str) -> (Value, Value) {
    let r = call(s, "file_read", id, json!({"file": F}));
    assert_eq!(r["ok"], json!(true), "{r}");
    (r["version"].clone(), r["text"].clone())
}

fn insert(
    s: &mut Space,
    id: &str,
    base: &str,
    line: i64,
    place: Option<&str>,
    text: &str,
) -> Value {
    let mut args = json!({"file": F, "base": &base[..12], "line": line, "text": text});
    if let Some(w) = place {
        args["where"] = json!(w);
    }
    call(s, "file_insert", id, args)
}

#[test]
fn gh996_before_the_line_after_the_last_appends() {
    if !shipped() {
        return;
    }
    let want = format!("{OPS}{MUL}");
    // The call of the measured runs, `before` line N+1 on N = 6.
    let (mut s, v) = space(OPS);
    let got = insert(&mut s, "c-1", &v, 7, Some("before"), MUL);
    assert_eq!(got["ok"], json!(true), "before line 7 of 6 appends: {got}");
    let (ver, text) = head(&mut s, "c-2");
    assert_eq!(ver, got["version"], "the answer names the new head");
    assert_eq!(text, json!(formatted(&want)), "appended at the end");

    // `after` line N is the same append: the same version, byte for byte.
    let (mut t, v) = space(OPS);
    let after = insert(&mut t, "c-3", &v, 6, Some("after"), MUL);
    assert_eq!(after["ok"], json!(true), "{after}");
    assert_eq!(after["version"], got["version"], "one append, one version");
}

#[test]
fn gh996_after_the_line_after_the_last_appends() {
    if !shipped() {
        return;
    }
    let want = format!("{OPS}{MUL}");
    let (mut a, v) = space(OPS);
    let end = insert(&mut a, "c-1", &v, 6, Some("after"), MUL);
    assert_eq!(end["ok"], json!(true), "after line 6 of 6: {end}");
    // The calls of the measured runs: `after` line N+1, with and without
    // `where` (`after` is the default).
    for (id, place) in [("c-2", Some("after")), ("c-3", None)] {
        let (mut s, v) = space(OPS);
        let got = insert(&mut s, id, &v, 7, place, MUL);
        assert_eq!(
            got["ok"],
            json!(true),
            "line 7 {place:?} of 6 appends: {got}"
        );
        assert_eq!(got["version"], end["version"], "one append, one version");
        let (ver, text) = head(&mut s, "c-4");
        assert_eq!(ver, got["version"], "the answer names the new head");
        assert_eq!(text, json!(formatted(&want)), "appended at the end");
    }
}

#[test]
fn gh996_a_last_line_without_its_end_is_not_glued_to() {
    if !shipped() {
        return;
    }
    // The same six lines, the last one without its line end.
    let open = OPS.strip_suffix('\n').unwrap();
    let want = format!("{open}\n{MUL}");
    let (mut a, v) = space(open);
    let end = insert(&mut a, "c-1", &v, 6, Some("after"), MUL);
    assert_eq!(end["ok"], json!(true), "after line 6 of 6: {end}");
    for (id, place) in [("c-2", "after"), ("c-3", "before")] {
        let (mut s, v) = space(open);
        let got = insert(&mut s, id, &v, 7, Some(place), MUL);
        assert_eq!(got["ok"], json!(true), "{place} line 7 of 6 appends: {got}");
        assert_eq!(got["version"], end["version"], "one append, one version");
        let (_, text) = head(&mut s, "c-4");
        assert_eq!(
            text,
            json!(formatted(&want)),
            "{place} 7: the old last line keeps its own line"
        );
    }
}

#[test]
fn gh996_an_empty_file_appends_at_line_zero_or_one() {
    if !shipped() {
        return;
    }
    // Valid Python: the space checks the syntax of a `.py` head.
    let first = "import os\n";
    let (mut s, v) = space("");
    let got = insert(&mut s, "c-1", &v, 1, Some("before"), first);
    assert_eq!(got["ok"], json!(true), "before line 1 of 0: {got}");
    let (ver, text) = head(&mut s, "c-2");
    assert_eq!(ver, got["version"]);
    assert_eq!(text, json!(formatted(first)));

    let (mut t, v) = space("");
    let after = insert(&mut t, "c-3", &v, 0, Some("after"), first);
    assert_eq!(after["ok"], json!(true), "after line 0 of 0: {after}");
    assert_eq!(
        after["version"], got["version"],
        "start and end are one place"
    );

    // `after` line N+1 = 1 is the same append.
    let (mut u, v) = space("");
    let one = insert(&mut u, "c-4", &v, 1, Some("after"), first);
    assert_eq!(one["ok"], json!(true), "after line 1 of 0: {one}");
    assert_eq!(one["version"], got["version"], "one append, one version");
}

#[test]
fn gh996_another_line_outside_names_the_count_and_how_to_append() {
    if !shipped() {
        return;
    }
    let (mut s, v) = space(OPS);
    // Only N+1 appends: N+2 and beyond, and line 0 `before`, stay refused.
    let cases: [(&str, i64, Option<&str>); 5] = [
        ("c-1", 8, Some("after")),
        ("c-2", 8, None),
        ("c-3", 8, Some("before")),
        ("c-4", 0, Some("before")),
        ("c-5", 9, Some("after")),
    ];
    for (id, line, place) in cases {
        let got = insert(&mut s, id, &v, line, place, MUL);
        let at = format!("line {line} {place:?}");
        assert_eq!(got["ok"], json!(false), "{at}: {got}");
        assert_eq!(got["error"]["code"], json!("out_of_range"), "{at}: {got}");
        assert_eq!(
            got["error"]["lines"],
            json!(6),
            "{at}: the answer counts the lines: {got}"
        );
        let m = got["error"]["message"].as_str().unwrap_or_default();
        assert!(
            m.contains("append") && m.contains("after line 6 or 7") && m.contains("before line 7"),
            "{at}: the message says how to append: {got}"
        );
    }
    let (ver, text) = head(&mut s, "c-6");
    assert_eq!(ver, json!(&v[..12]), "a refused insert moves no head");
    assert_eq!(text, json!(formatted(OPS)));

    // On an empty file the hint names line 0 and line 1; `after` 2 is N+2.
    let (mut e, v) = space("");
    let got = insert(&mut e, "c-7", &v, 2, Some("after"), MUL);
    assert_eq!(got["error"]["code"], json!("out_of_range"), "{got}");
    assert_eq!(got["error"]["lines"], json!(0), "{got}");
    let m = got["error"]["message"].as_str().unwrap_or_default();
    assert!(
        m.contains("after line 0 or 1") && m.contains("before line 1"),
        "{got}"
    );
}

#[test]
fn gh996_a_crlf_file_appends_with_crlf() {
    if !shipped() {
        return;
    }
    // The same six lines with `\r\n` ends; the model sends `\n` as always.
    let crlf = OPS.replace('\n', "\r\n");
    let want = format!("{crlf}{}", MUL.replace('\n', "\r\n"));
    let token = &sha256_hex(want.as_bytes())[..12];
    for (id, line, place) in [
        ("c-1", 6, Some("after")),
        ("c-2", 7, Some("after")),
        ("c-3", 7, None),
        ("c-4", 7, Some("before")),
    ] {
        let (mut s, v) = space(&crlf);
        let got = insert(&mut s, id, &v, line, place, MUL);
        assert_eq!(got["ok"], json!(true), "line {line} {place:?}: {got}");
        // The version is the sha256 of the bytes: every line end is `\r\n`,
        // the appended ones included -- no `\n` mixed in.
        assert_eq!(
            got["version"],
            json!(token),
            "line {line} {place:?} appends with the file's line end: {got}"
        );
    }
}

#[test]
fn gh996_a_moved_base_merges_after_n_plus_one_as_after_n() {
    if !shipped() {
        return;
    }
    // The head moved since the model read: line 1 changed, apart from the end.
    let apart = OPS.replacen("helpers.", "helpers, all of them.", 1);
    // The head moved at the end itself: another function was appended.
    let at_end = format!("{OPS}\ndef sub(a, b):\n    return a - b\n");
    let merged = format!("{apart}{MUL}");
    for (id, line, place) in [
        ("c-1", 6, Some("after")),
        ("c-2", 7, Some("after")),
        ("c-3", 7, None),
    ] {
        let mut s = Space::with(
            "/m/files",
            &[("derive", "script_inline", json!(DERIVE_RECORDER))],
        );
        let vs = s.seed_text(F, "/calc/ops.py", &[OPS, apart.as_str()], &[]);
        let got = insert(&mut s, id, &vs[0], line, place, MUL);
        let at = format!("line {line} {place:?} on a moved base");
        assert_eq!(got["ok"], json!(true), "{at}: {got}");
        assert_eq!(got["rebased"], json!(true), "{at}: {got}");
        assert_eq!(got["base"], json!(&vs[0][..12]), "{at}: {got}");
        let (_, text) = head(&mut s, "c-5");
        assert_eq!(text, json!(formatted(&merged)), "{at}: both changes stand");

        let mut t = Space::with(
            "/m/files",
            &[("derive", "script_inline", json!(DERIVE_RECORDER))],
        );
        let vt = t.seed_text(F, "/calc/ops.py", &[OPS, at_end.as_str()], &[]);
        let got = insert(&mut t, id, &vt[0], line, place, MUL);
        assert_eq!(got["ok"], json!(false), "{at}, overlapping: {got}");
        assert_eq!(got["error"]["code"], json!("base_moved"), "{at}: {got}");
        assert_eq!(got["error"]["current"], json!(&vt[1][..12]), "{at}: {got}");
        let (ver, _) = head(&mut t, "c-6");
        assert_eq!(ver, json!(&vt[1][..12]), "{at}: a refusal moves no head");
    }
}

#[test]
fn gh996_a_line_given_as_a_string_is_refused_at_n_plus_one_too() {
    if !shipped() {
        return;
    }
    // `line` is an integer on the menu; a string is refused before any line
    // is counted. N+1 opens no second road: `"7"` gets the answer `"6"` gets,
    // and neither moves the head.
    for (id, line, place) in [
        ("c-1", "7", Some("after")),
        ("c-2", "7", None),
        ("c-3", "7", Some("before")),
        ("c-4", "6", Some("after")),
    ] {
        let (mut s, v) = space(OPS);
        let mut args = json!({"file": F, "base": &v[..12], "line": line, "text": MUL});
        if let Some(w) = place {
            args["where"] = json!(w);
        }
        let got = call(&mut s, "file_insert", id, args);
        let at = format!("line {line:?} {place:?}");
        assert_eq!(got["ok"], json!(false), "{at}: {got}");
        assert_eq!(got["error"]["code"], json!("bad_request"), "{at}: {got}");
        let m = got["error"]["message"].as_str().unwrap_or_default();
        assert!(
            m.contains("line") && m.contains("integer"),
            "{at}: the message names the field and its type: {got}"
        );
        let (ver, _) = head(&mut s, "c-5");
        assert_eq!(ver, json!(&v[..12]), "{at}: a refusal moves no head");
    }
}

#[test]
fn gh996_the_tool_description_says_how_to_append() {
    if !shipped() {
        return;
    }
    let mut seen = Vec::new();
    for cell in ["tools", "schemas"] {
        let offer = pure(cell, "FILE_OFFER", json!(null));
        let ins = offer
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == json!("file_insert"))
            .unwrap_or_else(|| panic!("{cell}: file_insert is offered"))
            .clone();
        let d = ins["description"].as_str().unwrap().to_string();
        assert!(
            d.contains("appends at the end") && d.contains("`after` or `before` the line after"),
            "{cell}: file_insert says how to append: {d}"
        );
        seen.push(d);
    }
    assert_eq!(seen[0], seen[1], "both copies say it word for word");
}
