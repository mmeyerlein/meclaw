//! GH #907 -- `file-space/ingest` and `file-space/extract`: how a document's
//! name becomes a path in the inbox, which line the turn carries for it, how
//! `pdftotext` output splits into pages -- tables over the shipped
//! `script_inline`, loaded via `ast` (README § 3 of wave File Hive B1) -- and
//! the road through the space in one process: a turn whose `attachments`
//! point at a PDF in the blob store goes in on `in_ingest`, the file is born
//! with one `derived` page per page, and the turn comes out on `turn` without
//! `attachments`, carrying the address line; a failure on the way still hands
//! the turn on. The document travels as a blob reference only (R-FJ-1):
//! neither its bytes nor its pages sit in a hop, a context or a `pending` row.
//!
//! The road from a Telegram update through a member is
//! `gh907_a_pdf_from_telegram_is_read_by_its_address.rs`.

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};

/// R2b / GH #49: a tree without the cells skips.
fn i_shipped() -> bool {
    repo("templates/file-space/ingest/config.json").is_file()
        && repo("templates/file-space/extract/config.json").is_file()
}

fn two_pages() -> Vec<u8> {
    std::fs::read(repo("crates/meclaw-cells/tests/fixtures/two_pages.pdf"))
        .expect("the two-page fixture")
}

fn two_pages_b64() -> String {
    b64(&two_pages())
}

/// The blob ids of the space's blob store in the road tests.
const PDF_BLOB: &str = "01920000-0000-7000-8000-000000000001";
const TXT_BLOB: &str = "01920000-0000-7000-8000-000000000002";
const NO_BLOB: &str = "01920000-0000-7000-8000-00000000dead";

fn s(v: Value) -> String {
    v.as_str().expect("a string").to_string()
}

#[test]
fn a_name_is_made_safe_for_a_path() {
    if !i_shipped() {
        return;
    }
    for (raw, want) in [
        ("two_pages.pdf", "two_pages.pdf"),
        ("a/b\\c.pdf", "a_b_c.pdf"),
        ("tab\there\u{1}.txt", "tabhere.txt"),
        ("   ", "document"),
        ("", "document"),
        ("..", "_"),
        (".", "_"),
        ("Bericht März.pdf", "Bericht März.pdf"),
        // Review m-5: the address line quotes the name in `"` inside `[...]`.
        ("a \"b\" [c].pdf", "a 'b' (c).pdf"),
    ] {
        assert_eq!(
            s(pure("ingest", "norm_name(ARGS)", json!(raw))),
            want,
            "{raw:?}"
        );
    }
    let long = format!("{}.pdf", "x".repeat(300));
    let got = s(pure("ingest", "norm_name(ARGS)", json!(long)));
    assert_eq!(got.chars().count(), 120, "at most 120 characters");
    assert!(
        got.ends_with(".pdf"),
        "the extension survives the cut: {got}"
    );
}

#[test]
fn a_taken_path_gets_the_next_number_before_the_extension() {
    if !i_shipped() {
        return;
    }
    for (name, n, want) in [
        ("two_pages.pdf", 1, "two_pages.pdf"),
        ("two_pages.pdf", 2, "two_pages (2).pdf"),
        ("a.b.tar", 3, "a.b (3).tar"),
        ("noext", 2, "noext (2)"),
        (".bashrc", 2, ".bashrc (2)"),
    ] {
        assert_eq!(
            s(pure(
                "ingest",
                "with_suffix(ARGS[0], ARGS[1])",
                json!([name, n])
            )),
            want
        );
    }
    let got = pure(
        "ingest",
        "free_path(inbox_paths('a.pdf', '2026-09-29'), ARGS)",
        json!(["/inbox/2026-09-29/a.pdf", "/inbox/2026-09-29/a (2).pdf"]),
    );
    assert_eq!(s(got), "/inbox/2026-09-29/a (3).pdf");
    let got = pure(
        "ingest",
        "free_path(inbox_paths('a.pdf', '2026-09-29'), [])",
        json!(null),
    );
    assert_eq!(s(got), "/inbox/2026-09-29/a.pdf");
    // Review I-3: one earlier claim that has not chosen yet owns the first
    // free candidate, so this search takes the second.
    let got = pure(
        "ingest",
        "free_path(inbox_paths('a.pdf', '2026-09-29'), ARGS, 1)",
        json!(["/inbox/2026-09-29/a (2).pdf"]),
    );
    assert_eq!(s(got), "/inbox/2026-09-29/a (3).pdf");
    let claims = json!([
        {"op_id": "c:ing-1", "phase": "path:/inbox/d/a.pdf", "at": "1"},
        {"op_id": "c:ing-2", "phase": "claim", "at": "2"},
        {"op_id": "c:ing-3", "phase": "claim", "at": "3"},
        {"op_id": "c:ing-4", "phase": "claim", "at": "4"}]);
    assert_eq!(
        pure(
            "ingest",
            "ahead_of(ARGS, 'c:ing-3', '3', False)",
            claims.clone()
        ),
        json!([["/inbox/d/a.pdf"], 1]),
        "chosen paths are taken; only earlier open claims come first"
    );
    assert_eq!(
        pure("ingest", "ahead_of(ARGS, 'c:ing-3', '3', True)", claims),
        json!([["/inbox/d/a.pdf"], 2]),
        "on a retry every other open claim comes first"
    );
}

#[test]
fn the_turn_carries_one_line_for_the_file() {
    if !i_shipped() {
        return;
    }
    let line = |args: Value| s(pure("ingest", "address_line(*ARGS)", args));
    assert_eq!(
        line(json!([
            "fh-0123456789ab",
            "abcdef0123456789ffff",
            "two_pages.pdf",
            2,
            "A test with two pages.",
            true
        ])),
        "[file fh-0123456789ab@abcdef012345 \"two_pages.pdf\", 2 pages: A test with two pages.]"
    );
    assert_eq!(
        line(json!([
            "fh-0123456789ab",
            "abcdef012345",
            "one.pdf",
            1,
            "",
            true
        ])),
        "[file fh-0123456789ab@abcdef012345 \"one.pdf\", 1 page]"
    );
    assert_eq!(
        line(json!([
            "fh-0123456789ab",
            "abcdef012345",
            "n.txt",
            0,
            "notes\n  of a day",
            false
        ])),
        "[file fh-0123456789ab@abcdef012345 \"n.txt\": notes of a day]"
    );
    assert_eq!(
        s(pure(
            "ingest",
            "failure_line('x.pdf', 'too_large')",
            json!(null)
        )),
        "[file \"x.pdf\" could not be stored: too_large]"
    );
    // The line is one text turn right after the caption; `attachments` go.
    let turn = json!({"messages": [
        {"origin": "user", "type": "text", "text": "caption"}],
        "attachments": [{"blob_id": PDF_BLOB, "mime_type": "application/pdf",
                         "filename": "a.pdf", "size_bytes": 1, "sha256": "00"}]});
    let got = pure("ingest", "with_line(ARGS, 'LINE')", turn.clone());
    assert_eq!(
        got,
        json!({"messages": [{"origin": "user", "type": "text", "text": "caption"},
                            {"origin": "user", "type": "text", "text": "LINE"}]})
    );
    // The reference is kept without anything a reader may have added.
    let mut read = turn;
    read["attachments"][0]["data_b64"] = json!("AAAA");
    let got = pure("ingest", "first_ref(ARGS)", read);
    assert_eq!(got["blob_id"], json!(PDF_BLOB));
    assert!(got.get("data_b64").is_none(), "{got}");
    assert_eq!(
        pure("ingest", "first_ref(ARGS)", json!({"messages": []})),
        Value::Null
    );
}

#[test]
fn pdftotext_output_splits_into_pages() {
    if !i_shipped() {
        return;
    }
    for (text, max, want) in [
        (
            "page one\n\u{c}page two\n\u{c}",
            500,
            json!(["page one", "page two"]),
        ),
        ("a\u{c}b\u{c}c\u{c}", 2, json!(["a", "b"])),
        ("only\n", 500, json!(["only"])),
        ("", 500, json!([])),
    ] {
        assert_eq!(
            pure(
                "extract",
                "split_pages(ARGS[0], ARGS[1])",
                json!([text, max])
            ),
            want,
            "{text:?}"
        );
    }
}

#[test]
fn the_fixture_pdf_reads_as_two_pages_and_a_missing_program_says_so() {
    if !i_shipped() {
        return;
    }
    let b64 = two_pages_b64();
    let probe = "extract(ARGS['mime'], base64.b64decode(ARGS['b64']), ARGS['cmd'], 30, 500)";
    let got = pure(
        "extract",
        probe,
        json!({"mime": "application/pdf", "b64": b64, "cmd": "pdftotext"}),
    );
    assert_eq!(
        got,
        json!({"pages": ["page one", "page two"], "tool": "pdftotext"}),
        "poppler-utils must be installed where this test runs"
    );
    let got = pure(
        "extract",
        probe,
        json!({"mime": "application/pdf", "b64": b64, "cmd": "no-such-extractor-gh907"}),
    );
    assert_eq!(got, json!({"pages": [], "error": "no_extractor"}));
    let got = pure(
        "extract",
        probe,
        json!({"mime": "text/plain", "b64": b64, "cmd": "pdftotext"}),
    );
    assert_eq!(got, json!({"pages": [], "error": "unsupported_mime"}));
}

// ------------------------------------------------------------ the road

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn blobs(sp: &mut Space) {
    sp.blobs.insert(PDF_BLOB.into(), two_pages());
    sp.blobs.insert(TXT_BLOB.into(), b"hello\n".to_vec());
}

fn space() -> Space {
    // No embedding endpoint in this process: `derive` embeds nothing, the
    // summary still runs and `derived` still leaves (B1 E).
    let mut sp = Space::with("/x/files", &[("derive", "embed", json!("0"))]);
    blobs(&mut sp);
    sp
}

/// A document turn as the channel emits it (interface (1) of GH #907): the
/// caption as a text turn, the document as a blob reference.
fn turn_with(name: &str, mime: &str, blob: &str) -> Value {
    let size = if blob == PDF_BLOB {
        two_pages().len()
    } else {
        6
    };
    json!({"messages": [
        {"origin": "user", "type": "text", "text": "what is this?"}],
        "attachments": [{"blob_id": blob, "mime_type": mime, "filename": name,
                         "size_bytes": size, "sha256": sha256_hex(b"any")}]})
}

/// The store operations on `pending` among everything the space delivered.
fn pending_ops(sp: &Space) -> Vec<String> {
    let mut out = Vec::new();
    for m in sp.sent.iter().filter(|m| m["to"] == "./store") {
        for c in m["body"]["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let op: Value = meclaw_core::serde_json::from_str(c["text"].as_str().unwrap_or(""))
                .unwrap_or(Value::Null);
            if op["table"] == "pending" {
                out.push(format!("{} {}", m["from"], op));
            }
        }
    }
    out
}

fn ingest(sp: &mut Space, turn: Value) {
    sp.lane(
        "in_ingest",
        json!({"chat_id": 100, "channel": "telegram"}),
        json!({"engine": "text"}),
        turn,
    );
}

fn released(sp: &Space) -> Msg {
    let t = sp.routed("turn");
    assert_eq!(
        t.len(),
        1,
        "exactly one turn leaves; stderr {:?}",
        sp.stderr
    );
    t[0].clone()
}

#[test]
fn a_pdf_turn_is_born_as_a_file_and_leaves_with_its_address() {
    if !shipped() || !i_shipped() {
        return;
    }
    let mut sp = space();
    ingest(
        &mut sp,
        turn_with("two_pages.pdf", "application/pdf", PDF_BLOB),
    );
    assert!(
        sp.routed("turn").is_empty(),
        "the turn waits for the summary"
    );
    // The summary names no page text: the scans below look for it.
    sp.llm_answer("Two pages of a test.\n\nA short test document.", "stop");

    let path = format!("/inbox/{}/two_pages.pdf", today());
    let files = sp.rows(&format!(
        "SELECT file, head, mime FROM files WHERE path = '{path}'"
    ));
    assert_eq!(files.len(), 1, "one file under the inbox path");
    let file = files[0][0].as_str().unwrap().to_string();
    let head = files[0][1].as_str().unwrap().to_string();
    assert_eq!(files[0][2], json!("application/pdf"));
    let pages = sp.rows(&format!(
        "SELECT part, body FROM derived WHERE file = '{file}' AND version = '{head}' \
         ORDER BY part"
    ));
    assert_eq!(
        pages,
        vec![
            vec![json!(1), json!("page one")],
            vec![json!(2), json!("page two")]
        ]
    );

    let out = released(&sp);
    assert_eq!(
        out.hop.get("engine"),
        Some(&json!("text")),
        "hop keys go on"
    );
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_job"] {
        assert!(!out.context.contains_key(k), "{k} leaves the space");
    }
    assert_eq!(
        out.context.get("chat_id"),
        Some(&json!(100)),
        "the context goes on"
    );
    let msgs = out.messages();
    assert_eq!(msgs[0]["text"], "what is this?", "the caption is untouched");
    assert_eq!(
        msgs[1]["text"],
        json!(format!(
            "[file {file}@{} \"two_pages.pdf\", 2 pages: Two pages of a test.]",
            &head[..12]
        ))
    );
    assert_eq!(msgs.len(), 2, "one line after the caption: {msgs:?}");
    assert!(
        !out.body.contains_key("attachments"),
        "the turn leaves without its attachments"
    );
    let wire = Value::Object(out.body.clone()).to_string();
    let b64 = two_pages_b64();
    assert!(
        !wire.contains("data_b64") && !wire.contains(&b64[..64]),
        "no bytes leave"
    );
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM pending WHERE cell = 'ingest'")[0][0],
        json!(0),
        "nothing stays parked"
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);

    // R-FJ-1: the document is a blob reference everywhere behind the
    // channel. Neither its bytes nor its pages sit in a `pending` row or in
    // a header (hop and context) of any message the space delivered, and a
    // header stays <= 8 KiB (a message_log header row, measured).
    let needles = [&b64[..64], "page one", "page two"];
    for op in pending_ops(&sp) {
        for n in needles {
            assert!(!op.contains(n), "a pending op carries {n:?}: {op}");
        }
    }
    for m in &sp.sent {
        let header = m["header"].to_string();
        assert!(
            header.len() <= 8192,
            "{} -> {} header of {} bytes",
            m["from"],
            m["to"],
            header.len()
        );
        for n in needles {
            assert!(
                !header.contains(n),
                "{} -> {} carries {n:?} in its header",
                m["from"],
                m["to"]
            );
        }
    }
    // The pages travel once, in the body of the `create` `./extract` sends.
    let creates: Vec<&Value> = sp
        .sent
        .iter()
        .filter(|m| m["from"] == "./extract" && m["to"] == "./write")
        .collect();
    assert_eq!(creates.len(), 1, "extract sends one create");
    assert_eq!(creates[0]["body"]["args"]["attachment"], json!(0));
    assert_eq!(
        creates[0]["body"]["args"]["derived"],
        json!([{"part": 1, "text": "page one"}, {"part": 2, "text": "page two"}])
    );
    assert_eq!(
        creates[0]["body"]["attachments"][0]["blob_id"],
        json!(PDF_BLOB)
    );
    assert!(
        creates[0]["body"]["attachments"][0]
            .get("data_b64")
            .is_none(),
        "the reference goes on without the bytes the reader added"
    );
    let unscoped: Vec<String> = sp
        .unscoped()
        .into_iter()
        .filter(|x| x.starts_with("ingest:"))
        .collect();
    assert!(unscoped.is_empty(), "{unscoped:?}");
}

#[test]
fn a_second_document_of_the_same_name_is_a_second_file() {
    if !shipped() || !i_shipped() {
        return;
    }
    let mut sp = space();
    for _ in 0..2 {
        ingest(&mut sp, turn_with("n.txt", "text/plain", TXT_BLOB));
        sp.llm_answer("Hello.\n\nA greeting.", "stop");
    }
    let rows = sp.rows("SELECT path FROM files ORDER BY path");
    let day = today();
    assert_eq!(
        rows,
        vec![
            vec![json!(format!("/inbox/{day}/n (2).txt"))],
            vec![json!(format!("/inbox/{day}/n.txt"))]
        ]
    );
    let turns = sp.routed("turn");
    assert_eq!(turns.len(), 2);
    let last = turns[1].messages()[1]["text"].as_str().unwrap().to_string();
    assert!(last.contains("\"n (2).txt\": Hello.]"), "{last}");
    assert!(
        !last.contains("page"),
        "a text file has no page count: {last}"
    );
}

#[test]
fn a_file_that_cannot_be_born_still_hands_the_turn_on() {
    if !shipped() || !i_shipped() {
        return;
    }
    let mut sp = Space::with(
        "/x/files",
        &[
            ("derive", "embed", json!("0")),
            ("write", "max_bytes", json!(3)),
        ],
    );
    blobs(&mut sp);
    ingest(&mut sp, turn_with("n.txt", "text/plain", TXT_BLOB));
    let out = released(&sp);
    assert_eq!(
        out.messages()[1]["text"],
        "[file \"n.txt\" could not be stored: too_large]"
    );
    assert!(!out.body.contains_key("attachments"));
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM pending WHERE cell = 'ingest'")[0][0],
        json!(0)
    );

    // A reference the blob store does not know: `./write` reads it and
    // answers `not_found` -- for a PDF through `./extract` as well.
    for (name, mime) in [("n.txt", "text/plain"), ("x.pdf", "application/pdf")] {
        let mut sp = space();
        ingest(&mut sp, turn_with(name, mime, NO_BLOB));
        assert_eq!(
            released(&sp).messages()[1]["text"],
            json!(format!("[file \"{name}\" could not be stored: not_found]")),
            "{mime}; stderr {:?}",
            sp.stderr
        );
        assert_eq!(
            sp.rows("SELECT COUNT(*) FROM pending WHERE cell = 'ingest'")[0][0],
            json!(0)
        );
    }

    // A reference that is no UUID: `bad_ref`.
    let mut sp = space();
    ingest(&mut sp, turn_with("n.txt", "text/plain", "not-a-blob"));
    assert_eq!(
        released(&sp).messages()[1]["text"],
        "[file \"n.txt\" could not be stored: bad_ref]"
    );

    // An entry that is no object: refused before anything is parked.
    let mut sp = space();
    ingest(
        &mut sp,
        json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}],
               "attachments": ["x"]}),
    );
    assert_eq!(
        released(&sp).messages(),
        vec![
            json!({"origin": "user", "type": "text", "text": "hi"}),
            json!({"origin": "user", "type": "text",
                   "text": "[file \"document\" could not be stored: bad_request]"})
        ]
    );
    assert!(sp.sent.iter().all(|m| m["to"] != "./store"));

    // A turn without attachments goes on untouched.
    let mut sp = space();
    ingest(
        &mut sp,
        json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]}),
    );
    assert_eq!(
        released(&sp).messages(),
        vec![json!({"origin": "user", "type": "text", "text": "hi"})]
    );
}

/// Review I-3: two documents of one name in one poll. Both path searches run
/// before either `create`, and `write` reads a path in one phase and inserts
/// it in a later one -- without the claim both are born under one path.
#[test]
fn two_documents_of_one_name_in_one_batch_are_two_files() {
    if !shipped() || !i_shipped() {
        return;
    }
    let mut sp = space();
    let one = |caption: &str| {
        let mut t = turn_with("scan.pdf", "application/pdf", PDF_BLOB);
        t["messages"][0]["text"] = json!(caption);
        Space::on_lane(
            "in_ingest",
            json!({"chat_id": 100}),
            json!({"engine": "text"}),
            t,
        )
    };
    sp.pump_all(vec![
        (".".into(), one("first")),
        (".".into(), one("second")),
    ]);
    while !sp.llm.is_empty() {
        sp.llm_answer("A scan.\n\nTwo pages.", "stop");
    }
    let day = today();
    assert_eq!(
        sp.rows("SELECT path FROM files ORDER BY path"),
        vec![
            vec![json!(format!("/inbox/{day}/scan (2).pdf"))],
            vec![json!(format!("/inbox/{day}/scan.pdf"))]
        ],
        "two files, two paths; stderr {:?}",
        sp.stderr
    );
    let turns = sp.routed("turn");
    assert_eq!(turns.len(), 2, "both turns leave");
    for t in &turns {
        let line = t.messages()[1]["text"].as_str().unwrap().to_string();
        assert!(
            line.starts_with("[file fh-") && line.contains("2 pages"),
            "{line}"
        );
    }
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM pending WHERE cell = 'ingest'")[0][0],
        json!(0),
        "no request or claim row stays"
    );
    assert!(sp.store_errors.is_empty(), "{:?}", sp.store_errors);
}

/// Review I-3: a path taken between the search and the `create` (a writer
/// outside this cell) is searched again with the same reference. A PDF asks
/// `./extract` a second time: the pages are parked nowhere (R-FJ-1), and a
/// second `pdftotext` run on this rare path is the price.
#[test]
fn a_path_taken_after_the_search_is_searched_again() {
    if !shipped() || !i_shipped() {
        return;
    }
    let mut sp = space();
    let day = today();
    sp.before = Some((
        "write".into(),
        "in_write".into(),
        format!(
            "INSERT INTO files (file, path, kind, mime, head, head_seq, lock, tomb, oneline, \
             bytes, lines, born_at) VALUES ('fh-000000000000', '/inbox/{day}/scan.pdf', \
             'binary', 'application/pdf', '', 0, '', '', '', 0, 0, '')"
        ),
    ));
    ingest(&mut sp, turn_with("scan.pdf", "application/pdf", PDF_BLOB));
    assert!(sp.before.is_none(), "the outside writer came first");
    sp.llm_answer("A scan.\n\nTwo pages.", "stop");
    let line = released(&sp).messages()[1]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        line.contains("\"scan (2).pdf\", 2 pages: A scan."),
        "{line}"
    );
    let extracts = sp.sent.iter().filter(|m| m["to"] == "./extract").count();
    assert_eq!(extracts, 2, "the second try asks for the pages again");
    assert_eq!(
        sp.rows("SELECT COUNT(*) FROM pending WHERE cell = 'ingest'")[0][0],
        json!(0)
    );
}

/// Review m-4: a summary that fails still releases the turn -- name and page
/// count, no one-line summary (OR-FJ-G7).
#[test]
fn a_failed_summary_still_releases_the_turn() {
    if !shipped() || !i_shipped() {
        return;
    }
    let mut sp = space();
    ingest(
        &mut sp,
        turn_with("two_pages.pdf", "application/pdf", PDF_BLOB),
    );
    sp.llm_answer("", "error");
    let line = released(&sp).messages()[1]["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        line.starts_with("[file fh-") && line.ends_with("\"two_pages.pdf\", 2 pages]"),
        "{line}"
    );
}
