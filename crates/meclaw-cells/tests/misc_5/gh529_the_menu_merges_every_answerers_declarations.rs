//! GH #529 — the tool menu is a UNION over answerers, and it needs a memory.
//!
//! Since GH #464 a collector's tool menu is asked for rather than typed:
//! `params.tools` names what the agent declares it uses, an answerer sends the
//! declarations back on `in_menu`, and the collector writes them as the brain's
//! `system.tools` with `$replace` — so the subtree becomes exactly what came
//! back. (Since GH #889 that write goes to the curator, which hands it to the
//! brain with the next call.)
//!
//! That is right while exactly ONE thing answers, and it is the whole defect the
//! moment two do. A second answer would not merge with the first, it would
//! delete it, and the two answerers would take the menu away from each other on
//! every tick, forever.
//!
//! A second answerer is now required, and the reason is not a preference. A tool
//! the composite reaches by an EDGE ON ITS NAME is not the tool hive's — it is
//! topology of the level that draws the edge, and only the side that ANSWERS it
//! can declare it. `consult_cogny` is exactly such a name: before #464 its
//! schema was a hand-typed seed row in the surface's brain, #464 replaced the
//! hand-typed menu with an asked-for one, and the row went with it. The agent
//! kept a charter naming a tool the menu no longer carried.
//!
//! What this file pins, over the SHIPPED assembler:
//!
//! 1. **An answer is recorded, not written.** `in_menu` emits one store bundle —
//!    delete this answerer's row, insert the submenu it just delivered, select
//!    them all — and nothing else. Recording under the answerer's own key is
//!    what makes a union possible at all.
//! 2. **The write is the union.** Two answerers with disjoint sets produce ONE
//!    menu carrying both, and nothing else — since GH #889 the collector serves
//!    no name of its own — still under `$replace`, with `hop.menu_answerers`
//!    naming whose rows it stands on.
//! 3. **The order is decided, not accidental.** The union is ordered by answerer,
//!    and a name two answerers both declare is taken from the first of them —
//!    a menu no provider would accept declares one tool twice.
//! 4. **Both guards hold.** An answer with nothing usable records nothing and
//!    writes nothing, so the other answerer's half stands; and a merge that
//!    finds no row writes nothing either, because an empty `$replace` revokes
//!    the model's whole tool set.
//! 5. **`menu_unknown` is computed against the merged menu.** A name one
//!    answerer has nothing under but another delivers is not a finding; a name
//!    nobody delivers still is.
//! 6. **No answerer named is the old shape.** A tree wired before this behaves
//!    exactly as it did, under the default answerer.
//! 7. **A store refusal in a menu phase is not an answer.** A menu is durable
//!    state with no turn beside it, so a refusal is a warn line and a stop —
//!    never a `degraded` reply in front of somebody who asked nothing.
//! 8. **The shipped wiring carries it.** The assistant level draws the second
//!    `schemas` / `in_menu` pair to its core and, since GH #552, a third out of
//!    the level to the member's memory; every `tool_schemas` edge stamps an
//!    answerer, and the collector's own store has the table the round trip writes
//!    to.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{code_stdin, run_shipped_script, shipped_script};
use std::collections::BTreeSet;

const ASSEMBLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// R2b / GH #49: a tree without the template SKIPS instead of failing.
fn shipped(rel: &str) -> Option<Value> {
    let p = templates_root().join(rel);
    let raw = std::fs::read_to_string(p).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

const MEM: &str = "memory_recall";

fn run(doc: &Value) -> (Vec<Value>, String) {
    let out = run_shipped_script(&shipped_script(ASSEMBLE), &doc.to_string());
    assert!(out.status.success(), "the assembler exited non-zero");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("json out");
    (
        match v {
            Value::Array(a) => a,
            other => vec![other],
        },
        stderr,
    )
}

/// One `{name, description, parameters}` declaration, the provider-NEUTRAL shape
/// every answerer hands back.
fn decl(name: &str) -> Value {
    described(name, &format!("what {name} does"))
}

/// The same, with a description the caller chooses -- which is how a collision
/// between two answerers is made observable.
fn described(name: &str, description: &str) -> Value {
    json!({"name": name, "description": description,
           "parameters": {"type": "object", "properties": {}}})
}

/// Phase 1: one answerer's reply, recorded.
fn record(
    answerer: &str,
    schemas: &[Value],
    unknown: &[&str],
    declared: &Value,
) -> (Vec<Value>, String) {
    run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "in_menu"},
                   "context": {"tool_answerer": answerer}},
        "ttl": 64,
        "messages": [],
        "schemas": schemas,
        "unknown": unknown,
        "params": {"tools": declared.clone()},
    })))
}

/// The store row phase 1 asked for, read off the bundle's own `insert` leg.
fn row_of(recorded: &[Value]) -> Value {
    let leg = recorded.first().expect("phase 1 emits one bundle")["messages"]
        .as_array()
        .expect("a bundle of tool_calls")
        .iter()
        .find(|m| m["id"] == json!("c-menu-put"))
        .expect("the bundle inserts this answerer's submenu")
        .clone();
    let args: Value =
        meclaw_core::serde_json::from_str(leg["text"].as_str().expect("a tool_call text"))
            .expect("the op is json");
    args["row"].clone()
}

/// Phase 2: the merge, driven with the rows a store would hand back.
fn merge(rows: &[Value], declared: &Value) -> (Vec<Value>, String) {
    run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [{"id": "c-menu-all", "type": "tool_result",
                      "text": meclaw_core::serde_json::to_string(rows).unwrap()}],
        "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        "params": {"tools": declared.clone()},
    })))
}

/// The tool names on a written menu. A SET, because `system.tools` is a JSON
/// object and its order is the serialiser's, not the cell's -- what the cell
/// decides is WHICH names are there and WHOSE declaration each one carries, and
/// both of those are measured below.
fn names(msg: &Value) -> BTreeSet<String> {
    msg["system"]["tools"]
        .as_object()
        .expect("the subtree is an object")
        .keys()
        .filter(|k| *k != "$replace")
        .cloned()
        .collect()
}

fn want(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| (*s).to_string()).collect()
}

/// The declaration one leaf carries, unwrapped from the provider envelope.
fn leaf(msg: &Value, name: &str) -> Value {
    meclaw_core::serde_json::from_str(
        msg["system"]["tools"][name]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` must be a `text` leaf: {msg:#?}")),
    )
    .expect("a leaf holds json")
}

// ══════════════════════════════════════ 1. an answer is recorded, not written

#[test]
fn an_answer_is_recorded_under_the_answerer_that_gave_it() {
    let declared = json!(["a_tool", "consult_cogny"]);
    let (out, stderr) = record("tools", &[decl("a_tool")], &["consult_cogny"], &declared);
    assert_eq!(out.len(), 1, "one bundle and nothing beside it: {out:#?}");
    let msg = &out[0];
    assert_eq!(
        msg["header"]["route"], "cstore",
        "the answer goes to the store first: a union over answerers needs a memory, and \
         this hive's memory is its own store — before #529 the answer WAS the menu, which \
         meant a second answerer's reply deleted the first's: {msg:#?}"
    );
    assert_eq!(msg["header"]["phase"], "menu-merge");
    assert!(
        msg.get("system").is_none(),
        "nothing is written to the brain on this leg: {msg:#?}"
    );

    let legs: Vec<(String, Value)> = msg["messages"]
        .as_array()
        .expect("a bundle")
        .iter()
        .map(|m| {
            (
                m["id"].as_str().unwrap_or_default().to_string(),
                meclaw_core::serde_json::from_str(m["text"].as_str().expect("text"))
                    .expect("json op"),
            )
        })
        .collect();
    let ids: Vec<&str> = legs.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(
        ids,
        vec!["c-menu-put", "c-menu-all"],
        "replace this answerer's row with the new one in ONE write (GH #1199), read them \
         all — in ONE bundle, so the select runs over the same connection after the write \
         and sees it (GH #419): {legs:#?}"
    );
    assert_eq!(legs[0].1["operation"], "insert");
    assert!(
        legs[0].1.get("where").is_none(),
        "no separate delete leg: {legs:#?}"
    );
    assert_eq!(
        legs[0].1["replace"],
        json!({"answerer": "tools"}),
        "the replacement is scoped to the answerer that spoke; one without that scope is \
         the very overwrite this issue exists to remove: {legs:#?}"
    );
    assert_eq!(legs[0].1["row"]["answerer"], "tools");
    assert_eq!(
        legs[0].1["row"]["unknown"], "consult_cogny",
        "what THIS answerer had nothing under travels with its row instead of being \
         reported here: another answerer may well deliver it: {legs:#?}"
    );
    assert_eq!(legs[1].1["operation"], "select");
    assert!(
        stderr.is_empty(),
        "an answer with a declaration in it is not a finding: {stderr}"
    );
}

// ═════════════════════════════════════════════ 2.-3. the write is the union

#[test]
fn three_answerers_produce_one_menu_carrying_all_of_them() {
    // Three since GH #552: the tools hive, the reasoning core, and the member's
    // own MEMORY, which declares the one tool it answers. The merge machinery
    // did not change for the third — that is the claim.
    let declared = json!(["a_tool", "consult_cogny", MEM]);
    let (rec_tools, _) = record(
        "tools",
        &[decl("a_tool")],
        &["consult_cogny", MEM],
        &declared,
    );
    let (rec_core, _) = record(
        "cogny",
        &[decl("consult_cogny")],
        &["a_tool", MEM],
        &declared,
    );
    let (rec_mem, _) = record(
        "memory",
        &[decl(MEM)],
        &["a_tool", "consult_cogny"],
        &declared,
    );
    let rows = [row_of(&rec_tools), row_of(&rec_core), row_of(&rec_mem)];

    let (out, stderr) = merge(&rows, &declared);
    assert_eq!(out.len(), 1, "one menu message: {out:#?}");
    let msg = &out[0];
    assert_eq!(msg["header"]["route"], "menu");
    // GH #889: the union and nothing else -- `thread_recall`, the one name this
    // cell served itself, is gone with the round table it read.
    assert_eq!(
        names(msg),
        want(&["consult_cogny", "a_tool", MEM]),
        "the union of all three answerers: {msg:#?}"
    );
    assert_eq!(
        msg["system"]["tools"]["$replace"],
        json!(true),
        "still a `$replace`: what changed is WHOSE declarations the replaced subtree holds, \
         never that the subtree is replaced — a menu upserted leaf by leaf would keep every \
         declaration anybody ever dropped: {msg:#?}"
    );
    assert_eq!(msg["header"]["menu_count"], "3");
    assert_eq!(
        msg["header"]["menu_answerers"], "cogny,memory,tools",
        "the receipt says whose rows the write stands on: {msg:#?}"
    );
    assert_eq!(msg["header"]["menu_self"], "");
    assert_eq!(
        msg["header"]["menu_unknown"], "",
        "each answerer had nothing under the OTHERS' tools, and none of those is a \
         finding: `menu_unknown` is computed against the merged menu (GH #529): {msg:#?}"
    );
    assert!(stderr.is_empty(), "nothing to report: {stderr}");
}

#[test]
fn a_name_two_answerers_declare_is_taken_from_the_first_of_them() {
    let declared = json!(["shared"]);
    let (a, _) = record(
        "aaa",
        &[described("shared", "the first answerer's")],
        &[],
        &declared,
    );
    let (b, _) = record(
        "zzz",
        &[described("shared", "the second answerer's")],
        &[],
        &declared,
    );
    // Handed back in the order that would produce the WRONG answer if the cell
    // trusted the store's order instead of deciding one.
    let (out, _) = merge(&[row_of(&b), row_of(&a)], &declared);
    let msg = &out[0];
    // GH #889: no self-served name beside it any more.
    assert_eq!(
        names(msg),
        want(&["shared"]),
        "one leaf per name: a menu that declared the same tool twice is a menu no provider \
         accepts: {msg:#?}"
    );
    assert_eq!(
        leaf(msg, "shared")["function"]["description"],
        "the first answerer's",
        "the union is ordered by ANSWERER and the first of them wins -- decided, and not \
         whatever order the store happened to hand the rows back in, which is the order this \
         case deliberately reverses: {msg:#?}"
    );
    assert_eq!(msg["header"]["menu_answerers"], "aaa,zzz");
}

// ═══════════════════════════════════════════════════════ 4. both guards hold

#[test]
fn an_empty_answer_records_nothing_so_the_other_half_stands() {
    let declared = json!(["a_tool", "consult_cogny"]);
    let (out, stderr) = record("cogny", &[], &["a_tool", "consult_cogny"], &declared);
    assert!(
        out.is_empty(),
        "an answer with nothing usable in it must not reach the store either: overwriting \
         this answerer's stored half with an empty one is the same revocation one tick \
         later that writing an empty `$replace` is right now: {out:#?}"
    );
    assert!(
        stderr.contains("a_tool") && stderr.contains("consult_cogny"),
        "and it still says WHICH names came back empty — a name nobody has is a declaration \
         in the agent's own template pointing at nothing, and seeing that is the whole value \
         of declaring: {stderr:?}"
    );
}

#[test]
fn a_merge_with_no_rows_writes_nothing_at_all() {
    let (out, _) = merge(&[], &json!(["a_tool"]));
    assert!(
        out.is_empty(),
        "`tools_slot` is a `$replace`, so an empty write revokes the model's whole tool set \
         — the same guard as on the way in, on the other side of the round trip: {out:#?}"
    );
}

// ═════════════════════════════════════ 5. unknown, against the merged menu

#[test]
fn a_name_no_answerer_delivers_is_still_reported() {
    let declared = json!(["a_tool", "telepathy"]);
    let (rec, _) = record("tools", &[decl("a_tool")], &["telepathy"], &declared);
    let (out, stderr) = merge(&[row_of(&rec)], &declared);
    let msg = &out[0];
    assert_eq!(
        msg["header"]["menu_unknown"], "telepathy",
        "reported on the message: {msg:#?}"
    );
    assert!(
        stderr.contains("telepathy"),
        "and on stderr, which a `code` cell puts into `log.jsonl` at warn level with \
         `had_stderr` stamped on the emission: {stderr:?}"
    );
}

// ══════════════════════════════════ 6. the one-answerer shape is unchanged

#[test]
fn an_answer_with_no_answerer_named_is_the_shape_every_tree_had_before() {
    let declared = json!(["a_tool"]);
    let (out, _) = run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "in_menu"}, "context": {}},
        "ttl": 64,
        "messages": [],
        "schemas": [decl("a_tool")],
        "unknown": [],
        "params": {"tools": declared.clone()},
    })));
    assert_eq!(row_of(&out)["answerer"], "tools", "the default answerer");
    let (menu, _) = merge(&[row_of(&out)], &declared);
    // GH #889: no self-served name beside it any more.
    assert_eq!(
        names(&menu[0]),
        want(&["a_tool"]),
        "which produces exactly the menu a one-answerer tree wrote before #529: {menu:#?}"
    );
}

// ═════════════════════════ 7. a store refusal in a menu phase is not an answer

#[test]
fn a_store_refusal_on_the_menu_lane_never_becomes_a_reply() {
    let (out, stderr) = run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle",
                           "error_code": "table_unknown"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [],
        "params": {"tools": ["a_tool"]},
    })));
    assert!(
        out.is_empty(),
        "a menu is durable state and no turn travels beside it, so a refused merge has \
         nothing to report INTO a conversation: the `degraded` answer every other phase \
         sends would put 'context assembly stopped' in front of somebody who asked \
         nothing. The tick asks again: {out:#?}"
    );
    assert!(
        stderr.contains("menu-merge") && stderr.contains("table_unknown"),
        "and it is said where a `code` cell's failures are said: {stderr:?}"
    );
}

// ═════════════════════════════════════════ 8. the shipped tree carries it

#[test]
fn the_collectors_own_store_has_the_table_the_round_trip_writes_to() {
    let Some(cfg) = shipped("collector/window/config.json") else {
        return;
    };
    let menu = &cfg["params"]["schema"]["menu"];
    assert!(
        menu.is_object(),
        "the submenu rows live in the collector's OWN store — a cell reads only its own \
         `cell.db`, so 'its own state' is this table and nowhere else: {cfg:#?}"
    );
    for col in ["answerer", "tools", "unknown", "recorded_at"] {
        assert!(
            menu.get(col).is_some(),
            "the `menu` table is missing `{col}`: {menu:#?}"
        );
    }
    assert_eq!(
        menu["tools"], "json",
        "a submenu is a list of declarations, not a string somebody re-parses by hand"
    );
}

#[test]
fn every_declaration_answer_in_the_shipped_assistant_names_its_answerer() {
    let Some(cfg) = shipped("assistant/config.json") else {
        return;
    };
    let edges = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("a level has edges");

    let answers: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("hop.route == 'tool_schemas'"))
        })
        .collect();
    assert_eq!(
        answers.len(),
        5,
        "two answerers reach the menus of the three askers: the tool hive answers all \
         three callers and the core answers the two conversation keepers. Three of the \
         five were the whole set until GH #709 gave the channel `chat` a keeper of its \
         own -- the return edges are told apart by `context.tool_caller` exactly as they \
         already were, so a menu comes back to the keeper that asked for it and never to \
         the other: {answers:#?}"
    );
    for e in &answers {
        let who = e["modifier"]["set_context"]["tool_answerer"]
            .as_str()
            .unwrap_or_else(|| panic!("a declaration answer must name its answerer: {e:#?}"));
        assert!(
            who.starts_with('\'') && who.ends_with('\''),
            "the answerer is a literal, not a field read: {e:#?}"
        );
        assert_eq!(
            e["modifier"]["set_hop"]["route"], "'in_menu'",
            "and it lands on the asking collector's menu lane: {e:#?}"
        );
    }

    // Both conversation keepers ask, and each asks the same three places. Since GH #709
    // the level holds one talky per channel that asks for its own, and a keeper that did
    // not ask would run on a menu somebody else's round had merged.
    for surface in ["./talky", "./talky-chat"] {
        let asks: Vec<&Value> = edges
            .iter()
            .filter(|e| {
                e["from"] == json!(surface)
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains("hop.route == 'schemas'"))
            })
            .collect();
        assert_eq!(
            asks.len(),
            3,
            "{surface} asks both occupants AND the level's rim what they serve — the tool \
             hive has nothing under `consult_cogny`, the core has nothing under the search \
             tools, the member's memory has nothing under any of them, and since #529 none \
             of those is a finding: {asks:#?}"
        );
        let mut targets: Vec<&str> = asks.iter().map(|e| e["to"].as_str().unwrap()).collect();
        targets.sort_unstable();
        assert_eq!(
            targets,
            vec![".", "./cogny", "./tools"],
            "the third one leaves the level, because a memory belongs to the MEMBER and not \
             to one of its generations (GH #552)"
        );
    }

    assert!(
        !edges.iter().any(|e| e["condition"]
            .as_str()
            .is_some_and(|c| c.contains("ask_memory"))),
        "GH #530: the lookup errand is retired — the class it carried assumed the core \
         could answer a memory question, and the core has no memory leg while the surface \
         has one one hop away"
    );
}

#[test]
fn the_level_declares_the_errand_its_own_topology_answers() {
    let Some(surface) = shipped("assistant/talky/config.json") else {
        return;
    };
    let declared = &surface["override_params"]["collector/assemble"]["tools"];
    let list: Vec<&str> = declared
        .as_array()
        .unwrap_or_else(|| panic!("the level declares the surface's tool list: {surface:#?}"))
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect();
    assert!(
        list.contains(&"consult_cogny"),
        "`consult_cogny` is not a tool of any hive — it is the errand this LEVEL routes from \
         its surface to its core, so only the level can declare it. Standalone, a talky has \
         no core beside it and declares its two search tools: {list:#?}"
    );

    let Some(talky) = shipped("talky/collector/config.json") else {
        return;
    };
    let own: Vec<&str> = talky["override_params"]["assemble"]["tools"]
        .as_array()
        .expect("talky declares its own list")
        .iter()
        .map(|v| v.as_str().unwrap_or_default())
        .collect();
    assert!(
        !own.contains(&"consult_cogny"),
        "and it must NOT be declared one level down: a standalone talky would then ask every \
         tick for a declaration nothing beside it can answer: {own:#?}"
    );
    for name in &own {
        assert!(
            list.contains(name),
            "the level's list replaces talky's rather than adding to it, so anything the \
             composite declares standalone has to be repeated here or it is silently \
             dropped: `{name}` is missing from {list:#?}"
        );
    }
}

// ═══════════════════════════════════ 4. the fold (GH #1244, T5-02 … T5-04)

/// A declaration with typed parameters: `props` in declared order, `required`
/// the ones without `?` on the signature line.
fn typed(name: &str, description: &str, props: &[&str], required: &[&str]) -> Value {
    let mut p = meclaw_core::serde_json::Map::new();
    for k in props {
        p.insert((*k).to_string(), json!({"type": "string"}));
    }
    json!({"name": name, "description": description,
           "parameters": {"type": "object", "properties": p, "required": required}})
}

/// The rows of a menu with every case of R-WM-2 in it: the file family (read
/// and write together, write declared first), the workspace family of its own,
/// the tools hive's `file` with its own `op`, a two-member `web` family, an orga
/// family beside the single op tool `orga_change`, and the memory's one tool
/// (under the minimum size of two).
fn fold_rows() -> Vec<Value> {
    let all = json!(["*"]);
    let (space, _) = record(
        "file-space",
        &[
            typed(
                "file_write",
                "Write a file. It replaces the text.",
                &["file", "text"],
                &["file", "text"],
            ),
            typed(
                "file_read",
                "Describe a file (address `fh-<12 hex>` or a path) -- path, kind, size. \
                   More sentences follow.",
                &["file", "offset"],
                &["file"],
            ),
            typed("file_ws_open", "Open a workspace.", &["name"], &[]),
            typed(
                "file_ws_diff",
                "The diff of a workspace.",
                &["name"],
                &["name"],
            ),
        ],
        &[],
        &all,
    );
    let (tools, _) = record(
        "tools",
        &[
            typed(
                "file",
                "The tools hive's own file tool.",
                &["op", "path"],
                &["op"],
            ),
            typed("web_search", "Search the web.", &["query"], &["query"]),
            typed("web_fetch", "Fetch one page.", &["url"], &["url"]),
            typed("bash", "Run a command.", &["cmd"], &["cmd"]),
        ],
        &[],
        &all,
    );
    let (orga, _) = record(
        "orga",
        &[
            typed("orga_find", "Find a record.", &["query"], &["query"]),
            typed("orga_note", "Note a record.", &["text"], &["text"]),
            typed(
                "orga_change",
                "Change a record.",
                &["op", "id"],
                &["op", "id"],
            ),
        ],
        &[],
        &all,
    );
    let (mem, _) = record("memory", &[decl(MEM)], &[], &all);
    vec![row_of(&space), row_of(&tools), row_of(&orga), row_of(&mem)]
}

fn merge_with(rows: &[Value], params: Value) -> Value {
    let (out, stderr) = run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [{"id": "c-menu-all", "type": "tool_result",
                      "text": meclaw_core::serde_json::to_string(rows).unwrap()}],
        "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        "params": params,
    })));
    assert!(stderr.is_empty(), "nothing to report: {stderr}");
    assert_eq!(out.len(), 1, "one menu message: {out:#?}");
    out[0].clone()
}

fn folded() -> Value {
    merge_with(&fold_rows(), json!({"tools": ["*"], "fold_tools": "1"}))
}

#[test]
fn a_folded_menu_groups_the_tools_by_their_name_prefix() {
    let msg = folded();
    assert_eq!(
        names(&msg),
        want(&["bash", "file", "file_ws", MEM, "orga", "orga_change", "web"]),
        "first name segment, the workspace its own family, op tools single, a family needs \
         two members: {msg:#?}"
    );
    assert_eq!(msg["header"]["menu_count"], "7");
    assert_eq!(msg["header"]["menu_folded"], "file,file_ws,orga,web");
    assert_eq!(
        msg["header"]["menu_shadowed"], "file",
        "the tools hive's `file` loses to the file family and says so: {msg:#?}"
    );
    let file = leaf(&msg, "file");
    assert_eq!(
        file["function"]["parameters"]["properties"]["op"]["enum"],
        json!(["read", "write", "help"]),
        "the `file` leaf is the family, not the tools hive's tool: {file:#?}"
    );
}

#[test]
fn a_family_declares_an_op_enum_one_signature_line_per_op_and_free_args() {
    let msg = folded();
    let file = leaf(&msg, "file");
    let params = &file["function"]["parameters"];
    // orch-7 (K-DISPATCHER M-1): `args` is required too -- a call `F{op}`
    // without it is not unfolded by the dispatcher and dead-letters as tool `F`.
    assert_eq!(params["required"], json!(["op", "args"]));
    assert_eq!(
        params["properties"]["args"]["type"], "object",
        "free args: {params:#?}"
    );
    assert!(
        params["properties"]["args"]["description"]
            .as_str()
            .is_some_and(|d| d.contains("`{}`")),
        "an op without parameters (help) passes `{{}}`: {params:#?}"
    );
    assert!(
        params["properties"]["args"].get("properties").is_none(),
        "no union of the ops' parameters: {params:#?}"
    );
    let text = file["function"]["description"]
        .as_str()
        .expect("description");
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("- ")).collect();
    assert_eq!(
        lines,
        vec![
            "- read(file, offset?): Describe a file -- path, kind, size.",
            "- write(file, text): Write a file.",
            "- help(op?, query?): the full schema of one op, or the op list.",
        ],
        "first sentence, no address phrase, `?` for the optional, sorted, help last: {text}"
    );
}

/// OR-WG-4 (GH #1260): the hint level keeps the FIRST sentence of a
/// description and nothing else (`tool_scope::first_sentence`). For a family
/// that sentence was "2 operations on `web`." -- a count and no names, so a
/// model at hint level could not know what to ask `help` about. The head's
/// first sentence names the ops now.
#[test]
fn the_first_sentence_of_a_family_names_its_ops() {
    let msg = folded();
    for (family, ops) in [("file", "read, write"), ("web", "fetch, search")] {
        let text = leaf(&msg, family)["function"]["description"]
            .as_str()
            .expect("description")
            .to_string();
        let first = text.lines().next().unwrap_or_default();
        let end = first.find(". ").map_or(first.len(), |i| i + 1);
        let sentence = &first[..end];
        assert!(
            sentence.contains(&format!("`{family}`: {ops}")) && sentence.contains("`help`"),
            "the first sentence names the ops and the way to their arguments: {sentence:?}"
        );
    }
}

#[test]
fn the_fold_ships_off_and_is_one_switch() {
    let flat = merge_with(&fold_rows(), json!({"tools": ["*"]}));
    assert!(
        names(&flat).contains("file_read") && names(&flat).contains("orga_find"),
        "off writes the flat menu: {flat:#?}"
    );
    assert_eq!(
        flat["header"]["menu_folded"], "",
        "present and empty: {flat:#?}"
    );
    assert_eq!(
        flat["header"]["menu_shadowed"], "",
        "present and empty: {flat:#?}"
    );
    assert_eq!(
        folded()["system"]["tools"].to_string(),
        folded()["system"]["tools"].to_string(),
        "the same rows give the same bytes"
    );
    let cfg = shipped("collector/assemble/config.json").expect("the template");
    assert_eq!(cfg["params"]["fold_tools"], "0");
    assert_eq!(cfg["contract"]["settings"]["fold_tools"]["default"], "0");
    assert_eq!(cfg["params"]["fold_families"], json!(["file_ws"]));
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"fold_tools\", \"0\")"),
        "the script's fallback"
    );
    assert!(
        src.contains("_list(\"fold_families\", [\"file_ws\"])"),
        "the script's fallback"
    );
}

// ═══════════════ 5. help and the error from the menu store (GH #1246)

/// A result on `in_tool` with the hop keys the dispatcher stamped on it.
fn result_in(hop: Value, id: &str, text: &str) -> Vec<Value> {
    let mut h = hop;
    h["route"] = json!("in_tool");
    run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": h, "context": {"turn_id": "t1", "session_id": "s1"}},
        "ttl": 64,
        "messages": [{"origin": "tool", "type": "tool_result", "id": id, "text": text}],
        "params": {"tools": ["*"], "fold_tools": "1"},
    })))
    .0
}

/// The round-check row a result is parked with: `(id, text)` of its turn.
fn parked(out: &[Value]) -> (String, String) {
    let park = out
        .iter()
        .find(|m| m["header"]["phase"] == "round-check")
        .unwrap_or_else(|| panic!("no round-check park: {out:#?}"));
    let row: Value =
        meclaw_core::serde_json::from_str(park["messages"][0]["text"].as_str().expect("op text"))
            .expect("op json");
    let turn: Value =
        meclaw_core::serde_json::from_str(row["row"]["turn"].as_str().expect("turn")).unwrap();
    (
        turn["id"].as_str().unwrap_or_default().to_string(),
        turn["text"].as_str().unwrap_or_default().to_string(),
    )
}

/// Both steps of a looked-up result: the arrival parks a `help` row and reads the
/// menu store, the store's reply (with `round` rows beside it) parks the answer.
fn looked_up(hop: Value, id: &str, text: &str, round: &[Value]) -> (String, String) {
    let first = result_in(hop, id, text);
    let ask = first
        .iter()
        .find(|m| m["header"]["phase"] == "help")
        .unwrap_or_else(|| panic!("no menu lookup: {first:#?}"));
    let legs: Vec<(String, Value)> = ask["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["id"].as_str().unwrap().to_string(),
                meclaw_core::serde_json::from_str(m["text"].as_str().unwrap()).unwrap(),
            )
        })
        .collect();
    let put = legs
        .iter()
        .find(|(_, a)| a["operation"] == "insert")
        .expect("the request is parked");
    let mut rows = vec![put.1["row"].clone()];
    rows.extend(round.iter().cloned());
    let reply = |cid: &str, rows: Value| json!({"origin": "tool", "type": "tool_result", "id": cid, "text": rows.to_string()});
    let mut msgs = vec![];
    let mut results = vec![];
    for (cid, a) in &legs {
        let r = if a["table"] == "menu" {
            json!(fold_rows())
        } else if a["operation"] == "select" {
            json!(rows)
        } else {
            Value::Null
        };
        msgs.push(reply(cid, r));
        results.push(json!({"tool_call_id": cid, "operation": a["operation"]}));
    }
    parked(
        &run(&code_stdin(&json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "cstore", "operation": "bundle"},
                       "context": {"col_phase": "help", "turn_id": "t1",
                                   "session_id": "s1", "iter": "0",
                                   "store_origin": "collector"}},
            "ttl": 64,
            "messages": msgs,
            "results": results,
            "params": {"tools": ["*"], "fold_tools": "1"},
        })))
        .0,
    )
}

fn help(request: Value) -> String {
    let (id, text) = looked_up(
        json!({"help_for": "file", "tool_call_id": "call-h"}),
        "call-h",
        &request.to_string(),
        &[],
    );
    assert_eq!(id, "call-h", "answered under the call's own id");
    text
}

/// T5-07: help is answered from the menu store -- the op list with its signature
/// lines, the full schema of one op, a keyword search over names and lines.
#[test]
fn help_answers_from_the_menu_store_the_list_one_schema_or_a_search() {
    let list = help(json!({}));
    assert!(
        list.contains("- read(file, offset?): Describe a file -- path, kind, size.")
            && list.contains("- write(file, text): Write a file."),
        "without `op` the op list: {list}"
    );
    assert!(
        list.starts_with("Operations on `file`: read, write;"),
        "the head of the op list names the ops, the same head the family carries \
         (GH #1260): {list}"
    );
    let one = help(json!({"op": "read"}));
    assert!(
        one.contains("More sentences follow.") && one.contains("\"offset\""),
        "with `op` the full description and the parameters: {one}"
    );
    let found = help(json!({"query": "kind"}));
    assert!(
        found.contains("read(file, offset?)") && !found.contains("write(file"),
        "the query searches names, signature lines and short descriptions: {found}"
    );
}

/// T5-08: the error of a FOLDED call carries the schema of its op -- only for
/// `bad_request`/`invalid_input`; any other error is parked as it came.
#[test]
fn the_error_of_a_folded_call_carries_the_schema_of_its_op() {
    let assistant = json!({"turn_id": "t1", "iter": 0, "role": "assistant",
        "turn": json!([{"origin": "assistant", "type": "tool_call", "id": "call-e",
            "text": json!({"name": "file",
                           "arguments": json!({"op": "read", "args": {}}).to_string()})
                .to_string()}])
        .to_string()});
    let (id, text) = looked_up(
        json!({"error_code": "invalid_input", "tool_call_id": "call-e"}),
        "call-e",
        "file_read: `file` is required",
        &[assistant],
    );
    assert_eq!(id, "call-e");
    assert!(
        text.starts_with("file_read: `file` is required")
            && text.contains("\"offset\"")
            && text.contains("read"),
        "the error, then the op's schema: {text}"
    );
    let plain = result_in(
        json!({"error_code": "timeout", "tool_call_id": "call-t"}),
        "call-t",
        "too slow",
    );
    assert_eq!(parked(&plain), ("call-t".into(), "too slow".into()));
}

/// GH #1260: a folded call whose op the family does not have unfolds to a name
/// no answerer carries (`web_lookup`) and comes back `unknown_tool`. The model
/// asked for an op of `web`, so the error names the ops `web` has -- the
/// regular error, looked up from the same menu store as help.
#[test]
fn an_unknown_op_of_a_folded_call_names_the_ops_of_its_family() {
    let assistant = json!({"turn_id": "t1", "iter": 0, "role": "assistant",
        "turn": json!([{"origin": "assistant", "type": "tool_call", "id": "call-u",
            "text": json!({"name": "web",
                           "arguments": json!({"op": "lookup", "args": {}}).to_string()})
                .to_string()}])
        .to_string()});
    let (id, text) = looked_up(
        json!({"error_code": "unknown_tool", "tool_call_id": "call-u"}),
        "call-u",
        "no tool named web_lookup is wired into this hive",
        &[assistant],
    );
    assert_eq!(id, "call-u");
    assert!(
        text.starts_with("no tool named web_lookup is wired into this hive")
            && text.contains("`web` has no op `lookup`; its ops: fetch, search."),
        "the error, then the ops of the family: {text}"
    );
}

/// T5-08, review I-1: an error text that is a JSON object (the file tools'
/// refusal `{"ok": false, "error": {"code", ...}}`) stays one -- the schema goes
/// in as the field `op_schema`, so the curator's `tool_failure` still reads the
/// failure and its code off the parsed text (GH #925 `tool_error`).
#[test]
fn a_json_error_of_a_folded_call_takes_the_schema_as_a_field() {
    let assistant = json!({"turn_id": "t1", "iter": 0, "role": "assistant",
        "turn": json!([{"origin": "assistant", "type": "tool_call", "id": "call-e",
            "text": json!({"name": "file",
                           "arguments": json!({"op": "read", "args": {}}).to_string()})
                .to_string()}])
        .to_string()});
    let refusal = json!({"ok": false, "op": "read",
        "error": {"code": "bad_request", "message": "`file` is required"}});
    let (id, text) = looked_up(
        json!({"error_code": "bad_request", "tool_call_id": "call-e"}),
        "call-e",
        &refusal.to_string(),
        &[assistant],
    );
    assert_eq!(id, "call-e");
    let body: Value = meclaw_core::serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("the error stays a JSON object ({e}): {text}"));
    assert_eq!(
        (&body["ok"], &body["error"]["code"]),
        (&json!(false), &json!("bad_request")),
        "the failure and its code stay readable: {text}"
    );
    assert!(
        body["op_schema"]["op"] == "read"
            && body["op_schema"]["args"]["properties"]
                .get("offset")
                .is_some(),
        "the op's schema as a field: {text}"
    );
}
