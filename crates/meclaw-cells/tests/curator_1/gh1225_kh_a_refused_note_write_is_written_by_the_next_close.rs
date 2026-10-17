//! GH #1225 (A1) -- a note whose write the store refused is made by the next close.
//!
//! WHY this file exists: `./handover` runs `resident` -- one child whose
//! globals live from message to message -- and remembered a session as
//! "prepared" in its RAM the moment it SENT the note's `prepared` mark. When
//! the store refused that write, every later close of the session was dropped
//! as "prepared already" until the child restarted: the next session started
//! without the note (`repro_ram_truth.py`). Now only a `prepared` mark READ
//! from the ledger fills that memory. The test drives the shipped script in
//! ONE resident child (the way the colony runs it; the hive harness runs this
//! cell warm, with fresh globals per message) against the hive's ledger,
//! through the store cell's own dispatcher.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/warm_python.rs"]
#[allow(unused_imports)]
use crate::warm_python; // harness_fold: mod warm_python;

use curator_hive::*;
use meclaw_cells::store::ops::dispatch;
use meclaw_cells::store::output::{BundleLeg, build_bundle_result, build_tool_result};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use std::collections::VecDeque;

/// `./handover` in one resident child, its ledger the hive's.
struct Rig {
    h: Hive,
    child: warm_python::Resident,
    params: Map<String, Value>,
    /// The note requests for `./summarizer`, with the context they left in.
    summ: VecDeque<(Map<String, Value>, Value)>,
    stderr: String,
    /// `(leg, error_code)`: the store refuses that leg of the next ledger
    /// message that carries it.
    refuse: Vec<(String, String)>,
}

impl Rig {
    fn new(h: Hive) -> Self {
        let mut params = obj(cell_config("handover")["params"].clone());
        params.remove("script_inline");
        Self {
            h,
            child: warm_python::Resident::boot(&script_of("handover")),
            params,
            summ: VecDeque::new(),
            stderr: String::new(),
            refuse: Vec::new(),
        }
    }

    fn run(
        &mut self,
        ctx: &Map<String, Value>,
        hop: &Map<String, Value>,
        body: &Value,
    ) -> Vec<Value> {
        let doc = json!({
            "envelope": {"header": {"context": ctx, "hop": hop},
                         "target": "/x/curator/handover", "reply_to": ""},
            "body": body,
            "params": self.params,
        })
        .to_string();
        let out = self.child.run(&doc).expect("a one-line document");
        let err = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(out.status.success(), "handover exited non-zero: {err}");
        self.stderr.push_str(&err);
        match sj::from_slice(&out.stdout).expect("handover answers JSON") {
            Value::Array(a) => a,
            other => vec![other],
        }
    }

    /// One message into the child, run to rest: its own ledger round trips
    /// answered (the edges `./handover -> ./ledger -> ./handover` set the
    /// `cur_*` keys from the header), the note requests kept.
    fn pump(&mut self, ctx: Map<String, Value>, hop: Map<String, Value>, body: Value) {
        let mut queue = VecDeque::from([(ctx, hop, body)]);
        while let Some((ctx, hop, body)) = queue.pop_front() {
            for m in self.run(&ctx, &hop, &body) {
                let mut m = obj(m);
                let head = m.remove("header").map(obj).unwrap_or_default();
                match head.get("route").and_then(Value::as_str) {
                    Some("lstore") if head["reply_cell"] == "curator-handover" => {
                        let mut c = ctx.clone();
                        for (k, from) in [
                            ("cur_origin", "reply_cell"),
                            ("cur_phase", "phase"),
                            ("cur_call", "cur_call"),
                            ("cur_reason", "cur_reason"),
                        ] {
                            c.insert(k.into(), head[from].clone());
                        }
                        let (body, hop) = self.store(&m);
                        queue.push_back((c, hop, body));
                    }
                    Some("summarize") => self.summ.push_back((ctx.clone(), Value::Object(m))),
                    _ => {}
                }
            }
        }
    }

    /// The store cell's answer, in its own form; the legs `refuse` names refused.
    fn store(&mut self, msg: &Map<String, Value>) -> (Value, Map<String, Value>) {
        let calls: Vec<Value> = msg["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c["type"] == "tool_call")
            .collect();
        let struck: Vec<(String, String)> = self
            .refuse
            .iter()
            .filter(|(leg, _)| calls.iter().any(|c| c["id"] == leg.as_str()))
            .cloned()
            .collect();
        self.refuse.retain(|e| !struck.contains(e));
        if calls.len() == 1 && struck.is_empty() {
            let id = calls[0]["id"].as_str().unwrap_or("").to_string();
            let args: Value =
                sj::from_str(calls[0]["text"].as_str().unwrap_or("")).expect("op json");
            return match dispatch(&self.h.db, &args) {
                Ok(outcome) => build_tool_result(&outcome, id, 0),
                Err(e) => build_bundle_result(
                    &[BundleLeg::refusal(
                        args["operation"].as_str().unwrap_or("error"),
                        id,
                        0,
                        "invalid_input",
                        e,
                    )],
                    0,
                ),
            };
        }
        let legs: Vec<BundleLeg> = calls
            .iter()
            .map(|c| {
                let id = c["id"].as_str().unwrap_or("").to_string();
                let args: Value = sj::from_str(c["text"].as_str().unwrap_or("")).expect("op json");
                let op = args["operation"].as_str().unwrap_or("error").to_string();
                if let Some((_, code)) = struck.iter().find(|(leg, _)| *leg == id) {
                    return BundleLeg::refusal(&op, id, 0, code, "refused by the test".into());
                }
                match dispatch(&self.h.db, &args) {
                    Ok(outcome) => BundleLeg::from_outcome(&outcome, id, 0),
                    Err(e) => BundleLeg::refusal(&op, id, 0, "invalid_input", e),
                }
            })
            .collect();
        build_bundle_result(&legs, 0)
    }

    fn close(&mut self, session: &str) {
        self.pump(
            obj(json!({"session_id": session, "audience_set": ROUND_E})),
            obj(json!({"route": "in_close"})),
            json!({"messages": []}),
        );
    }

    /// The summarizer's note for the oldest request.
    fn answer(&mut self, text: &str) {
        let (ctx, _) = self.summ.pop_front().expect("a note request");
        self.pump(
            ctx,
            obj(json!({"finish_reason": "stop", "model": "summary-model",
                       "route": "in_summary"})),
            json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]}),
        );
    }
}

fn prepared_marks(h: &Hive, session: &str) -> usize {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = 'handover' AND session_id = '{session}'"
    ))
    .into_iter()
    .filter(|r| {
        r[0].as_str()
            .unwrap_or("")
            .contains("\"state\":\"prepared\"")
    })
    .count()
}

#[test]
fn a_close_after_a_refused_note_write_makes_the_note() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s-a",
        "a1",
        "kh1 a: the parcel went to Ulm.",
        "kh1 noted.",
        json!({}),
    );
    let mut rig = Rig::new(h);
    rig.close("s-a");
    assert_eq!(rig.summ.len(), 1, "the note is asked for: {}", rig.stderr);
    // The store refuses the `prepared` mark of the note.
    rig.refuse
        .push(("w-mark".to_string(), "store_error".to_string()));
    rig.answer("kh1 note: the parcel went to Ulm.");
    assert!(rig.refuse.is_empty(), "the note step wrote: {}", rig.stderr);
    assert_eq!(prepared_marks(&rig.h, "s-a"), 0, "the mark was refused");

    // The same child, the next close of the session: the note is made.
    rig.close("s-a");
    assert!(
        !rig.stderr.contains("prepared already"),
        "a mark it only sent never decides: {}",
        rig.stderr
    );
    assert_eq!(
        rig.summ.len(),
        1,
        "the note is asked for again: {}",
        rig.stderr
    );
    rig.answer("kh1 note: the parcel went to Ulm.");
    assert_eq!(prepared_marks(&rig.h, "s-a"), 1, "{}", rig.stderr);

    // A mark it READ decides: the next close is dropped.
    rig.close("s-a");
    assert!(rig.summ.is_empty(), "prepared once: {}", rig.stderr);
    assert!(rig.stderr.contains("prepared already"), "{}", rig.stderr);
}
