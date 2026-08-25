//! meclaw-os -- identity travels per message (GH #272; RULED Q11, 2026-08-21).
//!
//! #272 asked which of three end forms a BATCH's speaker granularity should
//! take: one speaker for the whole batch, a speaker per contained turn, or a
//! per-turn column filled by the decomposer. The ruling answers by removing the
//! question. After this wave there is no batch on the live path -- the collector
//! hands out ONE message per turn -- so identity is not a property of a
//! container any more. It is context, and context travels with a message. There
//! is nothing to give a granularity to.
//!
//! Nothing is built for that. What this file does is keep the dissolution
//! durable rather than incidental: the three facts below are the shape #272
//! cannot come back through, and two of them held before this wave while the
//! third is what Task 12 created. A lock that was green on arrival is a lock,
//! not a fix.
//!
//! 1. **An absent speaker degrades to an empty column, never to a role.** The
//!    writer's `speaker = str((ctx.get("speaker") if origin == "user" else
//!    ctx.get("agent_id")) or "")` is the whole mechanism: `sender` keeps the
//!    role, `speaker` keeps the identity, and a role written into the identity
//!    column is exactly the defect that column exists to end. Optional by
//!    contract ruling R1 -- the audience is the security-bearing field, the
//!    speaker is provenance detail.
//! 2. **`agent_id` and `speaker` answer different questions.** On an assistant
//!    turn the writer reads `agent_id`, and it does so even when a `speaker`
//!    key is sitting right next to it carrying somebody else. A caller that
//!    promotes a constant `context.speaker` onto a lane does not thereby
//!    attribute the agent's own answers to a person.
//! 3. **The per-turn emission mints no identity of its own.** The collector's
//!    `turn_write` message carries the turn -- `turn_id`, `turn_index`,
//!    `happened_at` -- and NO `speaker` hop key. Who spoke is whatever the
//!    chain's own context already says, which is what makes two consecutive
//!    turns of one session able to carry two different speakers: they are two
//!    messages, not two rows of one body.
//!
//! The hazard stays named because it is still reachable by hand: an edge that
//! promotes a **constant** `context.speaker` onto a path carrying more than one
//! participant's turns re-creates #272 one wiring decision at a time. After
//! this wave no shipped topology has such a path -- which is a reason to state
//! the hazard once, not a reason to stop stating it.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. No mock, no provider, nothing spent.

use std::io::Write;
use std::process::{Command, Stdio};

use meclaw_core::serde_json::{Value, json};

const WRITER_CONFIG: &str = "../../templates/memory-hive/writer/config.json";
const ASSEMBLE_CONFIG: &str = "../../templates/collector/assemble/config.json";

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one) -- the same substitution the colony performs at boot.
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

fn script_of(path: &str, over: &[(&str, &str)]) -> String {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts are within a few KB of it).
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

/// Runs a shipped script against a real stdin document and returns its
/// emissions (an empty multi-send is an empty vector).
fn run(script: &str, doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not json ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// The `params` object the substrate puts on a `code` cell's stdin: the values
/// the config ships, minus the script's own source.
fn params_of(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    let mut p = v["params"].as_object().cloned().expect("params object");
    p.remove("script_inline");
    Value::Object(p)
}

fn hop(m: &Value, key: &str) -> String {
    m["header"][key].as_str().unwrap_or_default().to_string()
}

/// The store args of an emission that goes to a store cell.
fn args(m: &Value) -> Value {
    let text = m["messages"][0]["text"].as_str().expect("tool_call text");
    meclaw_core::serde_json::from_str(text).expect("store args are json")
}

// ══════════════════════════════════════════════════════════════ the writer side

/// One turn on the hive's `in_episode` lane: the body carries what was said,
/// the context carries the provenance the edge promoted.
fn turn_doc(origin: &str, text: &str, ctx: Value) -> Value {
    let mut context = json!({
        "session_id": "s1",
        "turn_id": "s1#0",
        "audience_set": "[\"member:alex\"]",
        "channel": "tg:private"
    });
    for (k, v) in ctx.as_object().expect("context overrides") {
        context[k.clone()] = v.clone();
    }
    json!({
        "header": {"context": context},
        "messages": [{"origin": origin, "type": "text", "text": text}]
    })
}

/// The `episodes` row the writer hands to the store, or a panic naming what it
/// did instead (a refusal is a `reject` emission, never a row).
fn episode_row(doc: Value) -> Value {
    let mut doc = doc;
    doc["params"] = params_of(WRITER_CONFIG);
    let out = run(&script_of(WRITER_CONFIG, &[]), doc);
    let store: Vec<&Value> = out.iter().filter(|m| hop(m, "route") == "wstore").collect();
    assert_eq!(store.len(), 1, "one episode is written -- {out:?}");
    args(store[0])["row"].clone()
}

/// (a) The absent identity degrades to an EMPTY column, on both roles. The role
/// is right there in `sender` and it stays there: a row whose `speaker` reads
/// `"user"` would answer "who said this" with "somebody", which is the failure
/// mode a separate identity column exists to prevent.
#[test]
fn an_absent_speaker_is_an_empty_column_and_never_the_role() {
    for (origin, text) in [("user", "my editor is helix"), ("assistant", "noted")] {
        let row = episode_row(turn_doc(origin, text, json!({})));
        assert_eq!(
            row["speaker"],
            json!(""),
            "{origin}: absent identity stays empty -- {row:?}"
        );
        assert_eq!(
            row["sender"],
            json!(origin),
            "{origin}: the ROLE lives in sender, undisturbed -- {row:?}"
        );
        assert_ne!(
            row["speaker"], row["sender"],
            "{origin}: the role must never be copied into the identity column -- {row:?}"
        );
        // And the turn is written rather than refused: the speaker is optional
        // (contract ruling R1), the audience is not.
        assert_eq!(row["content"], json!(text));
    }
}

/// The other half of the same fact: an identity that IS there is written
/// verbatim, in the affinity vocabulary it arrived in. The hive looks nothing
/// up -- translating a connector's user id happens on the talky's edge
/// (ADR-0002 E8).
#[test]
fn a_present_speaker_is_written_exactly_as_it_arrived() {
    let row = episode_row(turn_doc(
        "user",
        "and i cook keto",
        json!({"speaker": "member:alex"}),
    ));
    assert_eq!(row["speaker"], json!("member:alex"), "{row:?}");
    assert_eq!(row["sender"], json!("user"), "{row:?}");
}

/// (b) `speaker` and `agent_id` answer DIFFERENT questions, and the writer
/// picks by the role of the turn it is writing. An assistant turn takes
/// `agent_id` even when a `speaker` sits right beside it carrying somebody
/// else -- otherwise a lane that promotes a constant `context.speaker` would
/// quietly attribute every agent answer to a person who never wrote it. That
/// mis-attribution IS #272, in the one shape still reachable by hand.
#[test]
fn an_assistant_turn_takes_the_agent_id_not_the_speaker_beside_it() {
    let row = episode_row(turn_doc(
        "assistant",
        "noted that too",
        json!({"agent_id": "agent:aiden", "speaker": "member:alex"}),
    ));
    assert_eq!(
        row["speaker"],
        json!("agent:aiden"),
        "the answering agent, not the person in the room -- {row:?}"
    );
    assert_eq!(row["sender"], json!("assistant"), "{row:?}");

    // The mirror case, so the pick is a rule and not an accident: a user turn
    // reads `speaker` and ignores an `agent_id` sitting beside it.
    let user = episode_row(turn_doc(
        "user",
        "my editor is helix",
        json!({"agent_id": "agent:aiden", "speaker": "member:alex"}),
    ));
    assert_eq!(user["speaker"], json!("member:alex"), "{user:?}");

    // And an assistant turn with no `agent_id` degrades to empty rather than
    // reaching sideways for the `speaker` that is present.
    let bare = episode_row(turn_doc(
        "assistant",
        "noted",
        json!({"speaker": "member:alex"}),
    ));
    assert_eq!(
        bare["speaker"],
        json!(""),
        "no agent_id means no identity, not the nearest one -- {bare:?}"
    );
}

// ═══════════════════════════════════════════════════════════ the collector side

/// One row of the collector's `turns` table, as the store hands it back.
fn turn_row(id: &str, role: &str, content: &str) -> Value {
    json!({"id": id, "session_id": "s1", "turn_id": "t-".to_string() + id,
           "role": role, "content": content,
           "recorded_at": "2026-08-23T09:00:0".to_string() + id,
           "interim": 0, "episode_written": 0})
}

/// The store's reply to the per-turn scan.
fn scan_reply(rows: Value) -> Value {
    json!({
        "header": {
            "hop": {"operation": "select",
                    "rows_affected": rows.as_array().map_or(0, Vec::len)},
            "context": {"session_id": "s1", "turn_id": "t1",
                        "col_phase": "tw-scan", "store_origin": "collector"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-sel",
                      "text": rows.to_string()}]
    })
}

/// The shipped collector's per-turn emissions, in the order they leave.
fn turn_episodes(rows: Value) -> Vec<Value> {
    let mut doc = scan_reply(rows);
    doc["params"] = params_of(ASSEMBLE_CONFIG);
    run(&script_of(ASSEMBLE_CONFIG, &[]), doc)
        .into_iter()
        .filter(|m| hop(m, "route") == "turn_write")
        .collect()
}

/// (c) The collector mints NO identity. Its header carries the turn -- the id,
/// the index, the event time -- and stops there. A `speaker` hop key here would
/// be the decomposer's column under a new name: a producer deciding who spoke
/// from a table it read, instead of the chain's own context saying so.
#[test]
fn the_per_turn_emission_mints_no_speaker_of_its_own() {
    let eps = turn_episodes(json!([
        turn_row("0", "user", "my editor is helix"),
        turn_row("1", "assistant", "noted"),
    ]));
    assert_eq!(eps.len(), 2, "one message per turn -- {eps:?}");

    for ep in &eps {
        let header = ep["header"].as_object().expect("header object");
        for minted in ["speaker", "agent_id", "sender"] {
            assert!(
                !header.contains_key(minted),
                "the collector minted {minted} -- identity is context, not a hop key: {header:?}"
            );
        }
        // What it DOES carry is the turn, and every key is present rather than
        // merely empty: a missing key makes the port edge's CEL modifier fail,
        // and a failed modifier skips the whole edge.
        for key in [
            "route",
            "session_id",
            "turn_id",
            "turn_index",
            "happened_at",
        ] {
            assert!(
                header.contains_key(key),
                "hop key {key} is absent -- {header:?}"
            );
        }
    }
}

/// (d) The mechanical form of "identity travels per message": two consecutive
/// turns of ONE session leave as TWO messages, so two contexts carry two
/// speakers and two rows land with two identities. In the batch this issue was
/// filed against, both turns rode in one body under one context -- and the
/// question "whose speaker is it" had no answer that was not a guess. Here the
/// question does not arise: each message brings its own.
#[test]
fn two_turns_of_one_session_carry_two_different_speakers() {
    let eps = turn_episodes(json!([
        turn_row("0", "user", "my editor is helix"),
        turn_row("1", "user", "and i cook keto"),
    ]));
    assert_eq!(
        eps.len(),
        2,
        "two turns are two messages, not one body -- {eps:?}"
    );
    assert_ne!(
        hop(&eps[0], "turn_id"),
        hop(&eps[1], "turn_id"),
        "each message names its own turn -- {eps:?}"
    );

    // Two participants in one room, one turn each. The edge promotes the
    // speaker of the message it is carrying; nothing about the second turn can
    // reach the first.
    let speakers = ["member:alex", "member:robin"];
    let rows: Vec<Value> = eps
        .iter()
        .zip(speakers)
        .map(|(ep, who)| {
            episode_row(json!({
                "header": {"context": {
                    "session_id": hop(ep, "session_id"),
                    "turn_id": hop(ep, "turn_id"),
                    "happened_at": hop(ep, "happened_at"),
                    "audience_set": "[\"member:alex\",\"member:robin\"]",
                    "channel": "tg:group",
                    "speaker": who
                }},
                "messages": ep["messages"].clone()
            }))
        })
        .collect();

    assert_eq!(
        rows.iter()
            .map(|r| r["speaker"].clone())
            .collect::<Vec<_>>(),
        vec![json!("member:alex"), json!("member:robin")],
        "two messages, two identities -- {rows:?}"
    );
    assert_eq!(
        rows.iter()
            .map(|r| r["content"].clone())
            .collect::<Vec<_>>(),
        vec![json!("my editor is helix"), json!("and i cook keto")],
        "and each identity sits on the turn it belongs to -- {rows:?}"
    );
    assert_ne!(
        rows[0]["turn_id"], rows[1]["turn_id"],
        "the deterministic turn id survives the hop -- {rows:?}"
    );
    // The event time of the ROW, not of the writer's clock: the bi-temporal
    // split is what lets a per-message identity be re-read in order later.
    assert_eq!(rows[0]["happened_at"], json!("2026-08-23T09:00:00"));
    assert_eq!(rows[1]["happened_at"], json!("2026-08-23T09:00:01"));
}

// ═══════════════════════════════════════════ the SoT rule, written and asserted

const HIVE_README: &str = "../../templates/memory-hive/README.md";

/// GH #330 (Q12 half b, drift lock) -- the source-of-truth rule for identity
/// references is stated where it is read, and the writer behaves that way.
///
/// The rule has two halves and this template is the transporting side of both:
/// the round carries ONE name (`audience_set`, and no template may introduce a
/// second), and the references inside it are `affinity`'s alone to mint -- this
/// hive stores the string it was handed byte for byte and resolves nothing.
///
/// Both halves of a drift lock: the sentences are read out of the shipped
/// documents, and the mechanism they describe is asserted beside them. A README
/// that keeps the promise while the script starts mapping is red here, and so
/// is a script that keeps behaving while the promise is edited away.
#[test]
fn the_sot_rule_for_identity_references_is_stated_where_it_is_read() {
    // ── the prose, in the section the #244 lineage anchors
    let readme =
        std::fs::read_to_string(HIVE_README).unwrap_or_else(|e| panic!("{HIVE_README}: {e}"));
    let gate_section = readme
        .split("## The audience gate")
        .nth(1)
        .expect("§ The audience gate is where the rule and its vocabulary live");
    let gate_section = gate_section
        .split("\n## ")
        .next()
        .expect("a section ends at the next one of its own level");
    for sentence in [
        "no template may ever introduce a second name for it",
        "byte for byte",
        "looks an identity up",
    ] {
        assert!(
            gate_section.contains(sentence),
            "§ The audience gate no longer carries the SoT rule: {sentence:?} is \
             gone. The rule is a contract promise (GH #330), not a paragraph -- \
             move it deliberately or not at all"
        );
    }

    // ── the writer's own scope note, the sentence the claims registry pins
    let raw =
        std::fs::read_to_string(WRITER_CONFIG).unwrap_or_else(|e| panic!("{WRITER_CONFIG}: {e}"));
    let cfg: Value = meclaw_core::serde_json::from_str(&raw).expect("writer config json");
    let not_in_scope = cfg["description"]["not_in_scope"]
        .as_str()
        .expect("the writer declares a string `description.not_in_scope`");
    assert!(
        not_in_scope.contains("never LOOKS UP an identity"),
        "the writer's scope note stopped saying that it resolves nothing:\n  {not_in_scope}"
    );

    // ── the mechanism: the reference reaches the column untouched
    let script = script_of(WRITER_CONFIG, &[]);
    assert!(
        script.contains("speaker = str((ctx.get(\"speaker\") if origin == \"user\" else ctx.get(\"agent_id\")) or \"\")"),
        "the writer's one identity line changed shape -- whatever replaced it, \
         `a_present_speaker_is_written_exactly_as_it_arrived` is the assertion \
         that has to move with it"
    );
    for lookup in ["select", "traverse", "affinity", ".replace(", ".lower()"] {
        assert!(
            !script.contains(&format!("speaker{lookup}"))
                && !script.contains(&format!("{lookup}(speaker")),
            "the writer looks the speaker up or rewrites it (`{lookup}`) -- \
             affinity alone mints and maps an identity reference, and a second \
             mapping authority is the defect this rule exists to prevent"
        );
    }
}
