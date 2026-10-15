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

use meclaw_colony::api_dto::MessageLogDto;
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::mock_http::{HeldRequest, MockResponse, start_mock_server_held};
use tokio::sync::mpsc;

use super::sealed::{Key, Sealed};
use super::{
    Boot, Colony, PROBE, Unsettled, Why, boot_with, calls_of, copy_tree, have_python, hop_of,
    patch_json, repo,
};

// The bridge from the compile probe to this fixture: the held decider IS the sealed one.
const _: fn(&Stage) -> &Sealed<mpsc::UnboundedReceiver<HeldRequest>> = |s| &s.decider;

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
    /// The decider's own operation timeout (`decide` `params.external_timeout_ms`); its
    /// message backstop is 15 s above it. 25 s by default: the test HOLDS the decider, and
    /// the shipped 3 s would fire first on a cold lane. A lock that holds the decider past
    /// the hang mark dials it higher.
    pub decide_timeout_ms: u64,
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
            decide_timeout_ms: 25_000,
        }
    }
}

/// A booted colony with a presenter, its held decider and the egress the test reads.
pub struct Stage {
    pub c: Colony,
    /// The held decider's requests; [`Stage::ask`] is the one way to them. Sealed
    /// (OR-T11-25): even a `pub` field would hand a test nothing to call.
    decider: Sealed<mpsc::UnboundedReceiver<HeldRequest>>,
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
    let c = boot_with(Key::new(), Boot::default(), move |root| {
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
            v["params"]["external_timeout_ms"] = json!(d.decide_timeout_ms);
            v["cell"]["message_timeout"] = json!(d.decide_timeout_ms + 15_000);
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
        decider: Sealed::new(decider),
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
            .w
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
    // no-settle: a send and its positive wait under a possibly held request; returns nothing
    pub async fn tool_call(&self, tool: &str, id: &str, args: Value, context: Value) {
        self.c
            .w
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
                let r = self.c.w.unsettled().because(Why::Watch);
                r.log(Some(&to)).await.iter().any(|r| {
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
            .w
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
            .w
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
            .w
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
            .w
            .send(to_presenter(
                "show_data",
                json!({"show_app": app}),
                json!({"messages": [], "topic": "sample", "turn_id": turn_id, "sets": sets}),
            ))
            .await;
    }

    /// The next message that left `/alex` whose hop route is `route` (others are kept for
    /// a later call), or a panic after the failure-marker window.
    // no-settle: the event itself: the next message that left /alex
    pub async fn out(&mut self, route: &str) -> Message {
        if let Some(i) = self
            .held
            .iter()
            .position(|m| m.headers.hop.get("route").and_then(Value::as_str) == Some(route))
        {
            return self.held.remove(i);
        }
        // Re-check tv-bn M7 M1 (TV-S3, TV-Z3): the scaled hang mark of the kit, not a std
        // `Instant` plus the unscaled `MARKER`. Re-check tv-bn M7b M1 (TV-S1): the wait is the
        // channel's own event, `recv`, as in `Colony::refusals` -- a `try_recv` polled under
        // `within_hang` woke on the poll interval, not on the message. A hang names the
        // routes that did leave /alex meanwhile.
        let (seen_tx, seen_rx) = tokio::sync::watch::channel(routes_of(&self.held));
        let w = &self.c.w;
        let held = &mut self.held;
        let got = async move {
            loop {
                let Some(msg) = w.next_out().await else {
                    panic!(
                        "the egress closed before `{route}` left /alex: {:?}",
                        routes_of(held)
                    );
                };
                if msg.headers.hop.get("route").and_then(Value::as_str) == Some(route) {
                    return msg;
                }
                held.push(msg);
                seen_tx.send_replace(routes_of(held));
            }
        };
        meclaw_testing::deadline::hang_with(&format!("`{route}` leaves /alex"), got, move || {
            seen_rx.borrow().clone()
        })
        .await
    }

    /// Wait until `stage` has sent the decide call of `turn_id` to `./decide` -- read at
    /// the receiver. `stage` runs one message at a time, so every event before that turn
    /// was handled too: the sentinel of a lock. (The decider itself is serial: a held
    /// request keeps the next one from reaching the stub, so the stub cannot be it.)
    ///
    /// Re-check tv-bn M7b4 B-M1: the decide call is an event of `stage` only. A claim of
    /// absence about any other path is judged once the request is released and the colony
    /// settled ([`Self::before_the_verdict`]), never at this event.
    // no-settle: a wait for a positive event under a held request; returns nothing
    pub async fn decided_on(&self, turn_id: &str) {
        let to = format!("{PRESENTER}/decide");
        self.c
            .wait_until("the decide call of the sentinel turn", || async {
                self.c
                    .w
                    .unsettled()
                    .because(Why::Watch)
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
        let r = self.c.w.settled().await;
        let mut out = Vec::new();
        for cell in ["decide", "store"] {
            let cfg = super::read_json(&repo("templates/presenter").join(cell).join("config.json"));
            let emits = &cfg["contract"]["emits"];
            let from = format!("{PRESENTER}/{cell}");
            let mut seen = 0;
            for row in r.log(None).await.iter().filter(|r| r.from_path == from) {
                seen += 1;
                let body = super::body_of(row);
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
    // no-settle: the account of a red run, read inside failing waits while a request may be held
    pub async fn trail(&self) -> Unsettled<String> {
        let r = self.c.w.unsettled().because(Why::Report);
        let rows: Vec<String> = r
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
        let dlq = r.take_dead_letters().await;
        Unsettled::new(format!("{}\ndead letters: {dlq:?}", rows.join("\n")))
    }

    /// Every `in_show` the test has seen so far and not taken, plus what is waiting once
    /// the colony settled.
    pub async fn held_routes(&mut self) -> Vec<String> {
        let waiting = self.c.w.settled().await.drain_egress().await;
        self.held.extend(waiting);
        self.held
            .iter()
            .filter_map(|m| m.headers.hop.get("route").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    }

    /// The next request the decider holds.
    // no-settle: the event itself: the next request the decider holds
    pub async fn ask(&mut self) -> HeldRequest {
        // Re-check tv-bn M7b M1 (TV-S1): `recv` under the hang mark, not a `try_recv` polled
        // under `within_hang`. A hang names the routes that had left /alex when it began.
        let routes = routes_of(&self.held);
        let decider = self.decider.open_mut();
        let got = async move { decider.recv().await.expect("the decider stands") };
        meclaw_testing::deadline::hang_with("the decider is asked", got, move || routes).await
    }

    /// The journal rows `stage` wrote, oldest first (the store bundles at `store`).
    pub async fn journal(&self) -> Vec<Value> {
        journal_in(
            self.c
                .w
                .settled()
                .await
                .log(Some(&format!("{PRESENTER}/store")))
                .await,
        )
    }

    /// The journal row of one turn, once it stands: the last one written for it once the
    /// colony settled after the first stood (re-check tv-bn M7b4 B-M2: the row at the event
    /// is half a turn).
    // no-settle: polls for the row's positive event (Why::Watch), the hang text is Why::Report; hands back the row read at rest
    pub async fn journal_of(&self, turn_id: &str) -> Value {
        let store = format!("{PRESENTER}/store");
        meclaw_testing::wait::until_some_with(
            &format!("a journal row for {turn_id} stands"),
            || async {
                let r = self.c.w.unsettled().because(Why::Watch);
                journal_in(r.log(Some(&store)).await)
                    .into_iter()
                    .any(|r| r["turn_id"] == turn_id)
                    .then_some(())
            },
            || async { self.trail().await.because(Why::Report) },
        )
        .await;
        self.journal()
            .await
            .into_iter()
            .rev()
            .find(|r| r["turn_id"] == turn_id)
            .expect("a journal row that stood still stands")
    }

    /// The journal row of one turn while the test holds `held`: the row as it stands at
    /// the event, read without the barrier -- the colony does not rest under a held request
    /// (a settle there waited until `decide` gave up, `timeout` after 25 003 ms, re-check
    /// tv-bn M7b4). The argument is the hold, so a read without the barrier says so in its
    /// call (re-check tv-bn M7b4 B-M1); what it is good for is a positive claim about the
    /// row itself, never one about the rest of the colony.
    // no-settle: a turn's row while the HeldRequest argument holds the decider; read at its event
    pub async fn journal_of_held(&self, held: &HeldRequest, turn_id: &str) -> Unsettled<Value> {
        let _ = held;
        let store = format!("{PRESENTER}/store");
        let row = meclaw_testing::wait::until_some_with(
            &format!("a journal row for {turn_id} stands while the decider is held"),
            || async {
                let r = self.c.w.unsettled().because(Why::Held);
                journal_in(r.log(Some(&store)).await)
                    .into_iter()
                    .rev()
                    .find(|r| r["turn_id"] == turn_id)
            },
            || async { self.trail().await.because(Why::Report) },
        )
        .await;
        Unsettled::new(row)
    }

    /// The pending record of one turn once `stage` has written it as done (the store
    /// bundle at `store`), or a panic after the failure-marker window.
    // no-settle: polls for the row's positive event (Why::Watch), the hang text is Why::Report; hands back the row read at rest
    pub async fn pending_done(&self, turn_id: &str) -> Value {
        let store = format!("{PRESENTER}/store");
        meclaw_testing::wait::until_some_with(
            &format!("a done pending row for {turn_id} stands"),
            || async {
                let r = self.c.w.unsettled().because(Why::Watch);
                done_pending_in(&r.log(Some(&store)).await, turn_id).map(|_| ())
            },
            || async { self.trail().await.because(Why::Report) },
        )
        .await;
        // Re-check tv-bn M7b4 A-M1: the row handed back is read at rest, not at the event.
        done_pending_in(&self.c.log(Some(&store)).await, turn_id)
            .expect("a done pending row that stood still stands")
    }

    /// Whether the patch log at `web` ever created an object whose id carries `needle`.
    pub async fn drawn(&self, needle: &str) -> bool {
        carries(&self.c.w.settled().await.tree().await, needle)
    }

    /// Wait until an object whose id carries `needle` stands at `web`.
    /// A red run names its cause: the presenter's hops and the dead letters.
    // no-settle: a wait for a positive event under a possibly held request; returns nothing
    pub async fn wait_drawn(&self, needle: &str) {
        meclaw_testing::wait::until_some_with(
            &format!("an object `{needle}` stands at web"),
            || async {
                let tree = self.c.w.unsettled().because(Why::Watch).tree().await;
                carries(&tree, needle).then_some(())
            },
            || async { self.trail().await.because(Why::Report) },
        )
        .await
    }

    /// Every row of the log that `pick` selects whose `parent_message_id` chain does NOT
    /// reach a row `is_cause` names, read once the colony settled, over the WHOLE log; with
    /// the number of rows picked.
    ///
    /// The chain is followed as far as it goes: a parent the log does not hold, or a cycle
    /// (each row is visited once, re-check tv-bn M7b9 A-M3), counts as NOT descending --
    /// fail closed, a row whose cause cannot be shown is reported (re-check tv-bn M7b5 #5).
    ///
    /// A clock's strike is a source message (no parent): its cause is the order that set
    /// it. tv-bn M7b8 (TV-F7, gh1027 3/5 at CPUQuota 80): the fresh second a new window
    /// orders (§ 4.34) strikes one second after its pass, and that pass carries the window
    /// to `web` again -- a chain that ended at the strike reported the member's own window
    /// as caused by nothing, whenever the run lasted past that second. So a strike (`hop.route`
    /// `in_tick` with a `schedule_id`; it enters the log from `@external`) is followed on
    /// into the `add` orders of that `schedule_id` its receiver sent before it; it descends when one of them does (the
    /// clock holds one order per second, and two passes close together order the same
    /// second, GH #681). A strike without such an order is a root that shows no cause.
    pub async fn descends_from(
        &self,
        pick: impl Fn(&MessageLogDto) -> bool,
        is_cause: impl Fn(&MessageLogDto) -> bool,
    ) -> (usize, Vec<String>) {
        let rows = self.c.w.settled().await.whole_log().await;
        let by_id: std::collections::HashMap<&str, &MessageLogDto> =
            rows.iter().map(|r| (r.id.as_str(), r)).collect();
        let mut picked = 0;
        let mut apart = Vec::new();
        for row in rows.iter().filter(|r| pick(r)) {
            picked += 1;
            if let Err(root) = reaches(row, &rows, &by_id, &is_cause) {
                // TV-F2: the report names where the chain ended, so a red row says its
                // cause; a strike there names each order of its second and where the
                // chain of that order ended (tv-bn M7b9).
                let orders: Vec<String> = orders_of(root, &rows)
                    .map(|o| match reaches(o, &rows, &by_id, &is_cause) {
                        Ok(()) => format!("order {} reaches", o.id),
                        Err(end) => format!(
                            "order {} ends at {} -> {} {} {}",
                            o.id,
                            end.from_path,
                            end.to_path,
                            end.headers_json,
                            end.body_payload
                                .as_deref()
                                .unwrap_or("")
                                .chars()
                                .take(300)
                                .collect::<String>()
                        ),
                    })
                    .collect();
                apart.push(format!(
                    "{} -> {} {} | chain ends at {} -> {} {} | orders {orders:?}",
                    row.from_path,
                    row.to_path,
                    row.headers_json,
                    root.from_path,
                    root.to_path,
                    root.headers_json
                ));
            }
        }
        (picked, apart)
    }

    /// Every row of the log that `pick` selects whose `parent_message_id` chain does NOT
    /// reach the decider's answer to `turn_id` (the hop `decide` -> `stage` of that turn),
    /// read once the colony settled; with the number of rows picked.
    ///
    /// Re-check tv-bn M7b4 B-M1: "nothing before the verdict" was judged while the test
    /// held the decider, with `ask()` as the fence -- but the decide call orders only the
    /// decide branch, and a premature request on a side path (stage -> edge -> affinity)
    /// could still be on its way. Released, settled and traced back, a premature row is in
    /// the log and has no verdict among its ancestors: the claim has the full horizon.
    pub async fn before_the_verdict(
        &self,
        turn_id: &str,
        pick: impl Fn(&MessageLogDto) -> bool,
    ) -> (usize, Vec<String>) {
        self.descends_from(pick, is_verdict(turn_id)).await
    }

    /// Every row `pick` selects that the log holds BEFORE the decider's answer to `member`,
    /// that descends from the turn `guest` (its ingress at the presenter), or whose cause
    /// is no named one; read once the colony settled; with the number of rows picked.
    ///
    /// tv-bn M7b9 (TV-F7, gh1027 2/10 at CPUQuota 80): the curator is stateful, so a pass
    /// that some other event started -- here the stroke of a second the `warm` window
    /// ordered, struck while the member's pass was replacing that order -- carries the
    /// member's window to the screen as well. Such a row descends from no verdict; it
    /// descends from the write of `warm`. So the causes a row after the verdict may have
    /// are named: the member's verdict, or the write of a view in `written` (an app's
    /// `in_view` into the screen, [`Colony::write_view`]).
    ///
    /// Fail closed (re-check tv-bn M7b9 A-M3 / C-M1): EVERY cause of the row is followed
    /// -- the parent chain, and at a stroke every order of its second, each row once -- and
    /// the row is reported when any branch ends at the guest's turn, at a parent the log
    /// does not hold, at a stroke without an order, or at a root that is no named cause,
    /// or when no branch reaches a named cause at all (a cycle). Before any row is judged,
    /// the guest's branch has to work: the log holds the guest's turn, and a row other than
    /// that turn traces back to it (its journal row, say) -- a search that cannot find the
    /// guest would call every row innocent.
    pub async fn of_the_turn_or_before(
        &self,
        guest: &str,
        member: &str,
        written: &[&str],
        pick: impl Fn(&MessageLogDto) -> bool,
    ) -> (usize, Vec<String>) {
        let rows = self.c.w.settled().await.whole_log().await;
        guest_or_before(&rows, guest, member, written, pick)
    }

    /// The ids at `web` carrying `needle`, in patch order of creation.
    /// Counted once per object, not once per screen: the tree at `web` carries the same
    /// object under several prefixes (`view.<owner>/<id>` and `<screen>.view.<owner>/<id>`,
    /// measured: 6 keys for 3 steps), so only the path part that holds `needle` -- up to the
    /// next `/` or `.` -- identifies the object, and the ids are deduplicated on it.
    pub async fn ids(&self, needle: &str) -> Vec<String> {
        let tree = self.c.w.settled().await.tree().await;
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
        for row in self.c.w.settled().await.log(None).await {
            if row.from_path == format!("{PRESENTER}/stage") {
                let hop = hop_of(&row);
                out.push((
                    hop["route"].as_str().unwrap_or("").to_string(),
                    super::body_of(&row),
                ));
            }
        }
        out
    }

    /// Install the app's topic and wait until `stage` has written it (the `shows` row).
    // no-settle: a send and its positive wait; returns nothing
    pub async fn install_sample(&self) {
        self.topics(json!([sample_topic()])).await;
        meclaw_testing::wait::until_some_with(
            "the topic `sample` is accepted",
            || async {
                let r = self.c.w.unsettled().because(Why::Watch);
                let rows = r.log(Some(&format!("{PRESENTER}/store"))).await;
                rows.iter()
                    .filter_map(calls_of)
                    .flatten()
                    .any(|c| {
                        // GH #1199: an app's topics are written as ONE insert of `rows`.
                        c["operation"] == "insert"
                            && c["table"] == "shows"
                            && c["rows"]
                                .as_array()
                                .is_some_and(|rows| rows.iter().any(|r| r["topic"] == "sample"))
                    })
                    .then_some(())
            },
            || async { self.trail().await.because(Why::Report) },
        )
        .await
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
// no-settle: a write and its positive wait; returns nothing
pub async fn warm(s: &Stage) {
    s.c.write_view(PROBE, "warm", json!({"pane_id": "warm", "title": "warm"}))
        .await;
    s.wait_drawn("warm").await;
}

/// The routes `stage` emitted, in log order, once the colony settled.
pub async fn stage_routes(s: &Stage) -> Vec<String> {
    s.c.settle().await;
    s.outputs_of_stage()
        .await
        .into_iter()
        .map(|(r, _)| r)
        .collect()
}

/// Where the causes of a row end, over every branch: the parent chain, and at a stroke
/// (a source row) every `add` order of its second (see [`Stage::descends_from`]). Each row
/// is visited once (re-check tv-bn M7b9 A-M3: the old shared step budget ran out on many
/// seconds and ended a branch as "no cause"); a branch that comes back to a visited row
/// adds no end.
#[derive(Debug, Clone, Copy)]
enum End<'a> {
    /// A row `stop` names.
    Named(&'a MessageLogDto),
    /// A source row that is no stroke, or a stroke without an order: no cause shown.
    Root(&'a MessageLogDto),
    /// A row whose parent the log does not hold.
    Broken(&'a MessageLogDto),
}

fn ends<'a>(
    row: &'a MessageLogDto,
    rows: &'a [MessageLogDto],
    by_id: &std::collections::HashMap<&str, &'a MessageLogDto>,
    stop: &impl Fn(&MessageLogDto) -> bool,
) -> Vec<End<'a>> {
    let mut seen = std::collections::HashSet::new();
    let mut todo = vec![row];
    let mut out = Vec::new();
    while let Some(at) = todo.pop() {
        if !seen.insert(at.id.as_str()) {
            continue;
        }
        if stop(at) {
            out.push(End::Named(at));
            continue;
        }
        match at.parent_message_id.as_deref() {
            Some(p) => match by_id.get(p) {
                Some(parent) => todo.push(parent),
                None => out.push(End::Broken(at)),
            },
            None => {
                let before = todo.len();
                todo.extend(orders_of(at, rows));
                if todo.len() == before {
                    out.push(End::Root(at));
                }
            }
        }
    }
    out
}

/// Whether `row`'s causes reach a row `is_cause` names: `Ok` when ONE branch does (a
/// positive claim; at a stroke one order suffices, OR-…M7b8-1), else `Err` with the row a
/// branch ended at (the row itself for a cycle).
fn reaches<'a>(
    row: &'a MessageLogDto,
    rows: &'a [MessageLogDto],
    by_id: &std::collections::HashMap<&str, &'a MessageLogDto>,
    is_cause: &impl Fn(&MessageLogDto) -> bool,
) -> Result<(), &'a MessageLogDto> {
    let found = ends(row, rows, by_id, is_cause);
    if found.iter().any(|e| matches!(e, End::Named(_))) {
        return Ok(());
    }
    Err(found.first().map_or(row, |e| match *e {
        End::Named(r) | End::Root(r) | End::Broken(r) => r,
    }))
}

/// The claim of [`Stage::of_the_turn_or_before`] over a settled log (pure, so the pin
/// `a_guest_cause_or_an_untraced_row_is_reported` judges it on hand-made logs).
pub fn guest_or_before(
    rows: &[MessageLogDto],
    guest: &str,
    member: &str,
    written: &[&str],
    pick: impl Fn(&MessageLogDto) -> bool,
) -> (usize, Vec<String>) {
    let by_id: std::collections::HashMap<&str, &MessageLogDto> =
        rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let is_guest = is_turn(guest);
    let is_member = is_verdict(member);
    let is_written = |r: &MessageLogDto| {
        r.parent_message_id.is_none()
            && r.to_path == super::SCREEN
            && hop_of(r)["route"] == "in_view"
            && super::body_of(r)["view_id"]
                .as_str()
                .is_some_and(|v| written.contains(&v))
    };
    let mut apart = Vec::new();
    // The guest's branch works: its turn is in the log, and the search finds a row of it.
    if !rows.iter().any(&is_guest) {
        apart.push(format!("the log holds no turn `{guest}` at the presenter"));
    } else if !rows
        .iter()
        .any(|r| !is_guest(r) && reaches(r, rows, &by_id, &is_guest).is_ok())
    {
        apart.push(format!(
            "no row traces back to the turn `{guest}`: the search cannot see the guest"
        ));
    }
    let verdict = rows.iter().position(&is_member);
    let named = |r: &MessageLogDto| is_guest(r) || is_member(r) || is_written(r);
    let mut picked = 0;
    for (at, row) in rows.iter().enumerate().filter(|(_, r)| pick(r)) {
        picked += 1;
        let head = format!("{} -> {} {}", row.from_path, row.to_path, row.headers_json);
        if verdict.is_none_or(|v| at < v) {
            apart.push(format!("{head} | logged before the verdict of `{member}`"));
            continue;
        }
        let found = ends(row, rows, &by_id, &named);
        let mut why: Vec<String> = found
            .iter()
            .filter_map(|e| match e {
                End::Named(r) if is_guest(r) => Some(format!("descends from the turn `{guest}`")),
                End::Named(_) => None,
                End::Root(r) => Some(format!(
                    "a cause ends at a root that is no named cause: {} -> {} {}",
                    r.from_path, r.to_path, r.headers_json
                )),
                End::Broken(r) => Some(format!(
                    "a cause ends at a parent the log does not hold: {} -> {} {}",
                    r.from_path, r.to_path, r.headers_json
                )),
            })
            .collect();
        if !found.iter().any(|e| matches!(e, End::Named(_))) {
            why.push("no branch reaches a named cause".to_string());
        }
        if !why.is_empty() {
            apart.push(format!("{head} | {}", why.join("; ")));
        }
    }
    (picked, apart)
}

/// The `add` orders a strike follows into (see [`Stage::descends_from`]): those of its
/// `schedule_id` its receiver sent to the clock before it. None for a row that is no strike.
fn orders_of<'a>(
    strike: &'a MessageLogDto,
    rows: &'a [MessageLogDto],
) -> impl Iterator<Item = &'a MessageLogDto> {
    let hop = hop_of(strike);
    let sid = hop["schedule_id"]
        .as_str()
        .filter(|_| hop["route"] == "in_tick")
        .map(str::to_string);
    rows.iter().filter(move |o| {
        let body = super::body_of(o);
        sid.is_some()
            && o.from_path == strike.to_path
            && o.created_at <= strike.created_at
            && body["op"] == "add"
            && body["schedule_id"].as_str() == sid.as_deref()
    })
}

/// The ingress of the turn `turn_id`: the hop `turn` into the presenter ([`Stage::turn_in`]).
fn is_turn(turn_id: &str) -> impl Fn(&MessageLogDto) -> bool + '_ {
    move |r: &MessageLogDto| {
        let hop = hop_of(r);
        r.to_path == PRESENTER && hop["route"] == "turn" && hop["turn_id"].as_str() == Some(turn_id)
    }
}

/// The decider's answer to `turn_id`: the hop `decide` -> `stage` of that turn.
fn is_verdict(turn_id: &str) -> impl Fn(&MessageLogDto) -> bool + '_ {
    let decide = format!("{PRESENTER}/decide");
    let stage = format!("{PRESENTER}/stage");
    move |r: &MessageLogDto| {
        r.from_path == decide
            && r.to_path == stage
            && hop_of(r)["show_id"].as_str() == Some(turn_id)
    }
}

/// The journal rows among the rows logged at `store`, oldest first.
fn journal_in(rows: Vec<MessageLogDto>) -> Vec<Value> {
    let mut out = Vec::new();
    for row in rows {
        for call in calls_of(&row).unwrap_or_default() {
            if call["operation"] == "insert" && call["table"] == "journal" {
                out.push(call["row"].clone());
            }
        }
    }
    out
}

/// The newest pending record of `turn_id` written as done among the rows logged at `store`.
fn done_pending_in(rows: &[MessageLogDto], turn_id: &str) -> Option<Value> {
    for row in rows.iter().rev() {
        for call in calls_of(row).unwrap_or_default() {
            let v = &call["row"]["value"];
            if call["operation"] == "insert"
                && call["table"] == "pending"
                && v["turn_id"] == turn_id
                && v["state"] == "done"
            {
                return Some(v.clone());
            }
        }
    }
    None
}

/// Whether `tree` holds an object whose id carries `needle`.
fn carries(tree: &Value, needle: &str) -> bool {
    tree.as_object()
        .map(|m| m.keys().any(|k| k.contains(needle)))
        .unwrap_or(false)
}

/// The hop routes of `held`, in order: the last state a hang of [`Stage::out`] or
/// [`Stage::ask`] names.
fn routes_of(held: &[Message]) -> Vec<String> {
    held.iter()
        .filter_map(|m| m.headers.hop.get("route").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}
