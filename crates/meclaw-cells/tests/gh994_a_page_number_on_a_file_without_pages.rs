//! GH #994: a page number on a file without pages does not refuse the read.
//!
//! Models in the OpenAI style fill every optional field of a tool schema and
//! send `file_read {"from": 1, "page": 0, "to": 6}` for a source file. `page`
//! took precedence over `from`/`to` in `./read`, and a source file has no
//! pages, so every such read answered `page_unknown` and the round burned
//! (the logs of a measured run: every file read of two hosted models refused
//! this way, none of a local one that leaves optional fields out).
//!
//! The contract: `page` picks among the real pages of a paged document only.
//! On a file without pages a `page` of 0 or 1 is ignored -- the range, or the
//! whole file, is read -- and the answer names the field it ignored
//! (`ignored: ["page"]`); any other page number on such a file stays
//! `page_unknown`. A paged document pages as before. Every assertion is made
//! on the one `tool_result` the model receives.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

const PY: &str = "fh-0a0b0c0d0e01";
const DOC: &str = "fh-0a0b0c0d0e02";
const SOURCE: &str = "import os\n\n\ndef main():\n    return os.getcwd()\n\n\nif __name__ == \"__main__\":\n    main()\n";

/// `<n>:<h4>|<text>` for lines `from..=to` of `src`, computed here.
fn formatted(src: &str, from: usize, to: usize) -> String {
    src.lines()
        .enumerate()
        .skip(from - 1)
        .take(to + 1 - from)
        .map(|(i, l)| format!("{}:{}|{}", i + 1, h4(l), l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One `file_read` tool call on the space's `in_tool` lane, as the core's
/// model sends it; the parsed text of the ONE `tool_result` that leaves.
fn file_read(s: &mut Space, id: &str, args: Value) -> Value {
    let before = s.out.len();
    s.lane(
        "in_tool",
        json!({"tool_caller": "talky"}),
        json!({"tool_name": "file_read", "tool_call_id": id}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                             "text": args.to_string()}]}),
    );
    let mine: Vec<Msg> = s.out[before..].to_vec();
    assert_eq!(
        mine.len(),
        1,
        "exactly one message leaves, got {mine:?}; stderr {:?}",
        s.stderr
    );
    assert_eq!(mine[0].route(), "tool_result", "{:?}", mine[0]);
    let msgs = mine[0].messages();
    assert_eq!(msgs.len(), 1);
    let text = msgs[0]["text"].as_str().unwrap();
    meclaw_core::serde_json::from_str(text).expect("the result is JSON")
}

fn source_space() -> (Space, String) {
    let mut s = Space::new();
    let v = s.seed_text(PY, "/src/tool.py", &[SOURCE], &[]).remove(0);
    (s, v)
}

#[test]
fn gh994_a_page_zero_on_a_source_file_reads_the_range() {
    if !shipped() {
        return;
    }
    let (mut s, v) = source_space();
    // The call of the measured run, word for word in its fields.
    let got = file_read(
        &mut s,
        "c-1",
        json!({"file": PY, "from": 1, "page": 0, "to": 6}),
    );
    assert_eq!(got["ok"], json!(true), "{got}");
    assert_eq!(got["file"], json!(PY));
    assert_eq!(got["version"], json!(&v[..12]));
    assert_eq!(
        (got["from"].clone(), got["to"].clone()),
        (json!(1), json!(6))
    );
    assert_eq!(got["text"], json!(formatted(SOURCE, 1, 6)), "{got}");
    assert_eq!(
        got["ignored"],
        json!(["page"]),
        "the answer names it: {got}"
    );
    assert!(
        got.get("page").is_none() && got.get("pages").is_none(),
        "{got}"
    );

    // `page: 1` beside a range is the same: the range is read.
    let got = file_read(
        &mut s,
        "c-2",
        json!({"file": "/src/tool.py", "from": 4, "to": 5, "page": 1}),
    );
    assert_eq!(got["text"], json!(formatted(SOURCE, 4, 5)), "{got}");
    assert_eq!(got["ignored"], json!(["page"]));
}

#[test]
fn gh994_page_one_without_range_reads_the_file() {
    if !shipped() {
        return;
    }
    let (mut s, _) = source_space();
    let got = file_read(&mut s, "c-1", json!({"file": PY, "page": 1}));
    assert_eq!(got["ok"], json!(true), "{got}");
    assert_eq!(
        (got["from"].clone(), got["to"].clone()),
        (json!(1), json!(9))
    );
    assert_eq!(got["text"], json!(formatted(SOURCE, 1, 9)));
    assert_eq!(got["ignored"], json!(["page"]));

    // A read without `page` names nothing ignored.
    let plain = file_read(&mut s, "c-2", json!({"file": PY}));
    assert_eq!(plain["text"], got["text"]);
    assert!(plain.get("ignored").is_none(), "{plain}");
}

#[test]
fn gh994_another_page_on_a_source_file_is_refused() {
    if !shipped() {
        return;
    }
    let (mut s, _) = source_space();
    for (id, page) in [("c-1", 2), ("c-2", 7), ("c-3", -1)] {
        let got = file_read(
            &mut s,
            id,
            json!({"file": PY, "from": 1, "to": 3, "page": page}),
        );
        assert_eq!(
            got["error"]["code"],
            json!("page_unknown"),
            "page {page} on a file without pages: {got}"
        );
    }
}

#[test]
fn gh994_a_paged_document_still_pages() {
    if !shipped() {
        return;
    }
    let mut s = Space::new();
    s.seed_file(DOC, "/doc.pdf", "binary", "application/pdf");
    let v = s.seed_version(DOC, b"%PDF-1.7\x00\x01binary", &[], "");
    s.seed_head(DOC, &v, 1000, "create");
    s.seed_derived(DOC, &v, 1, "first\npage\n");
    s.seed_derived(DOC, &v, 2, "second\n");
    let view = "--- page 1 ---\nfirst\npage\n--- page 2 ---\nsecond\n";

    // `page` still takes precedence over a range on a paged document.
    let got = file_read(
        &mut s,
        "c-1",
        json!({"file": DOC, "from": 1, "to": 2, "page": 2}),
    );
    assert_eq!(got["ok"], json!(true), "{got}");
    assert_eq!(
        (got["page"].clone(), got["pages"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(got["text"], json!(formatted(view, 4, 5)));
    assert!(got.get("ignored").is_none(), "{got}");
    let got = file_read(&mut s, "c-2", json!({"file": DOC, "page": 1}));
    assert_eq!(got["text"], json!(formatted(view, 1, 3)));
    assert!(got.get("ignored").is_none(), "{got}");
    // A page the document does not have is refused, 0 included.
    for (id, page) in [("c-3", 0), ("c-4", 3)] {
        let got = file_read(&mut s, id, json!({"file": DOC, "page": page}));
        assert_eq!(got["error"]["code"], json!("page_unknown"), "{got}");
    }
}

#[test]
fn gh994_the_tool_description_names_the_rule() {
    if !shipped() {
        return;
    }
    for cell in ["tools", "schemas"] {
        let offer = pure(cell, "FILE_OFFER", json!(null));
        let read = offer
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == json!("file_read"))
            .unwrap_or_else(|| panic!("{cell}: file_read is offered"));
        let d = read["description"].as_str().unwrap();
        assert!(
            d.contains("paged document") && d.contains("ignored"),
            "{cell}: file_read says `page` is for paged documents and ignored elsewhere: {d}"
        );
        let p = read["parameters"]["properties"]["page"]["description"]
            .as_str()
            .unwrap();
        assert!(
            p.contains("paged document"),
            "{cell}: the `page` parameter says it: {p}"
        );
    }
}
