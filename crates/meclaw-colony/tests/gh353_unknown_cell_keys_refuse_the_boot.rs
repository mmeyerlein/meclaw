//! GH #353 — an unknown key in the `cell` block of a `config.json` is a hard
//! refusal on **both** read paths, exactly as `docs/config.md` § Block
//! definition (canonical) has always claimed.
//!
//! Until now `CellHeader` (`crates/meclaw-colony/src/config.rs`) was the one
//! deserialize struct in that file WITHOUT `#[serde(deny_unknown_fields)]`, so
//! serde silently dropped an unknown key. The failure mode was invisible: a
//! typo in a real key (`idle_timout_ms`) deserialized into the struct's default
//! for the field it was meant to set, the cell booted with the default idle
//! timeout, and nothing said the operator's intent had been discarded.
//!
//! What this file pins:
//!   (a) bootstrap scan — a live tree whose `cell` block carries
//!       `idle_timout_ms` refuses to boot, and the refusal names the KEY and
//!       the FILE.
//!   (b) mutation/staging parse — the same key inside a staged template is a
//!       normal validation refusal (`error_code == "schema"`, no panic) that
//!       names the KEY and the FILE.
//!   (c) regression guard — a `cell` block carrying the FULL documented key
//!       list still boots (`id`, `type`, `timeout`, `restart_limit`,
//!       `idle_timeout_ms`, `mailbox_size`, `message_timeout`, `provenance`).
//!       `surface` was on this list until W8 (GH #383) and is deliberately off
//!       it now: the key is gone with the serving path it declared, so it is an
//!       unknown key like any other and case (a) is what covers it.
//!   (d) a `cell.type: "ref"` marker (`type` + `template`, GH #277) still
//!       parses — `template` is a declared, legal key of the block.
//!   (e) fix round 1 — the mutation barrier covers SUBTREE templates too: a
//!       multi-cell template whose INNER node carries the typo is refused at
//!       validation, naming the key and that node's own `config.json`. The
//!       single-cell parse in `mutation/header_views.rs` never sees an inner
//!       node, so without an enforcement point inside the subtree walk the
//!       mutation committed and the written tree refused to boot afterwards.
//!   (f) fix round 1 — `cell.template` is template-time ONLY. Declaring it on
//!       [`meclaw_colony::CellHeader`] (so the closed list admits `ref`
//!       markers) must not make a stray `ref` in a LIVE tree parse silently:
//!       the boot walk refuses it by name, as the hand-maintained allow-list
//!       used to.

use meclaw_colony::bootstrap::BootstrapError;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

/// The typo class the issue names: `idle_timout_ms` for `idle_timeout_ms`.
const TYPO_KEY: &str = "idle_timout_ms";

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    reg
}

/// Creates a template directory with the given `config.json` body.
fn write_template(td: &TempDir, name: &str, config_body: &str) {
    let tpl_dir = td.path().join("templates").join(name);
    std::fs::create_dir_all(&tpl_dir).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
    )
    .unwrap();
    std::fs::write(tpl_dir.join("config.json"), config_body).unwrap();
}

/// Sends a mutation and reads the outcome via the ack oneshot
/// (pattern: `hardening_contract_presence.rs`).
async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("mutation ack within 30s")
        .expect("ack sender not dropped")
}

/// Triggers a templates rescan.
async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .expect("colony inbox open");
    tokio::time::timeout(Duration::from_secs(30), ack_rx)
        .await
        .expect("rescan ack within 30s")
        .expect("ack sender not dropped")
        .expect("GH #440: the rescan must not have aborted");
}

// ── (a) bootstrap scan: unknown `cell` key → hard boot refusal ───────────────

/// A live tree whose `cell` block carries the typo `idle_timout_ms` must NOT
/// boot, and the refusal must name both the offending key and the file it
/// stands in — an operator cannot fix what the message does not locate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn boot_refuses_an_unknown_cell_key_and_names_key_and_file() {
    let td = TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        format!(
            r#"{{"cell":{{"type":"echo","{TYPO_KEY}":5}},
                 "params":{{"emitted_target":"/dev/null"}},
                 "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let err = bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect_err("an unknown key in the cell block must refuse the boot");

    let (path, reason) = err
        .items()
        .iter()
        .find_map(|e| match e {
            BootstrapError::InvalidJson { path, reason } => {
                Some((path.display().to_string(), reason.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected an InvalidJson boot error, got: {:?}", err.items()));

    assert!(
        reason.contains("unknown field"),
        "the refusal must flag the unknown field, got: {reason}"
    );
    assert!(
        reason.contains(TYPO_KEY),
        "the refusal must NAME the offending key, got: {reason}"
    );
    assert!(
        reason.contains("main/config.json"),
        "the refusal must NAME the file the key stands in, got: {reason}"
    );
    assert!(
        path.ends_with("main"),
        "the refusal must locate the offending cell, got: {path}"
    );

    h.shutdown().await;
}

// ── (b) mutation/staging parse: the same key in a staged template ────────────

/// The `cell` block is read on the mutation path too. An unknown key in a
/// staged template is a normal, pre-destructive validation refusal — an
/// `error_code`, not a panic — and it names key and file just like the boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_staged_template_with_an_unknown_cell_key_is_refused_at_validation() {
    let td = TempDir::new().unwrap();
    write_template(
        &td,
        "typo_tpl",
        &format!(
            r#"{{"cell":{{"type":"echo","{TYPO_KEY}":5}},
                 "params":{{"emitted_target":"/dev/null"}},
                 "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    );
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"n1","template":"typo_tpl@1.0.0"}]}}),
    )
    .await;

    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(
                error_code, "schema",
                "an unknown cell key is a schema refusal, details: {details}"
            );
            assert!(
                details.contains(TYPO_KEY),
                "the refusal must NAME the offending key, got: {details}"
            );
            assert!(
                details.contains("config.json"),
                "the refusal must NAME the file the key stands in, got: {details}"
            );
        }
        other => panic!("expected Rejected(schema) for an unknown cell key, got {other:?}"),
    }

    h.shutdown().await;
}

// ── (c) regression guard: the full documented key list still boots ───────────

/// Every key `docs/config.md` § `cell` lists, in one block, on a tree that
/// boots. This is the guard that `deny_unknown_fields` was added against the
/// DOCUMENTED list and not against the fields the struct happened to declare —
/// `id` and `provenance` in particular stand in every instantiated tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_full_documented_cell_key_list_still_boots() {
    let td = TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        r#"{"cell":{
              "id":"0190aaaa-bbbb-7ccc-8ddd-eeeeeeeeeeee",
              "type":"echo",
              "timeout":0,
              "restart_limit":5,
              "idle_timeout_ms":60000,
              "mailbox_size":16,
              "message_timeout":30000,
              "provenance":{"template":"echo","template_version":"1.0.0",
                            "instantiated_at":1755000000}
            },
            "params":{"emitted_target":"/dev/null"},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the full documented cell key list must still boot");
    h.shutdown().await;
}

// ── (d) `ref` markers keep parsing — `template` is a legal key ───────────────

/// A `cell.type: "ref"` marker (GH #277) carries `template` in the `cell`
/// block. `deny_unknown_fields` must not turn the shipped composites
/// (`talky`, `cogny`) into unparseable trees, so `template` is a declared key.
#[test]
fn a_ref_marker_still_parses_with_its_template_key() {
    let cfg: meclaw_colony::ParsedConfig = meclaw_core::serde_json::from_str(
        r#"{"cell":{"type":"ref","template":"dispatcher@1.0.1"},
            "override_params":{"":{"external_timeout_ms":30000}}}"#,
    )
    .expect("a ref marker must keep parsing: `template` is a documented cell key");
    assert_eq!(cfg.cell.cell_type, "ref");
}

// ── (e) the mutation barrier reaches INTO a subtree template ─────────────────

/// Writes a two-cell template: a hive root plus one inner cell whose `cell`
/// block carries `body`. `parse_subtree` counts two cells, so every mutation
/// path takes its SUBTREE branch — the branch that never ran a node's `cell`
/// block through `CellHeader`.
fn write_subtree_template(td: &TempDir, name: &str, inner_cell_block: &str) {
    let tpl_dir = td.path().join("templates").join(name);
    std::fs::create_dir_all(tpl_dir.join("inner")).unwrap();
    std::fs::write(
        tpl_dir.join("template.json"),
        format!(r#"{{"name":"{name}","version":"1.0.0"}}"#),
    )
    .unwrap();
    std::fs::write(
        tpl_dir.join("config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        tpl_dir.join("inner/config.json"),
        format!(
            r#"{{"cell":{inner_cell_block},
                 "params":{{"emitted_target":"/dev/null"}},
                 "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();
}

/// The multi-cell twin of (b). `add_nodes` with a subtree template whose inner
/// node carries `idle_timout_ms` must be refused at validation — pre-destructive
/// — naming the key and the INNER node's `config.json`. Committing it wrote a
/// tree that (a) then refused to boot: the mutation accepted what the next
/// restart rejects.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_staged_subtree_template_with_an_unknown_inner_cell_key_is_refused() {
    let td = TempDir::new().unwrap();
    write_subtree_template(
        &td,
        "typo_subtree",
        &format!(r#"{{"type":"echo","{TYPO_KEY}":5}}"#),
    );
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"sub","template":"typo_subtree@1.0.0"}]}}),
    )
    .await;

    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(
                error_code, "schema",
                "an unknown cell key inside a subtree is a schema refusal, details: {details}"
            );
            assert!(
                details.contains(TYPO_KEY),
                "the refusal must NAME the offending key, got: {details}"
            );
            assert!(
                details.contains("inner/config.json"),
                "the refusal must NAME the inner node's config.json, got: {details}"
            );
        }
        other => panic!("expected Rejected(schema) for an unknown inner cell key, got {other:?}"),
    }

    h.shutdown().await;
}

/// The regression guard for (e): the same subtree template WITHOUT the typo
/// still commits, so (e) cannot pass by the subtree branch having become
/// unusable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_clean_subtree_template_still_commits() {
    let td = TempDir::new().unwrap();
    write_subtree_template(
        &td,
        "clean_subtree",
        r#"{"type":"echo","idle_timeout_ms":60000}"#,
    );
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    rescan_templates(&h, td.path().join("templates")).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{"add_nodes":[{"name":"sub","template":"clean_subtree@1.0.0"}]}}),
    )
    .await;

    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a subtree template on the documented key list must still commit, got {outcome:?}"
    );

    h.shutdown().await;
}

// ── (f) `cell.template` in a live tree: a DECLARATION since GH #424 ──────────

/// Boots a one-cell tree whose `cell` block is `cell_block` and returns the
/// first `GrowthFailed` reason plus the marker path it names.
///
/// GH #424 moved this case: until then a `cell.template` in a root tree was an
/// `InvalidJson` refusal ("template-time only … must not stand in an
/// instantiated tree"), because the boot could not instantiate. It can now, and
/// a marker on a FIRST boot is a declaration the boot fulfils. What survives —
/// and is what this section was really protecting — is the LOUDNESS: the key is
/// never silently parsed into a cell of type `"ref"` that no factory can spawn.
/// A marker naming a template that does not exist refuses the boot, and the
/// refusal names both halves: which reference, and which directory.
async fn boot_growth_refusal(cell_block: &str) -> (String, String) {
    let td = TempDir::new().unwrap();
    std::fs::create_dir_all(td.path().join("main")).unwrap();
    std::fs::write(
        td.path().join("main/config.json"),
        format!(
            r#"{{"cell":{cell_block},
                 "params":{{"emitted_target":"/dev/null"}},
                 "contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
        ),
    )
    .unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());
    let err = bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect_err("a marker naming an unknown template must refuse the boot");
    let found = err
        .items()
        .iter()
        .find_map(|e| match e {
            BootstrapError::GrowthFailed {
                path,
                reference,
                reason,
            } => Some((format!("{reference}: {reason}"), path.display().to_string())),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected a GrowthFailed boot error, got: {:?}", err.items()));
    h.shutdown().await;
    found
}

/// Both spellings of the marker are read as a declaration, and an unresolvable
/// one refuses the boot by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_template_ref_in_a_live_tree_refuses_the_boot() {
    for cell_block in [
        r#"{"type":"echo","template":"dispatcher@1.0.1"}"#,
        r#"{"type":"ref","template":"dispatcher@1.0.1"}"#,
    ] {
        let (reason, path) = boot_growth_refusal(cell_block).await;
        assert!(
            reason.contains("dispatcher@1.0.1"),
            "the refusal must NAME the reference, got: {reason}"
        );
        assert!(
            reason.contains("template_missing"),
            "…with the mutation path's own code, got: {reason}"
        );
        assert!(
            path.ends_with("main"),
            "…and the directory the marker stands in, got: {path}"
        );
    }
}

/// The negative twin of (d): a key that is NOT on the documented list is
/// refused by the very same parse, so (d) cannot pass by the struct simply
/// having stayed open.
#[test]
fn the_same_parse_refuses_a_key_outside_the_documented_list() {
    let err = meclaw_core::serde_json::from_str::<meclaw_colony::ParsedConfig>(&format!(
        r#"{{"cell":{{"type":"echo","{TYPO_KEY}":5}}}}"#
    ))
    .expect_err("an unknown cell key must not deserialize");
    let msg = err.to_string();
    assert!(
        msg.contains("unknown field") && msg.contains(TYPO_KEY),
        "the parse error must flag and name the unknown key, got: {msg}"
    );
}
