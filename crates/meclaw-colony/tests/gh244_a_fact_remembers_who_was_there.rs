//! GH #244 / ADR-0002 — a fact remembers who was there.
//!
//! `memory-hive@2.0.1` has no column for a PUBLIC. Every row it ever wrote is
//! addressed to whoever asks next, so a sentence learned in a two-person
//! channel answers a question asked in front of a third person. This file is
//! the proof of the gate that closes that, written against
//! `plans/0.16.0-audience-gate/contract.md` — the shared contract, not against
//! whatever the templates happen to contain today.
//!
//! # What is under test, and how honestly
//!
//! The gate lives in two shipped scripts (`writer` on the write path, `recall`
//! on the read path, `dream-glue` where a belief is derived) and in one shipped
//! schema (`store`). None of those can be spawned from this crate: `store` and
//! `code` cells live in `meclaw-cells`, which depends on `meclaw-colony`, so a
//! colony carrying the real hive can only be booted from downstream of here
//! (`crates/meclaw-cells/tests/w10b_remember_colony.rs` is that test). What
//! this file drives instead is the P5/`cellrun` pattern already used by every
//! other memory-hive script test in the repo: the REAL `params.script_inline`
//! of the REAL shipped `config.json`, with `${VAR:-default}` resolved the way
//! the colony resolves it at instantiation, one hop per process. The logic
//! under test is the shipped artefact; only the transport around it is the
//! test's. Store REPLIES are fixtures — they are data, and data is what the
//! gate is supposed to read.
//!
//! Everything is guarded on the template being present (GH #49): the public
//! tree does not ship `templates/memory-hive`, and there the body does not run.
//!
//! # The claims
//!
//! 1. The seven rows of the contract's behaviour table, proven on the tier-0
//!    bundle — the thing a caller actually receives.
//! 2. **The open-history clause never crosses a channel boundary.** Its own
//!    test, because it is the clause that can leak a private channel into a
//!    group one, and because "same channel" is a condition, not a decoration.
//! 3. Fail-closed on both paths: a write without a public is refused and
//!    writes NOTHING, a read without a public is REFUSED — not answered with
//!    an empty bundle. An empty bundle and a refusal are different answers.
//! 4. Derived rows do not launder their sources (contract Nachtrag 1): a
//!    belief may only be told to the INTERSECTION of the publics of the facts
//!    it rests on, an empty intersection means nobody, and an entity edge is
//!    gated like the episode it came from.
//! 5. The read path asks the store for the columns it is going to filter on —
//!    a filter over a column nobody selected is a filter that never fires.

use meclaw_core::serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;

// ------------------------------------------------------------------ harness

/// The shipped template, or `None` in a tree that does not carry it (GH #49).
fn hive_root() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/memory-hive");
    p.join("config.json").is_file().then_some(p)
}

fn hive_config() -> Value {
    let p = hive_root().expect("template").join("config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("read")).expect("json")
}

fn cell_config(cell: &str) -> Value {
    let p = hive_root()
        .expect("template")
        .join(cell)
        .join("config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("read")).expect("json")
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` the empty string —
/// the substitution the colony performs at instantiation.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(cell: &str) -> String {
    resolve_vars(
        cell_config(cell)["params"]["script_inline"]
            .as_str()
            .unwrap_or_else(|| panic!("{cell} has no script_inline")),
    )
}

/// The three-object stdin document the substrate builds: its own fields under
/// `envelope`, the message slots under `body`, the configuration under
/// `params`.
fn stdin_doc(flat: &Value) -> Value {
    let mut envelope = meclaw_core::serde_json::Map::new();
    let mut slots = meclaw_core::serde_json::Map::new();
    for (k, v) in flat.as_object().expect("a flat message object") {
        if k == "header" {
            envelope.insert(k.clone(), v.clone());
        } else {
            slots.insert(k.clone(), v.clone());
        }
    }
    json!({"envelope": envelope, "body": slots, "params": {}})
}

/// Drives ONE real hop of a shipped cell: `body` goes in as stdin, whatever the
/// script emitted comes back parsed.
fn run_hop(cell: &str, body: &Value) -> Value {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "_real.write(_sink.getvalue())\n"
        ),
        meclaw_core::serde_json::to_string(&script_of(cell)).unwrap(),
        meclaw_core::serde_json::to_string(&stdin_doc(body).to_string()).unwrap(),
    );
    let out = Command::new("python3")
        .arg("-c")
        .arg(src)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "{cell} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{cell} emitted was not JSON ({e}): {raw}"))
}

fn emitted(out: &Value) -> Vec<Value> {
    out.as_array()
        .unwrap_or_else(|| panic!("an emission is an array: {out}"))
        .clone()
}

fn route_of(msg: &Value) -> String {
    msg["header"]["route"].as_str().unwrap_or_default().into()
}

/// The store-native args of a `tool_call` turn.
fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().unwrap_or("null");
    meclaw_core::serde_json::from_str(text).unwrap_or(Value::Null)
}

// ------------------------------------------------------------- vocabulary

const A: &str = "member:anna";
const B: &str = "member:bea";
const C: &str = "member:cem";
const CH1: &str = "tg:-100111";
const CH2: &str = "tg:-100222";
const RID: &str = "r-gh244";

/// A participant set as the contract carries it: a JSON list, as text.
fn aud(members: &[&str]) -> String {
    meclaw_core::serde_json::to_string(members).unwrap()
}

/// The read-path context of one question: who is in the room, which room, and
/// whether that room shows joiners its history (absent = the contract default,
/// `"0"`).
fn asking(now: &[&str], channel: &str, open_history: Option<&str>) -> Value {
    let mut ctx = json!({
        "audience_now": aud(now),
        "channel": channel,
        "recall_id": RID,
        "recall_query": "",
    });
    if let Some(o) = open_history {
        ctx["channel_open_history"] = json!(o);
    }
    ctx
}

/// One episode row as the store would return it, with the two columns of the
/// contract on it.
fn episode_row(id: &str, content: &str, channel: &str, audience: &str) -> Value {
    json!({"id": id, "session_id": "s-1", "sender": "user", "content": content,
           "happened_at": "2026-08-19T10:00:00Z", "recorded_at": "2026-08-19T10:00:00Z",
           "channel": channel, "audience_set": audience})
}

/// A `foresight` fact — the FACTS row that reaches a tier-0 bundle.
fn foresight_row(id: &str, claim: &str, channel: &str, audience: &str) -> Value {
    json!({"id": id, "subject": "user", "predicate": "plans", "claim": claim,
           "fact_kind": "foresight", "valid_from": "2026-08-19T10:00:00Z",
           "valid_until": "", "expired_at": "", "confidence": 90,
           "channel": channel, "audience_set": audience})
}

/// A belief. It has NO channel: it was derived, never said in a room (contract
/// Nachtrag 1), so only the subset rule can ever let it out.
fn belief_row(id: &str, statement: &str, audience: &str) -> Value {
    json!({"id": id, "holder": "self", "statement": statement, "confidence": 80,
           "active": 1, "updated_at": "2026-08-19T10:00:00Z", "audience_set": audience})
}

// ------------------------------------------------- driving the tier-0 bundle

/// Runs one tier-0 leg of the shipped `recall` script and returns the payload
/// it put into `recall_scratch` — the projection that survives into the bundle.
fn leg_payload(leg: &str, rows: &[Value], ctx: &Value) -> String {
    let mut ctx = ctx.clone();
    ctx["mem_phase"] = json!(leg);
    let body = json!({
        "header": {"context": ctx, "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "text": Value::Array(rows.to_vec()).to_string()}]
    });
    let out = emitted(&run_hop("recall", &body));
    assert_eq!(
        out.len(),
        1,
        "a tier-0 leg emits its collect insert: {out:?}"
    );
    args_of(&out[0])["row"]["payload"]
        .as_str()
        .expect("the leg payload")
        .to_string()
}

/// The whole tier-0 read path of the shipped `recall` script, hop by hop: the
/// three legs project their rows, then the `fire` phase fuses them into the
/// bundle a caller receives. Deliberately drives BOTH halves — the contract
/// says where the gate must hold (every row leaving the read path), not in
/// which phase the implementation puts it, and a test that pinned the phase
/// would forbid a legal implementation.
fn tier0_bundle(ctx: &Value, episodes: &[Value], beliefs: &[Value], foresight: &[Value]) -> Value {
    let scratch = json!([
        {"leg": "leg-episodes", "payload": leg_payload("leg-episodes", episodes, ctx)},
        {"leg": "leg-beliefs", "payload": leg_payload("leg-beliefs", beliefs, ctx)},
        {"leg": "leg-foresight", "payload": leg_payload("leg-foresight", foresight, ctx)},
    ]);
    let mut ctx = ctx.clone();
    ctx["mem_phase"] = json!("fire");
    let body = json!({
        "header": {"context": ctx, "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}]
    });
    let out = emitted(&run_hop("recall", &body));
    assert_eq!(out.len(), 1, "the fire phase emits one bundle: {out:?}");
    let text = out[0]["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("the bundle");
    meclaw_core::serde_json::from_str(text).expect("the bundle is JSON")
}

/// The ids of one bundle section.
fn ids(bundle: &Value, section: &str) -> Vec<String> {
    bundle[section]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|i| i["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// One episode said in front of `said_before` in `said_in`, asked for by
/// `now` in `asked_in`. Returns whether it reached the bundle.
fn episode_survives(
    said_before: &[&str],
    said_in: &str,
    now: &[&str],
    asked_in: &str,
    open_history: Option<&str>,
) -> bool {
    let ctx = asking(now, asked_in, open_history);
    // Every call carries a CONTROL row: untagged, and said in the very room the
    // question is asked from. Without it the four `allowed` rows of the
    // behaviour table would be green against a read path that has no gate at
    // all — and a test that passes before the feature exists proves nothing
    // about it. Since R2 (Nachtrag 2) it does not have to dodge into a third
    // channel any more: an untagged row is refused BEFORE the open-history
    // clause is reached, so the strongest possible placement is also the
    // correct one.
    let rows = [
        episode_row("e-1", "the sentence", said_in, &aud(said_before)),
        episode_row("e-control", "nobody may ever see this", asked_in, "[]"),
    ];
    let out = ids(&tier0_bundle(&ctx, &rows, &[], &[]), "episodes");
    assert!(
        !out.contains(&"e-control".to_string()),
        "the control row escaped — the read path is not gating at all: {out:?}"
    );
    out.contains(&"e-1".to_string())
}

// ================================================== the behaviour table (7)

#[test]
fn a_third_person_joining_the_room_silences_what_was_said_before_they_arrived() {
    if hive_root().is_none() {
        return;
    }
    assert!(
        !episode_survives(&[A, B], CH1, &[A, B, C], CH1, None),
        "{{A,B,C}} is not a subset of {{A,B}} — the fact must not appear"
    );
}

#[test]
fn someone_leaving_the_room_does_not_take_the_memory_with_them() {
    if hive_root().is_none() {
        return;
    }
    assert!(
        episode_survives(&[A, B, C], CH1, &[A, B], CH1, None),
        "{{A,B}} is a subset of {{A,B,C}} — A and B were both there"
    );
}

#[test]
fn the_same_circle_still_hears_what_it_said_itself() {
    if hive_root().is_none() {
        return;
    }
    assert!(episode_survives(&[A, B], CH1, &[A, B], CH1, None));
}

#[test]
fn an_open_history_in_another_channel_does_not_open_this_one() {
    if hive_root().is_none() {
        return;
    }
    assert!(
        !episode_survives(&[A, B], CH1, &[A, B, C], CH2, Some("1")),
        "the sentence was said in another room; this room's policy is not its policy"
    );
}

#[test]
fn a_channel_that_shows_joiners_its_history_may_be_told_its_own_past() {
    if hive_root().is_none() {
        return;
    }
    assert!(
        episode_survives(&[A, B], CH1, &[A, B, C], CH1, Some("1")),
        "the room has already shown it — the agent must not be more secretive than the room"
    );
}

#[test]
fn a_universal_audience_is_told_to_anyone() {
    if hive_root().is_none() {
        return;
    }
    assert!(episode_survives(&["*"], CH1, &[A, B, C], CH2, None));
    assert!(episode_survives(&["*"], CH1, &[C], CH2, None));
}

#[test]
fn an_untagged_row_is_invisible_rather_than_public() {
    if hive_root().is_none() {
        return;
    }
    assert!(
        !episode_survives(&[], CH1, &[A], CH1, None),
        "the empty set has no non-empty subset — an untagged row is silence, not a broadcast"
    );
    // …and the same holds when the column is absent altogether, which is what
    // every row written before this feature looks like.
    let ctx = asking(&[A], CH1, None);
    let legacy = json!({"id": "e-old", "session_id": "s-1", "sender": "user",
                        "content": "written before the gate existed",
                        "happened_at": "2026-01-01T00:00:00Z",
                        "recorded_at": "2026-01-01T00:00:00Z"});
    assert!(
        ids(&tier0_bundle(&ctx, &[legacy], &[], &[]), "episodes").is_empty(),
        "a row from before the gate has no public, and no public means nobody"
    );
}

// ============================================ the property that carries risk

/// The dangerous direction, in its own test because it is the one clause that
/// can carry a private room into a public one. `row_channel == now_channel` is
/// a CONDITION: an implementation that reads the open-history flag as "this
/// asker may see history" rather than "this ROOM may see ITS OWN history"
/// passes every other test in this file and leaks.
#[test]
fn the_open_history_clause_never_crosses_a_channel_boundary() {
    if hive_root().is_none() {
        return;
    }
    // Said between two people in a private channel. The group channel declares
    // its history open — for ITS history.
    assert!(
        !episode_survives(&[A, B], CH1, &[A, B, C], CH2, Some("1")),
        "private two-person channel -> group channel stays closed, whatever the group declares"
    );
    // The asker declaring an open history without naming a channel at all does
    // not open anything either.
    assert!(
        !episode_survives(&[A, B], CH1, &[A, B, C], "", Some("1")),
        "an empty channel matches nothing — it is not a wildcard"
    );
    // A row whose own channel is unknown cannot satisfy the clause either: the
    // rule requires `row_channel` to be truthy AND equal.
    let ctx = asking(&[A, B, C], CH1, Some("1"));
    let rows = [episode_row("e-1", "no room recorded", "", &aud(&[A, B]))];
    assert!(
        ids(&tier0_bundle(&ctx, &rows, &[], &[]), "episodes").is_empty(),
        "a row without a channel is not in every channel"
    );
    // And the clause only fires when it was ASKED for: the default is closed.
    assert!(
        !episode_survives(&[A, B], CH1, &[A, B, C], CH1, None),
        "channel_open_history defaults to 0 — silence unless the room says otherwise"
    );
    assert!(
        !episode_survives(&[A, B], CH1, &[A, B, C], CH1, Some("0")),
        "and an explicit 0 is an explicit no"
    );
}

// ==================================================== R2: the empty check first

/// Ruling R2 (contract Nachtrag 2) — the hole the first version of the rule
/// had, and the only clause whose ORDER is normative.
///
/// The open-history clause checks the room and its policy, never the public. A
/// row with an empty `audience_set` in the SAME room with the policy open would
/// therefore have passed it — so the one row whose provenance we do not know
/// would have been the one row the gate let out. The open channel says "the room
/// has shown it anyway", but for a row without a public we do not know WHETHER
/// the room ever showed it. The clause is a relaxation for rows whose provenance
/// we have, never a rescue for rows whose provenance is missing.
#[test]
fn an_untagged_row_stays_invisible_even_in_its_own_channel_with_an_open_history() {
    if hive_root().is_none() {
        return;
    }
    let ctx = asking(&[A, B, C], CH1, Some("1"));
    let rows = [episode_row(
        "e-untagged",
        "nobody knows who heard this",
        CH1,
        "[]",
    )];
    assert!(
        ids(&tier0_bundle(&ctx, &rows, &[], &[]), "episodes").is_empty(),
        "the empty-audience check runs BEFORE the open-history clause, not after it"
    );
    // The same row in the same room, with a public: now the clause may fire and
    // does. That is what makes the assertion above about ORDER rather than about
    // an open-history clause that never works.
    let rows = [episode_row(
        "e-tagged",
        "said in front of two",
        CH1,
        &aud(&[A, B]),
    )];
    assert_eq!(
        ids(&tier0_bundle(&ctx, &rows, &[], &[]), "episodes"),
        vec!["e-tagged".to_string()],
        "a row whose provenance we HAVE is what the relaxation is for"
    );
}

#[test]
fn an_untagged_row_cannot_slip_through_by_looking_universal() {
    if hive_root().is_none() {
        return;
    }
    // The order in R2 is a chain, so every later clause has to be unreachable
    // for an empty set — including the universal one. `*` that is not a JSON
    // list is not a participant set at all: it reads as the empty set, and the
    // empty set is refused first. A gate that looked for the marker in the raw
    // text, or that ran the universal clause before the empty check, would let
    // exactly this row out.
    let ctx = asking(&[A, B, C], CH1, Some("1"));
    for unreadable in ["*", "\"*\"", "[]", "", "{\"*\": true}", "not json at all"] {
        let rows = [episode_row("e-x", "unreadable provenance", CH1, unreadable)];
        assert!(
            ids(&tier0_bundle(&ctx, &rows, &[], &[]), "episodes").is_empty(),
            "audience_set {unreadable:?} is not a participant set — unreadable \
             provenance is not a licence"
        );
    }
    // …while the real thing still travels, so the loop above is a statement
    // about the empty set and not about the universal clause being broken.
    let rows = [episode_row(
        "e-star",
        "genuinely universal",
        CH1,
        &aud(&["*"]),
    )];
    assert_eq!(
        ids(&tier0_bundle(&ctx, &rows, &[], &[]), "episodes"),
        vec!["e-star".to_string()],
    );
}

// ================================================ the rule is not episode-only

#[test]
fn a_fact_is_gated_exactly_like_the_episode_it_was_extracted_from() {
    if hive_root().is_none() {
        return;
    }
    let hidden = foresight_row("f-1", "the plan", CH1, &aud(&[A, B]));
    let shown = foresight_row("f-2", "the shared plan", CH1, &aud(&["*"]));
    let bundle = tier0_bundle(
        &asking(&[A, B, C], CH1, None),
        &[],
        &[],
        &[hidden.clone(), shown],
    );
    assert_eq!(
        ids(&bundle, "foresight"),
        vec!["f-2".to_string()],
        "the private fact stays behind, the universal one travels"
    );
    let bundle = tier0_bundle(&asking(&[A, B], CH1, None), &[], &[], &[hidden]);
    assert_eq!(
        ids(&bundle, "foresight"),
        vec!["f-1".to_string()],
        "and the same circle still gets its own fact"
    );
}

// ======================================= derived rows (contract Nachtrag 1)

#[test]
fn a_belief_does_not_launder_the_facts_it_rests_on() {
    if hive_root().is_none() {
        return;
    }
    // Two facts, two different rooms full of different people: {A,B} and {A,C}.
    // The only person who could have heard BOTH is A, so the intersection — and
    // the belief's public — is {A}.
    let belief = belief_row("b-1", "anna prefers the early train", &aud(&[A]));
    for (now, channel) in [
        (vec![A, B], CH1),
        (vec![A, C], CH2),
        (vec![A, B, C], CH1),
        (vec![B], CH1),
    ] {
        let bundle = tier0_bundle(
            &asking(&now, channel, None),
            &[],
            std::slice::from_ref(&belief),
            &[],
        );
        assert!(
            ids(&bundle, "beliefs").is_empty(),
            "a belief on the intersection {{A}} must not be told to {now:?}"
        );
    }
    let bundle = tier0_bundle(&asking(&[A], CH1, None), &[], &[belief], &[]);
    assert_eq!(
        ids(&bundle, "beliefs"),
        vec!["b-1".to_string()],
        "…and A, who could have heard every source, is told"
    );
}

#[test]
fn an_empty_intersection_means_nobody_and_is_never_promoted_to_universal() {
    if hive_root().is_none() {
        return;
    }
    // Sources with no common listener: the belief is legitimately unsayable.
    let orphan = belief_row("b-empty", "derived from disjoint rooms", "[]");
    for now in [vec![A], vec![B], vec![A, B], vec![C]] {
        let bundle = tier0_bundle(
            &asking(&now, CH1, None),
            &[],
            std::slice::from_ref(&orphan),
            &[],
        );
        assert!(
            ids(&bundle, "beliefs").is_empty(),
            "an empty intersection is a legitimate result: nobody, not everybody ({now:?})"
        );
    }
    // The observable difference between "nobody" and "degraded to universal":
    // a genuinely universal belief IS told, so the assertion above is a real
    // discriminator and not an accident of an always-empty section.
    let universal = belief_row("b-star", "everyone may know this", &aud(&["*"]));
    assert_eq!(
        ids(
            &tier0_bundle(&asking(&[C], CH2, None), &[], &[universal], &[]),
            "beliefs"
        ),
        vec!["b-star".to_string()],
    );
}

#[test]
fn a_belief_has_no_channel_so_an_open_history_cannot_rescue_it() {
    if hive_root().is_none() {
        return;
    }
    // The conservative reading of Nachtrag 1: a belief was derived, not said in
    // a room. Giving it a channel to make the second clause fire would hand a
    // group everything the night derived from its members' private rooms.
    let belief = belief_row("b-1", "derived, never said", &aud(&[A]));
    for channel in [CH1, CH2, ""] {
        let bundle = tier0_bundle(
            &asking(&[A, B], channel, Some("1")),
            &[],
            std::slice::from_ref(&belief),
            &[],
        );
        assert!(
            ids(&bundle, "beliefs").is_empty(),
            "no channel on the row -> the open-history clause cannot fire (asked in {channel:?})"
        );
    }
}

/// The candidate ids one tier-1 leg wrote into `recall_scratch`.
fn t1_leg(phase: &str, leg: &str, now: &[&str], rows: Value) -> Vec<String> {
    let mut ctx = asking(now, CH1, None);
    ctx["mem_phase"] = json!(phase);
    ctx["memory_tier"] = json!("1");
    let body = json!({
        "header": {"context": ctx, "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
    });
    let insert = emitted(&run_hop("recall", &body))
        .iter()
        .map(args_of)
        .find(|a| a["table"] == json!("recall_scratch") && a["row"]["leg"] == json!(leg))
        .unwrap_or_else(|| panic!("{phase} writes the `{leg}` leg"));
    let payload: Value =
        meclaw_core::serde_json::from_str(insert["row"]["payload"].as_str().unwrap_or("[]"))
            .expect("the leg payload is JSON");
    payload
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|i| i["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[test]
fn a_tier1_leg_drops_a_hidden_row_before_the_fusion_ever_sees_it() {
    if hive_root().is_none() {
        return;
    }
    // The gate is a predicate on CANDIDATES, not a filter on rendered output: a
    // hidden row that reaches the RRF sum moves the ranking of the visible ones
    // and spends budget a visible row could have used. So each leg is asked
    // directly, with one row that may travel and one that may not.
    let eps = json!([
        {"id": "e-open", "content": "shared", "channel": CH1, "audience_set": aud(&["*"])},
        {"id": "e-shut", "content": "private", "channel": CH1, "audience_set": aud(&[A, B])},
    ]);
    assert_eq!(
        t1_leg("t1-kw-ep", "kw-ep", &[A, B, C], eps),
        vec!["e-open".to_string()],
        "the keyword leg over episodes"
    );
    let facts = json!([
        {"id": "f-open", "subject": "user", "claim": "shared", "valid_from": "2026-08-01T00:00:00Z",
         "channel": CH1, "audience_set": aud(&["*"])},
        {"id": "f-shut", "subject": "user", "claim": "private", "valid_from": "2026-08-02T00:00:00Z",
         "channel": CH1, "audience_set": aud(&[A, B])},
    ]);
    assert_eq!(
        t1_leg("t1-kw-fact", "kw-fact", &[A, B, C], facts.clone()),
        vec!["f-open".to_string()],
        "the keyword leg over facts"
    );
    assert_eq!(
        t1_leg("t1-temporal", "temporal", &[A, B, C], facts),
        vec!["f-open".to_string()],
        "the temporal leg"
    );
}

#[test]
fn an_entity_edge_is_gated_like_the_episode_it_came_from() {
    if hive_root().is_none() {
        return;
    }
    // The graph leg of tier 1. `traverse` returns paths whose `edge` carries the
    // provenance; ungated, a relationship ("who is X to me") leaks out of a
    // private channel while every episode row stays correctly hidden.
    let paths = |audience: &str| {
        json!({"paths": [{"node": "cem", "depth": 1, "weight_sum": 3,
                          "edge": {"episode_id": "e-private", "channel": CH1,
                                   "audience_set": audience}}]})
    };
    let leg = |now: &[&str], channel: &str, audience: &str| -> Vec<String> {
        let mut ctx = asking(now, channel, None);
        ctx["mem_phase"] = json!("t1-graph");
        ctx["memory_tier"] = json!("1");
        let body = json!({
            "header": {"context": ctx, "hop": {"operation": "traverse"}},
            "messages": [{"origin": "tool", "type": "tool_result",
                          "text": paths(audience).to_string()}]
        });
        let out = emitted(&run_hop("recall", &body));
        assert_eq!(
            out.len(),
            1,
            "the graph leg emits its scratch insert: {out:?}"
        );
        let insert = args_of(&out[0]);
        let payload = insert["row"]["payload"].as_str().unwrap_or("[]");
        let items: Value = meclaw_core::serde_json::from_str(payload).expect("payload is JSON");
        items
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|i| i["id"].as_str().unwrap_or_default().to_string())
            .collect()
    };
    assert!(
        leg(&[A, B, C], CH1, &aud(&[A, B])).is_empty(),
        "the edge was drawn in front of {{A,B}} — a third person does not get the relationship"
    );
    assert!(
        leg(&[A, B, C], CH1, "[]").is_empty(),
        "an untagged edge is invisible, like every untagged row"
    );
    assert_eq!(
        leg(&[A, B], CH1, &aud(&[A, B])),
        vec!["e-private".to_string()],
        "…and the circle that drew it still traverses it"
    );
}

// ==================================================== fail-closed, write path

/// One conversational turn as the drain hands it to the writer.
fn turn_body(ctx: Value) -> Value {
    json!({
        "header": {"context": ctx},
        "messages": [{"origin": "user", "type": "text", "text": "my favourite editor is Helix"}]
    })
}

fn full_write_ctx() -> Value {
    json!({"session_id": "s-1", "turn_id": "s-1#0", "channel": CH1,
           "audience_set": aud(&[A, B]), "speaker": A})
}

fn rejection_of(out: &[Value]) -> String {
    assert_eq!(
        out.len(),
        1,
        "a refusal is the ONLY thing that leaves — nothing is written on the way: {out:?}"
    );
    assert_eq!(route_of(&out[0]), "reject", "on the reject lane: {out:?}");
    for msg in out {
        assert_ne!(route_of(msg), "wstore", "nothing reaches the store");
        assert_ne!(route_of(msg), "enqueue", "and nothing reaches the queue");
    }
    out[0]["header"]["reject_reason"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn a_stored_episode_records_the_room_and_the_people_in_it() {
    if hive_root().is_none() {
        return;
    }
    let out = emitted(&run_hop("writer", &turn_body(full_write_ctx())));
    let store = out
        .iter()
        .find(|m| route_of(m) == "wstore")
        .expect("the episode insert");
    let row = &args_of(store)["row"];
    assert_eq!(row["channel"], json!(CH1), "the room is a column: {row}");
    // The set, not its spelling: the writer is free to normalise (dedupe, sort,
    // re-serialise) and a byte comparison would forbid that for no reason.
    let stored: Value = meclaw_core::serde_json::from_str(
        row["audience_set"]
            .as_str()
            .unwrap_or_else(|| panic!("audience_set is stored as text: {row}")),
    )
    .expect("audience_set is a JSON list");
    let mut members: Vec<String> = stored
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| m.as_str().unwrap_or_default().to_string())
        .collect();
    members.sort();
    assert_eq!(
        members,
        vec![A.to_string(), B.to_string()],
        "…and so is who was in it: {row}"
    );
    assert_eq!(
        row["speaker"],
        json!(A),
        "a user turn records WHO spoke, not the role: {row}"
    );
}

#[test]
fn an_episode_without_a_public_is_refused_and_writes_nothing() {
    if hive_root().is_none() {
        return;
    }
    let mut ctx = full_write_ctx();
    ctx.as_object_mut().unwrap().remove("audience_set");
    let out = emitted(&run_hop("writer", &turn_body(ctx)));
    assert_eq!(
        rejection_of(&out),
        "missing_audience",
        "the refusal names its reason: {out:?}"
    );
}

#[test]
fn an_empty_public_is_refused_like_a_missing_one() {
    if hive_root().is_none() {
        return;
    }
    // Missing or empty are the same answer: reject. A writer that stored `[]`
    // would produce a row
    // nobody can ever read — silent data loss dressed as a successful write.
    let mut ctx = full_write_ctx();
    ctx["audience_set"] = json!("[]");
    let out = emitted(&run_hop("writer", &turn_body(ctx)));
    assert_eq!(rejection_of(&out), "missing_audience", "{out:?}");
}

#[test]
fn an_episode_without_a_channel_is_refused_and_writes_nothing() {
    if hive_root().is_none() {
        return;
    }
    let mut ctx = full_write_ctx();
    ctx.as_object_mut().unwrap().remove("channel");
    let out = emitted(&run_hop("writer", &turn_body(ctx)));
    assert_eq!(
        rejection_of(&out),
        "missing_channel",
        "a missing room is its own reason, not a generic refusal: {out:?}"
    );
}

#[test]
fn the_writer_declares_the_lane_its_refusal_travels_on() {
    if hive_root().is_none() {
        return;
    }
    // A hop key the contract does not declare is rejected at the wire, so a
    // refusal the writer cannot emit is a refusal that never happens.
    let contract = cell_config("writer")["contract"].clone();
    let values: Vec<String> = contract["emits"]["hop"]["route"]["values"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        values.iter().any(|v| v == "reject"),
        "the writer emits `reject`, so it declares it: {values:?}"
    );
    assert!(
        contract["emits"]["hop"].get("reject_reason").is_some(),
        "…and the reason it travels with: {}",
        contract["emits"]["hop"]
    );
}

#[test]
fn a_writer_refusal_has_a_way_out_of_the_sealed_hive() {
    if hive_root().is_none() {
        return;
    }
    // The hive is sealed (#197): a refusal that has no edge to the hive path is
    // an unrouted dead end, and the fail-closed write becomes a silent one.
    let edges = hive_config()["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        edges.iter().any(|e| {
            e["from"] == json!("./writer")
                && e["to"] == json!(".")
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("'reject'")
        }),
        "no ./writer -> . edge for hop.route == 'reject': {edges:#?}"
    );
}

// ============================================ the inline lane writes facts too

/// One `in_remember` block at the inline ingress — the third lane the contract
/// puts the two mandatory keys on, and the only one that mints facts without an
/// episode refusing first.
fn inline_hop(ctx: Value) -> Vec<Value> {
    let block = json!({"facts": [{"subject": "user", "predicate": "Favorite Editor",
                                  "claim": "Helix", "fact_kind": "world",
                                  "confidence": 90}]});
    let body = json!({
        "header": {"context": ctx},
        "messages": [{"origin": "assistant", "type": "text", "text": block.to_string()}]
    });
    emitted(&run_hop("extract-glue", &body))
}

fn inline_ctx() -> Value {
    json!({"mem_phase": "inline", "store_origin": "inline", "session_id": "s-1",
           "channel": CH1, "audience_set": aud(&[A, B])})
}

#[test]
fn a_remembered_block_mints_facts_that_carry_the_room_and_its_people() {
    if hive_root().is_none() {
        return;
    }
    let out = inline_hop(inline_ctx());
    let stash = out
        .iter()
        .map(args_of)
        .find(|a| a["row"].get("payload").is_some())
        .expect("the inline lane stashes what it minted");
    let payload: Value =
        meclaw_core::serde_json::from_str(stash["row"]["payload"].as_str().unwrap_or("{}"))
            .expect("the stash is JSON");
    let facts = payload["facts"].as_array().cloned().unwrap_or_default();
    assert!(!facts.is_empty(), "a fact was minted: {payload}");
    for f in &facts {
        assert_eq!(f["channel"], json!(CH1), "the fact inherits the room: {f}");
        assert!(
            !f["audience_set"].as_str().unwrap_or_default().is_empty(),
            "…and the people in it: {f}"
        );
    }
}

#[test]
fn an_inline_block_without_a_public_is_refused_before_it_is_even_read() {
    if hive_root().is_none() {
        return;
    }
    // This ingress mints facts DIRECTLY. A block without provenance is the one
    // path on which an untagged row could reach the store without an episode
    // refusing it first, so the refusal comes before the payload is parsed.
    for (drop_key, reason) in [
        ("audience_set", "missing_audience"),
        ("channel", "missing_channel"),
    ] {
        let mut ctx = inline_ctx();
        ctx.as_object_mut().unwrap().remove(drop_key);
        let out = inline_hop(ctx);
        assert_eq!(out.len(), 1, "one refusal and nothing else: {out:?}");
        assert_eq!(route_of(&out[0]), "reject", "{out:?}");
        assert_eq!(out[0]["header"]["reject_reason"], json!(reason), "{out:?}");
        assert!(
            out.iter().all(|m| route_of(m) != "xstore"),
            "the refusal is write-free: {out:?}"
        );
    }
}

// ===================================================== fail-closed, read path

/// A fresh `in_query` at the hive door: the port edge stamps `phase: "recall"`
/// on the hop, and no store answer has come back yet.
fn request(ctx: Value) -> Value {
    json!({"header": {"context": ctx, "hop": {"phase": "recall"}}, "messages": []})
}

#[test]
fn a_question_without_a_public_is_refused_rather_than_answered_emptily() {
    if hive_root().is_none() {
        return;
    }
    // The distinction this test exists for: an empty bundle SAYS "memory knows
    // nothing about this", a refusal says "I will not answer without knowing
    // who is listening". A caller cannot tell them apart from the outside, so
    // the lane must not conflate them.
    let mut ctx = asking(&[A], CH1, None);
    ctx.as_object_mut().unwrap().remove("audience_now");
    let out = emitted(&run_hop("recall", &request(ctx)));
    assert_eq!(out.len(), 1, "one refusal, no leg fan-out: {out:?}");
    assert_eq!(route_of(&out[0]), "reject", "{out:?}");
    assert_eq!(
        out[0]["header"]["reject_reason"],
        json!("missing_audience"),
        "{out:?}"
    );
    assert!(
        out[0]["system"]["memory"]["bundle"].is_null(),
        "a refusal is not a bundle: {out:?}"
    );
    for msg in &out {
        assert_ne!(
            route_of(msg),
            "rstore",
            "and the store is never even asked: {out:?}"
        );
    }
}

#[test]
fn a_question_without_a_channel_is_refused_too() {
    if hive_root().is_none() {
        return;
    }
    // R3 (Nachtrag 2): the read path names the SAME reason the write path does.
    // A missing room is `missing_channel` on both sides — the read path's old
    // `missing_audience` said something that was not true about the request and
    // would have been "corrected" by whoever read it next.
    for ctx in [absent_channel(), empty_channel()] {
        let out = emitted(&run_hop("recall", &request(ctx)));
        assert_eq!(out.len(), 1, "one refusal, no leg fan-out: {out:?}");
        assert_eq!(route_of(&out[0]), "reject", "{out:?}");
        assert_eq!(
            out[0]["header"]["reject_reason"],
            json!("missing_channel"),
            "{out:?}"
        );
    }
}

/// The question with its `channel` key removed…
fn absent_channel() -> Value {
    let mut ctx = asking(&[A], CH1, None);
    ctx.as_object_mut().unwrap().remove("channel");
    ctx
}

/// …and the question that sends the key empty, which is the same absence
/// wearing a different shape.
fn empty_channel() -> Value {
    let mut ctx = asking(&[A], CH1, None);
    ctx["channel"] = json!("");
    ctx
}

#[test]
fn a_question_that_names_its_public_is_answered_as_before() {
    if hive_root().is_none() {
        return;
    }
    // The guard must refuse the caller who says nothing, never the caller who
    // says everything — otherwise the gate is a denial of service.
    let out = emitted(&run_hop("recall", &request(asking(&[A, B], CH1, None))));
    assert!(
        out.iter().all(|m| route_of(m) != "reject"),
        "a complete request is not a refusal: {out:?}"
    );
    assert!(
        out.iter().any(|m| route_of(m) == "rstore"),
        "the leg fan-out happens: {out:?}"
    );
}

// ====================================== the filter reads what the store sent

#[test]
fn the_read_path_selects_the_columns_it_is_going_to_filter_on() {
    if hive_root().is_none() {
        return;
    }
    // A gate over a column nobody selected never fires — or, fail-closed, hides
    // everything. Both are silent failures, so the request's own SELECTs are
    // pinned here.
    let out = emitted(&run_hop("recall", &request(asking(&[A, B], CH1, None))));
    let mut seen = 0;
    for msg in &out {
        let a = args_of(msg);
        let Some(table) = a["table"].as_str() else {
            continue;
        };
        if !matches!(table, "episodes" | "facts") {
            continue;
        }
        let cols: Vec<String> = a["columns"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|c| c.as_str().unwrap_or_default().to_string())
            .collect();
        seen += 1;
        for want in ["audience_set", "channel"] {
            assert!(
                cols.iter().any(|c| c == want),
                "the {table} leg does not ask for `{want}`: {cols:?}"
            );
        }
    }
    assert!(seen >= 2, "tier 0 asks episodes and facts: {out:?}");
}

#[test]
fn the_graph_leg_traverses_for_the_provenance_it_has_to_check() {
    if hive_root().is_none() {
        return;
    }
    let mut ctx = asking(&[A, B], CH1, None);
    ctx["mem_phase"] = json!("t1-anchor");
    ctx["memory_tier"] = json!("1");
    let body = json!({
        "header": {"context": ctx, "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "text": json!([{"id": "ent-1", "canonical_name": "cem"}]).to_string()}]
    });
    let out = emitted(&run_hop("recall", &body));
    let traverse = out
        .iter()
        .map(args_of)
        .find(|a| a["operation"] == json!("traverse"))
        .expect("the graph leg traverses");
    let cols: Vec<String> = traverse["columns"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| c.as_str().unwrap_or_default().to_string())
        .collect();
    for want in ["episode_id", "audience_set", "channel"] {
        assert!(
            cols.iter().any(|c| c == want),
            "traverse does not carry `{want}` out of the edge: {cols:?}"
        );
    }
}

// ============================================ the derived write side

/// Drives the night's belief write for one belief resting on `sources`, with
/// `facts` as the rows the store holds for them, and returns the public the
/// lane wrote onto the belief.
///
/// Two hops, and the second is fed from the FIRST: whatever bookkeeping row the
/// lane builds out of the fact rows is handed straight back to the apply phase.
/// The test therefore never has to know the key or the shape the lane chose —
/// only that the RESULT is the intersection, which is the part the contract
/// fixes.
fn belief_audience(sources: &[&str], facts: &[(&str, &str)]) -> Vec<String> {
    let rows: Vec<Value> = facts
        .iter()
        .map(|(id, audience)| json!({"id": id, "audience_set": audience}))
        .collect();
    let gather = json!({
        "header": {"context": {"mem_phase": "belief-audience-park", "dream_run": "run-1",
                               "dream_to": "2026-08-19T03:00:00Z"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "text": Value::Array(rows).to_string()}]
    });
    let carried = emitted(&run_hop("dream-glue", &gather))
        .iter()
        .map(args_of)
        .find(|a| a["operation"] == json!("insert") && a["row"].get("payload").is_some())
        .map(|a| json!({"kind": a["row"]["kind"], "payload": a["row"]["payload"]}))
        .expect("the night carries the publics of the facts it read");

    let verdicts = json!({"beliefs": [{"holder": "self", "statement": "the derived claim",
                                       "confidence": 80, "active": true,
                                       "source_fact_ids": sources}]});
    let scratch = json!([
        {"kind": "verdicts", "payload": verdicts.to_string()},
        {"kind": "beliefs", "payload": "[]"},
        carried,
    ]);
    let apply = json!({
        "header": {"context": {"mem_phase": "apply-run", "dream_run": "run-1",
                               "dream_to": "2026-08-19T03:00:00Z"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}]
    });
    let insert = emitted(&run_hop("dream-glue", &apply))
        .iter()
        .map(args_of)
        .find(|a| a["table"] == json!("beliefs") && a["operation"] == json!("insert"))
        .expect("the belief is written");
    let stored: Value = meclaw_core::serde_json::from_str(
        insert["row"]["audience_set"]
            .as_str()
            .unwrap_or_else(|| panic!("a belief carries a public: {insert}")),
    )
    .expect("the public is a JSON list");
    let mut out: Vec<String> = stored
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| m.as_str().unwrap_or_default().to_string())
        .collect();
    out.sort();
    out
}

#[test]
fn a_derived_belief_is_told_only_to_the_intersection_of_its_sources() {
    if hive_root().is_none() {
        return;
    }
    // The laundry, in its purest form: two facts learned in front of two
    // DIFFERENT circles. Only A could have heard both, so only A may be told
    // the claim that rests on both — not {A,B}, not {A,C}, and above all not
    // their union, which is how two private facts become one shareable one.
    assert_eq!(
        belief_audience(
            &["f-ab", "f-ac"],
            &[("f-ab", &aud(&[A, B])), ("f-ac", &aud(&[A, C]))]
        ),
        vec![A.to_string()],
    );
    // A universal source cuts nothing away: it was sayable to anyone, so it
    // constrains nobody.
    assert_eq!(
        belief_audience(
            &["f-star", "f-ab"],
            &[("f-star", &aud(&["*"])), ("f-ab", &aud(&[A, B]))]
        ),
        vec![A.to_string(), B.to_string()],
    );
}

#[test]
fn a_source_the_night_cannot_see_contributes_nobody_rather_than_anybody() {
    if hive_root().is_none() {
        return;
    }
    // "Missing" is not "irrelevant". A source the lane could not read makes the
    // intersection empty, and an empty intersection is a legitimate result: the
    // belief is sayable to nobody until its provenance is readable again.
    assert!(
        belief_audience(&["f-ab", "f-gone"], &[("f-ab", &aud(&[A, B]))]).is_empty(),
        "a source that is not there contributes the empty set"
    );
    assert!(
        belief_audience(
            &["f-ab", "f-bare"],
            &[("f-ab", &aud(&[A, B])), ("f-bare", "")]
        )
        .is_empty(),
        "and so does a source that carries no public of its own"
    );
}

#[test]
fn the_night_never_writes_a_belief_without_saying_who_may_hear_it() {
    if hive_root().is_none() {
        return;
    }
    // The write half of the laundry rule. The contract fixes the RESULT (a
    // belief's public is the intersection of its sources') but not the hop on
    // which the lane learns its sources' publics — so this pins the property
    // that holds under every mechanism: whatever the apply phase writes into
    // `beliefs`, it never writes one without a public. A lane that still has to
    // look the sources up emits no belief op here at all and passes; today's
    // lane emits the insert bare and fails.
    let verdicts = json!({"beliefs": [{"holder": "self",
                                       "statement": "anna prefers the early train",
                                       "confidence": 80, "active": true,
                                       "source_fact_ids": ["f-ab", "f-ac"]}]});
    let scratch = json!([
        {"kind": "verdicts", "payload": verdicts.to_string()},
        {"kind": "beliefs", "payload": "[]"},
    ]);
    let body = json!({
        "header": {"context": {"mem_phase": "apply-run", "dream_run": "run-1",
                               "dream_to": "2026-08-19T03:00:00Z"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": scratch.to_string()}]
    });
    let out = emitted(&run_hop("dream-glue", &body));
    for msg in &out {
        let a = args_of(msg);
        if a["table"] != json!("beliefs") {
            continue;
        }
        let carrier = if a["operation"] == json!("insert") {
            &a["row"]
        } else {
            &a["set"]
        };
        // A retraction only flips `active`; it is not a new statement and
        // carries no new public.
        if carrier.get("statement").is_none() && a["operation"] == json!("update") {
            continue;
        }
        assert!(
            carrier.get("audience_set").is_some(),
            "a belief written without a public would be readable by anyone the \
             read path lets through: {a}"
        );
        assert_ne!(
            carrier["audience_set"],
            json!("[\"*\"]"),
            "and an unknown intersection is never degraded to universal: {a}"
        );
    }
}

// ==================================================== the columns themselves

#[test]
fn the_store_carries_a_public_on_every_table_that_can_leak_one() {
    if hive_root().is_none() {
        return;
    }
    let schema = cell_config("store")["params"]["schema"].clone();
    let has = |table: &str, col: &str| !schema[table][col].is_null();
    for (table, col) in [
        ("episodes", "channel"),
        ("episodes", "audience_set"),
        ("episodes", "speaker"),
        ("facts", "channel"),
        ("facts", "audience_set"),
        // Nachtrag 1 — the derived tables, which is where the first version of
        // the contract had its hole.
        ("beliefs", "audience_set"),
        ("entity_edges", "channel"),
        ("entity_edges", "audience_set"),
        ("skills", "audience_set"),
    ] {
        assert!(has(table, col), "{table} has no `{col}` column");
    }
    // `entities` deliberately gets none: an entity is reached through edges and
    // facts, and those are filtered. A column here would be a second, weaker
    // gate on the same question.
    assert!(
        schema["entities"]["audience_set"].is_null(),
        "entities is reached THROUGH gated rows and carries no gate of its own"
    );
}
