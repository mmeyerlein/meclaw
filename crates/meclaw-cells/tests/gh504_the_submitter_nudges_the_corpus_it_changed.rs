//! GH #504 — the librarian's ingest nudge, wired at last.
//!
//! GH #496 built the reconciliation and `templates/builder-librarian/README.md`
//! § *Who drives it* named the wiring that should fire it: *an edge from
//! `submit` after a committed submission whose diff carried `add_templates` —
//! the one cell that knows both facts*. The form it named,
//! `./submit/gate -> ./builder/librarian`, is undrawable by anybody — no
//! manifest, no operator, no hand-written `config.json` — because both
//! endpoints are interior nodes of sealed hives and the port boundary refuses
//! each of them on its own account. The reachable form is between the two HIVE
//! PATHS, and it needed a lane at each end:
//!
//! 1. `templates/builder` accepts `in_ingest` at its hive path, forwards it to
//!    `./librarian`, and lets the reconciliation's report back out on
//!    `catalogue`. The two are paired in its own `required_drains`, so a caller
//!    that nudges and does not subscribe is refused rather than silently
//!    unreported.
//! 2. `templates/submit` publishes what its gate already derives from the diff:
//!    `hop.registers_class`, on every receipt its renderer produces. It has to
//!    be REMEMBERED — the colony answers on a fresh trace carrying `outcome`,
//!    `applied` and `ids` and nothing about what the declarations were — so it
//!    travels on the flight row and comes back with the correlation.
//! 3. `templates/meclaw-os` draws `./operator -> ./builder` on a submitter's
//!    receipt with no `error_code` AND that key, re-stamped `in_ingest`, plus
//!    `./builder -> .` for the report. Only this level can draw it, for the
//!    reason it is the only one that can draw `./operator -> ./access`: the two
//!    are siblings here and nowhere else.
//! 4. Since GH #556 the submitter is an occupant of the FRONT DOOR rather than
//!    of the shell, so the receipt has to cross one rim before the shell sees
//!    it: `./submit -> .` inside `templates/operator` re-stamps the submitter's
//!    own `receipt` onto `sub_receipt`, guarded on the two shapes the front
//!    door cannot answer for itself — one carrying `hop.error_code`, one whose
//!    committed diff registered a class. An ordinary committed receipt is
//!    rendered for the caller inside the hive and does not leave twice, which is
//!    why the lane is its own name rather than a second sender on `receipt`.
//!
//! The last three tests are the drift locks of `docs/development-rules.md`
//! § 2d: each greps a countable or behavioural promise on a public template
//! surface AND drives the mechanism it promises. A number in that prose is
//! derived from the config here rather than typed twice.
//!
//! **R2b guard.** Every read is guarded by [`shipped`]: where a template does
//! not ship, these tests skip rather than fail on a dead reference.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{emit_all, run_shipped_script, shipped_script};
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../")
}

/// The files this suite reads. The list is the guard AND the inventory.
const FILES: &[&str] = &[
    "templates/builder/config.json",
    "templates/builder/README.md",
    "templates/submit/gate/config.json",
    "templates/submit/store/config.json",
    "templates/submit/README.md",
    "templates/operator/config.json",
    "templates/operator/submit/config.json",
    "templates/meclaw-os/config.json",
    "templates/meclaw-os/template.json",
    "templates/meclaw-os/README.md",
    "templates/meclaw-os/builder/config.json",
    "templates/meclaw-os/operator/config.json",
    "templates/builder-librarian/README.md",
];

fn shipped() -> Option<PathBuf> {
    let r = root();
    FILES.iter().all(|f| r.join(f).exists()).then_some(r)
}

fn read(rel: &str) -> Value {
    let p = root().join(rel);
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).expect(rel)).expect(rel)
}

fn text(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).expect(rel)
}

fn gate() -> String {
    shipped_script(
        root()
            .join("templates/submit/gate/config.json")
            .to_str()
            .expect("path"),
    )
}

/// The one operation a store message carries, parsed out of its `tool_call`.
fn op_of(msg: &Value) -> Value {
    meclaw_core::serde_json::from_str(msg["messages"][0]["text"].as_str().expect("a tool_call"))
        .expect("the args are json")
}

/// The canonical digest of a declaration list, drawn the way the gate draws it.
fn digest_of(decls: &Value) -> String {
    let program = concat!(
        "import sys, json, hashlib\n",
        "d = json.load(sys.stdin)\n",
        "c = json.dumps(d, sort_keys=True, separators=(',', ':'), ensure_ascii=False)\n",
        "sys.stdout.write(hashlib.sha256(c.encode('utf-8')).hexdigest())\n"
    );
    String::from_utf8(run_shipped_script(program, &decls.to_string()).stdout).expect("hex")
}

// -------------------------------------------------------------- 1. the builder

#[test]
fn the_builder_takes_the_nudge_at_its_hive_path_and_lets_the_report_out() {
    let Some(_) = shipped() else { return };
    let cfg = read("templates/builder/config.json");
    let params = &cfg["params"];

    // The seal is what made the documented edge undrawable. It stays.
    assert_eq!(
        params["ports"].as_array().expect("ports").len(),
        0,
        "the hive path is the only address, which is why the lane is AT it"
    );

    let accepts = params["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .find(|l| l["route"] == "in_ingest")
        .expect("in_ingest is a declared lane of the builder");
    assert_eq!(
        accepts["context"],
        json!([]),
        "the nudge promotes nothing: the body is not read, the message IS the nudge"
    );

    assert!(
        params["contract"]["emits"]
            .as_array()
            .expect("emits")
            .iter()
            .any(|l| l["route"] == "catalogue"),
        "the report has to leave, or it is a dead letter one storey up"
    );

    let drain = params["required_drains"]
        .as_array()
        .expect("required_drains")
        .iter()
        .find(|d| d["accepts"] == "in_ingest")
        .expect("the pair is declared");
    assert_eq!(drain["emits"], "catalogue");

    let edges = params["graph"]["edges"].as_array().expect("edges");
    let door = edges
        .iter()
        .find(|e| e["from"] == "." && e["to"] == "./builder-librarian")
        .expect("the hive path forwards the nudge to the librarian");
    assert_eq!(
        door["condition"],
        "has(hop.route) && hop.route == 'in_ingest'"
    );
    assert!(
        door["modifier"].is_null(),
        "the librarian accepts `in_ingest` under that very name -- a re-stamp \
         here would be a second name for one lane"
    );

    let exit = edges
        .iter()
        .find(|e| {
            e["from"] == "./builder-librarian"
                && e["to"] == "."
                && e["condition"] == "has(hop.route) && hop.route == 'catalogue'"
        })
        .expect("the report crosses the builder's own boundary");
    assert_eq!(exit["to"], ".");

    // The retrieval lane is untouched: this is additive, and a nudge that had
    // taken `in_request`'s door away would have bought the corpus by losing
    // every lookup.
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == "./lib" && e["to"] == "./builder-librarian"),
        "the search lane still reaches the librarian"
    );
}

// --------------------------------------------------------------- 2. the submit

/// The store's answer to the FIFO `select`, in the shape the hive's own edge
/// hands it back: the marker in context, the rows as the text of one turn.
fn pop(rows: Value, carry: &str) -> Vec<Value> {
    emit_all(
        &gate(),
        &json!({
            "target": "/os/operator/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": "pop",
                             "sub_carry": carry }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

const COMMITTED: &str = r#"{"outcome":"committed","applied":1,"ids":["m1"]}"#;
const REJECTED: &str = r#"{"outcome":"rejected","applied":0,"ids":[],"failed_at":1,
                           "remaining":0,"error_code":"template_missing"}"#;

fn row(registers: i64) -> Value {
    json!([{ "id": "r1", "tool_call_id": "c2", "manifest_sha256": "abc123",
             "registers": registers }])
}

#[test]
fn the_gate_declares_the_key_before_it_stamps_it() {
    let Some(_) = shipped() else { return };
    let hop = &read("templates/submit/gate/config.json")["contract"]["emits"]["hop"];
    assert_eq!(hop["registers_class"]["type"], "boolean");
    assert_eq!(hop["registers_class"]["required"], false);
}

#[test]
fn the_receipt_says_whether_the_manifest_registered_a_class() {
    let Some(_) = shipped() else { return };

    let out = pop(row(1), COMMITTED);
    let h = &out[out.len() - 1]["header"];
    assert_eq!(h["route"], "receipt");
    assert_eq!(h["registers_class"], true);
    assert!(
        h.get("error_code").is_none(),
        "a committed receipt names no code -- the two facts are two keys"
    );

    let out = pop(row(0), COMMITTED);
    let h = &out[out.len() - 1]["header"];
    assert_eq!(
        h["registers_class"], false,
        "stamped `false` rather than omitted: an absent key and a manifest \
         that registered nothing must not look alike"
    );
}

#[test]
fn a_refused_manifest_carries_the_key_and_the_code_beside_it() {
    let Some(_) = shipped() else { return };
    // Both halves matter to the one edge drawn on this key. `registers_class`
    // alone would nudge after a manifest that registered a class and was
    // refused at the door; `committed` alone would nudge after every
    // submission a colony ever makes.
    let out = pop(row(1), REJECTED);
    let h = &out[out.len() - 1]["header"];
    assert_eq!(h["registers_class"], true);
    assert_eq!(h["error_code"], "template_missing");
}

/// The un-park: the broker said yes, the manifest comes back off the parked row.
fn unpark(decls: &Value, sha: &str) -> Vec<Value> {
    // The requester the gate parked is the path the SUBSTRATE stamped on the
    // emission that reached it — the front door's identity cell, which GH #556
    // renamed to `intake`.
    let rows = json!([{ "id": "p1", "manifest": decls,
                        "requester": "/os/operator/intake",
                        "tool_call_id": "c2", "manifest_sha256": sha }]);
    emit_all(
        &gate(),
        &json!({
            "target": "/os/operator/submit",
            "header": {
                "hop": { "operation": "select", "rows_affected": 1 },
                "context": { "sub_origin": "gate", "sub_phase": "subscribing",
                             "sub_carry": "{\"status\": \"allowed\"}" }
            },
            "ttl": 64,
            "messages": [{ "origin": "tool", "type": "tool_result", "id": "x",
                           "text": rows.to_string() }],
            "params": {}
        }),
    )
}

#[test]
fn the_flight_row_remembers_the_diff_the_colony_will_not_carry() {
    let Some(_) = shipped() else { return };

    let registering = json!([{ "scope": "/os/orgs", "ctx": {}, "diff": {
        "add_templates": [{ "name": "foo", "files": {} }] }}]);
    let plain = json!([{ "scope": "/os/orgs", "ctx": {}, "diff": {
        "add_nodes": [{ "name": "sink", "template": "terminal" }] }}]);

    for (decls, expected) in [(&registering, 1), (&plain, 0)] {
        let sha = digest_of(decls);
        let out = unpark(decls, &sha);
        assert_eq!(out.len(), 3, "forget the park, remember the flight, submit");
        let flight = op_of(&out[1]);
        assert_eq!(flight["row"]["kind"], "flight");
        assert_eq!(
            flight["row"]["registers"], expected,
            "the flight row is the only carrier: `/colony` replies with \
             outcome, applied and ids, and never with the diff"
        );
        assert_eq!(
            flight["row"]["manifest"],
            json!([]),
            "and it is still a CORRELATION, not a second copy of the bytes"
        );
    }
}

// ----------------------------------------------------------------- 3. the shell

/// The shell's guard on the nudge. The lane is the front door's `sub_receipt`
/// since GH #556 — the submitter's own receipt, re-stamped on its way out of the
/// hive it now lives in.
const NUDGE: &str = "has(hop.route) && hop.route == 'sub_receipt' && !has(hop.error_code) \
                     && has(hop.registers_class) && hop.registers_class == true";

/// The front door's own guard, one storey down: which receipts leave the hive at
/// all. An ordinary committed receipt is answered inside and is not on it.
const SUB_RECEIPT: &str = "has(hop.route) && hop.route == 'receipt' \
                           && (has(hop.error_code) \
                           || (has(hop.registers_class) && hop.registers_class == true))";

#[test]
fn the_shell_draws_the_edge_no_other_level_could() {
    let Some(_) = shipped() else { return };
    let cfg = read("templates/meclaw-os/config.json");
    let edges = cfg["params"]["graph"]["edges"].as_array().expect("edges");

    let nudge = edges
        .iter()
        .find(|e| {
            e["from"] == "./operator"
                && e["to"] == "./builder"
                && e["modifier"]["set_hop"]["route"] == "'in_ingest'"
        })
        .expect("./operator -> ./builder, re-stamped in_ingest");
    assert_eq!(
        nudge["condition"], NUDGE,
        "both guards, and neither is decoration"
    );

    // The half the shell cannot draw: the submitter is an occupant of the front
    // door, so its receipt has to be let out before this level ever sees it —
    // and only in the two shapes the front door cannot answer for itself.
    let inside = read("templates/operator/config.json");
    let inside = inside["params"]["graph"]["edges"]
        .as_array()
        .expect("the operator's edges")
        .clone();
    let lift = inside
        .iter()
        .find(|e| {
            e["from"] == "./submit"
                && e["to"] == "."
                && e["modifier"]["set_hop"]["route"] == "'sub_receipt'"
        })
        .expect("./submit -> . inside the front door, re-stamped sub_receipt");
    assert_eq!(
        lift["condition"], SUB_RECEIPT,
        "an ordinary committed receipt is answered inside the hive; letting a \
         copy out as well would be one submission with two answers"
    );

    let report = edges
        .iter()
        .find(|e| {
            e["from"] == "./builder"
                && e["to"] == "."
                && e["condition"] == "has(hop.route) && hop.route == 'catalogue'"
        })
        .expect("./builder -> . on catalogue -- the drain the builder demands");
    assert_eq!(
        report["modifier"]["delete_context"],
        json!(["operator_caller", "requester_build_op", "sub_sha"]),
        "an exit at this rim clears the level's own interior keys"
    );

    assert!(
        cfg["params"]["contract"]["emits"]
            .as_array()
            .expect("emits")
            .iter()
            .any(|l| l["route"] == "catalogue"),
        "a lane that leaves the hive path is a lane the level declares"
    );

    // The refusal lane the shell has always had is untouched: an error receipt
    // still goes to the builder as `in_receipt`, and it is a different edge. It
    // travels the same rim as the nudge now, which is why the two are told apart
    // by `error_code` and not by the lane they arrive on.
    let repair = edges
        .iter()
        .find(|e| {
            e["from"] == "./operator"
                && e["to"] == "./builder"
                && e["modifier"]["set_hop"]["route"] == "'in_receipt'"
        })
        .expect("the repair lane of GH #425");
    assert_eq!(
        repair["condition"],
        "has(hop.route) && hop.route == 'sub_receipt' && has(hop.error_code)"
    );

    // The pins the ref files carry: a bare name would resolve to whatever is
    // newest on disk, which is the drift `registry.template_chain` exists to
    // make visible rather than to excuse. The submitter's pin moved with it
    // (GH #556) and is now the front door's.
    assert_eq!(
        read("templates/meclaw-os/builder/config.json")["cell"]["template"],
        "builder@1.6.0"
    );
    assert_eq!(
        read("templates/meclaw-os/operator/config.json")["cell"]["template"],
        "operator@1.1.0"
    );
    assert_eq!(
        read("templates/operator/submit/config.json")["cell"]["template"],
        "submit@2.3.0"
    );
}

// ------------------------------------------------- the drift locks (§ 2d)

/// The number words this tree writes its counts in, small enough to be a table
/// and derived from the config rather than typed beside it.
fn word(n: usize) -> String {
    const ONES: [&str; 10] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
    ];
    const TENS: [&str; 10] = [
        "", "ten", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    ];
    const TEENS: [&str; 10] = [
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
    ];
    match n {
        0..=9 => ONES[n].to_string(),
        10..=19 => TEENS[n - 10].to_string(),
        _ if n.is_multiple_of(10) => TENS[n / 10].to_string(),
        _ => format!("{}-{}", TENS[n / 10], ONES[n % 10]),
    }
}

#[test]
fn the_shell_readme_counts_the_edges_the_shell_has() {
    let Some(_) = shipped() else { return };
    let edges = read("templates/meclaw-os/config.json")["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let rim = edges
        .iter()
        .filter(|e| e["from"] == "." || e["to"] == ".")
        .count();
    let inner = edges.len() - rim;
    let readme = text("templates/meclaw-os/README.md");

    let heading = format!("## The {} edges", word(edges.len()));
    assert!(readme.contains(&heading), "README heading: {heading}");

    let doors = format!("{} of them are a door or an exit", {
        let mut w = word(rim);
        w.replace_range(..1, &w[..1].to_uppercase());
        w
    });
    assert!(readme.contains(&doors), "README door count: {doors}");

    let wired = {
        let mut w = word(inner);
        w.replace_range(..1, &w[..1].to_uppercase());
        format!("**{w} wire two occupants to each other")
    };
    assert!(readme.contains(&wired), "README occupant count: {wired}");

    // and the mechanism behind the two the sentence gained: the report leaves
    // at the rim, the nudge does not.
    assert!(edges.iter().any(|e| e["from"] == "./builder"
        && e["to"] == "."
        && e["condition"] == "has(hop.route) && hop.route == 'catalogue'"));
    assert!(
        edges.iter().any(|e| e["from"] == "./operator"
            && e["to"] == "./builder"
            && e["condition"] == NUDGE)
    );
}

#[test]
fn the_shell_template_counts_the_lanes_it_declares() {
    let Some(_) = shipped() else { return };
    let contract = read("templates/meclaw-os/config.json")["params"]["contract"].clone();
    let ins = contract["accepts"].as_array().expect("accepts").len();
    let outs = contract["emits"].as_array().expect("emits").len();

    let sentence = format!(
        "it routes {} lanes in, {} lanes out, and owns the boundary.",
        word(ins),
        word(outs)
    );
    let purpose = read("templates/meclaw-os/template.json")["description"]["purpose"]
        .as_str()
        .expect("purpose")
        .to_string();
    assert!(purpose.contains(&sentence), "template.json: {sentence}");

    // The lane the count grew by is named in the same file's own inventory,
    // and it is the one this issue added.
    assert!(
        contract["emits"]
            .as_array()
            .expect("emits")
            .iter()
            .any(|l| l["route"] == "catalogue")
    );
    let examples = read("templates/meclaw-os/template.json")["description"]["examples"]
        .as_array()
        .expect("examples")
        .iter()
        .filter_map(|e| e.as_str().map(str::to_owned))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        examples.contains("pack_ack and catalogue leave the hive path"),
        "the lane inventory in the examples names the lane too"
    );
}

#[test]
fn the_submit_readme_promises_the_key_on_every_receipt() {
    let Some(_) = shipped() else { return };
    let readme = text("templates/submit/README.md");
    assert!(
        readme.contains("`hop.registers_class` is `true` when"),
        "the README states the promise"
    );
    assert!(
        readme.contains("is stamped on **every** receipt the renderer produces, refusals included"),
        "and states that it is stamped unconditionally"
    );
    // and the mechanism keeps it: both branches of the renderer carry the key.
    for (r, carry, expected) in [
        (1, COMMITTED, true),
        (0, COMMITTED, false),
        (1, REJECTED, true),
    ] {
        let out = pop(row(r), carry);
        assert_eq!(out[out.len() - 1]["header"]["registers_class"], expected);
    }
}

#[test]
fn the_builder_readme_names_the_transit_lane_it_gained() {
    let Some(_) = shipped() else { return };
    let readme = text("templates/builder/README.md");
    assert!(
        readme.contains("in_ingest"),
        "the README names the lane the hive path now takes"
    );
    assert!(
        readme.contains("catalogue"),
        "and the lane the report leaves on"
    );
    // The mechanism behind the sentence: both lanes are declared and both have
    // a door, which is what `check_lane_doors` asks of every contract.
    let params = read("templates/builder/config.json")["params"].clone();
    assert!(
        params["contract"]["accepts"]
            .as_array()
            .expect("accepts")
            .iter()
            .any(|l| l["route"] == "in_ingest")
    );
    assert!(
        params["graph"]["edges"]
            .as_array()
            .expect("edges")
            .iter()
            .any(|e| e["from"] == "." && e["to"] == "./builder-librarian")
    );
}

/// The section that named the undrawable form is the section that has to name
/// the drawable one — GH #496 wrote `./submit/gate -> ./builder/librarian`
/// there, and a reader who trusts a template README over a CHANGELOG would
/// still be trying to draw it.
///
/// Both halves per `docs/development-rules.md` § 2d: the prose is grepped AND
/// the edges it promises are asserted out of the shipped configs.
///
/// Since GH #556 the drawn form is `./operator -> ./builder` over the front
/// door's `sub_receipt` lane: the submitter moved into the front door, so the
/// two hive paths the edge runs between are the operator's and the
/// baumeister's. The README names that form and retracts the previous one
/// (`./submit -> ./builder`) by name, which is the § 3 shape this lock exists
/// to hold — a promise is retired in the text, never silently rewritten.
#[test]
fn the_librarian_readme_names_the_wiring_that_can_actually_be_drawn() {
    let Some(_) = shipped() else { return };
    let readme = text("templates/builder-librarian/README.md");

    // The promise, in the terms the README states it in today.
    assert!(
        readme.contains("`./operator -> ./builder`"),
        "the README must name the edge between the two HIVE PATHS the nudge \
         actually runs between since GH #556"
    );
    assert!(
        readme.contains("`./submit -> ./builder`"),
        "the form the shell drew through `meclaw-os@1.6.1` is retracted BY NAME \
         rather than silently rewritten (§ 3)"
    );
    assert!(
        readme.contains("hop.registers_class"),
        "the key the edge is guarded on is what makes the nudge conditional; \
         without it the section describes a nudge after every receipt"
    );
    assert!(
        readme.contains("in_ingest"),
        "the re-stamped route is the half the librarian's own lane reads"
    );
    // The retraction, per § 3: a promise is retired in the text, never
    // silently rewritten.
    assert!(
        readme.contains("./submit/gate -> ./builder/librarian")
            && readme.contains("undrawable")
            && readme.contains("hive_port_boundary"),
        "the form GH #496 named has to be retracted BY NAME, with the refusal \
         that retires it"
    );

    // …and the mechanism: every edge of the road stands in the shipped configs.
    // Three of them since GH #556 — the receipt leaves the front door, the shell
    // hands it to the builder, and the builder forwards it to the librarian.
    let inside = read("templates/operator/config.json")["params"]["graph"]["edges"].clone();
    let inside = inside.as_array().expect("the operator's edges");
    assert!(
        inside.iter().any(|e| e["from"] == "./submit"
            && e["to"] == "."
            && e["condition"] == SUB_RECEIPT
            && e["modifier"]["set_hop"]["route"] == json!("'sub_receipt'")),
        "the submitter's receipt crosses the front door's rim before the shell sees it"
    );

    let shell = read("templates/meclaw-os/config.json")["params"]["graph"]["edges"].clone();
    let shell = shell.as_array().expect("shell edges");
    let nudge = shell
        .iter()
        .find(|e| e["from"] == "./operator" && e["to"] == "./builder" && e["condition"] == NUDGE)
        .expect("the shell draws the nudge edge the README describes");
    assert_eq!(nudge["modifier"]["set_hop"]["route"], json!("'in_ingest'"));

    let builder = read("templates/builder/config.json")["params"]["graph"]["edges"].clone();
    let builder = builder.as_array().expect("builder edges");
    assert!(
        builder.iter().any(|e| e["from"] == "."
            && e["to"] == "./builder-librarian"
            && e["condition"] == "has(hop.route) && hop.route == 'in_ingest'"),
        "the builder forwards the nudge from its hive path to ./builder-librarian"
    );
    assert!(
        builder.iter().any(|e| e["from"] == "./builder-librarian"
            && e["to"] == "."
            && e["condition"] == "has(hop.route) && hop.route == 'catalogue'"),
        "and lets the report back out, which is the half the README calls the \
         subscription"
    );
}
