//! The presenter's colony locks (GH #959): the display colony of `display_colony.rs` with
//! the SHIPPED presenter under `/alex/apps/presenter`, a held decider and a test that plays
//! the app.
//!
//! ```text
//! /alex/apps/presenter     the shipped presenter (stage, decide, store, clock)
//!                            decide -> a held loopback decider (`start_mock_server_held`)
//! /alex/channels/display   the shipped display
//! ```
//!
//! **Why the test plays the app.** A lock has to show the FIRST reaction -- the window with
//! its working hint reaching `web` -- while the app still holds its data. A `code` cell
//! answers at once, so the app's half is the test itself: its `in_show` leaves `/alex` and
//! arrives on the egress channel, and the test answers with `show_topics`/`show_data`
//! after it has observed what it waits for. The same holds for the decider: every request
//! is held until the test releases it with the verdict it chose (lesson: a stub holds
//! until an observed event, never a fixed window).
//!
//! **Absence** ("no view", "no request") is proven by a sentinel: `stage` is resident with
//! one child, so a later event of the same order that DID produce its effect proves the
//! earlier one produced none.
//!
//! The wiring the builder's `shows` and `screen` kinds would draw is drawn by hand here:
//! `view`/`withdraw` leave the app level renamed `in_view`/`in_withdraw` towards the
//! screen, `in_show` leaves `/alex` to the test.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{HeldRequest, MockResponse, start_mock_server_held};
use tokio::sync::mpsc;

use crate::display_colony::{
    Boot, Colony, MARKER, PROBE, boot_with, calls_of, copy_tree, have_python, hop_of, patch_json,
    repo,
};

/// Where the presenter stands, and the app the test plays.
pub const PRESENTER: &str = "/alex/apps/presenter";
pub const SAMPLE_APP: &str = "/alex/apps/sample";

/// Every template this module reads besides the display's own (R2b, GH #9).
const NEEDED: [&str; 3] = ["templates/presenter", "templates/display", "templates/web"];

pub fn library_ships() -> bool {
    NEEDED
        .iter()
        .all(|rel| repo(rel).join("template.json").is_file())
}

/// The dials a presenter lock turns.
pub struct Dials {
    pub budget_ms: u64,
    pub data_wait_ms: u64,
    pub screen_audience: Value,
}

impl Default for Dials {
    fn default() -> Self {
        Dials {
            budget_ms: 20_000,
            data_wait_ms: 20_000,
            screen_audience: json!([]),
        }
    }
}

/// A booted colony with a presenter, its held decider and the egress the test reads.
pub struct Stage {
    pub c: Colony,
    pub decider: mpsc::UnboundedReceiver<HeldRequest>,
    _decider: tokio::task::JoinHandle<()>,
    held: Vec<Message>,
}

pub async fn boot(d: Dials) -> Stage {
    let (addr, join, decider) = start_mock_server_held()
        .await
        .expect("the held decider binds on 127.0.0.1");
    let c = boot_with(Boot::default(), move |root| {
        let at = root.join("main/alex/apps/presenter");
        copy_tree(&repo("templates/presenter"), &at);
        patch_json(&at.join("decide/config.json"), |v| {
            v["params"]["model"] = json!("mock-decider");
            v["params"]["api_key"] = json!("fake-key");
            v["params"]["base_url"] = json!(format!("http://{addr}"));
            // The test HOLDS the decider until it has seen what it waits for; the
            // shipped 3 s backstop would fire first on a cold lane (a red run: the
            // held `t1` came back `error_code timeout` after 3002 ms). The verdict's
            // deadline is the stage's `budget_ms`, which every lock dials itself.
            v["params"]["external_timeout_ms"] = json!(25_000);
            v["cell"]["message_timeout"] = json!(40_000);
        });
        patch_json(&at.join("stage/config.json"), |v| {
            v["params"]["budget_ms"] = json!(d.budget_ms);
            v["params"]["data_wait_ms"] = json!(d.data_wait_ms);
            v["params"]["screen_audience"] = d.screen_audience.clone();
        });
        // The app level: the screen lanes renamed on the way out (what `screen` draws),
        // `in_show` out of the level (what `shows` would carry to the app).
        patch_json(&root.join("main/alex/apps/config.json"), |v| {
            let edges = v["params"]["graph"]["edges"].as_array_mut().expect("edges");
            edges.push(json!({"from": "./presenter", "to": ".",
                              "condition": "has(hop.route) && hop.route == 'view'",
                              "modifier": {"set_hop": {"route": "'in_view'"}}}));
            edges.push(json!({"from": "./presenter", "to": ".",
                              "condition": "has(hop.route) && hop.route == 'withdraw'",
                              "modifier": {"set_hop": {"route": "'in_withdraw'"}}}));
            edges.push(json!({"from": "./presenter", "to": ".",
                              "condition": "has(hop.route) && (hop.route == 'in_show' || hop.route == 'error')"}));
        });
        patch_json(&root.join("main/alex/config.json"), |v| {
            let edges = v["params"]["graph"]["edges"].as_array_mut().expect("edges");
            edges.push(json!({"from": "./apps", "to": ".",
                              "condition": "has(hop.route) && (hop.route == 'in_show' || hop.route == 'error')"}));
        });
    })
    .await;
    Stage {
        c,
        decider,
        _decider: join,
        held: Vec::new(),
    }
}

/// The verdict the decider answers, on the decider's own wire.
///
/// OR-DP.P.3 / OR-DP-57: the shape is the hosted service's answer as strand E's
/// `decisions` translate reads it back (`answers.<key>` = `{type, choice,
/// probabilities, confidence}`); it is spelled once, here. The translate wants
/// EVERY asked key answered (`decision_incomplete` otherwise), so a key the
/// test does not name gets the quiet default of the one topic the locks use:
/// `sample.lead` = the standard `brief`, `sample.also` = `none`.
pub fn decision(answers: &[(&str, &str, f64)]) -> MockResponse {
    let mut a = Map::new();
    let defaults = [("sample.lead", "brief", 0.5), ("sample.also", "none", 0.5)];
    let named = answers.iter().map(|(k, _, _)| *k).collect::<Vec<_>>();
    let filled = defaults.iter().filter(|(k, _, _)| !named.contains(k));
    for (key, choice, p) in answers.iter().chain(filled) {
        a.insert(
            (*key).to_string(),
            json!({"type": "choice", "choice": choice,
                   "probabilities": {(*choice).to_string(): p}, "confidence": p}),
        );
    }
    MockResponse::ok_json(
        json!({"answers": a, "model": "mock-decider",
               "usage": {"input_tokens": 1, "output_tokens": 1, "cost": 0}})
        .to_string()
        .as_bytes(),
    )
}

/// The generic topic `sample`: standard `brief` (a card), `rows` (a list), `steps`
/// (steps) and the `on_choice` set `slow`.
pub fn sample_topic() -> Value {
    json!({
        "topic": "sample", "title": "Sample", "describe": "a generic sample topic for the locks",
        "glyph": "S", "standard": "brief",
        "candidates": [
            {"key": "brief", "block": "display-card", "describe": "a short summary",
             "set": "brief", "bind": {"title": "brief.title"}},
            {"key": "rows", "block": "display-list", "describe": "the items as a list",
             "set": "rows", "bind": {"title": "=Items"},
             "children": [{"each": "rows", "block": "display-item",
                           "bind": {"k": "$.name"}}]},
            {"key": "steps", "block": "display-steps", "describe": "the path as steps",
             "set": "steps",
             "children": [{"each": "steps", "block": "display-step",
                           "bind": {"label": "$.label"}}]},
            {"key": "slow", "block": "display-value", "describe": "a number costly to fetch",
             "set": "slow", "on_choice": true, "bind": {"value": "slow.n"}}
        ]
    })
}

fn to_presenter(route: &str, extra_hop: Value, body: Value) -> Message {
    let mut hop = Map::new();
    hop.insert("route".into(), json!(route));
    if let Some(m) = extra_hop.as_object() {
        hop.extend(m.clone());
    }
    MessageBuilder::new(Path::new(PRESENTER))
        .reply_to(Path::new(SAMPLE_APP))
        .hop(hop)
        .body(Body::Inline(body))
        .ttl(24)
        .build()
}

impl Stage {
    /// One turn with text, as the member's observer edge hands it to an app.
    pub async fn turn(&self, turn_id: &str, text: &str) {
        self.c
            .h
            .send(to_presenter(
                "turn",
                json!({"turn_id": turn_id}),
                json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
            ))
            .await;
    }

    /// The app says its topics.
    pub async fn topics(&self, topics: Value) {
        self.c
            .h
            .send(to_presenter(
                "show_topics",
                json!({"show_app": SAMPLE_APP}),
                json!({"messages": [], "topics": topics}),
            ))
            .await;
    }

    /// The app answers a data request (one part).
    pub async fn data(&self, turn_id: &str, sets: Value) {
        self.data_from(SAMPLE_APP, turn_id, sets).await;
    }

    /// A data answer as the edge of app `app` stamps it (`hop.show_app`, OR-DP-54).
    pub async fn data_from(&self, app: &str, turn_id: &str, sets: Value) {
        self.c
            .h
            .send(to_presenter(
                "show_data",
                json!({"show_app": app}),
                json!({"messages": [], "topic": "sample", "turn_id": turn_id, "sets": sets}),
            ))
            .await;
    }

    /// The next message that left `/alex` whose hop route is `route` (others are kept for
    /// a later call), or a panic after the failure-marker window.
    pub async fn out(&mut self, route: &str) -> Message {
        if let Some(i) = self
            .held
            .iter()
            .position(|m| m.headers.hop.get("route").and_then(Value::as_str) == Some(route))
        {
            return self.held.remove(i);
        }
        let deadline = Instant::now() + MARKER;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let Ok(msg) = tokio::time::timeout(left, self.c.egress.recv()).await else {
                panic!("no `{route}` left /alex within 30s\n{}", self.trail().await);
            };
            let msg = msg.expect("egress open");
            if msg.headers.hop.get("route").and_then(Value::as_str) == Some(route) {
                return msg;
            }
            self.held.push(msg);
        }
    }

    /// Wait until `stage` has sent the decide call of `turn_id` to `./decide` -- read at
    /// the receiver. `stage` runs one message at a time, so every event before that turn
    /// was handled too: the sentinel of a lock. (The decider itself is serial: a held
    /// request keeps the next one from reaching the stub, so the stub cannot be it.)
    pub async fn decided_on(&self, turn_id: &str) {
        let to = format!("{PRESENTER}/decide");
        self.c
            .wait_until("the decide call of the sentinel turn", || async {
                self.c
                    .log(Some(&to))
                    .await
                    .iter()
                    .any(|r| r.to_path == to && hop_of(r)["show_id"].as_str() == Some(turn_id))
            })
            .await;
    }

    /// Every key an emission of `decide` or `store` carried that its `contract.emits` does
    /// not declare (BUILD-PREAMBLE lesson: `contract.emits` is checked where it is
    /// emitted, review Minor 3). Read at the receiver; the keys the edge back to `stage`
    /// stamps itself (`route`, `show_id`) are the edge's, not the cell's. `clock` is a
    /// `timer`: its emission is the type's, and its config declares none.
    pub async fn undeclared_emissions(&self) -> Vec<String> {
        let mut out = Vec::new();
        for cell in ["decide", "store"] {
            let cfg = crate::display_colony::read_json(
                &repo("templates/presenter").join(cell).join("config.json"),
            );
            let emits = &cfg["contract"]["emits"];
            let from = format!("{PRESENTER}/{cell}");
            let mut seen = 0;
            for row in self
                .c
                .log(None)
                .await
                .iter()
                .filter(|r| r.from_path == from)
            {
                seen += 1;
                let body = crate::display_colony::body_of(row);
                for k in body.as_object().into_iter().flatten().map(|(k, _)| k) {
                    if k != "header" && emits["body"].get(k).is_none() {
                        out.push(format!("{cell} body `{k}`"));
                    }
                }
                for k in hop_of(row)
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, _)| k)
                {
                    if k != "route" && k != "show_id" && emits["hop"].get(k).is_none() {
                        out.push(format!("{cell} hop `{k}`"));
                    }
                }
            }
            if seen == 0 {
                out.push(format!("{cell} emitted nothing"));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// What the presenter's hops did, and the dead letters, so a red run names its cause.
    pub async fn trail(&self) -> String {
        let rows: Vec<String> = self
            .c
            .log(None)
            .await
            .iter()
            .filter(|r| r.from_path.contains("presenter") || r.to_path.contains("presenter"))
            .map(|r| {
                let body = r.body_payload.as_deref().unwrap_or("");
                format!(
                    "{} -> {} {} {}",
                    r.from_path,
                    r.to_path,
                    r.headers_json,
                    body.chars().take(400).collect::<String>()
                )
            })
            .collect();
        let dlq = self.c.h.drain_dead_letters().await;
        format!("{}\ndead letters: {dlq:?}", rows.join("\n"))
    }

    /// Every `in_show` the test has seen so far and not taken, plus what is waiting now.
    pub fn held_routes(&mut self) -> Vec<String> {
        while let Ok(m) = self.c.egress.try_recv() {
            self.held.push(m);
        }
        self.held
            .iter()
            .filter_map(|m| m.headers.hop.get("route").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }

    /// The next request the decider holds.
    pub async fn ask(&mut self) -> HeldRequest {
        tokio::time::timeout(MARKER, self.decider.recv())
            .await
            .expect("the decider is asked within 30s")
            .expect("the decider stands")
    }

    /// The journal rows `stage` wrote, oldest first (the store bundles at `store`).
    pub async fn journal(&self) -> Vec<Value> {
        let mut out = Vec::new();
        for row in self.c.log(Some(&format!("{PRESENTER}/store"))).await {
            for call in calls_of(&row).unwrap_or_default() {
                if call["operation"] == "insert" && call["table"] == "journal" {
                    out.push(call["row"].clone());
                }
            }
        }
        out
    }

    /// The journal row of one turn, once it stands (the last one written for it).
    pub async fn journal_of(&self, turn_id: &str) -> Value {
        let deadline = Instant::now() + MARKER;
        loop {
            if let Some(row) = self
                .journal()
                .await
                .into_iter()
                .rev()
                .find(|r| r["turn_id"] == turn_id)
            {
                return row;
            }
            assert!(
                Instant::now() < deadline,
                "no journal row for {turn_id} within 30s"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The pending record of one turn once `stage` has written it as done (the store
    /// bundle at `store`), or a panic after the failure-marker window.
    pub async fn pending_done(&self, turn_id: &str) -> Value {
        let deadline = Instant::now() + MARKER;
        loop {
            for row in self
                .c
                .log(Some(&format!("{PRESENTER}/store")))
                .await
                .iter()
                .rev()
            {
                for call in calls_of(row).unwrap_or_default() {
                    let v = &call["row"]["value"];
                    if call["operation"] == "insert"
                        && call["table"] == "pending"
                        && v["turn_id"] == turn_id
                        && v["state"] == "done"
                    {
                        return v.clone();
                    }
                }
            }
            assert!(
                Instant::now() < deadline,
                "no done pending row for {turn_id} within 30s"
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Whether the patch log at `web` ever created an object whose id carries `needle`.
    pub async fn drawn(&self, needle: &str) -> bool {
        let tree = self.c.tree().await;
        tree.as_object()
            .map(|m| m.keys().any(|k| k.contains(needle)))
            .unwrap_or(false)
    }

    /// Wait until an object whose id carries `needle` stands at `web`.
    pub async fn wait_drawn(&self, needle: &str) {
        let n = needle.to_string();
        self.c
            .wait_tree(&format!("an object `{needle}` at web"), move |t| {
                t.as_object()
                    .map(|m| m.keys().any(|k| k.contains(&n)))
                    .unwrap_or(false)
            })
            .await;
    }

    /// The ids at `web` carrying `needle`, in patch order of creation.
    pub async fn ids(&self, needle: &str) -> Vec<String> {
        let tree = self.c.tree().await;
        tree.as_object()
            .map(|m| m.keys().filter(|k| k.contains(needle)).cloned().collect())
            .unwrap_or_default()
    }

    /// The messages `stage` emitted, each as its list of `(route, body)` -- read at the
    /// receivers, in log order, grouped by the parent message that caused them.
    pub async fn outputs_of_stage(&self) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        for row in self.c.log(None).await {
            if row.from_path == format!("{PRESENTER}/stage") {
                let hop = hop_of(&row);
                out.push((
                    hop["route"].as_str().unwrap_or("").to_string(),
                    crate::display_colony::body_of(&row),
                ));
            }
        }
        out
    }

    /// Install the app's topic and wait until `stage` has written it (the `shows` row).
    pub async fn install_sample(&self) {
        self.topics(json!([sample_topic()])).await;
        let deadline = Instant::now() + MARKER;
        loop {
            let rows = self.c.log(Some(&format!("{PRESENTER}/store"))).await;
            let written = rows.iter().filter_map(calls_of).flatten().any(|c| {
                c["operation"] == "insert" && c["table"] == "shows" && c["row"]["topic"] == "sample"
            });
            if written {
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "the topic `sample` was not accepted\n{}",
                    self.trail().await
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// The inline body of a message that left `/alex`.
pub fn body(m: &Message) -> Value {
    match &m.body {
        Body::Inline(v) => v.clone(),
        _ => Value::Null,
    }
}

/// Whether a lock can run here; prints its SKIP line when not.
pub fn guard(name: &str) -> bool {
    if !library_ships() || !have_python() {
        println!("SKIP {name}: the templates or python3 are not here");
        return false;
    }
    true
}

/// A view of the stand-in app, drawn before the turn: compose boots on its first event
/// (reads its rows and the tree), and a lock about the FIRST reaction must not measure
/// that boot (plan P section 5).
pub async fn warm(s: &Stage) {
    s.c.write_view(PROBE, "warm", json!({"pane_id": "warm", "title": "warm"}))
        .await;
    s.wait_drawn("warm").await;
}

/// The routes `stage` emitted so far, in log order.
pub async fn stage_routes(s: &Stage) -> Vec<String> {
    s.outputs_of_stage()
        .await
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}
