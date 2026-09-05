//! GH #302 / GH #122 / GH #454 — `member@1.3.0` holds what two assistants of
//! one person must share.
//!
//! The level rule of this wave is *a level owns what its siblings must share*,
//! and the member is the level where that rule stops being an abstraction. Two
//! assistants of one person must know the **same person** — so the memory sits
//! here, not in either agent (GH #122, the three-holders ruling of 2026-08-19:
//! *the memory belongs to the member*). Two channels of one person must have
//! **one view of an attacker**, and a rate window must not restart because a
//! generation was replaced — so the firewall sits here too. Identity sits here
//! because a person's record and an agent's persona are the same document, read
//! by every assistant that person owns.
//!
//! # What GH #454 moved, and why
//!
//! Since 1.3.0 the CHANNELS are on that list as well, in a second open
//! container beside `assistants`. The argument is the same one, applied to the
//! thing everybody had left one level down: a bot a generation owns is one
//! agent's bot. *A member with two assistants sharing one channel* could not be
//! built at all — the person's second agent was not reachable through the first
//! one's account, a generation swap took the chat account with it, and a screen
//! both agents draw on had no owner to hang from. So a channel belongs to the
//! PERSON, an assistant is what a channel ADDRESSES (`context.assistant`,
//! stamped by the channel's own outbound edge), and the answer finds its way
//! back by `context.channel_node` rather than by having been born inside the agent
//! that produced it.
//!
//! Everything below is a fact about the FILES — no colony, no runtime, the same
//! reasoning as `gh173_shipped_hive_contracts`: whether a class's declared
//! interface matches the classes it composes is checkable before anything is
//! ever instantiated.
//!
//! 1. **Six children: three `ref`s, two containers and one cell of its own.**
//!    The refs are named exactly like the templates they pull in (the standing
//!    naming rule — an instance name that differs from its template name is
//!    drift, not intention), and each pins an exact version: a bare `<name>`
//!    resolves to the highest one present, which is precisely the drift
//!    `template_chain` exists to make visible (the ref-resolution receipt of
//!    GH #277).
//! 2. **No `memory-drain` and no `terminal`.** Per-turn extraction (GH #298,
//!    ruling Q11) removed the drain; ruling Q2 (GH #284) removed the sink — a
//!    refusal either has a consumer that records it or no edge at all, and this
//!    level chose the second.
//! 3. **No `params.ports` key anywhere in the template.** Both containers are
//!    OPEN because the mutation that instantiates an assistant or a channel into
//!    one draws edges to that node, and a sealed hive refuses exactly those
//!    endpoints with `hive_port_boundary`. And no slot: W4's slot governs an
//!    address that does **not** exist, while these containers do
//!    (`crates/meclaw-colony/src/colony.rs`, `unbound_slot_behaviour`; receipt
//!    `unbound_slot_behaviour` in `colony.rs`). Writing `params.ports` on
//!    the member for a slot's sake would **seal** the member — and the member is
//!    a level that gets wired into.
//! 4. **Every edge that states a lane into an occupant promotes what that lane
//!    requires.** Since GH #291 a key listed in a hive lane's
//!    `contract.accepts[].context` is enforced: the edge must promote it in its
//!    own `modifier.set_context` or have a setter reachable upstream
//!    (`crates/meclaw-colony/src/mutation/validate.rs`,
//!    `addressed_hive_lane_context`). At this level `.` is the door and nothing
//!    is upstream of it, so **the edge itself is the only setter root**. The
//!    round key is `audience_set` and nothing else — `participants` is
//!    **retired, not aliased** under GH #330.
//! 5. **No lane that the occupant no longer has.** The three contracts are read
//!    off the tree and the member's own wiring is checked against them, so a
//!    lane a template renamed (`in_flush` → `in_close_pass`, GH #300) cannot
//!    survive here as a copied literal.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_colony::config::HiveParams;
use meclaw_core::serde_json::Value;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// `templates/member`, or `None` when this tree did not ship it (GH #49).
fn shipped() -> Option<std::path::PathBuf> {
    let p = repo("templates/member");
    p.join("config.json").is_file().then_some(p)
}

/// Parse one `config.json` into JSON, blaming the file by name.
fn config_at(dir: &std::path::Path) -> Value {
    let p = dir.join("config.json");
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// The `params` of a hive `config.json`, through the substrate's own reader.
fn hive_params(dir: &std::path::Path) -> HiveParams {
    let cfg = config_at(dir);
    let params = cfg
        .get("params")
        .cloned()
        .unwrap_or_else(|| panic!("{}: no params block", dir.display()));
    meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{}/config.json: params: {e}", dir.display()))
}

/// The version a shipped top-level template declares, as a string.
fn declared_version(name: &str) -> Option<String> {
    let p = repo("templates").join(name).join("template.json");
    let raw = std::fs::read_to_string(p).ok()?;
    let v: Value = meclaw_core::serde_json::from_str(&raw).ok()?;
    v.get("version").and_then(Value::as_str).map(str::to_string)
}

/// `(accepts, emits)` of a shipped top-level hive template, read off the tree.
fn lanes_of(name: &str) -> (Vec<String>, Vec<String>) {
    let hp = hive_params(&repo("templates").join(name));
    let c = hp
        .contract
        .unwrap_or_else(|| panic!("templates/{name} declares no params.contract"));
    (
        c.accepts.iter().map(|l| l.route.clone()).collect(),
        c.emits.iter().map(|l| l.route.clone()).collect(),
    )
}

/// The `context` keys one lane of a shipped hive template requires.
fn required_context(name: &str, route: &str) -> Vec<String> {
    let hp = hive_params(&repo("templates").join(name));
    let c = hp.contract.expect("contract");
    c.accepts
        .iter()
        .find(|l| l.route == route)
        .unwrap_or_else(|| panic!("templates/{name} has no lane '{route}' any more"))
        .context
        .clone()
}

/// A single-quoted CEL string literal, or `None` for anything computed. Same
/// reading as `hive_contract::constant_route`.
fn constant(src: &str) -> Option<&str> {
    let t = src.trim();
    let inner = t.strip_prefix('\'')?.strip_suffix('\'')?;
    (!inner.contains('\'')).then_some(inner)
}

/// True iff this edge's condition names `route` the way every door and exit in
/// this template writes it.
fn condition_names(e: &meclaw_colony::config::EdgeSpec, route: &str) -> bool {
    let needle = format!("hop.route == '{route}'");
    e.condition
        .as_deref()
        .is_some_and(|c| c.contains(needle.as_str()))
}

/// The single `hop.route` literal this edge's condition names, if it names
/// exactly one — the way an unmodified door names the lane it carries.
fn stated_lane(e: &meclaw_colony::config::EdgeSpec) -> Option<&str> {
    let c = e.condition.as_deref()?;
    let at = c.find("hop.route == '")?;
    let rest = &c[at + "hop.route == '".len()..];
    let end = rest.find('\'')?;
    Some(&rest[..end])
}

/// The constant lane this edge STAMPS on what it takes, if any — the second way
/// an edge can name a lane (GH #176).
fn stamped_lane(e: &meclaw_colony::config::EdgeSpec) -> Option<&str> {
    e.modifier
        .as_ref()
        .and_then(|m| m.set_hop.get("route"))
        .and_then(|s| constant(s.as_str()))
}

// ────────────────── (a) eight children: four refs, three containers, one cell

#[test]
fn the_level_carries_four_refs_and_three_containers() {
    let Some(member) = shipped() else { return };

    let mut children: Vec<String> = std::fs::read_dir(&member)
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
        vec![
            "access".to_string(),
            "affinity".to_string(),
            "apps".to_string(),
            "assistants".to_string(),
            "channels".to_string(),
            "firewall".to_string(),
            "memory-hive".to_string(),
        ],
        "the member owns exactly four holders — the memory, the record, the screen and, \
         since 1.5.0, the `access` that holds this person's own provider credentials \
         (GH #560) — and THREE containers — `assistants`, and since 1.3.0 `channels` \
         (GH #454) and `apps` (GH #459). It owns NO cell of its own any more: since \
         1.6.0 each holder's store writes its own seed set through the `transfer` slot \
         (GH #555), so the one `code` cell that used to file somebody else's export is \
         gone with the lane it drained. A FIFTH holder is something the siblings did not \
         have to share; a missing one is something an assistant would have to hold \
         itself. `apps` is a container and not a holder for the same reason `channels` \
         is not: what stands in it is instantiated per person, and an app writes VIEWS \
         onto this member's screen rather than being something the assistants read."
    );

    // Adding a container and the lanes around it is an ADDITION, which is the
    // second digit (docs/development-rules.md § 4). The digit is read off the
    // tree, not written down here, so a later repair of this template does not
    // go red for a reason that has nothing to do with GH #454.
    let version = declared_version("member").unwrap_or_default();
    let mut digits = version.split('.');
    let major = digits.next().unwrap_or_default().to_string();
    let minor: u32 = digits.next().unwrap_or_default().parse().unwrap_or(0);
    assert!(
        major == "1" && minor >= 3,
        "templates/member/template.json says {version:?}. `channels` (GH #454) and `apps` \
         (GH #459) are new addresses with new edges around them, and nothing was taken \
         away — that is the second digit, and it must never go back below 1.3. Both \
         landed in the SAME unreleased 1.3.0: a version is a shipped fact, and neither \
         has shipped yet."
    );

    // And the level owns no cell at all: GH #555 moved the one it had into the
    // substrate, where a cell writes its OWN files and nobody else's.
    assert!(
        !member.join("export-sink").exists(),
        "the member level still ships the cell that filed somebody else's export"
    );

    // The instance name equals the template name — the standing naming rule.
    // A ref whose directory is called something else is drift, not intention.
    for name in ["affinity", "firewall", "memory-hive"] {
        let cfg = config_at(&member.join(name));
        let cell = cfg.get("cell").expect("cell block");
        assert_eq!(
            cell.get("type").and_then(Value::as_str),
            Some("ref"),
            "templates/member/{name}: a holder is pulled in by reference, never copied"
        );
        let reference = cell
            .get("template")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("templates/member/{name}: cell.template is missing"));
        let Some(version) = declared_version(name) else {
            continue; // GH #49 — the occupant did not travel into this tree
        };
        assert_eq!(
            reference,
            format!("{name}@{version}"),
            "templates/member/{name}: the ref must pin the exact version on disk. A bare \
             `<name>` resolves to the highest one present, and a level that silently adopts a \
             new holder is the drift `template_chain` exists to make visible, not to excuse."
        );
    }

    // All three containers ship EMPTY: an assistant, a channel and an app are
    // instantiated into them, never shipped inside them. Shipping one would also
    // pin a version this level has no business pinning — `display@1.0.0` and
    // `colony-view@1.0.0` are library templates a mutation names, and a ref on a
    // template that did not travel refuses the mutation that carries it.
    for container in ["apps", "assistants", "channels"] {
        let inside: Vec<String> = std::fs::read_dir(member.join(container))
            .unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                p.is_dir()
                    .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
            })
            .collect();
        assert!(
            inside.is_empty(),
            "the {container} container ships empty: {inside:?}"
        );
        assert_eq!(
            config_at(&member.join(container))
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(Value::as_str),
            Some("hive"),
            "templates/member/{container} is a real hive — an address that already exists, \
             so the mutation that instantiates a node has somewhere to put one"
        );
    }
}

// ─────────────────────────────────────── (b) no drain, no sink, anywhere in here

#[test]
fn nothing_in_this_level_is_a_drain_or_a_sink() {
    let Some(member) = shipped() else { return };

    for entry in std::fs::read_dir(&member).unwrap() {
        let p = entry.unwrap().path();
        if !p.is_dir() || !p.join("config.json").is_file() {
            continue;
        }
        let cfg = config_at(&p);
        let cell = cfg.get("cell").expect("cell block");
        let ty = cell.get("type").and_then(Value::as_str).unwrap_or("");
        assert_ne!(
            ty,
            "terminal",
            "{}: a terminal here would swallow a refusal. Ruling Q2 (GH #284): the DLQ is \
             the record.",
            p.display()
        );
        let reference = cell.get("template").and_then(Value::as_str).unwrap_or("");
        let name = reference.split('@').next().unwrap_or("");
        assert!(
            name != "memory-drain" && name != "terminal",
            "{}: resolves to `{name}`. Per-turn extraction (GH #298, ruling Q11) removed the \
             drain, and #302 says explicitly that it does not belong in the assistant either; \
             ruling Q2 (GH #284) removed the sink.",
            p.display()
        );
    }
}

// ──────────────────────────────────────────────── (c) no ports key, and no slot

#[test]
fn no_hive_of_this_template_carries_a_ports_key() {
    let Some(member) = shipped() else { return };

    for rel in ["", "apps", "assistants", "channels"] {
        let cfg = config_at(&member.join(rel));
        let ports = cfg.get("params").and_then(|p| p.get("ports"));
        assert!(
            ports.is_none(),
            "templates/member/{rel}: declares params.ports = {ports:?}. A container is OPEN \
             because the mutation that instantiates an assistant or a channel draws edges to \
             it, and a sealed hive refuses exactly those endpoints with `hive_port_boundary`. \
             There is no slot to declare either — a slot governs an address that does not \
             exist, and both containers do (unbound_slot_behaviour, colony.rs) — and writing \
             the key on the MEMBER would seal a level that gets wired into."
        );
    }
}

/// Orchestrator ruling W7-R2 (2026-08-25), pinned here because this is the
/// level whose own edges wire its container from birth.
///
/// A container hive that its own level wires declares **no** `params.contract`.
/// `addressed_lane_doors` skips a hive only while NOTHING addresses its path
/// (`hive_path_is_wired`); this member wires `./assistants` many times over and,
/// since 1.3.0, `./channels` three times, so both containers are wired the
/// moment the member is instantiated. From then on every declared `accepts` lane
/// owes a `door_exists` — a message arriving at the container path must reach a
/// cell **inside** it — and an empty container has no inside. The violation is
/// collected on **every** mutation of the colony, not only on the one that
/// touches this member, so a contract here would lock the colony for exactly as
/// long as the member has no assistant (or no channel) yet, which is a perfectly
/// legitimate intermediate state.
#[test]
fn neither_container_declares_a_contract_because_the_level_wires_them() {
    let Some(member) = shipped() else { return };

    for container in ["apps", "assistants", "channels"] {
        let cfg = config_at(&member.join(container));
        let contract = cfg.get("params").and_then(|p| p.get("contract"));
        assert!(
            contract.is_none(),
            "templates/member/{container} declares params.contract = {contract:?}. The \
             member's own edges address this container from birth, so `hive_path_is_wired` is \
             true and every declared lane owes a door INSIDE the container — which an empty \
             container cannot have. The transit lanes belong on the level whose edges satisfy \
             the check, and what an instantiating mutation must wire belongs in \
             `description`, which the substrate does not enforce against an address nobody \
             stands at yet."
        );
    }

    // The other half of the same rule: what the member DOES declare, it can
    // open. Every accepts lane has a door edge out of `.`, every emits lane an
    // edge from inside back to `.`.
    let hp = hive_params(&member);
    let c = hp
        .contract
        .as_ref()
        .expect("the member declares a contract");
    for lane in &c.accepts {
        if !lane.at.is_empty() {
            // GH #559 / #562 — a lane that names connect points docks BELOW this
            // rim by declaration, so its door is that address rather than an
            // edge out of `.`. The substrate reads it the same way
            // (`hive_contract::docks_below_the_rim`), and the connect points
            // themselves are policed per edge by the v-lane rule table.
            continue;
        }
        let doored = hp
            .graph
            .edges
            .iter()
            .any(|e| e.from == "." && condition_names(e, &lane.route));
        assert!(
            doored,
            "the member accepts `{}` and has no door edge out of `.` for it — a declared lane \
             with no door is `hive_contract` at the next mutation of the colony",
            lane.route
        );
    }
    for lane in &c.emits {
        if !lane.at.is_empty() {
            continue; // the same rule, one direction over
        }
        let exits = hp.graph.edges.iter().any(|e| {
            e.to == "."
                && (condition_names(e, &lane.route) || stamped_lane(e) == Some(lane.route.as_str()))
        });
        assert!(
            exits,
            "the member emits `{}` and no edge carries it back out to `.`",
            lane.route
        );
    }
}

// ───────────────────────── (d) every stated lane promotes the context it requires

/// The rule, in one test: an edge of this template whose `modifier.set_hop.route`
/// is a constant naming a lane of one of the three occupants must promote every
/// `context` key that lane declares — because at this level `.` is the door and
/// nothing is upstream of it, so the edge is the only setter root
/// (`addressed_hive_lane_context`, GH #291).
#[test]
fn every_edge_that_states_a_lane_promotes_what_the_lane_requires() {
    let Some(member) = shipped() else { return };
    let hp = hive_params(&member);

    let mut judged = 0usize;
    for e in &hp.graph.edges {
        let Some(occupant) = e.to.strip_prefix("./") else {
            continue;
        };
        if !["affinity", "firewall", "memory-hive"].contains(&occupant) {
            continue;
        }
        let Some(modifier) = e.modifier.as_ref() else {
            continue;
        };
        let Some(route) = stamped_lane(e) else {
            continue;
        };
        if declared_version(occupant).is_none() {
            continue; // GH #49
        }
        judged += 1;
        for key in required_context(occupant, route) {
            assert!(
                modifier.set_context.contains_key(&key),
                "templates/member: the edge {} -> {} states hop.route='{route}', whose lane \
                 requires context '{key}', and the edge does not promote it. Nothing is \
                 upstream of `.` at this level, so this edge is the only setter root — the \
                 mutation that instantiates this member would be refused with `hive_contract`.",
                e.from,
                e.to
            );
        }
    }
    assert!(
        judged >= 3,
        "the sweep judged almost no lane-stating edge: {judged}"
    );
}

/// The memory lane in particular, because it is what this level exists for and
/// because the round has exactly one spelling. `participants` was **retired,
/// not aliased** under GH #330: a request that spells the round that way
/// declared no round at all and is refused like any other undeclared one.
#[test]
fn the_memory_lane_carries_the_audience_set_and_never_participants() {
    let Some(member) = shipped() else { return };
    let hp = hive_params(&member);

    let mut writes = 0usize;
    for e in &hp.graph.edges {
        if e.to != "./memory-hive" {
            continue;
        }
        let Some(modifier) = e.modifier.as_ref() else {
            continue;
        };
        assert!(
            !modifier.set_context.contains_key("participants"),
            "templates/member: the edge {} -> {} promotes `participants`. It is retired, not \
             aliased (GH #330) — a request that spells the round that way declared no round \
             at all.",
            e.from,
            e.to
        );
        if stamped_lane(e) == Some("in_remember") {
            writes += 1;
            assert!(
                modifier.set_context.contains_key("audience_set"),
                "templates/member: the write half of the memory lane must carry the round the \
                 block was learned in — an untagged row is a row that may be told to anyone"
            );
        }
    }
    assert_eq!(
        writes, 1,
        "exactly one edge turns per-turn extraction into the hive's `in_remember` lane — \
         `talky`'s own shipped recipe is two edges, never one, and the second is the hive's \
         `reject` egress out of this level"
    );
}

// ───────────────────── the wiring itself: the pairs that make the level a level

#[test]
fn the_level_wires_the_screen_the_memory_and_the_record() {
    let Some(member) = shipped() else { return };
    let hp = hive_params(&member);

    let wired: Vec<(String, String, String)> = hp
        .graph
        .edges
        .iter()
        .map(|e| {
            (
                e.from.clone(),
                e.to.clone(),
                stamped_lane(e).unwrap_or("").to_string(),
            )
        })
        .collect();
    let has = |from: &str, to: &str, stamps: &str| {
        wired
            .iter()
            .any(|(f, t, r)| f == from && t == to && r == stamps)
    };

    // The screen. Since GH #454 the unscreened turn comes up out of the CHANNELS
    // container — the raw wire is the person's, not one generation's — and the
    // screened one goes down into the assistants container, where the
    // per-assistant guards on `context.assistant` decide which agent was meant.
    assert!(
        has("./channels", "./firewall", "in_turn"),
        "no unscreened turn reaches the screen from a channel of this person. Since GH #454 \
         this is the ingress: a bot is the member's, and its raw `turn` is re-stamped to \
         `in_turn` on the way into the firewall"
    );
    assert!(
        !wired
            .iter()
            .any(|(f, t, _)| f == "./assistants" && t == "./firewall"),
        "an assistant still sends a turn to the screen. The raw wire left the generation \
         with GH #454; what comes back OUT of ./assistants is an `answer`, and a level that \
         kept both doors would screen the same turn twice or none"
    );
    assert!(
        has("./firewall", "./assistants", "in_turn"),
        "the screened turn never gets down to the assistant it was addressed to"
    );
    assert!(
        has(".", "./firewall", ""),
        "a turn entering from outside the member reaches nothing"
    );

    // The way back. The answer is addressed by `context.channel_node` into the
    // person's channels container; a channel's own failure leaves the level on
    // the member's error lane, because the connector is the member's now and its
    // errors are not an assistant's.
    assert!(
        wired
            .iter()
            .any(|(f, t, _)| f == "./assistants" && t == "./channels"),
        "an assistant's answer cannot reach a channel of this person — the lane GH #454 \
         added and the reason the channel came up here at all"
    );
    assert!(
        wired.iter().any(|(f, t, _)| f == "./channels" && t == "."),
        "a channel's own failure cannot leave the level"
    );

    // The screen, since GH #459. A display is a channel like any other, so it
    // needs no lane of its own on the way down — the answer edge above carries a
    // prose view, which is the smallest view there is. What it DOES need is a
    // way back for the two lanes only a screen has: `event` (a person acted on a
    // view) and `receipt` (a write was refused). Both are addressed by the
    // OWNER the display stamped, which is the path of the cell that put the view
    // up, and the level splits on where that path lies.
    let owner_split = |to: &str, needle: &str| {
        hp.graph.edges.iter().any(|e| {
            e.from == "./channels"
                && e.to == to
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains("hop.owner") && c.contains(needle))
        })
    };
    assert!(
        owner_split("./assistants", "/assistants/"),
        "a browser event has no way back to the agent whose view it was. The display stamps \
         the owner on the HOP as well as in the body precisely so this edge can exist — an \
         edge condition reads `context.*` and `hop.*` and never the body \
         (`crates/meclaw-colony/src/cel_eval.rs`)"
    );
    assert!(
        owner_split("./apps", "/apps/"),
        "an application gets no browser event back. It writes views exactly the way an agent \
         does, and the return path is the same one — the only difference is which container \
         the owner path lies in"
    );
    assert!(
        hp.graph.edges.iter().any(|e| {
            e.from == "./apps"
                && e.to == "./channels"
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains("hop.route == 'view'"))
        }),
        "an application's view cannot reach a screen of this person. The app is display-blind \
         by design (templates/colony-view/README.md): which screen it draws on is named in ONE \
         literal, in the edge the instantiating mutation draws out of the app itself"
    );
    assert!(
        hp.graph.edges.iter().any(|e| {
            e.from == "./channels"
                && e.to == "."
                && e.condition
                    .as_deref()
                    .is_some_and(|c| c.contains("hop.route == 'receipt'"))
                && stamped_lane(e) == Some("error")
        }),
        "a receipt this level cannot attribute to an owner has nowhere to go. Dropping it \
         would make a screen refusing every write look exactly like a quiet one; the member \
         re-stamps it onto the `error` lane it already emits, so no new exit is owed to the \
         parent"
    );

    // The memory: one read pair, one write edge, one refusal out.
    assert!(
        has("./assistants", "./memory-hive", "in_query"),
        "an assistant cannot ask this member's memory anything"
    );
    assert!(
        has("./memory-hive", "./assistants", "in_bundle"),
        "the answer never gets back down to the assistant"
    );
    assert!(
        has("./assistants", "./memory-hive", "in_remember"),
        "per-turn extraction has nowhere to land: under GH #122 the memory is the MEMBER's, \
         so this is where `talky`'s shipped recipe sends it"
    );

    // Both refusals leave the level. Nobody inside consumes them, and that is
    // honest state (2) of GH #284.
    assert!(
        has("./firewall", ".", ""),
        "the firewall's `reject` is not wired out of the level"
    );
    assert!(
        has("./memory-hive", ".", ""),
        "the memory hive's `reject` is not wired out of the level — `memory-hive` declares \
         `required_drains` for `in_query`, `in_remember` and `in_episode`, and an undrained \
         refusal is the one failure that gate exists to prevent"
    );

    // The record.
    assert!(
        wired.iter().any(|(f, t, _)| f == "." && t == "./affinity"),
        "nothing can read or write this member's identity record"
    );
    assert!(
        wired.iter().any(|(f, t, _)| f == "./affinity" && t == "."),
        "the record's answer cannot leave the level"
    );
}

// ───────────────────────────── the drift-lock on the two doors into the screen

/// The one expression that promotes the channel onto a turn on its way into the
/// firewall. It is written here as a literal on purpose: this is a byte-lock,
/// and a lock that computed its own expectation would lock nothing.
const CHANNEL_PROMOTION: &str = "has(context.channel) ? context.channel : ''";

/// **Two doors into the screen, ONE stamp — byte for byte.**
///
/// A turn reaches the firewall on two edges now: from outside the member
/// (`. -> ./firewall` on `in_turn`, an operator, a digest, a second person's
/// agent) and from a channel of this person (`./channels -> ./firewall` on
/// `turn`, since GH #454). Both promote `context.channel` with the same CEL
/// expression, and this test compares the STRING rather than looking for a
/// family resemblance — the same reading as the corridor byte-gates, and for
/// the same reason: "looks equivalent" is not a measurement.
///
/// Why it is worth a lock of its own. `context.channel`, `context.user_id` and
/// `context.audience_set` are read three cells deep — the firewall buckets a
/// rate window by them, the memory hive filters a recall by the audience it was
/// learned in, and the channels container routes the answer back by the channel
/// name. A turn whose channel was lost on the way through a door does not
/// vanish, which is what makes the defect quiet: the promotion falls back to the
/// empty string, the turn goes through, and it lands in the SHARED bucket
/// instead of its own. One person hammering one bot then rate-limits every
/// unpromoted turn in the colony, and nothing anywhere reports an error. Two
/// doors that drifted apart by a character would produce exactly that, on one of
/// them only, which is the hardest version of it to see.
#[test]
fn both_doors_into_the_screen_stamp_the_channel_with_the_same_expression() {
    let Some(member) = shipped() else { return };
    let hp = hive_params(&member);

    let outer = hp
        .graph
        .edges
        .iter()
        .find(|e| e.from == "." && e.to == "./firewall" && condition_names(e, "in_turn"))
        .expect("the outer door: a turn entering from outside the member reaches the screen");
    let from_channel = hp
        .graph
        .edges
        .iter()
        .find(|e| e.from == "./channels" && e.to == "./firewall" && condition_names(e, "turn"))
        .expect(
            "the channel door: since GH #454 a raw `turn` comes up out of ./channels and is \
             re-stamped on its way into the screen",
        );

    assert_eq!(
        stamped_lane(from_channel),
        Some("in_turn"),
        "the channel door must re-stamp the connector's raw `turn` onto the firewall's own \
         inbound lane — a lane the screen does not declare is a message that dies at it"
    );

    let stamp = |e: &meclaw_colony::config::EdgeSpec, which: &str| -> String {
        e.modifier
            .as_ref()
            .and_then(|m| m.set_context.get("channel"))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "the {which} door ({} -> {}) promotes no `context.channel` at all. \
                     Nothing is upstream of it, so it is the only setter root: an unpromoted \
                     turn does not fail, it shares one rate bucket with every other \
                     unpromoted turn in the colony",
                    e.from, e.to
                )
            })
    };
    let outer_stamp = stamp(outer, "outer");
    let channel_stamp = stamp(from_channel, "channel");

    assert_eq!(
        outer_stamp, CHANNEL_PROMOTION,
        "the outer door's channel promotion drifted. The expression is a byte-lock: \
         `has(...)` guards a key the caller may not have sent, and the empty-string fallback \
         is what turns a missing channel into a shared bucket instead of a vanished turn."
    );
    assert_eq!(
        channel_stamp, outer_stamp,
        "the two doors into the screen promote `context.channel` with DIFFERENT expressions. \
         They are the same stamp on the same key at the same level, and the firewall cannot \
         tell which door a turn came through — a turn that loses its channel on one of them \
         is rate-limited in the wrong bucket, silently, on half the traffic."
    );

    // And no third door may appear that skips the stamp: every edge into the
    // screen that carries a turn carries the promotion too.
    for e in hp.graph.edges.iter().filter(|e| {
        e.to == "./firewall"
            && (condition_names(e, "in_turn") || stamped_lane(e) == Some("in_turn"))
    }) {
        assert_eq!(
            e.modifier
                .as_ref()
                .and_then(|m| m.set_context.get("channel"))
                .map(String::as_str),
            Some(CHANNEL_PROMOTION),
            "the edge {} -> ./firewall carries a turn into the screen without the one \
             channel promotion the other doors use",
            e.from
        );
    }
}

// ────────────────────── (e) no lane the occupant no longer has, contract or prose

#[test]
fn the_level_names_no_lane_its_occupants_lost() {
    let Some(member) = shipped() else { return };

    // Everything this template stamps into an occupant, and everything it reads
    // back out of one, measured against the occupant's contract on disk.
    let expected: [(&str, &[&str], &[&str]); 3] = [
        ("firewall", &["in_turn"], &["pass", "reject"]),
        (
            "memory-hive",
            &["in_query", "in_remember"],
            &["bundle", "reject"],
        ),
        (
            "affinity",
            &["in_brief", "in_propose"],
            &["answer", "ack", "error"],
        ),
    ];
    for (name, sends, reads) in expected {
        if declared_version(name).is_none() {
            continue; // GH #49
        }
        let (accepts, emits) = lanes_of(name);
        for lane in sends {
            assert!(
                accepts.contains(&(*lane).to_string()),
                "templates/member sends `{lane}` into `{name}`, which accepts {accepts:?}"
            );
        }
        for lane in reads {
            assert!(
                emits.contains(&(*lane).to_string()),
                "templates/member reads `{lane}` out of `{name}`, which emits {emits:?}"
            );
        }
    }

    // The lane W5 renamed (GH #300). A README that still names it is a page
    // describing a hive that no longer exists.
    let readme = std::fs::read_to_string(member.join("README.md")).expect("README.md");
    assert!(
        !readme.contains("in_flush"),
        "README.md names `in_flush`; `memory-hive` calls that lane `in_close_pass` since \
         GH #300, and this level does not send it at all"
    );
    assert!(
        readme.contains("a level owns what its siblings must share"),
        "the README carries the wave's level rule verbatim — Task 18 anchors an ADR on those \
         exact words"
    );
    assert!(
        readme.contains("unbound_slot_behaviour"),
        "the README names the receipt that explains why the container's unbound behaviour is \
         undeclared (unbound_slot_behaviour, colony.rs)"
    );
}

// ───────────────── the boundary to the level below: every lane, or a no_route

/// **The assertion whose absence let the defect through.**
///
/// `member@1.0.0` was authored against an `assistant` template that did not
/// exist yet, and it guessed which of the assistant's lanes it would have to
/// carry. It got three right — `turn` into the screen, `recall` and
/// `extraction` into the memory — and it lost the other four: `write`,
/// `turn_write`, `prune` and `error` had no edge out of `./assistants` at all
/// and died as `no_route` at the container. `error` was the sharp one: the
/// contract declared it, and the declaration was satisfied by `./affinity -> .`
/// — so an *assistant's* error died silently while the interface said it left.
/// Green because nobody compared the two files.
///
/// So the rule is read off `templates/assistant/config.json` at the tree, and
/// it admits exactly two answers per lane and no third:
///
/// 1. **A sibling inside this level consumes it** — there is an edge
///    `./assistants -> ./<holder>` on that lane. That is the union rule's one
///    permitted subtraction.
/// 2. **The level re-emits it** — the lane is in `params.contract.emits` AND an
///    edge `./assistants -> .` carries it.
///
/// Neither is a silent drop, and a lane that is declared without the edge is
/// worse than one that is missing from both: it is a promise with nothing
/// behind it.
///
/// Since GH #454 one lane is both at once, and it is read as one thing rather
/// than waved through as two. `answer` goes to a channel of the person when it
/// names one (`./assistants -> ./channels`, guarded on `context.channel_node`) and
/// leaves the level when it does not (`./assistants -> .`, `default: true`,
/// GH #283) — an answer to a caller from outside the member is answered to that
/// caller. That is ONE destination decided at runtime, and the test proves it by
/// reading `default: true` off the exit edge instead of counting edges: an
/// unguarded second edge here would deliver every reply twice.
#[test]
fn every_lane_an_assistant_emits_is_consumed_here_or_leaves_the_level() {
    let Some(member) = shipped() else { return };
    let assistant = repo("templates/assistant");
    if !assistant.join("config.json").is_file() {
        return; // GH #49 — the occupant did not travel into this tree
    }

    let hp = hive_params(&member);
    let c = hp
        .contract
        .as_ref()
        .expect("the member declares a contract");
    let emits: Vec<&str> = c.emits.iter().map(|l| l.route.as_str()).collect();

    let ahp = hive_params(&assistant);
    let raised = ahp
        .contract
        .as_ref()
        .expect("templates/assistant declares a contract");
    assert!(
        !raised.emits.is_empty(),
        "templates/assistant emits nothing — the boundary cannot be checked"
    );

    for lane in &raised.emits {
        let route = lane.route.as_str();
        let consumed_inside = hp
            .graph
            .edges
            .iter()
            .any(|e| e.from == "./assistants" && e.to != "." && condition_names(e, route));
        let leaves = hp
            .graph
            .edges
            .iter()
            .any(|e| e.from == "./assistants" && e.to == "." && condition_names(e, route));
        assert!(
            consumed_inside || leaves,
            "an assistant emits `{route}` and this member neither consumes it nor lets it out: \
             it dies as `no_route` at `<member>/assistants`. Either a holder inside takes it \
             (the union rule's one permitted subtraction) or the level re-emits it — there is \
             no third answer."
        );
        assert!(
            !leaves || emits.contains(&route),
            "the member carries `{route}` out of `./assistants` and does not declare it in \
             `params.contract.emits`: {emits:?}"
        );
        // One lane, two destinations, is normally a lane nobody can reason
        // about — which of the two was meant? There are exactly two declared
        // exceptions, and they are different shapes.
        //
        // `write` is a FAN-OUT (GH #447): BOTH edges fire, because the same
        // closed session is the memory's close pass below and the parent's
        // archive above, and both readers want all of it.
        //
        // `turn_write` is the second one (GH #527), and it is the same shape
        // for a sharper reason: it is the ONLY path in the substrate from a
        // conversation into an `episodes` table (#298 removed the others), and
        // the level that holds the memory is the level that has to fill it.
        // Until #527 this list was one lane long and this member declined the
        // lane, so every stored turn climbed nine hops and dead-lettered at the
        // OS root as `hive_no_route` while the collector stamped it
        // `episode_written = 1`. The copy still leaves the level, because a
        // parent that wants an archive of its own still gets one.
        //
        // `answer` is a GUARDED DEFAULT (GH #283, added by GH #454): exactly
        // ONE of the two fires. The regular edge carries the answer into the
        // person's channels container when it names a channel of this member,
        // and the default carries it out of the level when it does not — a
        // turn that entered from outside on `in_turn` is answered to whoever
        // sent it, not dropped into a container with nobody to route it. The
        // suppression is per SENDER (`crates/meclaw-colony/src/edge_table.rs`),
        // so this is one destination decided at runtime, not two.
        //
        // Both lists are short on purpose; an entry in either is a review
        // decision, not a copy-paste. The fan-out list grew to two exactly
        // once, in GH #527, and the reason is written above it.
        const DELIBERATE_FAN_OUT: [&str; 2] = ["write", "turn_write"];
        const GUARDED_DEFAULT_EXIT: [&str; 1] = ["answer"];
        assert!(
            !(leaves && consumed_inside)
                || DELIBERATE_FAN_OUT.contains(&route)
                || GUARDED_DEFAULT_EXIT.contains(&route),
            "`{route}` is both consumed inside the member and carried out of it — one lane, \
             two destinations, and nothing says which was meant"
        );
        if DELIBERATE_FAN_OUT.contains(&route) {
            assert!(
                leaves && consumed_inside,
                "`{route}` is listed as a deliberate fan-out and is not one any more: it has \
                 to be BOTH consumed inside the level and carried out of it, or the list is \
                 excusing something it no longer describes"
            );
            for e in hp
                .graph
                .edges
                .iter()
                .filter(|e| e.from == "./assistants" && condition_names(e, route))
            {
                assert!(
                    !e.is_default,
                    "the fan-out of `{route}` runs over a GUARDED DEFAULT ({} -> {}). A \
                     default fires only when no regular edge did, so one of the two readers \
                     would silently stop receiving — a fan-out is two regular edges or it is \
                     not a fan-out",
                    e.from, e.to
                );
            }
        }
        if GUARDED_DEFAULT_EXIT.contains(&route) {
            // Read `default: true` explicitly. Without it the exit is an
            // ordinary edge, both destinations fire for every answer, and every
            // reply the person gets is delivered twice — which is exactly the
            // defect GH #283's default edge was introduced to remove.
            let inside_edge = hp
                .graph
                .edges
                .iter()
                .find(|e| e.from == "./assistants" && e.to != "." && condition_names(e, route))
                .unwrap_or_else(|| panic!("`{route}` has no edge into a sibling any more"));
            let exit_edge = hp
                .graph
                .edges
                .iter()
                .find(|e| e.from == "./assistants" && e.to == "." && condition_names(e, route))
                .unwrap_or_else(|| panic!("`{route}` has no exit out of the level any more"));
            assert!(
                exit_edge.is_default,
                "the second outlet of `{route}` is a REGULAR edge, so both fire and every \
                 answer leaves the member twice: once to the channel that asked and once out \
                 of the level. It has to be the guarded default (GH #283): {exit_edge:#?}"
            );
            assert!(
                !inside_edge.is_default,
                "the edge that carries `{route}` to a sibling declared `default: true` as \
                 well. Guarded defaults do not compete with each other — they all fire — so \
                 two of them are two deliveries, not a choice: {inside_edge:#?}"
            );
            let guard = inside_edge.condition.as_deref().unwrap_or_default();
            // GH #522 — the NODE, not the chat. `context.channel` carries the
            // conversation partner now (one session generation, one rate
            // bucket, one memory room per value); the word that says whether a
            // channel of this member owns the turn is `context.channel_node`.
            // Guarding on a non-empty chat id would be an accident that
            // happens to hold.
            assert!(
                guard.contains("context.channel_node != ''"),
                "the regular edge of `{route}` must be guarded on the CHANNEL NODE the \
                 answer names, or it takes every answer — including one to a caller outside \
                 the member — and the default never fires: {guard:?}"
            );
        }
    }

    // The other direction of the same trap: a declared emit whose only exit edge
    // comes from somewhere else. `error` is exactly that shape — `./affinity`
    // satisfied the declaration while an assistant's error died — so every lane
    // an assistant raises and this level declares owes an `./assistants -> .`
    // edge of its own.
    for lane in &raised.emits {
        let route = lane.route.as_str();
        if !emits.contains(&route) {
            continue;
        }
        assert!(
            hp.graph
                .edges
                .iter()
                .any(|e| e.from == "./assistants" && e.to == "." && condition_names(e, route)),
            "the member declares `{route}`, an assistant raises it, and the only exit carrying \
             it comes from another sibling. The declaration would be satisfied while the \
             assistant's copy dies at the container."
        );
    }
}

/// The four lanes an assistant accepts that this member deliberately does NOT
/// carry, and the reason each of them does not cross the boundary.
///
/// Orchestrator ruling **W7-R5** (2026-08-25). Kept beside the assertion rather
/// than in prose alone, because a subtraction nobody wrote down is
/// indistinguishable from a subtraction nobody noticed.
const NOT_CARRIED: [(&str, &str); 5] = [
    (
        "in_advice",
        "answered inside the assistant by ./cogny; the other producer is a SECOND agent, \
         which stands beside this one in the same open container and addresses it directly",
    ),
    (
        "in_sweep",
        "an operator-forced session sweep -- the assistant's own `because` says it \
         \"enters at the assistant path rather than being produced by a sibling\"",
    ),
    (
        "in_prune",
        "a prune verdict from a timer or an operator, paired with the `prune` report the \
         member DOES carry outward",
    ),
    (
        "in_round_sweep",
        "same owner as `in_sweep`, and the same entry point",
    ),
    (
        "in_pack",
        "the identity a generation subscribes to (GH #458). Its producer is INSIDE this \
         level and not above it -- `<member>/affinity` is the record two assistants of one \
         person read -- so the push edge is drawn from one sibling to another and addresses \
         `<member>/assistants/<agent>` at its own path. A lane at the member's own door \
         would be an interface promising something nothing outside ever sends",
    ),
];

/// **The union rule, in the direction that is easy to get wrong (W7-R5).**
///
/// A level's union is the lanes that **cross** it. An emit always crosses: it is
/// produced inside and must get out, which is why the missing outward edges were
/// a real loss of real messages. An accepted lane crosses only when its producer
/// sits OUTSIDE the level and addresses through it. Operator and timer traffic
/// does not: it addresses the assistant's own path, which is reachable because
/// neither `member` nor `assistant` declares `params.ports` — the port boundary
/// forbids an outside endpoint below a hive's path only *"for a **sealed** hive"*
/// (`crates/meclaw-colony/src/mutation/port_boundary.rs`), and both are open.
///
/// So this level carries the two lanes a sibling of the container produces
/// (`in_turn` from the screen, `in_bundle` from the memory) and declares neither
/// of them as an inbound lane of its own — the member's own `in_turn` door goes
/// to the firewall, not to an assistant.
///
/// The assertion is deliberately exhaustive rather than a deny-list: every lane
/// the assistant accepts is either supplied by a sibling inside this member or
/// named in [`NOT_CARRIED`]. A **fifth** lane that really does arrive from above
/// therefore goes red HERE, instead of being silently skipped the way the
/// outward four were. And a later decision to carry one of the four is a
/// deliberate edit of this list plus a lane and a door — not a diff nobody reads.
#[test]
fn the_lanes_an_assistant_takes_from_an_operator_deliberately_do_not_cross_this_level() {
    let Some(member) = shipped() else { return };
    let assistant = repo("templates/assistant");
    if !assistant.join("config.json").is_file() {
        return; // GH #49
    }

    let hp = hive_params(&member);
    let ahp = hive_params(&assistant);
    let taken: Vec<&str> = ahp
        .contract
        .as_ref()
        .expect("templates/assistant declares a contract")
        .accepts
        .iter()
        .map(|l| l.route.as_str())
        .collect();

    // What reaches the container from inside this member. Read off the edges,
    // because the container carries no contract and the edges are the only
    // statement about this boundary the substrate itself reads.
    //
    // Two ways an edge names a lane, and BOTH count: it stamps one (a sibling's
    // own word renamed on the way, `turn` -> `in_turn`), or its condition states
    // one and it carries the message unchanged. The second is what a PLAIN door
    // looks like, and plainness is a requirement rather than a style for the
    // transfer lanes: `in_export` and `in_import` are named the same on both
    // sides of this boundary, so a `set_hop` here would rename a lane onto
    // itself and hide the pairing from the drain probe (GH #467, GH #475).
    let supplied: Vec<&str> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.to == "./assistants")
        .filter_map(|e| stamped_lane(e).or_else(|| stated_lane(e)))
        .collect();
    assert!(
        !supplied.is_empty(),
        "nothing is handed down into ./assistants — the boundary cannot be checked"
    );

    for lane in &taken {
        let inside = supplied.contains(lane);
        let excepted = NOT_CARRIED.iter().any(|(l, _)| l == lane);
        assert!(
            inside || excepted,
            "an assistant accepts `{lane}` and this member neither supplies it from a sibling \
             ({supplied:?}) nor names it as a documented exception. If its producer really is \
             outside the member, the level owes it a lane and a door — an accepted lane no \
             caller can reach through the boundary is the mirror image of an emitted one that \
             dies at it (W7-R5). If its producer addresses the assistant's own path instead, \
             add it to NOT_CARRIED with the reason and say so in the README."
        );
        assert!(
            !(inside && excepted),
            "`{lane}` is both handed down by a sibling and listed as not carried — one lane, \
             two stories"
        );
    }

    // A subtraction is a decision, so it is asserted as one: each exception must
    // still have a subject, and the member must still not be carrying it.
    let accepts: Vec<&str> = hp
        .contract
        .as_ref()
        .expect("contract")
        .accepts
        .iter()
        .map(|l| l.route.as_str())
        .collect();
    for (lane, why) in NOT_CARRIED {
        assert!(
            taken.contains(&lane),
            "the exception for `{lane}` is stale: an assistant no longer accepts it ({why})"
        );
        assert!(
            !accepts.contains(&lane),
            "the member accepts `{lane}`, which W7-R5 ruled it does not carry ({why}). \
             Carrying it is a legitimate later decision — but it is a decision, and it moves \
             this list, the README paragraph and the org and shell contracts with it."
        );
    }

    // And the reason has to be findable where the next reader looks for it.
    let readme = std::fs::read_to_string(member.join("README.md")).expect("README.md");
    for (lane, _) in NOT_CARRIED {
        assert!(
            readme.contains(lane),
            "README.md does not name `{lane}` — a lane this level deliberately does not carry \
             has to say so somewhere a reader will find it, or the decision reads as an \
             oversight"
        );
    }
}

// ─────────────────────────── the contract of the level, and the #284 sentence

#[test]
fn the_member_declares_the_lanes_that_cross_its_boundary() {
    let Some(member) = shipped() else { return };
    let hp = hive_params(&member);
    let contract = hp
        .contract
        .as_ref()
        .expect("a level a caller addresses owes a contract");

    let accepts: Vec<&str> = contract.accepts.iter().map(|l| l.route.as_str()).collect();
    let emits: Vec<&str> = contract.emits.iter().map(|l| l.route.as_str()).collect();
    for lane in ["in_turn", "in_recall", "in_brief", "in_propose"] {
        assert!(
            accepts.contains(&lane),
            "a caller outside the member cannot reach `{lane}`: {accepts:?}"
        );
    }
    for lane in [
        "answer",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "prune",
    ] {
        assert!(
            emits.contains(&lane),
            "`{lane}` cannot leave the member: {emits:?}"
        );
    }
    for l in contract.accepts.iter().chain(contract.emits.iter()) {
        assert!(
            !l.because.trim().is_empty(),
            "lane '{}' says nothing about what it is for",
            l.route
        );
    }

    // The two lists did not move with GH #454 — six in, ten out, exactly as
    // 1.2.0 had them. What moved is how many senders stand behind two of them,
    // and that is the part a contract cannot say, so it is measured on the
    // edges. `answer` has TWO producers since 1.3.0: the record's brief, and an
    // assistant's answer to a caller who named no channel of this member.
    // `error` has FIVE sources: a CHANNEL of this person fails on its own
    // account since GH #454, and since GH #459 an APP does too — and a screen
    // whose refusal this level cannot attribute to any owner leaves on the same
    // lane rather than dead-lettering, which is what makes the display's "an
    // unattributable event leaves ANYWAY" a statement somebody can act on.
    let senders_of = |route: &str| -> Vec<&str> {
        let mut v: Vec<&str> = hp
            .graph
            .edges
            .iter()
            .filter(|e| e.to == "." && condition_names(e, route))
            .map(|e| e.from.as_str())
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    assert_eq!(
        senders_of("answer"),
        vec!["./affinity", "./assistants"],
        "the `answer` lane must carry both what the record said and what an agent said to a \
         caller outside this member (the guarded default of GH #454)"
    );
    assert_eq!(
        senders_of("error"),
        vec![
            "./access",
            "./affinity",
            "./apps",
            "./assistants",
            "./channels"
        ],
        "every failure that is not a refusal leaves on one lane. Since GH #560 the member's \
         own `access` is one of them, and its drain is the ONLY edge of this graph that \
         touches that hive — everything else the broker carries is a v-lane the manifest \
         draws (GH #559). Since GH #454 a CHANNEL is \
         one of the things that can fail — a connector's own error is the member's, not an \
         assistant's, because the connector is the member's — and since GH #459 an APP is \
         another, for exactly the same reason: an app is the person's, it outlives a \
         generation swap, and its failure was never one agent's to report"
    );

    // Ruling Q2 (GH #284), state (2), in the level's own words. This is the one
    // sentence the wave asked for verbatim.
    let reject = contract
        .emits
        .iter()
        .find(|l| l.route == "reject")
        .expect("the reject lane");
    assert!(
        reject.because.contains("no_route")
            && reject.because.contains("recorded and self-localising"),
        "the `reject` lane must say what an unconsumed refusal becomes: {:?}",
        reject.because
    );
}

/// GH #425 — the member carries the builder lane pair, and carries nothing else
/// about it.
///
/// ADR-0013: a level declares the union of its occupants' lanes that CROSS it.
/// `build` leaves `./assistants` and no sibling of this level consumes it;
/// `in_build_result` comes from the OS level and addresses THROUGH this one.
/// Both cross, both are declared, and neither is read here — the builder is at
/// the OS level because one colony has one baumeister, and everything between
/// is transit.
#[test]
fn the_member_carries_the_builder_lane_pair_and_reads_nothing_in_it() {
    let Some(_root) = shipped() else { return };
    let (assistant_accepts, assistant_emits) = lanes_of("assistant");
    if assistant_emits.is_empty() {
        return;
    }
    let (accepts, emits) = lanes_of("member");

    assert!(
        assistant_emits.contains(&"build".to_string()),
        "the assistant no longer emits `build`: {assistant_emits:?}"
    );
    assert!(
        emits.contains(&"build".to_string()),
        "the member does not carry `build`: {emits:?}"
    );
    assert!(
        assistant_accepts.contains(&"in_build_result".to_string()),
        "the assistant no longer accepts `in_build_result`: {assistant_accepts:?}"
    );
    assert!(
        accepts.contains(&"in_build_result".to_string()),
        "the member does not carry `in_build_result`: {accepts:?}"
    );
}
