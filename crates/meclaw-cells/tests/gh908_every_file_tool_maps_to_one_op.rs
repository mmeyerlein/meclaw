//! GH #908: the file tools on a model's menu, the file space's half. The shipped
//! `templates/file-space/schemas` and `templates/file-space/tools` programs,
//! their pure halves loaded by `ast` (`support/file_space_hive.rs` `pure`), and
//! the whole space in one process for the calls (the shipped edges evaluated by
//! the colony's CEL, the store behind its own dispatcher).
//!
//! 1. **One list, one op per name.** `FILE_OFFER` holds 30 tools, every name is
//!    `file_<op>` of an op the space's own cells serve (`./read` `OPS`,
//!    `./write` `WRITE_OPS`, `./ws` `OPS`), the lane follows from the name, and
//!    `raw`, `ws_tree` and the projection ops are on no menu (OR-FJ-G4).
//! 2. **Two copies, one value.** `./schemas` hands the list out and `./tools`
//!    checks every call against it; a code cell shares no library, so the two
//!    literals are held equal here.
//! 3. **Every schema is a JSON Schema of the checked subset**, and every
//!    description names the address form and the `version` token a write
//!    passes back as `base`.
//! 4. **An argument is typed before a request leaves** (OR-FJ-63, B1-PP-12):
//!    `limit: "ten"` is answered `bad_request` and no cell of the space runs.
//! 5. **Reads through the tools, writes only for the core**: a read call comes
//!    back as ONE `tool_result` under its call id with the version token; a
//!    write without `context.tool_caller == 'cogny'` is `read_only` and asks
//!    nothing; the core opens a workspace, replaces with `base` and commits,
//!    and `file_history` shows the commit.
//! 6. **Every edge out of the space clears the context keys the space sets**
//!    (OR-FJ-61 (a), gh494).

#[path = "support/file_space_hive.rs"]
mod file_space_hive;

use file_space_hive::*;
use meclaw_core::serde_json::{Value, json};
use std::collections::BTreeSet;

const F: &str = "fh-0a0b0c0d0e0f";
const TEXT: &str = "alpha\nbeta\ngamma\n";

/// `./derive` as a recorder (the pattern of gh902): one stderr line per message
/// that reaches it over an edge, and nothing emitted -- `./derive`'s own work is
/// GH #903's and calls a model and an embedder.
const DERIVE_RECORDER: &str = "import sys, json\n\
doc = json.load(sys.stdin)\n\
b = doc.get('body') or {}\n\
h = doc['envelope']['header']['hop']\n\
sys.stderr.write('%s %s %s\\n' % (h.get('route'), h.get('op', ''), h.get('caller', '')))\n\
sys.stdout.write('[]')\n";

fn space() -> Space {
    Space::with(
        "/m/files",
        &[("derive", "script_inline", json!(DERIVE_RECORDER))],
    )
}

const READS: [&str; 12] = [
    "file_info",
    "file_read",
    "file_search",
    "file_summary",
    "file_ask",
    "file_history",
    "file_show",
    "file_diff",
    "file_list",
    "file_find",
    "file_outline",
    "file_links",
];

#[test]
fn every_file_tool_maps_to_one_op_the_space_serves() {
    if !shipped() {
        return;
    }
    let map = pure(
        "tools",
        "[[t['name']] + list(op_of(t['name'])) for t in FILE_OFFER]",
        json!(null),
    );
    let read_ops: BTreeSet<String> = strings(&pure("read", "list(OPS)", json!(null)))
        .into_iter()
        .collect();
    let write_ops: BTreeSet<String> = strings(&pure("write", "list(WRITE_OPS)", json!(null)))
        .into_iter()
        .collect();
    let ws_ops: BTreeSet<String> = strings(&pure("ws", "list(OPS)", json!(null)))
        .into_iter()
        .collect();
    let rows = map.as_array().unwrap();
    assert_eq!(rows.len(), 30, "30 file tools: {map}");
    let mut seen = BTreeSet::new();
    let (mut r, mut w, mut s) = (Vec::new(), 0, 0);
    for row in rows {
        let name = row[0].as_str().unwrap();
        let op = row[1]
            .as_str()
            .unwrap_or_else(|| panic!("{name} maps to no op"));
        let lane = row[2].as_str().unwrap();
        assert!(seen.insert(name.to_string()), "{name} twice");
        assert_eq!(name, format!("file_{op}"), "the name IS the mapping");
        match lane {
            "in_read" => {
                assert!(
                    read_ops.contains(op) || op == "ask",
                    "{op} is no op of ./read"
                );
                r.push(name.to_string());
            }
            "in_write" => {
                assert!(write_ops.contains(op), "{op} is no op of ./write");
                w += 1;
            }
            "in_ws" => {
                assert!(ws_ops.contains(op), "{op} is no op of ./ws");
                s += 1;
            }
            other => panic!("{name}: lane {other}"),
        }
        for banned in ["raw", "ws_tree", "ws_materialize", "ws_exec", "ws_adopt"] {
            assert_ne!(op, banned, "{banned} is on no menu (OR-FJ-G4)");
        }
    }
    assert_eq!(r, READS.to_vec(), "the twelve reads, in menu order");
    assert_eq!((w, s), (11, 7), "eleven writes, seven workspace ops");
    // `ask` is the one read `./read` does not serve: `./derive` does (B1 E).
    assert!(!read_ops.contains("ask"));
}

#[test]
fn the_menu_and_the_checker_hold_one_list() {
    if !shipped() {
        return;
    }
    let a = pure("schemas", "FILE_OFFER", json!(null));
    let b = pure("tools", "FILE_OFFER", json!(null));
    assert_eq!(a, b, "./schemas and ./tools carry FILE_OFFER word for word");
}

#[test]
fn every_schema_is_a_checked_json_schema_and_says_address_and_token() {
    if !shipped() {
        return;
    }
    let offer = pure("tools", "FILE_OFFER", json!(null));
    let kinds: BTreeSet<&str> = ["string", "integer", "boolean", "array"].into();
    for t in offer.as_array().unwrap() {
        let name = t["name"].as_str().unwrap();
        let d = t["description"].as_str().unwrap();
        assert!(
            d.contains("fh-<12 hex>"),
            "{name}: the description names the address form: {d}"
        );
        assert!(
            d.contains("`version`") || d.contains("`version` token"),
            "{name}: the description names the version token: {d}"
        );
        assert_eq!(
            d.trim_end_matches('.').matches(". ").count(),
            0,
            "{name}: one sentence: {d}"
        );
        let p = &t["parameters"];
        assert_eq!(p["type"], json!("object"), "{name}");
        assert_eq!(p["additionalProperties"], json!(false), "{name}");
        let props = p["properties"].as_object().unwrap();
        for req in strings(&p["required"]) {
            assert!(
                props.contains_key(&req),
                "{name}: required {req} undeclared"
            );
        }
        for (k, s) in props {
            let ty = &s["type"];
            let ok = match ty {
                Value::String(x) => kinds.contains(x.as_str()),
                Value::Array(xs) => xs.iter().all(|x| kinds.contains(x.as_str().unwrap())),
                _ => false,
            };
            assert!(ok, "{name}.{k}: a checked type: {s}");
        }
        let needs_base = !matches!(name, "file_create" | "file_snapshot")
            && strings(&pure("tools", "list(WRITE_OPS)", json!(null)))
                .contains(&name["file_".len()..].to_string());
        assert_eq!(
            strings(&p["required"]).contains(&"base".to_string()),
            needs_base,
            "{name}: `base` is required exactly on the ops that move content"
        );
    }
}

#[test]
fn the_checker_types_every_argument() {
    if !shipped() {
        return;
    }
    let cases = json!([
        ["file_history", {"file": F, "limit": "ten"}, ["arguments.limit must be integer"]],
        ["file_history", {"file": F, "limit": 10}, []],
        ["file_history", {"file": F, "limit": true}, ["arguments.limit must be integer"]],
        ["file_history", {"file": F, "limit": 900}, ["arguments.limit must be at most 200"]],
        ["file_read", {"from": 1}, ["arguments.file is required"]],
        ["file_read", {"file": F, "bogus": 1}, ["arguments.bogus is not an argument of this tool"]],
        ["file_search", {"file": F, "pattern": "x", "mode": "fuzzy"},
         ["arguments.mode must be one of exact, regex, semantic"]],
        ["file_replace", {"file": F, "base": "ab", "old": "x", "new": "y", "expected": "all"}, []],
        ["file_replace", {"file": F, "base": "ab", "old": "x", "new": "y", "expected": 2}, []],
        ["file_replace_lines", {"file": F, "base": "ab", "from": 1, "to": 1,
                                "hashes": ["abcd", 7], "new": "z"},
         ["arguments.hashes[1] must be string"]]
    ]);
    let got = pure(
        "tools",
        "[check([t for t in FILE_OFFER if t['name'] == n][0]['parameters'], a) \
         for n, a, _ in ARGS]",
        cases.clone(),
    );
    for (i, c) in cases.as_array().unwrap().iter().enumerate() {
        assert_eq!(got[i], c[2], "{}: {}", c[0], c[1]);
    }
}

/// One tool call on the space's `in_tool` lane; the ONE `tool_result` that left
/// the hive path for it: (hop, the parsed result text).
fn call(s: &mut Space, caller: &str, name: &str, id: &str, args: Value) -> (Value, Value) {
    let before = s.out.len();
    s.lane(
        "in_tool",
        json!({"tool_caller": caller, "cur_origin": "stale", "cur_phase": "stale"}),
        json!({"tool_name": name, "tool_call_id": id}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": id,
                             "text": args.to_string()}]}),
    );
    let mine: Vec<Msg> = s.out[before..].to_vec();
    assert_eq!(
        mine.len(),
        1,
        "{name}: exactly one message leaves, got {mine:?}; stderr {:?}",
        s.stderr
    );
    let m = &mine[0];
    assert_eq!(m.route(), "tool_result", "{name}: {m:?}");
    assert_eq!(
        m.hop["tool_call_id"],
        json!(id),
        "{name}: under the call's id"
    );
    for k in ["cur_origin", "cur_phase", "cur_call", "cur_job"] {
        assert!(
            !m.context.contains_key(k),
            "{name}: `{k}` leaves the space (OR-FJ-61 a): {:?}",
            m.context
        );
    }
    assert_eq!(
        m.context.get("tool_caller"),
        Some(&json!(caller)),
        "the assistant's stamp survives the space: it finds the surface on the way back"
    );
    let msgs = m.messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0]["id"], json!(id));
    let text = msgs[0]["text"].as_str().unwrap();
    let v: Value = meclaw_core::serde_json::from_str(text).expect("the result is JSON");
    assert!(v.get("op_id").is_none() && v.get("caller").is_none(), "{v}");
    (Value::Object(m.hop.clone()), v)
}

fn cells_run(s: &Space, cell: &str) -> usize {
    s.store_ops.iter().filter(|(c, _)| c == cell).count()
}

#[test]
fn a_mistyped_argument_is_refused_before_any_cell_runs() {
    if !shipped() {
        return;
    }
    let mut s = space();
    s.seed_text(F, "/notes/a.md", &[TEXT], &[]);
    let (hop, v) = call(
        &mut s,
        "talky",
        "file_history",
        "c-ten",
        json!({"file": F, "limit": "ten"}),
    );
    assert_eq!(v["error"]["code"], json!("bad_request"), "{v}");
    assert_eq!(hop["error_code"], json!("bad_request"));
    assert_eq!(cells_run(&s, "read"), 0, "no request reached ./read");
    assert!(s.stderr.is_empty(), "{:?}", s.stderr);
}

#[test]
fn a_surface_reads_through_its_tools_and_never_writes() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let v1 = s.seed_text(F, "/notes/a.md", &[TEXT], &[]).remove(0);
    s.seed_summary(F, &v1, "oneline", "three greek letters");
    let (_, sum) = call(&mut s, "talky", "file_summary", "c-1", json!({"file": F}));
    assert_eq!(sum["ok"], json!(true), "{sum}");
    assert_eq!(sum["text"], json!("three greek letters"));
    assert_eq!(
        sum["version"],
        json!(&v1[..12]),
        "the token rides the result"
    );
    let (_, rd) = call(
        &mut s,
        "talky-chat",
        "file_read",
        "c-2",
        json!({"file": "/notes/a.md", "from": 2}),
    );
    assert_eq!(rd["file"], json!(F), "a path answers with the id");
    assert_eq!(rd["version"], json!(&v1[..12]));
    assert!(rd["text"].as_str().unwrap().starts_with("2:"), "{rd}");

    let runs = cells_run(&s, "write");
    let (_, w) = call(
        &mut s,
        "talky",
        "file_replace",
        "c-3",
        json!({"file": F, "base": &v1[..12], "old": "beta", "new": "BETA"}),
    );
    assert_eq!(w["error"]["code"], json!("read_only"), "{w}");
    let (_, w) = call(&mut s, "", "file_ws_open", "c-4", json!({"name": "w1"}));
    assert_eq!(
        w["error"]["code"],
        json!("read_only"),
        "no stamp is no core: {w}"
    );
    assert_eq!(cells_run(&s, "write"), runs, "a refused write asks nothing");
    assert_eq!(cells_run(&s, "ws"), 0);

    // `ask` and a semantic search go where the hive's own `in_read` edge sends
    // them: `./derive`, under the caller `tools`.
    let before = s.stderr.len();
    s.lane(
        "in_tool",
        json!({"tool_caller": "talky"}),
        json!({"tool_name": "file_ask", "tool_call_id": "c-5"}),
        json!({"messages": [{"id": "c-5", "type": "tool_call",
                             "text": json!({"file": F, "question": "which?"}).to_string()}]}),
    );
    assert_eq!(
        s.stderr[before..],
        ["derive: in_read ask tools\n".to_string()]
    );
}

#[test]
fn the_core_writes_in_a_workspace_and_the_history_shows_the_commit() {
    if !shipped() {
        return;
    }
    let mut s = space();
    let v1 = s.seed_text(F, "/notes/a.md", &[TEXT], &[]).remove(0);
    let (_, o) = call(
        &mut s,
        "cogny",
        "file_ws_open",
        "c-1",
        json!({"name": "w1", "root": "/"}),
    );
    assert_eq!(o["ok"], json!(true), "{o}");
    let (_, r) = call(
        &mut s,
        "cogny",
        "file_replace",
        "c-2",
        json!({"file": F, "base": &v1[..12], "old": "beta", "new": "BETA", "ws": "w1"}),
    );
    assert_eq!(r["ok"], json!(true), "{r}");
    let (_, c) = call(
        &mut s,
        "cogny",
        "file_ws_commit",
        "c-3",
        json!({"ws": "w1", "note": "caps"}),
    );
    assert_eq!(c["ok"], json!(true), "{c}");
    let (_, h) = call(
        &mut s,
        "talky",
        "file_history",
        "c-4",
        json!({"file": F, "limit": 5}),
    );
    let top = &h["entries"][0];
    assert!(
        top["commit"].as_str().is_some_and(|x| !x.is_empty()),
        "the newest line is the commit: {h}"
    );
    assert_eq!(top["version"], r["version"], "{h}");
    let (_, rd) = call(&mut s, "talky", "file_read", "c-5", json!({"file": F}));
    assert!(rd["text"].as_str().unwrap().contains("|BETA"), "{rd}");
}

/// Review m-1 / m-6: a provider's call ids are unique only within one answer
/// (stubs and local servers repeat `call_0`), and B1 parks under (`op_id`,
/// cell) -- so the request a call becomes is correlated under the cell's own
/// prefix `tools:` (the form `git:` of the projection), and the answer finds
/// the call id again without it. A `file_*` name this space does not serve is
/// `tool_unknown`, the code the menu's `schemas` gives the same name.
#[test]
fn a_request_is_correlated_under_the_cells_prefix_and_an_unknown_name_is_tool_unknown() {
    if !shipped() {
        return;
    }
    let req = pure(
        "tools",
        "translate('file_read', json.dumps({'file': ARGS}), 'call_0', 'talky')",
        json!(F),
    );
    assert_eq!(req["header"]["op_id"], json!("tools:call_0"), "{req}");
    assert_eq!(req["header"]["caller"], json!("tools"), "{req}");

    let back = pure(
        "tools",
        "tool_result(call_of(ARGS), {'ok': True, 'op': 'read'})",
        json!("tools:call_0"),
    );
    assert_eq!(back["header"]["tool_call_id"], json!("call_0"), "{back}");
    assert_eq!(back["messages"][0]["id"], json!("call_0"), "{back}");

    let nope = pure(
        "tools",
        "translate('file_nope', '{}', 'c-9', 'cogny')",
        json!(null),
    );
    assert_eq!(
        nope["header"]["error_code"],
        json!("tool_unknown"),
        "{nope}"
    );
}
