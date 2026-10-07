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
    /// GH #963: the presenter's own declared-source topics (`params.builtin_topics`);
    /// `null` leaves the shipped value. The default is `[]`: the GH #959/#963 locks test
    /// the verdict with one app topic and a decider that answers only its questions; the
    /// shipped built-in topics (GH #965) add questions of their own, and an unanswered one
    /// turned the verdict into `decision_incomplete` (receipt test-20261003T081405Z, seven
    /// locks red; since GH #977 it is a partial verdict naming the key in `missing`). The locks of the shipped topics (`gh965_the_member_residents_show_on_
    /// the_screen`) dial `null` and answer every question.
    pub builtin_topics: Value,
    /// GH #963: which of the presenter's observed topics are on (`params.observed_topics`);
    /// `null` leaves the shipped `["search", "work"]`, `[]` is a presenter with no topic of
    /// its own.
    pub observed_topics: Value,
    /// The stage's `window_requires_star_data`; `null` leaves the shipped value (off,
    /// R-HP-18: a window shows all of the member's data, also on a screen a third party
    /// shares), `true` is the R-HP-9 (c) way back (a window only on `*` data).
    pub window_requires_star_data: Value,
}

impl Default for Dials {
    fn default() -> Self {
        Dials {
            budget_ms: 20_000,
            data_wait_ms: 20_000,
            screen_audience: json!([]),
            builtin_topics: json!([]),
            observed_topics: Value::Null,
            window_requires_star_data: Value::Null,
        }
    }
}

/// A booted colony with a presenter, its held decider and the egress the test reads.
pub struct Stage {
    pub c: Colony,
    pub decider: mpsc::UnboundedReceiver<HeldRequest>,
    _decider: tokio::task::JoinHandle<()>,
    held: Vec<Message>,
    /// The screen's round as dialled: `turn` stamps it on the turn, as the member's
    /// observer edge stamps the member round (GH #1027 -- a turn without a round that
    /// covers the screen opens nothing).
    screen: Value,
}

pub async fn boot(d: Dials) -> Stage {
    let screen = d.screen_audience.clone();
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
            if !d.builtin_topics.is_null() {
                v["params"]["builtin_topics"] = d.builtin_topics.clone();
            }
            if !d.observed_topics.is_null() {
                v["params"]["observed_topics"] = d.observed_topics.clone();
            }
            if !d.window_requires_star_data.is_null() {
                v["params"]["window_requires_star_data"] = d.window_requires_star_data.clone();
            }
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
                              "condition": "has(hop.route) && (hop.route == 'in_show' || hop.route == 'error' || hop.route == 'resident_read')"}));
        });
        // GH #963: the observed CALL reaches `stage` the way `install_app` draws it -- an
        // edge on lane `tool`, restamped `in_tool_call`. A message sent straight onto
        // `stage` under that route is no lane the presenter's contract declares at
        // `./stage` and dies at the hive boundary (measured 03.10., `HiveBoundary`), so a
        // relay cell stands in for the surface that calls.
        let relay = root.join("main/alex/caller/config.json");
        std::fs::create_dir_all(relay.parent().expect("a parent")).expect("mkdir");
        std::fs::write(
            &relay,
            json!({
                "cell": {"type": "code"},
                "params": {"runner": "python3", "script_inline": CALLER,
                           "external_timeout_ms": 10000},
                "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                             "emits": {"body": {"messages": {"type": "array", "required": false}}},
                             "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                             "capabilities": ["shell:exec"]},
                "description": {"purpose": "Test relay: a surface's tool call.",
                                "use_when": "Test fixture only.", "not_in_scope": "Not a template."}
            })
            .to_string(),
        )
        .expect("write the relay");
        patch_json(&root.join("main/alex/config.json"), |v| {
            let edges = v["params"]["graph"]["edges"].as_array_mut().expect("edges");
            edges.push(json!({"from": "./caller", "to": "./apps/presenter/stage", "lane": "tool",
                              "condition": "has(hop.route) && hop.route == 'tool'",
                              "modifier": {"set_hop": {"route": "'in_tool_call'"}}}));
            edges.push(json!({"from": "./apps", "to": ".",
                              "condition": "has(hop.route) && (hop.route == 'in_show' || hop.route == 'error' || hop.route == 'resident_read')"}));
        });
    })
    .await;
    Stage {
        c,
        decider,
        _decider: join,
        held: Vec::new(),
        screen,
    }
}

/// The relay of an observed call: what the test hands it leaves as a surface's `tool`.
const CALLER: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc["envelope"].get("header") or {}).get("hop") or {})
body = doc.get("body") or {}
sys.stdout.write(json.dumps({
    "header": {"route": "tool", "tool_name": str(hop.get("tool_name") or ""),
               "tool_call_id": str(hop.get("tool_call_id") or "")},
    "messages": body.get("messages") or [],
    "arguments": body.get("arguments") or {}}))
"#;

/// The verdict the decider answers, on the decider's own wire.
///
/// OR-DP.P.3 / OR-DP-57: the shape is the hosted service's answer as strand E's
/// `decisions` translate reads it back (`answers.<key>` = `{type, choice,
/// probabilities, confidence}`); it is spelled once, here. A key the test does
/// not name gets the quiet default of the one topic the locks use:
/// `sample.lead` = the standard `brief`, `sample.also` = `none` -- an omitted
/// key is a partial decision since GH #977, which `decision_without` says.
pub fn decision(answers: &[(&str, &str, f64)]) -> MockResponse {
    decision_without(answers, &[])
}

/// GH #977: the verdict with the keys in `omit` left out of the answers -- the
/// translate names them in `decision.missing`, and only a call without any
/// answer is still `decision_incomplete`.
pub fn decision_without(answers: &[(&str, &str, f64)], omit: &[&str]) -> MockResponse {
    let mut a = Map::new();
    // GH #963: the shipped presenter offers its own topics `search` and `work` too
    // (`observed_topics`), so their four questions are asked on every turn; the read-back
    // ignores an answer to a question not asked.
    let defaults = [
        ("sample.lead", "brief", 0.5),
        ("sample.also", "none", 0.5),
        ("search.lead", "results", 0.5),
        ("search.also", "none", 0.5),
        ("work.lead", "steps", 0.5),
        ("work.also", "none", 0.5),
    ];
    let named = answers.iter().map(|(k, _, _)| *k).collect::<Vec<_>>();
    let filled = defaults.iter().filter(|(k, _, _)| !named.contains(k));
    for (key, choice, p) in answers.iter().chain(filled) {
        if omit.contains(key) {
            continue;
        }
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

/// GH #963: a message to any path inside the presenter, with a context -- what a builder
/// tap delivers straight onto `./stage` (the lane docks there, past the rim), or a
/// resident's answer at the rim.
pub fn to_path(path: &str, hop: Value, context: Value, body: Value) -> Message {
    MessageBuilder::new(Path::new(path))
        .reply_to(Path::new(SAMPLE_APP))
        .hop(hop.as_object().cloned().unwrap_or_default())
        .context(context.as_object().cloned().unwrap_or_default())
        .body(Body::Inline(body))
        .ttl(24)
        .build()
}

impl Stage {
    /// One turn with text and a round, as the member's observer edge hands it to an app.
    pub async fn turn_in(&self, turn_id: &str, text: &str, round: Value) {
        self.c
            .h
            .send(to_path(
                PRESENTER,
                json!({"route": "turn", "turn_id": turn_id}),
                json!({"audience_set": round.to_string()}),
                json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
            ))
            .await;
    }

    /// One observed tool call, as `observes_tool_calls` delivers it onto `./stage`: through
    /// the relay `/alex/caller` and the edge `install_app` would draw.
    pub async fn tool_call(&self, tool: &str, id: &str, args: Value, context: Value) {
        self.c
            .h
            .send(to_path(
                "/alex/caller",
                json!({"route": "relay", "tool_name": tool, "tool_call_id": id}),
                context,
                json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                                     "text": args.to_string()}], "arguments": args}),
            ))
            .await;
        // The relay is one hop longer than a result sent straight onto `stage`; a result
        // that overtook its call would be dropped as unobserved (measured 03.10.). The
        // call is delivered to `stage` before the test goes on -- `stage` takes its inbox
        // in order, so whatever the test sends next is handled after it.
        let to = format!("{PRESENTER}/stage");
        self.c
            .wait_until("the relayed call reaches stage", || async {
                self.c.log(Some(&to)).await.iter().any(|r| {
                    r.from_path == "/alex/caller" && hop_of(r)["tool_call_id"].as_str() == Some(id)
                })
            })
            .await;
    }

    /// One observed tool result, as the string-form `observes_tool_results` delivers it.
    pub async fn tool_result(&self, id: &str, text: &str, hop: Value, context: Value) {
        let mut h = hop.as_object().cloned().unwrap_or_default();
        h.insert("route".into(), json!("tool_result"));
        self.c
            .h
            .send(to_path(
                &format!("{PRESENTER}/stage"),
                Value::Object(h),
                context,
                json!({"messages": [{"origin": "tool", "type": "tool_result", "id": id,
                                     "text": text}]}),
            ))
            .await;
    }

    /// One turn with text, as the member's observer edge hands it to an app: on a screen
    /// with a round, under that round (the edge stamps the member round, and the screen
    /// shows the member's round -- GH #1027); on a screen without one, with no round.
    pub async fn turn(&self, turn_id: &str, text: &str) {
        let has_round = self.screen.as_array().is_some_and(|a| !a.is_empty());
        if has_round {
            return self.turn_in(turn_id, text, self.screen.clone()).await;
        }
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
        match tokio::time::timeout(MARKER, self.decider.recv()).await {
            Ok(held) => held.expect("the decider stands"),
            Err(_) => panic!(
                "the decider is not asked within 30s\n{}",
                self.trail().await
            ),
        }
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
    /// A red run names its cause: the presenter's hops and the dead letters.
    pub async fn wait_drawn(&self, needle: &str) {
        let deadline = Instant::now() + MARKER;
        while !self.drawn(needle).await {
            if Instant::now() >= deadline {
                panic!(
                    "an object `{needle}` at web did not hold within 30s\n{}",
                    self.trail().await
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The ids at `web` carrying `needle`, in patch order of creation.
    /// Counted once per object, not once per screen: the tree at `web` carries the same
    /// object under several prefixes (`view.<owner>/<id>` and `<screen>.view.<owner>/<id>`,
    /// measured: 6 keys for 3 steps), so only the path part that holds `needle` -- up to the
    /// next `/` or `.` -- identifies the object, and the ids are deduplicated on it.
    pub async fn ids(&self, needle: &str) -> Vec<String> {
        let tree = self.c.tree().await;
        let mut out: Vec<String> = Vec::new();
        for k in tree.as_object().into_iter().flat_map(|m| m.keys()) {
            let Some(at) = k.find(needle) else {
                continue;
            };
            let tail = &k[at..];
            let id = tail
                .find(['/', '.'])
                .map_or(tail, |end| &tail[..end])
                .to_string();
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
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
