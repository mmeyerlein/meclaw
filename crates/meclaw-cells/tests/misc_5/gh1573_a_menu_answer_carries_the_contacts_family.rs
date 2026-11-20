//! GH #1573 -- a menu answer may carry the CONTACTS of the member beside its
//! tools, and the collector writes them as the durable family `contacts`.
//!
//! WHY this file exists: on a member's own turn the fast voice was offered a
//! tool that addresses one of the member's contacts, but no window showed who
//! those contacts are -- `roster` lists the participants of the channel and
//! nothing else. Measured (d54g diagnosis of rollout R046, round 1): the tool
//! stood in the menu, `roster` was empty, no contact id stood anywhere in the
//! request, and the model guessed (a fetch of a local file) and told the person
//! it had no access. The menu answer is the declared source: whoever answers
//! the menu question for that tool knows the contacts it reaches, so the list
//! rides on the same lane as a third half beside `schemas[]` and `sidecar[]`.
//!
//! What this file pins, over the SHIPPED assembler:
//!
//! 1. An answer with `contacts[]` is recorded with its answerer, normalised
//!    (an entry without an id is dropped, a repeated id kept once).
//! 2. The merge writes `system.contacts` as one text leaf, one line per
//!    contact, the answerers in name order and the first one winning an id.
//! 3. An answer without the key revokes THIS answerer's contacts (durable
//!    state is revoked, never merely abandoned, GH #606): its row holds none,
//!    and the merge writes the family anew from the answerers still holding a
//!    list -- empty when none does. A window whose tree has no contact source
//!    at all reads the same prompt as before: an empty leaf contributes
//!    nothing to it (GH #259).
//! 4. `contacts: []` is a revocation: recorded, and the merge writes the
//!    family empty.
//! 5. The block names no product, role or channel (curator omni, R-34-6).

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{code_stdin, run_shipped_script, shipped_script};

const ASSEMBLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);

fn present() -> bool {
    std::path::Path::new(ASSEMBLE).exists()
}

fn run(doc: &Value) -> Vec<Value> {
    let out = run_shipped_script(&shipped_script(ASSEMBLE), &doc.to_string());
    assert!(out.status.success(), "the assembler exited non-zero");
    let v: Value = meclaw_core::serde_json::from_slice(&out.stdout).expect("json out");
    match v {
        Value::Array(a) => a,
        other => vec![other],
    }
}

fn decl(name: &str) -> Value {
    json!({"name": name, "description": format!("what {name} does"),
           "parameters": {"type": "object", "properties": {}}})
}

/// One answer on `in_menu`; `contacts` None leaves the key out.
fn record(answerer: &str, schemas: &[Value], contacts: Option<Value>) -> Vec<Value> {
    let mut body = json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "in_menu"},
                   "context": {"tool_answerer": answerer}},
        "ttl": 64,
        "messages": [],
        "schemas": schemas,
        "unknown": [],
        "params": {"tools": ["a_tool"]},
    });
    if let Some(c) = contacts {
        body["contacts"] = c;
    }
    run(&code_stdin(&body))
}

/// The row the answer's bundle inserts.
fn row_of(recorded: &[Value]) -> Value {
    assert_eq!(recorded.len(), 1, "one bundle: {recorded:#?}");
    let leg = recorded[0]["messages"]
        .as_array()
        .expect("a bundle of tool_calls")
        .iter()
        .find(|m| m["id"] == json!("c-menu-put"))
        .expect("the bundle inserts this answerer's row")
        .clone();
    let args: Value =
        meclaw_core::serde_json::from_str(leg["text"].as_str().expect("a tool_call text"))
            .expect("the op is json");
    args["row"].clone()
}

/// The columns the merge reads back.
fn read_columns(recorded: &[Value]) -> Value {
    let leg = recorded[0]["messages"]
        .as_array()
        .expect("a bundle")
        .iter()
        .find(|m| m["id"] == json!("c-menu-all"))
        .expect("the bundle reads every row")
        .clone();
    let args: Value =
        meclaw_core::serde_json::from_str(leg["text"].as_str().expect("text")).expect("json");
    args["columns"].clone()
}

/// The merge over `rows`, as a store hands them back (json columns as text).
fn merge(rows: &[Value]) -> Vec<Value> {
    let stored: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut s = r.clone();
            for k in ["tools", "sidecar", "contacts"] {
                if let Some(v) = r.get(k) {
                    s[k] = if v.is_null() {
                        Value::Null
                    } else {
                        json!(v.to_string())
                    };
                }
            }
            s
        })
        .collect();
    run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [{"id": "c-menu-all", "type": "tool_result",
                      "text": meclaw_core::serde_json::to_string(&stored).unwrap()}],
        "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        "params": {"tools": ["a_tool"]},
    })))
}

const HEAD: &str = "Contacts (address a contact by its id):";

#[test]
fn gh1573_a_menu_answer_carries_the_contacts_family() {
    if !present() {
        return;
    }
    // 1. recorded, normalised.
    let book = record(
        "book",
        &[],
        Some(json!([
            {"id": "k-1", "name": "The workshop"},
            {"id": "k-2"},
            {"id": "", "name": "no id, dropped"},
            {"id": "k-1", "name": "a repeat, dropped"},
            "not an object"
        ])),
    );
    assert_eq!(
        row_of(&book)["contacts"],
        json!([{"id": "k-1", "name": "The workshop"}, {"id": "k-2", "name": ""}]),
        "an answer that only offers contacts is recorded under its answerer: {book:#?}"
    );
    assert!(
        read_columns(&book)
            .as_array()
            .is_some_and(|c| c.contains(&json!("contacts"))),
        "the merge reads the contacts column back: {book:#?}"
    );

    // 2. the merge writes the family; tools stand beside it untouched.
    let tools = record("tools", &[decl("a_tool")], None);
    let other = record(
        "zeta",
        &[],
        Some(json!([{"id": "k-1", "name": "loses to the first answerer"},
                    {"id": "k-3", "name": "A neighbour"}])),
    );
    let out = merge(&[row_of(&tools), row_of(&book), row_of(&other)]);
    assert_eq!(out.len(), 1, "one menu message: {out:#?}");
    assert_eq!(out[0]["header"]["route"], "menu");
    assert_eq!(
        out[0]["system"]["contacts"],
        json!({"text": format!("{HEAD}\nk-1 = The workshop\nk-2\nk-3 = A neighbour")}),
        "one line per contact, answerers in name order, the first one wins an id: {out:#?}"
    );
    assert!(
        out[0]["system"]["tools"].get("a_tool").is_some(),
        "the tools of the other answerer stand: {out:#?}"
    );

    // 3. without the key: this answerer's contacts are revoked.
    assert!(
        row_of(&tools)["contacts"].is_null(),
        "an answer without the key records no contacts: {tools:#?}"
    );
    // `book` held a list and answers again, now with a tool and no key.
    let silent = record("book", &[decl("b_tool")], None);
    assert!(row_of(&silent)["contacts"].is_null(), "{silent:#?}");
    let rest = merge(&[row_of(&tools), row_of(&silent), row_of(&other)]);
    assert_eq!(
        rest[0]["system"]["contacts"],
        json!({"text": format!("{HEAD}\nk-1 = loses to the first answerer\nk-3 = A neighbour")}),
        "the family is written anew from the answerers still holding a list: {rest:#?}"
    );
    let orphan = merge(&[row_of(&tools), row_of(&silent)]);
    assert_eq!(orphan.len(), 1, "{orphan:#?}");
    assert_eq!(
        orphan[0]["system"]["contacts"],
        json!({"text": ""}),
        "no answerer holds a list any more: the family is written empty, never left \
         standing (GH #606): {orphan:#?}"
    );
    let plain = merge(&[row_of(&tools)]);
    assert_eq!(plain.len(), 1, "{plain:#?}");
    assert_eq!(
        plain[0]["system"]["contacts"],
        json!({"text": ""}),
        "a tree without any contact source writes an empty leaf, which adds nothing \
         to the prompt: {plain:#?}"
    );
    assert!(
        plain[0]["system"]["tools"].get("a_tool").is_some(),
        "the menu stands as before: {plain:#?}"
    );

    // 4. an empty list revokes.
    let gone = record("book", &[], Some(json!([])));
    assert_eq!(row_of(&gone)["contacts"], json!([]), "{gone:#?}");
    let revoked = merge(&[row_of(&tools), row_of(&gone)]);
    assert_eq!(
        revoked[0]["system"]["contacts"],
        json!({"text": ""}),
        "an empty list writes the family empty, revoking the last one: {revoked:#?}"
    );
    let only = merge(&[row_of(&gone)]);
    assert_eq!(
        only.len(),
        1,
        "a merge with nothing but a revocation still writes it: {only:#?}"
    );
    assert_eq!(only[0]["system"]["contacts"], json!({"text": ""}));
    assert!(
        only[0]["system"].get("tools").is_none(),
        "and never an empty tool menu: {only:#?}"
    );
}

/// The block between the markers names no product, role or channel.
#[test]
fn gh1573_the_contacts_block_names_no_product() {
    if !present() {
        return;
    }
    let script = shipped_script(ASSEMBLE);
    let start = script
        .find("# >>> contacts v1")
        .expect("the contacts block opens with its marker");
    let end = script
        .find("# <<< contacts v1")
        .expect("the contacts block closes with its marker");
    assert!(start < end);
    let block = script[start..end].to_lowercase();
    for word in [
        "soul2me", "peer", "affinity", "talky", "cogny", "quick", "brain", "egon", "telegram",
        "phone", "voice", "workshop", "member",
    ] {
        assert!(
            !block.contains(word),
            "the contacts block of the collector names {word:?} -- the family is generic: \
             a source declares it, no product, role or channel is named in core"
        );
    }
}
