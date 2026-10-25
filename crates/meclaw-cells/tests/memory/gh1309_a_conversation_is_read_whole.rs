//! GH #1309 (empathy plan C4, agent-2 A4) -- a conversation is read whole.
//!
//! Two doors of the memory hive share one read in `./sessions`, without a
//! model: `in_query` with `memory_tier: "session"` hands the asking round the
//! episodes of ONE session in order, each with its role and `episode_ref`, as
//! a `bundle`; `in_conversations` hands a READER the same form for a session or
//! for a channel and/or a time, on `conversations`, and only for sessions on
//! conversation channels -- never `role`, `ask` or `inner`. Both are
//! fail-closed on the round and both say `truncated` when `max_chars` cut.

// harness_fold: #[path = "support/memory_hive_subject.rs"]
#[allow(unused_imports)]
use crate::support; // harness_fold: mod support;

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::*;

fn ep(
    id: &str,
    session: &str,
    sender: &str,
    channel: &str,
    audience: &str,
    text: &str,
    at: u32,
) -> Value {
    json!({"id": id, "session_id": session, "turn_id": format!("t-{id}"), "sender": sender,
           "speaker": if sender == "assistant" { "agent:a" } else { "member:e" },
           "channel": channel, "audience_set": audience, "content": text,
           "happened_at": format!("2026-10-{at:02}T10:00:00Z"),
           "recorded_at": format!("2026-10-{at:02}T10:00:01Z")})
}

fn seeded() -> Vec<Value> {
    let mut forgotten = ep("e-forgot", "s1", "user", "chat", ROUND_EA, "forget this", 5);
    forgotten["closure_source"] = json!("forget_request:t-x");
    vec![
        // Seeded out of order on purpose: the read orders, the seed does not.
        ep(
            "e-2",
            "s1",
            "assistant",
            "chat",
            ROUND_EA,
            "Hello Erika.",
            2,
        ),
        ep("e-1", "s1", "user", "chat", ROUND_EA, "Hi.", 1),
        ep(
            "e-3",
            "s1",
            "user",
            "chat",
            r#"["agent:a","member:e","person:x"]"#,
            "Ann is here.",
            3,
        ),
        ep("e-4", "s1", "assistant", "chat", ROUND_EB, "Only for b.", 4),
        forgotten,
        ep("e-null", "s1", "user", "chat", "", "no round", 6),
        ep("r-1", "s-role", "user", "role", ROUND_EA, "role run", 7),
        ep(
            "a-1",
            "s-ask",
            "user",
            "ask:affinity",
            ROUND_EA,
            "an ask",
            8,
        ),
        ep(
            "i-1",
            "s-inner",
            "assistant",
            "inner:care",
            ROUND_EA,
            "inner turn",
            9,
        ),
        ep(
            "p-1",
            "s-peer",
            "peer",
            "peer:north",
            ROUND_EA,
            "a peer thread",
            10,
        ),
    ]
}

async fn hive() -> (
    tempfile::TempDir,
    meclaw_testing::ColonyHandle,
    tokio::sync::mpsc::Receiver<Message>,
) {
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &[]);
    let store = td.path().join("main/memory-hive/store");
    let schema = read_json(&store.join("config.json"))["params"]["schema"]["episodes"].clone();
    let body: String = std::iter::once(json!({"schema": schema}))
        .chain(seeded())
        .map(|r| meclaw_core::serde_json::to_string(&r).expect("serialise") + "\n")
        .collect();
    std::fs::write(store.join("seed/episodes.jsonl"), body).expect("seed episodes");
    let (h, rx) = boot(&td).await;
    (td, h, rx)
}

fn session_query(session: &str, round: Option<&str>, max_chars: Option<u64>) -> Message {
    let mut ctx = json!({"recall_caller": "objects", "memory_tier": "session", "recall_query": "",
                         "recall_as_of": "", "recall_window_from": "", "recall_window_to": "",
                         "channel": "chat", "session_id": session});
    if let Some(r) = round {
        ctx["audience_now"] = json!(r);
    }
    let mut hop = json!({"route": "in_query"});
    if let Some(n) = max_chars {
        hop["max_chars"] = json!(n);
    }
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&hop))
        .context(as_map(&ctx))
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

fn conversations(body: Value) -> Message {
    let mut b = as_map(&body);
    b.entry("messages").or_insert(json!([]));
    MessageBuilder::new(Path::new(HIVE))
        .hop(as_map(&json!({"route": "in_conversations"})))
        .context(Map::new())
        .body(Body::Inline(Value::Object(b)))
        .build()
}

async fn bundle(rx: &mut tokio::sync::mpsc::Receiver<Message>, seen: &mut Vec<Message>) -> Value {
    let m = next_on(rx, seen, "bundle", |m| {
        hop_str(m, "recall_caller") == "objects"
    })
    .await;
    for key in ["conv_req", "mem_phase", "store_origin"] {
        assert!(!m.headers.context.contains_key(key), "{key} left the hive");
    }
    bundle_json(&m).expect("a bundle")
}

async fn answer(
    rx: &mut tokio::sync::mpsc::Receiver<Message>,
    seen: &mut Vec<Message>,
    reader: &str,
) -> Value {
    let r = reader.to_string();
    let m = next_on(rx, seen, "conversations", move |m| {
        hop_str(m, "reader") == r
    })
    .await;
    assert!(
        !m.headers.context.contains_key("conv_req"),
        "conv_req left the hive"
    );
    let text = body_of(&m)["messages"][0]["text"]
        .as_str()
        .expect("answer text")
        .to_string();
    meclaw_core::serde_json::from_str(&text).expect("answer json")
}

fn refs(eps: &Value) -> Vec<String> {
    eps.as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|e| e["episode_ref"].as_str().map(str::to_string))
        .collect()
}

fn sessions(a: &Value) -> Vec<String> {
    a["sessions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s["session_id"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1309_query_by_session_returns_its_episodes_in_order() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    h.send(session_query("s1", Some(ROUND_EA), None)).await;
    let b = bundle(&mut rx, &mut seen).await;
    assert_eq!(b["tier"], "session");
    assert_eq!(refs(&b["episodes"]), vec!["e-1", "e-2", "e-3"], "{b}");
    assert_eq!(b["episodes"][0]["role"], "person");
    assert_eq!(b["episodes"][1]["role"], "assistant");
    assert_eq!(b["episodes"][1]["text"], "Hello Erika.");
    assert_eq!(b["truncated"], json!(false));
    assert!(
        b.get("beliefs").is_none() && b.get("preview").is_none(),
        "{b}"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1309_query_by_session_respects_the_round() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    h.send(session_query("s1", Some(ROUND_EB), None)).await;
    let b = bundle(&mut rx, &mut seen).await;
    assert_eq!(refs(&b["episodes"]), vec!["e-4"], "round b: {b}");
    h.send(session_query("s1", None, None)).await;
    let r = next_on(&mut rx, &mut seen, "reject", |m| {
        hop_str(m, "recall_caller") == "objects"
    })
    .await;
    assert_eq!(hop_str(&r, "reject_reason"), "missing_audience");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1309_query_by_session_says_truncated() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    // "Hi." (3) + "Hello Erika." (12) = 15 > 10: the second episode is cut off.
    h.send(session_query("s1", Some(ROUND_EA), Some(10))).await;
    let b = bundle(&mut rx, &mut seen).await;
    assert_eq!(refs(&b["episodes"]), vec!["e-1"], "{b}");
    assert_eq!(b["truncated"], json!(true));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_conversations_returns_a_session_in_order_with_refs() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    h.send(conversations(
        json!({"reader": "counselor", "audience_set": ROUND_EA, "session_id": "s1"}),
    ))
    .await;
    let a = answer(&mut rx, &mut seen, "counselor").await;
    assert_eq!(sessions(&a), vec!["s1"], "{a}");
    assert_eq!(
        refs(&a["sessions"][0]["episodes"]),
        vec!["e-1", "e-2", "e-3"]
    );
    assert_eq!(a["sessions"][0]["episodes"][0]["role"], "person");
    assert_eq!(
        a["withheld"],
        json!(1),
        "the forgotten turn is counted, never shown"
    );
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_conversations_respects_the_readers_round() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    h.send(conversations(
        json!({"reader": "rb", "audience_set": ROUND_EB, "session_id": "s1"}),
    ))
    .await;
    let a = answer(&mut rx, &mut seen, "rb").await;
    assert_eq!(refs(&a["sessions"][0]["episodes"]), vec!["e-4"], "{a}");
    for bad in [json!(null), json!("[]"), json!("not json")] {
        h.send(conversations(
            json!({"reader": "rx", "audience_set": bad, "session_id": "s1"}),
        ))
        .await;
        let a = answer(&mut rx, &mut seen, "rx").await;
        assert_eq!(a["error_code"], "missing_audience", "{a}");
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_conversations_never_returns_role_ask_or_inner_sessions() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    h.send(conversations(
        json!({"reader": "all", "audience_set": ROUND_EA,
                                "since": "2026-10-01T00:00:00Z"}),
    ))
    .await;
    let a = answer(&mut rx, &mut seen, "all").await;
    assert_eq!(sessions(&a), vec!["s1", "s-peer"], "{a}");
    for s in ["s-role", "s-ask", "s-inner"] {
        h.send(conversations(
            json!({"reader": s, "audience_set": ROUND_EA, "session_id": s}),
        ))
        .await;
        let a = answer(&mut rx, &mut seen, s).await;
        assert_eq!(a["sessions"], json!([]), "{s}: {a}");
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sa2_conversations_says_truncated() {
    if !shipped() {
        return;
    }
    let (_td, h, mut rx) = hive().await;
    let mut seen = Vec::new();
    h.send(conversations(
        json!({"reader": "cut", "audience_set": ROUND_EA,
                                "since": "2026-10-01T00:00:00Z", "max_chars": 16}),
    ))
    .await;
    let a = answer(&mut rx, &mut seen, "cut").await;
    assert_eq!(sessions(&a), vec!["s1"], "{a}");
    assert_eq!(refs(&a["sessions"][0]["episodes"]), vec!["e-1", "e-2"]);
    assert_eq!(a["truncated"], json!(true));
    h.shutdown().await;
}
