//! GH #284 — no shipped topology silences a `reject` or an `error`.
//!
//! # The invariant
//!
//! A lane that reports a refusal has exactly **two** honest states:
//!
//! 1. **It has a consumer that does something.** Emitting nothing is fine;
//!    *recording* nothing is not. A cell that writes the refusal where an
//!    operator sees it — a store row, stderr, an alert — is a consumer even if
//!    it emits `[]`.
//! 2. **It has no edge at all**, and the emission becomes `no_route` in the
//!    dead-letter queue, where it localises itself with its sender and its
//!    trace.
//!
//! What it must never have is the third arrangement: a cell that accepts the
//! refusal and drops it. That is the one shape in which nobody finds out, and
//! it is the shape this file forbids in every artifact this repository ships.
//! Ruling Q2 (2026-08-21) chose state (2) as the default for the shipped
//! examples: **the DLQ is the record.**
//!
//! # The baseline this file was written against
//!
//! Measured on the tree at the start of W7 Task 5, before any edit. Four edges
//! matched the discriminator below, all four ending in a node instantiated from
//! `terminal`, and all four were deleted in the same commit as this file:
//!
//! | file | edge | condition |
//! |---|---|---|
//! | `examples/meclaw-os/grow.json` (`:56-68`) | `./firewall -> ./sink` | `hop.route == 'reject'` |
//! | `examples/meclaw-os/grow.json` (`:79-83`) | `./talky -> ./sink` | `hop.route == 'error'` |
//! | `examples/meclaw-os/grow-steward.json` (`:14-18`) | `./steward -> ./sink` | `hop.route == 'error'` |
//! | `examples/never-forgets/grow.json` (`:122-126`) | `./talky -> ./sink` | `hop.route == 'error'` |
//!
//! None of the four was load-bearing: `firewall@2.0.4` and `steward@2.0.10`
//! declare no `required_drains` at all, and `talky@4.2.0` declares exactly one
//! pairing, `in_prune -> prune`, which no sink edge served. So all four became
//! `no_route`, which is state (2).
//!
//! **What stays, and why it is not a defect.** The same `terminal` instances are
//! still reached by lanes that are genuinely undecided rather than refused —
//! `./talky -> ./sink` on `answer` (an answer nothing sends back out yet) and on
//! `turn_write` (a finished turn with no memory behind it). Those are the
//! template's documented job: *a lane that ends HERE is a decision you have not
//! made yet.* A refusal is not undecided; somebody refused something.
//!
//! # The discriminator, and where it deliberately stops
//!
//! An edge is in scope when its `condition` names `hop.route == 'reject'` or
//! `hop.route == 'error'`. Its `to` endpoint is a **silencer** when it resolves
//! to either
//!
//! * a node instantiated from the `terminal` template, or
//! * a `code` cell whose `script_inline` writes `[]` to stdout — every write, not
//!   just one branch — and writes **nothing** to stderr and **nothing** to a
//!   store.
//!
//! That last clause is the whole distinction, and it is what keeps the real
//! consumers out of the net: `templates/talky/errors` re-emits a normalised
//! error, and `templates/research-assistant/drain` and
//! `templates/coder-pipeline/drain` write an `errors` row through a store
//! `tool_call` — each of them writes `[]` on *some* path and is still a
//! consumer, because on the refusal path it records.
//!
//! **Two deliberate boundaries, named so they are arguments and not oversights:**
//!
//! * **`hop.error_code`, `hop.finish_reason` and friends are out of scope.**
//!   `examples/hard-shell/grow.json` ends two lanes in its `terminal` keyed on
//!   `hop.error_code` (`target_blocked`, `timeout`, …) — by the letter of the
//!   discriminator that is not a hit, and widening the rule here would rewrite
//!   that example's whole narrative rather than enforce #284's. Whether a
//!   denial reported on `hop.error_code` deserves the same treatment is a
//!   decision for #284, not a silent extension of a gate.
//! * **An endpoint of `.` is never a silencer.** A hive is a scope marker, not
//!   an actor — an edge to `.` hands the lane out of the composite to whoever
//!   wired it, which is the opposite of swallowing it.

use meclaw_core::serde_json::{Value, json};

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(p: &std::path::Path) -> Option<Value> {
    let raw = std::fs::read_to_string(p).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

fn collect(dir: &std::path::Path, want: Option<&str>, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect(&p, want, out);
        } else if p.extension().and_then(|e| e.to_str()) == Some("json") {
            match want {
                Some(name) if p.file_name().and_then(|n| n.to_str()) != Some(name) => {}
                _ => out.push(p),
            }
        }
    }
}

// ────────────────────────────────────────────────────── the two classifiers

/// Is this condition one #284 governs?
///
/// Only the two literal route names. `hop.finish_reason == 'error'` is a
/// provider outcome on a working lane, not a refusal report, and it is out of
/// scope by the same reasoning that keeps `hop.error_code` out (see the header).
fn is_refusal_condition(condition: &str) -> bool {
    let compact: String = condition.chars().filter(|c| !c.is_whitespace()).collect();
    compact.contains("hop.route=='reject'") || compact.contains("hop.route=='error'")
}

/// Every argument of a `sys.stdout.write(...)` call in a python script.
fn stdout_writes(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = "sys.stdout.write(";
    let bytes: Vec<char> = script.chars().collect();
    let mut from = 0usize;
    while let Some(rel) = script[from..].find(needle) {
        let open = from + rel + needle.len();
        // Walk to the matching close paren, counting nesting.
        let mut depth = 1i32;
        let mut idx = script[..open].chars().count();
        let mut arg = String::new();
        while idx < bytes.len() && depth > 0 {
            let c = bytes[idx];
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            arg.push(c);
            idx += 1;
        }
        out.push(arg.trim().to_string());
        from = open;
    }
    out
}

/// A `code` cell that emits nothing AND records nothing — the silencer of #284.
///
/// Conservative on purpose: it takes at least one stdout write to judge, and
/// every one of them has to be an empty array. A cell that writes `[]` on one
/// branch and a store `tool_call` on another is a consumer.
fn is_swallowing_code(params: &Value) -> bool {
    let Some(script) = params.get("script_inline").and_then(Value::as_str) else {
        return false;
    };
    if script.contains("sys.stderr") {
        return false;
    }
    // A store round trip is a record: the refusal lands in a table an operator
    // can read. Both spellings the shipped drains use.
    if script.contains("tool_call")
        || script.contains("\"operation\"")
        || script.contains("'operation'")
    {
        return false;
    }
    let writes = stdout_writes(script);
    if writes.is_empty() {
        return false;
    }
    writes
        .iter()
        .all(|w| matches!(w.as_str(), "\"[]\"" | "'[]'" | "json.dumps([])"))
}

/// Does the config at this path describe a silencer?
fn config_is_silencer(cfg: &Value) -> bool {
    if cfg["cell"]["type"] != "code" {
        return false;
    }
    is_swallowing_code(&cfg["params"])
}

// ──────────────────────────────────────────────────── resolving an endpoint

/// `terminal`, from either `"terminal"` or `"terminal@1.0.1"`.
fn template_name(raw: &str) -> &str {
    raw.split('@').next().unwrap_or(raw)
}

/// Follow `cell.type: "ref"` to the standalone template it names.
fn resolve_ref(mut dir: std::path::PathBuf) -> std::path::PathBuf {
    for _ in 0..8 {
        let Some(cfg) = read_json(&dir.join("config.json")) else {
            return dir;
        };
        if cfg["cell"]["type"] != "ref" {
            return dir;
        }
        let Some(t) = cfg["cell"]["template"].as_str() else {
            return dir;
        };
        dir = repo(&format!("templates/{}", template_name(t)));
    }
    dir
}

/// What a `to` endpoint of a template-internal edge points at.
///
/// `base` is the directory the `config.json` carrying the edge lives in.
/// `Some(dir)` is the directory of the addressed cell; `None` means the edge
/// leaves the composite (`.`, `..`, an absolute colony path) and is therefore
/// nobody's silencer.
fn resolve_internal(base: &std::path::Path, to: &str) -> Option<std::path::PathBuf> {
    let rest = to.strip_prefix("./")?;
    if rest.is_empty() {
        return None;
    }
    let mut dir = base.to_path_buf();
    for seg in rest.split('/') {
        dir = resolve_ref(dir).join(seg);
    }
    Some(resolve_ref(dir))
}

/// What a `to` endpoint of a declaration edge points at.
///
/// `nodes` maps the declaration's node names to the templates they grow from.
/// A first segment that is not a declared node is a seed cell, resolved under
/// the example's own `seed/main`.
fn resolve_declared(
    example_dir: &std::path::Path,
    nodes: &[(String, String)],
    to: &str,
) -> Option<(std::path::PathBuf, Option<String>)> {
    let rest = to.strip_prefix("./")?;
    if rest.is_empty() {
        return None;
    }
    let mut segs = rest.split('/');
    let head = segs.next()?;
    let tail: Vec<&str> = segs.collect();
    let (dir, template) = match nodes.iter().find(|(name, _)| name == head) {
        Some((_, t)) => (repo(&format!("templates/{t}")), Some(t.clone())),
        None => (example_dir.join("seed/main").join(head), None),
    };
    let mut dir = resolve_ref(dir);
    for seg in &tail {
        dir = resolve_ref(dir.join(seg));
    }
    // Only the ROOT of an instantiated node carries that node's template
    // identity; a path into it addresses a cell of the composite instead.
    let template = if tail.is_empty() { template } else { None };
    Some((dir, template))
}

// ─────────────────────────────────────────────────────────────── the sweep

/// One line per silencer edge, in the shape a reader can go and delete.
///
/// Formatted at the find rather than carried in a struct: the assertion message
/// is the whole product of this file, so there is nothing to keep the parts
/// apart for.
fn hit_line(file: &std::path::Path, edge: &Value, why: &str) -> String {
    format!(
        "{}: {} -> {} on `{}` — {why}",
        file.display(),
        edge["from"].as_str().unwrap_or("?"),
        edge["to"].as_str().unwrap_or("?"),
        edge["condition"].as_str().unwrap_or("?"),
    )
}

/// Fewest in-scope edges the sweep must actually look at. A sweep that finds
/// nothing because it *scanned* nothing passes for free.
///
/// Both floors are set for the SMALLER of the two trees this file has to be
/// green in. Counted before the commit that added this file, not guessed: the
/// full tree carries **26** `reject`/`error` edges across **191** documents,
/// the published subset **15** across **105** — the difference is the thirteen
/// templates that do not travel. Ten and thirty sit below the smaller count and
/// far above zero.
const MIN_EDGES_EXAMINED: usize = 10;

/// Fewest documents the sweep must read, same reasoning and the same two
/// measurements (191 private, 105 public).
const MIN_DOCUMENTS_SCANNED: usize = 30;

fn sweep() -> (Vec<String>, usize, usize) {
    let mut hits = Vec::new();
    let mut examined = 0usize;
    let mut documents = 0usize;

    // ── templates: every config.json, at every depth
    let mut files = Vec::new();
    collect(&repo("templates"), Some("config.json"), &mut files);
    files.sort();
    for path in &files {
        let Some(cfg) = read_json(path) else { continue };
        documents += 1;
        let base = path.parent().expect("a config.json has a directory");
        let Some(edges) = cfg["params"]["graph"]["edges"].as_array() else {
            continue;
        };
        for e in edges {
            let condition = e["condition"].as_str().unwrap_or_default();
            if !is_refusal_condition(condition) {
                continue;
            }
            examined += 1;
            let to = e["to"].as_str().unwrap_or_default();
            let Some(target) = resolve_internal(base, to) else {
                continue;
            };
            let Some(target_cfg) = read_json(&target.join("config.json")) else {
                continue;
            };
            if config_is_silencer(&target_cfg) {
                hits.push(hit_line(
                    path,
                    e,
                    &format!(
                        "{} is a code cell that emits [] and records nothing",
                        target.display()
                    ),
                ));
            }
        }
    }

    // ── examples: every *.json, declaration or seed config
    let mut files = Vec::new();
    collect(&repo("examples"), None, &mut files);
    files.sort();
    for path in &files {
        let Some(doc) = read_json(path) else { continue };
        documents += 1;
        let dir = path.parent().expect("a json file has a directory");

        // a mutation declaration
        if let Some(edges) = doc["diff"]["add_edges"].as_array() {
            let nodes: Vec<(String, String)> = doc["diff"]["add_nodes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| {
                            Some((
                                n["name"].as_str()?.to_string(),
                                template_name(n["template"].as_str()?).to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            // A declaration sits beside the seed it grows.
            for e in edges {
                let condition = e["condition"].as_str().unwrap_or_default();
                if !is_refusal_condition(condition) {
                    continue;
                }
                examined += 1;
                let to = e["to"].as_str().unwrap_or_default();
                let Some((target, template)) = resolve_declared(dir, &nodes, to) else {
                    continue;
                };
                let why = if template.as_deref() == Some("terminal") {
                    Some("the node grows from the `terminal` template".to_string())
                } else {
                    read_json(&target.join("config.json"))
                        .filter(config_is_silencer)
                        .map(|_| {
                            format!(
                                "{} is a code cell that emits [] and records nothing",
                                target.display()
                            )
                        })
                };
                if let Some(why) = why {
                    hits.push(hit_line(path, e, &why));
                }
            }
        }

        // a seed cell's own graph
        if let Some(edges) = doc["params"]["graph"]["edges"].as_array() {
            for e in edges {
                let condition = e["condition"].as_str().unwrap_or_default();
                if !is_refusal_condition(condition) {
                    continue;
                }
                examined += 1;
                let to = e["to"].as_str().unwrap_or_default();
                let Some(target) = resolve_internal(dir, to) else {
                    continue;
                };
                let Some(target_cfg) = read_json(&target.join("config.json")) else {
                    continue;
                };
                if config_is_silencer(&target_cfg) {
                    hits.push(hit_line(
                        path,
                        e,
                        &format!(
                            "{} is a code cell that emits [] and records nothing",
                            target.display()
                        ),
                    ));
                }
            }
        }
    }

    (hits, examined, documents)
}

#[test]
fn no_shipped_topology_routes_a_refusal_into_a_cell_that_swallows_it() {
    let (hits, examined, documents) = sweep();
    assert!(
        documents >= MIN_DOCUMENTS_SCANNED,
        "the sweep read almost nothing ({documents} documents) — it is passing for free"
    );
    assert!(
        examined >= MIN_EDGES_EXAMINED,
        "the sweep found almost no reject/error edges ({examined}) — the condition \
         matcher stopped matching"
    );
    assert!(
        hits.is_empty(),
        "a shipped artifact routes a refusal into a cell that swallows it. A \
         reject/error lane has two honest states (a consumer that records, or no \
         edge and the DLQ) and this is neither — GH #284:\n{}",
        hits.join("\n")
    );
}

// ───────────────────────────────────────────────────────── the test of the test
//
// The sweep is green on the tree as it stands, so on its own it would be green
// whether the classifiers work or not. These drive fabricated input through the
// SAME functions the sweep uses — no file is touched.

#[test]
fn the_condition_matcher_reads_the_two_route_names_and_nothing_else() {
    assert!(is_refusal_condition(
        "has(hop.route) && hop.route == 'reject'"
    ));
    assert!(is_refusal_condition(
        "has(hop.route) && hop.route == 'error'"
    ));
    assert!(is_refusal_condition(
        "has(hop.route)&&(hop.route == 'ack' || hop.route == 'error')"
    ));
    // Out of scope by the header's two boundaries.
    assert!(!is_refusal_condition(
        "has(hop.finish_reason) && hop.finish_reason == 'error'"
    ));
    assert!(!is_refusal_condition(
        "has(hop.error_code) && hop.error_code == 'target_blocked'"
    ));
    assert!(!is_refusal_condition(
        "has(hop.route) && hop.route == 'answer'"
    ));
}

#[test]
fn the_shipped_terminal_is_a_silencer_and_the_shipped_drains_are_not() {
    let terminal = read_json(&repo("templates/terminal/config.json")).expect("terminal ships");
    assert!(
        config_is_silencer(&terminal),
        "the terminal stopped being classifiable as a silencer — either its script \
         changed or the classifier did, and either way this gate stopped biting"
    );

    // Real consumers: each writes `[]` on some path and still records on the
    // refusal path. If the classifier ever calls one of these a silencer, the
    // gate would demand deleting a working error lane.
    //
    // Guarded per file rather than asserted: two of the three live in templates
    // that do not travel with the published subset, and a missing template must
    // skip cleanly instead of failing on a dead path. `talky` travels, so the
    // floor below holds in both trees.
    let mut checked = 0usize;
    for consumer in [
        "templates/talky/errors/config.json",
        "templates/research-assistant/drain/config.json",
        "templates/coder-pipeline/drain/config.json",
    ] {
        let Some(cfg) = read_json(&repo(consumer)) else {
            continue;
        };
        checked += 1;
        assert!(
            !config_is_silencer(&cfg),
            "{consumer} was classified as a silencer, but it records the refusal"
        );
    }
    assert!(
        checked >= 1,
        "not one recording consumer was on disk — the negative half of the \
         classifier proved nothing"
    );
}

#[test]
fn a_fabricated_swallowing_cell_is_caught_and_a_recording_one_is_not() {
    let swallower = json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": "import sys\nsys.stdout.write(\"[]\")\n"
        }
    });
    assert!(config_is_silencer(&swallower));

    // Emits nothing, records to stderr. State (1) of #284, not a silencer.
    let to_stderr = json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline":
                "import sys\nsys.stderr.write(\"refused\")\nsys.stdout.write(\"[]\")\n"
        }
    });
    assert!(
        !config_is_silencer(&to_stderr),
        "a cell that emits nothing but WRITES the refusal to stderr is state (1) of #284"
    );

    // Emits nothing on this branch AND writes a store row. Also state (1) —
    // this is the shape the shipped drains have, so the store clause has to
    // outrank the empty-emission one.
    let to_store = json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline":
                "import sys, json\nargs = {\"operation\": \"insert\", \"table\": \"errors\"}\nsys.stdout.write(json.dumps([]))\n"
        }
    });
    assert!(
        !config_is_silencer(&to_store),
        "a cell that emits [] and writes the refusal into a store is state (1) of #284"
    );

    let not_code = json!({
        "cell": {"type": "store"},
        "params": {"script_inline": "import sys\nsys.stdout.write(\"[]\")\n"}
    });
    assert!(
        !config_is_silencer(&not_code),
        "only a code cell is judged by its script"
    );
}

#[test]
fn a_fabricated_silencer_edge_would_be_reported() {
    // The shape the four deleted edges had, run through the same two steps the
    // sweep performs on a declaration edge: match the condition, then resolve
    // the endpoint to the template it grows from.
    let nodes = vec![("sink".to_string(), "terminal".to_string())];
    let condition = "has(hop.route) && hop.route == 'reject'";
    assert!(is_refusal_condition(condition));
    let (_dir, template) =
        resolve_declared(&repo("examples/meclaw-os"), &nodes, "./sink").expect("./sink resolves");
    assert_eq!(
        template.as_deref(),
        Some("terminal"),
        "the endpoint resolver stopped recognising a node grown from `terminal`"
    );
}
