//! Warm answers like cold (GH #1197, generalised for GH #1276): the shipped
//! script of a `code` cell, spawned on the production path (`CodeCellFactory`)
//! once `warm` with ONE child and once `cold`, gets the same messages one after
//! the other, and warm's answers equal cold's. Message 2 runs in the
//! interpreter message 1 left behind, so a module global, a cache, a working
//! directory or a `sys.path` a run leaves behind shows up as a different answer
//! (the pattern of GH #1026).
//!
//! What a run makes fresh on purpose -- a `secrets` token, a `uuid4`, a file
//! id, the wall clock -- differs between ANY two runs, cold against cold as
//! well; the comparison masks exactly those shapes (`mask`) and drops the one
//! header field the cell itself adds from the wall clock (`duration_ms`),
//! nothing else. The trace id of probe n is the same in both modes, so a
//! script that echoes it answers alike.
#![allow(dead_code)]

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use std::sync::Arc;

/// The shipped template tree.
pub fn templates() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// The shipped `config.json` of `cell` (a path below `templates/`).
pub fn shipped(cell: &str) -> Value {
    let path = templates().join(cell).join("config.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path:?}: {e}"))
}

/// The shipped `config.json` of `cell`, its params overlaid with `overlay`,
/// as the colony boots it: the late-binding pass of instantiation
/// (`substitute_env_only`) over an empty environment, so every
/// `${NAME:-default}` token -- in a param or in the script itself -- is its
/// default (a script that reads `int("${X:-5}")` runs only so), and a token
/// without a default needs its param in the overlay, as an instance gives it.
pub fn booted(cell: &str, overlay: &Value) -> Value {
    let mut cfg = shipped(cell);
    for (k, v) in object(overlay) {
        cfg["params"][k] = v;
    }
    let env = std::collections::HashMap::new();
    meclaw_colony::mutation::substitute::substitute_env_only(&cfg, &env)
        .unwrap_or_else(|e| panic!("{cell}: the config binds with defaults: {e:?}"))
}

/// The `error_code`s the code cell writes itself when the run did not give an
/// answer (`crates/meclaw-cells/src/code/cell.rs`). A script's own answer may
/// carry an `error_code` -- a refusal cell (`tools/unknown`) or an error sink
/// (`talky/errors`) exists to say one -- and is compared like any answer.
pub const CELL_ERROR_CODES: [&str; 8] = [
    "invalid_input",
    "credential_pending",
    "io_error",
    "script_timeout",
    "script_failed",
    "invalid_json",
    "multi_send_not_declared",
    "contract_violation",
];

/// One message of a probe: its hop, its context and its body.
#[derive(Clone)]
pub struct Probe {
    pub hop: Value,
    pub context: Value,
    pub body: Value,
}

/// A probe without context.
pub fn probe(hop: Value, body: Value) -> Probe {
    Probe {
        hop,
        context: json!({}),
        body,
    }
}

fn object(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn is_uuid(s: &[u8]) -> bool {
    let is_hex = |c: &u8| c.is_ascii_digit() || (b'a'..=b'f').contains(c);
    s.len() >= 36
        && s[..36].iter().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                *c == b'-'
            } else {
                is_hex(c)
            }
        })
}

/// `text` with what a run makes fresh replaced: a lowercase `8-4-4-4-12` UUID
/// (a `uuid4`, a v7 id) becomes `<uuid>`, a run of 8 or more lowercase hex
/// digits (a `secrets` token, a file id, the `uuid4().hex[:8]` suffix of a
/// minted id, a second, micro- or millisecond stamp) `<hex>`, a `YYYY-MM-DD` date `<date>` and a `THH:MM:SS[.f]Z` right after it
/// `<time>`. Everything else stays, byte for byte.
pub fn mask(text: &str) -> String {
    let b = text.as_bytes();
    let is_date = |s: &[u8]| {
        s.len() >= 10
            && s[..4].iter().all(u8::is_ascii_digit)
            && s[4] == b'-'
            && s[5..7].iter().all(u8::is_ascii_digit)
            && s[7] == b'-'
            && s[8..10].iter().all(u8::is_ascii_digit)
    };
    let is_hex = |c: &u8| c.is_ascii_digit() || (b'a'..=b'f').contains(c);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if is_uuid(&b[i..]) {
            out.push_str("<uuid>");
            i += 36;
            continue;
        }
        if is_date(&b[i..]) {
            out.push_str("<date>");
            i += 10;
            if b.get(i) == Some(&b'T') {
                i += 1;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b':' || b[i] == b'.') {
                    i += 1;
                }
                if b.get(i) == Some(&b'Z') {
                    i += 1;
                }
                out.push_str("<time>");
            }
            continue;
        }
        let run = b[i..].iter().take_while(|c| is_hex(c)).count();
        if run >= 8 {
            out.push_str("<hex>");
            i += run;
        } else if run > 0 {
            out.push_str(&text[i..i + run]);
            i += run;
        } else {
            let ch = text[i..].chars().next().expect("i is a char boundary");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// Spawn `script_params` (a cell's shipped params) as `cell` in `mode`
/// (overlaid with `overlay`, one child), send `probes` one after the other and
/// return, per message, every emission up to its consume mark -- target and
/// body, masked, with `base` (a directory the overlay names) written as
/// `<base>`.
pub async fn answers(
    cell: &str,
    script_params: &Value,
    mode: &str,
    overlay: &Value,
    base: Option<&str>,
    probes: &[Probe],
) -> Vec<Vec<String>> {
    answers_under(
        cell,
        script_params,
        mode,
        overlay,
        base,
        probes,
        meclaw_colony::ContractView::default(),
    )
    .await
}

/// The contract the colony hands a cell born from `cfg`, as far as the code
/// cell reads it for its answers: `multi_send_capable` (a script that writes a
/// JSON array is refused without it, `multi_send_not_declared`).
pub fn contract_of(cfg: &Value) -> meclaw_colony::ContractView {
    meclaw_colony::ContractView {
        multi_send_capable: cfg["contract"]["multi_send_capable"] == json!(true),
        ..Default::default()
    }
}

/// [`answers`] under the contract `contract`.
pub async fn answers_under(
    cell: &str,
    script_params: &Value,
    mode: &str,
    overlay: &Value,
    base: Option<&str>,
    probes: &[Probe],
    contract: meclaw_colony::ContractView,
) -> Vec<Vec<String>> {
    let mut params = script_params.clone();
    for (k, v) in object(overlay) {
        params[k] = v;
    }
    params["runner_mode"] = json!(mode);
    // One child: message 2 runs in the interpreter message 1 left behind.
    params["max_concurrency"] = json!(1);
    let at = Path::new(&format!("/files/{}", cell.rsplit('/').next().unwrap()));
    let (otx, mut orx) = tokio::sync::mpsc::channel(64);
    let td = tempfile::TempDir::new().expect("tempdir");
    let (itx, _irx) = tokio::sync::mpsc::channel(8);
    let spawned = Arc::new(CodeCellFactory)
        .spawn_cell(
            at.clone(),
            params,
            otx,
            td.path().to_path_buf(),
            contract,
            itx,
            None,
            0,
            None,
            None,
            1000,
        )
        .unwrap_or_else(|e| panic!("{cell} {mode}: the shipped params spawn: {e:?}"));
    let tx = match spawned {
        SpawnedCellKind::Active { sender, .. } => sender,
        SpawnedCellKind::Dormant { .. } => unreachable!("code spawns Active"),
    };
    let mut all = Vec::with_capacity(probes.len());
    for (n, p) in probes.iter().enumerate() {
        let msg = MessageBuilder::new(at.clone())
            .trace_id(Uuid::from_u128(0x1276_0000 + n as u128))
            .hop(object(&p.hop))
            .context(object(&p.context))
            .body(Body::Inline(p.body.clone()))
            .reply_to(Path::new("/sink"))
            .build();
        let id = msg.id;
        tx.send(msg).await.expect("the cell takes the message");
        let mut got = Vec::new();
        loop {
            let em = tokio::time::timeout(
                meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG),
                orx.recv(),
            )
            .await
            .unwrap_or_else(|_| panic!("{cell} {mode} message {}: HANG, no consume mark", n + 1))
            .unwrap_or_else(|| panic!("{cell} {mode}: the cell ended"));
            if em.consumed_mark_id() == Some(id) {
                break;
            }
            let code = em.content["header"]["error_code"].as_str().unwrap_or("");
            assert!(
                !CELL_ERROR_CODES.contains(&code),
                "{cell} {mode} message {}: the script must run, not fail: {}",
                n + 1,
                em.content
            );
            // `header.duration_ms` is the cell's wall clock around the run,
            // not the script's answer (time is never a verdict, rule 11).
            let mut content = em.content.clone();
            if let Some(h) = content.get_mut("header").and_then(Value::as_object_mut) {
                h.remove("duration_ms");
            }
            let text = format!("{} {}", em.target.as_str(), content);
            let text = match base {
                Some(b) => text.replace(b, "<base>"),
                None => text,
            };
            got.push(mask(&text));
        }
        assert!(
            !got.is_empty(),
            "{cell} {mode} message {}: a probe that answers nothing proves nothing",
            n + 1
        );
        all.push(got);
    }
    all
}

/// The acceptance for one cell: the probes, warm on one child against cold,
/// the same answers. `overlay` gets the base directory of each mode (its own,
/// so neither run finds what the other wrote).
pub async fn warm_answers_like_cold(cell: &str, overlay: impl Fn(&str) -> Value, probes: &[Probe]) {
    warm_answers_like_cold_at(cell, overlay, |_| probes.to_vec()).await;
}

/// [`warm_answers_like_cold`] with probes that name the base directory of
/// each mode too (a run directory, a file below it).
pub async fn warm_answers_like_cold_at(
    cell: &str,
    overlay: impl Fn(&str) -> Value,
    probes: impl Fn(&str) -> Vec<Probe>,
) {
    let cold_base = tempfile::TempDir::new().expect("tempdir");
    let warm_base = tempfile::TempDir::new().expect("tempdir");
    let cold_dir = cold_base.path().to_str().unwrap().to_string();
    let warm_dir = warm_base.path().to_str().unwrap().to_string();
    let (cold_probes, warm_probes) = (probes(&cold_dir), probes(&warm_dir));
    assert!(
        cold_probes.len() >= 2,
        "{cell}: one message cannot show what a run leaves for the next"
    );
    let cold_cfg = booted(cell, &overlay(&cold_dir));
    let cold = answers_under(
        cell,
        &cold_cfg["params"],
        "cold",
        &json!({}),
        Some(&cold_dir),
        &cold_probes,
        contract_of(&cold_cfg),
    )
    .await;
    let warm_cfg = booted(cell, &overlay(&warm_dir));
    let warm = answers_under(
        cell,
        &warm_cfg["params"],
        "warm",
        &json!({}),
        Some(&warm_dir),
        &warm_probes,
        contract_of(&warm_cfg),
    )
    .await;
    for (n, (c, w)) in cold.iter().zip(&warm).enumerate() {
        assert_eq!(
            w,
            c,
            "{cell} message {}: warm answers what cold answers{}",
            n + 1,
            if n > 0 {
                " -- nothing of an earlier message may reach this one"
            } else {
                ""
            }
        );
    }
}

/// No overlay: the shipped params as they are.
pub fn no_overlay(_: &str) -> Value {
    json!({})
}
