//! GH #567 (half 1) — a newborn's contracts are read from the whole STAGED
//! SUBTREE, and a `swap_nodes` successor's are read at all.
//!
//! GH #562 taught stage 6 to read the contract of a hive this diff gives BIRTH
//! to: until the node is staged that declaration lives only in the template
//! directory, so a v-lane drawn onto a hive the same mutation instantiates has
//! something to ask. It read exactly ONE file, though — the template root's
//! `config.json` — and only for `add_nodes`. Two gaps followed from that, and
//! both of them sit on the shape a grow recipe actually has:
//!
//! * **the nested occupant.** A composite template is a hive with `ref`
//!   markers inside it, and the rim that declares the connect point is one of
//!   those refs (`talky@4.6.1` declares `credential_request` at `./brain`; the
//!   `assistant` that holds it declares nothing). A naive directory walk gets
//!   this wrong twice over — the marker directory holds no contract of its own,
//!   and the contract lives in a completely different template.
//! * **the swapped-in successor.** A generation change is ONE mutation (GH
//!   #256): `add_nodes` grows the successor, `swap_nodes` swings the old node's
//!   edges onto it, and the level's own wiring is drawn beside both. A v-lane
//!   onto the successor's rim is part of that wiring — and it was refused for
//!   the same root-only read, because the successor is a composite too. The
//!   successor's contract WAS read a second time further down, for the
//!   re-anchor verdict (`v_lane_reanchor_verdict`), and thrown away; that read
//!   is now the same one, hoisted to where the birth contracts are built.
//!
//! Both are one question — *which contracts does this diff bring into
//! existence* — and the answer is now one read
//! ([`contracts_from_template_subtree`], built on the ref-aware
//! `parse_subtree`), used at both doors.
//!
//! # Why a real colony
//!
//! The verdict is read off the colony's own edge table through `/colony/graph`:
//! a commit is proven by the v-lane BEING there with the lane it declared, not
//! by "the mutation did not say no".
//!
//! # What is deliberately unchanged
//!
//! The list stays APPENDED, never substituted, and it still reaches the port
//! boundary and nothing else. A path that already stands keeps the contract it
//! was born with — a diff cannot talk a live hive into a connect point by
//! naming a template. Two tests hold that line, and they hold it at the two
//! doors an occupied path can be named through: the third names a path whose
//! cells are AWAKE (refused before stage 6 ever runs), the fourth names one
//! whose cells are asleep, which is a legal RESUME and therefore the case where
//! stage 6 really does get a say. The guard asks the colony's own hive table
//! what STANDS — asking the contract list instead would have missed every
//! standing hive that declares nothing, which is exactly the hive a resume
//! could otherwise have talked a connect point into.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::{CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::{JsonValue, Path, Uuid, serde_json::json};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use tokio::sync::oneshot;

// ── Harness ──────────────────────────────────────────────────────────────────

const ECHO: &str = r#"{"cell":{"type":"echo"},"params":{"emitted_target":"/dev/null"},
    "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#;

/// The lane every case draws, and the connect point the rim declares for it.
const LANE: &str = "credential_request";
const CONNECT_POINT: &str = "./brain";

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// `talky@1.0.0` — the SEALED rim that declares the lane.
///
/// ```text
/// talky/            hive, ports: [], accepts credential_request at ./brain
/// └── brain/        echo cell — the connect point
/// ```
///
/// This is the shape of the real `talky@4.6.1`: `ports: []` means no caller may
/// name anything inside, and the v-lane is the one exception the template
/// pronounces about ITSELF.
fn write_talky_template(root: &std::path::Path) {
    let tpl = root.join("templates/talky");
    write(
        &tpl,
        "template.json",
        r#"{"name":"talky","version":"1.0.0"}"#,
    );
    write(
        &tpl,
        "config.json",
        &format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{"ports":[],"contract":{{"accepts":[
                {{"route":"{LANE}","at":["{CONNECT_POINT}"],
                  "because":"a brain runs on a grant id, not on a key in its config"}}]}}}}}}"#
        ),
    );
    write(&tpl, "brain/config.json", ECHO);
}

/// `gen@1.0.0` — the generation unit: a hive whose OCCUPANT is a `ref` at
/// `./talky`. It declares no contract itself; everything the caller may dock on
/// lives one template further in.
fn write_gen_template(root: &std::path::Path) {
    let tpl = root.join("templates/gen");
    write(&tpl, "template.json", r#"{"name":"gen","version":"1.0.0"}"#);
    write(&tpl, "config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        &tpl,
        "talky/config.json",
        r#"{"cell":{"type":"ref","template":"talky@1.0.0"}}"#,
    );
}

/// `talky_bare@1.0.0` / `gen_bare@1.0.0` — the SAME shape with no contract
/// anywhere in it. What a hive was born from is what it keeps, so this is the
/// generation a resume must not be able to re-declare.
fn write_bare_templates(root: &std::path::Path) {
    let tpl = root.join("templates/talky_bare");
    write(
        &tpl,
        "template.json",
        r#"{"name":"talky_bare","version":"1.0.0"}"#,
    );
    write(&tpl, "config.json", r#"{"cell":{"type":"hive"}}"#);
    write(&tpl, "brain/config.json", ECHO);

    let tpl = root.join("templates/gen_bare");
    write(
        &tpl,
        "template.json",
        r#"{"name":"gen_bare","version":"1.0.0"}"#,
    );
    write(&tpl, "config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        &tpl,
        "talky/config.json",
        r#"{"cell":{"type":"ref","template":"talky_bare@1.0.0"}}"#,
    );
}

/// The root tree: one caller (`/broker`) and nothing else. `plant_standing_gen`
/// adds a LIVE `gen` beside it for the two cases that need a pre-state.
fn plant(root: &std::path::Path) {
    write(root, "main/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(root, "main/broker/config.json", ECHO);
    write_talky_template(root);
    write_gen_template(root);
    write_bare_templates(root);
}

/// A `gen` that STANDS, grown by the BOOT out of `gen_bare@1.0.0` and born
/// asleep: the `ref` marker declares `birth: "inactive"` (GH #437), so every
/// cell of the unit is registered and addressable but never spawned. That is
/// what opens the RESUME door — an `add_nodes` at this path is not a collision,
/// it is the same node being re-addressed.
fn plant_sleeping_gen(root: &std::path::Path) {
    write(
        root,
        "main/gen/config.json",
        r#"{"cell":{"type":"ref","template":"gen_bare@1.0.0"},"birth":"inactive"}"#,
    );
}

/// A standing `/gen` with the same INSIDE shape as the template and NO contract
/// anywhere in it — the hive that must not be talked into a connect point.
fn plant_standing_gen(root: &std::path::Path) {
    write(root, "main/gen/config.json", r#"{"cell":{"type":"hive"}}"#);
    write(
        root,
        "main/gen/talky/config.json",
        r#"{"cell":{"type":"hive"},"params":{"ports":[]}}"#,
    );
    write(root, "main/gen/talky/brain/config.json", ECHO);
}

async fn boot(root: &std::path::Path) -> ColonyHandle {
    let h = ColonyHandle::new_with_echo_at(root);
    let mut factories = CellFactoryRegistry::new();
    factories.insert(
        "echo".to_string(),
        std::sync::Arc::new(EchoCellFactory) as std::sync::Arc<dyn meclaw_colony::CellFactory>,
    );
    rescan_templates(&h, root.join("templates")).await;
    bootstrap_from_filesystem(root, &factories, &h.runtime())
        .await
        .expect("the tree boots");
    h
}

async fn send_mutation(h: &ColonyHandle, payload: JsonValue) -> MutationOutcome {
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
    ack_rx.await.unwrap()
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("the template scan succeeds");
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// The POSITIVE receipt: the v-lane onto `to`, as the colony's own edge table
/// reports it over `/colony/graph`.
async fn v_lane_edge(h: &ColonyHandle, to: &str) -> Option<meclaw_colony::api_dto::GraphEdgeDto> {
    read_graph(h)
        .await
        .edges
        .into_iter()
        .find(|e| e.from == "/broker" && e.to == to)
}

/// The v-lane onto a deep endpoint below `unit`, named as the lane it carries.
fn v_lane_onto(unit: &str) -> JsonValue {
    json!({"from":"./broker","to":format!("./{unit}/talky/brain"),"lane":LANE})
}

// ── (a) the nested occupant of a newborn ─────────────────────────────────────

/// `add_nodes` grows `gen`, whose occupant `./talky` is a `ref` to another
/// template — and THAT template is the one declaring `credential_request` at
/// `./brain`. The v-lane is drawn in the same diff, straight onto
/// `/gen/talky/brain`.
///
/// Before GH #567 stage 6 read exactly one file, `templates/gen/config.json`,
/// which declares nothing: the target hive `/gen/talky` had no contract in the
/// list at all, so the rule table's last row fired and the whole grow recipe
/// earned `v_lane_no_connect_point`. The declaration was there the whole time,
/// one `ref` hop away.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_v_lane_onto_a_nested_occupant_of_a_newborn_commits() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    let h = boot(td.path()).await;

    let grown = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen","template":"gen@1.0.0"}],
            "add_edges":[v_lane_onto("gen")]
        }}),
    )
    .await;
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "the rim one `ref` below the newborn root declares the connect point: {grown:?}"
    );

    let edge = v_lane_edge(&h, "/gen/talky/brain")
        .await
        .expect("the v-lane stands on the newborn's nested connect point");
    assert_eq!(
        edge.lane.as_deref(),
        Some(LANE),
        "and it carries the lane it declared: {edge:?}"
    );

    h.shutdown().await;
}

// ── (b) the swapped-in successor ─────────────────────────────────────────────

/// A generation change, the way GH #256 says one is written: ONE mutation that
/// grows `gen2`, swings `gen`'s edges onto it, and draws the level's own wiring
/// beside both — here the v-lane straight onto the successor's connect point,
/// `/gen2/talky/brain`.
///
/// This is the shape the GH #562 CHANGELOG paragraph named as refused: "drawn in
/// one breath the same four edges are refused `v_lane_no_connect_point`; drawn
/// one declaration later the level stands and its contracts are read off the
/// disk". Order was semantics, and it was semantics for a reason nobody wanted —
/// the successor is a composite, so its connect point lives one `ref` hop below
/// the template root that was the only file being read.
///
/// The swap block reaches the same successor through the same read
/// (`swap_successors`), so the re-anchor verdict and the port boundary cannot
/// disagree about what the successor declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_v_lane_onto_a_swapped_in_successors_connect_point_commits() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    plant_standing_gen(td.path());
    let h = boot(td.path()).await;

    let swapped = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen2","template":"gen@1.0.0"}],
            "swap_nodes":[{"match":{"name":"gen"},"with":{"name":"gen2"}}],
            "add_edges":[v_lane_onto("gen2")]
        }}),
    )
    .await;
    assert!(
        matches!(swapped, MutationOutcome::Committed { .. }),
        "the successor is staged by this diff and its nested contract counts: {swapped:?}"
    );

    let edge = v_lane_edge(&h, "/gen2/talky/brain")
        .await
        .expect("the v-lane stands on the successor's connect point");
    assert_eq!(
        edge.lane.as_deref(),
        Some(LANE),
        "and it carries the lane it declared: {edge:?}"
    );

    h.shutdown().await;
}

// ── (c) the append semantics stay append ─────────────────────────────────────

/// `/gen` and `/gen/talky` STAND, and neither declares a thing. The diff names
/// a template for the very path that is already occupied and draws the v-lane
/// beside it.
///
/// This must not commit. The birth contracts are APPENDED, never substituted:
/// a path that already stands keeps the contract it was born with, and reading
/// a whole subtree instead of one file must not turn "name a template at an
/// occupied path" into a way to hand a live hive a connect point it never
/// declared. An earlier stage says so first and by name — an `add_nodes` at an
/// occupied path is a RESUME, and a subtree resume onto living cells is
/// `resume_requires_stopped_cell` — so stage 6 never gets a say at all, and the
/// graph is unchanged afterwards, which is the half that matters.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standing_hive_keeps_the_contract_it_was_born_with() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    plant_standing_gen(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen","template":"gen@1.0.0"}],
            "add_edges":[v_lane_onto("gen")]
        }}),
    )
    .await;
    let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    else {
        panic!("a live hive must not be talked into a connect point: {outcome:?}");
    };
    assert_eq!(
        error_code, "resume_requires_stopped_cell",
        "the occupied path is refused before any contract is read: {details}"
    );

    assert!(
        v_lane_edge(&h, "/gen/talky/brain").await.is_none(),
        "and the refused diff left no v-lane behind"
    );

    h.shutdown().await;
}

/// The same line, at the door where stage 6 actually gets a say.
///
/// `/gen` STANDS — the boot grew it from `gen_bare@1.0.0`, which declares no
/// contract anywhere — and its cells were born asleep, so an `add_nodes` at that
/// path is a legal RESUME rather than a collision: it passes stage 4, stages
/// nothing, and rewrites no `config.json`. The diff names `gen@1.0.0` instead,
/// whose `./talky` DOES declare `credential_request` at `./brain`, and draws the
/// v-lane onto it.
///
/// Nothing about that resume changes what is on disk, so the connect point it
/// names exists nowhere: `/gen/talky` stands with the contract it was born with,
/// which is none. Permitting the lane here would be the port boundary waving an
/// edge through on the strength of a declaration the colony does not hold —
/// found in review 2026-09-02, because the guard used to dedup against the
/// CONTRACT list, and a hive that declares nothing is not in it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_cannot_re_declare_a_standing_hives_connect_point() {
    let td = tempfile::TempDir::new().unwrap();
    plant(td.path());
    plant_sleeping_gen(td.path());
    let h = boot(td.path()).await;

    let outcome = send_mutation(
        &h,
        json!({"scope":"/","diff":{
            "add_nodes":[{"name":"gen","template":"gen@1.0.0"}],
            "add_edges":[v_lane_onto("gen")]
        }}),
    )
    .await;
    let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    else {
        panic!("a resume must not import a contract onto a hive that stands: {outcome:?}");
    };
    assert_eq!(
        error_code, "v_lane_no_connect_point",
        "the standing hive keeps the contract it was born with — none: {details}"
    );

    assert!(
        v_lane_edge(&h, "/gen/talky/brain").await.is_none(),
        "and the refused diff left no v-lane behind"
    );

    h.shutdown().await;
}
