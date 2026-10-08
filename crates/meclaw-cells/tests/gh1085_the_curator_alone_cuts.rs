//! GH #1085 (R-IG-1) -- the curator is the one place that cuts what its model
//! reads, and it cuts by shares of the usable window, never by a typed number.
//!
//! Until curator 1.11.0 / collector 5.1.0 the road to the model carried fixed
//! bounds: an earlier answer's sidecar block left the window over 6000
//! characters, the collector dropped optional contract sections over 6000
//! (said on stderr alone), a tool result in the summarizer's transcript was
//! cut at 2000 with " [cut]", the handover block at 3000, the note's
//! transcript at 60000, a topic at 240, a find said aloud at 400, a gap at
//! 300, a pin was refused over 16000 and a push candidate over 600. Pinned
//! here, through the shipped hive (`support/curator_hive.rs`) and the pure
//! halves of the shipped scripts, for every bound that fell:
//!
//! * over the old value, without a known window, the content comes WHOLE;
//! * over its share of a known window it comes with the mark that names what
//!   was shown of how much (`...[cut: <shown> of <total> chars shown; ...]`,
//!   `...[dropped: <name> (<n> chars) over budget]`), and the cut is on the
//!   hop (`cuts`) or the ledger mark;
//! * the window is the one of the model that READS the content (OR-IG-4):
//!   the summarizer's for its transcripts -- the registry's push of its
//!   package, the stamp on its answers, the bound of a refusal, before them
//!   the catalogue row in `input_soft_fallback` (OR-IG-8) --, never the one
//!   this hive serves; a find said aloud to the voice model goes whole;
//! * over the carrier (one system leaf, 4 MiB) a pin is refused with its
//!   size and the bound;
//! * the contract's frame is written anew over the sections kept (GH #606);
//! * every ask of a producer carries the window as `input_soft`;
//! * the usable window is the catalog row of the answering model alone: no
//!   role types one (`quality_cap`, talky 120 000 tokens until curator
//!   1.11.2), so a talky under luna rebuilds at the row's lines, not at
//!   60 000.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// Imports, defs and upper-case constants of a script, under `params`; then
/// `probe` -- a python expression over that scope -- printed as JSON (the
/// loader `support/curator_hive.rs` keeps for the policy, for any script; a
/// constant of the message -- the round, the lane -- does not load and is
/// left out).
const PURE: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"P": params, "doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    try:
        exec(compile(ast.Module(body=[n], type_ignores=[]), "cell", "exec"), scope)
    except NameError:
        pass
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

fn pure(script: &str, probe: &str, args: Value) -> Value {
    let doc = json!({"src": script, "params": {}, "probe": probe, "args": args});
    let out = run_python(PURE, &doc.to_string());
    assert!(
        out.status.success(),
        "the pure half does not load: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    sj::from_slice(&out.stdout).expect("json")
}

fn talky() -> Hive {
    Hive::with(&[("policy", "role", json!("talky"))])
}

fn put_state(h: &Hive, key: &str, value: &str) {
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )
    .unwrap();
}

fn cuts_of(m: &Msg) -> Vec<Value> {
    m.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

// ============================================== 1. the window of ./policy

/// The row of the llm-registry catalog for `model_id`, as the llm cell
/// stamps it on an answer (GH #1037): limits and prices, and the window.
fn catalog_row(model_id: &str) -> Value {
    let text = std::fs::read_to_string(repo("templates/llm-registry/store/seed/models.jsonl"))
        .expect("the catalog");
    let row: Value = text
        .lines()
        .filter_map(|l| sj::from_str::<Value>(l).ok())
        .find(|r| r["model_id"] == model_id)
        .unwrap_or_else(|| panic!("no catalog row {model_id}"));
    let mut out = json!({});
    for k in [
        "input_soft",
        "input_hard",
        "cost_in",
        "cost_cached_in",
        "context_window",
    ] {
        out[k] = row[k].clone();
    }
    out
}

/// GH #1085 (R-IG-1): the usable window is the catalog row of the model, and
/// nothing else. Until curator 1.11.2 a role typed its own (`quality_cap`,
/// talky 120 000 tokens, GH #892): a talky under luna kept 120 000 as its
/// window, handed that to every producer, and on an answer without the
/// row's limits ordered a rebuild at half of it -- 60 000 tokens. Now the
/// first rebuild under luna is the row's: the cost rule (GH #1038) once
/// keeping the cached rest ten turns costs more than one rebuild to
/// `input_soft` x `rebuild_to` (over 175 000 at luna's prices), and `soft`
/// at `input_soft`.
#[test]
fn talky_under_luna_rebuilds_at_the_catalog_line() {
    if !shipped() {
        return;
    }
    let luna = catalog_row("openai/gpt-6-luna");
    assert_eq!(luna["input_soft"], json!(250000), "the row this lock names");
    // An answer with the model's window alone: 70 000 tokens are far under
    // half of it, nothing is ordered.
    let mut h = talky();
    turn(
        &mut h,
        "s1",
        "t1",
        "q",
        "a",
        json!({"tokens_prompt": 70000, "context_window": luna["context_window"]}),
    );
    assert!(h.clock.is_empty(), "no rebuild at 70 000: {:?}", h.clock);
    // With the row on the answer: a measured window of 120 000 tokens -- the
    // role's whole window of before -- orders nothing, and the window kept
    // for every producer is the row's `input_soft`.
    let mut h = talky();
    let mut usage = luna.clone();
    usage["tokens_prompt"] = json!(120000);
    turn(&mut h, "s1", "t1", &"x".repeat(360_003), "a", usage);
    assert!(h.clock.is_empty(), "no rebuild at 120 000: {:?}", h.clock);
    assert_eq!(h.state("input_soft"), "250000", "{:?}", h.stderr);
    // The lines themselves, out of the row alone.
    let (marks, err) = policy_scope(
        json!({"role": "talky"}),
        "[curate_mark(w, ARGS)['mark'] for w in (120000, 175000, 175001, 249999, 250000)]",
        luna,
    );
    assert_eq!(
        marks,
        json!(["none", "none", "cost", "cost", "soft"]),
        "{err}"
    );
}

fn block_answer(n: usize) -> String {
    format!("long.\n\n```sidecar\n{{\"a\": \"{}\"}}\n```", "x".repeat(n))
}

/// 7000 characters of block, over the 6000 of before: whole without a
/// window, and whole inside a large one (luna's row: 250000 tokens, 75000
/// characters of sidecar).
#[test]
fn an_earlier_block_over_the_old_bound_comes_whole() {
    if !shipped() {
        return;
    }
    let big = block_answer(7_000);
    for extra in [json!({}), json!({"input_soft": 250000})] {
        let mut h = talky();
        turn(&mut h, "s1", "t1", "q1", &big, extra.clone());
        let call = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
        let got = texts(&call);
        assert_eq!(got.len(), 3, "{extra}: {got:?}");
        assert_eq!(got[1], big, "{extra}");
        assert!(cuts_of(&call).is_empty(), "{extra}: {:?}", call.hop);
    }
}

fn assemble_script() -> String {
    read_json(&repo("templates/collector/assemble/config.json"))["params"]["script_inline"]
        .as_str()
        .expect("script")
        .to_string()
}

fn offer(section: &str, required: bool, words: &str, kind: &str) -> Value {
    json!({"section": section, "required": required, "instruction": words,
           "schema": {"type": kind}})
}

/// The collector's contract (`instructions.sidecar`) as `collector/assemble`
/// composes it (`sidecar_block`): its frame, a required section, two
/// optional ones of 500 characters.
fn contract_of(offers: Value) -> String {
    pure(&assemble_script(), "sidecar_block(ARGS)[0]", offers)
        .as_str()
        .expect("the contract")
        .to_string()
}

fn contract() -> String {
    contract_of(json!([
        offer("memory", true, "Write it.", "object"),
        offer("gap", false, &"g".repeat(500), "string"),
        offer("window", false, &"w".repeat(500), "object")
    ]))
}

fn contract_in_call(h: &mut Hive) -> (String, Msg) {
    h.lane(
        "in_slots",
        json!({}),
        json!({}),
        json!({"system": {"instructions": {"sidecar": {"text": contract()}}}}),
    );
    let call = h.curate("s1", "t2", 0, json!([user("q2")]), mode("Be brief."));
    let text = call.body["system"]["instructions"]["sidecar"]["text"]
        .as_str()
        .expect("the contract rides in the call")
        .to_string();
    (text, call)
}

/// The contract goes whole without a window; over its share the optional
/// sections fall from the back, each said in the contract and on the hop,
/// the required one stays -- and the frame is written anew over the sections
/// kept, byte for byte the frame the collector writes for them (review I1:
/// a frame that still named a dropped section asked the model for it, the
/// malformed blocks of GH #606). A tenth of 1000 tokens (300 characters)
/// keeps the required section alone; a tenth of 4000 (1200) keeps `gap`.
#[test]
fn the_contract_is_cut_by_the_curator_alone() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let (whole, call) = contract_in_call(&mut h);
    assert_eq!(whole, contract(), "no window: the contract whole");
    assert!(cuts_of(&call).is_empty());

    let gap = "## gap (optional)\n".len() + 500 + "\n{\"gap\":\"\"}".len();
    let mut h = Hive::new();
    turn(&mut h, "s1", "t1", "q1", "a1", json!({"input_soft": 1000}));
    let (cut, call) = contract_in_call(&mut h);
    let kept = contract_of(json!([offer("memory", true, "Write it.", "object")]));
    assert!(cut.starts_with(&format!("{kept}\n\n")), "{cut}");
    assert!(
        cut.contains(&format!("...[dropped: gap ({gap} chars) over budget]")),
        "{cut}"
    );
    assert!(cut.contains("...[dropped: window ("), "{cut}");
    let frame = cut.split("\n\n...[dropped: ").next().unwrap();
    for name in ["gap", "window"] {
        assert!(
            !frame.contains(name),
            "{name} is asked for nowhere: {frame}"
        );
    }
    let what: Vec<Value> = cuts_of(&call).iter().map(|c| c["what"].clone()).collect();
    assert_eq!(
        what,
        vec![
            json!("sidecar section gap"),
            json!("sidecar section window")
        ]
    );

    let mut h = Hive::new();
    turn(&mut h, "s1", "t1", "q1", "a1", json!({"input_soft": 4000}));
    let (cut, _) = contract_in_call(&mut h);
    let kept = contract_of(json!([
        offer("memory", true, "Write it.", "object"),
        offer("gap", false, &"g".repeat(500), "string")
    ]));
    assert!(cut.starts_with(&format!("{kept}\n\n")), "{cut}");
    assert!(cut.ends_with("over budget]"), "{cut}");
    assert!(cut.chars().count() <= 1200, "{}", cut.chars().count());
}

/// A heading inside an offer's own words opens no section (review M5): the
/// section it stands in falls whole, the heading with it.
#[test]
fn a_heading_in_an_offer_is_no_section() {
    if !shipped() {
        return;
    }
    let text = contract_of(json!([
        offer("memory", true, "Write it.", "object"),
        offer(
            "window",
            false,
            &format!("{}\n\n## Example\nnot a section", "w".repeat(500)),
            "object"
        )
    ]));
    let policy = script_of("policy");
    let got = pure(&policy, "fit_contract(ARGS, 300)", json!(text));
    let cut = got[0].as_str().unwrap();
    assert!(!cut.contains("Example"), "{cut}");
    assert_eq!(got[1].as_array().map(Vec::len), Some(1), "{}", got[1]);
}

/// The frame is one block of script, byte for byte the same in the collector
/// that writes it and the curator that writes it anew (GH #1085).
#[test]
fn the_contracts_frame_is_one_block() {
    if !shipped() {
        return;
    }
    let block = |s: &str| {
        let a = s.find("# >>> sidecar-frame v1").expect("the block opens");
        let b = s.find("# <<< sidecar-frame v1").expect("the block closes");
        s[a..b].to_string()
    };
    assert_eq!(block(&assemble_script()), block(&script_of("policy")));
}

/// One round with the tool result `result`, two plain rounds after it and
/// the cache clock fired: the rebuild's request is the newest the
/// summarizer holds.
fn rebuild_after(h: &mut Hive, n: u32, result: &str) {
    let t = |i: u32| format!("t{n}-{i}");
    let id = format!("c{n}");
    let call = h.curate(
        "s1",
        &t(0),
        0,
        json!([
            user(&format!("q{n}")),
            tool_call(&id, "look"),
            tool_result(&id, result)
        ]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", json!({}), json!([said(&format!("a{n}"))]));
    turn(
        h,
        "s1",
        &t(1),
        &format!("q{n}-1"),
        &format!("a{n}-1"),
        json!({}),
    );
    turn(
        h,
        "s1",
        &t(2),
        &format!("q{n}-2"),
        &format!("a{n}-2"),
        json!({"cache_expires_at": format!("2099-01-01T00:{n:02}:00Z")}),
    );
    h.fire(&last_add(h));
}

fn transcript_of(req: &Msg) -> String {
    req.messages()[0]["text"].as_str().unwrap().to_string()
}

/// The summarizer's transcript is measured by the window of the model that
/// READS it (OR-IG-4), never by the window this hive serves: a tool result
/// over the 2000 characters of before goes whole while the summarizer's
/// window is unknown, though the served window is stamped (3000 tokens, whose
/// half would be 4500 characters); once the summarizer's answer named its
/// own (the llm cell's stamp, 1000 tokens: half of it, 1500 characters), the
/// next transcript keeps to it, the long line cut with its length, the cut on
/// the hop of the request.
#[test]
fn the_transcript_keeps_to_the_summarizers_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "input_soft_fallback", Value::Null),
    ]);
    turn(&mut h, "s1", "t0", "q0", "a0", json!({"input_soft": 3000}));
    assert_eq!(h.state("input_soft"), "3000", "the served window is known");
    let first = "r".repeat(5_000);
    rebuild_after(&mut h, 1, &first);
    let req = h.answer_with("S1.", "stop", json!({"input_soft": 1000}));
    let transcript = transcript_of(&req);
    assert!(
        transcript.contains(&format!("tool result: {first}")),
        "the tool result whole: {} characters",
        transcript.len()
    );
    assert!(cuts_of(&req).is_empty());
    assert_eq!(h.state("summarizer_input_soft"), "1000", "{:?}", h.stderr);

    let second = "s".repeat(5_000);
    rebuild_after(&mut h, 2, &second);
    let req = h.answer("S2.", "stop");
    let transcript = transcript_of(&req);
    assert!(
        transcript.chars().count() <= 1500,
        "{} characters",
        transcript.chars().count()
    );
    let total = "tool result: ".len() + 5_000;
    assert!(
        transcript.contains(&format!(" of {total} chars shown;")),
        "{transcript}"
    );
    let cuts = cuts_of(&req);
    assert!(
        cuts.iter().any(|c| c["total"] == json!(total)),
        "the cut on the hop: {cuts:?}"
    );
}

/// Before any window is known the catalogue row in `input_soft_fallback`
/// measures the transcript (OR-IG-8); a summarizer that refuses the request
/// as over its window teaches its bound (`input_over_hard`, 1000 tokens),
/// and the next transcript keeps to half of it (OR-IG-4 (4)).
#[test]
fn a_refusal_teaches_the_summarizers_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    assert_eq!(
        cell_config("policy")["params"]["input_soft_fallback"],
        json!(250000),
        "the row the summarizer is born on (OR-IG-9)"
    );
    assert_eq!(
        cell_config("policy")["params"]["input_soft_fallback_row"],
        json!("openai/gpt-6-luna"),
        "a birth token, so the `light` tier"
    );
    let first = "r".repeat(5_000);
    rebuild_after(&mut h, 1, &first);
    let req = h.refuse_window(1000);
    assert!(
        transcript_of(&req).contains(&first),
        "whole under the fallback"
    );
    assert_eq!(h.state("summarizer_input_soft"), "1000", "{:?}", h.stderr);
    assert_eq!(h.rows("SELECT COUNT(*) FROM summaries")[0][0], json!(0));

    rebuild_after(&mut h, 2, &"s".repeat(5_000));
    let req = h.answer("S2.", "stop");
    assert!(
        transcript_of(&req).chars().count() <= 1500,
        "{} characters",
        transcript_of(&req).chars().count()
    );
}

// ============================================== 2. the handover

/// The ledger of a session `s-prev` served last, closed with `note`, and a
/// topic `topic` open.
fn handed_over(note: &str, topic: &str) -> Hive {
    let h = Hive::new();
    let el = json!({"type": "summary", "text": note});
    let hash = sha256_hex(&canonical(&el));
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) VALUES (?1, 'summary', ?2, ?3, 'x')",
        rusqlite::params![hash, note.len() as i64, canonical(&el)],
    )
    .unwrap();
    for (seq, kind, value) in [
        (
            20,
            "handover",
            json!({"state": "prepared", "hash": hash, "from_session": "s-prev"}),
        ),
        (10, "topic", json!({"movement": "start", "name": topic})),
    ] {
        h.db.execute(
            "INSERT INTO marks (seq, session_id, turn_id, kind, value, at, audience_set) \
             VALUES (?1, 's-prev', '', ?2, ?3, '2026-10-07T10:00:00.000000Z', ?4)",
            rusqlite::params![seq, kind, value.to_string(), ROUND_E],
        )
        .unwrap();
    }
    put_state(&h, "handover_for", "s-prev");
    h
}

fn leaf_of(h: &mut Hive) -> String {
    let call = h.curate(
        "s-new",
        "n1",
        0,
        json!([user("Where were we?")]),
        mode("Be brief."),
    );
    call.body["system"]["history"]["handover"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A note over the 3000 characters of before and a topic over the 240 go
/// whole without a window; in a window of 1000 tokens (half: 1500
/// characters) the block keeps to it, the note cut with its length, and the
/// `set` mark names the cut.
#[test]
fn the_handover_block_keeps_to_its_share() {
    if !shipped() {
        return;
    }
    let note = "n".repeat(5_000);
    let topic = format!("the long topic {}", "t".repeat(400));
    let mut h = handed_over(&note, &topic);
    let leaf = leaf_of(&mut h);
    assert!(
        leaf.contains(&note),
        "the note whole: {} characters",
        leaf.len()
    );
    assert!(
        leaf.contains(&topic),
        "the topic whole -- the model's words"
    );

    let mut h = handed_over(&note, &topic);
    put_state(&h, "input_soft", "1000");
    let leaf = leaf_of(&mut h);
    let (_, block) = leaf.split_once('\n').expect("a head line and the block");
    assert!(
        block.chars().count() <= 1500,
        "{} characters",
        block.chars().count()
    );
    assert!(
        block.contains(&topic),
        "the topic ranks before the note: {block}"
    );
    assert!(block.contains(" of 5000 chars shown;"), "{block}");
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'handover' ORDER BY seq DESC LIMIT 1");
    let set: Value = sj::from_str(marks[0][0].as_str().unwrap()).unwrap();
    assert_eq!(set["state"], "set");
    assert!(
        set["cuts"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["what"] == "summary")),
        "the mark names the cut: {set}"
    );
}

/// The carrier: whatever the window, the block a new session starts with
/// fits the one system leaf the llm cell takes (the carrier, 4 MiB
/// serialized, `content_budget::CARRIER_MAX_BYTES`).
#[test]
fn the_handover_block_fits_its_leaf() {
    if !shipped() {
        return;
    }
    // Two bytes a character: 4.4 MB of note, over the leaf.
    let note = "\u{fc}".repeat(2_200_000);
    let mut h = handed_over(&note, "a topic");
    put_state(&h, "input_soft", "1000000");
    let leaf = leaf_of(&mut h);
    let bytes = sj::to_string(&json!({"text": leaf})).unwrap().len();
    assert!(
        bytes <= meclaw_cells::content_budget::CARRIER_MAX_BYTES,
        "{bytes} bytes"
    );
    assert!(leaf.contains("...[cut: "), "the note said cut");
}

/// The session `s1`: three turns of 25000 characters, a tool result of 3000.
fn long_session(h: &mut Hive) -> String {
    let long = |c: &str| c.repeat(25_000);
    let tool = "o".repeat(3_000);
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([
            user(&long("a")),
            tool_call("c1", "look"),
            tool_result("c1", &tool)
        ]),
        mode("Be brief."),
    );
    h.tap(&call, "stop", json!({}), json!([said("ok")]));
    turn(h, "s1", "t2", &long("b"), "ok", json!({}));
    turn(h, "s1", "t3", &long("c"), "ok", json!({}));
    tool
}

fn close(h: &mut Hive) {
    close_session(h, "s1");
}

fn close_session(h: &mut Hive, session: &str) {
    h.summ.clear();
    h.lane(
        "in_close",
        json!({"session_id": session, "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": []}),
    );
}

/// The note's transcript: a session far over the 60000 characters of before,
/// with a tool result over the 2000, goes whole under the catalogue row of
/// `input_soft_fallback` -- and the served window (1000 tokens) never
/// measures it (OR-IG-4); once the summarizer stamped its own window on the
/// note of an earlier session (1000 tokens: 1500 characters) the newest
/// lines that fit, the earlier ones said as left out, the cut on the hop.
#[test]
fn the_note_transcript_keeps_to_the_summarizers_window() {
    if !shipped() {
        return;
    }
    for stamped in [false, true] {
        let mut h = Hive::new();
        put_state(&h, "input_soft", "1000");
        if stamped {
            turn(&mut h, "s0", "e1", "earlier", "ok", json!({}));
            close_session(&mut h, "s0");
            h.answer_with("An earlier note.", "stop", json!({"input_soft": 1000}));
            assert_eq!(h.state("summarizer_input_soft"), "1000", "{:?}", h.stderr);
        }
        let tool = long_session(&mut h);
        close(&mut h);
        let req = h.summ.back().expect("a note request").clone();
        let text = transcript_of(&req);
        if stamped {
            assert!(text.chars().count() <= 1500, "{}", text.chars().count());
            assert!(text.starts_with("...[dropped: "), "{text}");
            assert!(!cuts_of(&req).is_empty(), "{:?}", req.hop);
        } else {
            assert!(text.chars().count() > 75_000, "{}", text.chars().count());
            assert!(text.contains(&format!("tool result: {tool}")));
            assert!(!text.contains("...[dropped:"));
        }
        let prompt = req.body["system"]["instructions"]["text"].as_str().unwrap();
        assert!(
            prompt.contains("Keep it as short as useful and as long as needed."),
            "{prompt}"
        );
    }
}

/// A note the summarizer refuses as over its window (`input_over_hard`, 1000
/// tokens) is measured anew by that bound once -- the next request keeps to
/// half of it -- and a second refusal leaves the session unprepared rather
/// than asking a third time (OR-IG-4 (4)).
#[test]
fn a_refused_note_is_measured_anew_once() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    long_session(&mut h);
    close(&mut h);
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.refuse_window(1000);
    assert_eq!(h.state("summarizer_input_soft"), "1000", "{:?}", h.stderr);
    assert_eq!(h.summ.len(), 1, "asked once more: {:?}", h.stderr);
    let again = h.summ.front().unwrap().clone();
    assert!(
        transcript_of(&again).chars().count() <= 1500,
        "{}",
        transcript_of(&again).chars().count()
    );
    h.refuse_window(1000);
    assert!(h.summ.is_empty(), "not a third time: {:?}", h.summ);
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM marks WHERE kind = 'handover'")[0][0],
        json!(0)
    );
}

// ============================================== 3. the push

/// A find said aloud goes to the voice model, whose window this hive does
/// not know (OR-IG-4): over the 400 characters of before it goes whole.
#[test]
fn a_find_said_aloud_goes_whole() {
    if !shipped() {
        return;
    }
    let script = script_of("push");
    let find = "f".repeat(1_000);
    let whole = pure(&script, "said_aloud('the gap', ARGS)", json!(find));
    assert!(whole.as_str().unwrap().ends_with(&find), "{whole}");
}

/// A gap is the model's words: over the 300 characters of before it stands
/// whole on the record.
#[test]
fn a_gap_is_never_cut() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    let gap = format!("whether {}", "g".repeat(600));
    let call = turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    h.section(&call, "gap", json!({"payload": gap}));
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'gap'");
    assert_eq!(marks.len(), 1, "{:?}", h.stderr);
    let v: Value = sj::from_str(marks[0][0].as_str().unwrap()).unwrap();
    assert_eq!(v["text"], json!(gap));
}

/// Every ask of a producer carries the window as `input_soft` -- a number,
/// beside its alias `recall_input_soft` -- and no key while none is known.
#[test]
fn every_ask_carries_the_window() {
    if !shipped() {
        return;
    }
    let mut h = talky();
    let ask = |h: &mut Hive, turn: &str| {
        h.out.clear();
        h.lane(
            "in_recall_ask",
            json!({"session_id": "s1", "channel": "test", "audience_set": ROUND_E}),
            json!({"phase": "recall", "turn_id": turn, "session_id": "s1", "iter": "0",
                   "recall_query": "Who is my son?", "memory_tier": "1",
                   "recall_window_from": "", "recall_window_to": ""}),
            json!({"messages": [user("Who is my son?")]}),
        );
        let asks = h.routed("recall");
        assert_eq!(asks.len(), 1, "{:?}", h.stderr);
        asks[0].clone()
    };
    let first = ask(&mut h, "t1");
    assert!(first.hop.get("input_soft").is_none(), "{:?}", first.hop);
    turn(
        &mut h,
        "s1",
        "t1",
        "Who is my son?",
        "Ben.",
        json!({"input_soft": 250000}),
    );
    let next = ask(&mut h, "t2");
    assert_eq!(
        next.hop["input_soft"],
        json!(250000),
        "the row's input_soft, no role's quality_cap"
    );
    assert_eq!(next.hop["recall_input_soft"], json!("250000"));
}

// ============================================== 4. the intake

fn pin(h: &mut Hive, text: &str) {
    h.out.clear();
    h.stderr.clear();
    h.lane(
        "in_pin",
        json!({"audience_set": ROUND_E}),
        json!({}),
        json!({"pins": [{"text": text, "source": "probe"}]}),
    );
}

/// A pin over the 16000 characters of before -- and over the 65536 bytes of
/// the leaf before (GH #1085) -- is kept; one over the carrier -- one system
/// leaf, 4 MiB -- is refused with its size and the bound.
#[test]
fn a_pin_is_refused_only_over_its_leaf() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    pin(&mut h, &"p".repeat(70_000));
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM pins")[0][0],
        json!(1),
        "{:?}",
        h.stderr
    );
    let over = "q".repeat(meclaw_cells::content_budget::CARRIER_MAX_BYTES);
    pin(&mut h, &over);
    assert_eq!(h.rows("SELECT COUNT(*) FROM pins")[0][0], json!(1));
    let size = sj::to_string(&json!({"text": over})).unwrap().len();
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains(&format!("too_long: {size} > 4194304 bytes"))),
        "{:?}",
        h.stderr
    );
}

/// A push candidate over the 600 characters of before is kept: the push says
/// it within its share and says what it left out.
#[test]
fn a_long_candidate_is_kept() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.out.clear();
    h.lane(
        "in_candidate",
        json!({"audience_set": ROUND_E}),
        json!({"ack": "1"}),
        json!({"source": "probe", "candidates": [{"id": "c1", "text": "c".repeat(2_000)}]}),
    );
    let acks = h.routed("candidate_ack");
    assert_eq!(acks.len(), 1, "{:?}", h.out);
    assert_eq!(acks[0].body["stored"], json!(1), "{:?}", acks[0].body);
}

// ============================================== 5. the collector and the carrier

/// The collector writes every offered section, far over the 6000 characters
/// it capped at until 5.1.0: what fits is the curator's decision.
#[test]
fn the_collector_writes_the_contract_whole() {
    if !shipped() {
        return;
    }
    let cfg = read_json(&repo("templates/collector/assemble/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("script")
        .to_string();
    let offers: Vec<Value> = (0..10)
        .map(|i| {
            json!({"section": format!("s{i}"), "required": i == 0,
                   "instruction": "i".repeat(1_000),
                   "schema": {"type": "string"}})
        })
        .collect();
    let got = pure(&script, "sidecar_block(ARGS)", json!(offers));
    let text = got[0].as_str().unwrap();
    assert!(text.len() > 10_000, "{}", text.len());
    assert_eq!(got[1].as_array().map(Vec::len), Some(10), "{}", got[1]);
}

/// The carrier bound of a pin, a summary and the handover block is the llm
/// cell's leaf bound without a window, the carrier ceiling (R-IG-2): the one
/// number they mirror. Since GH #1085 the gate has no fixed leaf number of its
/// own (`system_gate::leaf_bound`: the param, else the window, else the carrier).
#[test]
fn the_leaf_bound_is_the_llm_cells() {
    if !shipped() {
        return;
    }
    let n = meclaw_cells::content_budget::CARRIER_MAX_BYTES;
    let params = std::fs::read_to_string(repo("crates/meclaw-cells/src/llm/params.rs"))
        .expect("the llm params");
    let bound = params
        .split("pub fn window_bound_bytes")
        .nth(1)
        .expect("window_bound_bytes");
    assert!(
        bound[..bound.find("\n    }\n").unwrap_or(bound.len())]
            .contains("crate::content_budget::CARRIER_MAX_BYTES"),
        "the gate falls back to the carrier ceiling without a window"
    );
    for cell in ["handover", "intake", "policy"] {
        assert!(
            script_of(cell).contains(&format!("LEAF_MAX_BYTES = {n}\n")),
            "{cell} mirrors {n}"
        );
    }
}

/// Fix review G1 M5: the curator learns its summarizer's window from the
/// refusal the `llm` cell writes (`window::apply`, `Refusal::meta`). The
/// curator harness builds that refusal by hand, so a renamed key would leave
/// every curator lock green while the curator stops learning. Held here at
/// the source: each word of the refusal stands in `llm/window.rs` (and the
/// error code at a `window::apply` call in `llm/cell.rs`), and each stands
/// in the two scripts that read it, policy and handover.
#[test]
fn the_curator_reads_the_refusal_the_llm_cell_writes() {
    let window = std::fs::read_to_string(repo("crates/meclaw-cells/src/llm/window.rs"))
        .expect("llm/window.rs");
    let cell =
        std::fs::read_to_string(repo("crates/meclaw-cells/src/llm/cell.rs")).expect("llm/cell.rs");
    let at_apply = cell.split("window::apply(").skip(1).any(|after| {
        after
            .chars()
            .take(1200)
            .collect::<String>()
            .contains("\"invalid_input\"")
    });
    assert!(at_apply, "a window refusal leaves as `invalid_input`");
    for word in ["input_over_hard", "input_over_window"] {
        assert!(
            window.contains(&format!("kind: \"{word}\"")),
            "window.rs names `{word}`"
        );
    }
    for word in ["input_hard", "context_window"] {
        assert!(
            window.contains(&format!("bound_key: \"{word}\"")),
            "window.rs names `{word}`"
        );
    }
    for cell_name in ["policy", "handover"] {
        let script = read_json(&repo(&format!("templates/curator/{cell_name}/config.json")))
            ["params"]["script_inline"]
            .as_str()
            .expect("script")
            .to_string();
        for word in [
            "invalid_input",
            "input_over_hard",
            "input_over_window",
            "input_hard",
            "context_window",
        ] {
            assert!(
                script.contains(&format!("\"{word}\"")),
                "curator/{cell_name} reads `{word}` of the llm cell's refusal"
            );
        }
    }
}
