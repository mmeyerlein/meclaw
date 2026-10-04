//! GH #929 -- every restoring edge in a shipped template sits on a seam.
//!
//! `modifier.restore_ttl` takes a message's cycle out of the TTL guard, so it
//! is a contract at named seams (`docs/meclaw-overview.md` § Edge model, the
//! seam table), not a local patch. Two seams are the substrate's and carry no
//! edge at all: the source (a fresh root is stamped with
//! `message_default_ttl`) and the peer boundary (`contract.ingress
//! .carries_trace` takes the budget from the wire and restores nothing). Three
//! are edges, and every restoring edge of every template under `templates/`
//! belongs to exactly one row of the table below:
//!
//! - **door** -- a unit of work enters the hive that owns it: `in_turn` of an
//!   assistant generation, a consult between the two brains of a generation,
//!   a file job inside a file space, an index job in a graph space or in a
//!   librarian. Every crossing costs a new turn, model call or job, so no
//!   routing circle returns to the door without one; the condition names the
//!   lane.
//! - **curator entry** -- `./collector -> ./curator` on `curate`, bounded by
//!   the round's iteration counter in the condition.
//! - **round** -- the loopback of a shape whose round is made of routing
//!   (`./curator -> ./brain`, `./weave -> ./compose`), bounded by an
//!   iteration counter in the condition.
//!
//! The table is explicit on purpose: a new restoring edge is a new row, and a
//! row whose edge is gone is a dead row -- both fail here with the template,
//! the edge and what is missing. The edges are counted off the parsed JSON
//! (`modifier.restore_ttl == true`), never off text lines: a `because` that
//! mentions the modifier is prose, not an edge. Every branch of a restoring
//! edge's condition names its lane (and its tool), and every branch of a
//! curator entry or a round carries its counter -- a term in one branch of an
//! `||` bounds nothing. The overview prints the same five seams in the same
//! order in both editions.
//!
//! No colony boots. Guarded like every template-reading test (GH #49).

use meclaw_core::serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seam {
    Door,
    CuratorEntry,
    Round,
}

/// One row of the seam table: the template file (relative to `templates/`),
/// the edge's `from` and `to` as declared, the lane its condition names
/// (`hop.route == '<route>'`, plus `hop.tool_name == '<tool>'` for a tool
/// call) and the seam it sits on.
struct Row {
    template: &'static str,
    from: &'static str,
    to: &'static str,
    route: &'static str,
    tool: Option<&'static str>,
    seam: Seam,
}

const fn row(
    template: &'static str,
    from: &'static str,
    to: &'static str,
    route: &'static str,
    tool: Option<&'static str>,
    seam: Seam,
) -> Row {
    Row {
        template,
        from,
        to,
        route,
        tool,
        seam,
    }
}

const ASSISTANT: &str = "assistant/config.json";
const TALKY: &str = "talky/config.json";
const COGNY: &str = "cogny/config.json";
const BUILDER: &str = "builder/config.json";
const FILE_SPACE: &str = "file-space/config.json";
const MEMBER: &str = "member/config.json";

/// The seam table of the shipped templates.
const SEAMS: &[Row] = &[
    // A person's turn enters the generation that owns it (GH #929).
    row(ASSISTANT, ".", "./talky", "in_turn", None, Seam::Door),
    row(ASSISTANT, ".", "./talky-chat", "in_turn", None, Seam::Door),
    // A consult between the two brains of one generation: each crossing is a
    // model call on the other side.
    row(
        ASSISTANT,
        "./talky",
        "./cogny",
        "tool",
        Some("consult_cogny"),
        Seam::Door,
    ),
    row(
        ASSISTANT,
        "./talky-chat",
        "./cogny",
        "tool",
        Some("consult_cogny"),
        Seam::Door,
    ),
    row(
        ASSISTANT,
        "./talky",
        "./cogny",
        "tool",
        Some("reply_to_consult"),
        Seam::Door,
    ),
    row(
        ASSISTANT,
        "./talky-chat",
        "./cogny",
        "tool",
        Some("reply_to_consult"),
        Seam::Door,
    ),
    row(ASSISTANT, "./cogny", "./talky", "answer", None, Seam::Door),
    row(
        ASSISTANT,
        "./cogny",
        "./talky-chat",
        "answer",
        None,
        Seam::Door,
    ),
    row(ASSISTANT, "./cogny", "./talky", "ask", None, Seam::Door),
    row(
        ASSISTANT,
        "./cogny",
        "./talky-chat",
        "ask",
        None,
        Seam::Door,
    ),
    // The curator entry and the round of both brains.
    row(
        TALKY,
        "./collector",
        "./curator",
        "curate",
        None,
        Seam::CuratorEntry,
    ),
    row(TALKY, "./curator", "./brain", "brain", None, Seam::Round),
    // A close job enters the curator (GH #940, ADR-0002 E8): since a turn of
    // another round seals its channel's generation, a `close` can leave the
    // turn's own chain mid-turn -- the night's leaves a fresh root. The edge
    // is shared, so the night's close loses nothing; nothing routes back to
    // the session keeper's `close` without a new turn or a new night.
    row(
        TALKY,
        "./session-keeper",
        "./curator",
        "close",
        None,
        Seam::Door,
    ),
    // The close pass enters the memory hive that owns it (GH #940): a close
    // a round change sends mid-turn reaches the member having spent the
    // curator's chain, and one group measured 50 decisions against 48 behind
    // the curator's door alone. Nothing routes back without a new close.
    row(
        MEMBER,
        "./assistants",
        "./memory-hive",
        "write",
        None,
        Seam::Door,
    ),
    row(
        COGNY,
        "./collector",
        "./curator",
        "curate",
        None,
        Seam::CuratorEntry,
    ),
    row(COGNY, "./curator", "./brain", "brain", None, Seam::Round),
    // The builder's round: a new composition, and its repair.
    row(BUILDER, "./weave", "./compose", "fire", None, Seam::Round),
    row(BUILDER, "./weave", "./compose", "repair", None, Seam::Round),
    // A file job inside a file space: each request out of `./ingest` is one
    // try of the document (`./extract` for a PDF, `./write` for any other).
    // A `path_taken` runs the whole leg again, and the script bounds the
    // tries (`TRIES` = 3); S5 measures a document raced twice, and the
    // space's own door alone does not carry it.
    row(
        FILE_SPACE,
        "./ingest",
        "./extract",
        "in_extract",
        None,
        Seam::Door,
    ),
    row(
        FILE_SPACE,
        "./ingest",
        "./write",
        "in_write",
        None,
        Seam::Door,
    ),
    // A directory sync in a file space (GH #947, review I-1): each `in_dirs`
    // is one sync of one file -- after a derive job, a move or a remove --
    // and the sync's own reads, claims and rounds are bounded by its script
    // (`SYNC_READS`, `SYNC_TRIES`, `agg_retries`); nothing in it sends
    // `in_dirs` again. The derive road before it (a workspace commit with its
    // embedding and summary) left the sync no budget for a lost round.
    row(
        FILE_SPACE,
        "./write",
        "./derive",
        "in_dirs",
        None,
        Seam::Door,
    ),
    row(FILE_SPACE, "./ws", "./derive", "in_dirs", None, Seam::Door),
    row(
        FILE_SPACE,
        "./derive",
        "./derive",
        "in_dirs",
        None,
        Seam::Door,
    ),
    // A file step of a projection job (GH #975): every request the child
    // hive `./projection` sends into its space -- a workspace op, a read, a
    // write -- is one step of one job for one file, the job's request to the
    // room. The job's file list bounds its steps (the git cell refuses a tree
    // past `import_max_files` = 2000), and no step returns to the door without
    // the job sending the next one. Measured before: a `ws_pull` of six files
    // ran 138 hops against the colony TTL of 64.
    row(
        FILE_SPACE,
        "./projection",
        "./ws",
        "in_ws",
        None,
        Seam::Door,
    ),
    row(
        FILE_SPACE,
        "./projection",
        "./read",
        "in_read",
        None,
        Seam::Door,
    ),
    row(
        FILE_SPACE,
        "./projection",
        "./write",
        "in_write",
        None,
        Seam::Door,
    ),
    // A projection job of a tool call (GH #980): `file_ws_exec`,
    // `file_ws_export` and `file_ws_push` of a writer leave `./tools` for the
    // child hive once per call -- one model call, bounded by the round's own
    // counter at the curator entry -- and nothing in the job returns to
    // `./tools` but its one answer. An export of six files ran out of the
    // budget the call arrived with.
    row(
        FILE_SPACE,
        "./tools",
        "./projection",
        "in_proj",
        None,
        Seam::Door,
    ),
    // An index job in a graph space (GH #945): each `source_changed` is one
    // head move of one source, and the graph space answers it with a constant
    // number of pulls and two store bundles -- nothing in it returns to the
    // door without a new head move. A workspace commit of many files is one
    // door per file, never one chain.
    row(
        MEMBER,
        "./file-space",
        "./graph-space",
        "source_changed",
        None,
        Seam::Door,
    ),
    // An index job in a librarian (GH #950): the same head move of one source,
    // answered with one store bundle and at most two pulls (`info`,
    // `outline`) whose answers write and pull nothing further -- nothing in it
    // returns to the door without a new head move. One door per file, as in
    // the graph space.
    row(
        MEMBER,
        "./file-space",
        "./librarian",
        "source_changed",
        None,
        Seam::Door,
    ),
    // The description of that head (GH #950, OR-BC-68): the space emits
    // `source_described` once its model has summarised the head, a job of its
    // own after a model call; the librarian writes it with one store bundle
    // and sends nothing on -- nothing returns to the door without a new
    // summary.
    row(
        MEMBER,
        "./file-space",
        "./librarian",
        "source_described",
        None,
        Seam::Door,
    ),
    // A recognition becomes a node (GH #951): an object's index job in a
    // graph space. Each `source_changed` out of `./objects` is one version
    // change of one active row, and the chain that carries it started at a
    // turn (a `thing_seen`, a tool write, a facts answer) -- the same seam as
    // the file space's door above, and nothing in the graph space returns to
    // it without a new version.
    row(
        MEMBER,
        "./objects",
        "./graph-space",
        "source_changed",
        None,
        Seam::Door,
    ),
];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    SEAMS
        .iter()
        .all(|r| repo("templates").join(r.template).is_file())
}

fn configs_under(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable") {
        let p = entry.expect("entry").path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != "seed") {
                configs_under(&p, out);
            }
        } else if p.file_name().is_some_and(|n| n == "config.json") {
            out.push(p);
        }
    }
}

/// A restoring edge as found: template file, from, to, condition.
#[derive(Debug)]
struct Found {
    template: String,
    from: String,
    to: String,
    condition: String,
}

impl Found {
    fn say(&self) -> String {
        format!(
            "{}: {} -> {} [{}]",
            self.template, self.from, self.to, self.condition
        )
    }
}

/// Every edge of every `config.json` under `templates/` (nested hives
/// included) whose `modifier.restore_ttl` is `true`, off the parsed JSON.
fn restoring_edges() -> Vec<Found> {
    let root = repo("templates");
    let mut files = Vec::new();
    configs_under(&root, &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let raw = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        let cfg: Value = meclaw_core::serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} does not parse: {e}", f.display()));
        let rel = f
            .strip_prefix(&root)
            .expect("under templates")
            .to_string_lossy()
            .replace('\\', "/");
        for e in cfg["params"]["graph"]["edges"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            if e["modifier"]["restore_ttl"] != Value::Bool(true) {
                continue;
            }
            out.push(Found {
                template: rel.clone(),
                from: e["from"].as_str().unwrap_or_default().to_string(),
                to: e["to"].as_str().unwrap_or_default().to_string(),
                condition: e["condition"].as_str().unwrap_or_default().to_string(),
            });
        }
    }
    out
}

fn fits(r: &Row, f: &Found) -> bool {
    r.template == f.template
        && r.from == f.from
        && r.to == f.to
        && f.condition.contains(&format!("hop.route == '{}'", r.route))
        && r.tool
            .is_none_or(|t| f.condition.contains(&format!("hop.tool_name == '{t}'")))
}

/// Split `s` at every `op` (`&&` or `||`) outside parentheses, brackets and
/// string literals.
fn split_top<'a>(s: &'a str, op: &str) -> Vec<&'a str> {
    let bytes = s.as_bytes();
    let (mut depth, mut quote, mut start, mut i) = (0i32, None::<u8>, 0usize, 0usize);
    let mut parts = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(_) if c == b'\\' => i += 1,
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == b'\'' || c == b'"' => quote = Some(c),
            None if c == b'(' || c == b'[' => depth += 1,
            None if c == b')' || c == b']' => depth -= 1,
            None if depth == 0 && bytes[i..].starts_with(op.as_bytes()) => {
                parts.push(&s[start..i]);
                i += op.len();
                start = i;
                continue;
            }
            None => {}
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

/// `s` without parentheses that wrap all of it.
fn strip_parens(s: &str) -> &str {
    let t = s.trim();
    if !(t.starts_with('(') && t.ends_with(')')) {
        return t;
    }
    let (mut depth, mut quote) = (0i32, None::<char>);
    for (i, c) in t.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 && i != t.len() - 1 {
                        return t;
                    }
                }
                _ => {}
            },
        }
    }
    strip_parens(&t[1..t.len() - 1])
}

/// Whether `condition` can only hold when some term `atom` accepts holds:
/// every `||` branch carries such a term among its `&&` terms. Sufficient,
/// not complete -- a negated or rewritten term does not count, and a term in
/// only one branch of an `||` does not bound the edge (review T, M-1/M-2).
fn implies(condition: &str, atom: &dyn Fn(&str) -> bool) -> bool {
    let s = strip_parens(condition);
    let branches = split_top(s, "||");
    if branches.len() > 1 {
        return branches.iter().all(|b| implies(b, atom));
    }
    let terms = split_top(s, "&&");
    if terms.len() > 1 {
        return terms.iter().any(|t| implies(t, atom));
    }
    atom(s)
}

/// One iteration counter: `int(<context|hop>.<key>) < <n>` or `<= <n>`.
fn is_counter(term: &str) -> bool {
    let Some((inner, after)) = term
        .strip_prefix("int(")
        .and_then(|rest| rest.split_once(')'))
    else {
        return false;
    };
    let key = inner
        .strip_prefix("context.")
        .or_else(|| inner.strip_prefix("hop."));
    let after = after.trim_start();
    let bound = after
        .strip_prefix("<=")
        .or_else(|| after.strip_prefix('<'))
        .map(str::trim);
    matches!((key, bound), (Some(k), Some(b))
        if !k.is_empty()
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !b.is_empty()
            && b.chars().all(|c| c.is_ascii_digit()))
}

/// Does the condition bound the cycle with an iteration counter in every
/// branch?
fn has_counter(condition: &str) -> bool {
    implies(condition, &is_counter)
}

/// Does every branch of the condition require `<key> == '<value>'`?
fn requires(condition: &str, key: &str, value: &str) -> bool {
    let want = format!("{key} == '{value}'");
    implies(condition, &|t: &str| t == want)
}

#[test]
fn the_counter_rule_reads_what_the_templates_write() {
    assert!(has_counter("has(hop.iter) && int(hop.iter) < 12"));
    assert!(has_counter("int(context.iter) < 6"));
    assert!(has_counter("has(hop.repairs) && int(hop.repairs) <= 2"));
    assert!(has_counter("(int(hop.iter) < 3) && hop.route == 'x'"));
    assert!(!has_counter("has(hop.route) && hop.route == 'turn'"));
    assert!(!has_counter("int(hop.iter) > 3"));
    assert!(!has_counter("int(hop.iter) < limit"));
    assert!(!has_counter("int(hop.a.b) < 3"));
    // A counter in one branch of an `||` bounds nothing (review T, M-2).
    assert!(!has_counter("int(hop.iter) < 3 || hop.route == 'x'"));
    assert!(!has_counter(
        "has(hop.route) && (hop.route == 'x' || int(hop.iter) < 3)"
    ));
    assert!(!has_counter("!(int(hop.iter) < 3)"));
}

#[test]
fn the_lane_rule_reads_what_the_templates_write() {
    let turn = "has(hop.route) && hop.route == 'in_turn' && \
                (!has(context.channel_node) || context.channel_node != 'chat')";
    assert!(requires(turn, "hop.route", "in_turn"));
    assert!(requires(
        "hop.route == 'tool' && has(hop.tool_name) && hop.tool_name == 'consult_cogny'",
        "hop.tool_name",
        "consult_cogny"
    ));
    // A door widened by an `||` is no longer the door of its lane (review T,
    // M-1): the lane has to hold in every branch.
    assert!(!requires(
        "hop.route == 'in_turn' || hop.route == 'in_sweep'",
        "hop.route",
        "in_turn"
    ));
    assert!(!requires(
        "hop.route == 'in_turn' || has(hop.other)",
        "hop.route",
        "in_turn"
    ));
    assert!(!requires(
        "hop.route in ['in_turn', 'in_sweep']",
        "hop.route",
        "in_turn"
    ));
    assert!(!requires("hop.route != 'in_turn'", "hop.route", "in_turn"));
    assert!(!requires("'a||b' == hop.x", "hop.route", "in_turn"));
}

#[test]
fn every_restoring_edge_sits_on_a_seam() {
    if !shipped() {
        eprintln!("a template of the seam table did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let found = restoring_edges();
    let mut problems = Vec::new();
    for f in &found {
        let rows: Vec<&Row> = SEAMS.iter().filter(|r| fits(r, f)).collect();
        match rows.len() {
            1 => {}
            0 => problems.push(format!(
                "restoring edge on no seam (add a row, or drop the modifier): {}",
                f.say()
            )),
            n => problems.push(format!("restoring edge on {n} seam rows: {}", f.say())),
        }
    }
    for r in SEAMS {
        let n = found.iter().filter(|f| fits(r, f)).count();
        if n != 1 {
            problems.push(format!(
                "seam row finds {n} restoring edges, not one: {}: {} -> {} on '{}'{}",
                r.template,
                r.from,
                r.to,
                r.route,
                r.tool.map(|t| format!(" ({t})")).unwrap_or_default()
            ));
        }
    }
    eprintln!("gh929 restoring edges in templates: {}", found.len());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn a_curator_entry_and_a_round_carry_their_counter_and_a_door_names_its_lane() {
    if !shipped() {
        eprintln!("a template of the seam table did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let found = restoring_edges();
    let mut problems = Vec::new();
    for r in SEAMS {
        assert!(!r.route.is_empty(), "every row names its lane");
        for f in found.iter().filter(|f| fits(r, f)) {
            if matches!(r.seam, Seam::CuratorEntry | Seam::Round) && !has_counter(&f.condition) {
                problems.push(format!(
                    "{:?} without an iteration counter in every branch of its condition: {}",
                    r.seam,
                    f.say()
                ));
            }
            if !requires(&f.condition, "hop.route", r.route) {
                problems.push(format!(
                    "{:?} without its lane '{}' in every branch of its condition: {}",
                    r.seam,
                    r.route,
                    f.say()
                ));
            }
            if let Some(t) = r.tool
                && !requires(&f.condition, "hop.tool_name", t)
            {
                problems.push(format!(
                    "{:?} without its tool '{t}' in every branch of its condition: {}",
                    r.seam,
                    f.say()
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The seam table of the overview, row by row: its German and its English
/// name, and the seam of [`SEAMS`] it stands for (`None`: the substrate's
/// seam, which carries no edge).
const DOC_ROWS: [(&str, &str, Option<Seam>); 5] = [
    ("Quelle", "Source", None),
    ("Peer-Grenze", "Peer boundary", None),
    ("Tür", "Door", Some(Seam::Door)),
    (
        "Kurator-Eintritt",
        "Curator entry",
        Some(Seam::CuratorEntry),
    ),
    ("Runde", "Round", Some(Seam::Round)),
];

/// The body rows of the first table after the paragraph that opens with
/// `lead`, cell by cell (header and rule dropped).
fn seam_table(doc: &str, lead: &str) -> Vec<Vec<String>> {
    let at = doc
        .find(lead)
        .unwrap_or_else(|| panic!("the overview carries the paragraph {lead:?}"));
    doc[at..]
        .lines()
        .skip_while(|l| !l.trim_start().starts_with('|'))
        .take_while(|l| l.trim_start().starts_with('|'))
        .skip(2)
        .map(|l| {
            l.trim()
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect()
}

/// The table this file enforces is the table the overview prints: five seams
/// in the same order in both editions, the substrate's two without an edge,
/// the other three the seams of [`SEAMS`]. The export compares the editions
/// by their headings only, and this table has none (review T, I-1).
///
/// Privately `docs/meclaw-overview.md` is the German edition and the English
/// one sits beside it; the published tree carries the English bytes under the
/// plain name and no `.en.md` -- so the second edition is read only where it
/// is a file.
#[test]
fn the_overview_prints_the_seam_table_in_both_editions() {
    let plain = repo("docs/meclaw-overview.md");
    if !plain.is_file() {
        eprintln!("the overview did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let read = |p: &std::path::Path| {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    };
    let english = repo("docs/meclaw-overview.en.md");
    let editions = if english.is_file() {
        vec![
            ("German", read(&plain), "**Nähte von `restore_ttl`", false),
            ("English", read(&english), "**Seams of `restore_ttl`", true),
        ]
    } else {
        vec![("English", read(&plain), "**Seams of `restore_ttl`", true)]
    };
    let mut problems = Vec::new();
    for (edition, doc, lead, en) in &editions {
        let rows = seam_table(doc, lead);
        let names: Vec<&str> = rows.iter().map(|r| r[0].trim_matches('*')).collect();
        let want: Vec<&str> = DOC_ROWS
            .iter()
            .map(|(de, en_name, _)| if *en { *en_name } else { *de })
            .collect();
        if names != want {
            problems.push(format!(
                "{edition}: the seam table names {names:?}, not {want:?}"
            ));
            continue;
        }
        for (row, (_, _, seam)) in rows.iter().zip(DOC_ROWS) {
            let how = row.last().map(|c| c.to_lowercase()).unwrap_or_default();
            let edge = how.starts_with("kante") || how.starts_with("edge");
            let substrate = how.starts_with("substrat");
            if row.len() != 4 || edge != seam.is_some() || substrate != seam.is_none() {
                problems.push(format!(
                    "{edition}: the row {:?} is not a four-cell row whose last cell says {}: {row:?}",
                    row[0],
                    if seam.is_some() { "edge" } else { "substrate" }
                ));
            }
        }
    }
    for kind in [Seam::Door, Seam::CuratorEntry, Seam::Round] {
        let documented = DOC_ROWS.iter().any(|(_, _, s)| *s == Some(kind));
        let used = SEAMS.iter().any(|r| r.seam == kind);
        if documented != used {
            problems.push(format!(
                "{kind:?}: documented {documented}, used by a row of SEAMS {used}"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
