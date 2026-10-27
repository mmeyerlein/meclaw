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

    const RECALL_ID: &str = "call_recall_0123456789abcdef";
    const BRIEF_ID: &str = "call_brief_0123456789abcdef";

    fn kind(m: &Value) -> String {
        format!(
            "{}/{}",
            m["origin"].as_str().unwrap_or(""),
            m["type"].as_str().unwrap_or("")
        )
    }

    /// The text of `m` without the short id the window puts in front of it.
    fn bare(m: &Value) -> String {
        let t = m["text"].as_str().unwrap_or("");
        match t.strip_prefix("[#") {
            Some(rest) => rest.split_once("] ").map(|x| x.1).unwrap_or(t).to_string(),
            None => t.to_string(),
        }
    }

    fn ambient_round(cake: &str, said_before: &str) -> Value {
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
    fn strict_anthropic_shape(msgs: &[Value]) -> Result<(), String> {
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
            "s",
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
