//! GH #1332 -- one grant id serves several consumers, each with its own
//! requester, in the same access.
//!
//! Since #1329 a channel hive brings no access of its own, so the recipe seeds
//! the grants of the web voice (`agent:voice/channel`) and of the phone line
//! (`agent:freeswitch/voice`) into ONE `./access`, both under the shipped
//! handles `grant:deepgram@template-voice/stt` and
//! `grant:cartesia@template-voice/tts`. `access/invoke` read the grant with
//! `where {grant_id} limit 1` and compared the requester afterwards, so the
//! second consumer got the first one's row and was refused
//! `requester_mismatch`. The lookup is by `(grant_id, requester)` now.
//!
//! | claim | where |
//! |---|---|
//! | two consumers of one grant id each get a sealed box they can open | `gh1332_two_consumers_of_one_grant_id_each_get_their_box` |
//! | a foreign requester of that id is still refused `requester_mismatch` | same |
//! | an unknown id is still `grant_unknown` | same |
//! | the quota is the consumer's row's: one consumer's spends do not count against the other | `gh1332_the_quota_of_a_shared_grant_id_is_the_consumer_s` |
//! | the ack names its requester, so the answer reaches only the consumer that asked | both, via `hop.requester` |
//! | the expiry is the consumer's row's: the web voice's row ran out and was swept, the phone line is still served | `gh1332_one_consumer_s_expiry_does_not_end_the_id_for_the_other` |
//! | a revocation is the id's: every consumer of it is refused | `gh1332_a_revoked_grant_id_refuses_every_consumer` |
//! | a phone line beside the web voice in one member: both recipe-seeded consumers are served on their own answer edge | `gh1332_a_phone_line_beside_the_web_voice_both_deliveries_succeed` |

use super::access_template::{
    BROKER, VAULT_PASSPHRASE, body_of, boot, build_tree, hop_of, patch, probe, recv_route,
    seed_vault_secret, send_json, shipped_access, turn_json, vault_calls,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, shipped_script};

const RECIPES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/builder/recipes/config.json"
);
const MEMBER: &str = "/os/orgs/acme/members/alex";
const STT: &str = "grant:deepgram@template-voice/stt";
const WEB: &str = "agent:voice/channel";
const LINE: &str = "agent:freeswitch/voice";

fn grant_row(requester: &str, constraints: Value) -> Value {
    grant_row_until(requester, constraints, "2099-01-01T00:00:00.000000Z")
}

fn grant_row_until(requester: &str, constraints: Value, expires_at: &str) -> Value {
    json!({"operation": "insert", "table": "grants", "row": {
        "grant_id": STT, "requester": requester, "capability": "credential.read",
        "subject": "template:test", "scope": {"actions": ["vault.deliver"]},
        "cred_ref": "cred:deepgram", "purpose": "test", "issued_at": "2026-01-01T00:00:00.000000Z",
        "expires_at": expires_at, "rule_id": "test",
        "constraints": constraints}})
}

fn granted_event(id: &str, grant_id: &str) -> Value {
    grant_event(id, grant_id, "granted", "2026-01-01T00:00:00.000000Z")
}

fn grant_event(id: &str, grant_id: &str, event: &str, at: &str) -> Value {
    json!({"operation": "insert", "table": "grant_events", "row": {
        "id": id, "grant_id": grant_id, "event": event,
        "at": at, "actor": "operator",
        "reason_code": "", "detail": {}}})
}

fn spend(who: &str, grant: &str, key: &str) -> Value {
    json!({"who": who, "grant_id": grant, "operation": "vault.deliver",
           "payload": {"recipient_key": key}})
}

/// A tree whose hived vault holds `secrets` and opens itself from `env`.
async fn unlocked(td: &tempfile::TempDir, env: &str, secrets: &[(&str, &str)]) {
    let Some(root) = shipped_access() else {
        unreachable!("checked by the caller")
    };
    build_tree(td, &root);
    for (name, value) in secrets {
        seed_vault_secret(td, name, value, VAULT_PASSPHRASE).await;
    }
    // `set_var` is unsafe in edition 2024 (a concurrent `getenv` would be a
    // data race). Sound here because the name is unique to this test and the
    // cell that reads it does not exist yet.
    unsafe { std::env::set_var(env, VAULT_PASSPHRASE) };
    let env = env.to_string();
    patch(td.path(), "main/access/vault/config.json", move |v| {
        v["params"]["unlock_env"] = json!(env);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1332_two_consumers_of_one_grant_id_each_get_their_box() {
    const ENV: &str = "MECLAW_TEST_GH1332_TWO_CONSUMERS";
    if shipped_access().is_none() {
        return; // GH #49: the template library does not travel with every tree
    }
    let td = tempfile::TempDir::new().unwrap();
    unlocked(&td, ENV, &[("cred:deepgram", "DEEPGRAM-SECRET")]).await;
    let (h, mut rx) = boot(&td).await;

    for op in [
        grant_row(WEB, json!({})),
        grant_row(LINE, json!({})),
        granted_event("ev-web", STT),
        granted_event("ev-line", STT),
    ] {
        probe(&h, &mut rx, op).await;
    }

    for who in [WEB, LINE] {
        let me = meclaw_cells::sealed::RecipientKeypair::generate().expect("keypair");
        h.send(send_json("/spender", &spend(who, STT, &me.public_hex())))
            .await;
        let ack = recv_route(&mut rx, "ack").await;
        let payload = turn_json(&ack);
        assert_eq!(payload["outcome"], "ok", "{who} is served: {payload}");
        assert_eq!(hop_of(&ack, "operation"), "vault.deliver");
        assert_eq!(hop_of(&ack, "grant_id"), STT);
        assert_eq!(
            hop_of(&ack, "requester"),
            who,
            "the answer names the consumer that asked, so only its edge takes it"
        );
        let sealed = meclaw_cells::sealed::SealedBox::from_json(&body_of(&ack)["sealed"])
            .expect("the ack carries a sealed box");
        assert_eq!(
            me.open(&sealed).expect("its own box opens"),
            b"DEEPGRAM-SECRET".to_vec(),
            "{who}"
        );
    }

    for (who, grant, reason) in [
        ("agent:stranger", STT, "requester_mismatch"),
        (WEB, "grant:nobody@x/y", "grant_unknown"),
    ] {
        h.send(send_json("/spender", &spend(who, grant, &"aa".repeat(32))))
            .await;
        let ack = turn_json(&recv_route(&mut rx, "ack").await);
        assert_eq!(
            (ack["outcome"].as_str(), ack["reason_code"].as_str()),
            (Some("denied"), Some(reason)),
            "{who} on {grant}: {ack}"
        );
    }

    let usage = probe(
        &h,
        &mut rx,
        json!({"operation": "select", "table": "usage",
               "columns": ["requester", "outcome"],
               "where": {"grant_id": STT, "outcome": "ok"}}),
    )
    .await;
    h.quiesce().await;
    let calls = vault_calls(&h).await;
    h.shutdown().await;
    unsafe { std::env::remove_var(ENV) };

    let mut spenders: Vec<&str> = usage
        .as_array()
        .expect("usage rows")
        .iter()
        .filter_map(|r| r["requester"].as_str())
        .collect();
    spenders.sort();
    assert_eq!(
        spenders,
        vec![LINE, WEB],
        "every spend is booked on its consumer: {usage}"
    );
    let delivered = calls
        .iter()
        .filter(|(from, a)| from == BROKER && a["op"] == "deliver")
        .count();
    assert_eq!(
        delivered, 2,
        "the stranger never reached the vault: {calls:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1332_the_quota_of_a_shared_grant_id_is_the_consumer_s() {
    if shipped_access().is_none() {
        return; // GH #49
    }
    let td = tempfile::TempDir::new().unwrap();
    // The vault stays locked: the quota is decided before the vault is asked,
    // so a delivery reads as the vault's own refusal and a quota refusal as
    // `quota_exceeded`.
    build_tree(&td, &shipped_access().expect("checked"));
    let (h, mut rx) = boot(&td).await;
    for op in [
        grant_row(WEB, json!({"max_invocations": 1})),
        grant_row(LINE, json!({"max_invocations": 1})),
        granted_event("ev-web", STT),
    ] {
        probe(&h, &mut rx, op).await;
    }
    // The vault is locked, so a spend comes back as its refusal -- which is not
    // booked as an `ok` usage row. Book one ok spend for WEB by hand, the way
    // `invoke` books a delivery, and ask with both consumers.
    probe(
        &h,
        &mut rx,
        json!({"operation": "insert", "table": "usage", "row": {
            "id": "u-web", "grant_id": STT, "requester": WEB,
            "at": "2026-01-01T00:00:00.000000Z", "operation": "vault.deliver",
            "outcome": "ok"}}),
    )
    .await;
    let mut seen = Vec::new();
    for who in [WEB, LINE] {
        h.send(send_json("/spender", &spend(who, STT, &"aa".repeat(32))))
            .await;
        let ack = turn_json(&recv_route(&mut rx, "ack").await);
        seen.push((
            who,
            ack["reason_code"].as_str().unwrap_or_default().to_string(),
        ));
    }
    h.shutdown().await;
    assert_eq!(seen[0].1, "quota_exceeded", "WEB spent its one: {seen:?}");
    assert_ne!(
        seen[1].1, "quota_exceeded",
        "LINE's quota is its own row's, not WEB's spend: {seen:?}"
    );
}

/// The recipe's two renderings: the web voice and the phone line, one member.
fn render(params: Value) -> Vec<Value> {
    let out: Vec<Value> = emit_all(
        &shipped_script(RECIPES),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "…",
                                         "params": params}).to_string()}],
        }),
    )
    .into_iter()
    .filter(|m| m["header"]["operation"] == "recipe")
    .collect();
    let first = out.first().expect("the recipe answers").clone();
    assert!(first["header"]["error_code"].is_null(), "refused: {first}");
    first["manifest"].as_array().expect("a manifest").clone()
}

fn seeds(manifest: &[Value], table: &str) -> Vec<Value> {
    manifest
        .iter()
        .flat_map(|d| {
            d["diff"]["seed_rows"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|s| s["target"] == "./access/store" && s["table"] == table)
        .flat_map(|s| s["rows"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// The `in_sealed` guards of every answer edge out of the member's access.
fn answers(manifest: &[Value]) -> Vec<(String, String)> {
    manifest
        .iter()
        .flat_map(|d| {
            d["diff"]["add_edges"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter(|e| e["from"] == "./access" && e["modifier"]["set_hop"]["route"] == "'in_sealed'")
        .map(|e| {
            (
                e["to"].as_str().unwrap_or_default().to_string(),
                e["condition"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1332_a_phone_line_beside_the_web_voice_both_deliveries_succeed() {
    const ENV: &str = "MECLAW_TEST_GH1332_LINE_BESIDE_WEB";
    if shipped_access().is_none() || !std::path::Path::new(RECIPES).exists() {
        return; // GH #49
    }
    let web = render(json!({"scope": MEMBER, "level": "channel", "name": "voice",
                            "template": "voice", "assistant": "scribe",
                            "ctx": {"member_person": "alex"},
                            "credential": {"stt_cred_ref": "cred:deepgram",
                                           "tts_cred_ref": "cred:cartesia",
                                           "subject": "member:alex",
                                           "expires_at": "2099-01-01T00:00:00.000000Z"}}));
    let line = render(
        json!({"scope": MEMBER, "level": "channel", "name": "freeswitch",
                             "template": "freeswitch", "assistant": "scribe",
                             "bind_chat": "4711", "ctx": {"member_person": "alex"}}),
    );
    let grants: Vec<Value> = seeds(&web, "grants")
        .into_iter()
        .chain(seeds(&line, "grants"))
        .collect();
    let events: Vec<Value> = seeds(&web, "grant_events")
        .into_iter()
        .chain(seeds(&line, "grant_events"))
        .collect();
    // The finding itself: both renderings seed the shipped stt and tts handles.
    let shared: Vec<String> = seeds(&web, "grants")
        .iter()
        .filter_map(|g| g["grant_id"].as_str().map(str::to_string))
        .filter(|id| seeds(&line, "grants").iter().any(|g| g["grant_id"] == *id))
        .collect();
    assert!(
        shared.contains(&STT.to_string()),
        "shared handles: {shared:?}"
    );

    let td = tempfile::TempDir::new().unwrap();
    unlocked(
        &td,
        ENV,
        &[
            ("cred:deepgram", "DEEPGRAM-SECRET"),
            ("cred:cartesia", "CARTESIA-SECRET"),
        ],
    )
    .await;
    let (h, mut rx) = boot(&td).await;
    for row in &grants {
        probe(
            &h,
            &mut rx,
            json!({"operation": "insert", "table": "grants", "row": row}),
        )
        .await;
    }
    for row in &events {
        probe(
            &h,
            &mut rx,
            json!({"operation": "insert", "table": "grant_events", "row": row}),
        )
        .await;
    }

    let edges: Vec<(String, String)> = answers(&web).into_iter().chain(answers(&line)).collect();
    for handle in &shared {
        let secret = if handle.contains("deepgram") {
            "DEEPGRAM-SECRET"
        } else {
            "CARTESIA-SECRET"
        };
        for g in grants.iter().filter(|g| g["grant_id"] == *handle) {
            let who = g["requester"].as_str().expect("requester");
            let me = meclaw_cells::sealed::RecipientKeypair::generate().expect("keypair");
            h.send(send_json("/spender", &spend(who, handle, &me.public_hex())))
                .await;
            let ack = recv_route(&mut rx, "ack").await;
            let payload = turn_json(&ack);
            assert_eq!(payload["outcome"], "ok", "{who} on {handle}: {payload}");
            let sealed = meclaw_cells::sealed::SealedBox::from_json(&body_of(&ack)["sealed"])
                .expect("a sealed box");
            assert_eq!(me.open(&sealed).expect("opens"), secret.as_bytes().to_vec());
            // Exactly one answer edge of the member takes this ack: the asking
            // consumer's. Its guard names the handle and the requester the hop
            // now carries; the other consumer's guard names another requester.
            assert_eq!(hop_of(&ack, "requester"), who);
            let takers: Vec<&String> = edges
                .iter()
                .filter(|(_, c)| {
                    c.contains(&format!("hop.grant_id == '{handle}'"))
                        && c.contains(&format!("hop.requester == '{who}'"))
                })
                .map(|(to, _)| to)
                .collect();
            let rivals = edges
                .iter()
                .filter(|(_, c)| {
                    c.contains(&format!("hop.grant_id == '{handle}'"))
                        && !c.contains(&format!("hop.requester == '{who}'"))
                })
                .count();
            assert_eq!(takers.len(), 1, "{who} on {handle}: {edges:?}");
            assert_eq!(
                rivals, 1,
                "the other consumer's edge stands, guarded: {edges:?}"
            );
        }
    }
    h.shutdown().await;
    unsafe { std::env::remove_var(ENV) };
}

/// Boots a tree with both consumers of `STT` -- the web voice's row ending at
/// `web_until` -- plus `events` on the id, then spends once as each consumer
/// and returns `(who, outcome, reason_code)` per consumer.
async fn spend_as_both(
    env: &'static str,
    web_until: &str,
    events: Vec<Value>,
) -> Vec<(String, String, String)> {
    let td = tempfile::TempDir::new().unwrap();
    unlocked(&td, env, &[("cred:deepgram", "DEEPGRAM-SECRET")]).await;
    let (h, mut rx) = boot(&td).await;
    let mut ops = vec![
        grant_row_until(WEB, json!({}), web_until),
        grant_row(LINE, json!({})),
    ];
    ops.extend(events);
    for op in ops {
        probe(&h, &mut rx, op).await;
    }
    let mut out = Vec::new();
    for who in [WEB, LINE] {
        let me = meclaw_cells::sealed::RecipientKeypair::generate().expect("keypair");
        h.send(send_json("/spender", &spend(who, STT, &me.public_hex())))
            .await;
        let ack = recv_route(&mut rx, "ack").await;
        assert_eq!(hop_of(&ack, "requester"), who, "the ack names its consumer");
        let p = turn_json(&ack);
        if p["outcome"] == "ok" {
            let sealed = meclaw_cells::sealed::SealedBox::from_json(&body_of(&ack)["sealed"])
                .expect("an ok ack carries a sealed box");
            assert_eq!(
                me.open(&sealed).expect("its own box opens"),
                b"DEEPGRAM-SECRET".to_vec(),
                "{who}"
            );
        }
        out.push((
            who.to_string(),
            p["outcome"].as_str().unwrap_or("").to_string(),
            p["reason_code"].as_str().unwrap_or("").to_string(),
        ));
    }
    h.shutdown().await;
    unsafe { std::env::remove_var(env) };
    out
}

/// Review FS3 I-1: the recipe seeds the web voice with the expiry the owner
/// chose and the phone line with 2099. `access/sweep` writes one `expired`
/// event per grant id when a row runs out; `invoke` read that event as the
/// state of the whole id, so the phone went silent with the web voice. The
/// TTL is the row's (phase `grant`), so the sweep's event is no refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1332_one_consumer_s_expiry_does_not_end_the_id_for_the_other() {
    if shipped_access().is_none() {
        return; // GH #49: the template library does not travel with every tree
    }
    let got = spend_as_both(
        "MECLAW_TEST_GH1332_EXPIRY_PER_ROW",
        "2026-01-02T00:00:00.000000Z",
        vec![
            granted_event("ev-granted", STT),
            grant_event("ev-swept", STT, "expired", "2026-01-02T00:00:01.000000Z"),
        ],
    )
    .await;
    assert_eq!(
        got,
        vec![
            (
                WEB.to_string(),
                "denied".to_string(),
                "grant_expired".to_string()
            ),
            (LINE.to_string(), "ok".to_string(), String::new()),
        ],
        "the web voice's row ran out, the phone line's did not"
    );
}

/// The ruling OR-agent-2.FS3.1 stands: a revocation ends the id for every
/// consumer of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1332_a_revoked_grant_id_refuses_every_consumer() {
    if shipped_access().is_none() {
        return; // GH #49: the template library does not travel with every tree
    }
    let got = spend_as_both(
        "MECLAW_TEST_GH1332_REVOKED_PER_ID",
        "2099-01-01T00:00:00.000000Z",
        vec![
            granted_event("ev-granted", STT),
            grant_event("ev-revoked", STT, "revoked", "2026-01-02T00:00:01.000000Z"),
        ],
    )
    .await;
    for (who, outcome, reason) in got {
        assert_eq!(
            (outcome.as_str(), reason.as_str()),
            ("denied", "grant_revoked"),
            "{who}"
        );
    }
}
