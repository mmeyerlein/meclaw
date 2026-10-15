//! GH #490 — the second and third capability questions of a submission can be
//! answered, and the identity door is committed through the submission front.
//!
//! `submit/gate` keeps TWO round trips it does not control: one to the store
//! beside it, which is its whole memory, and one to the capability broker,
//! which is its whole policy. Both used to be recognised out of the same key
//! space. The store answer is marked by `context.sub_origin`/`sub_phase`,
//! promoted by the hive's own interior edge — and a cell emission inherits the
//! context it was handling, so a question asked from INSIDE the un-parking
//! branch went out wearing the store's marker, and the verdict came back
//! wearing it too. The gate then read a BROKER VERDICT as a STORE ROW, found no
//! row in it, and reported the manifest as lost: `submission_check_failed`
//! after an `allowed`, with the row still parked and nothing deleted.
//!
//! Structurally, not by a missed case: question 1 is asked while the phase is
//! `written`, which is not in the read set, so its verdict falls through
//! correctly. Every later question is asked from a phase that IS in the read
//! set, and there is no phase value it could carry that is not — so any
//! question after the first was unanswerable, and `affinity.subscribe` (GH
//! #458, GH #479) could only ever be grown through `/colony/mutations`, which
//! asks nobody.
//!
//! Two rules hold it now, each sufficient alone, and they are the same pair
//! `access` needed one level down in GH #481:
//!
//! * the hive's three exit edges clear `sub_origin`/`sub_phase`/`sub_carry`
//!   with `delete_context`, so an interior marker never leaves the hive; and
//! * a delivery on `hop.route == 'in_verdict'` is a VERDICT whatever the
//!   context carries — a store answer has no `hop.route` at all.
//!
//! And the correlation itself stops being a phase: the ask mints its own id
//! (`ask.<capability>.<nonce>`), the broker echoes it on the `tool_result`, and
//! the question a verdict belongs to is read off the broker's own `capability`
//! first and off that id second. A phase says what has been BOOKED on the
//! parked row; it never says which question was put.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};

const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/config.json"
);
const GATE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/gate/config.json"
);
const README: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/submit/README.md"
);

/// The brain, as the substrate stamps it onto the envelope it emitted.
const REQUESTER: &str = "/os/orgs/acme/members/alex/talky";
/// The affinity hive beside it. The HIVE path: `affinity`'s `params.ports` is
/// empty, so an edge naming `./push` is refused with `hive_port_boundary`.
const AFFINITY: &str = "/os/orgs/acme/members/alex/affinity";

/// The three interior keys the hive promotes on the way INTO its store.
const INTERIOR: [&str; 3] = ["sub_origin", "sub_phase", "sub_carry"];

fn read(path: &str) -> Value {
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(path).expect(path)).expect(path)
}

/// The canonical digest of a declaration list, drawn by the same two lines the
/// shipped helper draws it with.
fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

/// The one operation a store message carries, parsed out of its `tool_call`.
fn op_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

/// The arguments of the question a message on the `ask` lane carries.
fn question_of(msg: &Value) -> Value {
    op_of(msg)
}

/// The `tool_call` id of an ask — the ask's own name for the question it puts.
fn ask_id_of(msg: &Value) -> String {
    msg["messages"][0]["id"]
        .as_str()
        .expect("a tool_call id")
        .to_string()
}

/// One declaration that needs ALL THREE questions: it mutates a scope, it hands
/// a `code` cell a script nobody reviewed, and it draws an `in_pack` edge into
/// the requester's own hive.
fn three_capability_manifest() -> Value {
    json!([{
        "scope": "/os/orgs/acme",
        "ctx": {},
        "diff": {
            "add_nodes": [{
                "name": "./members/alex/watch",
                "template": "argus",
                "override_params": { "script_inline": "print('mine')" }
            }],
            "add_edges": [{
                "from": AFFINITY,
                "to": REQUESTER,
                "condition": "has(hop.subscriber) && hop.subscriber == 'sub:alex-self'",
                "modifier": { "set_hop": { "route": "'in_pack'" } }
            }]
        }
    }])
}

/// The same manifest without the script: two questions, not three.
fn subscribe_only_manifest() -> Value {
    json!([{
        "scope": "/os/orgs/acme",
        "ctx": {},
        "diff": {
            "add_edges": [{
                "from": AFFINITY,
                "to": REQUESTER,
                "condition": "has(hop.subscriber) && hop.subscriber == 'sub:alex-self'",
                "modifier": { "set_hop": { "route": "'in_pack'" } }
            }]
        }
    }])
}

/// Phase A: a submission arriving on `in_apply`.
fn submit(decls: &Value) -> Vec<Value> {
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "reply_to": REQUESTER,
            "header": { "hop": { "route": "in_apply",
                                 "manifest_sha256": digest_of(decls),
                                 "tool_call_id": "c2" }, "context": {} },
            "ttl": 64,
            "manifest": decls,
            "messages": [{ "origin": "assistant", "type": "tool_call",
                           "id": "c2", "text": "{}" }],
            "params": {}
        }),
    )
}

/// A broker verdict, on the lane the shell re-stamps it onto, with the context
/// it really arrives with: `sub_ask`/`sub_sha` from the composition's ask edge,
/// plus whatever `stale` puts back into the submitter's own key space.
///
/// `stale` is the whole of GH #490: before the fix an ask emitted from inside
/// the un-parking branch carried `sub_origin=gate, sub_phase=parked` out of the
/// hive, the broker's edges preserved the `sub_*` key space by design, and the
/// verdict came back wearing the store's marker.
fn verdict_with(
    capability: &str,
    status: &str,
    sha: &str,
    echo: &str,
    stale: Option<&str>,
) -> Vec<Value> {
    let mut context = json!({ "sub_ask": "1", "sub_sha": sha });
    if let Some(phase) = stale {
        context["sub_origin"] = json!("gate");
        context["sub_phase"] = json!(phase);
        context["sub_carry"] = json!("{\"status\":\"allowed\"}");
    }
    let mut payload = json!({ "status": status, "reason_code": "" });
    if !capability.is_empty() {
        payload["capability"] = json!(capability);
    }
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": { "hop": { "route": "in_verdict" }, "context": context },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": echo,
                           "text": payload.to_string() }],
            "params": {}
        }),
    )
}

/// The store's answer to an un-parking `select`, in one named phase.
fn unpark(phase: &str, decls: &Value) -> Vec<Value> {
    let rows = json!([{ "id": "p1", "manifest": decls, "requester": REQUESTER,
                        "tool_call_id": "c2", "manifest_sha256": digest_of(decls) }]);
    emit_all(
        &shipped_script(GATE),
        &json!({
            "target": "/os/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": phase,
                             "sub_carry": "{\"status\":\"allowed\"}" }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

// ── the red probe ────────────────────────────────────────────────────────────

/// The measured failure of GH #490, in one message: an `allowed` verdict that
/// arrives still wearing the store's marker.
///
/// Before the fix this produced a single `receipt` with
/// `submission_check_failed` — the gate read the verdict body as the row it was
/// meant to un-park, found no row in it, and declared a manifest lost that was
/// still parked. The lane is what decides, so it is the un-parking `select`
/// that goes out.
#[test]
fn a_verdict_wearing_the_store_marker_is_still_a_verdict() {
    let decls = three_capability_manifest();
    let sha = digest_of(&decls);
    let out = verdict_with(
        "code.author",
        "allowed",
        &sha,
        "ask.code-author.deadbeef",
        Some("parked"),
    );

    assert_eq!(out.len(), 1, "one message: the un-parking read");
    assert_eq!(
        out[0]["header"]["route"], "sstore",
        "a verdict is answered by reading the row, never by reporting it lost"
    );
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "submissions");
    assert_eq!(op["where"]["kind"], "parked");
    assert_eq!(op["where"]["manifest_sha256"], sha.as_str());
    assert_eq!(
        out[0]["header"]["phase"], "authored",
        "the phase the ANSWER books, derived from the capability that was answered"
    );
    for m in &out {
        assert_ne!(
            m["header"]["error_code"], "submission_check_failed",
            "the row is parked and nobody looked in the store"
        );
    }
}

// ── three questions, in sequence, over one row and one digest ────────────────

/// The whole chain, driven message by message: three questions, three verdicts,
/// one parked row, and the identity door reaching `mutate` at the end.
#[test]
fn three_questions_are_asked_and_answered_in_sequence() {
    let decls = three_capability_manifest();
    let sha = digest_of(&decls);

    // 1. Phase A parks the manifest and puts the FIRST question.
    let out = submit(&decls);
    assert_eq!(out.len(), 2, "park, then ask");
    assert_eq!(op_of(&out[0])["row"]["kind"], "parked");
    assert_eq!(out[1]["header"]["route"], "ask");
    assert_eq!(question_of(&out[1])["capability"], "colony.mutate");
    let first = ask_id_of(&out[1]);

    // 2. Its verdict un-parks by digest, into the phase that question books.
    let out = verdict_with("colony.mutate", "allowed", &sha, &first, None);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["phase"], "parked");

    // 3. The SECOND question, asked from inside the un-parking branch. Nothing
    //    is un-parked while a question about the manifest is open.
    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 1, "one question, and the row left where it is");
    assert_eq!(out[0]["header"]["route"], "ask");
    assert_eq!(question_of(&out[0])["capability"], "code.author");
    assert_eq!(question_of(&out[0])["check_only"], true);
    assert_eq!(question_of(&out[0])["subject"], REQUESTER);
    assert_eq!(out[0]["header"]["manifest_sha256"], sha.as_str());
    let second = ask_id_of(&out[0]);

    // 4. And its verdict is answerable — the defect this file is named after.
    let out = verdict_with("code.author", "allowed", &sha, &second, Some("parked"));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "sstore");
    assert_eq!(out[0]["header"]["phase"], "authored");

    // 5. The THIRD question, from the phase the second answer booked.
    let out = unpark("authored", &decls);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "ask");
    assert_eq!(question_of(&out[0])["capability"], "affinity.subscribe");
    let third = ask_id_of(&out[0]);

    // 6. Answerable too, and it books the last phase.
    let out = verdict_with(
        "affinity.subscribe",
        "allowed",
        &sha,
        &third,
        Some("authored"),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["phase"], "subscribing");

    // 7. Every question answered: the park is forgotten, the flight is
    //    remembered, and the identity door goes out on `mutate` — through the
    //    submission front, not through `/colony/mutations`.
    let out = unpark("subscribing", &decls);
    assert_eq!(out.len(), 3, "forget the park, remember the flight, submit");
    assert_eq!(op_of(&out[0])["operation"], "delete");
    assert_eq!(op_of(&out[1])["row"]["kind"], "flight");
    assert_eq!(out[2]["header"]["route"], "mutate");
    assert_eq!(out[2]["header"]["manifest_sha256"], sha.as_str());
    assert_eq!(out[2]["header"]["requester"], REQUESTER);
    let door = &out[2]["manifest"][0]["diff"]["add_edges"][0];
    assert_eq!(
        door["modifier"]["set_hop"]["route"], "'in_pack'",
        "the identity door is committed through the submission front"
    );

    // Three ids, three questions, and no two the same.
    assert!(first.starts_with("ask.colony-mutate."));
    assert!(second.starts_with("ask.code-author."));
    assert!(third.starts_with("ask.affinity-subscribe."));
    assert_ne!(first, second);
    assert_ne!(second, third);
}

/// A manifest that only subscribes asks two questions and skips the one it does
/// not owe: the sequence is derived from the diff, never from a phase count.
#[test]
fn a_manifest_asks_only_the_questions_its_diff_owes() {
    let decls = subscribe_only_manifest();
    let out = unpark("parked", &decls);
    assert_eq!(out.len(), 1);
    assert_eq!(
        question_of(&out[0])["capability"],
        "affinity.subscribe",
        "no script in the diff, so `code.author` is never asked"
    );

    // And a plain manifest asks nothing more at all.
    let plain = json!([{ "scope": "/os/orgs/acme", "ctx": {}, "diff": { "add_edges": [] } }]);
    let out = unpark("parked", &plain);
    assert_eq!(out.len(), 3, "straight to the door");
    assert_eq!(out[2]["header"]["route"], "mutate");
}

// ── the correlation is the ask's own id, never a phase ───────────────────────

/// A broker that answers `allowed` and echoes the id but not the capability is
/// still understood: the id names the question.
///
/// This is what makes the sequence safe without trusting a second template's
/// prose. Reading the phase instead is what GH #490 measured, and guessing "the
/// one this cell has always asked" would send the second question round again,
/// for ever.
#[test]
fn the_ask_names_its_own_question() {
    let decls = three_capability_manifest();
    let sha = digest_of(&decls);

    let out = verdict_with(
        "",
        "allowed",
        &sha,
        "ask.affinity-subscribe.0badcafe",
        Some("authored"),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0]["header"]["phase"], "subscribing",
        "the id said which question this answers"
    );

    // And a denial is classified by the same reading.
    let out = verdict_with(
        "",
        "denied",
        &sha,
        "ask.code-author.0badcafe",
        Some("parked"),
    );
    let refusal = out
        .iter()
        .find(|m| m["header"]["route"] == "receipt")
        .expect("a receipt");
    assert_eq!(refusal["header"]["error_code"], "code_author_denied");

    // A verdict from a broker that echoes neither is still the one question
    // every submission asks — the historical default, kept.
    let out = verdict_with("", "denied", &sha, "q1", None);
    let refusal = out
        .iter()
        .find(|m| m["header"]["route"] == "receipt")
        .expect("a receipt");
    assert_eq!(refusal["header"]["error_code"], "requester_not_permitted");
}

// ── the form: an interior marker never leaves the hive ───────────────────────

/// Every edge that leaves `./gate` for the hive rim clears the three interior
/// keys, and the README says so.
///
/// The drift lock of `docs/development-rules.md` § 2d: the sentence is grepped
/// AND the mechanism is asserted, because a sentence alone pins a string and a
/// mechanism alone lets the prose walk away from it.
#[test]
fn the_exit_edges_clear_the_interior_markers() {
    let hive = read(HIVE);
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive declares its edges");

    let exits: Vec<&Value> = edges
        .iter()
        .filter(|e| e["from"] == "./gate" && e["to"] == ".")
        .collect();
    assert_eq!(exits.len(), 3, "mutate, receipt and ask leave the hive");
    for edge in exits {
        let cleared = edge["modifier"]["delete_context"]
            .as_array()
            .unwrap_or_else(|| panic!("{} has no delete_context", edge["condition"]));
        for key in INTERIOR {
            assert!(
                cleared.iter().any(|k| k == key),
                "{} must clear {key}",
                edge["condition"]
            );
        }
    }

    // The edge INTO the store is the one that sets them, and it still does.
    let into_store = edges
        .iter()
        .find(|e| e["from"] == "./gate" && e["to"] == "./store")
        .expect("the store edge");
    for key in INTERIOR {
        assert!(
            into_store["modifier"]["set_context"][key].is_string(),
            "the interior marker {key} is still promoted on the way in"
        );
    }

    let readme = std::fs::read_to_string(README).expect(README);
    assert!(
        readme.contains("delete_context"),
        "the README names the mechanism that holds the two round trips apart"
    );
    assert!(
        readme.contains("GH #490"),
        "the README records the correction, so a reader can tell a promise from a measurement"
    );
}

/// The gate reads the verdict lane positively, and the store answer only where
/// there is no lane. Either rule alone closes GH #490; the script carries the
/// second one so a composition cannot re-open it by forgetting an edge.
#[test]
fn the_script_tells_the_two_round_trips_apart_by_the_lane() {
    let gate = read(GATE);
    let script = gate["params"]["script_inline"].as_str().expect("a script");
    assert!(
        script.contains("verdict_lane = str(hop.get(\"route\") or \"\") == \"in_verdict\""),
        "the lane is computed once"
    );
    assert!(
        script.contains(
            "if not verdict_lane and str(context.get(\"sub_origin\") or \"\") == \"gate\":"
        ),
        "and a store marker is only read where the message is not a verdict"
    );
}
