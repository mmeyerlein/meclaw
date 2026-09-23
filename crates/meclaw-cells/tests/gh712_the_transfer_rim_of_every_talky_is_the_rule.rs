//! GH #712 — the transfer rim of every talky is ONE rule, and the template is held
//! against it.
//!
//! Since `assistant@2.7.0` a generation holds two talkys (`./talky`, `./talky-chat`),
//! and each carries a session keeper of its own. Until `session-keeper@2.2.2` the
//! keeper's porter filed its document under the constant `HIVE = "session-keeper"`
//! and the member imported on `hop.import_hive == 'session-keeper'`, so two keepers
//! of one generation collided on the way out and could not be told apart on the way
//! in — which is why the four transfer lanes stood at `./talky` alone and the chat
//! keeper's sessions never travelled (measured on the e26 → e27 transfer: one keeper
//! directory, 29 rows, the chat ledger absent).
//!
//! The repair has three halves and this file pins each of them statically; the
//! colony run that proves them together is
//! `gh712_every_keeper_files_its_sessions_under_its_own_node.rs`.
//!
//! 1. **The rim is rendered, not hand-drawn.** [`talky_transfer`] is the rendering
//!    rule: four edges per talky node. For EVERY child `./talky*` of the shipped
//!    assistant the transfer edges at that node are exactly the rule's, and the rule
//!    rendered for a node the template does not have (`talky-voice`) has the shape
//!    of the template's own non-default talky — so a third talky gets its transfer
//!    rim from the rule, never from a hand-typed edge.
//! 2. **The porter locates itself.** No `HIVE =` literal any more; the directory is
//!    derived from `envelope.target` — the last two segments above the porter,
//!    `<talky>/session-keeper` — and a faked stdin proves the derivation.
//! 3. **The member routes on the path.** `hop.import_hive.endsWith('/session-keeper')`.
//!
//! Guarded like every template-reading test (GH #49).

use meclaw_core::serde_json::{Value, from_str, json};
use meclaw_testing::{emit_one, shipped_script};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped(rel: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(repo(rel)).ok()?;
    Some(from_str(&raw).unwrap_or_else(|e| panic!("{rel}: {e}")))
}

/// The four transfer lanes of the assistant level's rim, in the order they are
/// asked for.
const TRANSFER: [&str; 4] = ["in_export", "in_import", "export_done", "dump"];

/// The `delete_context` every exit of the assistant level carries — the working
/// keys of the level's own rounds, which must not leak one level up.
const EXIT_SCRUB: [&str; 4] = ["col_phase", "consult_class", "consult_id", "tool_answerer"];

/// **The rendering rule.** The four transfer edges of one talky node of the
/// assistant level.
///
/// - `in_export` is PLAIN: every keeper writes into its own directory
///   (`<talky>/session-keeper`), so a fan-out to all of them is the whole export of
///   the generation and nothing collides.
/// - `in_import` reads the address off the hop: a part is filed under the keeper's
///   path relative to the generation, so the talky whose name is the first segment
///   takes it. The DEFAULT rim — `./talky`, the node every lane without a channel
///   of its own reaches (the `in_turn` split has the same asymmetry) — also takes a
///   part that names no address: a bare route is what the door probe of the
///   mutation door sends (`HiveContract::probe`), and a level whose every
///   `in_import` door is guarded on a hop key has no door for the lane at all.
/// - `export_done` and `dump` are plain exits with the level's scrub: a drain that
///   tests a second key reads as no drain under the `required_drains` probe.
fn talky_transfer(node: &str) -> Vec<Value> {
    let at = format!("./{node}");
    let import_guard = if node == "talky" {
        format!("(!has(hop.import_hive) || hop.import_hive.startsWith('{node}/'))")
    } else {
        format!("has(hop.import_hive) && hop.import_hive.startsWith('{node}/')")
    };
    vec![
        json!({"from": ".", "to": at,
               "condition": "has(hop.route) && hop.route == 'in_export'"}),
        json!({"from": ".", "to": at,
               "condition": format!("has(hop.route) && hop.route == 'in_import' && {import_guard}")}),
        json!({"from": at, "to": ".",
               "condition": "has(hop.route) && hop.route == 'export_done'",
               "modifier": {"delete_context": EXIT_SCRUB}}),
        json!({"from": at, "to": ".",
               "condition": "has(hop.route) && hop.route == 'dump'",
               "modifier": {"delete_context": EXIT_SCRUB}}),
    ]
}

fn lane_of(e: &Value) -> String {
    e["condition"]
        .as_str()
        .unwrap_or_default()
        .split("hop.route == '")
        .nth(1)
        .and_then(|r| r.split('\'').next())
        .unwrap_or_default()
        .to_string()
}

/// The transfer edges the shipped template draws at one node, in rule order.
fn transfer_edges_at(edges: &[Value], node: &str) -> Vec<Value> {
    let at = format!("./{node}");
    let mut out = Vec::new();
    for lane in TRANSFER {
        out.extend(
            edges
                .iter()
                .filter(|e| e["to"] == at.as_str() || e["from"] == at.as_str())
                .filter(|e| lane_of(e) == lane)
                .cloned(),
        );
    }
    out
}

/// Every child of the shipped assistant template that is a talky: a directory
/// whose name starts with `talky` and whose config is a ref onto `talky`.
fn talky_children() -> Vec<String> {
    let root = repo("templates/assistant");
    let mut out: Vec<String> = std::fs::read_dir(&root)
        .expect("templates/assistant")
        .flatten()
        .filter(|e| e.path().join("config.json").is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let cfg = shipped(&format!("templates/assistant/{name}/config.json"))?;
            let tpl = cfg["cell"]["template"].as_str().unwrap_or_default();
            (cfg["cell"]["type"] == "ref" && tpl.split('@').next() == Some("talky")).then_some(name)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn every_talky_of_the_level_carries_the_rendered_transfer_rim() {
    let Some(assistant) = shipped("templates/assistant/config.json") else {
        return;
    };
    let edges = assistant["params"]["graph"]["edges"]
        .as_array()
        .expect("assistant edges")
        .clone();
    let talkys = talky_children();
    assert_eq!(
        talkys,
        vec!["talky".to_string(), "talky-chat".to_string()],
        "the level's talkys are read off the tree; a third one is fine, and this \
         assertion is the place that says so -- the loop below then holds it to the rule"
    );
    for node in &talkys {
        assert_eq!(
            Value::Array(transfer_edges_at(&edges, node)),
            Value::Array(talky_transfer(node)),
            "`./{node}` does not carry the transfer rim the rule renders. Every keeper \
             of a generation is reached by the export (plain), takes the import part \
             whose address starts with its own node, and hands `export_done` and `dump` \
             out plainly (GH #712)"
        );
    }
}

#[test]
fn the_rule_rendered_for_a_talky_the_template_does_not_have_has_the_templates_shape() {
    let Some(assistant) = shipped("templates/assistant/config.json") else {
        return;
    };
    let edges = assistant["params"]["graph"]["edges"]
        .as_array()
        .expect("assistant edges")
        .clone();
    // A fictitious third talky. Its rim is rendered, then read back in the role of the
    // shipped non-default talky: if the two are equal, the template's `talky-chat`
    // rim IS the rule and a `talky-voice` would get the same form by the same rule,
    // without a hand-typed edge.
    let rendered = Value::Array(talky_transfer("talky-voice"));
    let as_chat: Value =
        from_str(&rendered.to_string().replace("talky-voice", "talky-chat")).unwrap();
    assert_eq!(
        as_chat,
        Value::Array(transfer_edges_at(&edges, "talky-chat")),
        "the rule for a third talky no longer has the shape of the template's own \
         non-default talky"
    );
    let import = rendered[1]["condition"].as_str().unwrap_or_default();
    assert!(
        import.contains("hop.import_hive.startsWith('talky-voice/')")
            && !import.contains("!has(hop.import_hive)"),
        "a non-default talky takes exactly the parts addressed to it: {import}"
    );
}

#[test]
fn the_porter_derives_its_directory_from_its_own_path() {
    let Some(porter) = shipped("templates/session-keeper/porter/config.json") else {
        return;
    };
    let script = porter["params"]["script_inline"].as_str().expect("script");
    assert!(
        !script.contains("HIVE ="),
        "the porter still carries a hive-name constant. Two keepers of one generation \
         then file their documents under one directory and the second walk overwrites \
         the first (GH #712)"
    );
    assert!(
        script.contains("envelope.get(\"target\""),
        "the porter does not read its own path. `envelope.target` is the one thing a \
         code cell knows about where it stands (`crates/meclaw-cells/src/code/wire.rs`)"
    );
}

#[test]
fn the_member_routes_a_keeper_part_on_the_path_it_names() {
    let Some(member) = shipped("templates/member/config.json") else {
        return;
    };
    let to_assistants: Vec<&Value> = member["params"]["graph"]["edges"]
        .as_array()
        .expect("member edges")
        .iter()
        .filter(|e| e["from"] == "." && e["to"] == "./assistants" && lane_of(e) == "in_import")
        .collect();
    assert_eq!(to_assistants.len(), 1, "{to_assistants:?}");
    let cond = to_assistants[0]["condition"].as_str().unwrap_or_default();
    assert!(
        cond.contains("hop.import_hive.endsWith('/session-keeper')"),
        "the member routes a keeper part on the bare hive name; since \
         session-keeper@2.2.2 the part names `<talky>/session-keeper`: {cond}"
    );
}

/// One porter hop, faked the way the substrate would hand it over.
fn porter_emits(target: &str, hop: Value, ctx: Value, text: &str) -> Value {
    let script = shipped_script(
        repo("templates/session-keeper/porter/config.json")
            .to_str()
            .expect("utf-8 path"),
    );
    emit_one(
        &script,
        &json!({
            "target": target,
            "header": {"hop": hop, "context": ctx},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": text}],
        }),
    )
}

#[test]
fn a_porter_under_the_chat_talky_writes_into_the_chat_talkys_directory() {
    if shipped("templates/session-keeper/porter/config.json").is_none() {
        return;
    }
    let target = "/members/m/assistants/a/talky-chat/session-keeper/porter";
    let out = porter_emits(
        target,
        json!({"route": "in_export", "export_to": "run"}),
        json!({"port_phase": "export", "port_run": "r1"}),
        "",
    );
    assert_eq!(
        out["transfer"]["to"], "run/talky-chat/session-keeper",
        "the export slot names the keeper's own directory under the run: {out}"
    );
    let bare = porter_emits(
        target,
        json!({"route": "in_export"}),
        json!({"port_phase": "export", "port_run": "r1"}),
        "",
    );
    assert_eq!(
        bare["transfer"]["to"], "talky-chat/session-keeper",
        "{bare}"
    );

    // The store's answer to the slot: the completion word names the same path.
    let done = porter_emits(
        target,
        json!({"route": "kstore", "operation": "export"}),
        json!({"port_phase": "export-file", "port_run": "r1"}),
        &json!({"seed_dir": "run/talky-chat/session-keeper/seed", "tables": ["sessions"],
                "rows": {"sessions": 3}})
        .to_string(),
    );
    assert_eq!(done["header"]["route"], "export_done", "{done}");
    assert_eq!(
        done["header"]["export_hive"], "talky-chat/session-keeper",
        "`export_done` names the directory the keeper wrote: {done}"
    );
    assert_eq!(done["header"]["rows_written"], 3);
}
