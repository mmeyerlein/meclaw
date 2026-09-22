//! Welle Live (GH #784) -- the `in_delegation` lane: the voice model hands
//! backend work to `talky`, and what it hands over never becomes a memory.
//!
//! WHAT A DELEGATION IS. In a duplex call the model decides by itself when it
//! needs the backend -- tools, recall, the reasoning core -- and says so with
//! `session.delegation.created`, which carries an id and no task text. The
//! `voice` cell turns that into one arrival on `in_delegation`. It is assembled
//! exactly like `in_advice`: an EVENT that opens a fresh round of its own,
//! because the turn it belongs to is on the model's clock, not on ours.
//!
//! WHY IT MUST NOT DRAIN. The row reads like a sentence and is none: nobody in
//! this conversation said it. `SAID` names the two roles that are something
//! somebody said, `drainable_turns` refuses everything else, and both writers
//! -- the close lane's batch and the per-turn episode lane -- ask there. A
//! delegation drained into the memory would be quoted back to the caller as her
//! own words, which is the defect GH #282 measured on `advice` rows.
//!
//! The form is `collector_window.rs`: the SHIPPED `params.script_inline` runs
//! under `python3` over a real stdin document, so what is measured here is what
//! ships and nothing is mocked.

const ASSEMBLE_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);

/// The frame the row wears on the wire. The wire role stays `user` -- the only
/// inbound role every provider accepts mid-conversation -- so the frame is what
/// says the row is not the caller's word (the GH #540 repair, applied to the
/// second event lane).
const FRAME: &str = "[delegation d-7 from the voice channel: the caller expects \
                     backend work; last heard: book me a table for two]";

fn assemble_config() -> serde_json::Value {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    serde_json::from_str(&raw).expect("config json")
}

fn assemble_params() -> serde_json::Value {
    let mut p = assemble_config()["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    serde_json::Value::Object(p)
}

fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut doc = doc;
    doc["params"] = assemble_params();
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(ASSEMBLE_CONFIG),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "assemble exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One leg of a bundle message, by its `tool_call_id`.
fn leg_of(msg: &serde_json::Value, id: &str) -> serde_json::Value {
    let turn = msg["messages"]
        .as_array()
        .expect("bundle turns")
        .iter()
        .find(|t| t["id"] == id)
        .unwrap_or_else(|| panic!("no leg `{id}` in {msg}"));
    serde_json::from_str(turn["text"].as_str().expect("leg text")).expect("leg json")
}

/// The delegation as the member's direct edge delivers it: the lane on the hop,
/// the id in context. It does NOT come through the firewall, whose exit stamps
/// `in_turn` -- a delegation is not a turn of the conversation.
fn delegation_doc() -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "channel_node": "voice", "engine": "duplex",
                               "delegation_id": "d-7"},
                   "hop": {"route": "in_delegation", "delegation_id": "d-7"}},
        "messages": [{"origin": "user", "type": "text",
                      "text": "book me a table for two"}]
    })
}

/// A store reply on a phase of this hive's own chain.
fn reply(phase: &str, op: &str, payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": op, "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": payload.to_string()}]
    })
}

#[test]
fn a_delegation_is_written_as_its_own_role_under_its_own_id() {
    let out = emit(delegation_doc());

    assert_eq!(
        out[0]["header"]["phase"], "turn-open",
        "the SAME chain as a turn and as an advice: {out:?}"
    );
    let row = leg_of(&out[0], "c-open-turn")["row"].clone();
    assert_eq!(
        row["role"], "delegation",
        "neither the caller's word nor the agent's own"
    );
    assert_eq!(row["content"], "book me a table for two");
    assert_eq!(
        row["consult_id"], "d-7",
        "the id the model will hear its answer under: {row}"
    );
    let minted = out[0]["header"]["turn_id"].as_str().expect("turn_id");
    assert!(
        !minted.is_empty() && minted != "t1",
        "a FRESH round: the turn that asked is on the model's clock and over"
    );
    assert_eq!(row["turn_id"], minted);
}

#[test]
fn a_delegation_opens_its_round_with_the_whole_budget() {
    // GH #541, the same reason `in_advice` was named there: a lane that opens a
    // round must not inherit the counter of the round it arrived out of.
    let mut doc = delegation_doc();
    doc["header"]["context"]["iter"] = serde_json::json!("9");
    let out = emit(doc);

    assert_eq!(
        out[0]["header"]["iter"], "0",
        "a spent budget somewhere else is not this round's: {out:?}"
    );
}

#[test]
fn the_delegation_says_on_the_wire_what_it_is() {
    let turns = serde_json::json!([
        {"role": "user", "text": "hello"},
        {"role": "delegation", "text": "book me a table for two",
         "consult_id": "d-7"}
    ]);
    let payload = serde_json::json!({"turns": turns, "bytes": 0, "dropped": 0, "capped": 0});
    let rows = serde_json::json!([{"turn_id": "t1", "iter": 0, "role": "leg-window",
                                   "turn": payload.to_string(), "fired": 0}]);
    let doc = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "collect", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-collect-read",
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": "c-collect-read", "operation": "select",
                     "rows_affected": 1, "duration_ms": 0}]
    });
    let out = emit(doc);

    assert_eq!(out[0]["header"]["route"], "brain");
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(
        msgs[1]["origin"], "user",
        "inbound on the wire -- the two roles a provider knows"
    );
    assert_eq!(
        msgs[1]["text"], FRAME,
        "and the frame says which of the two it is, names the id the answer \
         travels under and quotes what was last heard: {}",
        msgs[1]
    );
}

#[test]
fn a_delegation_becomes_no_episode() {
    // The per-turn write path (GH #298): one message per turn of the session
    // that has not been written yet. A delegation is not one.
    let rows = serde_json::json!([
        {"id": "r1", "role": "user", "content": "hello", "interim": 0,
         "recorded_at": "r1", "episode_written": 0},
        {"id": "r2", "role": "delegation", "content": "book me a table for two",
         "interim": 0, "recorded_at": "r2", "episode_written": 0}
    ]);
    let out = emit(reply("tw-scan", "select", rows));

    let episodes: Vec<&serde_json::Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "turn_write")
        .collect();
    assert_eq!(
        episodes.len(),
        1,
        "one episode, for the one thing somebody said: {out:?}"
    );
    assert_eq!(episodes[0]["messages"][0]["text"], "hello");
    let whole = serde_json::to_string(&out).expect("emission");
    assert!(
        !whole.contains("r2"),
        "and the row nobody said is not even marked as written: {out:?}"
    );
}

#[test]
fn a_delegation_is_no_part_of_the_days_batch() {
    // The other writer, the close lane: the session leaves as ONE batch, and
    // both writers ask `drainable_turns` -- so what may become a memory is
    // decided in one place and stays decidable there.
    let turns = serde_json::json!([
        {"role": "user", "text": "hello", "interim": 0},
        {"role": "delegation", "text": "book me a table for two", "interim": 0},
        {"role": "assistant", "text": "a table for two, then", "interim": 0}
    ]);
    let rows = serde_json::json!([
        {"turn_id": "close-s1", "iter": 0, "role": "leg-close",
         "turn": turns.to_string(), "fired": 0, "recorded_at": "b"}
    ]);
    let doc = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "close-s1|b", "iter": "0",
                               "col_phase": "close-fire", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-close-read",
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": "c-close-read", "operation": "select",
                     "rows_affected": 1, "duration_ms": 0}]
    });
    let out = emit(doc);

    assert_eq!(out[0]["header"]["route"], "write");
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(
        msgs.len(),
        2,
        "the caller's word and the agent's own, and nothing between them: {}",
        out[0]
    );
    assert_eq!(msgs[0]["text"], "hello");
    assert_eq!(msgs[1]["text"], "a table for two, then");
    assert_eq!(out[0]["header"]["turn_count"], "2");
}

#[test]
fn the_lane_is_named_in_the_shipped_contract() {
    // The door and the lane are one statement (`gh173_shipped_hive_contracts`
    // checks the tree-wide version of this): a lane the script knows and the
    // contract does not is a lane no edge may stamp.
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../templates/collector/config.json"
    ))
    .expect("collector config");
    let cfg: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    let accepts = cfg["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts");
    let entry = accepts
        .iter()
        .find(|a| a["route"] == "in_delegation")
        .unwrap_or_else(|| panic!("no `in_delegation` in the collector's accepts: {accepts:?}"));
    assert!(
        !entry["because"].as_str().unwrap_or_default().is_empty(),
        "a door says what it is for: {entry}"
    );
}

#[test]
fn a_delegation_is_still_refused_by_the_op_guard() {
    // The same guard every turn-opening lane carries: an arrival that is really
    // a store reply of a chain in flight assembles nothing.
    let mut doc = delegation_doc();
    doc["header"]["hop"]["operation"] = serde_json::json!("insert");
    let out = emit(doc);

    assert!(
        out.is_empty(),
        "an incomplete fan-in emits NOTHING, by design: {out:?}"
    );
}
