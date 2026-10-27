//! GH #1103, #1329 — the phone line's keys come from the member's access,
//! never from the colony's environment and never from a broker of its own.
//!
//! Up to `freeswitch@2.4.8` the hive brought its own `./access`: it seeded the
//! three grants of its media half and drew the edges. Since 3.0.0 (#1329) it
//! declares the credential lane at its rim (`credential_request` out of
//! `./voice`, `in_sealed` back into it, both `at` the connect point) and the
//! builder recipe, its consumer in the library, seeds the same three grants
//! into the member's `./access` and draws the two v-lanes (`SEALED_CHANNELS`).
//! The recognizer and the synthesizer inherit their handles from `voice`; the
//! `duplex` block is null in `voice`, so a wish that names one gets the hive's
//! handle and an empty literal key.
//!
//! | lock | claim |
//! |---|---|
//! | `gh1103_the_sealed_table_matches_the_hive_s_rim` | every row of `SEALED_CHANNELS` is a connect point of the hive's credential lane, the media half's shipped handles are rows, and the hive ships no broker |
//! | `gh1103_a_recipe_grown_phone_line_names_the_hive_s_duplex_handle` | a channel wish with a duplex block gets the hive's handle there and an empty literal key; a foreign handle is refused |
//! | `gh1329_a_recipe_grown_phone_line_carries_its_credential_road` | the line's declaration stands at the member and carries the three grants for its access and the two v-lanes at `./voice` |

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

fn shipped() -> bool {
    Path::new(RECIPES).is_file() && templates().join("freeswitch/template.json").is_file()
}

fn read(p: &Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// `SEALED_CHANNELS`, read out of the recipe's source without running it.
fn table() -> Value {
    let script = read(Path::new(RECIPES))["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string();
    let start = script
        .find("SEALED_CHANNELS = {")
        .expect("the recipe declares SEALED_CHANNELS")
        + "SEALED_CHANNELS = ".len();
    let end = script[start..]
        .find("}  # end SEALED_CHANNELS")
        .expect("the table's end marker")
        + start
        + 1;
    meclaw_core::serde_json::from_str(&script[start..end])
        .unwrap_or_else(|e| panic!("SEALED_CHANNELS is not JSON: {e}"))
}

const DUPLEX: &str = "grant:openai@template-freeswitch/duplex";
const MEMBER: &str = "/os/orgs/acme/members/alex";
const REQUESTER: &str = "agent:freeswitch/voice";

fn rim_at(root: &Value, list: &str, route: &str) -> BTreeSet<String> {
    root["params"]["contract"][list]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["route"] == route)
        .flat_map(|l| l["at"].as_array().cloned().unwrap_or_default())
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect()
}

#[test]
fn gh1103_the_sealed_table_matches_the_hive_s_rim() {
    if !shipped() {
        return; // GH #49
    }
    let table = table();
    let rows = table["freeswitch"].as_array().expect("freeswitch rows");
    let dir = templates().join("freeswitch");
    assert!(
        !dir.join("access").exists(),
        "freeswitch brings no broker of its own (#1329)"
    );
    let root = read(&dir.join("config.json"));
    let asks = rim_at(&root, "emits", "credential_request");
    let boxes = rim_at(&root, "accepts", "in_sealed");
    let voice = read(&templates().join("voice/config.json"));
    let mut listed = BTreeSet::new();
    for r in rows {
        let child = r[0].as_str().expect("child");
        assert_eq!(child, "voice", "the media half is the one asker: {r}");
        let point = format!("./{child}");
        assert!(asks.contains(&point), "{point} is no ask connect point");
        assert!(
            boxes.contains(&point),
            "{point} is no in_sealed connect point"
        );
        let block = r[1].as_str().expect("block");
        let h = r[2].as_str().expect("handle");
        let shipped = &voice["params"][block]["credential_grant_id"];
        if !shipped.is_null() {
            assert_eq!(shipped, h, "the row of {block} is the handle voice ships");
        }
        assert!(
            r[3].as_str().is_some_and(|c| c.starts_with("cred:")),
            "a row names its credential: {r}"
        );
        listed.insert(block.to_string());
    }
    assert_eq!(
        listed,
        ["duplex", "stt", "tts"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        "the three blocks of the media half are rows"
    );
    assert!(
        rows.iter().any(|r| r[2] == DUPLEX),
        "the duplex handle is a row"
    );
}

fn grow(voice: Option<Value>) -> Vec<Value> {
    let mut p = json!({"scope": MEMBER, "level": "channel",
                       "name": "freeswitch", "template": "freeswitch",
                       "assistant": "scribe", "bind_chat": "4711",
                       "ctx": {"member_person": "alex"}});
    if let Some(v) = voice {
        p["override_params"] = json!({"voice": v});
    }
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

fn manifest(out: &[Value]) -> Vec<Value> {
    let first = out.first().expect("the recipe answers");
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"].as_array().expect("a manifest").clone()
}

#[test]
fn gh1103_a_recipe_grown_phone_line_names_the_hive_s_duplex_handle() {
    if !shipped() {
        return; // GH #49
    }
    let out = grow(Some(json!({"stt": null, "tts": null,
                               "duplex": {"provider": "gpt_live", "sample_rate": 16000,
                                          "api_key": "${OPENAI_API_KEY}"}})));
    let manifest = manifest(&out);
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

    let refused = grow(Some(json!({"duplex": {"provider": "gpt_live",
                                              "credential_grant_id": "grant:other@x/y"}})));
    let first = refused.first().expect("the recipe answers");
    assert!(
        !first["header"]["error_code"].is_null(),
        "a foreign handle is refused: {first}"
    );
}

#[test]
fn gh1329_a_recipe_grown_phone_line_carries_its_credential_road() {
    if !shipped() {
        return; // GH #49
    }
    let manifest = manifest(&grow(None));
    assert_eq!(manifest.len(), 1, "one act (GH #567): {manifest:?}");
    let d = &manifest[0];
    assert_eq!(
        d["scope"], MEMBER,
        "the road reaches the member's ./access, so the declaration stands at the member"
    );
    let at = "./channels/freeswitch/voice";
    let edges = d["diff"]["add_edges"].as_array().expect("edges");
    let asks: Vec<&Value> = edges
        .iter()
        .filter(|e| e["lane"] == "credential_request")
        .collect();
    assert_eq!(asks.len(), 1, "one ask, the media half is one requester");
    assert_eq!(asks[0]["from"], at);
    assert_eq!(asks[0]["to"], "./access");
    assert_eq!(asks[0]["modifier"]["set_hop"]["route"], "'in_invoke'");
    assert_eq!(
        asks[0]["modifier"]["set_context"]["requester"],
        json!(format!("'{REQUESTER}'"))
    );
    let table = table();
    let handles: BTreeSet<String> = table["freeswitch"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r[2].as_str().expect("handle").to_string())
        .collect();
    for h in &handles {
        assert!(
            edges.iter().any(|e| e["lane"] == "in_sealed"
                && e["from"] == "./access"
                && e["to"] == at
                && e["modifier"]["set_hop"]["route"] == "'in_sealed'"
                && e["condition"].as_str().is_some_and(|c| {
                    c.contains("hop.operation == 'vault.deliver'")
                        && c.contains(&format!("hop.grant_id == '{h}'"))
                })),
            "no in_sealed v-lane ./access -> {at} addressed by {h}"
        );
    }
    let seeds = d["diff"]["seed_rows"].as_array().expect("seed_rows");
    let grants: Vec<Value> = seeds
        .iter()
        .filter(|s| s["target"] == "./access/store" && s["table"] == "grants")
        .flat_map(|s| s["rows"].as_array().cloned().unwrap_or_default())
        .collect();
    let seeded: BTreeSet<String> = grants
        .iter()
        .map(|g| g["grant_id"].as_str().expect("grant_id").to_string())
        .collect();
    assert_eq!(
        seeded, handles,
        "every handle is seeded into the member's access"
    );
    for g in &grants {
        assert_eq!(g["requester"], REQUESTER, "{g}");
        assert_eq!(g["capability"], "credential.read", "{g}");
        assert_eq!(g["scope"]["actions"], json!(["vault.deliver"]), "{g}");
    }
    let events: usize = seeds
        .iter()
        .filter(|s| s["target"] == "./access/store" && s["table"] == "grant_events")
        .map(|s| s["rows"].as_array().map_or(0, Vec::len))
        .sum();
    assert_eq!(events, handles.len(), "one granted event per grant");
}
