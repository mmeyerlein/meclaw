//! GH #302 — `org@1.0.0` is a namespace, and the file says so by being thin.
//!
//! The level rule of this wave is *a level owns what its siblings must share*.
//! The members of one organisation share a name and a boundary. They share
//! nothing else — not a memory, not an identity, not a broker, not a firewall,
//! because under the GH #122 ruling of 2026-08-19 **the memory belongs to the
//! member** and *a group is an audience, not a holder*. So this level owns a
//! name and a boundary and it owns them by being a hive with one empty
//! container and a transit contract. #302 puts the acceptance in one sentence:
//! *its value is the namespace, not the contents; it should not be padded to
//! look substantial.*
//!
//! Thinness is exactly the property a test can hold. Four questions, and the
//! first three are facts about the FILES — checkable with no colony and no
//! runtime, the same reasoning as `gh173_shipped_hive_contracts`:
//!
//! 1. **One container, named `members`, with nothing under it.** A second child
//!    directory would be a sibling the level did not have to own.
//! 2. **Both `config.json` files are hives, and no other cell type exists
//!    anywhere in the template.** An org that grows a cell has stopped being a
//!    namespace — that is the whole of the level's definition, and it is the
//!    one thing padding would break first.
//! 3. **The level declares the UNION of its occupant's lanes.** A container
//!    level's contract is derived, never invented: *the union of what its
//!    occupants accept and emit, minus the lanes a sibling inside the level
//!    consumes itself.* Inside an org there is only the container, so nothing
//!    is subtracted and the union is the whole of `member`'s two lists —
//!    checked by reading `templates/member/config.json` off the tree rather
//!    than from a list written here, so that moving a lane in the occupant
//!    goes red here instead of losing a message to `no_route` at a boundary.
//! 4. **Neither hive carries a `params.ports` key, and the org's
//!    `params.contract` carries the transit lanes.** The `ports` half is the
//!    container convention (Wave 4 preamble): a container is **open**, because
//!    the mutation that instantiates a member into it draws edges to that
//!    member and a sealed hive refuses exactly those endpoints with
//!    `hive_port_boundary`. No slot either — W4's slot governs an address that
//!    does **not** exist, and `members` does exist
//!    (`crates/meclaw-colony/src/colony.rs`, `unbound_slot_behaviour`; receipt
//!    `unbound_slot_behaviour` in `colony.rs`), so a slot declaration
//!    here would be silent, and writing `params.ports` for its sake would
//!    **seal** the namespace instead.
//!
//! The last one is a question about the SUBSTRATE and is asked of the real
//! mutation path:
//!
//! 5. **A hive template with zero cells instantiates.** Every composite the
//!    library ships so far carries at least one cell; this is the first one
//!    that carries none, and whether the mutation pipeline commits a diff whose
//!    `add_nodes` registers nothing is not knowable from the template. If this
//!    test goes red, the finding is about the substrate, not about the
//!    template: the answer is **not** to pad `templates/org` with a cell until
//!    the mutation passes — that would trade the level's definition for a green
//!    test. Stop, record the rejection's `error_code` and `details`, and
//!    escalate.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_colony::config::HiveParams;
use meclaw_colony::{ColonyMsg, MutationOutcome, bootstrap_from_filesystem};
use meclaw_core::Uuid;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::ColonyHandle;
use tokio::sync::oneshot;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `templates/org`, or `None` when this tree did not ship it (GH #49).
fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("templates/org");
    p.join("config.json").is_file().then_some(p)
}

/// Parse one `config.json` into JSON, blaming the file by name.
fn config_at(dir: &std::path::Path) -> Value {
    let p = dir.join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every directory at or below `dir` that carries a `config.json`, sorted, as
/// paths relative to `dir` (the root itself is the empty string).
fn cell_directories(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    collect(dir, dir, &mut out);
    out.sort();
    out
}

fn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    if dir.join("config.json").is_file() {
        out.push(
            dir.strip_prefix(root)
                .expect("inside the walk root")
                .display()
                .to_string(),
        );
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(root, &p, out);
        }
    }
}

// ─────────────────────────────────────── (a) one container, and nothing under it

#[test]
fn the_level_carries_exactly_one_container_and_it_is_called_members() {
    let Some(org) = shipped() else { return };

    let mut children: Vec<String> = std::fs::read_dir(&org)
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            (p.is_dir() && p.join("config.json").is_file())
                .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    children.sort();
    assert_eq!(
        children,
        vec!["members".to_string()],
        "an org level owns exactly one thing — the container its members are instantiated \
         into. A second child is a sibling this level did not have to own."
    );

    let members = org.join("members");
    let grandchildren: Vec<String> = std::fs::read_dir(&members)
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            p.is_dir()
                .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        grandchildren.is_empty(),
        "the container ships EMPTY — a member is instantiated into it, never shipped inside \
         it: {grandchildren:?}"
    );
}

// ───────────────────────────── (b)+(c) two hives, and no cell of any other type

#[test]
fn every_config_in_the_level_is_a_hive_and_there_are_exactly_two() {
    let Some(org) = shipped() else { return };

    let dirs = cell_directories(&org);
    assert_eq!(
        dirs,
        vec!["".to_string(), "members".to_string()],
        "the level is the org hive plus its container, and nothing else carries a config.json"
    );

    for rel in &dirs {
        let cfg = config_at(&org.join(rel));
        let ty = cfg
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str);
        assert_eq!(
            ty,
            Some("hive"),
            "templates/org/{rel}: cell type is {ty:?} — an org that grows a cell has stopped \
             being a namespace (GH #302). Both levels are scope markers; the only actors in \
             this tree are the members instantiated into it."
        );
    }
}

// ────────────────────── (d) no ports key anywhere, and the transit lanes declared

#[test]
fn neither_hive_carries_a_ports_key() {
    let Some(org) = shipped() else { return };

    for rel in ["", "members"] {
        let cfg = config_at(&org.join(rel));
        let ports = cfg.get("params").and_then(|p| p.get("ports"));
        assert!(
            ports.is_none(),
            "templates/org/{rel}: declares params.ports = {ports:?}. A container is OPEN by \
             the container convention — the mutation that instantiates a member draws edges \
             to that member, and a sealed hive refuses exactly those endpoints with \
             `hive_port_boundary`. There is no slot to declare either: a slot governs an \
             address that does not exist, and this container does \
             (unbound_slot_behaviour, colony.rs)."
        );
    }
}

/// The `params` of one shipped hive `config.json`, as the substrate reads it.
fn hive_params(dir: &std::path::Path) -> HiveParams {
    let cfg = config_at(dir);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{}: the hive has no params", dir.display()));
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}: params: {e}", dir.display()))
}

/// The lane routes of one contract, in declaration order.
fn lanes(hp: &HiveParams) -> (Vec<String>, Vec<String>) {
    let c = hp
        .contract
        .as_ref()
        .expect("a level a caller addresses by path and lane owes a contract");
    (
        c.accepts.iter().map(|l| l.route.clone()).collect(),
        c.emits.iter().map(|l| l.route.clone()).collect(),
    )
}

#[test]
fn every_transit_lane_says_it_is_a_boundary_and_names_where_it_came_from() {
    let Some(org) = shipped() else { return };
    let hp = hive_params(&org);
    let contract = hp.contract.as_ref().expect("params.contract");

    // The version the union was derived from, read off the tree rather than
    // written down here — see `the_level_declares_the_union_of_its_occupants_lanes`.
    let member_ref = member_reference();

    for l in contract.accepts.iter().chain(contract.emits.iter()) {
        assert!(
            !l.because.trim().is_empty(),
            "lane '{}' says nothing about why it crosses the level",
            l.route
        );
        assert!(
            l.because.contains("namespace") || l.because.contains("boundary"),
            "lane '{}': the `because` of a transit lane says that this level is a boundary \
             and a namespace, not a participant. It says instead: {:?}",
            l.route,
            l.because
        );
        if let Some(reference) = member_ref.as_deref() {
            assert!(
                l.because.contains(reference),
                "lane '{}': a container level's lanes are DERIVED, and the derivation rule \
                 says to name the version they were derived from. This one does not mention \
                 `{reference}`: {:?}",
                l.route,
                l.because
            );
        }
    }
}

#[test]
fn every_internal_edge_is_one_transit_lane_crossing_the_container() {
    let Some(org) = shipped() else { return };
    let hp = hive_params(&org);
    let (accepts, emits) = lanes(&hp);

    for e in &hp.graph.edges {
        let endpoints = (e.from.as_str(), e.to.as_str());
        assert!(
            endpoints == (".", "./members") || endpoints == ("./members", "."),
            "templates/org: the edge {} -> {} routes something other than a transit lane \
             across the container. Below `./members` there is nothing to route to until a \
             member is instantiated, and the mutation that instantiates one draws its own \
             edges.",
            e.from,
            e.to
        );
        assert!(
            !e.is_default,
            "templates/org: {} -> {} is a default edge. Nothing here chooses; there is \
             nothing to choose between.",
            e.from, e.to
        );
    }

    // One door per accepted lane, one exit per emitted lane, and nothing else.
    // Without them the contract would be a promise nothing keeps: a declared
    // lane with no door is `hive_contract` at the next mutation the colony
    // runs, and a door with no lane is an undocumented one.
    let doors: Vec<&str> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.from == ".")
        .map(|e| route_of(e.condition.as_deref(), &e.from, &e.to))
        .collect();
    let exits: Vec<&str> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.to == ".")
        .map(|e| route_of(e.condition.as_deref(), &e.from, &e.to))
        .collect();
    assert_eq!(
        doors, accepts,
        "one door per accepted lane, in the contract's own order"
    );
    assert_eq!(
        exits, emits,
        "one exit per emitted lane, in the contract's own order"
    );
    assert_eq!(
        hp.graph.edges.len(),
        accepts.len() + emits.len(),
        "the level's whole behaviour is its transit edges: {} lanes, {} edges",
        accepts.len() + emits.len(),
        hp.graph.edges.len()
    );
}

/// The single `hop.route` value an edge's condition names.
///
/// Every edge of this level is written in one shape on purpose — a namespace
/// translates nothing and decides nothing, so its conditions are the plainest
/// form the library uses. Anything else is a finding, not a spelling.
fn route_of<'a>(condition: Option<&'a str>, from: &str, to: &str) -> &'a str {
    let c = condition
        .unwrap_or_else(|| panic!("templates/org: the edge {from} -> {to} carries no condition"));
    let want = "has(hop.route) && hop.route == '";
    let rest = c.strip_prefix(want).unwrap_or_else(|| {
        panic!("templates/org: {from} -> {to}: unexpected condition shape {c:?}")
    });
    rest.strip_suffix('\'').unwrap_or_else(|| {
        panic!("templates/org: {from} -> {to}: unexpected condition shape {c:?}")
    })
}

// ───────────────────── the boundary itself: the union of the occupant's lanes

/// `member@<version>` as the tree declares it, or `None` when this checkout
/// does not carry the template (GH #49 — the export ships a subset).
fn member_reference() -> Option<String> {
    let raw = std::fs::read_to_string(repo("templates/member/template.json")).ok()?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    let name = v.get("name")?.as_str()?;
    let version = v.get("version")?.as_str()?;
    Some(format!("{name}@{version}"))
}

/// The defect this test exists for, and the reason it reads the neighbour's
/// file instead of a list written here.
///
/// The level's lanes are DERIVED: *a container level declares the union of what
/// its occupants accept and emit, minus the lanes a sibling inside the level
/// consumes itself.* Inside an org there is only the container, so nothing is
/// subtracted and the union is the whole of both of the member's lists.
///
/// This level was first authored against a `member` template that did not exist
/// yet, and it guessed — `in_turn` in, `turn`/`error` out. Every third of that
/// guess was wrong in a way nothing would have reported: `turn` is a lane **no
/// member ever emits** (a member consumes its own turn internally,
/// `./assistants -> ./firewall`), so the exit edge carrying it could never
/// fire; and `answer`, `ack`, `reject`, `in_recall`, `in_brief`, `in_propose`
/// were missing, so a member's answer and — worse — a member's *refusal* would
/// have died as `no_route` at a level boundary, which is exactly the silent
/// swallowing GH #284 has just finished taking out of this tree.
///
/// A list written down here would have to be re-derived by hand every time the
/// occupant moves, which is the thing that already failed once. Reading both
/// files makes the boundary a pinned fact: move a lane in `member` and this
/// goes red, instead of a message quietly disappearing.
#[test]
fn the_level_declares_the_union_of_its_occupants_lanes() {
    let Some(org) = shipped() else { return };
    let member = repo("templates/member");
    if !member.join("config.json").is_file() {
        // The public export ships a subset; an occupant that did not travel is
        // not a defect.
        return;
    }

    let (org_accepts, org_emits) = lanes(&hive_params(&org));
    let (member_accepts, member_emits) = lanes(&hive_params(&member));

    assert_eq!(
        org_accepts, member_accepts,
        "the accepts of `org` must be the union of its occupants' accepts, and its only \
         occupant is `member`. An accepted lane the member does not have is an interface \
         that lies; one it has and this level does not is a message that dies as `no_route` \
         at the boundary."
    );
    assert_eq!(
        org_emits, member_emits,
        "the emits of `org` must be the union of its occupants' emits, and its only occupant \
         is `member`. `reject` is the sharp one: a refusal that cannot leave the namespace is \
         a refusal swallowed at a level boundary (GH #284)."
    );
    assert!(
        !org_emits.iter().any(|l| l == "turn"),
        "`turn` is not a lane a member emits — it is consumed inside the member \
         (./assistants -> ./firewall). An org that declares it carries an exit that can \
         never fire: {org_emits:?}"
    );
}

// ───────────────── the substrate question: does a zero-cell hive template grow?

async fn mutate(h: &ColonyHandle, payload: Value) -> MutationOutcome {
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

/// Copy a directory tree verbatim — the template must arrive as it ships.
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// A RED HERE IS A SUBSTRATE FINDING, NOT A TEMPLATE ONE.
///
/// `templates/org` is the first shipped template whose instantiation registers
/// no cell at all. If the mutation is rejected, the repair is **not** to add a
/// cell to the template so the diff has something to register — that would
/// give the level an actor and end its definition. Record the `error_code` and
/// the `details` the panic prints, stop, and escalate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_template_with_zero_cells_instantiates() {
    let Some(source) = shipped() else { return };

    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();

    // The smallest colony that can receive a mutation: a root hive with an
    // empty graph. What is under test is what the mutation ADDS.
    std::fs::create_dir_all(root.join("main")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(root.join(".env"), "").unwrap();
    copy_tree(&source, &root.join("templates/org"));

    // NO factories, and that is an assertion in itself: a namespace needs no
    // cell type, so the colony that grows one needs no factory registry beyond
    // the empty default.
    let h = ColonyHandle::new_with_factories_at(&td, Vec::new());
    bootstrap_from_filesystem(
        root,
        &meclaw_colony::CellFactoryRegistry::new(),
        &h.runtime(),
    )
    .await
    .expect("the empty root must boot");

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack");

    // Pinned by version rather than by bare name: a bare `<name>` resolves to
    // the highest version present, which is the drift `template_chain` exists
    // to make visible (the ref-resolution receipt of GH #277).
    let declared: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(source.join("template.json")).unwrap(),
    )
    .unwrap();
    let version = declared
        .get("version")
        .and_then(Value::as_str)
        .expect("templates/org/template.json declares a string version");
    let reference = format!("org@{version}");

    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {
            "add_nodes": [{"name": "acme", "template": reference}]
        }}),
    )
    .await;

    match &outcome {
        MutationOutcome::Committed { .. } => {}
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => panic!(
            "a hive template with ZERO cells was refused: {error_code} — {details}\n\
             This is a SUBSTRATE finding, not a template defect. Do NOT add a cell to \
             templates/org to make this pass: an org that grows a cell has stopped being a \
             namespace (GH #302). Record this rejection and escalate."
        ),
    }

    // `Committed` on a diff that built nothing would be a vacuous green, so the
    // tree is read back three ways.
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: meclaw_core::Path::new("/"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let graph = ack_rx.await.unwrap();

    // 1. No node anywhere under the level. A hive is a scope marker and never a
    //    registry row, and this level carries nothing else — so the whole
    //    instantiation must be invisible to the node projection.
    let nodes: Vec<&str> = graph.nodes.iter().map(|n| n.path.as_str()).collect();
    assert!(
        !nodes.iter().any(|p| p.starts_with("/acme")),
        "the namespace grew an actor: {nodes:?}"
    );

    // 2. Both paths exist as routable endpoints: the level's own pass-through
    //    pair survived the instantiation, and the edge table only ever admits
    //    endpoints the mutation resolved.
    let wired: Vec<(&str, &str)> = graph
        .edges
        .iter()
        .map(|e| (e.from.as_str(), e.to.as_str()))
        .collect();
    assert!(
        wired.contains(&("/acme", "/acme/members")),
        "the door into the container is gone: {wired:?}"
    );
    assert!(
        wired.contains(&("/acme/members", "/acme")),
        "the exit out of the container is gone: {wired:?}"
    );

    // 3. Both directories were staged into the colony root.
    for rel in ["main/acme/config.json", "main/acme/members/config.json"] {
        assert!(
            root.join(rel).is_file(),
            "{rel} was not staged — the container is part of the level, not an optional extra"
        );
    }

    h.shutdown().await;

    // 4. And both are hive SCOPES, which is what "the path exists" means for a
    //    level: `<org>` and `<org>/members` are addresses a later mutation can
    //    instantiate a member into.
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    let mut stmt = conn
        .prepare("SELECT path FROM hive_scopes ORDER BY path")
        .unwrap();
    let scopes: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for want in ["/acme", "/acme/members"] {
        assert!(
            scopes.iter().any(|s| s == want),
            "{want} is not a hive scope: {scopes:?}"
        );
    }
}
