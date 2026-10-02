//! GH #948 -- bounds are refused.
//!
//! Both new requests are refused whole, before the store is asked, when they
//! are out of form: `in_alias` with 0 or more than 32 entries, an empty alias
//! (also one that is empty in the store's normal form), an alias longer than
//! 200 characters, an id that is not `<prefix>-<rest>` in lowercase ASCII or
//! longer than 64 characters, an unknown key, a `force` that is not a boolean,
//! a tag longer than 64 characters; `in_query {subject}` with an empty or
//! over-long subject or a `limit` outside 1..20. A refusal names the place
//! (`error_key` on `alias_ack`, the sentence on `reject`) and writes nothing.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

#[test]
fn gh948_an_alias_request_is_parsed_whole_or_refused_with_its_place() {
    if !shipped() {
        return;
    }
    let program = r#"
ok = {"alias": "Firewall X", "canonical": "ob-1a2b3c4d5e6f"}
cases = [
  {"aliases": [ok]},
  {"aliases": [ok] * 32},
  {"aliases": []},
  {"aliases": [ok] * 33},
  {"aliases": "Firewall X"},
  {},
  {"aliases": [ok], "force": "yes"},
  {"aliases": [ok], "force": True},
  {"aliases": [ok, "x"]},
  {"aliases": [{"alias": "", "canonical": "ob-1"}]},
  {"aliases": [{"alias": "   ", "canonical": "ob-1"}]},
  {"aliases": [{"alias": "a" * 200, "canonical": "ob-1"}]},
  {"aliases": [{"alias": "a" * 201, "canonical": "ob-1"}]},
  {"aliases": [{"alias": "x", "canonical": "ob 1"}]},
  {"aliases": [{"alias": "x", "canonical": "ob1"}]},
  {"aliases": [{"alias": "x", "canonical": "OB-1"}]},
  {"aliases": [{"alias": "x", "canonical": "ob-1\n"}]},
  {"aliases": [{"alias": "x", "canonical": "ob-" + "a" * 61}]},
  {"aliases": [{"alias": "x", "canonical": "ob-" + "a" * 62}]},
  {"aliases": [{"alias": "x"}]},
  {"aliases": [{"alias": 7, "canonical": "ob-1"}]},
  {"aliases": [ok, {"alias": "x", "canonical": "ob-1", "note": "smuggled"}]},
]
out = []
for c in cases:
    entries, force, bad = parse_aliases(c)
    out.append(bad if bad else "ok")
print(json.dumps(out))
"#;
    assert_eq!(
        probe(WRITER, program),
        concat!(
            r#"["ok", "ok", "aliases", "aliases", "aliases", "aliases", "force", "ok", "#,
            r#""aliases[1]", "aliases[0].alias", "aliases[0].alias", "ok", "aliases[0].alias", "#,
            r#""aliases[0].canonical", "aliases[0].canonical", "aliases[0].canonical", "#,
            r#""aliases[0].canonical", "ok", "aliases[0].canonical", "aliases[0].canonical", "#,
            r#""aliases[0].alias", "aliases[1].note"]"#
        )
    );
}

fn door(body: Value, tag: &str) -> Vec<Value> {
    let mut flat = body;
    flat["messages"] = json!([]);
    flat["header"] = json!({"context": {"mem_phase": "alias", "store_origin": "alias"},
                            "hop": {"route": "in_alias", "alias_tag": tag}});
    meclaw_testing::emit_all(&meclaw_testing::shipped_script(WRITER), &flat)
}

#[test]
fn gh948_a_refused_alias_request_writes_nothing_and_says_where() {
    if !shipped() {
        return;
    }
    let mut all = Vec::new();
    for (body, tag, key) in [
        (json!({"aliases": []}), "t", "aliases"),
        (
            json!({"aliases": [{"alias": "x", "canonical": "ob 1"}]}),
            "t",
            "aliases[0].canonical",
        ),
        (
            json!({"aliases": [{"alias": "x", "canonical": "ob-1"}], "force": 1}),
            "t",
            "force",
        ),
        (
            json!({"aliases": [{"alias": "x", "canonical": "ob-1"}]}),
            &*"t".repeat(65),
            "alias_tag",
        ),
    ] {
        let out = door(body, tag);
        assert_eq!(out.len(), 1, "{out:?}");
        let h = &out[0]["header"];
        assert_eq!(
            h["route"], "alias_ack",
            "no store op before the form is right"
        );
        assert_eq!(h["error_code"], "invalid_input");
        assert_eq!(h["error_key"], key);
        assert_eq!(out[0]["done"], json!(0));
        assert_eq!(out[0]["refused"], json!([]));
        all.extend(out);
    }
    assert_the_declaration_admits(WRITER, &all);
}

fn ask(body: Value, round: Option<&str>) -> Vec<Value> {
    let mut ctx = json!({"recall_caller": "objects"});
    if let Some(r) = round {
        ctx["audience_now"] = json!(r);
    }
    let mut flat = body;
    flat["messages"] = json!([]);
    flat["header"] = json!({"context": ctx, "hop": {"route": "in_query", "phase": "recall"}});
    meclaw_testing::emit_all(&meclaw_testing::shipped_script(RECALL), &flat)
}

#[test]
fn gh948_a_subject_question_out_of_form_is_refused() {
    if !shipped() {
        return;
    }
    let mut all = Vec::new();
    for (body, round, reason, place) in [
        (json!({"subject": "ob-1"}), None, "missing_audience", ""),
        (json!({"subject": "ob-1"}), Some(""), "missing_audience", ""),
        (
            json!({"subject": "ob-1"}),
            Some("not json"),
            "missing_audience",
            "",
        ),
        (
            json!({"subject": ""}),
            Some(ROUND_EA),
            "invalid_input",
            "subject",
        ),
        (
            json!({"subject": "   "}),
            Some(ROUND_EA),
            "invalid_input",
            "subject",
        ),
        (
            json!({"subject": 7}),
            Some(ROUND_EA),
            "invalid_input",
            "subject",
        ),
        (
            json!({"subject": "a".repeat(201)}),
            Some(ROUND_EA),
            "invalid_input",
            "subject",
        ),
        (
            json!({"subject": "ob-1", "limit": 0}),
            Some(ROUND_EA),
            "invalid_input",
            "limit",
        ),
        (
            json!({"subject": "ob-1", "limit": 21}),
            Some(ROUND_EA),
            "invalid_input",
            "limit",
        ),
        (
            json!({"subject": "ob-1", "limit": true}),
            Some(ROUND_EA),
            "invalid_input",
            "limit",
        ),
        (
            json!({"subject": "ob-1", "limit": "5"}),
            Some(ROUND_EA),
            "invalid_input",
            "limit",
        ),
    ] {
        let out = ask(body.clone(), round);
        assert_eq!(out.len(), 1, "{body}: {out:?}");
        let h = &out[0]["header"];
        assert_eq!(h["route"], "reject", "{body}: refused before the store");
        assert_eq!(h["reject_reason"], reason, "{body}");
        let text = out[0]["messages"][0]["text"].as_str().unwrap_or_default();
        assert!(
            text.contains(place),
            "{body}: the sentence names `{place}`: {text}"
        );
        all.extend(out);
    }
    // The edges of the range are taken.
    for limit in [1, 20] {
        let out = ask(json!({"subject": "ob-1", "limit": limit}), Some(ROUND_EA));
        assert_eq!(out[0]["header"]["route"], "rsubj", "limit {limit}");
        all.extend(out);
    }
    let out = ask(json!({"subject": "a".repeat(200)}), Some(ROUND_EA));
    assert_eq!(out[0]["header"]["route"], "rsubj");
    all.extend(out);
    assert_the_declaration_admits(RECALL, &all);
}
