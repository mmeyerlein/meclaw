//! Welle Live (GH #784) -- the collector switches `talky` into ADVISE mode on
//! `context.engine`, and on nothing else.
//!
//! In a duplex call the voice model speaks to the caller and `talky` advises
//! it: everything `talky` produces belongs in the ```sidecar block, and the
//! prose outside it is spoken by nobody. That is a different job from
//! answering, so the charter of the round has to say which one it is --
//! `system.instructions.mode`, written by this cell on every assembly.
//!
//! WHY THE ENGINE AND NOT THE CHANNEL (OR-L21). A channel node says where a
//! turn came IN, never what is talking on the other end: the same `apps/voice`
//! carries a turn of the half-duplex pipeline and a turn of a duplex session.
//! What decides is the engine behind the call, promoted into `context.engine`
//! by the member's firewall edge.
//!
//! WHY IT IS ALWAYS WRITTEN. `system.*` is durable state in the brain cell,
//! upserted per slot path -- a path that is not sent is a path that is not
//! TOUCHED. A slot that is only ever set keeps saying `advise` for the rest of
//! the agent's life after one duplex call, so a non-duplex turn writes the slot
//! EMPTY. Durable state is revoked, never merely abandoned; it is the argument
//! `system.consult` is already built on.
//!
//! The form is `collector_window.rs`: the SHIPPED `params.script_inline` runs
//! under `python3` over a real stdin document, so what is measured here is what
//! ships and nothing is mocked.

const ASSEMBLE_CONFIG: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);

/// The shipped `config.json` of `./assemble`, parsed.
fn assemble_config() -> serde_json::Value {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    serde_json::from_str(&raw).expect("config json")
}

/// The `params` object the substrate puts on this cell's stdin: the SHIPPED
/// values of the template, minus the script's own source (`build_stdin_json`
/// withholds it). Reading the defaults out of the config instead of restating
/// them here is what makes a case that names no knob a test of the shipped
/// value.
fn assemble_params() -> serde_json::Value {
    let mut p = assemble_config()["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    serde_json::Value::Object(p)
}

/// Run the real script against a real stdin document and return the emitted
/// messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut doc = doc;
    // Last, exactly like `build_stdin_json` -- a body slot cannot shadow it.
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

/// The reply that fires the seam: the window leg parked and the round table was
/// read back in the SAME message (GH #419), so this is where the brain message
/// is assembled -- and the context of a store reply is the context of the turn,
/// because `context` travels the whole message lifecycle.
fn seam_with(ctx_extra: &[(&str, &str)]) -> serde_json::Value {
    let turns = serde_json::json!([
        {"role": "user", "text": "how is the weather?"},
        {"role": "assistant", "text": "sunny"}
    ]);
    let payload = serde_json::json!({"turns": turns, "bytes": 0, "dropped": 0, "capped": 0});
    let rows = serde_json::json!([{"turn_id": "t1", "iter": 0, "role": "leg-window",
                                   "turn": payload.to_string(), "fired": 0}]);
    let mut doc = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "collect", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1, "bundle_errors": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-collect-read",
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": "c-collect-read", "operation": "select",
                     "rows_affected": 1, "duration_ms": 0}]
    });
    for (k, v) in ctx_extra {
        doc["header"]["context"][*k] = serde_json::json!(v);
    }
    doc
}

/// The mode slot of the brain message -- the one path this cell writes into the
/// `instructions` family, beside a charter it never touches.
fn mode_text(out: &[serde_json::Value]) -> String {
    assert_eq!(
        out[0]["header"]["route"], "brain",
        "the seam is the first message of the emission: {out:?}"
    );
    let slot = &out[0]["system"]["instructions"]["mode"];
    assert!(
        slot.is_object(),
        "the mode slot travels on EVERY assembly, empty or not: {}",
        out[0]
    );
    slot["text"]
        .as_str()
        .unwrap_or_else(|| panic!("the slot reaches a provider through a `text` leaf: {slot}"))
        .to_string()
}

#[test]
fn a_duplex_engine_puts_the_brain_into_advise_mode() {
    let text = mode_text(&emit(seam_with(&[("engine", "duplex")])));

    assert!(
        text.starts_with("## mode: advise"),
        "the charter of the round names the mode in its first line: {text}"
    );
    assert!(
        text.contains("sidecar"),
        "and says where everything it produces goes: {text}"
    );
    assert!(
        text.contains("delegation"),
        "a delegation asks for backend work -- the mode says what one is: {text}"
    );
}

#[test]
fn a_turn_without_an_engine_writes_the_mode_slot_empty() {
    let text = mode_text(&emit(seam_with(&[])));

    assert_eq!(
        text, "",
        "present and empty: an only-ever-set slot would keep advising the brain \
         for the rest of its life after one duplex call"
    );
}

#[test]
fn the_channel_node_alone_switches_nothing() {
    // OR-L21: the same `apps/voice` carries half-duplex turns and duplex turns.
    // A channel says where the turn came in, never what is talking.
    let text = mode_text(&emit(seam_with(&[("channel_node", "voice")])));

    assert_eq!(
        text, "",
        "the channel is not the engine -- a voice turn of the half-duplex \
         pipeline is answered, not advised"
    );
}

#[test]
fn another_engine_is_not_the_duplex_one() {
    let text = mode_text(&emit(seam_with(&[("engine", "gpt_live_next")])));

    assert_eq!(
        text, "",
        "`duplex` is the engine the advise mode belongs to, matched whole and \
         never by prefix: {text}"
    );
}

#[test]
fn the_mode_never_touches_the_charter_beside_it() {
    let out = emit(seam_with(&[("engine", "duplex")]));
    let instructions = out[0]["system"]["instructions"]
        .as_object()
        .expect("the instructions family");

    assert_eq!(
        instructions.keys().collect::<Vec<_>>(),
        vec!["mode", "peer"],
        "`instructions.charter` is written by the pack lane and upserted per \
         slot PATH: a sibling key here would be a revocation of somebody \
         else's durable state: {}",
        out[0]
    );
}
