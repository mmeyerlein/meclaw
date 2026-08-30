//! GH #500 — the acceptance receipt: an edge condition that compares a numeric
//! hop key with the plain equality form routes the message.
//!
//! # What was broken
//!
//! `evaluate_condition` bound the two header compartments through serde, and
//! `serde_json::Number` serialises every non-negative integer via
//! `serialize_u64`. So `200` reached CEL as a `uint`, and cel 0.13's runtime
//! equality downcasts to its own type and nothing else: `uint(200) == 200` was
//! `false`. No eval error, no advisory — the edge simply never fired, and the
//! emission dead-lettered as `no_route` while the dead letter itself showed
//! `http_status: 200`. Ordering (`Val::compare`) *was* cross-type, which is the
//! asymmetry that made the class expensive to find: `> 100` worked.
//!
//! # What this file pins
//!
//! The unit half lives next to the binding (`cel_eval.rs`, the `gh500_*`
//! tests). This file is the half a unit test cannot reach: a real colony, a
//! real emission, a real routing decision. Three tests:
//!
//!   1. [`a_plain_numeric_equality_routes_the_message`] — the issue's own
//!      reproduction. `hop.http_status == 200` fires, the 404 arm stays silent,
//!      nothing dead-letters.
//!   2. [`the_range_form_still_routes_what_it_used_to`] — the workaround a
//!      shipped template had to use because the equality form did not work. It
//!      is still correct, and must stay correct: the fix must not buy the plain
//!      form at the price of the form the tree already runs.
//!   3. [`no_shipped_expression_spells_a_uint_literal`] — the one behaviour
//!      that changed direction. CEL equality across `uint`/`int` is false in
//!      both directions, so a `u`-suffixed literal no longer matches an
//!      int-bound header. The sweep is what keeps that from being a silent
//!      migration cost: it walks every shipped `condition` and modifier
//!      expression and requires none of them to spell one.
//!
//! The frames carry their numbers the same way GH #283's do: `hop` is the
//! emitting cell's own contract output, so the probe's `context` is lifted into
//! the emission's `content.header` block, which the outputs arm splits into the
//! `hop` compartment of the follow-up message.

use meclaw_colony::{CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Cell, CellOutput, Message, MessageBuilder, OutputSink, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::future::Future;
use std::path::{Path as FsPath, PathBuf};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::mpsc;

const PROBE: &str = "/probe";

/// Lifts the inbound message's `context` verbatim into its emission's
/// `content.header` block, so an edge condition over `hop.<key>` sees exactly
/// what the probe put in — numbers included, as JSON numbers.
struct HopStamperCell;

impl Cell for HopStamperCell {
    #[allow(clippy::manual_async_fn)]
    fn handle(&mut self, msg: Message, sink: &OutputSink) -> impl Future<Output = ()> + Send {
        let sink = sink.clone();
        let header = msg.headers.context.clone();
        async move {
            let mut content = Map::new();
            content.insert(
                "messages".into(),
                json!([{"origin": "assistant", "type": "text", "text": "fetched"}]),
            );
            content.insert("header".into(), Value::Object(header));
            let _ = sink
                .push(CellOutput {
                    target: Path::new("/unrouted"),
                    content: Value::Object(content),
                })
                .await;
        }
    }
}

/// One ingress frame carrying a numeric status the probe lifts into `hop`.
fn frame(status: i64) -> Message {
    let mut context: Map<String, Value> = Map::new();
    context.insert("http_status".into(), json!(status));
    MessageBuilder::new(Path::new(PROBE))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"fetch"}]}),
        ))
        .context(context)
        .ttl(8)
        .build()
}

/// Bounded wait for a delivery (30s failure-marker convention). A miss prints
/// the dead-letter queue, which is exactly where the defect used to put the
/// message — as `no_route`, with the matching status still in the header.
async fn recv_one(h: &ColonyHandle, rx: &mut mpsc::Receiver<Message>, ctx: &str) -> Message {
    match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("{ctx}: capture channel closed"),
        Err(_) => {
            let dlq = h.drain_dead_letters().await;
            panic!("{ctx}: no delivery within 30s; DLQ: {dlq:?}")
        }
    }
}

/// Assert nothing arrives. Short window on purpose: a wrong-arm delivery leaves
/// the same `apply_edges` call as the delivery the test already holds.
async fn assert_silent(rx: &mut mpsc::Receiver<Message>, ctx: &str) {
    let got = tokio::time::timeout(Duration::from_millis(700), rx.recv()).await;
    assert!(
        got.is_err(),
        "{ctx}: this sink must stay silent, got {got:?}"
    );
}

/// A colony whose only file is a hive declaring `edges_json` as its
/// `params.graph`, with every endpoint spawned before the boot.
async fn colony_with(
    edges_json: &str,
    sinks: &[(&str, mpsc::Sender<Message>)],
) -> (TempDir, ColonyHandle) {
    let td = TempDir::new().unwrap();
    let cfg = td.path().join("main");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("config.json"),
        format!(r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":{edges_json}}}}}}}"#),
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());
    h.spawn(Path::new(PROBE), || HopStamperCell).await;
    for (path, tx) in sinks {
        let tx = tx.clone();
        h.spawn(Path::new(path), move || CaptureCell::new(tx.clone()))
            .await;
    }
    bootstrap_from_filesystem(td.path(), &CellFactoryRegistry::new(), &h.runtime())
        .await
        .expect("bootstrap succeeds");
    (td, h)
}

// ── 1. The issue's reproduction ──────────────────────────────────────────────

/// `hop.http_status == 200` on a message carrying exactly 200. Before GH #500
/// this delivered nothing at all and produced a `no_route` dead letter whose
/// own header read `http_status: 200`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_numeric_equality_routes_the_message() {
    let (ok_tx, mut ok_rx) = mpsc::channel(16);
    let (missing_tx, mut missing_rx) = mpsc::channel(16);

    let edges = r#"[
        {"from":"/probe","to":"/ok","condition":"has(hop.http_status) && hop.http_status == 200"},
        {"from":"/probe","to":"/missing","condition":"has(hop.http_status) && hop.http_status == 404"}
    ]"#;
    let (_td, h) = colony_with(edges, &[("/ok", ok_tx), ("/missing", missing_tx)]).await;

    h.send(frame(200)).await;
    let got = recv_one(&h, &mut ok_rx, "the 200 arm").await;
    assert_eq!(
        got.target,
        Path::new("/ok"),
        "GH #500: the plain equality form must route a numeric hop key"
    );
    assert_silent(&mut missing_rx, "the 404 arm does not match a 200").await;

    // The other side of the same comparison: a non-matching number must still
    // NOT route, or the fix would have bought the true case with a false one.
    h.send(frame(404)).await;
    let got = recv_one(&h, &mut missing_rx, "the 404 arm").await;
    assert_eq!(got.target, Path::new("/missing"));
    assert_silent(&mut ok_rx, "the 200 arm does not match a 404").await;

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "GH #500: both frames found their arm — nothing may dead-letter as no_route"
    );
    h.shutdown().await;
}

// ── 2. The workaround the tree already runs ──────────────────────────────────

/// The range form is what a shipped template had to write because the equality
/// form did not work. It is correct on its own terms and must stay correct: the
/// binding change moves both operands to `int`, and ordering has to keep
/// behaving the way it did when one of them was a `uint`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_range_form_still_routes_what_it_used_to() {
    let (ok_tx, mut ok_rx) = mpsc::channel(16);
    let (fail_tx, mut fail_rx) = mpsc::channel(16);

    let edges = r#"[
        {"from":"/probe","to":"/ok","condition":"has(hop.http_status) && hop.http_status >= 200 && hop.http_status < 300"},
        {"from":"/probe","to":"/fail","condition":"has(hop.http_status) && hop.http_status >= 400"}
    ]"#;
    let (_td, h) = colony_with(edges, &[("/ok", ok_tx), ("/fail", fail_tx)]).await;

    h.send(frame(204)).await;
    let got = recv_one(&h, &mut ok_rx, "the 2xx range").await;
    assert_eq!(got.target, Path::new("/ok"));
    assert_silent(&mut fail_rx, "204 is not a 4xx").await;

    h.send(frame(503)).await;
    let got = recv_one(&h, &mut fail_rx, "the 4xx-and-up range").await;
    assert_eq!(got.target, Path::new("/fail"));
    assert_silent(&mut ok_rx, "503 is not a 2xx").await;

    assert!(
        h.drain_dead_letters().await.is_empty(),
        "the range form must keep routing exactly what it routed before"
    );
    h.shutdown().await;
}

// ── 3. The migration cost, swept ─────────────────────────────────────────────

/// `templates/` and `examples/` sit two levels above this crate.
fn repo_path(rel: &str) -> PathBuf {
    FsPath::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn json_files(dir: &FsPath, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let p = entry.path();
        if p.is_dir() {
            json_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p);
        }
    }
}

/// Every CEL source string a shipped `config.json` carries: `condition` plus
/// the two `set_*` compartments of a modifier, which are the only places an
/// expression is written.
fn collect_expressions(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                match (k.as_str(), v) {
                    ("condition", Value::String(s)) => out.push(s.clone()),
                    ("set_context" | "set_hop", Value::Object(exprs)) => {
                        for expr in exprs.values() {
                            if let Value::String(s) = expr {
                                out.push(s.clone());
                            }
                        }
                    }
                    _ => collect_expressions(v, out),
                }
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_expressions(v, out);
            }
        }
        _ => {}
    }
}

/// Blank out every quoted segment, so a model name like `'gpt-4u'` cannot be
/// read as a numeric literal. The replacement keeps the byte length, which
/// keeps every index the scan reports pointing at the original string.
fn without_string_literals(expr: &str) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut quote: Option<char> = None;
    for c in expr.chars() {
        match quote {
            Some(q) if c == q => {
                quote = None;
                out.push(' ');
            }
            Some(_) => out.extend(std::iter::repeat_n(' ', c.len_utf8())),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                out.push(' ');
            }
            None => out.push(c),
        }
    }
    out
}

/// A `u`-suffixed integer literal: a run of digits directly followed by `u`,
/// with the `u` not continuing into an identifier (`200u` yes, `200uu` and
/// `x2u_` no) and the digits not being the tail of one (`sha256u` no). String
/// literals are blanked first — a `u` inside quotes is text, not a type.
fn spells_a_uint_literal(expr: &str) -> Option<String> {
    let scan = without_string_literals(expr);
    let b = scan.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c != b'u' {
            continue;
        }
        // `u` must end the token.
        if b.get(i + 1)
            .is_some_and(|n| n.is_ascii_alphanumeric() || *n == b'_')
        {
            continue;
        }
        // and be preceded by at least one digit ...
        let mut start = i;
        while start > 0 && b[start - 1].is_ascii_digit() {
            start -= 1;
        }
        if start == i {
            continue;
        }
        // ... that does not continue an identifier to its left.
        if start > 0 && (b[start - 1].is_ascii_alphabetic() || b[start - 1] == b'_') {
            continue;
        }
        return Some(scan[start..=i].to_string());
    }
    None
}

/// The one direction the fix changed: CEL equality across `uint` and `int` is
/// false in both directions, so `hop.http_status == 200u` — the spelling that
/// used to be the ONLY one that worked — no longer matches an int-bound header.
///
/// Nothing in the tree spells one today; this sweep is what keeps it that way,
/// so the change cannot become a silent breakage in a template written from an
/// old workaround. `uint(hop.k) == 200u` is the escape hatch for anyone who
/// wants unsigned semantics on purpose, and it casts the header rather than
/// suffixing the literal — which is why the sweep looks for the literal.
///
/// Both trees are swept for what each carries: only a subset of `templates/` is
/// part of the export, and `examples/` travels whole.
#[test]
fn no_shipped_expression_spells_a_uint_literal() {
    let mut files = Vec::new();
    json_files(&repo_path("examples"), &mut files);
    let templates = repo_path("templates");
    if templates.exists() {
        json_files(&templates, &mut files);
    }
    assert!(files.len() > 20, "the sweep found almost nothing to read");

    let mut checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();
    for file in files {
        let shown = file.to_string_lossy().replace('\\', "/");
        let raw = std::fs::read_to_string(&file).expect("read");
        let doc: Value = match meclaw_core::serde_json::from_str(&raw) {
            Ok(v) => v,
            // A non-config json (seed data, fixtures) is not this sweep's business.
            Err(_) => continue,
        };
        let mut expressions = Vec::new();
        collect_expressions(&doc, &mut expressions);
        for expr in expressions {
            checked += 1;
            if let Some(literal) = spells_a_uint_literal(&expr) {
                offenders.push(format!("{shown}: {expr} (uint literal `{literal}`)"));
            }
        }
    }

    assert!(
        checked > 25,
        "only {checked} expressions swept, expected more than 25"
    );
    assert!(
        offenders.is_empty(),
        "GH #500: a shipped expression spells a `u`-suffixed literal, which no longer \
         matches an int-bound header. Drop the suffix, or cast the header with uint():\n  {}",
        offenders.join("\n  ")
    );
}

/// The sweep's own detector, pinned: a sweep that cannot see the thing it looks
/// for is a green light, not a gate.
#[test]
fn the_uint_literal_detector_sees_what_it_must() {
    for hit in [
        "hop.http_status == 200u",
        "hop.n > 0u",
        "hop.big == 18446744073709551615u",
    ] {
        assert!(
            spells_a_uint_literal(hit).is_some(),
            "must be detected: {hit}"
        );
    }
    for miss in [
        "hop.http_status == 200",
        "uint(hop.http_status) == 200",
        "hop.route == 'in_turn'",
        "hop.model.contains('gpt-4u')",
        "int(context.iter) + 1",
        "hop.sha256u == 'x'",
    ] {
        assert_eq!(
            spells_a_uint_literal(miss),
            None,
            "must NOT be detected: {miss}"
        );
    }
}
