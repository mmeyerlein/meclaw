//! The close pass groups a session by round before any closer sees it
//! (GitHub #933).
//!
//! A session can hold turns of several audiences, and the close pass used to
//! put all of them into ONE prompt: a fact learned in one round could then be
//! written for, corrected from or shown to a round that never heard it. The
//! pass now runs one closer call per canonical audience, and three pure
//! functions of the shipped script decide what a call may see. This file pins
//! them as tables, so a change to the grouping rule is a change to a row here
//! rather than a silent widening of who reads what:
//!
//! * `canon` -- one spelling per participant set; anything that is not a
//!   non-empty set is NO audience, never an empty one.
//! * `groups` -- one group per canonical audience, oldest first; turns without
//!   an audience are only counted, and `["*"]` is a round of its own.
//! * `facts_for` -- equality, not a subset rule: a fact of a wider or narrower
//!   round is a leak once it is offered as a `sharpen` candidate.
//! * `topics_for` -- the same equality: a topic NAME is content.
//! * `parked_verdicts` -- the verdicts of THIS run only, one per group: a
//!   verdict parked by an earlier close of the session is never acted on.
//!
//! Below the tables, the two scripts are walked hop by hop the way the colony
//! would hand them their documents (the `run_pass` form of
//! `gh300_the_four_point_contract.rs`, every verdict injected, no model):
//!
//! * close-glue checks every verdict against ITS group -- an `add`, a
//!   correction or a topic closure that names another group's turn, record or
//!   topic is dropped and counted -- and writes, sweeps and reports once, after
//!   the last group; a failed closer of any group writes nothing at all;
//! * extract-glue writes a fact or an edge with the audience of its own turn
//!   (or the close pass's group audience, carried in the block and honoured only
//!   on the close lane), never with the round of the request, and counts every
//!   row it refuses for want of one.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const CLOSE_GLUE: &str = "../../templates/memory-hive/close-glue/config.json";
const EXTRACT_GLUE: &str = "../../templates/memory-hive/extract-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates the
/// template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The shipped close-glue script, or None where the templates are not shipped
/// with this checkout.
fn script() -> Option<String> {
    if !Path::new(CLOSE_GLUE).exists() {
        eprintln!("{CLOSE_GLUE} not present, skipping");
        return None;
    }
    let raw = std::fs::read_to_string(CLOSE_GLUE).expect("read close-glue");
    let v: Value = serde_json::from_str(&raw).expect("close-glue config is json");
    let src = v["params"]["script_inline"]
        .as_str()
        .expect("params.script_inline")
        .to_string();
    Some(resolve_vars(&src))
}

/// Call one function of the script on JSON arguments and return its result.
///
/// The script runs whole first, on a document whose phase it does not know, so
/// it falls through to `park()`; the `SystemExit` is caught and the function is
/// then called in the same globals. Every definition the run reached is the
/// definition the colony runs -- nothing is extracted or re-typed here.
fn call(script: &str, function: &str, args: &[Value]) -> Value {
    let doc = json!({
        "params": {},
        "envelope": {"header": {"context": {"session_id": "s-933", "mem_phase": "probe"},
                                "hop": {}}},
        "body": {}
    });
    let call_args: Vec<String> = args
        .iter()
        .map(|a| {
            format!(
                "json.loads({})",
                serde_json::to_string(&a.to_string()).unwrap()
            )
        })
        .collect();
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "_out = {}({})\n",
            "sys.stdout.write('\\n' + json.dumps(_out) + '\\n')\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&doc.to_string()).unwrap(),
        function,
        call_args.join(", "),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{function} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .expect("the call printed its result");
    serde_json::from_str(last).unwrap_or_else(|e| panic!("{function} result ({e}): {last}"))
}

/// One episode row of the window read.
fn episode(id: &str, audience: Value, at: &str) -> Value {
    json!({"id": id, "session_id": "s-933", "content": format!("turn {id}"),
           "happened_at": at, "recorded_at": at, "audience_set": audience})
}

/// The `groups` result reduced to (audience, episode ids) per group and the
/// unaudienced count.
fn grouped(script: &str, turns: &Value) -> (Vec<(Value, Vec<String>)>, u64) {
    let out = call(script, "groups", std::slice::from_ref(turns));
    let rounds = out[0]
        .as_array()
        .expect("a list of groups")
        .iter()
        .map(|g| {
            let mut ids: Vec<String> = g["turns"]
                .as_array()
                .expect("group turns")
                .iter()
                .map(|t| t["id"].as_str().unwrap_or("").to_string())
                .collect();
            ids.sort();
            (g["audience"].clone(), ids)
        })
        .collect();
    (rounds, out[1].as_u64().expect("unaudienced is a count"))
}

/// Episode ids as owned strings, the shape `grouped` returns.
fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn canon_is_one_spelling_per_set_and_nothing_else_is_an_audience() {
    let Some(s) = script() else { return };
    let table = [
        (Value::Null, Value::Null),
        (json!(""), Value::Null),
        (json!("[]"), Value::Null),
        (json!("not json"), Value::Null),
        (json!("{}"), Value::Null),
        (json!("7"), Value::Null),
        (json!([]), Value::Null),
        (json!(r#"["b","a","a"]"#), json!(["a", "b"])),
        (json!(["*"]), json!(["*"])),
        (json!(["", "x"]), json!(["x"])),
        (json!([""]), Value::Null),
    ];
    for (input, want) in table {
        assert_eq!(
            call(&s, "canon", std::slice::from_ref(&input)),
            want,
            "canon({input})"
        );
    }
}

#[test]
fn groups_are_rounds_oldest_first_and_turns_without_one_are_only_counted() {
    let Some(s) = script() else { return };
    let turns = json!([
        episode(
            "b-2",
            json!(r#"["member:e","agent:b"]"#),
            "2026-08-21T10:00:40Z"
        ),
        episode(
            "a-1",
            json!(r#"["agent:a","member:e"]"#),
            "2026-08-21T10:00:20Z"
        ),
        episode("n-1", Value::Null, "2026-08-21T10:00:00Z"),
        episode("n-2", json!("[]"), "2026-08-21T10:00:01Z"),
        // One audience, another spelling: same round.
        episode(
            "b-1",
            json!(r#"["agent:b","member:e","agent:b"]"#),
            "2026-08-21T10:00:10Z"
        ),
        episode("s-1", json!(r#"["*"]"#), "2026-08-21T10:00:30Z"),
    ]);
    let (rounds, unaudienced) = grouped(&s, &turns);
    assert_eq!(unaudienced, 2, "n-1 and n-2 are counted, never grouped");
    assert_eq!(
        rounds,
        vec![
            (json!(["agent:b", "member:e"]), owned(&["b-1", "b-2"])),
            (json!(["agent:a", "member:e"]), owned(&["a-1"])),
            (json!(["*"]), owned(&["s-1"])),
        ],
        "ordered by each round's earliest turn; `*` is a round of its own"
    );

    let (rounds, unaudienced) = grouped(&s, &json!([]));
    assert!(rounds.is_empty());
    assert_eq!(unaudienced, 0);

    let only_blind = json!([episode("n-1", json!("not json"), "2026-08-21T10:00:00Z")]);
    let (rounds, unaudienced) = grouped(&s, &only_blind);
    assert!(rounds.is_empty(), "no audience, no round: {rounds:?}");
    assert_eq!(unaudienced, 1);
}

#[test]
fn facts_for_is_equality_and_never_a_subset_rule() {
    let Some(s) = script() else { return };
    let fact = |id: &str, audience: Value| json!({"id": id, "audience_set": audience});
    let facts = json!([
        fact("same", json!(r#"["member:e","agent:a"]"#)),
        fact(
            "same-respelled",
            json!(r#"["agent:a","member:e","member:e"]"#)
        ),
        fact("wider", json!(r#"["agent:a","agent:b","member:e"]"#)),
        fact("narrower", json!(r#"["member:e"]"#)),
        fact("star", json!(r#"["*"]"#)),
        fact("null", Value::Null),
        fact("empty", json!("")),
    ]);
    let ids = |audience: Value| -> Vec<String> {
        call(&s, "facts_for", &[audience, facts.clone()])
            .as_array()
            .expect("a list of facts")
            .iter()
            .map(|f| f["id"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let table: [(Value, Vec<&str>); 4] = [
        (
            json!(["agent:a", "member:e"]),
            vec!["same", "same-respelled"],
        ),
        (json!(["agent:a", "agent:b", "member:e"]), vec!["wider"]),
        (json!(["*"]), vec!["star"]),
        (json!(["agent:c"]), vec![]),
    ];
    for (audience, want) in table {
        assert_eq!(ids(audience.clone()), want, "facts_for({audience})");
    }
}

#[test]
fn topics_for_is_the_same_equality_as_facts_for() {
    let Some(s) = script() else { return };
    let topic = |id: &str, audience: Value| {
        json!({"id": id, "name": format!("name {id}"),
                                                    "audience_set": audience})
    };
    let topics = json!([
        topic("same", json!(r#"["member:e","agent:a"]"#)),
        topic(
            "same-respelled",
            json!(r#"["agent:a","member:e","agent:a"]"#)
        ),
        topic("wider", json!(r#"["agent:a","agent:b","member:e"]"#)),
        topic("narrower", json!(r#"["member:e"]"#)),
        topic("star", json!(r#"["*"]"#)),
        topic("null", Value::Null),
        topic("empty", json!("")),
    ]);
    let ids = |audience: Value| -> Vec<String> {
        call(&s, "topics_for", &[audience, topics.clone()])
            .as_array()
            .expect("a list of topics")
            .iter()
            .map(|t| t["id"].as_str().unwrap_or("").to_string())
            .collect()
    };
    let table: [(Value, Vec<&str>); 5] = [
        (
            json!(["agent:a", "member:e"]),
            vec!["same", "same-respelled"],
        ),
        (json!(["agent:a", "agent:b", "member:e"]), vec!["wider"]),
        (json!(["member:e"]), vec!["narrower"]),
        (json!(["*"]), vec!["star"]),
        (json!(["agent:c"]), vec![]),
    ];
    for (audience, want) in table {
        assert_eq!(ids(audience.clone()), want, "topics_for({audience})");
    }
}

/// One `scratch` row of the meeting read.
fn parked(kind: &str, at: &str, payload: &str) -> Value {
    json!({"key": "close:s-933", "kind": kind, "created_at": at, "payload": payload})
}

#[test]
fn parked_verdicts_are_this_run_s_one_per_group() {
    let Some(s) = script() else { return };
    let verdict = |g: u64, tag: &str| json!({"close_group": g, "tag": tag}).to_string();
    let tags = |rows: Value| -> BTreeMap<String, String> {
        let out = call(&s, "parked_verdicts", std::slice::from_ref(&rows));
        out.as_object()
            .expect("a map group -> verdict")
            .iter()
            .map(|(g, v)| (g.clone(), v["tag"].as_str().unwrap_or("").to_string()))
            .collect()
    };
    let want = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(g, t)| ((*g).to_string(), (*t).to_string()))
            .collect()
    };
    // The meeting answer, newest first: the second close of the session has
    // parked its sets and ONE verdict (group 0); the first close left a verdict
    // for each of its two groups underneath.
    let second_close = json!([
        parked("verdict", "2026-10-01T10:00:08.000000Z", "not json"),
        parked(
            "verdict",
            "2026-10-01T10:00:07.000000Z",
            &verdict(0, "run2-g0-newest")
        ),
        parked(
            "verdict",
            "2026-10-01T10:00:06.000000Z",
            &verdict(0, "run2-g0")
        ),
        parked("turns", "2026-10-01T10:00:05.000000Z", "{}"),
        parked(
            "verdict",
            "2026-10-01T10:00:04.000000Z",
            &verdict(1, "run1-g1")
        ),
        parked(
            "verdict",
            "2026-10-01T10:00:03.000000Z",
            &verdict(0, "run1-g0")
        ),
        parked("turns", "2026-10-01T10:00:01.000000Z", "{}"),
    ]);
    assert_eq!(
        tags(second_close),
        want(&[("0", "run2-g0-newest")]),
        "the run boundary is this run's `turns` row: group 1 of the FIRST close is \
         not a verdict of this one, and an unreadable payload is no verdict"
    );
    let first_close = json!([
        parked(
            "verdict",
            "2026-10-01T10:00:04.000000Z",
            &verdict(1, "run1-g1")
        ),
        parked(
            "verdict",
            "2026-10-01T10:00:03.000000Z",
            &verdict(0, "run1-g0")
        ),
        parked("turns", "2026-10-01T10:00:01.000000Z", "{}"),
    ]);
    assert_eq!(
        tags(first_close),
        want(&[("0", "run1-g0"), ("1", "run1-g1")])
    );
    let no_boundary = json!([parked(
        "verdict",
        "2026-10-01T10:00:03.000000Z",
        &verdict(0, "orphan")
    )]);
    assert!(
        tags(no_boundary).is_empty(),
        "without this run's turns row no verdict is this run's"
    );
}

// ─────────────────────────────────────────────── walking the scripts hop by hop

const SESSION: &str = "s-933";
const ROUND_EA: &str = r#"["agent:a","member:e"]"#;
const ROUND_EB: &str = r#"["agent:b","member:e"]"#;

/// Run a shipped script over one flatly spelled message; (emission, stderr).
fn run(path: &str, flat: &Value) -> (Vec<Value>, String) {
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(path),
        &meclaw_testing::code_stdin(flat).to_string(),
    );
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{path} exited non-zero: {err}");
    let msgs = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    });
    (msgs, err)
}

fn args_of(msg: &Value) -> Value {
    serde_json::from_str(msg["messages"][0]["text"].as_str().expect("op text")).expect("op args")
}

fn route(msg: &Value) -> &str {
    msg["header"]["route"].as_str().unwrap_or("")
}

/// The close lane's context; after a closer call it carries the group the
/// edge to the closer promoted (`set_context close_group`).
fn close_ctx(phase: &str, group: Option<usize>) -> Value {
    let mut c = json!({"store_origin": "close", "mem_phase": phase, "session_id": SESSION,
                       "channel": "c-933", "audience_set": ROUND_EA});
    if let Some(g) = group {
        c["close_group"] = json!(g.to_string());
    }
    c
}

/// One session of two rounds and a turn without one: `ea1`/`ea2` in round
/// `{e,a}`, `eb1`/`eb2` in round `{e,b}`, `en` in none; one open fact and one
/// open topic per round.
fn two_rounds(table: &str) -> Value {
    let ep = |id: &str, speaker: &str, aud: Value, at: &str| {
        json!({"id": id, "session_id": SESSION, "sender": "user", "speaker": speaker,
               "content": format!("SECRET-{} said here", id.to_uppercase()),
               "happened_at": at, "recorded_at": at, "audience_set": aud})
    };
    let fact = |id: &str, episode: &str, aud: &str| {
        json!({"id": id, "episode_id": episode, "subject": format!("subject {id}"),
               "canonical_subject": format!("subject {id}"), "predicate": "has_code",
               "canonical_predicate": "has_code", "claim": format!("claim {id}"),
               "canonical_claim": format!("claim {id}"), "fact_kind": "world",
               "confidence": 70, "valid_from": "2026-10-01T10:00:00Z",
               "recorded_at": "2026-10-01T10:00:00Z", "audience_set": aud})
    };
    match table {
        "episodes" => json!([
            ep("en", "member:e", Value::Null, "2026-10-01T10:00:04Z"),
            ep("eb2", "agent:b", json!(ROUND_EB), "2026-10-01T10:00:03Z"),
            ep("ea2", "agent:a", json!(ROUND_EA), "2026-10-01T10:00:02Z"),
            ep("eb1", "member:e", json!(ROUND_EB), "2026-10-01T10:00:01Z"),
            ep("ea1", "member:e", json!(ROUND_EA), "2026-10-01T10:00:00Z"),
        ]),
        "facts" => json!([fact("f-ea", "ea1", ROUND_EA), fact("f-eb", "eb1", ROUND_EB)]),
        "topics" => json!([
            {"id": "t-ea", "name": "TOPIC-EA", "session_id": SESSION,
             "opened_episode_id": "ea1", "opened_at": "2026-10-01T10:00:00Z",
             "audience_set": ROUND_EA},
            {"id": "t-eb", "name": "TOPIC-EB", "session_id": SESSION,
             "opened_episode_id": "eb1", "opened_at": "2026-10-01T10:00:01Z",
             "audience_set": ROUND_EB},
        ]),
        _ => json!([]),
    }
}

/// What one walk of the close lane produced.
struct Walk {
    /// Every emission of close-glue, in order.
    emissions: Vec<Vec<Value>>,
    /// Every closer prompt: (group, the raw user text).
    prompts: Vec<(usize, String)>,
}

impl Walk {
    fn last(&self) -> &[Value] {
        self.emissions.last().expect("at least one emission")
    }

    /// Every store op of the whole walk.
    fn ops(&self) -> Vec<Value> {
        self.emissions
            .iter()
            .flatten()
            .filter(|m| route(m) == "cstore")
            .map(args_of)
            .collect()
    }

    fn count(&self, route_name: &str) -> usize {
        self.emissions
            .iter()
            .flatten()
            .filter(|m| route(m) == route_name)
            .count()
    }

    fn report(&self) -> Value {
        self.last()
            .iter()
            .find(|m| route(m) == "close_report")
            .map(|m| m["header"].clone())
            .unwrap_or_else(|| panic!("the walk ends in a close_report: {:?}", self.last()))
    }

    fn blocks(&self) -> Vec<Value> {
        self.emissions
            .iter()
            .flatten()
            .filter(|m| route(m) == "close_write")
            .map(args_of)
            .collect()
    }
}

/// Walk one close pass: every read answered from `rows` (and a real `scratch`,
/// newest first), every closer call answered by `answer(group, prompt)` --
/// `None` is a failed call (`finish_reason: error`).
fn walk(rows: &dyn Fn(&str) -> Value, answer: &dyn Fn(usize, &Value) -> Option<Value>) -> Walk {
    let mut scratch: Vec<Value> = Vec::new();
    let mut group: Option<usize> = None;
    let mut emissions = Vec::new();
    let mut prompts = Vec::new();
    let mut msgs = run(
        CLOSE_GLUE,
        &json!({"header": {"context": close_ctx("window", None),
                           "hop": {"route": "in_close_pass"}},
                "messages": [{"origin": "user", "type": "text", "id": "m1", "text": "close"}]}),
    )
    .0;
    for _ in 0..80 {
        emissions.push(msgs.clone());
        if msgs
            .iter()
            .any(|m| route(m) == "close_report" || route(m) == "reject")
        {
            return Walk { emissions, prompts };
        }
        if let Some(ask) = msgs.iter().find(|m| route(m) == "close") {
            let g: usize = ask["header"]["close_group"]
                .as_str()
                .and_then(|s| s.parse().ok())
                .expect("the ask names its group");
            let text = ask["messages"][0]["text"]
                .as_str()
                .expect("prompt text")
                .to_string();
            let doc: Value = serde_json::from_str(&text).expect("the prompt is json");
            prompts.push((g, text));
            group = Some(g);
            let (finish, verdict) = match answer(g, &doc) {
                Some(v) => ("stop", v),
                None => ("error", json!({})),
            };
            msgs = run(
                CLOSE_GLUE,
                &json!({"header": {"context": close_ctx("verdict", group),
                                   "hop": {"finish_reason": finish}},
                        "messages": [{"origin": "assistant", "type": "text", "id": "v",
                                      "text": verdict.to_string()}]}),
            )
            .0;
            continue;
        }
        let ops: Vec<&Value> = msgs.iter().filter(|m| route(m) == "cstore").collect();
        assert_eq!(ops.len(), 1, "the read chain is sequential: {msgs:?}");
        let args = args_of(ops[0]);
        let phase = ops[0]["header"]["phase"]
            .as_str()
            .expect("phase")
            .to_string();
        let operation = args["operation"].as_str().unwrap_or("").to_string();
        let table = args["table"].as_str().unwrap_or("");
        let answer_rows = match (operation.as_str(), table) {
            ("insert", "scratch") => {
                scratch.push(args["row"].clone());
                json!([])
            }
            ("select", "scratch") => {
                let limit = args["limit"].as_u64().unwrap_or(u64::MAX) as usize;
                Value::Array(
                    scratch
                        .iter()
                        .rev()
                        .filter(|r| r["key"] == args["where"]["key"])
                        .take(limit)
                        .cloned()
                        .collect(),
                )
            }
            ("select", t) => rows(t),
            _ => json!([]),
        };
        msgs = run(
            CLOSE_GLUE,
            &json!({"header": {"context": close_ctx(&phase, group),
                               "hop": {"operation": operation, "rows_affected": 1}},
                    "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                                  "text": answer_rows.to_string()}]}),
        )
        .0;
    }
    panic!("the close pass did not terminate within 80 round trips");
}

fn nothing() -> Value {
    json!({"nothing_to_add": true, "add": [], "sharpen": [], "correct": [], "close_topics": []})
}

fn add(episode: &str, claim: &str) -> Value {
    json!({"episode_id": episode, "subject": format!("subject {claim}"),
           "predicate": "has_code", "claim": claim, "fact_kind": "world", "confidence": 80})
}

#[test]
fn each_group_sees_only_its_round_and_the_pass_writes_once_at_the_end() {
    if script().is_none() {
        return;
    }
    let w = walk(&two_rounds, &|g, _| {
        Some(match g {
            0 => json!({"nothing_to_add": false, "add": [add("ea2", "claim from ea")],
                        "sharpen": [], "correct": [],
                        "close_topics": [{"id": "t-ea", "ended_episode_id": "ea2"}]}),
            _ => json!({"nothing_to_add": false, "add": [add("eb1", "claim from eb")],
                        "sharpen": [], "correct": [],
                        "close_topics": [{"id": "t-eb", "ended_episode_id": "eb2"}]}),
        })
    });

    // One prompt per round, oldest round first; each holds its own round and
    // NOTHING of the other: no text, no id, no speaker, no fact, no topic.
    let groups: Vec<usize> = w.prompts.iter().map(|(g, _)| *g).collect();
    assert_eq!(groups, vec![0, 1], "one closer call per round");
    let rounds = [
        (
            &w.prompts[0].1,
            ["SECRET-EA1", "SECRET-EA2", "TOPIC-EA", "f-ea"],
            [
                "SECRET-EB",
                "\"eb1\"",
                "\"eb2\"",
                "agent:b",
                "TOPIC-EB",
                "t-eb",
                "f-eb",
            ],
        ),
        (
            &w.prompts[1].1,
            ["SECRET-EB1", "SECRET-EB2", "TOPIC-EB", "f-eb"],
            [
                "SECRET-EA",
                "\"ea1\"",
                "\"ea2\"",
                "agent:a",
                "TOPIC-EA",
                "t-ea",
                "f-ea",
            ],
        ),
    ];
    for (text, own, foreign) in rounds {
        for o in own {
            assert!(text.contains(o), "{o} is offered to its own round: {text}");
        }
        for f in foreign {
            assert!(
                !text.contains(f),
                "{f} of the other round is in this prompt: {text}"
            );
        }
        assert!(
            !text.contains("SECRET-EN") && !text.contains("\"en\""),
            "the turn without a round is in no prompt: {text}"
        );
    }

    // Between the two calls the lane asks the next group and does NOTHING
    // else: no write, no sweep, no report.
    let second_ask = w
        .emissions
        .iter()
        .position(|e| {
            e.iter()
                .any(|m| route(m) == "close" && m["header"]["close_group"] == "1")
        })
        .expect("the second group is asked");
    assert_eq!(
        w.emissions[second_ask].len(),
        1,
        "the ask of group 1 travels alone: {:?}",
        w.emissions[second_ask]
    );

    // The sweep and the report: exactly once, in the last emission.
    let sweeps = |ops: &[Value]| {
        ops.iter()
            .filter(|a| a["table"] == "pending_extraction" && a["operation"] == "update")
            .count()
    };
    assert_eq!(
        sweeps(w.ops().as_slice()),
        1,
        "one sweep per pass: {:?}",
        w.ops()
    );
    let last_ops: Vec<Value> = w
        .last()
        .iter()
        .filter(|m| route(m) == "cstore")
        .map(args_of)
        .collect();
    assert_eq!(
        sweeps(last_ops.as_slice()),
        1,
        "and it is the last emission's"
    );
    assert_eq!(w.count("close_report"), 1);
    let report = w.report();
    assert_eq!(report["groups"], 2, "{report}");
    assert_eq!(report["unaudienced"], 1, "{report}");
    assert_eq!(report["added"], 2, "{report}");
    assert_eq!(report["closed"], 2, "{report}");
    assert_eq!(report["unseen_refs"], 0, "{report}");

    // Every block carries the audience of ITS group.
    let blocks = w.blocks();
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    for b in &blocks {
        let want = if b["episode_id"] == "ea2" {
            json!(["agent:a", "member:e"])
        } else {
            json!(["agent:b", "member:e"])
        };
        let got: Value = serde_json::from_str(b["audience_set"].as_str().expect("a string"))
            .expect("the block audience is json");
        assert_eq!(got, want, "{b}");
    }
}

#[test]
fn a_verdict_that_names_another_round_is_dropped_and_counted() {
    if script().is_none() {
        return;
    }
    // The closer of round {e,a} names round {e,b}'s turn, record and topic in
    // every slot a verdict has -- and a topic of its own with a foreign end.
    let w = walk(&two_rounds, &|g, _| {
        Some(match g {
            0 => json!({
                "nothing_to_add": false,
                "add": [add("eb1", "leaked claim")],
                "sharpen": [{"fact_id": "f-eb", "subject": "subject f-eb",
                             "predicate": "has_code", "claim": "sharper", "why": "x"}],
                "correct": [{"fact_id": "f-eb", "subject": "subject f-eb",
                             "predicate": "has_code", "claim": "fixed", "why": "x"}],
                "close_topics": [{"id": "t-eb", "ended_episode_id": "ea1"},
                                 {"id": "t-ea", "ended_episode_id": "eb1"}]}),
            _ => nothing(),
        })
    });
    let report = w.report();
    assert!(
        w.blocks().is_empty(),
        "nothing is written: {:?}",
        w.blocks()
    );
    assert!(
        // The walk also records the topic READ of the gather phase; a close
        // is an `update`, so only writes count here.
        !w.ops()
            .iter()
            .any(|a| a["table"] == "topics" && a["operation"] != "select"),
        "no topic is closed: {:?}",
        w.ops()
    );
    for key in ["added", "sharpened", "corrected", "closed"] {
        assert_eq!(report[key], 0, "{key}: {report}");
    }
    assert_eq!(
        report["unseen_refs"], 5,
        "every reference across the round is counted: {report}"
    );
}

#[test]
fn a_failed_closer_of_a_later_group_writes_nothing_and_sweeps_nothing() {
    if script().is_none() {
        return;
    }
    let w = walk(&two_rounds, &|g, _| match g {
        0 => Some(
            json!({"nothing_to_add": false, "add": [add("ea1", "claim from ea")],
                         "sharpen": [], "correct": [],
                         "close_topics": [{"id": "t-ea", "ended_episode_id": "ea1"}]}),
        ),
        _ => None,
    });
    assert_eq!(w.prompts.len(), 2, "both groups were asked");
    assert!(
        w.last().iter().any(|m| route(m) == "reject"),
        "the pass ends refused: {:?}",
        w.last()
    );
    assert_eq!(w.count("close_write"), 0, "group 0's add is not written");
    assert_eq!(w.count("close_report"), 0);
    let writes: Vec<Value> = w
        .ops()
        .into_iter()
        .filter(|a| a["table"] != "scratch")
        .filter(|a| a["operation"] != "select")
        .collect();
    assert!(
        writes.is_empty(),
        "no topic closed, no sweep of the exception list: {writes:?}"
    );
}

// ─────────────────────────────────────────────── extract-glue, the write side

const BATCH: &str = "b-933";

/// The apply phase of extract-glue over one staged payload and its episodes.
fn apply(facts: Value, edges: Value, episodes: Value) -> (Vec<Value>, String) {
    let payload = json!({"facts": facts, "entities": [], "edges": edges});
    let rows = json!([
        {"key": BATCH, "kind": "payload", "payload": payload.to_string()},
        {"key": BATCH, "kind": "known", "payload": "[]"},
        {"key": BATCH, "kind": "episodes", "payload": episodes.to_string()},
    ]);
    let (msgs, err) = run(
        EXTRACT_GLUE,
        &json!({"header": {"context": {"store_origin": "extract", "mem_phase": "apply",
                                       "batch_id": BATCH},
                           "hop": {"operation": "select", "rows_affected": 1}},
                "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                              "text": rows.to_string()}]}),
    );
    (msgs.iter().map(args_of).collect(), err)
}

/// One staged fact as the inline phase leaves it: `audience_set` is the
/// close pass's group audience or the round of a per-turn inline block's own
/// turn -- never the round that requested a close (see the staging test below).
fn staged(episode: &str, claim: &str, staged_audience: &str) -> Value {
    json!({"episode_id": episode, "subject": "subject", "predicate": "has_code",
           "claim": claim, "fact_kind": "world", "valid_from": "", "valid_until": null,
           "confidence": 80, "replaces": "", "claim_hash": claim.replace(' ', "_"),
           "channel": "c-933", "audience_set": staged_audience})
}

fn edge(episode: &str) -> Value {
    json!({"episode_id": episode, "src_entity": "x", "dst_entity": "y",
           "edge_kind": "knows", "weight": 1, "channel": "c-933", "audience_set": ""})
}

fn episode_rows(pairs: &[(&str, Option<&str>)]) -> Value {
    let mut m = serde_json::Map::new();
    for (id, aud) in pairs {
        let mut row = json!({"happened_at": "2026-10-01T10:00:00Z", "session_id": SESSION,
                             "channel": "c-933"});
        if let Some(a) = aud {
            row["audience_set"] = json!(a);
        }
        m.insert((*id).to_string(), row);
    }
    Value::Object(m)
}

/// The `extract-missing-audience` receipt of an apply, if it wrote one.
fn missing(ops: &[Value]) -> Option<Value> {
    ops.iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "extract-missing-audience")
        .map(|a| serde_json::from_str(a["row"]["payload"].as_str().expect("payload")).unwrap())
}

#[test]
fn the_apply_counts_every_row_it_drops_for_want_of_an_audience() {
    if !Path::new(EXTRACT_GLUE).exists() {
        return;
    }
    // A block that carries only an edge, on a turn without an audience: the
    // edge is dropped, and the drop is on the record -- never silent.
    let (ops, err) = apply(
        json!([]),
        json!([edge("e-none")]),
        episode_rows(&[("e-none", None)]),
    );
    assert!(
        !ops.iter().any(|a| a["table"] == "entity_edges"),
        "no edge without an audience: {ops:?}"
    );
    assert_eq!(
        missing(&ops),
        Some(json!({"missing_audience": 1, "episode_ids": ["e-none"]})),
        "the dropped edge is counted: {ops:?}"
    );
    assert!(err.contains("without an audience"), "and said: {err}");

    // A fact and an edge, both dropped: one receipt counting both. Beside
    // them an edge with an audience is written, so the count is not vacuous.
    let (ops, _) = apply(
        json!([staged("e-none", "a claim", "")]),
        json!([edge("e-other"), edge("e-ea")]),
        episode_rows(&[
            ("e-none", None),
            ("e-other", None),
            ("e-ea", Some(ROUND_EA)),
        ]),
    );
    assert_eq!(
        missing(&ops),
        Some(json!({"missing_audience": 2, "episode_ids": ["e-none", "e-other"]})),
        "{ops:?}"
    );
    let written: Vec<&Value> = ops
        .iter()
        .filter(|a| a["table"] == "entity_edges" && a["operation"] == "insert")
        .collect();
    assert_eq!(written.len(), 1, "{ops:?}");
    assert_eq!(written[0]["row"]["audience_set"], ROUND_EA);
    assert!(
        !ops.iter()
            .any(|a| a["table"] == "facts" && a["operation"] == "insert"),
        "{ops:?}"
    );
}

#[test]
fn a_fact_takes_the_audience_of_its_turn_and_never_the_request_s() {
    if !Path::new(EXTRACT_GLUE).exists() {
        return;
    }
    let eb_block = r#"["agent:b", "member:e"]"#;
    // (episode audience, staged audience, what the fact is written with).
    let table: [(Option<&str>, &str, Option<&str>); 5] = [
        // No audience of its own turn, none staged: not written, counted.
        (None, "", None),
        // The staged audience (the close pass's group, carried in the block).
        (None, eb_block, Some(eb_block)),
        // The episode's own, verbatim (an audit compares text).
        (
            Some(r#"["member:e","agent:a"]"#),
            "",
            Some(r#"["member:e","agent:a"]"#),
        ),
        // The same set, spelled twice: the episode's spelling wins.
        (
            Some(r#"["member:e","agent:a"]"#),
            r#"["agent:a", "member:e"]"#,
            Some(r#"["member:e","agent:a"]"#),
        ),
        // Episode and stage disagree: refused, never guessed between.
        (Some(ROUND_EA), eb_block, None),
    ];
    for (ep_aud, block, want) in table {
        let fact = staged("e-1", "a claim", block);
        let (ops, _) = apply(json!([fact]), json!([]), episode_rows(&[("e-1", ep_aud)]));
        let inserts: Vec<&Value> = ops
            .iter()
            .filter(|a| a["table"] == "facts" && a["operation"] == "insert")
            .collect();
        let case = format!("episode {ep_aud:?}, staged {block:?}");
        match want {
            Some(aud) => {
                assert_eq!(inserts.len(), 1, "{case}: {ops:?}");
                assert_eq!(inserts[0]["row"]["audience_set"], aud, "{case}");
                assert_eq!(missing(&ops), None, "{case}");
            }
            None => {
                assert!(inserts.is_empty(), "{case}: {ops:?}");
                assert_eq!(
                    missing(&ops),
                    Some(json!({"missing_audience": 1, "episode_ids": ["e-1"]})),
                    "{case}"
                );
            }
        }
    }
}

#[test]
fn the_close_lane_stages_its_group_and_never_the_round_that_asked() {
    if !Path::new(EXTRACT_GLUE).exists() {
        return;
    }
    // The request comes in under round {e,a}. A close-pass block stages the
    // group audience it carries ({e,b}) or nothing -- never the request's
    // round. A per-turn inline block is written IN its turn, so the ingress
    // round is that turn's own and is staged as before (gh244, gh849); a
    // model-declared `audience_set` in such a block is ignored.
    let staged_facts = |close_pass: bool, block_aud: Option<&str>| -> Vec<Value> {
        let mut block = json!({"episode_id": "e-1",
                               "facts": [{"episode_id": "e-1", "subject": "Alex",
                                          "predicate": "favorite_color", "claim": "blue",
                                          "fact_kind": "world", "confidence": 80}]});
        if let Some(a) = block_aud {
            block["audience_set"] = json!(a);
        }
        let mut ctx = json!({"mem_phase": "inline", "session_id": SESSION,
                             "channel": "c-933", "audience_set": ROUND_EA});
        if close_pass {
            ctx["close_pass"] = json!("1");
        }
        let (msgs, _) = run(
            EXTRACT_GLUE,
            &json!({"header": {"context": ctx, "hop": {}},
                    "messages": [{"origin": "assistant", "type": "tool_call", "id": "b",
                                  "text": block.to_string()}]}),
        );
        let payload = msgs
            .iter()
            .filter(|m| route(m) == "xstore")
            .map(args_of)
            .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
            .unwrap_or_else(|| panic!("the payload is staged: {msgs:?}"));
        let parsed: Value =
            serde_json::from_str(payload["row"]["payload"].as_str().expect("payload"))
                .expect("payload json");
        parsed["facts"].as_array().cloned().unwrap_or_default()
    };
    let set = |s: &str| -> Vec<String> {
        let mut v: Vec<String> = serde_json::from_str(s).expect("json list");
        v.sort();
        v
    };
    let ea = set(ROUND_EA);
    let eb = set(ROUND_EB);
    let cases = [
        (true, Some(ROUND_EB), Some(&eb)),
        (true, None, None),
        (false, Some(ROUND_EB), Some(&ea)),
        (false, None, Some(&ea)),
    ];
    for (close_pass, block_aud, want) in cases {
        let facts = staged_facts(close_pass, block_aud);
        assert_eq!(facts.len(), 1, "{facts:?}");
        let f = &facts[0];
        let got = match f["audience_set"].as_str() {
            Some("") | None => None,
            Some(s) => Some(set(s)),
        };
        assert_eq!(
            got.as_ref(),
            want,
            "close_pass={close_pass}, block {block_aud:?}: {f}"
        );
    }
}
