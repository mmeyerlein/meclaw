//! GH #849, follow-up -- a sidecar fact names the participant it came from.
//!
//! Measured before this change: a sidecar or `remember` block named no turn,
//! so in a group channel where more than one participant spoke since the last
//! answer `extract-glue` could not tell whose words a fact was. It filed
//! nothing and refused the whole block as `ambiguous_speaker`, and the close
//! pass had to pick every fact up at the end of the session. Two participants
//! arriving together was enough.
//!
//! The model sees who spoke: every peer turn reaches it behind the frame
//! `[peer <ref> · <name>]`. So the contract now asks for that reference on each
//! fact (`source`), and the ingress binds the fact to exactly that participant's
//! turn when the reference is among the peer turns since the last answer. A
//! reference that is not there is dropped and receipted on `reject`
//! (`unknown_source`), never re-attributed. A fact without a source keeps the
//! old rule: one speaker since the last answer binds, several wait for the
//! close pass. No fact of a peer ever becomes the member's own side.
//!
//! Every script below is the REAL shipped `params.script_inline`, run over
//! stdin against injected store replies; no colony and no model.

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::{code_stdin, emit_all, run_shipped_script, shipped_script};

const EXTRACT: &str = "../../templates/memory-hive/extract-glue/config.json";
const SCHEMAS: &str = "../../templates/memory-hive/schemas/config.json";
const CONTRACT: &str = "../../templates/memory-hive/inline-contract.md";

const AUDIENCE: &str = "[\"agent:helper\", \"member:alex\", \"peer:jo\", \"peer:sam\"]";
/// Participant references as affinity mints them.
const A: &str = "3a47fe3e";
const B: &str = "5c0d9e21";
const C: &str = "9f00aa11";

fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().unwrap_or("null");
    serde_json::from_str(text).unwrap_or(Value::Null)
}

/// The message of one ingress step in `phase`, with the store's answer `rows`.
fn step_doc(phase: &str, op: &str, batch: &str, rows: Value) -> Value {
    json!({
        "header": {"context": {"mem_phase": phase, "batch_id": batch, "session_id": "s-1",
                               "audience_set": AUDIENCE, "channel": "group:trip"},
                   "hop": {"operation": op, "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// One run of the ingress script in `phase`, with the store's answer `rows`.
fn step(phase: &str, op: &str, batch: &str, rows: Value) -> Vec<Value> {
    emit_all(&shipped_script(EXTRACT), &step_doc(phase, op, batch, rows))
}

/// The same run, read on the other channel: what the script wrote to stderr,
/// the operator's log.
fn step_stderr(phase: &str, op: &str, batch: &str, rows: Value) -> String {
    let out = run_shipped_script(
        &shipped_script(EXTRACT),
        &code_stdin(&step_doc(phase, op, batch, rows)).to_string(),
    );
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The sidecar section `memory` as the splitter hands it to the door.
fn arrive(facts: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(EXTRACT),
        &json!({
            "header": {"context": {"mem_phase": "inline", "batch_id": "", "session_id": "s-1",
                                   "audience_set": AUDIENCE, "channel": "group:trip"},
                       "hop": {"route": "sidecar", "section": "memory"}},
            "section": "memory",
            "payload": {"facts": facts, "topic": {"movement": "continue", "name": ""}},
            "messages": []
        }),
    )
}

fn fact(claim: &str, source: Option<&str>) -> Value {
    let mut f = json!({"subject": "member:alex", "predicate": "agreed_to", "claim": claim,
                       "fact_kind": "experience", "valid_from": null});
    if let Some(s) = source {
        f["source"] = json!(s);
    }
    f
}

fn ep(id: &str, sender: &str, speaker: &str, at: &str) -> Value {
    json!({"id": id, "turn_id": format!("s-1#{id}"), "session_id": "s-1", "sender": sender,
           "speaker": speaker, "recorded_at": at})
}

/// The page `known-eps` reads for the same turns, so `apply` sees their source.
fn episode_page(page: &Value) -> Value {
    Value::Array(
        page.as_array()
            .expect("page")
            .iter()
            .map(|r| {
                json!({"id": r["id"], "happened_at": r["recorded_at"], "session_id": "s-1",
                       "channel": "group:trip", "audience_set": AUDIENCE,
                       "sender": r["sender"], "speaker": r["speaker"]})
            })
            .collect(),
    )
}

/// [peer A, peer B] in one arrival, above the previous answer. The newest
/// assistant episode may be this answer's own (it is written concurrently), so
/// the member's turn below it counts as answerable too; C spoke before the
/// answer before that and is not among the turns this block can be answering.
fn two_peers() -> Value {
    json!([
        ep("ep-b", "peer", B, "2026-09-20T10:00:02Z"),
        ep("ep-a", "peer", A, "2026-09-20T10:00:01Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T09:59:00Z"),
        ep("ep-u", "user", "member:alex", "2026-09-20T09:58:00Z"),
        ep("ep-x0", "assistant", "agent:helper", "2026-09-20T09:57:00Z"),
        ep("ep-c", "peer", C, "2026-09-20T09:56:00Z"),
    ])
}

/// What the whole inline road produced for one block over one bind page.
struct Road {
    /// The emission of the bind step.
    bind: Vec<Value>,
    /// The emission of the meeting step after the bind (empty if the bind refused).
    meet: Vec<Value>,
    /// The fact rows the store was finally asked to insert.
    facts: Vec<Value>,
    /// The emission of the apply step.
    apply: Vec<Value>,
    /// The batch key of the meeting read (empty if the bind refused).
    key: String,
    /// The scratch row the block was parked as.
    parked: Value,
}

fn road(facts: Value, page: Value) -> Road {
    let parked_out = arrive(facts);
    let park = parked_out
        .iter()
        .find(|m| m["header"]["phase"] == "inline-turn")
        .unwrap_or_else(|| panic!("the block was not parked: {parked_out:?}"));
    let bid = park["header"]["batch_id"]
        .as_str()
        .expect("bid")
        .to_string();
    let parked = args_of(park)["row"].clone();
    let bind = step("inline-bind", "select", &bid, page.clone());
    let Some(next) = bind.iter().find(|m| m["header"]["phase"] == "inline-apply") else {
        return Road {
            bind,
            meet: vec![],
            facts: vec![],
            apply: vec![],
            key: String::new(),
            parked,
        };
    };
    let key = next["header"]["batch_id"]
        .as_str()
        .expect("key")
        .to_string();
    let meet = step("inline-apply", "select", &key, json!([parked.clone()]));
    let staged = meet
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
        .map(|a| a["row"]["payload"].as_str().unwrap_or("{}").to_string());
    let Some(staged) = staged else {
        return Road {
            bind,
            meet,
            facts: vec![],
            apply: vec![],
            key,
            parked,
        };
    };
    let eps_out = step("known-eps", "select", "inline:b1", episode_page(&page));
    let eps = args_of(&eps_out[0])["row"]["payload"].clone();
    let rows = json!([
        {"key": "inline:b1", "kind": "payload", "payload": staged},
        {"key": "inline:b1", "kind": "known", "payload": "[]"},
        {"key": "inline:b1", "kind": "episodes", "payload": eps},
    ]);
    let apply = step("apply", "select", "inline:b1", rows);
    let facts = apply
        .iter()
        .map(args_of)
        .filter(|a| a["operation"] == "insert" && a["table"] == "facts")
        .map(|a| a["row"].clone())
        .collect();
    Road {
        bind,
        meet,
        facts,
        apply,
        key,
        parked,
    }
}

fn rejects(out: &[Value]) -> Vec<(String, String)> {
    out.iter()
        .filter(|m| m["header"]["route"] == "reject")
        .map(|m| {
            (
                m["header"]["reject_reason"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                m["messages"][0]["text"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn covered(out: &[Value]) -> Vec<String> {
    out.iter()
        .map(args_of)
        .filter(|a| a["table"] == "pending_extraction" && a["operation"] == "update")
        .flat_map(|a| {
            a["where"]["episode_id"]["in"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn by_claim<'a>(facts: &'a [Value], claim: &str) -> Option<&'a Value> {
    facts.iter().find(|f| f["claim"] == claim)
}

#[test]
fn two_peers_in_one_arrival_each_named_fact_binds_to_its_speaker() {
    let r = road(
        json!([
            fact("A drives on Saturday", Some(A)),
            fact("B brings the map", Some(B))
        ]),
        two_peers(),
    );
    assert!(
        rejects(&r.bind).is_empty(),
        "a block whose facts name their speakers was refused: {:?}",
        r.bind
    );
    let a = by_claim(&r.facts, "A drives on Saturday")
        .unwrap_or_else(|| panic!("A's fact was not written: {:?}", r.apply));
    let b = by_claim(&r.facts, "B brings the map")
        .unwrap_or_else(|| panic!("B's fact was not written: {:?}", r.apply));
    assert_eq!(
        (&a["episode_id"], &a["source"]),
        (&json!("ep-a"), &json!(A))
    );
    assert_eq!(
        (&b["episode_id"], &b["source"]),
        (&json!("ep-b"), &json!(B))
    );
    let mut cov = covered(&r.meet);
    cov.sort();
    assert_eq!(cov, ["ep-a", "ep-b"], "{:?}", r.meet);
    assert!(rejects(&r.meet).is_empty(), "{:?}", r.meet);

    // B arrived while the brain was answering A: the assistant episode on the
    // page may be this answer's own, so A's turn below it is still answerable.
    let r = road(
        json!([fact("A drives on Saturday", Some(A))]),
        json!([
            ep("ep-b", "peer", B, "2026-09-20T10:00:05Z"),
            ep("ep-x", "assistant", "agent:helper", "2026-09-20T10:00:04Z"),
            ep("ep-a", "peer", A, "2026-09-20T10:00:01Z"),
        ]),
    );
    assert_eq!(r.facts.len(), 1, "{:?} {:?}", r.bind, r.meet);
    assert_eq!(
        (&r.facts[0]["episode_id"], &r.facts[0]["source"]),
        (&json!("ep-a"), &json!(A))
    );
}

#[test]
fn a_named_source_that_did_not_speak_is_dropped_and_receipted() {
    // C spoke, but two answers ago: not among the turns this block can be
    // answering. Z never spoke at all.
    let r = road(
        json!([
            fact("A drives on Saturday", Some(A)),
            fact("C lends the car", Some(C)),
            fact("Z pays", Some("0badf00d"))
        ]),
        two_peers(),
    );
    assert_eq!(r.facts.len(), 1, "{:?}", r.facts);
    assert_eq!(r.facts[0]["source"], A);
    assert!(
        r.facts.iter().all(|f| f["source"] == A),
        "a fact with an absent source was bent onto a speaker who was there: {:?}",
        r.facts
    );
    let rej = rejects(&r.meet);
    let unknown: Vec<&(String, String)> =
        rej.iter().filter(|(w, _)| w == "unknown_source").collect();
    assert_eq!(
        unknown.len(),
        1,
        "the drop is receipted on `reject`: {:?}",
        r.meet
    );
    assert!(
        unknown[0].1.contains(C) && unknown[0].1.contains("0badf00d"),
        "the receipt names the sources it dropped: {}",
        unknown[0].1
    );
    assert_eq!(covered(&r.meet), ["ep-a"], "{:?}", r.meet);

    // A block whose only fact names an absent source writes nothing at all.
    let r = road(json!([fact("C lends the car", Some(C))]), two_peers());
    assert!(r.facts.is_empty(), "{:?}", r.facts);
    assert!(
        r.meet.iter().all(|m| m["header"]["route"] == "reject"),
        "nothing to file is write-free: {:?}",
        r.meet
    );
    assert_eq!(rejects(&r.meet)[0].0, "unknown_source");
}

#[test]
fn a_fact_without_a_source_between_two_speakers_still_waits_for_the_close_pass() {
    // No fact names a source: exactly the refusal of before, at the bind.
    let r = road(json!([fact("somebody drives", None)]), two_peers());
    assert!(r.meet.is_empty() && r.facts.is_empty(), "{:?}", r.meet);
    assert_eq!(
        rejects(&r.bind),
        [(
            "ambiguous_speaker".to_string(),
            "inline payload rejected: the turns this block may answer name more than one \
             speaker (or a peer without a reference) -- the source of its facts cannot be \
             vouched for, the close pass speaks for them"
                .to_string()
        )],
        "{:?}",
        r.bind
    );
    assert!(r.bind.iter().all(|m| m["header"]["route"] == "reject"));

    // Next to a named fact, the unnamed one is not filed and is receipted; the
    // turns it may belong to keep their `pending` rows.
    let r = road(
        json!([
            fact("A drives on Saturday", Some(A)),
            fact("somebody drives", None)
        ]),
        two_peers(),
    );
    assert_eq!(r.facts.len(), 1, "{:?}", r.facts);
    assert_eq!(r.facts[0]["source"], A);
    assert_eq!(covered(&r.meet), ["ep-a"], "{:?}", r.meet);
    assert!(
        rejects(&r.meet)
            .iter()
            .any(|(w, _)| w == "ambiguous_speaker"),
        "{:?}",
        r.meet
    );
}

#[test]
fn no_fact_of_a_peer_ever_becomes_the_own_side() {
    // [peer A, user] -- the member and a peer. A named fact is A's; an unnamed
    // one is not guessed onto either.
    let page = json!([
        ep("ep-a", "peer", A, "2026-09-20T10:00:02Z"),
        ep("ep-u", "user", "member:alex", "2026-09-20T10:00:01Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T09:59:00Z"),
    ]);
    let r = road(
        json!([
            fact("A drives on Saturday", Some(A)),
            fact("somebody drives", None)
        ]),
        page,
    );
    assert_eq!(r.facts.len(), 1, "{:?}", r.facts);
    assert_eq!(
        (&r.facts[0]["episode_id"], &r.facts[0]["source"]),
        (&json!("ep-a"), &json!(A))
    );

    // On the road where the block names its episode (the close pass): a named
    // source that is not the speaker of that episode is dropped, never filed
    // under the episode's speaker or the member.
    let staged = json!({"facts": [
        {"episode_id": "ep-a", "subject": "member:alex", "predicate": "agreed_to",
         "claim": "B brings the map", "fact_kind": "experience", "valid_from": "",
         "valid_until": null, "confidence": 70, "replaces": "", "claim_hash": "h1",
         "source": B},
        {"episode_id": "ep-u", "subject": "member:alex", "predicate": "agreed_to",
         "claim": "A drives", "fact_kind": "experience", "valid_from": "",
         "valid_until": null, "confidence": 70, "replaces": "", "claim_hash": "h2",
         "source": A}], "entities": [], "edges": []});
    let eps_out = step(
        "known-eps",
        "select",
        "inline:b1",
        episode_page(&json!([
            ep("ep-a", "peer", A, "2026-09-20T10:00:02Z"),
            ep("ep-u", "user", "member:alex", "2026-09-20T10:00:01Z"),
        ])),
    );
    let eps = args_of(&eps_out[0])["row"]["payload"].clone();
    let out = step(
        "apply",
        "select",
        "inline:b1",
        json!([
            {"key": "inline:b1", "kind": "payload", "payload": staged.to_string()},
            {"key": "inline:b1", "kind": "known", "payload": "[]"},
            {"key": "inline:b1", "kind": "episodes", "payload": eps},
        ]),
    );
    let facts: Vec<Value> = out
        .iter()
        .map(args_of)
        .filter(|a| a["operation"] == "insert" && a["table"] == "facts")
        .collect();
    assert!(
        facts.is_empty(),
        "a named source was bent onto the episode's speaker: {facts:?}"
    );
    assert!(
        rejects(&out).iter().any(|(w, _)| w == "unknown_source"),
        "{out:?}"
    );
}

#[test]
fn one_to_one_is_unchanged() {
    let page = json!([
        ep("ep-u2", "user", "member:alex", "2026-09-20T10:00:02Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T10:00:01Z"),
        ep("ep-u1", "user", "member:alex", "2026-09-20T10:00:00Z"),
    ]);
    let parked_out = arrive(json!([fact("likes maps", None)]));
    let park = parked_out
        .iter()
        .find(|m| m["header"]["phase"] == "inline-turn")
        .expect("parked");
    let bid = park["header"]["batch_id"].as_str().expect("bid");
    assert!(bid.starts_with("inline:"), "{bid}");
    let parked: Value =
        serde_json::from_str(args_of(park)["row"]["payload"].as_str().expect("payload"))
            .expect("json");
    let mut keys: Vec<&String> = parked["facts"][0]
        .as_object()
        .expect("fact")
        .keys()
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "audience_set",
            "channel",
            "claim",
            "claim_hash",
            "confidence",
            "episode_id",
            "fact_kind",
            "predicate",
            "replaces",
            "subject",
            "valid_from",
            "valid_until"
        ],
        "a fact without a source parks exactly as before"
    );
    let r = road(json!([fact("likes maps", None)]), page);
    let key = r
        .bind
        .iter()
        .find(|m| m["header"]["phase"] == "inline-apply")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .expect("bound");
    assert_eq!(
        key.split_once('|').map(|(_, e)| e),
        Some("ep-u2"),
        "the batch key carries the bound turn and nothing else: {key}"
    );
    assert_eq!(r.facts.len(), 1);
    assert_eq!(
        (&r.facts[0]["episode_id"], &r.facts[0]["source"]),
        (&json!("ep-u2"), &json!(""))
    );
    assert_eq!(covered(&r.meet), ["ep-u2"]);
    assert!(rejects(&r.meet).is_empty() && rejects(&r.apply).is_empty());
}

#[test]
fn the_contract_asks_each_fact_for_its_source() {
    let contract = std::fs::read_to_string(CONTRACT).expect("contract");
    let block = contract
        .split("```text\n")
        .nth(1)
        .and_then(|b| b.split("\n```").next())
        .expect("the contract block");
    assert!(
        block.contains("`source`") && block.contains("[peer <ref> · <name>]"),
        "the block asks for the reference out of the frame: {block}"
    );
    // The offered schema declares it, optional -- an own-side fact has none,
    // and a required key would be rendered into every example.
    let out = meclaw_testing::emit_one(
        &shipped_script(SCHEMAS),
        &json!({"tools": ["*"], "messages": []}),
    );
    let item = &out["sidecar"][0]["schema"]["properties"]["facts"]["items"];
    assert_eq!(item["properties"]["source"]["type"], "string", "{item}");
    assert!(
        !item["required"]
            .as_array()
            .expect("required")
            .iter()
            .any(|k| k == "source"),
        "{item}"
    );
}

#[test]
fn a_named_source_that_is_the_agent_or_the_member_is_dropped() {
    // A group page on which the member spoke next to A. Neither the agent nor
    // the member is a peer: the own side has no `source`, so a fact that names
    // one of them names nobody the page can vouch for. Dropped and receipted,
    // never filed as the member's own statement or under the agent.
    let page = json!([
        ep("ep-a", "peer", A, "2026-09-20T10:00:02Z"),
        ep("ep-u", "user", "member:alex", "2026-09-20T10:00:01Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T09:59:00Z"),
    ]);
    let r = road(
        json!([
            fact("A drives on Saturday", Some(A)),
            fact("the helper books the train", Some("agent:helper")),
            fact("alex pays", Some("member:alex"))
        ]),
        page,
    );
    let filed: Vec<(&Value, &Value, &Value)> = r
        .facts
        .iter()
        .map(|f| (&f["claim"], &f["episode_id"], &f["source"]))
        .collect();
    assert_eq!(
        filed,
        [(&json!("A drives on Saturday"), &json!("ep-a"), &json!(A))],
        "{:?}",
        r.apply
    );
    let rej = rejects(&r.meet);
    let unknown: Vec<&(String, String)> =
        rej.iter().filter(|(w, _)| w == "unknown_source").collect();
    assert_eq!(unknown.len(), 1, "{:?}", r.meet);
    assert!(
        unknown[0].1.contains("dropped 2 fact(s)"),
        "{}",
        unknown[0].1
    );
    assert_eq!(covered(&r.meet), ["ep-a"], "{:?}", r.meet);
}

#[test]
fn a_named_stranger_next_to_an_unnamed_fact_on_a_page_with_one_source() {
    // A page with ONE source (A twice, the agent between): the bind vouches for
    // the answered turn, and the named road still runs because one fact names
    // somebody. The unnamed fact takes the answered turn, as on the old road;
    // the stranger C is dropped, never bent onto A.
    let page = json!([
        ep("ep-a", "peer", A, "2026-09-20T10:00:03Z"),
        ep("ep-x", "assistant", "agent:helper", "2026-09-20T10:00:02Z"),
        ep("ep-a0", "peer", A, "2026-09-20T10:00:01Z"),
    ]);
    let r = road(
        json!([
            fact("drives on Saturday", None),
            fact("C lends the car", Some(C))
        ]),
        page,
    );
    assert!(r.key.starts_with("inline-sourced:"), "{}", r.key);
    let a_ref = format!("{A}=ep-a");
    assert_eq!(
        r.key.split('|').skip(1).collect::<Vec<_>>(),
        ["ep-a", a_ref.as_str()],
        "the meeting key carries the answered turn and the references: {}",
        r.key
    );
    let filed: Vec<(&Value, &Value, &Value)> = r
        .facts
        .iter()
        .map(|f| (&f["claim"], &f["episode_id"], &f["source"]))
        .collect();
    assert_eq!(
        filed,
        [(&json!("drives on Saturday"), &json!("ep-a"), &json!(A))],
        "{:?}",
        r.apply
    );
    let rej = rejects(&r.meet);
    assert_eq!(
        rej.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>(),
        ["unknown_source"],
        "{rej:?}"
    );
    assert!(rej[0].1.contains(C), "{}", rej[0].1);
    assert_eq!(covered(&r.meet), ["ep-a"], "{:?}", r.meet);
}

#[test]
fn a_source_is_trimmed_and_case_folded_and_a_non_string_drops_its_item() {
    // OR-SN.N2.5: the reference is only trimmed and case-folded, never looked
    // up; a `source` that is not a string is a malformed field and drops its
    // item like any other.
    let mut bad = fact("B brings the map", None);
    bad["source"] = json!(42);
    let r = road(
        json!([fact("A drives on Saturday", Some(" 3A47FE3E ")), bad]),
        two_peers(),
    );
    let filed: Vec<(&Value, &Value, &Value)> = r
        .facts
        .iter()
        .map(|f| (&f["claim"], &f["episode_id"], &f["source"]))
        .collect();
    assert_eq!(
        filed,
        [(&json!("A drives on Saturday"), &json!("ep-a"), &json!(A))],
        "{:?}",
        r.apply
    );
    let parked: Value =
        serde_json::from_str(r.parked["payload"].as_str().expect("payload")).expect("json");
    let sources: Vec<&Value> = parked["facts"]
        .as_array()
        .expect("facts")
        .iter()
        .map(|f| &f["source"])
        .collect();
    assert_eq!(sources, [&json!(A)], "{parked}");
    assert!(rejects(&r.meet).is_empty(), "{:?}", r.meet);
}

#[test]
fn a_source_the_model_wrote_reaches_no_receipt_or_log_line_unfiltered() {
    // Review rev-N2 M6: the `source` of a dropped fact is the model's text. Cut
    // to 64 characters it could still carry a line break and a control
    // sequence into the `reject` receipt and into stderr, and forge a log line.
    // Only a value of the reference form is shown; anything else is counted.
    let forged = "0badf00d\nextract-glue: wrote 99 facts\u{1b}[2J";
    let r = road(
        json!([
            fact("A drives on Saturday", Some(A)),
            fact("Z pays", Some("0badf00d")),
            fact("nobody", Some(forged))
        ]),
        two_peers(),
    );
    assert_eq!(r.facts.len(), 1, "{:?}", r.facts);
    let rej = rejects(&r.meet);
    let unknown: Vec<&(String, String)> =
        rej.iter().filter(|(w, _)| w == "unknown_source").collect();
    assert_eq!(unknown.len(), 1, "{:?}", r.meet);
    let text = &unknown[0].1;
    assert!(
        !text.contains("wrote 99") && !text.chars().any(char::is_control),
        "the receipt carries the model's text unfiltered: {text:?}"
    );
    assert!(
        text.contains("0badf00d") && text.contains("<1 non-ref value(s)>"),
        "a reference is still named and the rest is counted: {text:?}"
    );
    let err = step_stderr("inline-apply", "select", &r.key, json!([r.parked.clone()]));
    assert!(
        !err.contains("wrote 99") && !err.contains('\u{1b}'),
        "a log line was forged: {err:?}"
    );
    assert!(err.contains("dropped 2 fact(s)"), "{err:?}");
}
