//! GH #302 / GH #122 — `member@1.0.0` holds what two assistants of one person
//! must share.
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
//! Everything below is a fact about the FILES — no colony, no runtime, the same
//! reasoning as `gh173_shipped_hive_contracts`: whether a class's declared
//! interface matches the classes it composes is checkable before anything is
//! ever instantiated.
//!
//! 1. **Four children: three `ref`s and one container.** The refs are named
//!    exactly like the templates they pull in (the standing naming rule — an
//!    instance name that differs from its template name is drift, not
//!    intention), and each pins an exact version: a bare `<name>` resolves to
//!    the highest one present, which is precisely the drift `template_chain`
//!    exists to make visible (the ref-resolution receipt of GH #277).
//! 2. **No `memory-drain` and no `terminal`.** Per-turn extraction (GH #298,
//!    ruling Q11) removed the drain; ruling Q2 (GH #284) removed the sink — a
//!    refusal either has a consumer that records it or no edge at all, and this
//!    level chose the second.
//! 3. **No `params.ports` key anywhere in the template.** The container is OPEN
//!    because the mutation that instantiates an assistant into it draws edges to
//!    that assistant, and a sealed hive refuses exactly those endpoints with
//!    `hive_port_boundary`. And no slot: W4's slot governs an address that does
//!    **not** exist, while this container does
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

/// The constant lane this edge STAMPS on what it takes, if any — the second way
/// an edge can name a lane (GH #176).
fn stamped_lane(e: &meclaw_colony::config::EdgeSpec) -> Option<&str> {
    e.modifier
        .as_ref()
        .and_then(|m| m.set_hop.get("route"))
        .and_then(|s| constant(s.as_str()))
}

// ───────────────────────────────── (a) four children, three refs, one container

#[test]
fn the_level_carries_three_refs_and_one_container() {
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
            "affinity".to_string(),
            "assistants".to_string(),
            "firewall".to_string(),
            "memory-hive".to_string(),
        ],
        "the member owns exactly three holders and one container. A fourth holder is \
         something the siblings did not have to share; a missing one is something an \
         assistant would have to hold itself."
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

    // The container ships EMPTY: an assistant is instantiated into it, never
    // shipped inside it.
    let inside: Vec<String> = std::fs::read_dir(member.join("assistants"))
        .unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            p.is_dir()
                .then(|| p.file_name().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    assert!(
        inside.is_empty(),
        "the assistants container ships empty: {inside:?}"
    );
    assert_eq!(
        config_at(&member.join("assistants"))
            .get("cell")
            .and_then(|c| c.get("type"))
            .and_then(Value::as_str),
        Some("hive"),
        "the container is a real hive — an address that already exists, so the mutation that \
         instantiates an assistant has somewhere to put one"
    );
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

    for rel in ["", "assistants"] {
        let cfg = config_at(&member.join(rel));
        let ports = cfg.get("params").and_then(|p| p.get("ports"));
        assert!(
            ports.is_none(),
            "templates/member/{rel}: declares params.ports = {ports:?}. The container is OPEN \
             because the mutation that instantiates an assistant draws edges to it, and a \
             sealed hive refuses exactly those endpoints with `hive_port_boundary`. There is \
             no slot to declare either — a slot governs an address that does not exist, and \
             this container does (unbound_slot_behaviour, colony.rs) — and writing \
             the key on the MEMBER would seal a level that gets wired into."
        );
    }
}

/// Orchestrator ruling W7-R2 (2026-08-25), pinned here because this is the
/// level whose own edges wire its container from birth.
///
/// A container hive that its own level wires declares **no** `params.contract`.
/// `addressed_lane_doors` skips a hive only while NOTHING addresses its path
/// (`hive_path_is_wired`); this member wires `./assistants` four times, so the
/// container is wired the moment the member is instantiated. From then on every
/// declared `accepts` lane owes a `door_exists` — a message arriving at the
/// container path must reach a cell **inside** it — and an empty container has
/// no inside. The violation is collected on **every** mutation of the colony,
/// not only on the one that touches this member, so a contract here would lock
/// the colony for exactly as long as the member has no assistant yet, which is
/// a perfectly legitimate intermediate state.
#[test]
fn the_container_declares_no_contract_because_the_level_wires_it() {
    let Some(member) = shipped() else { return };

    let cfg = config_at(&member.join("assistants"));
    let contract = cfg.get("params").and_then(|p| p.get("contract"));
    assert!(
        contract.is_none(),
        "templates/member/assistants declares params.contract = {contract:?}. The member's own \
         edges address this container from birth, so `hive_path_is_wired` is true and every \
         declared lane owes a door INSIDE the container — which an empty container cannot \
         have. The transit lanes belong on the level whose edges satisfy the check, and what \
         an instantiating mutation must wire belongs in `description`, which the substrate \
         does not enforce against an address nobody stands at yet."
    );

    // The other half of the same rule: what the member DOES declare, it can
    // open. Every accepts lane has a door edge out of `.`, every emits lane an
    // edge from inside back to `.`.
    let hp = hive_params(&member);
    let c = hp
        .contract
        .as_ref()
        .expect("the member declares a contract");
    for lane in &c.accepts {
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

    // The screen. An unscreened turn goes up out of the container, the screened
    // one comes back down to it.
    assert!(
        has("./assistants", "./firewall", "in_turn"),
        "no unscreened turn reaches the screen"
    );
    assert!(
        has("./firewall", "./assistants", "in_turn"),
        "the screened turn never gets back to the assistant that sent it"
    );
    assert!(
        has(".", "./firewall", ""),
        "a turn entering from outside the member reaches nothing"
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
        assert!(
            !(leaves && consumed_inside),
            "`{route}` is both consumed inside the member and carried out of it — one lane, \
             two destinations, and nothing says which was meant"
        );
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
const NOT_CARRIED: [(&str, &str); 4] = [
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

    // What a sibling of the container hands down. Read off the edges, because
    // the container carries no contract and the edges are the only statement
    // about this boundary the substrate itself reads.
    let supplied: Vec<&str> = hp
        .graph
        .edges
        .iter()
        .filter(|e| e.to == "./assistants")
        .filter_map(stamped_lane)
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
