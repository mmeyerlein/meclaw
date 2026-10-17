//! R-AG-1 (GH #1229) -- the session keeper's night is a watchdog, not a tick.
//!
//! Until this issue `./night` seeded a cron that struck the close pass every
//! thirty minutes through the UTC night, twelve times a night, whether a
//! single generation was open or not. Now the timer ships without a schedule
//! and `./close` keeps ONE one-shot on it (`night-close`, `rearm`,
//! `catch_up`): the stamp's insert of a new generation arms it for the next
//! opening of the night window (or IS the firing, inside the window), and every
//! firing arms it again for the moment the generation that falls silent next
//! crosses the idle threshold. A keeper with nothing open arms nothing.
//!
//! Everything here runs the shipped `params.script_inline` over real stdin
//! documents. Time is only ever a value -- a clock read before the run fixes
//! the window and the stamps the script is measured against -- never a verdict
//! about how long anything took.

use meclaw_cells::timer::op::TimerOp;
use meclaw_cells::timer::schedule::ScheduleKind;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn keeper(rel: &str) -> Value {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/session-keeper")
        .join(rel);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The one moment the close script reads as "now" (2030-01-01T00:00:00Z): its
/// clock is injected, so every moment below is a value, never a reading of the
/// wall clock (development-rules § 11).
const NOW_MS: i64 = 1_893_456_000_000;

/// Run the shipped close script over `doc` with `params`, the program on
/// stdin (a script near the argv cap must not break the harness, GH #279),
/// with `datetime.now` frozen at `NOW_MS`.
fn close_with(params: Value, mut doc: Value) -> Vec<Value> {
    doc["params"] = params;
    let script = keeper("close/config.json")["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let src = format!(
        concat!(
            "import sys, io, datetime\n",
            "class _Frozen(datetime.datetime):\n",
            "    @classmethod\n",
            "    def now(cls, tz=None):\n",
            "        return cls.fromtimestamp({} / 1000.0, tz)\n",
            "datetime.datetime = _Frozen\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        NOW_MS,
        serde_json::to_string(&script).unwrap(),
        serde_json::to_string(&meclaw_testing::code_stdin(&doc).to_string()).unwrap(),
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
        "close exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The store's answer to the stamp's insert of a new generation, as the hive
/// edge `./sessions -> ./close` hands it on: the stamp's context, a
/// successful `insert`.
fn open_reply() -> Value {
    json!({
        "header": {"context": {"store_origin": "keeper-stamp", "ses_phase": "open",
                               "channel": "tg:42", "keeper_session": "tg:42-x",
                               "keeper_audience": "member:alex"},
                   "hop": {"operation": "insert", "rows_affected": 1, "duration_ms": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "k-open", "text": "{}"}]
    })
}

fn window(from: &str, until: &str) -> Value {
    json!({"night_from_utc": from, "night_until_utc": until})
}

/// The moment of a night order, read the way the timer reads it: through the
/// timer's own parser, as the body it receives (the header rides apart).
fn at_ms(order: &Value) -> i64 {
    let mut body = order.clone();
    body.as_object_mut().expect("order object").remove("header");
    match TimerOp::parse(&body).unwrap_or_else(|e| panic!("the timer refuses {order}: {e}")) {
        TimerOp::Add { row, rearm } => {
            assert!(rearm, "one name, one standing order: {order}");
            assert!(
                row.catch_up,
                "a strike missed while down fires late: {order}"
            );
            match row.kind {
                ScheduleKind::At(t) => t.timestamp_millis(),
                other => panic!("a one-shot, not {other:?}"),
            }
        }
        other => panic!("an add, not {other:?}"),
    }
}

fn utc(ms: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(ms).expect("a representable moment")
}

#[test]
fn gh1229_the_keeper_night_is_armed_by_an_open_generation() {
    // (a) The timer ships no schedule, and no setting describes one.
    let night = keeper("night/config.json");
    assert_eq!(
        night["params"]["schedules"],
        json!([]),
        "./night seeds no schedule: the night is a watchdog ./close arms"
    );
    assert!(
        night["contract"]["settings"].get("night_cron").is_none(),
        "no setting may describe a night cron that no longer exists: {}",
        night["contract"]["settings"]
    );

    // (b) An open generation and an EMPTY window: no night, nothing armed.
    let none = close_with(window("00:00", "00:00"), open_reply());
    assert!(none.is_empty(), "an empty window arms nothing: {none:?}");

    // A window ahead of now: exactly ONE order, for the window's opening.
    let opens = NOW_MS + 2 * 3_600_000;
    let ahead = window(
        &utc(opens).format("%H:%M").to_string(),
        &utc(opens + 3_600_000).format("%H:%M").to_string(),
    );
    let out = close_with(ahead, open_reply());
    assert_eq!(out.len(), 1, "one order, nothing else: {out:?}");
    let order = &out[0];
    assert_eq!(order["header"]["route"], "night", "{order}");
    for key in ["phase", "channel", "session_id", "audience_set"] {
        assert_eq!(
            order["header"][key], "",
            "{key} rides present and empty: {order}"
        );
    }
    assert_eq!(order["op"], "add");
    assert_eq!(order["schedule_name"], "night-close");
    assert_eq!(order["emit_to"], "../close");
    assert_eq!(order["rearm"], true);
    assert_eq!(order["catch_up"], true);
    assert_eq!(
        at_ms(order),
        opens,
        "the strike is the next opening of the window: {order}"
    );

    // A window that is always night: the open IS the firing, and the firing
    // asks for the generation that falls silent next.
    let always = window("00:00", "24:00");
    let firing = close_with(always.clone(), open_reply());
    let phases: Vec<&str> = firing
        .iter()
        .map(|m| m["header"]["phase"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        phases,
        ["sweep", "owed", "heal-page", "next"],
        "an open inside the night is a firing: {firing:?}"
    );
    for m in &firing {
        assert_eq!(m["header"]["route"], "kstore", "{m}");
        assert_eq!(
            m["header"]["channel"], "",
            "the firing is about no single channel: {m}"
        );
    }
    let next: Value =
        serde_json::from_str(firing[3]["messages"][0]["text"].as_str().expect("op text"))
            .expect("op json");
    assert_eq!(next["operation"], "select");
    assert_eq!(next["columns"], json!(["last_seen"]));
    assert!(next["where"]["last_seen"]["gte"].is_string(), "{next}");
    assert_eq!(
        next["order_by"],
        json!([{"col": "last_seen", "dir": "asc"}])
    );
    assert_eq!(next["limit"], 1);

    // (c) The answer to `next`: the strike lands one millisecond past the
    // moment that row crosses the idle threshold.
    let idle_ms = keeper("close/config.json")["params"]["idle_ms"]
        .as_i64()
        .expect("idle_ms");
    let seen = NOW_MS - 30 * 60_000;
    let stamp = utc(seen).format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    let reply = json!({
        "header": {"context": {"store_origin": "keeper-close", "ses_phase": "next"},
                   "hop": {"operation": "select", "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "k-next",
                      "text": json!([{"last_seen": stamp}]).to_string()}]
    });
    let out = close_with(always, reply);
    assert_eq!(out.len(), 1, "one order: {out:?}");
    assert_eq!(out[0]["header"]["route"], "night", "{out:?}");
    assert_eq!(
        at_ms(&out[0]),
        seen + idle_ms + 1,
        "last_seen {stamp} + idle_ms + 1 ms: {}",
        out[0]
    );

    // (d) Both edges stand in the hive: the arm event reaches ./close, and
    // the order reaches the timer.
    let hive = keeper("config.json");
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let arm = edges
        .iter()
        .find(|e| {
            e["from"] == "./sessions"
                && e["to"] == "./close"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("keeper-stamp"))
        })
        .unwrap_or_else(|| panic!("no edge carries the stamp's insert to ./close: {edges:?}"));
    let cond = arm["condition"].as_str().unwrap_or_default();
    for part in [
        "context.ses_phase == 'open'",
        "hop.operation == 'insert'",
        "!has(hop.error_code) || hop.error_code == ''",
    ] {
        assert!(cond.contains(part), "the arm edge tests `{part}`: {cond}");
    }
    assert!(
        edges.iter().any(|e| e["from"] == "./close"
            && e["to"] == "./night"
            && e["condition"] == "has(hop.route) && hop.route == 'night'"),
        "no edge carries the order from ./close to ./night: {edges:?}"
    );
}

/// Re-check K-UHREN A I1: a generation enters the keeper's store a second way.
/// The porter's import writes `sessions` rows exactly as they stand, open
/// generations included, and the receiving keeper CONTINUES such a
/// conversation (`examples/memory-import/README.md`) -- no `open` insert of the
/// stamp follows. Without its own arm event an imported keeper armed no night,
/// and a one-channel member never ended that session. The store's answer to
/// the porter's insert of a `sessions` row reaches ./close and arms exactly as
/// an open does: outside the night the next opening, inside it a firing.
#[test]
fn gh1229_an_imported_generation_arms_the_keeper_night() {
    let hive = keeper("config.json");
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let arm = edges
        .iter()
        .find(|e| {
            e["from"] == "./sessions"
                && e["to"] == "./close"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("kporter"))
        })
        .unwrap_or_else(|| {
            panic!("no edge carries the porter's import of a generation to ./close: {edges:?}")
        });
    let cond = arm["condition"].as_str().unwrap_or_default();
    for part in [
        "context.port_phase == 'import-applied'",
        "context.port_table == 'sessions'",
        "hop.operation == 'insert'",
        "!has(hop.error_code) || hop.error_code == ''",
    ] {
        assert!(
            cond.contains(part),
            "the import arm edge tests `{part}`: {cond}"
        );
    }

    // The context the edge hands on: the porter's own keys, then the edge's
    // modifier -- each `set_context` value is a CEL string literal, each
    // `delete_context` entry a key.
    let mut context = json!({"store_origin": "kporter", "port_phase": "import-applied",
                             "port_run": "import:1", "port_table": "sessions"});
    let ctx = context.as_object_mut().expect("context object");
    for (key, cel) in arm["modifier"]["set_context"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        let literal = cel
            .as_str()
            .and_then(|s| s.strip_prefix('\''))
            .and_then(|s| s.strip_suffix('\''))
            .unwrap_or_else(|| panic!("set_context.{key} is a string literal, not {cel}"));
        ctx.insert(key, json!(literal));
    }
    for key in arm["modifier"]["delete_context"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        ctx.remove(key.as_str().expect("a key"));
    }
    let reply = json!({
        "header": {"context": context,
                   "hop": {"operation": "insert", "rows_affected": 1, "duration_ms": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "p-import-applied",
                      "text": "{}"}]
    });

    // A window ahead of now: exactly ONE order, for the window's opening.
    let opens = NOW_MS + 2 * 3_600_000;
    let ahead = window(
        &utc(opens).format("%H:%M").to_string(),
        &utc(opens + 3_600_000).format("%H:%M").to_string(),
    );
    let out = close_with(ahead, reply.clone());
    assert_eq!(
        out.len(),
        1,
        "an imported generation arms one order: {out:?}"
    );
    assert_eq!(out[0]["header"]["route"], "night", "{out:?}");
    assert_eq!(
        at_ms(&out[0]),
        opens,
        "the strike is the next opening of the window: {}",
        out[0]
    );

    // Inside the night the import IS a firing, as an open is.
    let firing = close_with(window("00:00", "24:00"), reply);
    let phases: Vec<&str> = firing
        .iter()
        .map(|m| m["header"]["phase"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        phases,
        ["sweep", "owed", "heal-page", "next"],
        "an import inside the night is a firing: {firing:?}"
    );
}
