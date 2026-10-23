//! GH #1292 -- `templates/figure`: a character in a shared world as an onion
//! around a referenced `simple-agent`.
//!
//! The template is three children: `world` (the resident adapter of the
//! world's lanes and the only sender into the agent's turn door), `phone`
//! (the handset, a human channel seen from the colony) and `agent` (a `ref`
//! of `simple-agent`, never a copy). The world's tools are client tools of
//! each turn: a world call goes out as `call`, its `call_result` goes back
//! into the same turn, and the turn ends when the model stops.
//!
//! This file is the file layer (no colony): the children and the ref, the
//! rim (contract V-W and V-H), the absence of the old names and of product
//! names, the required role and its frames, and the adapters themselves, fed
//! message by message the way their resident runner feeds them. The colony
//! layer with stub models lives in `figure_turns.rs`.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry the template is skipped, never judged.

use meclaw_colony::config::HiveParams;
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    repo("templates/figure/config.json").is_file()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} does not parse: {e}", p.display()))
}

fn figure(rel: &str) -> Value {
    read_json(&repo(&format!("templates/figure/{rel}")))
}

fn routes(lanes: &Value) -> BTreeSet<String> {
    lanes
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["route"].as_str().map(str::to_string))
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// The single-quoted routes a condition names in `hop.route == '<r>'`.
fn named_routes(condition: &str) -> Vec<String> {
    condition
        .split("hop.route == '")
        .skip(1)
        .filter_map(|s| s.split('\'').next().map(str::to_string))
        .collect()
}

/// Every file of the template, relative path and text.
fn files_under(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            files_under(&p, out);
        } else {
            let text = std::fs::read_to_string(&p).expect("a text file");
            out.push((p.display().to_string(), text));
        }
    }
}

/// The adapter `cell` (`world` or `phone`) fed `docs` one after another in ONE
/// process, the way its resident runner feeds it: the script's namespace and
/// its `STATE` survive between messages. Returns the output of each message.
fn resident(cell: &str, overlay: Value, docs: &[(Value, Value, Value)]) -> Vec<Vec<Value>> {
    let cfg = figure(&format!("{cell}/config.json"));
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("an inline script");
    let mut params = cfg["params"].clone();
    let p = params.as_object_mut().expect("params");
    p.remove("script_inline");
    for (k, v) in overlay.as_object().cloned().unwrap_or_default() {
        p.insert(k, v);
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    let file = td.path().join("cell.py");
    std::fs::write(&file, script).expect("write the script");
    // The runner keeps one namespace per child: the script is executed once
    // under a name that is not `__main__`, then `on_message` is called per
    // message with the same `STATE`.
    let driver = td.path().join("drive.py");
    std::fs::write(
        &driver,
        "import json, sys\n\
         ns = {'__name__': 'cell'}\n\
         exec(compile(open(sys.argv[1]).read(), sys.argv[1], 'exec'), ns)\n\
         state = {}\n\
         outs = []\n\
         for doc in json.load(sys.stdin):\n\
         \x20   fn = ns['on_message']\n\
         \x20   outs.append(fn(doc, state) if fn.__code__.co_argcount == 2 else fn(doc))\n\
         sys.stdout.write(json.dumps(outs))\n",
    )
    .expect("write the driver");
    let input: Vec<Value> = docs
        .iter()
        .map(|(hop, ctx, body)| {
            json!({"envelope": {"header": {"hop": hop, "context": ctx}}, "body": body,
                   "params": params.clone()})
        })
        .collect();
    let mut child = std::process::Command::new("python3")
        .arg("-I")
        .arg(&driver)
        .arg(&file)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("python3 runs");
    {
        use std::io::Write;
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(Value::from(input).to_string().as_bytes())
            .expect("feed the adapter");
    }
    let out = child.wait_with_output().expect("the adapter ends");
    assert!(out.status.success(), "the adapter failed: {out:?}");
    meclaw_core::serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("the adapter's output is a JSON array of arrays: {e}"))
}

fn instance() -> Value {
    json!({"figure_id": "ada", "world_id": "w1", "role": "human-sim",
           "phone": {"enabled": true, "speaker": "member", "contact": "Marie"}})
}

fn msg(route: &str, slot: Value) -> (Value, Value, Value) {
    let mut body = json!({"messages": []});
    body[route] = slot;
    (json!({"route": route}), json!({}), body)
}

fn on_hop(route: &str, hop: Value, body: Value) -> (Value, Value, Value) {
    let mut h = hop;
    h["route"] = json!(route);
    (h, json!({}), body)
}

fn route_of(doc: &Value) -> &str {
    doc["header"]["route"].as_str().unwrap_or_default()
}

fn routes_of(out: &[Value]) -> Vec<&str> {
    out.iter().map(route_of).collect()
}

// ═══════════════════════════════════════════════════════ the file

#[test]
fn figure_is_three_children_and_refs_simple_agent() {
    if !shipped() {
        return;
    }
    let tpl = figure("template.json");
    assert_eq!(tpl["name"], "figure");
    // The merge point numbers the version (a memory-hive fix moves figure's
    // third digit); the lock is the line, not a literal of one release.
    let version = tpl["version"].as_str().expect("version");
    let parts: Vec<&str> = version.split('.').collect();
    assert_eq!(parts.len(), 3, "{version}");
    assert_eq!(parts[0], "1", "{version}");
    assert!(parts.iter().all(|p| p.parse::<u32>().is_ok()), "{version}");
    let mut children = BTreeSet::new();
    for entry in std::fs::read_dir(repo("templates/figure")).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            assert!(
                p.join("config.json").is_file(),
                "{} is a directory without a cell",
                p.display()
            );
            children.insert(p.file_name().expect("a name").to_string_lossy().to_string());
        }
    }
    assert_eq!(
        children,
        set(&["phone", "simple-agent", "world"]),
        "exactly three children"
    );
    let agent = figure("simple-agent/config.json");
    assert_eq!(agent["cell"]["type"], "ref", "the agent is referenced");
    assert_eq!(
        agent["cell"]["template"], "simple-agent@1.1.2",
        "the agent is the pinned simple-agent, never a copy"
    );
    // GH #1276: `world` caches its queue, open turn and call book
    // (resident); `phone` keeps nothing between messages (warm).
    for (cell, mode) in [("world", "resident"), ("phone", "warm")] {
        let cfg = figure(&format!("{cell}/config.json"));
        assert_eq!(cfg["cell"]["type"], "code", "{cell} is an adapter");
        assert_eq!(cfg["params"]["runner_mode"], mode, "{cell} is {mode}");
    }
    let mut files = Vec::new();
    files_under(&repo("templates/figure"), &mut files);
    for (path, text) in &files {
        assert!(
            !text.contains("\"type\": \"llm\""),
            "{path}: the figure thinks only through its agent"
        );
    }
}

#[test]
fn figure_lanes_are_the_world_and_phone_contract() {
    if !shipped() {
        return;
    }
    let cfg = figure("config.json");
    let params = &cfg["params"];
    assert_eq!(params["ports"], json!([]), "the figure is sealed");
    let accepts = routes(&params["contract"]["accepts"]);
    let emits = routes(&params["contract"]["emits"]);
    assert_eq!(
        accepts,
        set(&[
            "obs",
            "call_result",
            "night",
            "interview",
            "mandate",
            "answer",
            "in_model",
            "mutation_committed"
        ]),
        "V-W in, the human channel's answer, the agent's two pushes"
    );
    assert_eq!(
        emits,
        set(&[
            "call",
            "intent",
            "night",
            "phone_capped",
            "interview",
            "turn",
            "model_refused",
            "error"
        ]),
        "V-W out, the handset's turn, the agent's refusals"
    );
    let drains: BTreeSet<String> = params["required_drains"]
        .as_array()
        .expect("drains")
        .iter()
        .map(|d| {
            let to = d["emits"].as_str().expect("emits");
            assert!(emits.contains(to), "a drain into the undeclared lane {to}");
            d["accepts"].as_str().expect("accepts").to_string()
        })
        .collect();
    assert_eq!(
        drains,
        set(&[
            "obs",
            "call_result",
            "night",
            "interview",
            "answer",
            "in_model"
        ]),
        "every lane that is answered declares its drain"
    );
    let hp: HiveParams =
        meclaw_core::serde_json::from_value(params.clone()).expect("the figure's params parse");
    let mut opened = BTreeSet::new();
    let mut left = BTreeSet::new();
    for e in &hp.graph.edges {
        let cond = e.condition.as_deref().unwrap_or("");
        let named = named_routes(cond);
        if e.from == "." {
            for r in &named {
                assert!(
                    accepts.contains(r),
                    "the door -> {} opens the undeclared {r}",
                    e.to
                );
                opened.insert(r.clone());
            }
            if e.to == "./world" || e.to == "./phone" {
                assert!(
                    cond.contains("!has(hop.figure_rim)"),
                    "the door -> {} takes back what leaves: {cond}",
                    e.to
                );
            }
        }
        if e.to == "." {
            for r in &named {
                assert!(
                    emits.contains(r),
                    "the exit {} -> . carries the undeclared {r}",
                    e.from
                );
                left.insert(r.clone());
            }
            // GH #863: the model door's one way back names no route, it
            // stamps `model_refused`.
            if e.from == "./simple-agent" && cond == "has(hop.refused_subscriber)" {
                left.insert("model_refused".to_string());
            }
            if e.from != "./simple-agent" {
                assert!(
                    cond.contains("hop.figure_rim == 'out'"),
                    "the exit {} -> . leaves without the rim mark: {cond}",
                    e.from
                );
            }
        }
        if e.to == "./simple-agent" && e.from == "." {
            assert!(
                named
                    .iter()
                    .all(|r| r == "in_model" || r == "mutation_committed"),
                "only the agent's pushes pass the world adapter by: {cond}"
            );
        }
        if e.to == "./simple-agent" && e.from != "." {
            assert_eq!(
                e.from, "./world",
                "the world adapter is the agent's only caller"
            );
        }
    }
    assert_eq!(opened, accepts, "every accepted lane has a door");
    assert_eq!(left, emits, "every emitted lane has an exit");

    // V-W: an obs becomes one turn with the world's tools as client tools.
    let out = resident(
        "world",
        instance(),
        &[msg(
            "obs",
            json!({"turn_id": 7, "kind": "contact", "text": "Tom is here", "day": 2,
                   "tools": [{"name": "look", "description": "look", "parameters": {}}]}),
        )],
    );
    let turn = out[0]
        .iter()
        .find(|d| route_of(d) == "turn")
        .expect("an obs is a turn");
    assert_eq!(turn["header"]["channel"], "world:w1");
    assert_eq!(turn["header"]["session_id"], "day-2");
    assert_eq!(turn["header"]["turn_id"], "7");
    assert_eq!(turn["header"]["audience_set"], "[\"member:ada\"]");
    let tools: Vec<&str> = turn["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(tools, vec!["look", "phone_message"]);

    // V-H: a message of the figure leaves as a door-shaped turn.
    let out = resident(
        "phone",
        json!({}),
        &[on_hop(
            "phone_out",
            json!({"phone_chat_id": "hp:w1:ada"}),
            json!({"phone_out": {"text": "see you"}, "messages": []}),
        )],
    );
    assert_eq!(
        out[0],
        vec![
            json!({"header": {"route": "turn", "chat_id": "hp:w1:ada", "figure_rim": "out"},
                     "messages": [{"origin": "user", "type": "text", "text": "see you"}]})
        ],
        "the human channel's form, nothing more"
    );
}

#[test]
fn figure_has_no_npc_names_and_no_product_names() {
    if !shipped() {
        return;
    }
    assert!(
        !repo("templates/npc").exists(),
        "`npc` left the catalogue: no compatibility path"
    );
    let mut files = Vec::new();
    files_under(&repo("templates/figure"), &mut files);
    for (path, text) in &files {
        let lower = text.to_lowercase();
        for old in ["npc_", "npc-", "-npc", "/npc", "npc@", "agent:npc"] {
            assert!(
                !lower.contains(old),
                "{path} still names `{old}`: only the role value `npc` stays"
            );
        }
        for product in ["egon", "soul2me", "village"] {
            assert!(
                !lower.contains(product),
                "{path} names `{product}`: the template is neutral"
            );
        }
    }
}

#[test]
fn figure_role_is_required_and_frames_differ() {
    if !shipped() {
        return;
    }
    let world = figure("world/config.json");
    assert_eq!(
        world["contract"]["settings"]["role"]["operator_set"],
        json!(true),
        "an instance names the role"
    );
    let obs = msg(
        "obs",
        json!({"turn_id": 1, "kind": "event", "text": "rain", "day": 1}),
    );
    let unset = resident("world", json!({"role": ""}), std::slice::from_ref(&obs));
    assert_eq!(
        routes_of(&unset[0]),
        vec!["error", "intent"],
        "{:?}",
        unset[0]
    );
    assert_eq!(unset[0][0]["error"]["error_code"], "role_unknown");
    assert_eq!(unset[0][1]["intent"], json!({"turn_id": 1, "never": true}));

    let frame = |role: &str| -> String {
        let mut over = instance();
        over["role"] = json!(role);
        let out = resident("world", over, std::slice::from_ref(&obs));
        let pack = out[0]
            .iter()
            .find(|d| route_of(d) == "in_pack")
            .expect("the frame reaches the agent at boot");
        pack["system"]["instructions"]["role"]["text"]
            .as_str()
            .expect("a frame text")
            .to_string()
    };
    let human = frame("human-sim");
    let npc = frame("npc");
    assert_ne!(human, npc, "each role has its own frame");
    for f in [&human, &npc] {
        assert!(
            f.contains("whether you are an AI") && f.contains("truthfully"),
            "R-GD-2: the AI question is answered truthfully: {f}"
        );
        assert!(f.contains("decline"), "free to decline: {f}");
    }
    assert!(
        npc.contains("game"),
        "the npc frame plays a role in a game: {npc}"
    );
}

#[test]
fn figure_world_adapter_runs_one_turn_at_a_time() {
    if !shipped() {
        return;
    }
    let keys = json!({"turn_id": "7", "session_id": "day-2", "iter": "2", "channel": "world:w1",
                      "audience_set": "[\"member:ada\"]"});
    let call = |id: &str, tool: &str, args: Value| {
        let mut hop = keys.clone();
        hop["tool_name"] = json!(tool);
        hop["tool_call_id"] = json!(id);
        on_hop(
            "calls",
            hop,
            json!({"calls": {"name": tool, "arguments": args}, "messages": []}),
        )
    };
    let answer = |tid: &str, text: &str| {
        on_hop(
            "answer",
            json!({"turn_id": tid}),
            json!({"answer": {"turn_id": tid, "text": text}, "messages": []}),
        )
    };
    let out = resident(
        "world",
        instance(),
        &[
            msg(
                "obs",
                json!({"turn_id": 7, "kind": "contact", "text": "Tom", "day": 2,
                       "tools": [{"name": "look", "parameters": {}}]}),
            ),
            // 1: a phone answer during the open turn waits.
            msg(
                "phone_in",
                json!({"channel": "hp:w1:ada", "text": "Hi Ada"}),
            ),
            // 2, 3: two world calls leave as `call`.
            call("c1", "look", json!({"at": "door"})),
            call("c2", "look", json!({"at": "sky"})),
            // 4, 5: their results go back with the keys of their call.
            msg(
                "call_result",
                json!({"turn_id": 7, "call_id": "c1", "result": {"accepted": true}}),
            ),
            msg(
                "call_result",
                json!({"turn_id": 7, "call_id": "c2", "result": {"accepted": false}}),
            ),
            // 6: texting answers at once.
            call("c3", "phone_message", json!({"text": "on my way"})),
            // 7: the final answer ends the turn, the phone turn goes in.
            answer("7", "Hello Tom"),
        ],
    );
    assert_eq!(routes_of(&out[0]), vec!["in_pack", "turn"]);
    assert!(out[1].is_empty(), "the phone turn waits: {:?}", out[1]);
    for (i, id) in [(2, "c1"), (3, "c2")] {
        assert_eq!(routes_of(&out[i]), vec!["call"]);
        assert_eq!(
            out[i][0]["call"]["turn_id"],
            json!(7),
            "an int stays an int"
        );
        assert_eq!(out[i][0]["call"]["call_id"], id);
    }
    for (i, failed) in [(4, false), (5, true)] {
        let r = &out[i][0];
        assert_eq!(route_of(r), "tool_result");
        for k in ["turn_id", "session_id", "iter", "channel", "audience_set"] {
            assert_eq!(
                r["header"][k], keys[k],
                "the result brings back `{k}` of its call"
            );
        }
        assert_eq!(r["tool_result"]["is_error"], json!(failed));
    }
    assert_eq!(routes_of(&out[6]), vec!["phone_out", "tool_result"]);
    assert_eq!(out[6][0]["header"]["phone_chat_id"], "hp:w1:ada");
    assert_eq!(
        out[6][1]["tool_result"]["content"],
        "{\"status\": \"sent\"}"
    );
    assert_eq!(routes_of(&out[7]), vec!["intent", "turn"]);
    assert_eq!(
        out[7][0]["intent"],
        json!({"turn_id": 7, "done": true, "text": "Hello Tom"})
    );
    let phone = &out[7][1];
    assert_eq!(phone["header"]["channel"], "phone:w1");
    assert_eq!(phone["header"]["session_id"], "phone-2");
    assert_eq!(
        phone["tools"],
        json!([]),
        "a phone turn has no client tools"
    );
    assert!(
        phone["messages"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("Message from Marie: Hi Ada")),
        "{phone}"
    );
}

#[test]
fn figure_night_acks_after_every_session_of_the_day() {
    if !shipped() {
        return;
    }
    let report = |s: &str, added: u64| {
        msg(
            "close_report",
            json!({"session_id": s, "ok": true, "added": added, "closed": 1}),
        )
    };
    let out = resident(
        "world",
        instance(),
        &[
            msg(
                "obs",
                json!({"turn_id": "a", "kind": "event", "text": "x", "day": 4}),
            ),
            msg("phone_in", json!({"channel": "hp:w1:ada", "text": "hey"})),
            on_hop(
                "answer",
                json!({"turn_id": "a"}),
                json!({"answer": {"turn_id": "a", "text": "ok"}, "messages": []}),
            ),
            // 3: the night waits behind the open phone turn.
            msg("night", json!({"day": 4})),
        ],
    );
    let phone_key = out[2][1]["header"]["turn_id"]
        .as_str()
        .expect("the phone turn's id")
        .to_string();
    assert!(
        out[3].is_empty(),
        "a night waits for the open turn: {:?}",
        out[3]
    );
    let out = resident(
        "world",
        instance(),
        &[
            msg(
                "obs",
                json!({"turn_id": "a", "kind": "event", "text": "x", "day": 4}),
            ),
            msg("phone_in", json!({"channel": "hp:w1:ada", "text": "hey"})),
            on_hop(
                "answer",
                json!({"turn_id": "a"}),
                json!({"answer": {"turn_id": "a", "text": "ok"}, "messages": []}),
            ),
            msg("night", json!({"day": 4})),
            on_hop(
                "answer",
                json!({"turn_id": phone_key}),
                json!({"answer": {"turn_id": phone_key, "text": ""}, "messages": []}),
            ),
            report("day-4", 3),
            report("phone-4", 2),
        ],
    );
    let closes: Vec<&Value> = out[4].iter().filter(|d| route_of(d) == "close").collect();
    assert_eq!(
        closes
            .iter()
            .map(|d| d["close"]["session_id"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["day-4", "phone-4"],
        "an empty reply sends nothing, then the night closes both sessions"
    );
    assert!(
        !routes_of(&out[4]).contains(&"phone_out"),
        "an empty final text is no reply"
    );
    assert!(out[5].is_empty(), "no receipt before the last report");
    assert_eq!(
        out[6],
        vec![json!({"header": {"route": "night", "figure_rim": "out"},
                     "night": {"day": 4, "ok": true, "added": 5, "closed": 2}, "messages": []})]
    );
}

#[test]
fn figure_failures_name_their_error_code() {
    if !shipped() {
        return;
    }
    let out = resident(
        "world",
        instance(),
        &[
            msg(
                "interview",
                json!({"question_id": 5, "question": "Are you an AI?"}),
            ),
            on_hop(
                "answer",
                json!({"turn_id": "interview-5"}),
                json!({"answer": {"turn_id": "interview-5", "error_code": "model_timeout"},
                       "messages": []}),
            ),
            msg(
                "obs",
                json!({"turn_id": 9, "kind": "event", "text": "x", "day": 1}),
            ),
            on_hop(
                "answer",
                json!({"turn_id": "9"}),
                json!({"answer": {"turn_id": "9", "error": true}, "messages": []}),
            ),
            msg("obs", json!({"turn_id": 10, "kind": "event", "text": "x"})),
            msg("phone_in", json!({"channel": "hp:w1:bob", "text": "?"})),
            msg("phone_in", json!({"text": "no channel at all"})),
        ],
    );
    let ask = out[0]
        .iter()
        .find(|d| route_of(d) == "turn")
        .expect("an interview is a turn");
    assert_eq!(ask["header"]["channel"], "interview:w1");
    assert_eq!(ask["tools"], json!([]), "an interview offers no tools");
    assert_eq!(
        out[1][0]["interview"],
        json!({"question_id": 5, "answer": null, "error": "model_timeout",
               "error_code": "model_timeout"})
    );
    assert_eq!(routes_of(&out[3]), vec!["error", "intent"]);
    assert!(
        out[3][0]["error"]["error_code"]
            .as_str()
            .is_some_and(|c| !c.is_empty()),
        "never a null code: {:?}",
        out[3]
    );
    assert_eq!(out[3][1]["intent"], json!({"turn_id": 9, "never": true}));
    assert_eq!(out[4][0]["error"]["error_code"], "obs_without_day");
    assert_eq!(out[5][0]["error"]["error_code"], "foreign_channel");
    // V-H: only an answer on THIS channel is the figure's own; none is no proof.
    assert_eq!(out[6][0]["error"]["error_code"], "foreign_channel");
    assert_eq!(
        routes_of(&out[6]),
        vec!["error"],
        "no phone turn: {:?}",
        out[6]
    );
}

/// One client call of the agent into `world`, with the five round keys of
/// turn `tid` (contract V-T).
fn call_in(tid: &str, id: &str, tool: &str, args: Value) -> (Value, Value, Value) {
    on_hop(
        "calls",
        json!({"turn_id": tid, "session_id": "day-2", "iter": "1", "channel": "world:w1",
               "audience_set": "[\"member:ada\"]", "tool_name": tool, "tool_call_id": id}),
        json!({"calls": {"name": tool, "arguments": args}, "messages": []}),
    )
}

fn answer_in(tid: &str, text: Value) -> (Value, Value, Value) {
    on_hop(
        "answer",
        json!({"turn_id": tid}),
        json!({"answer": {"turn_id": tid, "text": text}, "messages": []}),
    )
}

fn phone_in(text: &str) -> (Value, Value, Value) {
    msg("phone_in", json!({"channel": "hp:w1:ada", "text": text}))
}

fn not_offered(out: &[Value], tool: &str) {
    assert_eq!(routes_of(out), vec!["tool_result"], "{tool}: {out:?}");
    let r = &out[0]["tool_result"];
    assert_eq!(r["is_error"], json!(true), "{tool}: {r}");
    assert_eq!(r["error_code"], "tool_not_offered", "{tool}: {r}");
    assert!(
        r["content"]
            .as_str()
            .is_some_and(|c| c.contains("tool_not_offered")),
        "the model reads the code too: {r}"
    );
}

#[test]
fn a_call_outside_the_menu_of_its_turn_is_refused_as_tool_not_offered() {
    if !shipped() {
        return;
    }
    let docs = vec![
        msg(
            "obs",
            json!({"turn_id": 7, "kind": "event", "text": "x", "day": 2,
                   "tools": [{"name": "look", "parameters": {}}]}),
        ),
        call_in("7", "c1", "walk", json!({"to": "market"})),
        call_in("7", "c2", "look", json!({})),
        answer_in("7", json!("done")),
        phone_in("Hi Ada"),
    ];
    let out = resident("world", instance(), &docs);
    not_offered(&out[1], "walk");
    assert_eq!(
        routes_of(&out[2]),
        vec!["call"],
        "an offered tool still leaves"
    );
    let phone_turn = out[4]
        .iter()
        .find(|d| route_of(d) == "turn")
        .expect("the phone turn");
    let key = phone_turn["header"]["turn_id"]
        .as_str()
        .expect("a phone turn id")
        .to_string();
    // The phone turn offers no tools: texting back inside it is no second
    // channel for its reply.
    let mut more = docs.clone();
    more.push(call_in(&key, "c3", "phone_message", json!({"text": "hi"})));
    let out = resident("world", instance(), &more);
    not_offered(&out[5], "phone_message");
}

#[test]
fn a_phone_turn_may_stay_silent() {
    if !shipped() {
        return;
    }
    let silent = on_hop("answer", json!({}), json!({"answer": {}, "messages": []}));
    let out = resident(
        "world",
        instance(),
        &[
            phone_in("Are you there?"),
            answer_in("", json!("")),
            phone_in("Hello?"),
            silent,
            phone_in("Dinner?"),
            answer_in("", json!("Sure.")),
        ],
    );
    let frame = out[0]
        .iter()
        .find(|d| route_of(d) == "turn")
        .expect("a phone turn");
    assert!(
        frame["messages"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("You may stay silent") && t.contains("nothing is sent")),
        "the phone turn says it may stay silent: {frame}"
    );
    assert!(
        out[1].is_empty(),
        "an empty final text sends nothing: {:?}",
        out[1]
    );
    assert!(
        out[3].is_empty(),
        "an end without text sends nothing: {:?}",
        out[3]
    );
    assert_eq!(routes_of(&out[5]), vec!["phone_out"], "a reply leaves");
}

#[test]
fn phone_replies_over_the_cap_of_the_day_open_no_turn() {
    if !shipped() {
        return;
    }
    let mut overlay = instance();
    overlay["phone_replies_max"] = json!(2);
    let obs = |tid: u64, day: u64| {
        msg(
            "obs",
            json!({"turn_id": tid, "kind": "event", "text": "x", "day": day}),
        )
    };
    let out = resident(
        "world",
        overlay,
        &[
            obs(1, 1),
            answer_in("1", json!("ok")),
            phone_in("one"),
            answer_in("", json!("")),
            phone_in("two"),
            answer_in("", json!("")),
            phone_in("three"),
            obs(2, 1),
            answer_in("2", json!("ok")),
            obs(3, 2),
            answer_in("3", json!("ok")),
            phone_in("four"),
        ],
    );
    assert_eq!(routes_of(&out[2]), vec!["turn"]);
    assert_eq!(routes_of(&out[4]), vec!["turn"]);
    assert_eq!(
        routes_of(&out[6]),
        vec!["phone_capped"],
        "the third reply of the day opens no turn: {:?}",
        out[6]
    );
    assert_eq!(out[6][0]["header"]["figure_rim"], "out");
    assert_eq!(
        out[6][0]["phone_capped"],
        json!({"day": 1, "channel": "hp:w1:ada", "text": "three", "replies": 2, "max": 2})
    );
    let next = &out[7][0]["messages"][0]["text"];
    assert!(
        next.as_str().is_some_and(|t| t.contains("- three")),
        "the capped reply reaches the next world turn: {next}"
    );
    assert!(
        out[9][0]["messages"][0]["text"]
            .as_str()
            .is_some_and(|t| !t.contains("three")),
        "once"
    );
    assert_eq!(routes_of(&out[11]), vec!["turn"], "a new day counts anew");
}
