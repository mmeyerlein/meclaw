//! GH #488 — the FORM half: an agent's identity and its reply instructions
//! have a durable home that a transfer can carry, and a lane that puts them
//! back.
//!
//! The measured defect: `identity.soul` and `instructions.reply` lived as
//! `system.*` rows in a brain's own `cell.db`. A brain has no `porter`, its
//! `system` table is in no schema mirror, and no template seeds either slot —
//! so a member-level export carried everything a person had said and nothing
//! the agent was, and a grown generation answered as the vendor's default
//! assistant.
//!
//! The way out chosen here does not give the brain a porter. It moves the
//! HOME: the durable original is the agent's own record in `affinity` — a
//! reserved `mx.brain` subtree of the `entities` row — and the brain's
//! `system.*` is a DELIVERED COPY of it. Everything else follows from that one
//! move and is pinned below:
//!
//! * `entities.mx` is a `json` column that already stands in `affinity`'s
//!   porter schema mirror, so the record travels on the transfer lane that
//!   exists. No fifth holder, no new document format, no change to the porter.
//! * `subscribers` travels too, with `pack_hash`/`sent_at` blanked by the
//!   porter's own `RESET` — which is exactly what makes the FIRST identity pack
//!   after an import fire instead of being suppressed as already delivered.
//! * `brief` renders the subtree verbatim, under the request slot `brain`, and
//!   the disclosure filter decides what may travel — the audience rule stays in
//!   one place.
//! * `collector/assemble` is the whitelist, and since this issue it names
//!   `instructions` too. That is a REOPENING of one of GH #458's three
//!   subtractions, and claim 1 states the reason where the next reader will
//!   find it.
//!
//! No colony here: this file is about constants, a schema mirror, a seed and
//! four sentences. The end-to-end run lives in
//! `gh488_a_reborn_agent_answers_as_itself.rs`.

use meclaw_core::serde_json::Value;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

fn read_file(rel: &str) -> String {
    let p = templates_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p:?} must be readable: {e}"))
}

fn script_of(rel: &str) -> String {
    let raw = read_file(rel);
    let v: Value = meclaw_core::serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{rel} must be JSON: {e}"));
    v["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{rel} must carry params.script_inline"))
        .to_string()
}

/// The string members of a python tuple constant `NAME = ("a", "b", ...)`, read
/// out of the shipped source — the same technique `w11_curator.rs` and
/// `gh458_the_pack_slots_are_a_subset_of_sys_keep.rs` use. Reading the constant
/// out of the artefact is the point: a copy of the list in this file could
/// agree with itself while disagreeing with what ships.
fn tuple_constant(src: &str, name: &str) -> Vec<String> {
    let needle = format!("\n{name} = (");
    let at = src
        .find(&needle)
        .unwrap_or_else(|| panic!("`{name}` must stand in the shipped script as a tuple constant"));
    let from = at + needle.len();
    let len = src[from..]
        .find(')')
        .unwrap_or_else(|| panic!("the `{name}` declaration must close its paren"));
    let decl = &src[from..from + len];
    let mut out = Vec::new();
    let mut rest = decl;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        out.push(after[..close].to_string());
        rest = &after[close + 1..];
    }
    assert!(!out.is_empty(), "`{name}` must name at least one member");
    out
}

/// A `NAME = "value"` string constant out of a shipped script.
fn str_constant(src: &str, name: &str) -> String {
    let needle = format!("\n{name} = \"");
    let at = src.find(&needle).unwrap_or_else(|| {
        panic!("`{name}` must stand in the shipped script as a string constant")
    });
    let from = at + needle.len();
    let len = src[from..]
        .find('"')
        .unwrap_or_else(|| panic!("the `{name}` declaration must close its quote"));
    src[from..from + len].to_string()
}

/// One JSONL seed table: the header row's `schema` and the data rows.
fn seed_rows(rel: &str) -> Vec<Value> {
    read_file(rel)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str::<Value>(l).expect("a seed line is JSON"))
        .filter(|v| v.get("schema").is_none())
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════ pins

/// Claim 1. The charter has a family it may land in.
///
/// GH #458 subtracted `instructions` from `PACK_SLOTS` on the ground that "an
/// identity that could overwrite it would be an identity that could rewrite
/// what the agent is for". That argument assumed the charter had ANOTHER
/// owner. GH #488 measured that it had none — nothing exported it, no template
/// supplied it — so the subtraction did not protect a slot, it kept one empty.
/// The subset relation to `SYS_KEEP` is untouched and re-asserted here, because
/// it is what keeps a written pack out of the curator's reach at any budget.
#[test]
fn the_charter_may_be_written_over_the_pack_lane() {
    let src = script_of("collector/assemble/config.json");
    let pack = tuple_constant(&src, "PACK_SLOTS");
    let keep = tuple_constant(&src, "SYS_KEEP");

    assert!(
        pack.contains(&"instructions".to_string()),
        "`instructions` must be writable over `in_pack`: it is the family the \
         agent's reply instructions land in, and with the family closed there \
         is no lane at all through which a rebuilt agent could be told how it \
         answers. PACK_SLOTS = {pack:?}"
    );
    assert!(
        pack.contains(&"identity".to_string()),
        "`identity` carries `identity.soul`, the other half of GH #488. \
         PACK_SLOTS = {pack:?}"
    );

    let outside: Vec<&String> = pack.iter().filter(|s| !keep.contains(s)).collect();
    assert!(
        outside.is_empty(),
        "PACK_SLOTS must stay a subset of SYS_KEEP, and {outside:?} is not in \
         it — a writable family outside SYS_KEEP is one the curator may cut \
         behind the sender's back on the very next assembly. \
         PACK_SLOTS = {pack:?}, SYS_KEEP = {keep:?}"
    );

    // The two subtractions that are still subtractions, and why they are:
    // this cell re-derives both every round, so a sender writing them would
    // fight it for the same path forever.
    for derived in ["tools", "budget"] {
        assert!(
            !pack.contains(&derived.to_string()),
            "`{derived}` must NOT be writable over in_pack — the collector \
             re-derives it on every assembly. PACK_SLOTS = {pack:?}"
        );
    }
}

/// Claim 2. `affinity`'s renderer has a request slot for the brain's own state,
/// and it names the reserved subtree it reads.
#[test]
fn the_renderer_has_a_slot_for_the_agents_own_durable_state() {
    let src = script_of("affinity/brief/config.json");
    let slots = tuple_constant(&src, "SLOTS");
    assert!(
        slots.contains(&"brain".to_string()),
        "`brief` must offer the `brain` request slot — it is what a \
         self-subscription asks for, and without it the agent's own record can \
         be curated and exported but never delivered. SLOTS = {slots:?}"
    );

    let root = str_constant(&src, "BRAIN_ROOT");
    assert_eq!(
        root, "brain",
        "the reserved subtree of `mx` is `mx.brain`; the seed, the disclosure \
         rows and the offline export all address it by that name"
    );
}

/// Claim 3. The home travels on the lane that already exists.
///
/// This is the whole reason the chosen way needs no fifth holder and no change
/// to a porter: `entities.mx` is a `json` column in `affinity`'s schema mirror,
/// so a record written into it is in the export document by construction. A
/// future change that drops the column from the mirror would silently stop
/// exporting every agent's identity, and that is what this pin is for.
#[test]
fn the_record_is_inside_the_document_the_porter_already_writes() {
    let src = script_of("affinity/porter/config.json");

    for (table, column) in [
        ("entities", "\"mx\": \"json\""),
        ("subscribers", "\"slots\": \"json\""),
    ] {
        let at = src
            .find(&format!("\"{table}\": {{"))
            .unwrap_or_else(|| panic!("the porter's SCHEMA mirror must still declare `{table}`"));
        let rest = &src[at..(at + 900).min(src.len())];
        assert!(
            rest.contains(column),
            "the porter's mirror of `{table}` must carry {column} — it is what \
             makes an agent's own record travel with the hive it lives in. \
             GH #488 chose that home precisely because this column already \
             travels; drop it and every rebuilt agent is anonymous again. \
             Read: {rest:?}"
        );
    }

    // And the other half of the arithmetic: the delivery stamps are RESET on
    // import. Without that the receiving push lane compares a fresh pack
    // against a hash it never sent, stays silent for ever, and the reborn
    // agent never gets its first pack (the porter says so itself).
    assert!(
        src.contains(r#"RESET = {"subscribers": ["pack_hash", "sent_at"]}"#),
        "the porter must keep blanking `pack_hash`/`sent_at` on import — it is \
         what makes the FIRST identity pack after a rebuild fire at all"
    );
}

/// Claim 4. The shipped record shows the shape, and the audience rule releases
/// it — a template that carried the mechanism and no example would leave the
/// operator to guess the one thing that is fail-closed.
#[test]
fn the_shipped_agent_record_carries_the_example_and_its_release() {
    let src = script_of("collector/assemble/config.json");
    let pack = tuple_constant(&src, "PACK_SLOTS");

    let agent = seed_rows("affinity/store/seed/entities.jsonl")
        .into_iter()
        .find(|r| r["kind"] == "agent")
        .expect("the shipped affinity seeds an agent record");
    let brain = agent["mx"]["brain"]
        .as_object()
        .unwrap_or_else(|| {
            panic!(
                "the seeded agent record must carry an `mx.brain` example; its \
                 mx is {}",
                agent["mx"]
            )
        })
        .clone();
    assert!(
        !brain.is_empty(),
        "an empty `mx.brain` documents nothing at all"
    );
    for (family, leaves) in &brain {
        assert!(
            pack.contains(family),
            "`mx.brain.{family}` names a family the pack lane would refuse \
             WHOLE (`slot_unknown`): the writable list is {pack:?}"
        );
        let leaves = leaves
            .as_object()
            .unwrap_or_else(|| panic!("`mx.brain.{family}` must be an object of leaves"));
        assert!(
            !leaves.is_empty(),
            "`mx.brain.{family}` must name at least one leaf — a family with no \
             leaf has no slot path"
        );
        for (leaf, value) in leaves {
            assert!(
                value.is_string(),
                "`mx.brain.{family}.{leaf}` must be the RAW text of the slot. \
                 The delivery side wraps it as {{\"text\": …}}, and that wrap \
                 is what makes the slot byte-identical to the one it was read \
                 from"
            );
        }
    }

    let released = seed_rows("affinity/store/seed/disclosure.jsonl")
        .into_iter()
        .any(|r| {
            r["entity_id"] == agent["entity_id"]
                && r["field_path"] == "mx.brain"
                && r["mode"] == "share"
        });
    assert!(
        released,
        "the seeds must release `mx.brain` of the agent to itself: `brief` is \
         fail-closed and builds by ADDITION, so without a disclosure row the \
         renderer answers `not_disclosed` and the lane delivers nothing at all"
    );
}

/// Claim 5. The prose says the same. A constant and a README that disagree are
/// worse than either alone — an operator reads the README and the lane refuses
/// what it promised (`docs/development-rules.md` § 2d).
#[test]
fn the_prose_names_the_family_the_constant_names() {
    let src = script_of("collector/assemble/config.json");
    let pack = tuple_constant(&src, "PACK_SLOTS");

    for readme in ["talky/README.md", "cogny/README.md", "collector/README.md"] {
        let text = read_file(readme);
        let at = text.find("closed list").unwrap_or_else(|| {
            panic!(
                "{readme}: the sentence that states the closed list of writable \
                 `in_pack` slots has been reworded away; repair the pin in the \
                 same change (`docs/development-rules.md` § 2d)"
            )
        });
        let sentence = &text[at..(at + 320).min(text.len())];
        for slot in &pack {
            assert!(
                sentence.contains(&format!("`{slot}`")),
                "{readme} must name `{slot}` where it states the closed list, \
                 because the shipped PACK_SLOTS does: {sentence:?}"
            );
        }
    }

    // And the machine-readable half: the collector's own `in_pack` accept term.
    let cfg = read_file("collector/config.json");
    let v: Value = meclaw_core::serde_json::from_str(&cfg).expect("collector config is JSON");
    let because = v["params"]["contract"]["accepts"]
        .as_array()
        .expect("the collector declares accepts")
        .iter()
        .find(|a| a["route"] == "in_pack")
        .expect("the collector accepts in_pack")["because"]
        .as_str()
        .expect("an accept term has a reason")
        .to_string();
    for slot in &pack {
        assert!(
            because.contains(&format!("`{slot}`")),
            "the collector's `in_pack` accept term must name `{slot}` — it is \
             the machine-readable half of the same promise: {because:?}"
        );
    }

    // The affinity side states which slot carries it, in its own README.
    let aff = read_file("affinity/README.md");
    assert!(
        aff.contains("mx.brain"),
        "`templates/affinity/README.md` must document `mx.brain` — the hive \
         that became the durable home of an agent's identity has to say so \
         where an operator looks for it"
    );
}
