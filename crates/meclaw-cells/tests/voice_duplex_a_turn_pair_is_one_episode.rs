//! Welle Live (GH #784) -- a duplex turn arrives WHOLE, and both halves of it
//! become the day's record.
//!
//! R-25-9 cuts a turn on the MODEL's clock: a user block plus the agent
//! fragments that follow it up to the next user fragment. The duplex `voice`
//! cell therefore delivers `in_turn` with `messages[user, assistant]` -- both
//! sides at once, because the model decided by itself when it spoke. Until now
//! the two halves arrived on two lanes (`in_turn`, then `in_answer` when the
//! brain had answered), and the collector wrote one row per arrival.
//!
//! So the pair is written as a pair: two rows under ONE `turn_id`, in front of
//! the window read of the same bundle, and the per-turn episode lane hands out
//! BOTH in the one emission the turn-opening reply produces. The exchange is
//! one occasion of the day, not two halves that happen to meet in a table.
//!
//! A turn with only a `user` half is untouched -- that is every chat surface
//! and the half-duplex voice pipeline, and their answer still arrives on
//! `in_answer`.
//!
//! The form is `collector_window.rs`: the SHIPPED `params.script_inline` runs
//! under `python3` over a real stdin document, so what is measured here is what
//! ships and nothing is mocked.

const ASSEMBLE_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);

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

/// The legs of a bundle message, in call order: `(tool_call_id, args)`.
fn legs(msg: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    msg["messages"]
        .as_array()
        .expect("bundle turns")
        .iter()
        .map(|t| {
            let id = t["id"].as_str().expect("leg id").to_string();
            let args =
                serde_json::from_str(t["text"].as_str().expect("leg text")).expect("leg json");
            (id, args)
        })
        .collect()
}

/// The turn as the duplex `voice` cell delivers it: what the caller said and
/// what the model already answered, in one message.
fn turn_doc(messages: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "channel_node": "voice",
                               "engine": "duplex"},
                   "hop": {"route": "in_turn", "turn_id": "call-1#4"}},
        "messages": messages
    })
}

fn pair() -> serde_json::Value {
    serde_json::json!([
        {"origin": "user", "type": "text", "text": "how is the weather?"},
        {"origin": "assistant", "type": "text", "text": "sunny, sixteen degrees"}
    ])
}

/// The inserts into `turns` of one turn-opening bundle, in call order.
fn inserts(out: &[serde_json::Value]) -> Vec<serde_json::Value> {
    legs(&out[0])
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert" && a["table"] == "turns")
        .map(|(_, a)| a["row"].clone())
        .collect()
}

#[test]
fn both_halves_of_a_duplex_turn_are_written_under_one_turn_id() {
    let out = emit(turn_doc(pair()));

    assert_eq!(out[0]["header"]["phase"], "turn-open");
    let rows = inserts(&out);
    assert_eq!(rows.len(), 2, "the pair is written as a pair: {}", out[0]);
    assert_eq!(rows[0]["role"], "user");
    assert_eq!(rows[0]["content"], "how is the weather?");
    assert_eq!(rows[1]["role"], "assistant");
    assert_eq!(rows[1]["content"], "sunny, sixteen degrees");
    assert_eq!(
        rows[0]["turn_id"], rows[1]["turn_id"],
        "ONE turn, cut on the model's clock (R-25-9): {rows:?}"
    );
    assert_eq!(
        rows[0]["turn_id"], "call-1#4",
        "and it keeps the id the channel assigned"
    );
    assert_eq!(
        rows[1]["interim"], 0,
        "an answer the caller has already HEARD is not an interim"
    );
    assert_eq!(rows[1]["deferred"], 0);
}

#[test]
fn the_answer_half_sorts_behind_the_question() {
    // `turns.id` is the table's time order and the window reads it descending.
    // Two rows minted in the same microsecond would come back in the order two
    // random hex strings happen to compare in -- so the answer's id is derived
    // from the question's instead of drawn beside it.
    let rows = inserts(&emit(turn_doc(pair())));
    let first = rows[0]["id"].as_str().expect("question id");
    let second = rows[1]["id"].as_str().expect("answer id");

    assert!(
        second > first,
        "the answer follows the question in the window, always: {first} / {second}"
    );
    assert!(
        second.starts_with(first),
        "and it says whose answer it is: {second}"
    );
}

#[test]
fn the_pair_stands_in_front_of_the_window_read() {
    // GH #419: a bundle's ops run in order over the store's one connection, so
    // the window this very turn is assembled from contains the whole turn.
    let out = emit(turn_doc(pair()));
    let ids: Vec<String> = legs(&out[0]).into_iter().map(|(id, _)| id).collect();
    let win = ids
        .iter()
        .position(|id| id == "c-open-win")
        .expect("the window leg");
    let answer = ids
        .iter()
        .position(|id| id == "c-open-pair")
        .expect("the answer leg");

    assert!(answer < win, "in this order: {ids:?}");
}

#[test]
fn a_turn_with_only_a_question_is_written_as_it_always_was() {
    let out = emit(turn_doc(serde_json::json!([
        {"origin": "user", "type": "text", "text": "how is the weather?"}
    ])));

    let rows = inserts(&out);
    assert_eq!(
        rows.len(),
        1,
        "every chat surface and the half-duplex pipeline answer on `in_answer`, \
         and nothing here changed for them: {}",
        out[0]
    );
    assert_eq!(rows[0]["role"], "user");
    let ids: Vec<String> = legs(&out[0]).into_iter().map(|(id, _)| id).collect();
    assert!(
        !ids.contains(&"c-open-pair".to_string()),
        "no empty answer row rides along: {ids:?}"
    );
}

#[test]
fn an_empty_answer_half_writes_no_row() {
    let out = emit(turn_doc(serde_json::json!([
        {"origin": "user", "type": "text", "text": "how is the weather?"},
        {"origin": "assistant", "type": "text", "text": ""}
    ])));

    assert_eq!(
        inserts(&out).len(),
        1,
        "a turn written with empty content would be an answer nobody gave: {}",
        out[0]
    );
}

#[test]
fn the_episode_lane_hands_out_both_halves_in_one_emission() {
    // The per-turn scan rides in the SAME reply as the round check (GH #419),
    // and it is where R-25-10 gets its turns: `turn_write` draws from R-25-9.
    let day = serde_json::json!([
        {"id": "r1", "role": "user", "content": "how is the weather?",
         "interim": 0, "recorded_at": "r1", "episode_written": 0},
        {"id": "r1-a", "role": "assistant", "content": "sunny, sixteen degrees",
         "interim": 0, "recorded_at": "r1", "episode_written": 0}
    ]);
    let bundle_legs = [
        ("c-open-day", day),
        ("c-open-round", serde_json::json!([])),
        ("c-open-win", serde_json::json!([])),
    ];
    let doc = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "call-1#4", "iter": "0",
                               "col_phase": "turn-open", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 2, "bundle_errors": 0}},
        "messages": bundle_legs.iter().map(|(id, rows)| serde_json::json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": bundle_legs.iter().map(|(id, _)| serde_json::json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    });
    let out = emit(doc);

    let episodes: Vec<&serde_json::Value> = out
        .iter()
        .filter(|m| m["header"]["route"] == "turn_write")
        .collect();
    assert_eq!(
        episodes.len(),
        2,
        "the exchange is one occasion of the day, and the memory hive's writer \
         takes ONE turn per message: {out:?}"
    );
    assert_eq!(episodes[0]["messages"][0]["origin"], "user");
    assert_eq!(episodes[0]["messages"][0]["text"], "how is the weather?");
    assert_eq!(episodes[1]["messages"][0]["origin"], "assistant");
    assert_eq!(episodes[1]["messages"][0]["text"], "sunny, sixteen degrees");
    assert_eq!(
        episodes[0]["header"]["turn_id"], "s1#0",
        "the index counts the DRAINABLE turns of the session: {}",
        episodes[0]
    );
    assert_eq!(episodes[1]["header"]["turn_id"], "s1#1");
}
