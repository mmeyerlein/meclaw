//! meclaw-os -- the receptionist: ONE talky per channel (GH #29).
//!
//! A colony that talks to many chats through one communicator flattens all of
//! them. The receptionist is the front door that prevents it: the first turn of
//! an UNKNOWN channel makes it instantiate a fresh `talky` for exactly that
//! channel -- by emitting a mutation at `/colony/mutations` (EDA) that carries
//! the `add_nodes` AND the crossing port edges in ONE diff -- and every later
//! turn of that channel is handed straight through the edge that mutation drew.
//!
//! Three claims are pinned here:
//!
//! 1. THE LEDGER decides. A channel with a row is known; a channel without one
//!    is new. Nothing else is asked, and no registry is polled.
//! 2. THE MUTATION is one. Node plus all four port edges travel together, so
//!    the instance is edge-connected -- and therefore active -- from apply.
//! 3. THE TRIGGERING TURN survives. It is emitted AFTER the mutation, in the
//!    same burst, and the edge the mutation just drew is what takes it.
//!
//! The script group runs the shipped `params.script_inline` against real stdin
//! documents. The colony group boots the shipped receptionist in front of the
//! shipped `talky` template and drives three turns over two channels; both
//! `llm` cells talk to the mock OpenAI wire, so the file spends nothing.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const TEMPLATE: &str = "../../templates/receptionist";

/// The round a conversation is spoken in, in the affinity vocabulary the
/// audience gate speaks. The reception never derives it: a channel identity is
/// a room, and a room is not a participant (ADR-0002 E8).
const ROUND: &str = r#"["member:alex","agent:scribe"]"#;
/// The same set as a CEL string literal, for an edge that declares it.
const ROUND_CEL: &str = r#"'["member:alex","agent:scribe"]'"#;

// ════════════════════════════════════════════════════════ the script harness

fn config_of(rel: &str) -> Value {
    let raw = std::fs::read_to_string(format!("{TEMPLATE}/{rel}")).expect("template config");
    meclaw_core::serde_json::from_str(&raw).expect("config json")
}

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one), a bare `${VAR}` becomes the empty string -- the same substitution the
/// colony performs when it reads the config.
fn resolve_vars(script: &str, over: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let inner = &tail[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, d),
            None => (inner, ""),
        };
        let value = over
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or(default);
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(cell: &str, over: &[(&str, &str)]) -> String {
    let v = config_of(&format!("{cell}/config.json"));
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the real script against a real stdin document and return the emissions.
fn greet(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(
        &script_of("greet", over),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "greet exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The default wiring the colony group also uses.
fn knobs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("RECEPTIONIST_MODEL", "gpt-4o-mock"),
        ("RECEPTIONIST_REPLY_TO", "./sink"),
        ("RECEPTIONIST_WRITE_TO", "./archive"),
        ("RECEPTIONIST_ERROR_TO", "./park"),
    ]
}

/// An inbound turn as it reaches `greet`: the parent edge named the lane and
/// promoted the channel and the round.
fn in_turn(channel: &str, text: &str) -> Value {
    in_turn_declaring(channel, text, Some(ROUND))
}

/// The same door, with the round it declared spelled out -- `None` is a door
/// that declares none at all.
fn in_turn_declaring(channel: &str, text: &str, round: Option<&str>) -> Value {
    let mut context = json!({"channel": channel});
    if let Some(round) = round {
        context["audience_set"] = json!(round);
    }
    json!({
        "target": "/reception/greet",
        "header": {"hop": {"route": "in_turn"}, "context": context},
        "messages": [{"origin": "user", "type": "text", "text": text}]
    })
}

/// The ledger's answer to the lookup: `rows` verbatim as the store returns them.
fn look_reply(channel: &str, keep: &Value, rows: Value) -> Value {
    look_reply_declaring(channel, keep, rows, Some(ROUND))
}

/// The same reply, with the round the hive edge promoted back onto it --
/// `None` is the round a door that declared none produced, which is the empty
/// string and never a guess.
fn look_reply_declaring(channel: &str, keep: &Value, rows: Value, round: Option<&str>) -> Value {
    json!({
        "target": "/reception/greet",
        "header": {
            "hop": {"operation": "select", "rows_affected": rows.as_array().map(|a| a.len()).unwrap_or(0)},
            "context": {"rec_origin": "greet", "rec_phase": "look",
                        "rec_channel": channel, "rec_aud": round.unwrap_or_default(),
                        "rec_body": keep.to_string()}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r-look",
                      "text": rows.to_string()}]
    })
}

fn op_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    meclaw_core::serde_json::from_str(text).expect("op json")
}

fn route_of(msg: &Value) -> String {
    msg["header"]["route"].as_str().unwrap_or_default().into()
}

// ═══════════════════════════════════════════════════════════ 1. THE LEDGER

/// A turn asks the ledger and nothing else -- no registry read, no probing of
/// the graph. One select, keyed by the channel, and the turn rides along on the
/// hop while the lookup runs.
#[test]
fn an_inbound_turn_asks_the_ledger_for_its_channel() {
    let out = greet(&knobs(), in_turn("c-42", "hi"));
    assert_eq!(out.len(), 1, "one lookup, nothing else: {out:?}");
    assert_eq!(route_of(&out[0]), "rstore");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], json!("select"));
    assert_eq!(op["table"], json!("channels"));
    assert_eq!(op["where"]["channel"], json!("c-42"));
    assert_eq!(
        out[0]["header"]["phase"],
        json!("look"),
        "the phase is what dispatches the reply"
    );
    let carried: Value =
        meclaw_core::serde_json::from_str(out[0]["header"]["rec_body"].as_str().unwrap())
            .expect("the turn travels as a header copy");
    assert_eq!(carried["messages"][0]["text"], json!("hi"));
}

/// A channel the ledger knows is handed through -- no mutation, no second
/// instance, just the turn on the lane the earlier mutation drew.
#[test]
fn a_known_channel_is_handed_through_without_a_mutation() {
    let keep = json!({"messages": [{"origin": "user", "type": "text", "text": "again"}]});
    let rows = json!([{"channel": "c-42", "talky_path": "/talky-c-42",
                       "created_at": "2026-08-14T00:00:00.000000Z"}]);
    let out = greet(&knobs(), look_reply("c-42", &keep, rows));
    assert_eq!(out.len(), 1, "exactly the turn: {out:?}");
    assert_eq!(route_of(&out[0]), "turn");
    assert!(
        out.iter().all(|m| m["header"]["msg_type"].is_null()),
        "a known channel emits NO mutation"
    );
    assert_eq!(out[0]["header"]["chan"], json!("c-42"));
    assert_eq!(out[0]["header"]["chan_raw"], json!("c-42"));
    assert_eq!(out[0]["messages"][0]["text"], json!("again"));
}

// ═══════════════════════════════════════════════════════ 2. THE MUTATION

/// The unknown channel: ONE mutation carries the node AND every crossing port
/// edge, so the instance derives active from apply instead of landing as an
/// island (the one-mutation-per-connection discipline of the builder docs).
#[test]
fn an_unknown_channel_instantiates_a_talky_and_wires_it_in_one_mutation() {
    let keep = json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]});
    let out = greet(&knobs(), look_reply("c-42", &keep, json!([])));
    assert_eq!(out.len(), 3, "mutation, ledger insert, turn: {out:?}");

    let m = &out[0];
    assert_eq!(m["header"]["msg_type"], json!("mutation"));
    assert_eq!(
        m["scope"],
        json!("/"),
        "self-locating: the hive's PARENT is the scope"
    );
    assert_eq!(m["ctx"]["model"], json!("gpt-4o-mock"), "Lane-B literal");

    let nodes = m["diff"]["add_nodes"].as_array().expect("add_nodes");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0]["name"], json!("talky-c-42"));
    assert_eq!(nodes[0]["template"], json!("talky"));

    let edges = m["diff"]["add_edges"].as_array().expect("add_edges");
    assert_eq!(edges.len(), 4, "ingress, reply, write, error: {edges:?}");
    assert_eq!(edges[0]["from"], json!("./reception"));
    // The composite's own path, never a cell inside it: `talky@3` is sealed
    // (GH #228), so the lane the edge sets is what selects the cell behind the
    // door and `RECEPTIONIST_INGRESS` defaults to empty.
    assert_eq!(edges[0]["to"], json!("./talky-c-42"));
    let cond = edges[0]["condition"].as_str().unwrap();
    assert!(
        cond.contains("has(hop.chan)") && cond.contains("hop.chan == 'c-42'"),
        "the channel condition is guarded (GH #80): {cond}"
    );
    assert_eq!(
        edges[0]["modifier"]["set_context"]["channel"],
        json!("hop.chan_raw"),
        "the RAW channel is promoted, so the keeper mints ids the surface knows"
    );
    assert_eq!(edges[1]["to"], json!("./sink"));
    assert_eq!(edges[2]["to"], json!("./archive"));
    assert_eq!(edges[3]["from"], json!("./talky-c-42"));
    assert_eq!(edges[3]["to"], json!("./park"));
}

/// The ledger row is written in the same burst, and it names the instance the
/// mutation creates -- a later turn finds the channel, not a guess.
#[test]
fn the_new_channel_is_written_into_the_ledger() {
    let keep = json!({"messages": []});
    let out = greet(&knobs(), look_reply("c-42", &keep, json!([])));
    let op = op_of(&out[1]);
    assert_eq!(op["operation"], json!("insert"));
    assert_eq!(op["table"], json!("channels"));
    assert_eq!(op["row"]["channel"], json!("c-42"));
    assert_eq!(op["row"]["talky_path"], json!("/talky-c-42"));
    assert!(
        op["row"]["created_at"].as_str().unwrap().ends_with('Z'),
        "created_at is a UTC stamp"
    );
}

// ═══════════════════════════════════════════════════ 3. THE TRIGGERING TURN

/// The turn that triggered the instantiation is emitted LAST -- after the
/// mutation -- so the edge the mutation drew is already in the table when the
/// colony routes it. Losing this turn would cost the user their first message.
#[test]
fn the_triggering_turn_is_re_emitted_after_the_mutation() {
    let keep = json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]});
    let out = greet(&knobs(), look_reply("c-42", &keep, json!([])));
    assert_eq!(route_of(&out[2]), "turn", "the turn is the LAST emission");
    assert_eq!(out[2]["messages"][0]["text"], json!("hi"));
    assert_eq!(out[2]["header"]["chan"], json!("c-42"));
    let mutation_at = out
        .iter()
        .position(|m| m["header"]["msg_type"] == json!("mutation"));
    let turn_at = out.iter().position(|m| route_of(m) == "turn");
    assert!(
        mutation_at < turn_at,
        "the mutation MUST precede the turn: {out:?}"
    );
}

// ═══════════════════════════════════════════════════════════ the sharp edges

/// A channel identity is free text; a node name and a CEL string literal are
/// not. The key is sanitised, and a channel that HAD to be sanitised keeps a
/// digest -- so `tg:42` and `tg/42` can never collapse onto one talky.
#[test]
fn a_channel_that_is_not_a_name_gets_a_safe_unique_key() {
    let keep = json!({"messages": []});
    let a = greet(&knobs(), look_reply("tg:42", &keep, json!([])));
    let b = greet(&knobs(), look_reply("tg/42", &keep, json!([])));
    let name_a = a[0]["diff"]["add_nodes"][0]["name"]
        .as_str()
        .unwrap()
        .to_string();
    let name_b = b[0]["diff"]["add_nodes"][0]["name"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(name_a.starts_with("talky-tg_42-"), "sanitised: {name_a}");
    assert_ne!(name_a, name_b, "two channels, two talkys");
    assert!(
        !name_a.contains(':') && !name_b.contains('/'),
        "no separator survives into a node name"
    );
    assert_eq!(
        a[0]["diff"]["add_edges"][0]["modifier"]["set_context"]["channel"],
        json!("hop.chan_raw"),
        "the RAW channel still reaches the keeper"
    );
}

/// The colony's verdict is not a UBF body and carries no context (cookbook
/// `colony-endpoint-roundtrip`, rules 1-3). It is TERMINAL here: recognising it
/// as "some other request" is exactly the reply-fallback loop that spins until
/// the TTL kills it.
#[test]
fn the_mutation_verdict_is_terminal() {
    for verdict in [
        json!({"mutation": {"outcome": "committed", "id": "m-1"}}),
        json!({"mutation": {"outcome": "rejected", "id": "m-2",
                            "error_code": "naming_collision", "details": "x"}}),
    ] {
        let mut doc = verdict.clone();
        doc["target"] = json!("/reception/greet");
        let out = greet(&knobs(), doc);
        assert!(
            out.is_empty(),
            "a verdict emits nothing: {out:?} for {verdict}"
        );
    }
}

/// Two turns of the SAME new channel can cross: the colony serialises the
/// mutations, so the second `add_nodes` is rejected on the name. That is not an
/// error the receptionist has to prevent -- the turn behind it still finds the
/// edge the first mutation drew, because the key is derived from the channel
/// and not from who got there first.
#[test]
fn the_race_resolves_on_the_channel_key() {
    let keep = json!({"messages": []});
    let a = greet(&knobs(), look_reply("c-42", &keep, json!([])));
    let b = greet(&knobs(), look_reply("c-42", &keep, json!([])));
    assert_eq!(
        a[0]["diff"]["add_nodes"][0]["name"], b[0]["diff"]["add_nodes"][0]["name"],
        "same channel, same node name -- the second mutation collides, by design"
    );
    assert_eq!(a[2]["header"]["chan"], b[2]["header"]["chan"]);
}

/// A wiring the operator did not configure is not invented: an empty target
/// means the edge is left out, and the README says what that costs.
#[test]
fn unconfigured_ports_are_left_unwired() {
    let keep = json!({"messages": []});
    let out = greet(
        &[
            ("RECEPTIONIST_MODEL", "gpt-4o-mock"),
            ("RECEPTIONIST_REPLY_TO", "./sink"),
        ],
        look_reply("c-42", &keep, json!([])),
    );
    let edges = out[0]["diff"]["add_edges"].as_array().expect("add_edges");
    assert_eq!(edges.len(), 2, "ingress + reply only: {edges:?}");
}

// ══════════════════════════════════════════════════════════════ 4. THE ROUND

/// GH #274. The reception draws the ingress edge of every generation it builds,
/// so whatever that edge declares is the only thing the keeper behind it will
/// ever record about the round -- and the keeper records it exactly once, when
/// the generation opens (ADR-0002 E8/E12).
///
/// It has to be a DECLARATION, not a survival. The round does reach the talky
/// today as plain context that no hop happened to delete, which is the same
/// accident GH #273 refused to build on: an inherited value is not a promise,
/// and the next version of any cell on the path may stop carrying it.
#[test]
fn the_ingress_edge_the_reception_draws_declares_the_round() {
    let keep = json!({"messages": [{"origin": "user", "type": "text", "text": "hi"}]});
    let out = greet(&knobs(), look_reply("c-42", &keep, json!([])));
    let edges = out[0]["diff"]["add_edges"].as_array().expect("add_edges");
    assert_eq!(
        edges[0]["modifier"]["set_context"]["audience_set"],
        json!("hop.aud"),
        "the edge into the new generation NAMES the round: {edges:?}"
    );
    assert_eq!(
        edges[0]["modifier"]["set_context"]["channel"],
        json!("hop.chan_raw"),
        "and it still names the room"
    );
    assert_eq!(
        out[2]["header"]["aud"],
        json!(ROUND),
        "the turn behind the mutation carries the round the door declared"
    );
}

/// The round rides the ledger round trip the way the channel does -- on the
/// hop, promoted back to `context.rec_aud` by the hive's own edge. A key that
/// is only inherited is a key the next hop may drop.
#[test]
fn the_round_rides_the_lookup_on_the_hop() {
    let out = greet(&knobs(), in_turn("c-42", "hi"));
    assert_eq!(out.len(), 1, "one lookup, nothing else: {out:?}");
    assert_eq!(
        out[0]["header"]["aud"],
        json!(ROUND),
        "the lookup carries the round to the other side of the store: {out:?}"
    );
}

/// A door that declares nothing gets nothing. Not the channel turned into a
/// participant, not `["*"]`, not a set derived from the ledger row -- an EMPTY
/// round, which the write path refuses visibly rather than storing a day that
/// claims everyone was present (GH #269, GH #273).
///
/// The key is still PRESENT and empty: a missing hop key makes the CEL modifier
/// fail, a failed modifier skips the edge, and a turn that cannot name its
/// round would then vanish instead of being answered.
#[test]
fn a_door_that_declares_no_round_leaves_it_empty() {
    let out = greet(&knobs(), in_turn_declaring("tg:42", "hi", None));
    assert_eq!(
        out[0]["header"]["aud"],
        json!(""),
        "present and empty, never absent: {out:?}"
    );

    let keep = json!({"messages": []});
    let out = greet(
        &knobs(),
        look_reply_declaring("tg:42", &keep, json!([]), None),
    );
    let edges = out[0]["diff"]["add_edges"].as_array().expect("add_edges");
    assert_eq!(
        edges[0]["modifier"]["set_context"]["audience_set"],
        json!("hop.aud"),
        "the edge is drawn the same way either way: {edges:?}"
    );
    for m in &out {
        let aud = m["header"]["aud"].as_str().expect("aud is always present");
        assert_eq!(aud, "", "nothing is invented: {m:?}");
    }
    let whole = meclaw_core::serde_json::to_string(&out).unwrap();
    assert!(
        !whole.contains("member:") && !whole.contains(r#"["*"]"#),
        "no participant is derived from a channel id: {whole}"
    );
}

/// The other half of the same fix, and the half a caller actually reads: the
/// hive's own contract ASKS for the round. A caller that satisfies the contract
/// must end up with a generation that closes tagged.
#[test]
fn the_contract_asks_the_caller_for_the_round() {
    let cfg = config_of("config.json");
    let accepts = cfg["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts");
    let entry = accepts
        .iter()
        .find(|a| a["route"] == json!("in_turn"))
        .expect("the in_turn lane");
    let keys: Vec<&str> = entry["context"]
        .as_array()
        .expect("context keys")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        keys.contains(&"channel") && keys.contains(&"audience_set"),
        "the lane asks for the room AND the round: {keys:?}"
    );
}

// ═════════════════════════════════════════════════════════ the colony group

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Only `config.json` files travel -- the tree under test IS the template.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// GH #277: a directory whose `config.json` declares `cell.type: "ref"` is a
/// REFERENCE, not a cell -- the referenced template's tree belongs in its
/// place. `talky` names its four sub-units that way, so a tree copied straight
/// off the library follows the same hop the substrate's staging path follows.
fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        let name = reference.split('@').next().unwrap_or_default();
        dir = templates_root().join(name);
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

/// A template directory the colony can scan: cells plus the `template.json`.
fn copy_template(name: &str, dst_root: &std::path::Path) {
    let src = templates_root().join(name);
    let dst = dst_root.join(name);
    copy_cells_verbatim(&src, &dst);
    std::fs::copy(src.join("template.json"), dst.join("template.json")).unwrap();
    // GH #277: the template travels VERBATIM -- a `cell.type: "ref"` sub-unit
    // stays a ref, because resolving it is the substrate's job and that is what
    // this test drives. So every template it names travels next to it, exactly
    // as the shipped library carries them.
    for referenced in refs_in(&dst) {
        if !dst_root.join(&referenced).is_dir() {
            copy_template(&referenced, dst_root);
        }
    }
}

/// `copy_cells` without the ref hop: the tree as the library holds it.
fn copy_cells_verbatim(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells_verbatim(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

/// Every template name a `cell.type: "ref"` marker under `dir` points at.
fn refs_in(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let p = entry.path();
        if p.is_dir() {
            out.extend(refs_in(&p));
        } else if entry.file_name() == "config.json" {
            let raw = std::fs::read_to_string(&p).unwrap();
            let v: Value = meclaw_core::serde_json::from_str(&raw).unwrap();
            if v["cell"]["type"] == "ref" {
                let r = v["cell"]["template"].as_str().expect("a ref names one");
                out.push(r.split('@').next().unwrap_or_default().to_string());
            }
        }
    }
    out
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

const SURFACE: &str = r#"
import sys, json
doc = json.load(sys.stdin)
d = doc["body"]
envelope = doc["envelope"]
hop = ((envelope.get("header") or {}).get("hop") or {})
sys.stdout.write(json.dumps({"header": {"route": "turn",
                                        "chat_id": str(hop.get("chat_id") or "c-42")},
                             "messages": d.get("messages", [])}))
"#;

const ARCHIVE: &str = r#"
import sys, json
d = json.load(sys.stdin)["body"]
sys.stdout.write(json.dumps({"header": {"route": "archived"}, "messages": []}))
"#;

fn code_cell(script: &str, routes: &[&str], extra_hop: Value) -> Value {
    let mut hop = json!({"route": {"type": "string", "values": routes, "required": false}});
    if let Some(extra) = extra_hop.as_object() {
        for (k, v) in extra {
            hop[k] = v.clone();
        }
    }
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "multi_send_capable": true,
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": hop
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in around the receptionist.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The parent tree: ONE ingress edge into the receptionist and the ONE
/// privileged edge out of it. No mutation can mint the second one at any scope
/// (`scope_out_of_bounds`), so it is written here, at bootstrap, on purpose.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./surface", "to": "./reception",
         "condition": "has(hop.route) && hop.route == 'turn'",
         "modifier": {"set_hop": {"route": "'in_turn'"},
                      "set_context": {"channel": "hop.chat_id",
                                      "audience_set": ROUND_CEL}}},
        {"from": "./reception", "to": "/colony/mutations",
         "condition": "has(hop.route) && hop.route == 'mutate'"},
        {"from": "./archive", "to": "/park"}
    ]}}})
}

fn build_tree(td: &tempfile::TempDir, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        concat!(
            "OPENROUTER_API_KEY=test-key\n",
            "KEEPER_NIGHT_CRON=0 0 0 1 1 *\n",
            "RECEPTIONIST_MODEL=gpt-4o-mock\n",
            "RECEPTIONIST_REPLY_TO=./sink\n",
            "RECEPTIONIST_WRITE_TO=./archive\n",
            "RECEPTIONIST_ERROR_TO=./park\n",
        ),
    )
    .unwrap();
    write(root, "main/config.json", &main_config());
    write(
        root,
        "main/surface/config.json",
        &code_cell(
            SURFACE,
            &["turn"],
            json!({"chat_id": {"type": "string", "required": false}}),
        ),
    );
    write(
        root,
        "main/archive/config.json",
        &code_cell(ARCHIVE, &["archived"], json!({})),
    );
    copy_cells(
        &templates_root().join("receptionist"),
        &root.join("main/reception"),
    );

    // The talky the receptionist instantiates lives in the TEMPLATE directory,
    // which is where a mutation looks for it.
    copy_template("talky", &root.join("templates"));
    for rel in [
        "templates/talky/brain/config.json",
        "templates/summarizer/writer/config.json",
    ] {
        patch(root, rel, |v| v["params"]["base_url"] = json!(base_url));
    }
}

async fn boot(
    td: &tempfile::TempDir,
) -> (
    ColonyHandle,
    mpsc::Receiver<Message>,
    mpsc::Receiver<Message>,
) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    let (park_tx, park_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack");
    (h, sink_rx, park_rx)
}

fn turn(channel: &str, text: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("chat_id".into(), json!(channel));
    MessageBuilder::new(Path::new("/surface"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
        ))
        .hop(hop)
        .ttl(200)
        .build()
}

fn answer_text(m: &Message) -> String {
    match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or_default().into(),
        Body::Blob(_) => panic!("inline expected"),
    }
}

async fn recv_bounded(rx: &mut mpsc::Receiver<Message>) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .ok()
        .flatten()
}

/// Every registered path, straight out of the colony's in-memory registry.
async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("read registry");
    ack_rx
        .await
        .expect("registry ack")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect()
}

/// Every mutation the colony has seen, as `(status, scope)`. The count is the
/// positive receipt that a known channel emits NO mutation: a second
/// instantiation would land here as a `naming_collision` REJECT, which the
/// registry alone could never tell apart from "no mutation at all".
async fn mutation_log(h: &ColonyHandle) -> Vec<(String, Option<String>)> {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::ReadMutationsAudit {
            since: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("read audit");
    ack_rx
        .await
        .expect("audit ack")
        .entries
        .into_iter()
        .map(|e| (e.status, e.error_code))
        .collect()
}

/// The channel ledger, read straight from the store's own `cell.db`.
fn ledger_rows(root: &std::path::Path) -> Vec<(String, String)> {
    let db = root.join("main/reception/ledger/cell.db");
    let conn = rusqlite::Connection::open(db).expect("ledger cell.db");
    let mut stmt = conn
        .prepare("SELECT channel, talky_path FROM channels ORDER BY channel")
        .expect("channels table");
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .expect("query")
        .filter_map(|r| r.ok())
        .collect()
}

/// The whole claim of #29 in one run: two channels, two talkys, one template,
/// and the second turn of the first channel does NOT build a third.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_channel_gets_its_own_talky_and_only_one() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("a1", "stop"),
        canned_chat_completion("a2", "stop"),
        canned_chat_completion("b1", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    // --- turn 1, channel A: an unknown channel builds its talky and is answered
    h.send(turn("chan-a", "hello a")).await;
    let first = recv_bounded(&mut sink_rx).await.expect("an answer for A");
    assert_eq!(answer_text(&first), "a1");

    let paths = registry_paths(&h).await;
    assert!(
        paths.iter().any(|p| p == "/talky-chan-a/brain"),
        "channel A got its own talky: {paths:?}"
    );
    assert!(
        !paths.iter().any(|p| p.starts_with("/talky-chan-b")),
        "and nothing else did: {paths:?}"
    );

    // --- turn 2, channel A: the SAME talky, no second instantiation
    h.send(turn("chan-a", "hello again")).await;
    let second = recv_bounded(&mut sink_rx)
        .await
        .expect("a second answer for A");
    assert_eq!(answer_text(&second), "a2");
    let paths = registry_paths(&h).await;
    let a_roots = paths
        .iter()
        .filter(|p| p.starts_with("/talky-chan-a"))
        .count();
    let brains = paths.iter().filter(|p| p.ends_with("/brain")).count();
    assert_eq!(brains, 1, "still exactly ONE talky: {paths:?}");
    assert!(a_roots > 1, "the subtree is still there: {paths:?}");
    // ... and it did not merely COLLIDE: the second turn emitted no mutation.
    assert_eq!(
        mutation_log(&h).await,
        vec![("committed".to_string(), None)],
        "a known channel asks the ledger and stops -- exactly ONE mutation so far"
    );

    // --- turn 3, channel B: a second channel, a second talky
    h.send(turn("chan-b", "hello b")).await;
    let third = recv_bounded(&mut sink_rx).await.expect("an answer for B");
    assert_eq!(answer_text(&third), "b1");
    let paths = registry_paths(&h).await;
    assert!(
        paths.iter().any(|p| p == "/talky-chan-b/brain"),
        "channel B got its own talky: {paths:?}"
    );
    assert_eq!(
        paths.iter().filter(|p| p.ends_with("/brain")).count(),
        2,
        "two channels, two brains: {paths:?}"
    );

    assert_eq!(
        mutation_log(&h).await,
        vec![
            ("committed".to_string(), None),
            ("committed".to_string(), None)
        ],
        "two channels, two mutations, both committed -- and not one more"
    );

    // The ledger is what said "known" the second time -- and it carries both.
    let ledger = ledger_rows(td.path());
    assert_eq!(
        ledger,
        vec![
            ("chan-a".to_string(), "/talky-chan-a".to_string()),
            ("chan-b".to_string(), "/talky-chan-b".to_string()),
        ],
        "one row per channel, naming the instance it owns"
    );

    h.shutdown().await;
}

/// Two turns of the same COLD channel, fired without waiting: both selects may
/// see an empty ledger, and then both mutations ask for the same node name.
///
/// The colony serialises them, so exactly one can win -- and the loser needs no
/// repair: every turn is emitted BEHIND its own mutation, so by the time the
/// second turn is routed the winner's ingress edge is in the table and takes
/// it. What the test pins is therefore the outcome, not the interleaving: ONE
/// talky, BOTH turns answered, and never a second committed mutation.
///
/// (The reject the loser collects is `resume_requires_stopped_cell`, not
/// `naming_collision`: `add_nodes` at an occupied path is a per-node RESUME,
/// and the sitting cells are awake. Measured, not assumed -- and it is a clean
/// pre-rename reject either way, which is what makes the loser harmless.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_on_a_cold_channel_never_forks_it() {
    let mock = MockOpenAI::start(vec![
        canned_chat_completion("r1", "stop"),
        canned_chat_completion("r2", "stop"),
    ])
    .await;
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, &mock.base_url);
    let (h, mut sink_rx, _park_rx) = boot(&td).await;

    h.send(turn("chan-r", "first")).await;
    h.send(turn("chan-r", "second")).await;

    let a = recv_bounded(&mut sink_rx).await.expect("first answer");
    let b = recv_bounded(&mut sink_rx).await.expect("second answer");
    let mut got = [answer_text(&a), answer_text(&b)];
    got.sort();
    assert_eq!(got, ["r1", "r2"], "no turn was lost in the burst");

    let paths = registry_paths(&h).await;
    assert_eq!(
        paths.iter().filter(|p| p.ends_with("/brain")).count(),
        1,
        "one channel, one talky -- whoever won: {paths:?}"
    );
    let log = mutation_log(&h).await;
    assert_eq!(
        log.iter().filter(|(s, _)| s == "committed").count(),
        1,
        "exactly ONE mutation may commit for a channel: {log:?}"
    );
    assert!(log.len() <= 2, "at most the winner and one loser: {log:?}");
    assert_eq!(
        ledger_rows(td.path())
            .iter()
            .filter(|(c, _)| c == "chan-r")
            .count(),
        log.len(),
        "one ledger row per turn that found the channel cold -- the row is written \
         unconditionally, the INSTANCE is what the colony makes unique"
    );

    h.shutdown().await;
}
