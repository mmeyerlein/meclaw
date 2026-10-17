//! GH #1249 -- the memory hive finds the questions that come back and sends
//! prewarm candidates for them.
//!
//! Three units, one test each, plus the scale of `cosine_min`:
//!
//! - T4-14: `./recur` groups a member's questions by the cosine of their
//!   episode vectors (>= 0.82, the float cosine the stored sign bits stand
//!   for), keeps a routine only when it came back on four
//!   distinct local days of the fourteen before the night at one time of day
//!   (+-30 min), and states it as an rrule with BYDAY, read in the time zone
//!   handed to it.
//! - T4-15: the night arms it -- `./dream-glue` sends `recur` when its receipt
//!   is booked (phase `closed`), no clock of its own (R-AG-1) -- and it reads
//!   only this hive's store: `episodes` and their `embeddings`.
//! - T4-16: the dreamer's candidates leave the hive on `recurring`, and only
//!   those with a `ttl_min` and a tool (R-WP-6).
//!
//! No colony boots: the shipped scripts run over the documents the store and
//! the dreamer would hand them, and the wiring is read off the hive's graph.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Value, json};

const RECUR: &str = "../../templates/memory-hive/recur/config.json";
const GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";
const HIVE: &str = "../../templates/memory-hive/config.json";
const NIGHT: &str = "2026-10-15T01:00:00Z";

/// Run a shipped code cell over `header` and `messages` -> the messages it emits.
fn run(config: &str, header: Value, messages: Value) -> Vec<Value> {
    let doc = json!({"header": header, "messages": messages});
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(config),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{config}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).expect("a message array")
}

/// A program over the shipped recur script's functions, its stdout. The
/// script runs over a message of no phase first (it parks), so every function
/// is defined.
fn probe(program: &str) -> String {
    let script = meclaw_testing::shipped_script(RECUR);
    let src = format!(
        concat!(
            "import sys, io, json\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO(json.dumps({{\"envelope\": {{\"header\": {{\"context\": ",
            "{{\"mem_phase\": \"probe\"}}}}}}, \"body\": {{}}, \"params\": {{}}}}))\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'cell', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}\n"
        ),
        meclaw_core::serde_json::to_string(&script).expect("script"),
        program
    );
    let mut child = std::process::Command::new("python3")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("python3");
    {
        use std::io::Write;
        let mut sink = child.stdin.take().expect("stdin");
        sink.write_all(src.as_bytes()).expect("write");
    }
    let out = child.wait_with_output().expect("python3 ran");
    assert!(
        out.status.success(),
        "probe: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn edges() -> Vec<Value> {
    let raw = std::fs::read_to_string(HIVE).expect("hive config");
    let hive: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

fn edge(from: &str, to: &str, names: &str) -> Value {
    edges()
        .into_iter()
        .find(|e| {
            e["from"] == json!(from)
                && e["to"] == json!(to)
                && e["condition"].as_str().unwrap_or("").contains(names)
        })
        .unwrap_or_else(|| panic!("the hive wires {from} -> {to} on {names}"))
}

/// The ops of one store message, parsed out of its tool_call turns.
fn ops_of(msg: &Value) -> Vec<Value> {
    msg["messages"]
        .as_array()
        .expect("turns")
        .iter()
        .filter_map(|t| t["text"].as_str())
        .map(|t| meclaw_core::serde_json::from_str(t).expect("an op"))
        .collect()
}

#[test]
fn t4_14_a_question_back_on_four_days_at_one_time_is_a_routine() {
    // Mondays and Wednesdays at 07:25-07:40 in Berlin (UTC+2 in October), one
    // noon asking of the same question, one question of another meaning, one
    // asking before the window. 64-bit vectors, a few bits apart.
    let out = probe(concat!(
        "def it(i, at, flips, text='weather today?', vec=None):\n",
        "    v = bytearray(b'\\xff' * 8)\n",
        "    for f in flips:\n",
        "        v[f // 8] ^= 1 << (f % 8)\n",
        "    return {'id': i, 'vec': bytes(v) if vec is None else vec, 'at': parse_at(at),\n",
        "            'aud': audience_key('[\"member:m\"]'), 'text': text}\n",
        "items = [it('e1', '2026-10-05T05:25:00Z', [1]), it('e2', '2026-10-07T05:35:00Z', [2, 9]),\n",
        "         it('e3', '2026-10-12T05:30:00Z', []), it('e4', '2026-10-14T05:40:00Z', [3]),\n",
        "         it('x-noon', '2026-10-13T10:00:00Z', [4]), it('x-old', '2026-09-28T05:30:00Z', []),\n",
        "         it('x-other', '2026-10-06T05:30:00Z', [], 'call mum', bytes(8))]\n",
        "four = find_routines(items, '2026-10-15T01:00:00Z', 'Europe/Berlin')\n",
        "three = find_routines([x for x in items if x['id'] != 'e4'], '2026-10-15T01:00:00Z',\n",
        "                       'Europe/Berlin')\n",
        "print(json.dumps({'four': [{k: r[k] for k in ('rrule', 'tz', 'days', 'episode_ids')}\n",
        "                           for r in four], 'three': len(three)}))\n",
    ));
    let got: Value = meclaw_core::serde_json::from_str(&out).expect("json");
    assert_eq!(
        got,
        json!({"four": [{"rrule": "FREQ=WEEKLY;BYDAY=MO,WE;BYHOUR=7;BYMINUTE=30",
                         "tz": "Europe/Berlin", "days": 4,
                         "episode_ids": ["e1", "e2", "e3", "e4"]}],
               "three": 0}),
        "one routine of four days at 07:30 local on MO,WE; three days are none"
    );
}

#[test]
fn t4_14_cosine_min_reads_as_the_float_cosine_of_the_episode_vectors() {
    // Two 1024-bit sign vectors that differ in the share of bits a float cosine
    // c leaves on average (acos(c) / pi): just above 0.82 they are one
    // question, just below two.
    let out = probe(concat!(
        "import math, random\n",
        "def pair(c):\n",
        "    rng = random.Random(1249)\n",
        "    seed = bytes(rng.getrandbits(8) for _ in range(128))\n",
        "    v = bytearray(seed)\n",
        "    for b in rng.sample(range(1024), round(1024 * math.acos(c) / math.pi)):\n",
        "        v[b // 8] ^= 1 << (b % 8)\n",
        "    at, aud = parse_at('2026-10-05T05:30:00Z'), audience_key('[\"member:m\"]')\n",
        "    return [{'id': 'a', 'vec': seed, 'at': at, 'aud': aud, 'text': 'a'},\n",
        "            {'id': 'b', 'vec': bytes(v), 'at': at + datetime.timedelta(days=2),\n",
        "             'aud': aud, 'text': 'b'}]\n",
        "print(json.dumps({'above': len(group_questions(pair(0.83))),\n",
        "                  'below': len(group_questions(pair(0.81)))}))\n",
    ));
    let got: Value = meclaw_core::serde_json::from_str(&out).expect("json");
    assert_eq!(
        got,
        json!({"above": 1, "below": 2}),
        "cosine_min 0.82 is the float cosine the sign bits stand for"
    );
}

#[test]
fn t4_15_the_night_arms_recur_and_recur_reads_only_its_own_store() {
    let booked = run(
        GLUE,
        json!({"context": {"store_origin": "dream", "mem_phase": "closed",
                           "dream_run": "r-1249", "dream_to": NIGHT},
               "hop": {"operation": "update", "rows_affected": 1}}),
        json!([{"origin": "tool", "type": "tool_result", "id": "d-closed", "text": "[]"}]),
    );
    assert!(
        booked
            .iter()
            .any(|m| m["header"]["route"] == "recur" && m["header"]["phase"] == "recur-arm"),
        "the booked night arms recur: {booked:?}"
    );

    let read = run(
        RECUR,
        json!({"context": {"store_origin": "recur", "mem_phase": "recur-arm",
                           "dream_run": "r-1249", "dream_to": NIGHT}, "hop": {}}),
        json!([{"origin": "assistant", "type": "text", "text": "{}"}]),
    );
    assert_eq!(read.len(), 1, "{read:?}");
    assert_eq!(read[0]["header"]["route"], "rstore");
    let ops = ops_of(&read[0]);
    let tables: Vec<&str> = ops.iter().filter_map(|o| o["table"].as_str()).collect();
    assert_eq!(tables, vec!["episodes", "embeddings"]);
    assert!(ops.iter().all(|o| o["operation"] == "select"), "{ops:?}");
    assert_eq!(
        ops[0]["where"]["happened_at"],
        json!({"gte": "2026-10-01T01:00:00Z"})
    );
    assert_eq!(ops[1]["where"]["owner_table"], json!("episodes"));
    // A full page keeps the newest turns of the window: both reads run newest first.
    assert_eq!(
        ops[0]["order_by"],
        json!([{"col": "happened_at", "dir": "desc"}])
    );
    assert_eq!(
        ops[1]["order_by"],
        json!([{"col": "created_at", "dir": "desc"}])
    );

    let door = edge("./dream-glue", "./recur", "'recur'");
    assert_eq!(door["modifier"]["restore_ttl"], json!(true));
    edge("./recur", "./store", "'rstore'");
    edge("./store", "./recur", "context.store_origin == 'recur'");
}

#[test]
fn t4_16_the_dreamers_candidates_leave_on_recurring_and_only_with_ttl_and_a_tool() {
    let routine = |group: &str| {
        json!({"group": group, "rrule": "FREQ=DAILY;BYHOUR=7;BYMINUTE=30",
               "tz": "Europe/Berlin", "days": 5, "audience_set": ["member:m"],
               "triggers": ["weather today?"], "episode_ids": ["e1", "e2"],
               "at_min": 450})
    };
    let routines = json!([routine("g1"), routine("g2"), routine("g3")]);
    let answer = json!({"candidates": [
        {"group": "g1", "topic": "weather", "query": "weather in Berlin today",
         "tool": "weather", "lead_min": 15, "ttl_min": 60},
        {"group": "g2", "topic": "mood", "query": "how am I", "tool": "none",
         "lead_min": 0, "ttl_min": 30},
        {"group": "g3", "topic": "news", "query": "news today", "tool": "news",
         "lead_min": 10}
    ]});
    let parked = json!([
        {"kind": "recur-routines", "payload": routines.to_string()},
        {"kind": "recur-answer", "payload": answer.to_string()}
    ]);
    let out = run(
        RECUR,
        json!({"context": {"store_origin": "recur", "mem_phase": "recur-merge",
                           "dream_run": "r-1249", "dream_to": NIGHT},
               "hop": {"operation": "select", "rows_affected": 2}}),
        json!([{"origin": "tool", "type": "tool_result", "id": "r-0",
                "text": parked.to_string()}]),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "recurring");
    let lane: Value =
        meclaw_core::serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("payload"))
            .expect("json");
    assert_eq!(lane["audience_set"], json!(["member:m"]));
    let cands = lane["candidates"].as_array().expect("candidates");
    assert_eq!(
        cands.len(),
        1,
        "no tool or no ttl_min is no prewarm: {cands:?}"
    );
    let mut keys: Vec<&str> = cands[0]
        .as_object()
        .expect("a candidate")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "evidence", "lead_min", "query", "rrule", "tool", "topic", "triggers", "ttl_min", "tz"
        ]
    );
    assert_eq!(cands[0]["ttl_min"], json!(60));

    let raw = std::fs::read_to_string(HIVE).expect("hive config");
    let hive: Value = meclaw_core::serde_json::from_str(&raw).expect("json");
    let emits: Vec<&str> = hive["params"]["contract"]["emits"]
        .as_array()
        .expect("emits")
        .iter()
        .filter_map(|l| l["route"].as_str())
        .collect();
    assert!(emits.contains(&"recurring"), "{emits:?}");
    assert!(!raw.contains("recurring_ack"), "nobody answers the lane");
    edge("./recur", ".", "'recurring'");
    edge("./recur", "./dreamer", "'recur-ask'");
    edge("./dreamer", "./recur", "context.store_origin == 'recur'");
}
