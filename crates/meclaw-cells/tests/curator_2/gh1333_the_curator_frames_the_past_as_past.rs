//! GH #1333 -- the curator frames the past as past: the handover note reports
//! the last session and never orders the next one, and the ambient recall of a
//! round stands before the person's turn, framed as memory.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

#[allow(unused_imports)]
use curator_hive::*;

mod handover {
    //! GH #1333 -- the handover note reports the last session and never gives
    //! the next one orders.
    //! Measured in a lab run: under a prompt that asked for "what the next
    //! session has to know to carry on" and "what was promised and is still
    //! open", and a title that framed the note as "The last session,
    //! summarised:", the summarizer wrote "Urgent: ... For the next session:
    //! ..." with emergency numbers, although the prompt said "Report, do not
    //! judge". The cause is the framing, so the framing is pinned here: the
    //! prompt asks for a report in the third person and the past tense, keeps an
    //! open point as the fact of who promised what to whom, and forbids
    //! addressing the next session; the title the next session reads frames the
    //! note as a report, not as instructions. No model is called: the two
    //! constants are read from the shipped script with `ast`.
    //! Filter: `binary_id(meclaw-cells::curator_2) & test(/^gh1333/)`.
    use super::curator_hive::{read_json, repo, run_python};
    use meclaw_core::serde_json::{self as sj, Value, json};

    const HANDOVER: &str = "templates/curator/handover/config.json";

    /// R2b / GH #49: a tree without the template skips.
    fn shipped() -> bool {
        repo(HANDOVER).is_file()
    }

    /// Only the two literal constants of the shipped script, nothing else of it
    /// runs.
    const AST_LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
scope = {}
for n in ast.parse(inp["src"]).body:
    if isinstance(n, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id in ("HANDOVER_PROMPT", "NOTE_TITLE")
            for t in n.targets):
        exec(compile(ast.Module(body=[n], type_ignores=[]), "handover", "exec"), scope)
print(json.dumps({"prompt": scope.get("HANDOVER_PROMPT"), "title": scope.get("NOTE_TITLE")}))
"#;

    /// `(HANDOVER_PROMPT, NOTE_TITLE)` of the shipped script, lower-cased.
    fn framing() -> (String, String) {
        let cfg = read_json(&repo(HANDOVER));
        let src = cfg["params"]["script_inline"]
            .as_str()
            .expect("script_inline");
        let out = run_python(AST_LOADER, &json!({ "src": src }).to_string());
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "the constants do not load: {err}");
        let got: Value = sj::from_slice(&out.stdout).expect("JSON");
        let prompt = got["prompt"].as_str().expect("HANDOVER_PROMPT");
        let title = got["title"].as_str().expect("NOTE_TITLE");
        (prompt.to_lowercase(), title.to_lowercase())
    }

    #[test]
    fn the_prompt_asks_for_a_report_of_the_past_session() {
        if !shipped() {
            return;
        }
        let (prompt, _) = framing();
        assert!(
            prompt.contains("third person") && prompt.contains("past tense"),
            "the note must report in the third person and the past tense: {prompt}"
        );
        assert!(
            !prompt.contains("has to know to carry on"),
            "the prompt still writes the note for the next session to act on: {prompt}"
        );
    }

    #[test]
    fn an_open_point_is_a_fact_of_who_promised_what() {
        if !shipped() {
            return;
        }
        let (prompt, _) = framing();
        assert!(
            prompt.contains("who promised what to whom"),
            "an open point must stand as the fact of a promise: {prompt}"
        );
        assert!(
            prompt.contains("never a task"),
            "an open point must never become a task: {prompt}"
        );
        assert!(
            !prompt.contains("is still open"),
            "the prompt still asks for open items as such: {prompt}"
        );
    }

    #[test]
    fn the_note_never_addresses_the_next_session() {
        if !shipped() {
            return;
        }
        let (prompt, _) = framing();
        assert!(
            prompt.contains("never address the next session"),
            "the prompt must forbid addressing the next session: {prompt}"
        );
        for kind in ["no instructions", "no urgency", "no advice"] {
            assert!(
                prompt.contains(kind),
                "the prompt must say {kind:?}: {prompt}"
            );
        }
    }

    /// GH #1335: in a lab run the note still judged -- it listed what had not
    /// happened in the session and asked, as open, how the person was doing
    /// now -- under a prompt that asked "whether it was done". The note reports
    /// what happened and what was said; "open" is a promise not kept yet, never
    /// a question about the person.
    #[test]
    fn gh1335_the_note_reports_only_what_happened_and_was_said() {
        if !shipped() {
            return;
        }
        let (prompt, _) = framing();
        for pin in [
            "report only what happened and what was said",
            "never list what did not happen or was not said",
            "never assess the person's state",
            "no open questions",
            "open is only a promise not kept yet",
        ] {
            assert!(
                prompt.contains(pin),
                "the prompt must say {pin:?}: {prompt}"
            );
        }
        assert!(
            !prompt.contains("whether it was done"),
            "the prompt still asks for an assessment of what was done: {prompt}"
        );
    }

    #[test]
    fn the_title_frames_the_note_as_a_report() {
        if !shipped() {
            return;
        }
        let (_, title) = framing();
        assert!(
            title.contains("report") && title.contains("not instructions"),
            "the next session must read the note as a report, not as instructions: {title}"
        );
    }
}

mod recall {
    //! GH #1333 (second way) -- the ambient recall of a round is memory, and the
    //! person's turn stands last.
    //! The collector hangs its ambient evidence (the `memory_recall` pair, GH #278,
    //! and the `affinity_brief` pair, GH #834) behind the person's words of the
    //! round. Rendered in wall order, the recall's result was the LAST message
    //! before the answer: in a lab turn a crisis sentence from an earlier session,
    //! quoted verbatim in the recall, stood where the newest turn belongs, and the
    //! model answered it instead of the cake the person had just mentioned.
    //! Nothing is hidden: the pair stays whole in the window. It moves in front of
    //! the person's turn of its round, and its result opens with one fixed frame
    //! that names it as recalled memory -- the same frame whatever the bundle
    //! says, so the framing is structure, never a reading of the words. A call the
    //! model made itself (an id the collector did not derive) stays where it was.
    use super::*;

    use meclaw_core::serde_json::{Value, json};

    pub(super) const RECALL_ID: &str = "call_recall_0123456789abcdef";
    /// The head of the notes block (GH #1303), from a care note.
    pub(super) const NOTES_OPEN: &str = "[notes for this turn -- from care;";
    const BRIEF_ID: &str = "call_brief_0123456789abcdef";

    pub(super) fn kind(m: &Value) -> String {
        format!(
            "{}/{}",
            m["origin"].as_str().unwrap_or(""),
            m["type"].as_str().unwrap_or("")
        )
    }

    /// The text of `m` without the short id the window puts in front of it.
    pub(super) fn bare(m: &Value) -> String {
        let t = m["text"].as_str().unwrap_or("");
        match t.strip_prefix("[#") {
            Some(rest) => rest.split_once("] ").map(|x| x.1).unwrap_or(t).to_string(),
            None => t.to_string(),
        }
    }

    pub(super) fn ambient_round(cake: &str, said_before: &str) -> Value {
        json!([
            user(cake),
            tool_call(RECALL_ID, "memory_recall"),
            tool_result(
                RECALL_ID,
                &format!("WHAT WAS SAID\n  user: \"{said_before}\"")
            ),
            tool_call(BRIEF_ID, "affinity_brief"),
            tool_result(BRIEF_ID, "who: the person"),
        ])
    }

    #[test]
    fn gh1333_the_persons_turn_is_the_last_message_before_the_answer() {
        if !shipped() {
            return;
        }
        let mut h = Hive::new();
        turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
        let call = h.curate(
            "s1",
            "t2",
            0,
            ambient_round("I baked a plum cake.", "I want to end it tonight."),
            mode("Be brief."),
        );
        let msgs = call.messages();
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        assert!(
            last["text"]
                .as_str()
                .unwrap_or("")
                .ends_with("I baked a plum cake."),
            "the last message is the person's turn of this round: {msgs:#?}"
        );
        // Nothing is hidden: the recall and the brief are both in the window,
        // each a whole pair, before the turn.
        let at = |id: &str, ty: &str| {
            msgs.iter()
                .position(|m| m["id"] == id && m["type"] == ty)
                .unwrap_or_else(|| panic!("{id} {ty} in the window: {msgs:#?}"))
        };
        let turn_at = msgs.len() - 1;
        for id in [RECALL_ID, BRIEF_ID] {
            let (c, r) = (at(id, "tool_call"), at(id, "tool_result"));
            assert!(
                c + 1 == r && r < turn_at,
                "{id}: a whole pair before the turn"
            );
        }
        assert!(at(RECALL_ID, "tool_result") < at(BRIEF_ID, "tool_call"));
        // The earlier round keeps its order.
        let hello = msgs.iter().position(|m| bare(m) == "hello").expect("hello");
        let hi = msgs.iter().position(|m| bare(m) == "hi").expect("hi");
        assert!(hello < hi && hi < at(RECALL_ID, "tool_call"));
    }

    #[test]
    fn gh1333_the_recall_is_framed_as_memory_whatever_it_holds() {
        if !shipped() {
            return;
        }
        let mut frames = Vec::new();
        for (cake, before) in [
            ("I baked a plum cake.", "I want to end it tonight."),
            ("The weather is fine.", "The doctor calls at noon."),
        ] {
            let mut h = Hive::new();
            turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
            let call = h.curate(
                "s1",
                "t2",
                0,
                ambient_round(cake, before),
                mode("Be brief."),
            );
            let msgs = call.messages();
            let result = msgs
                .iter()
                .find(|m| m["id"] == RECALL_ID && m["type"] == "tool_result")
                .expect("the recall result");
            let text = bare(result);
            let raw = format!("WHAT WAS SAID\n  user: \"{before}\"");
            assert!(
                text.ends_with(&raw) && text.len() > raw.len(),
                "the bundle stays whole, behind a frame: {text:?}"
            );
            frames.push(text[..text.len() - raw.len()].to_string());
        }
        assert_eq!(frames[0], frames[1], "one frame, whatever the bundle says");
        assert!(!frames[0].trim().is_empty());
    }

    #[test]
    fn gh1333_a_recall_the_model_called_stays_behind_the_turn() {
        if !shipped() {
            return;
        }
        let mut h = Hive::new();
        turn(&mut h, "s1", "t1", "hello", "hi", json!({}));
        let call = h.curate(
            "s1",
            "t2",
            0,
            json!([
                user("what did I say?"),
                tool_call("toolu_own", "memory_recall"),
                tool_result("toolu_own", "WHAT WAS SAID\n  user: \"hello\""),
            ]),
            mode("Be brief."),
        );
        let msgs = call.messages();
        let q = msgs
            .iter()
            .position(|m| bare(m) == "what did I say?")
            .expect("the turn");
        let c = msgs
            .iter()
            .position(|m| m["id"] == "toolu_own" && m["type"] == "tool_call")
            .expect("the call");
        assert!(
            q < c,
            "the model's own call answers the turn, after it: {msgs:#?}"
        );
    }

    /// The two refusals of the Anthropic Messages API a window can earn, checked
    /// as strictly as the provider does (GH #1283: no model call in a test): the
    /// request opens with the person's words, and every tool call stands right
    /// before its result.
    pub(super) fn strict_anthropic_shape(msgs: &[Value]) -> Result<(), String> {
        let first = msgs.first().ok_or("an empty request")?;
        if first["origin"] != "user" {
            return Err(format!(
                "messages: first message must use the \"user\" role: {first}"
            ));
        }
        for (i, m) in msgs.iter().enumerate() {
            let next = msgs.get(i + 1);
            if m["type"] == "tool_call"
                && !next.is_some_and(|n| n["type"] == "tool_result" && n["id"] == m["id"])
            {
                return Err(format!(
                    "tool_use ids were found without tool_result blocks: {m}"
                ));
            }
            if m["type"] == "tool_result" && (i == 0 || msgs[i - 1]["id"] != m["id"]) {
                return Err(format!("unexpected tool_use_id in tool_result blocks: {m}"));
            }
        }
        Ok(())
    }

    #[test]
    fn gh1333_a_recall_that_would_open_the_window_goes_into_the_turn() {
        if !shipped() {
            return;
        }
        let mut h = Hive::new();
        let raw = "WHAT WAS SAID\n  user: \"I want to end it tonight.\"";
        let call = h.curate(
            "s1",
            "t1",
            0,
            ambient_round("I baked a plum cake.", "I want to end it tonight."),
            mode("Be brief."),
        );
        let msgs = call.messages();
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        let text = last["text"].as_str().unwrap_or("");
        assert!(
            text.ends_with("I baked a plum cake."),
            "the person's words close it: {text:?}"
        );
        // Nothing is hidden: the recall and the brief ride in the turn, framed,
        // the recall with the frame it carries as a pair.
        let rec = text
            .find(raw)
            .unwrap_or_else(|| panic!("the recall in the turn: {text:?}"));
        let brief = text.find("who: the person").expect("the brief in the turn");
        assert!(rec < brief && brief < text.len() - "I baked a plum cake.".len());
        // Review M-1: the brief's frame names its tool, read from the call's
        // JSON text -- not the fallback `context`.
        let brief_frame = &text[rec + raw.len()..brief];
        assert!(
            brief_frame.contains("affinity_brief") && !brief_frame.contains("context"),
            "the brief framed by its name: {brief_frame:?}"
        );
        let mut h2 = Hive::new();
        turn(&mut h2, "s1", "t1", "hello", "hi", json!({}));
        let paired = h2.curate(
            "s1",
            "t2",
            0,
            ambient_round("x", "I want to end it tonight."),
            mode("Be brief."),
        );
        let pm = paired.messages();
        strict_anthropic_shape(&pm).unwrap_or_else(|e| panic!("{e}: {pm:#?}"));
        let result = pm
            .iter()
            .find(|m| m["id"] == RECALL_ID && m["type"] == "tool_result")
            .expect("the recall pair");
        let framed = bare(result);
        let frame = &framed[..framed.len() - raw.len()];
        assert!(
            text[..rec].ends_with(frame),
            "the same frame in the turn: {text:?}"
        );
    }

    /// GH #1335 (the notes contract, curator README § Notes): the notes block
    /// is ONE block that opens the newest message of the person. With the
    /// recall folded into the turn (the pair would open the window), the
    /// framed recall follows the block, never stands in front of it.
    #[test]
    fn gh1335_the_notes_block_opens_the_turn_before_a_folded_recall() {
        if !shipped() {
            return;
        }
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        h.lane(
            "in_note",
            json!({"session_id": "s1"}),
            json!({}),
            json!({"notes": [{"id": "n1", "source": "care", "text": "Stay with the topic.",
                              "scope": "session"}]}),
        );
        let call = h.curate(
            "s1",
            "t1",
            0,
            ambient_round("I baked a plum cake.", "I want to end it tonight."),
            mode("Be brief."),
        );
        let msgs = call.messages();
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        let text = last["text"].as_str().unwrap_or("");
        assert!(
            text.starts_with(NOTES_OPEN),
            "the notes block opens the person's newest message: {text:?}"
        );
        let note = text.find("Stay with the topic.").expect("the note");
        let rec = text
            .find("[memory, from before this turn -- not said now]")
            .unwrap_or_else(|| panic!("the folded recall: {text:?}"));
        assert!(note < rec, "the recall follows the notes block: {text:?}");
        assert!(text.ends_with("I baked a plum cake."), "{text:?}");
    }

    /// A package with a hard line of `hard` tokens (as GH #1225's test).
    fn package(soft: i64, hard: i64, tokens_prompt: i64) -> Value {
        json!({"input_soft": soft, "input_hard": hard, "cost_in": 10.0, "cost_cached_in": 0.0,
               "cache_expires_at": "2099-01-01T00:00:00Z", "tokens_prompt": tokens_prompt})
    }

    /// Review I-1: the guard at `input_hard` (GH #1225) cuts whole old rounds.
    /// The pairs move after the cut, so the round that ends up in front still
    /// opens the request with the person's words, never with a tool call.
    #[test]
    fn gh1333_a_wall_over_the_hard_line_still_opens_with_the_person() {
        over_the_hard_line("s");
    }

    /// GH #1335 review M-5: the same cut when the wall is an earlier session
    /// -- the boundary lands on what is sent, after the cut.
    #[test]
    fn gh1335_a_cut_wall_of_an_earlier_session_still_opens_with_the_person() {
        over_the_hard_line("s2");
    }

    /// Twenty rounds of session `s` over the hard line, then a turn of
    /// session `now`.
    fn over_the_hard_line(now: &str) {
        if !shipped() {
            return;
        }
        let mut h = Hive::with(&[
            ("policy", "role", json!("talky")),
            ("policy", "keep_recent", json!(2)),
        ]);
        // The model's hard line: 4 000 tokens, the guard's margin 3 600.
        turn(&mut h, "s", "t0", "hello", "hi", package(2000, 4000, 100));
        // About 16 000 tokens of wall, an ambient pair in every round.
        let base = chrono::Utc::now() - chrono::Duration::seconds(50);
        for i in 0..20i64 {
            let at = |ms: i64| base + chrono::Duration::milliseconds(100 * i + ms);
            let r = format!("r{i}");
            let text = format!("{i} {}", "x".repeat(1200));
            let rid = format!("call_recall_{i:016x}");
            let bid = format!("call_brief_{i:016x}");
            let rows = [
                (0, "user", user(&text), 0),
                (1, "recall", tool_call(&rid, "memory_recall"), 0),
                (2, "recall", tool_result(&rid, "WHAT WAS SAID"), 0),
                (3, "brief", tool_call(&bid, "affinity_brief"), 0),
                (4, "brief", tool_result(&bid, "who: the person"), 0),
                (10, "assistant", said(&text), 1),
            ];
            for (ms, k, el, fin) in rows {
                h.row(at(ms), "s", &r, k, &el, fin);
            }
        }
        let call = h.curate(
            now,
            "t1",
            0,
            ambient_round("I baked a plum cake.", "hello"),
            mode("Be brief."),
        );
        let cut = call.hop["cuts"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c["what"] == "overflow_cut")
            .count();
        assert_eq!(cut, 1, "the guard cut: {:?} {:?}", call.hop, h.stderr);
        let msgs = call.messages();
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        if now != "s" {
            let t = last["text"].as_str().unwrap_or("");
            assert!(
                t.starts_with("[this session, since ") && t.ends_with("I baked a plum cake."),
                "the boundary opens the turn after the cut: {t:?}"
            );
        }
    }

    /// Review I-2: the quick way leaves recall and brief out (GH #1247 T4-08),
    /// also when the round is the only one and its pair would open the window.
    #[test]
    fn gh1333_the_quick_way_never_reads_the_recall() {
        if !shipped() {
            return;
        }
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                         "path": "simple", "families": "web"});
        let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "simple"});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": ambient_round("I baked a plum cake.", "I want to end it tonight."),
                   "system": mode("Be brief.")}),
        );
        let quick = h.routed("quick");
        assert_eq!(
            quick.len(),
            1,
            "one call on quick: {:?} {:?}",
            h.out,
            h.stderr
        );
        let msgs = quick[0].messages();
        assert!(
            msgs.iter()
                .any(|m| bare(m).ends_with("I baked a plum cake.")),
            "the turn is there: {msgs:#?}"
        );
        for m in &msgs {
            let t = m["text"].as_str().unwrap_or("");
            assert!(
                !t.contains("WHAT WAS SAID") && !t.contains("who: the person"),
                "no recall or brief on the quick way: {msgs:#?}"
            );
        }
    }
}

mod session {
    //! GH #1335 -- an earlier session stands in the window as past, behind a
    //! visible session boundary.
    //! The window reads the wall over every session (OR-KX-K4); a handover sets
    //! its note but cuts nothing, and only a rebuild covers older rows. In a lab
    //! turn of a new session 75 s after the last one, every line of that session
    //! stood raw and seamless before the new turn -- a crisis sentence among
    //! them -- and the model answered it as said "just now". Nothing is hidden:
    //! the lines stay. Every person line of an earlier session opens with one
    //! frame naming the session's start as past; the first person line of the
    //! running session opens with the boundary. The frame is structure (the
    //! rows' session id and time), the same whatever the words say.
    use super::recall::{NOTES_OPEN, RECALL_ID, ambient_round, bare, kind, strict_anthropic_shape};
    use super::*;

    use meclaw_core::serde_json::{Value, json};

    const DOCTOR: &str = "The doctor calls at noon.";
    const CRISIS: &str = "I have the pills in front of me and want to end it tonight.";
    const CAKE: &str = "I baked a plum cake.";

    /// The k1 case: an earlier session `s1` (two person lines, one recall pair,
    /// two answers) written straight into the wall at `base`, then the first
    /// turn of a new session `s2` whose recall quotes the crisis line.
    fn earlier_session_then_a_new_turn(
        note: Option<&str>,
    ) -> (Vec<Value>, chrono::DateTime<chrono::Utc>) {
        clear_of_midnight();
        let mut h = match note {
            // The role whose preset gives notes their share (GH #1303).
            Some(_) => Hive::with(&[("policy", "role", json!("talky"))]),
            None => Hive::new(),
        };
        let base = backdated(50);
        let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
        let rid = "call_recall_00000000000000aa";
        let rows = [
            (0, "t1", "user", user(DOCTOR), 0),
            (10, "t1", "assistant", said("I am with you."), 1),
            (20, "t2", "user", user(CRISIS), 0),
            (21, "t2", "recall", tool_call(rid, "memory_recall"), 0),
            (
                22,
                "t2",
                "recall",
                tool_result(rid, "WHAT WAS SAID\n  user: \"I am afraid.\""),
                0,
            ),
            (30, "t2", "assistant", said("I am here and I stay."), 1),
        ];
        for (ms, t, k, el, fin) in rows {
            h.row(at(ms), "s1", t, k, &el, fin);
        }
        if let Some(text) = note {
            h.lane(
                "in_note",
                json!({"session_id": "s2"}),
                json!({}),
                json!({"notes": [{"id": "n1", "source": "care", "text": text, "scope": "session"}]}),
            );
        }
        let call = h.curate(
            "s2",
            "t1",
            0,
            ambient_round(CAKE, CRISIS),
            mode("Be brief."),
        );
        (call.messages(), base)
    }

    #[test]
    fn gh1335_an_earlier_session_stands_framed_behind_the_boundary() {
        if !shipped() {
            return;
        }
        let (msgs, base) = earlier_session_then_a_new_turn(None);
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        let turn = last["text"].as_str().unwrap_or("");
        assert!(
            turn.ends_with(CAKE),
            "the person's words close it: {turn:?}"
        );
        assert!(
            turn.starts_with("[this session, since "),
            "the running session opens with the boundary: {turn:?}"
        );
        let frame = format!(
            "[earlier session, since {} -- from before this session, not said now]\n",
            base.format("%Y-%m-%d %H:%M UTC")
        );
        // Nothing is hidden, and every person line of s1 is framed as past.
        for line in [DOCTOR, CRISIS] {
            let m = msgs
                .iter()
                .find(|m| {
                    kind(m) == "user/text" && m["text"].as_str().unwrap_or("").ends_with(line)
                })
                .unwrap_or_else(|| panic!("{line:?} stays in the window: {msgs:#?}"));
            let t = m["text"].as_str().unwrap_or("");
            assert!(
                t.starts_with(&frame),
                "{line:?} framed as an earlier session: {t:?}"
            );
        }
        for answer in ["I am with you.", "I am here and I stay."] {
            assert!(
                msgs.iter().any(|m| bare(m) == answer),
                "{answer:?} stays: {msgs:#?}"
            );
        }
        // The recall of this turn keeps its own frame (GH #1333).
        let rec = msgs
            .iter()
            .find(|m| m["id"] == RECALL_ID && m["type"] == "tool_result")
            .expect("the recall of this turn");
        assert!(
            rec["text"]
                .as_str()
                .unwrap_or("")
                .contains("[memory, from before this turn -- not said now]"),
            "the recall stays framed as memory: {rec:#?}"
        );
    }

    /// The notes block opens the newest message of the person (curator
    /// README, Notes): the session boundary follows it, never stands in
    /// front of it.
    #[test]
    fn gh1335_the_notes_block_still_opens_the_turn_before_the_boundary() {
        if !shipped() {
            return;
        }
        let (msgs, _) = earlier_session_then_a_new_turn(Some("Look in on the person once."));
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        let turn = last["text"].as_str().unwrap_or("");
        assert!(
            turn.starts_with(NOTES_OPEN),
            "the notes block opens the person's newest message: {turn:?}"
        );
        let note = turn.find("Look in on the person once.").expect("the note");
        let bound = turn.find("[this session, since ").expect("the boundary");
        assert!(
            note < bound,
            "the boundary follows the notes block: {turn:?}"
        );
        assert!(
            turn.ends_with(CAKE),
            "the person's words close it: {turn:?}"
        );
    }

    /// Review I-1: session-keeper holds one open session per channel, so a
    /// session of another channel runs beside the current one. A session
    /// whose newest row lies before the start of the running session is
    /// earlier and framed as past; one that overlaps it gets a neutral frame
    /// -- never "from before this session" for what was said in between.
    #[test]
    fn gh1335_an_overlapping_session_is_never_framed_as_before_this_one() {
        if !shipped() {
            return;
        }
        clear_of_midnight();
        let mut h = Hive::new();
        let base = backdated(50);
        let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
        const OLD: &str = "Yesterday I was in the garden.";
        let rows = [
            // s0 ends before s2 starts: earlier.
            (0, "s0", "t1", "user", user(OLD), 0),
            (5, "s0", "t1", "assistant", said("That sounds nice."), 1),
            // s1 starts before s2 and goes on after its start: overlapping.
            (10, "s1", "t1", "user", user(DOCTOR), 0),
            (15, "s1", "t1", "assistant", said("I am with you."), 1),
            (20, "s2", "t1", "user", user("hello"), 0),
            (25, "s2", "t1", "assistant", said("hi"), 1),
            (30, "s1", "t2", "user", user(CRISIS), 0),
            (
                35,
                "s1",
                "t2",
                "assistant",
                said("I am here and I stay."),
                1,
            ),
        ];
        for (ms, sid, t, k, el, fin) in rows {
            h.row(at(ms), sid, t, k, &el, fin);
        }
        let call = h.curate(
            "s2",
            "t2",
            0,
            ambient_round(CAKE, "hello"),
            mode("Be brief."),
        );
        let msgs = call.messages();
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        assert!(
            last["text"].as_str().unwrap_or("").ends_with(CAKE),
            "the person's words close it: {last:#?}"
        );
        let since = base.format("%Y-%m-%d %H:%M UTC").to_string();
        let text_of = |line: &str| -> String {
            msgs.iter()
                .find(|m| {
                    kind(m) == "user/text" && m["text"].as_str().unwrap_or("").ends_with(line)
                })
                .unwrap_or_else(|| panic!("{line:?} stays in the window: {msgs:#?}"))["text"]
                .as_str()
                .unwrap_or("")
                .to_string()
        };
        let old = text_of(OLD);
        assert!(
            old.starts_with(&format!(
                "[earlier session, since {since} -- from before this session, not said now]\n"
            )),
            "s0 ended before s2: framed as past: {old:?}"
        );
        let hello = text_of("hello");
        assert!(
            hello.starts_with("[this session, since "),
            "the running session opens with the boundary: {hello:?}"
        );
        for line in [DOCTOR, CRISIS] {
            let t = text_of(line);
            assert!(
                t.starts_with(&format!("[other session, since {since}]\n")),
                "{line:?} of the overlapping s1 carries the neutral frame: {t:?}"
            );
        }
        for m in &msgs {
            let t = m["text"].as_str().unwrap_or("");
            if t.ends_with(DOCTOR) || t.ends_with(CRISIS) {
                assert!(!t.contains("not said now"), "{t:?}");
            }
        }
    }

    /// Review M-5: the quick way (GH #1247) with an earlier session in the
    /// wall -- the boundary lands on what quick sends, the turn stays last.
    #[test]
    fn gh1335_the_quick_way_keeps_the_boundary_and_the_turn_last() {
        if !shipped() {
            return;
        }
        clear_of_midnight();
        let mut h = Hive::with(&[("policy", "role", json!("talky"))]);
        let base = backdated(50);
        let at = |ms: i64| base + chrono::Duration::milliseconds(ms);
        h.row(at(0), "s1", "t1", "user", &user(DOCTOR), 0);
        h.row(at(10), "s1", "t1", "assistant", &said("I am with you."), 1);
        let hop = json!({"session_id": "s2", "turn_id": "t1", "iter": "0", "phase": "",
                         "path": "simple", "families": "web"});
        let ctx = json!({"session_id": "s2", "turn_id": "t1", "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "simple"});
        h.lane(
            "in_curate",
            ctx,
            hop,
            json!({"messages": ambient_round(CAKE, DOCTOR), "system": mode("Be brief.")}),
        );
        let quick = h.routed("quick");
        assert_eq!(
            quick.len(),
            1,
            "one call on quick: {:?} {:?}",
            h.out,
            h.stderr
        );
        let msgs = quick[0].messages();
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        let last = msgs.last().expect("a window");
        assert_eq!(kind(last), "user/text", "the turn stands last: {msgs:#?}");
        let t = last["text"].as_str().unwrap_or("");
        assert!(
            t.starts_with("[this session, since ") && t.ends_with(CAKE),
            "the boundary opens the turn: {t:?}"
        );
        assert!(
            msgs.iter().any(|m| {
                let t = m["text"].as_str().unwrap_or("");
                t.starts_with("[earlier session, since ") && t.ends_with(DOCTOR)
            }),
            "the earlier line stays, framed as past: {msgs:#?}"
        );
    }

    #[test]
    fn gh1335_one_session_carries_no_session_frame() {
        if !shipped() {
            return;
        }
        let mut h = Hive::new();
        turn(&mut h, "s1", "t1", DOCTOR, "I am with you.", json!({}));
        let call = h.curate(
            "s1",
            "t2",
            0,
            ambient_round(CAKE, DOCTOR),
            mode("Be brief."),
        );
        let msgs = call.messages();
        strict_anthropic_shape(&msgs).unwrap_or_else(|e| panic!("{e}: {msgs:#?}"));
        for m in &msgs {
            let t = m["text"].as_str().unwrap_or("");
            assert!(
                !t.contains("[earlier session") && !t.contains("[this session"),
                "a window of one session needs no boundary: {msgs:#?}"
            );
        }
    }
}
