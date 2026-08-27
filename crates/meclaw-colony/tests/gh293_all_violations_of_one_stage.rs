//! GH #293 — a stage names ALL of its violations, not just the first one.
//!
//! Today every mutation check returns `Result<(), MutationError>` and the first
//! violation ends validation, so a diff with three unresolvable templates costs
//! three round trips to learn three names. The collecting validator fixes the
//! *report*, not the verdict: what is accepted and what is refused does not
//! change, the refusal just carries every violation the stage produced.
//!
//! The pipeline still stops at the first stage that produced anything. That is
//! the second half of the fix and it is deliberate: an unresolved template makes
//! every later endpoint error a consequence rather than a cause, and twenty
//! derived errors hide the one real one.
//!
//! This file covers stage 2 (`template_resolution`, task 18), stage 4
//! (`post_state_addresses`, task 19), stage 5 (`edge_endpoints`, task 20) and
//! stage 6 (`contract_locality`, task 20).

use meclaw_colony::mutation::port_boundary::{SealedHive, collect_hive_port_boundary};
use meclaw_colony::mutation::rejection::MutationRejection;
use meclaw_colony::mutation::validate::{
    collect_edge_endpoints, collect_post_state_addresses, collect_template_resolution,
};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry};
use meclaw_colony::{CellFactory, CellFactoryRegistry};
use meclaw_core::JsonValue;
use meclaw_core::serde_json::json;
use meclaw_testing::factories::PersistCellFactory;
use std::sync::Arc;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn entry(root: &std::path::Path, name: &str) -> TemplateEntry {
    TemplateEntry {
        template_id: format!("t-{name}"),
        name: name.to_string(),
        version: None,
        filesystem_path: root.join("templates").join(name),
    }
}

/// Four templates that DO resolve:
///
/// - `solo`, a single cell whose type the colony knows,
/// - `dangling`, whose sub-unit is a `cell.type: "ref"` marker pointing at a
///   template that is not in the registry — the "ref target does not exist"
///   case, which only the subtree parse can see,
/// - `unit`, a subtree whose `override_params` keys are its cells' paths, and
/// - `stranger`, whose cell type no factory serves.
fn write_templates(root: &std::path::Path) {
    let solo = root.join("templates").join("solo");
    write(&solo, "template.json", r#"{"name":"solo"}"#);
    write(
        &solo,
        "config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let dangling = root.join("templates").join("dangling");
    write(&dangling, "template.json", r#"{"name":"dangling"}"#);
    write(
        &dangling,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        &dangling,
        "r/config.json",
        r#"{"cell":{"type":"ref","template":"ghost"}}"#,
    );

    let unit = root.join("templates").join("unit");
    write(&unit, "template.json", r#"{"name":"unit"}"#);
    write(
        &unit,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        &unit,
        "a/config.json",
        r#"{"cell":{"type":"persist_mock"},"params":{"p":1},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );

    let stranger = root.join("templates").join("stranger");
    write(&stranger, "template.json", r#"{"name":"stranger"}"#);
    write(
        &stranger,
        "config.json",
        r#"{"cell":{"type":"stranger_type"},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

fn registry(root: &std::path::Path) -> TemplatesRegistry {
    TemplatesRegistry::from_entries(vec![
        entry(root, "solo"),
        entry(root, "dangling"),
        entry(root, "unit"),
        entry(root, "stranger"),
    ])
}

/// Every violation of the rejection, as `stage` tokens.
fn stages(rejection: &MutationRejection) -> Vec<&'static str> {
    rejection
        .entries()
        .iter()
        .map(|v| v.stage.as_str())
        .collect()
}

fn collect_templates(diff: &JsonValue, root: &std::path::Path) -> MutationRejection {
    let mut rejection = MutationRejection::new();
    collect_template_resolution(diff, &registry(root), &mut rejection);
    rejection
}

// ───────────────────────────────── task 18: stage 2 collects

#[test]
fn three_unresolvable_templates_are_all_named_at_once() {
    let td = tempfile::TempDir::new().unwrap();
    write_templates(td.path());

    let diff = json!({"add_nodes":[
        {"name":"a","template":"nope_a"},
        {"name":"b","template":"nope_b@1.0.0"},
        {"name":"c","template":"dangling"},
        {"name":"ok","template":"solo"}
    ]});
    let rejection = collect_templates(&diff, td.path());

    assert_eq!(
        rejection.entries().len(),
        3,
        "one entry per unresolvable reference, and none for the one that resolves: {:?}",
        rejection.entries()
    );
    assert_eq!(
        stages(&rejection),
        ["template_resolution"; 3],
        "all three belong to stage 2"
    );

    let addresses: Vec<Option<&str>> = rejection
        .entries()
        .iter()
        .map(|v| v.address.as_deref())
        .collect();
    assert_eq!(
        addresses,
        [Some("nope_a"), Some("nope_b@1.0.0"), Some("dangling"),],
        "each entry names its own reference"
    );

    let rendered = rejection.render();
    for name in ["nope_a", "nope_b@1.0.0", "dangling"] {
        assert!(
            rendered.contains(name),
            "the rendered refusal names {name}: {rendered}"
        );
    }
    assert!(
        rendered.contains("ghost"),
        "and the dangling ref says what it could not reach: {rendered}"
    );
    assert_eq!(
        rejection.error_code(),
        Some("template_missing"),
        "the reported code is the first entry's — what the Result form returned"
    );
}

#[test]
fn a_diff_whose_template_fails_to_resolve_is_refused_at_that_stage_with_no_endpoint_errors_appended()
 {
    // GH #293 acceptance: the diff also draws an edge to the node the missing
    // template would have created. That endpoint error is a CONSEQUENCE of the
    // unresolved template, so stage 2 refuses alone and stage 5 never runs.
    let td = tempfile::TempDir::new().unwrap();
    write_templates(td.path());

    let diff = json!({
        "add_nodes":[{"name":"a","template":"nope_a"}],
        "add_edges":[{"from":"a","to":"b"}]
    });
    let rejection = collect_templates(&diff, td.path());

    assert_eq!(
        rejection.entries().len(),
        1,
        "exactly one entry — no derived endpoint errors appended: {:?}",
        rejection.entries()
    );
    assert_eq!(stages(&rejection), ["template_resolution"]);
    let rendered = rejection.render();
    assert!(
        !rendered.contains("edge") && !rendered.contains("endpoint"),
        "nothing about the edge is said: {rendered}"
    );
}

// ───────────────────────────────── task 19: stage 4 collects

/// The colony knows `persist_mock` and nothing else — `stranger_type` has no
/// factory, which is what makes `stranger` an unknown cell type.
fn factories() -> CellFactoryRegistry {
    let mut reg = CellFactoryRegistry::new();
    reg.insert(
        "persist_mock".to_string(),
        Arc::new(PersistCellFactory {
            spawn_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        }) as Arc<dyn CellFactory>,
    );
    reg
}

#[test]
fn five_address_violations_in_one_diff_are_all_named() {
    // Spec acceptance 5: one diff, five things wrong with the addresses its
    // post-state would carry — two names already taken, a `swap_nodes` match
    // that hits nothing, an `override_params` key addressing no cell of the
    // subtree, and a template whose cell type no factory serves. One round trip.
    let td = tempfile::TempDir::new().unwrap();
    write_templates(td.path());

    let diff = json!({
        "add_nodes":[
            {"name":"taken1","template":"solo"},
            {"name":"taken2","template":"solo"},
            {"name":"u1","template":"unit","override_params":{"nope":{"p":1}}},
            {"name":"s1","template":"stranger"}
        ],
        "swap_nodes":[{"match":{"name":"ghost_node"},"with":{"name":"taken1"}}]
    });

    let registry_names = ["taken1".to_string(), "taken2".to_string()];
    let template_to_cell_type = [
        ("solo".to_string(), "persist_mock".to_string()),
        ("unit".to_string(), "hive".to_string()),
        ("stranger".to_string(), "stranger_type".to_string()),
    ];

    let mut rejection = MutationRejection::new();
    collect_post_state_addresses(
        &diff,
        &registry(td.path()),
        &factories(),
        &registry_names,
        &template_to_cell_type,
        &[],
        "/",
        &[],
        &[],
        &mut rejection,
    );

    assert_eq!(
        rejection.entries().len(),
        5,
        "all five address violations at once: {:?}",
        rejection.entries()
    );
    assert_eq!(
        stages(&rejection),
        ["post_state_addresses"; 5],
        "all five belong to stage 4"
    );

    let rendered = rejection.render();
    for subject in ["taken1", "taken2", "ghost_node", "nope", "stranger_type"] {
        assert!(
            rendered.contains(subject),
            "the rendered refusal names {subject}: {rendered}"
        );
    }
    assert_eq!(
        rejection.error_code(),
        Some("naming_collision"),
        "the reported code is the first entry's — what the Result form returned"
    );
}

// ───────────────────────────────── task 20: stage 5 collects

#[test]
fn three_dangling_edge_endpoints_are_all_named() {
    // Three edges, three endpoints that no post-state node answers to. The
    // `Result` form returned the first one and the caller learned the other two
    // one mutation round trip apiece.
    let diff = json!({"add_edges":[
        {"from":"ghost_a","to":"here"},
        {"from":"here","to":"ghost_b"},
        {"from":"ghost_c","to":"here"}
    ]});

    let mut rejection = MutationRejection::new();
    collect_edge_endpoints(
        &diff,
        &["here".to_string()],
        &[],
        &[],
        &[],
        &[],
        "/",
        &[],
        // GH #285: no hive here declares a slot, so the second known-set is empty
        // and this stage sees exactly the universe it saw before.
        &std::collections::HashSet::new(),
        &mut rejection,
    );

    assert_eq!(
        rejection.entries().len(),
        3,
        "one entry per dangling endpoint, and none for the one that exists: {:?}",
        rejection.entries()
    );
    assert_eq!(
        stages(&rejection),
        ["edge_endpoints"; 3],
        "all three belong to stage 5"
    );

    let addresses: Vec<Option<&str>> = rejection
        .entries()
        .iter()
        .map(|v| v.address.as_deref())
        .collect();
    assert_eq!(
        addresses,
        [Some("ghost_a"), Some("ghost_b"), Some("ghost_c")],
        "each entry names its own endpoint"
    );

    let rendered = rejection.render();
    for name in ["ghost_a", "ghost_b", "ghost_c"] {
        assert!(
            rendered.contains(name),
            "the rendered refusal names {name}: {rendered}"
        );
    }
    assert_eq!(
        rejection.error_code(),
        Some("edge_schema"),
        "the reported code is the first entry's — what the Result form returned"
    );
}

// ───────────────────────────────── task 20: stage 6 collects

#[test]
fn two_port_boundary_breaches_in_one_diff_are_both_named() {
    // One sealed hive, two edges that wire past its port — one reaching in, one
    // reaching out. They are the same breach seen from two ends, and a caller
    // that fixes only the first one comes straight back with the second.
    let sealed = vec![SealedHive {
        path: "/aff".into(),
        ports: vec!["brief".into()],
        // GH #285: this hive declares no slot; the collecting check does not
        // read the list either way.
        slots: vec![],
    }];
    let diff = json!({"add_edges":[
        {"from":"./caller","to":"./aff/store"},
        {"from":"./aff/inner","to":"./caller"}
    ]});

    let mut rejection = MutationRejection::new();
    collect_hive_port_boundary(&diff, "/", &sealed, &mut rejection);

    assert_eq!(
        rejection.entries().len(),
        2,
        "both breaches at once: {:?}",
        rejection.entries()
    );
    assert_eq!(
        stages(&rejection),
        ["contract_locality"; 2],
        "the port boundary is one of stage 6's three checks"
    );

    let addresses: Vec<Option<&str>> = rejection
        .entries()
        .iter()
        .map(|v| v.address.as_deref())
        .collect();
    assert_eq!(
        addresses,
        [Some("/aff/store"), Some("/aff/inner")],
        "each entry names the interior node it reached, resolved"
    );

    let rendered = rejection.render();
    for name in ["/aff/store", "/aff/inner", "brief"] {
        assert!(
            rendered.contains(name),
            "the rendered refusal names {name}: {rendered}"
        );
    }
    assert_eq!(
        rejection.error_code(),
        Some("hive_port_boundary"),
        "the reported code is the first entry's — what the Result form returned"
    );
}

// ─────────────────── task 21: the pipeline reports once, end to end

/// The whole point of the wave, measured through the real
/// `/colony/mutations` path rather than through a collector called by hand: a
/// diff carrying five independent violations of ONE stage comes back as ONE
/// refusal that names all five.
///
/// `every_single_violation_test_still_names_what_it_named` is deliberately NOT
/// a test of its own — the existing suite IS that test. Some 4800 workspace
/// tests assert the `error_code` and the message of a single-violation
/// refusal, and the collecting pipeline is only correct if every one of them
/// still passes untouched. Writing one more test that asserts the same thing a
/// hundred other tests already assert would prove nothing they do not.
mod pipeline {
    use meclaw_colony::api_dto::ReadRegistryReply;
    use meclaw_colony::{
        CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
    };
    use meclaw_core::{Uuid, serde_json::json};
    use meclaw_testing::ColonyHandle;
    use meclaw_testing::factories::PersistCellFactory;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;
    use tokio::sync::oneshot;

    const CELL_CONFIG: &str = r#"{"cell":{"type":"persist_mock","idle_timeout_ms":60000},"params":{"terminal":true},"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;
    const HIVE_CONFIG: &str = r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#;

    fn write(dir: &std::path::Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    /// `main` (logical `/`) with a cell `/anchor`, a second cell `/resumed`
    /// whose directory an `add_nodes` can reconnect to, and one template
    /// `brokenref` whose sub-unit points at a template nobody holds — the
    /// cheapest thing that makes `parse_subtree` fail.
    async fn bootstrapped_colony() -> (tempfile::TempDir, ColonyHandle) {
        let td = tempfile::TempDir::new().unwrap();
        write(td.path(), "main/config.json", HIVE_CONFIG);
        write(td.path(), "main/anchor/config.json", CELL_CONFIG);
        write(td.path(), "main/resumed/config.json", CELL_CONFIG);
        write(
            td.path(),
            "templates/brokenref/template.json",
            r#"{"name":"brokenref"}"#,
        );
        write(td.path(), "templates/brokenref/config.json", CELL_CONFIG);
        write(
            td.path(),
            "templates/brokenref/r/config.json",
            r#"{"cell":{"type":"ref","template":"ghost"}}"#,
        );
        write(
            td.path(),
            "templates/solo/template.json",
            r#"{"name":"solo"}"#,
        );
        write(td.path(), "templates/solo/config.json", CELL_CONFIG);
        let factory: Arc<dyn CellFactory> = Arc::new(PersistCellFactory {
            spawn_count: Arc::new(AtomicU32::new(0)),
        });
        let h = ColonyHandle::new_with_factories_at(
            &td,
            vec![("persist_mock".to_string(), factory.clone())],
        );
        let mut reg = CellFactoryRegistry::new();
        reg.insert("persist_mock".into(), factory);
        bootstrap_from_filesystem(td.path(), &reg, &h.runtime())
            .await
            .expect("bootstrap must succeed");
        let (ack_tx, ack_rx) = oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::RescanTemplates {
                templates_root: td.path().join("templates"),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx
            .await
            .unwrap()
            .expect("GH #440: the rescan must not have aborted");
        (td, h)
    }

    async fn send_mutation(
        h: &ColonyHandle,
        payload: meclaw_core::serde_json::Value,
    ) -> MutationOutcome {
        let (ack_tx, ack_rx) = oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::Mutation {
                payload,
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx
            .await
            .expect("the colony must answer the mutation, not die validating it")
    }

    async fn registry_size(h: &ColonyHandle) -> usize {
        let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
        h.inbox_tx
            .send(ColonyMsg::ReadRegistry {
                path: None,
                path_prefix: None,
                cell_type: None,
                active: None,
                limit: 200,
                ack: ack_tx,
            })
            .await
            .unwrap();
        ack_rx.await.unwrap().entries.len()
    }

    /// Five `remove_nodes` patterns that hit nothing. Every one of them is a
    /// stage-4 (`post_state_addresses`) `match_no_hit`, and before GH #293 the
    /// operator learned about them one refusal at a time — five round trips,
    /// each of which (GH #276) could leave residue behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_mutation_with_five_violations_in_one_stage_is_refused_once_naming_all_five() {
        let (_td, h) = bootstrapped_colony().await;
        let before = registry_size(&h).await;

        let ghosts = ["ghost_a", "ghost_b", "ghost_c", "ghost_d", "ghost_e"];
        let (ack_tx, ack_rx) = oneshot::channel();
        h.inbox_tx
            .send(ColonyMsg::Mutation {
                payload: json!({
                    "scope": "/",
                    "diff": {
                        "remove_nodes": ghosts
                            .iter()
                            .map(|g| json!({"match": {"name": g}}))
                            .collect::<Vec<_>>()
                    }
                }),
                reply_to: None,
                trace_id: Uuid::now_v7(),
                parent_message_id: Uuid::now_v7(),
                ack: ack_tx,
            })
            .await
            .unwrap();
        let outcome = ack_rx
            .await
            .expect("the colony must answer the mutation, not die validating it");

        let MutationOutcome::Rejected {
            error_code,
            details,
            violations,
            ..
        } = &outcome
        else {
            panic!("five no-hit patterns must be refused; got {outcome:?}");
        };

        assert_eq!(
            violations.len(),
            5,
            "one refusal carrying all five violations of the stage, not the \
             first one: {violations:?}"
        );
        for v in violations {
            assert_eq!(
                v.stage.as_str(),
                "post_state_addresses",
                "all five belong to stage 4: {v:?}"
            );
        }
        assert_eq!(
            error_code, violations[0].code,
            "the reported code is the first violation's — what the \
             single-violation pipeline returned"
        );
        assert_eq!(error_code, "match_no_hit");
        for ghost in ghosts {
            assert!(
                details.contains(ghost),
                "the rendered refusal must name {ghost}: {details}"
            );
        }

        assert_eq!(
            registry_size(&h).await,
            before,
            "a refused mutation changes nothing"
        );
        h.shutdown().await;
    }

    /// Fix round 1 — the accept→refuse flip that was suspected here, measured
    /// and found not to exist, and pinned so nobody has to re-derive it.
    ///
    /// The suspicion was reasonable: stage 2 walks every `add_nodes` template
    /// with `parse_subtree`, and the sequential pipeline reached that walk only
    /// from stage 3 (`validate_requires`) — which exempts a Resume, because a
    /// Reconnect instantiates nothing (Task 15). Reading validation alone, a
    /// Resume onto a template with a broken `ref` and no `override_params`
    /// looks like something that used to commit.
    ///
    /// It did not. `build_staging_tree_from_templates` calls `parse_subtree`
    /// with a `?` BEFORE its single-cell existence-skip (the subtree-dispatch
    /// is deliberately ahead of the skip, so a partially-existing subtree still
    /// merge-stages). The Resume was refused with the very same
    /// `template_missing` — just later, after a `.staging` directory had been
    /// built and with a `failed` audit row instead of a `rejected` one.
    ///
    /// So stage 2 takes NO Resume exemption, and this test holds both ends of
    /// that: a broken template is refused either way, and an intact template is
    /// still resumable. The day someone adds the exemption "for symmetry with
    /// stage 3", the second assertion below goes red.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_broken_ref_is_refused_for_a_resume_and_a_fresh_instantiation_alike() {
        let (_td, h) = bootstrapped_colony().await;

        // `/resumed` exists on disk, so this `add_nodes` is a Reconnect, and it
        // supplies neither `ctx` nor `override_params` — the constellation in
        // which validation alone would never have parsed the template.
        let resumed = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "resumed", "template": "brokenref"}]}
            }),
        )
        .await;
        let MutationOutcome::Rejected {
            error_code,
            details,
            violations,
            ..
        } = &resumed
        else {
            panic!(
                "a broken `ref` is refused for a Resume too — staging parses the \
                 template before its existence-skip; got {resumed:?}"
            );
        };
        assert_eq!(error_code, "template_missing", "{details}");
        assert_eq!(
            violations.len(),
            1,
            "and it is refused at stage 2 now, pre-destructively, instead of at \
             staging: {violations:?}"
        );
        assert_eq!(violations[0].stage.as_str(), "template_resolution");
        assert!(
            details.contains("ghost"),
            "the refusal says what it could not reach: {details}"
        );

        // The same template at a free name is an ordinary instantiation and is
        // refused identically — the exemption question changes nothing here.
        let fresh = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "fresh", "template": "brokenref"}]}
            }),
        )
        .await;
        assert!(
            matches!(
                &fresh,
                MutationOutcome::Rejected { error_code, .. } if error_code == "template_missing"
            ),
            "a fresh instantiation of a broken template must be refused; got {fresh:?}"
        );

        // The control: a Resume onto an INTACT template still commits. This is
        // the assertion that would have caught a real stage-2 flip, and it is
        // the one to look at first if this file ever goes red.
        let intact = send_mutation(
            &h,
            json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": "resumed", "template": "solo"}]}
            }),
        )
        .await;
        assert!(
            matches!(intact, MutationOutcome::Committed { .. }),
            "a Reconnect onto a healthy template is not something any stage may \
             refuse; got {intact:?}"
        );

        h.shutdown().await;
    }
}
