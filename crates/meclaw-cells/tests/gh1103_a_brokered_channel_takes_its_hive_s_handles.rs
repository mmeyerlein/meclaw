//! GH #1103 — the phone line's duplex key comes from the hive's own broker,
//! never from the colony's environment.
//!
//! `freeswitch` brings its own `./access`: it seeds the grants of its media
//! half and draws the edges. The recognizer and the synthesizer inherit their
//! handles from `voice`; the `duplex` block is null in `voice`, so a line that
//! runs one duplex provider named its key as `${OPENAI_API_KEY}` until
//! `freeswitch@2.4.2` shipped a third grant and the builder recipe learned to
//! put the hive's handle on every block a wish names (`BROKERED_CHANNELS`).
//!
//! | lock | claim |
//! |---|---|
//! | `gh1103_the_brokered_table_matches_the_hive_s_broker` | every row of `BROKERED_CHANNELS` is a grant the hive's `access` seeds for its media half, with its `in_sealed` edge, and every seeded grant is a row |
//! | `gh1103_a_recipe_grown_phone_line_names_the_hive_s_duplex_handle` | a channel wish with a duplex block gets the hive's handle there and an empty literal key; a foreign handle is refused |

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, shipped_script};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);

fn templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn read(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn read_jsonl(p: &Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a jsonl row"))
        .filter(|v: &Value| v.get("schema").is_none())
        .collect()
}

/// `BROKERED_CHANNELS`, read out of the recipe's source without running it.
fn table() -> Value {
    let script = read(Path::new(RECIPES))["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let start = script
        .find("BROKERED_CHANNELS = {")
        .expect("the recipe declares BROKERED_CHANNELS")
        + "BROKERED_CHANNELS = ".len();
    let end = script[start..]
        .find("}  # end BROKERED_CHANNELS")
        .expect("the table's end marker")
        + start
        + 1;
    meclaw_core::serde_json::from_str(&script[start..end])
        .unwrap_or_else(|e| panic!("BROKERED_CHANNELS is not JSON: {e}"))
}

const DUPLEX: &str = "grant:openai@template-freeswitch/duplex";

#[test]
fn gh1103_the_brokered_table_matches_the_hive_s_broker() {
    let table = table();
    let rows = table["freeswitch"].as_array().expect("freeswitch rows");
    let dir = templates().join("freeswitch");
    let seeded: BTreeSet<String> = read_jsonl(&dir.join("access/store/seed/grants.jsonl"))
        .iter()
        .filter(|g| g["requester"] == "agent:freeswitch/voice")
        .map(|g| g["grant_id"].as_str().expect("grant_id").to_string())
        .collect();
    let edges = read(&dir.join("config.json"))["params"]["graph"]["edges"].clone();
    let answered = |h: &str| {
        edges.as_array().expect("edges").iter().any(|e| {
            e["from"] == "./access"
                && e["to"] == "./voice"
                && e["modifier"]["set_hop"]["route"] == "'in_sealed'"
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("hop.grant_id == '{h}'")))
        })
    };
    let mut listed = BTreeSet::new();
    for r in rows {
        assert_eq!(r[0], "voice", "the media half is the one asker: {r}");
        let h = r[2].as_str().expect("handle");
        assert!(
            seeded.contains(h),
            "{h} is not seeded by freeswitch's access"
        );
        assert!(answered(h), "{h} has no in_sealed edge back to ./voice");
        listed.insert(h.to_string());
    }
    assert!(
        listed.contains(DUPLEX),
        "the duplex handle is a row: {listed:?}"
    );
    assert_eq!(
        listed, seeded,
        "every grant the hive seeds is a row and back"
    );
}

fn grow_line(voice: Value) -> Vec<Value> {
    let p = json!({"scope": "/os/orgs/acme/members/alex", "level": "channel",
                   "name": "freeswitch", "template": "freeswitch",
                   "assistant": "scribe", "bind_chat": "4711",
                   "ctx": {"member_person": "alex"},
                   "override_params": {"voice": voice}});
    emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": p}).to_string()}],
        }),
    )
    .into_iter()
    .filter(|m| m["header"]["operation"] == "recipe")
    .collect()
}

#[test]
fn gh1103_a_recipe_grown_phone_line_names_the_hive_s_duplex_handle() {
    let out = grow_line(json!({"stt": null, "tts": null,
                               "duplex": {"provider": "gpt_live", "sample_rate": 16000,
                                          "api_key": "${OPENAI_API_KEY}"}}));
    let first = out.first().expect("the recipe answers");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    let manifest = first["manifest"].as_array().expect("a manifest");
    let node = manifest
        .iter()
        .flat_map(|d| {
            d["diff"]["add_nodes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .find(|n| {
            n["template"]
                .as_str()
                .is_some_and(|t| t.starts_with("freeswitch"))
        })
        .expect("the line's node");
    let duplex = &node["override_params"]["voice"]["duplex"];
    assert_eq!(duplex["credential_grant_id"], DUPLEX, "{node}");
    assert_eq!(duplex["api_key"], "", "no key from the environment: {node}");
    assert_eq!(
        duplex["provider"], "gpt_live",
        "the rest of the block stays"
    );
    assert!(
        node["override_params"]["voice"]["stt"].is_null(),
        "a null block stays null"
    );
    assert!(
        !manifest
            .iter()
            .any(|d| d["diff"].get("seed_rows").is_some()),
        "the hive's own broker seeds the grant, the recipe seeds none"
    );

    let refused = grow_line(json!({"duplex": {"provider": "gpt_live",
                                              "credential_grant_id": "grant:other@x/y"}}));
    let first = refused.first().expect("the recipe answers");
    assert!(
        !first["header"]["error_code"].is_null(),
        "a foreign handle is refused: {first}"
    );
}
