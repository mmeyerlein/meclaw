//! The librarian on a booted colony, for the locks of GH #950: the shipped
//! `templates/librarian` hive by itself, laid out the way instantiation lays a
//! template out (`copy_resolved` of `support/graph_space_colony.rs`). Every
//! lane that leaves the hive is drained to a capture: `pull` to `/pulls` (the
//! test plays the file space and the graph space and answers on `in_pulled`,
//! in any order it chooses), `answer` to `/lsink`, `tool_result` to `/tsink`,
//! `tool_schemas` to `/msink`, any other lane the hive declares to `/park`.
//!
//! The test plays the file space on both of its lanes into the hive:
//! `source_changed` when a head moves and `source_described` once the
//! space's model has summarised that head (GH #950, OR-BC-68). A space
//! announces a head before its summary exists, so the `info` it answers on an
//! announcement carries the summary of the head before; the catalog takes
//! the summary line and the tags from `source_described` alone.
//!
//! Measured at the receivers: the librarian's own `cell.db` (its catalog
//! table `entries`), the colony's `message_log`, the captures. The librarian
//! has no model, no embedder and no network, so nothing paid is reachable by
//! construction.
//!
//! The including test file declares at its root
//! `#[path = "mock_openai.rs"] mod mock_openai;`,
//! `#[path = "support/graph_space_colony.rs"] mod space;` and
//! `#[path = "support/librarian_hive.rs"] mod librarian;`: the generic half
//! (`copy_resolved`, `message_log`, `next_matching`, `quiet`, the readers) is
//! the graph space's harness, reused rather than copied.
#![allow(dead_code)]

use crate::space::{self, DEADLINE, Logged, body_of, hop_str, next_matching, read_json, repo};
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, MESSAGE_DEFAULT_TTL, Message, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// The hive under test and its cells, as the colony addresses them.
pub const HIVE: &str = "/librarian";
pub const INDEX: &str = "/librarian/index";
pub const QUERY: &str = "/librarian/query";
pub const TOOLS: &str = "/librarian/tools";
pub const SCHEMAS: &str = "/librarian/schemas";
pub const STORE: &str = "/librarian/store";

/// The four lanes the librarian emits (GH #950 § 1) and the capture each one
/// is drained to.
pub const SINKS: [(&str, &str); 4] = [
    ("pull", "/pulls"),
    ("answer", "/lsink"),
    ("tool_result", "/tsink"),
    ("tool_schemas", "/msink"),
];

/// The columns of the catalog table `entries` (GH #950 § 2), every one `text`.
pub const COLUMNS: [&str; 15] = [
    "source",
    "path",
    "dir",
    "kind",
    "fmt",
    "oneline",
    "tags",
    "names",
    "nodes",
    "version",
    "announced",
    "changed_at",
    "tomb",
    "pterms",
    "nterms",
];

// ═══════════════════════════════════════════════════════════════ the shipped tree

/// GH #49 and GH #950. `false`: this tree carries no template library at all
/// (the published one), and the lock skips. A tree that carries the member
/// carries the librarian the member seats, so there the template is required
/// and its absence is red -- that is what makes these locks the TDD locks of
/// #950 rather than locks that skip until someone remembers them.
pub fn shipped() -> bool {
    if !repo("templates/member/config.json").is_file() {
        return false;
    }
    assert!(
        repo("templates/librarian/config.json").is_file(),
        "GH #950: the template library carries no `templates/librarian` -- the member's \
         librarian has no template to be built from"
    );
    true
}

/// The shipped config of one cell of the librarian.
pub fn cell_config(cell: &str) -> Value {
    read_json(&repo(&format!("templates/librarian/{cell}/config.json")))
}

// ═════════════════════════════════════════════════════════════════════ the layout

fn drain(lane: &str, to: &str) -> Value {
    json!({"from": "./librarian", "to": to,
           "condition": format!("has(hop.route) && hop.route == '{lane}'")})
}

/// Lay the colony out: the librarian under `/librarian`, every lane of its rim
/// drained to a capture.
pub fn build(td: &tempfile::TempDir) {
    let main = td.path().join("main");
    space::copy_resolved(&repo("templates/librarian"), &main.join("librarian"), 0);
    let mut edges: Vec<Value> = SINKS.iter().map(|(lane, to)| drain(lane, to)).collect();
    for lane in space::rim_emits("librarian") {
        if SINKS.iter().all(|(known, _)| *known != lane) {
            edges.push(drain(&lane, "/park"));
        }
    }
    space::write_json(
        &main.join("config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
}

pub struct Ports {
    pub pulls: mpsc::Receiver<Message>,
    pub lsink: mpsc::Receiver<Message>,
    pub tsink: mpsc::Receiver<Message>,
    pub msink: mpsc::Receiver<Message>,
    /// Held, not dropped: a capture whose receiver is gone turns every
    /// delivery into a send error.
    pub park: mpsc::Receiver<Message>,
}

/// Boot the colony [`build`] laid out. Only the two cell types the librarian
/// is made of are registered: a cell of any other type (a model, a timer) is
/// off its build spec and fails the boot.
pub async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, Ports) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let mut rx = Vec::new();
    for name in ["/pulls", "/lsink", "/tsink", "/msink", "/park"] {
        let (tx, r) = mpsc::channel::<Message>(4096);
        h.spawn(Path::new(name), move || CaptureCell::new(tx.clone()))
            .await;
        rx.push(r);
    }
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped librarian must boot");
    let mut it = rx.into_iter();
    let mut next = || it.next().expect("a receiver");
    let ports = Ports {
        pulls: next(),
        lsink: next(),
        tsink: next(),
        msink: next(),
        park: next(),
    };
    (h, ports)
}

// ═══════════════════════════════════════════════════════════════════ the messages

pub fn message(to: &str, hop: Value, ctx: Value, body: Value) -> Message {
    MessageBuilder::new(Path::new(to))
        .hop(space::map(hop))
        .context(space::map(ctx))
        .body(Body::Inline(body))
        .ttl(MESSAGE_DEFAULT_TTL)
        .build()
}

/// The version a pull names: the announced one cut to twelve, lower case
/// (GH #950 § 3).
pub fn v12(version: &str) -> String {
    version.chars().take(12).collect::<String>().to_lowercase()
}

/// The parent directory of a path: `/pkg/a.py` -> `/pkg`, `/a` -> `/`.
pub fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(i) => path[..i].to_string(),
    }
}

/// The format the file space's extractor table names for a path (its
/// `EX_EXT`, and `pdf`); '' for a path no extractor reads.
pub fn fmt_of(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or_default().to_lowercase();
    match name.rfind('.').filter(|d| *d > 0).map(|d| &name[d..]) {
        Some(".py") => "python",
        Some(".rs") => "rust",
        Some(".md") | Some(".markdown") => "markdown",
        Some(".json") => "json",
        Some(".toml") => "toml",
        Some(".pdf") => "pdf",
        _ => "",
    }
}

/// An announcement at the librarian, in the file space's own form (its
/// `source_changed`: `{source, version, path, fmt, parser, mark, nodes,
/// links, tomb: false}`).
pub fn announce(source: &str, version: &str, path: &str, fmt: &str, nodes: u64) -> Message {
    message(
        HIVE,
        json!({"route": "source_changed"}),
        json!({}),
        json!({"source": source, "version": version, "path": path, "fmt": fmt,
               "parser": fmt, "mark": "", "nodes": nodes, "links": 0, "tomb": false,
               "messages": []}),
    )
}

/// A removal, in the file space's own form: `{source, path, tomb: true}` and
/// no version.
pub fn removal(source: &str, path: &str) -> Message {
    message(
        HIVE,
        json!({"route": "source_changed"}),
        json!({}),
        json!({"source": source, "path": path, "tomb": true, "messages": []}),
    )
}

/// A description at the librarian, in the file space's own form (its
/// `source_described`: `{source, version, path, oneline, tags}`, the head
/// only, after its `source_changed`). GH #950: a space announces a head
/// before its model has summarised it, so the summary line and the tags of a
/// head travel on this lane of their own, once the summary is written.
pub fn described(source: &str, version: &str, path: &str, oneline: &str, tags: Value) -> Message {
    message(
        HIVE,
        json!({"route": "source_described"}),
        json!({}),
        json!({"source": source, "version": version, "path": path, "oneline": oneline,
               "tags": tags, "messages": []}),
    )
}

/// The summary line an `info` answer carries on an announcement: the one of
/// the head BEFORE, because the space's model has not summarised the new head
/// yet (GH #950). The catalog never takes it, so no lock ever finds it; a
/// word of it in a row or a `find` hit is the bug.
pub const STALE_ONELINE: &str = "A superseded summary line.";

/// The tags an `info` answer may carry beside [`STALE_ONELINE`], never taken
/// either.
pub fn stale_tags() -> Value {
    json!(["superseded"])
}

/// One node of an `outline` answer, in the file space's form (`node_of`):
/// its kind is the article of its last segment.
pub fn node(anchor: &str, parent: &str) -> Value {
    let last = anchor.rsplit('/').next().unwrap_or_default();
    let kind = last.split(':').next().unwrap_or_default();
    let unit = if kind == "page" { "page" } else { "line" };
    json!({"anchor": anchor, "kind": kind, "parent": parent, "unit": unit,
           "from": 1, "to": 1, "oneline": "", "parser": "test"})
}

/// The fields of the file space's `info` answer, `tags` only when given. The
/// catalog reads `kind` from it and nothing else: `oneline` and `tags` may
/// stand in it, and are ignored -- on an announcement they are the head
/// before's (GH #950, OR-BC-68; the head's own come on `source_described`).
pub fn info_body(path: &str, kind: &str, oneline: &str, tags: Option<Value>) -> Value {
    let mime = if path.ends_with(".pdf") {
        "application/pdf"
    } else {
        "text/plain"
    };
    let mut b = json!({"path": path, "kind": kind, "mime": mime, "bytes": 64, "lines": 3,
                       "oneline": oneline});
    if let Some(t) = tags {
        b["tags"] = t;
    }
    b
}

/// The fields of the file space's `outline` answer: one page, no `next`.
pub fn outline_body(nodes: Value) -> Value {
    json!({"parser": "test", "nodes": nodes, "next": ""})
}

/// A source's answer to one pull, as the member's edge restamps it onto
/// `in_pulled`: `op` and `op_id` mirrored off the pull, `ok` true unless the
/// body says otherwise, `file` and `version` off the pull when it named them.
pub fn answer_pull(pull: &Message, body: Value) -> Message {
    let op = hop_str(pull, "op");
    let op_id = hop_str(pull, "op_id");
    let asked = body_of(pull);
    let mut b = body;
    if b.get("ok").is_none() {
        b["ok"] = json!(true);
    }
    b["op"] = json!(op);
    b["op_id"] = json!(op_id);
    if b.get("file").is_none()
        && let Some(s) = asked.get("source").and_then(Value::as_str)
    {
        b["file"] = json!(s);
    }
    if b.get("version").is_none()
        && let Some(v) = asked.get("version").and_then(Value::as_str)
    {
        b["version"] = json!(v);
    }
    b["messages"] = json!([]);
    message(
        HIVE,
        json!({"route": "in_pulled", "op": op, "op_id": op_id}),
        json!({}),
        b,
    )
}

/// A question on `in_lib`, with no `caller` (its answer leaves the hive).
pub fn ask_lib(op: &str, op_id: &str, args: Value) -> Message {
    message(
        HIVE,
        json!({"route": "in_lib", "op": op, "op_id": op_id}),
        json!({}),
        json!({"op": op, "args": args, "messages": []}),
    )
}

/// One tool call on `in_tool`, as the member's edge restamps the assistant's
/// `tool` lane: the arguments are the TEXT of the one turn. An empty `id`
/// leaves the id out of the hop and the turn both.
pub fn tool_call(name: &str, id: &str, text: &str, ctx: Value) -> Message {
    let mut hop = json!({"route": "in_tool", "tool_name": name});
    let mut turn = json!({"origin": "assistant", "type": "tool_call", "text": text});
    if !id.is_empty() {
        hop["tool_call_id"] = json!(id);
        turn["id"] = json!(id);
    }
    message(HIVE, hop, ctx, json!({"messages": [turn]}))
}

/// A menu question on `in_schemas`; `None` asks without a `tools` list.
pub fn schemas_question(tools: Option<Value>) -> Message {
    let mut body = json!({"messages": []});
    if let Some(t) = tools {
        body["tools"] = t;
    }
    message(HIVE, json!({"route": "in_schemas"}), json!({}), body)
}

// ═══════════════════════════════════════════════════════════════════ the sources

/// One version of one file as the test announces it, answers its pulls and
/// describes it.
#[derive(Clone, Debug)]
pub struct Doc {
    pub source: String,
    pub version: String,
    pub path: String,
    pub fmt: String,
    pub kind: String,
    /// The summary line its `source_described` carries.
    pub oneline: String,
    /// The tags its `source_described` carries; `None` describes it with `[]`.
    pub tags: Option<Value>,
    pub nodes: Vec<Value>,
    /// The node count the announcement carries, when not `nodes.len()`.
    pub count: Option<u64>,
}

impl Doc {
    /// A file with no item yet: its format off its name, `binary` for a pdf
    /// and `text` otherwise (the file space's `kind`).
    pub fn new(source: &str, version: &str, path: &str) -> Self {
        let fmt = fmt_of(path);
        let kind = if fmt == "pdf" { "binary" } else { "text" };
        Self {
            source: source.to_string(),
            version: version.to_string(),
            path: path.to_string(),
            fmt: fmt.to_string(),
            kind: kind.to_string(),
            oneline: String::new(),
            tags: None,
            nodes: Vec::new(),
            count: None,
        }
    }

    pub fn with_oneline(mut self, oneline: &str) -> Self {
        self.oneline = oneline.to_string();
        self
    }

    pub fn with_tags(mut self, tags: Value) -> Self {
        self.tags = Some(tags);
        self
    }

    /// Top-level `def:<name>` items, one per name.
    pub fn with_names(mut self, names: &[&str]) -> Self {
        self.nodes = names
            .iter()
            .map(|n| node(&format!("def:{n}"), ""))
            .collect();
        self
    }

    pub fn with_nodes(mut self, nodes: Vec<Value>) -> Self {
        self.nodes = nodes;
        self
    }

    pub fn with_kind(mut self, kind: &str) -> Self {
        self.kind = kind.to_string();
        self
    }

    pub fn with_fmt(mut self, fmt: &str) -> Self {
        self.fmt = fmt.to_string();
        self
    }

    pub fn with_count(mut self, count: u64) -> Self {
        self.count = Some(count);
        self
    }

    /// The same version under another path: a move.
    pub fn moved_to(mut self, path: &str) -> Self {
        self.path = path.to_string();
        self
    }

    /// The node count the announcement carries.
    pub fn announced_nodes(&self) -> u64 {
        self.count.unwrap_or(self.nodes.len() as u64)
    }

    pub fn announcement(&self) -> Message {
        announce(
            &self.source,
            &self.version,
            &self.path,
            &self.fmt,
            self.announced_nodes(),
        )
    }

    /// The `op_id` of one of the two index pulls of this version.
    pub fn op_id(&self, part: &str) -> String {
        format!("lib:f:i:{part}:{}:{}", self.source, v12(&self.version))
    }

    /// The `info` answer to this version's pull: its kind, and the summary
    /// line and tags of the head before (see [`STALE_ONELINE`]), as a space
    /// answers before its model summarised this head. A catalog that took
    /// them would hold [`STALE_ONELINE`] instead of this doc's own line.
    pub fn info(&self) -> Value {
        info_body(&self.path, &self.kind, STALE_ONELINE, Some(stale_tags()))
    }

    /// This version's `source_described`: its summary line and tags, under
    /// its own path.
    pub fn description(&self) -> Message {
        described(
            &self.source,
            &self.version,
            &self.path,
            &self.oneline,
            self.tags.clone().unwrap_or_else(|| json!([])),
        )
    }

    /// The key under which [`described_written`] counts this version.
    pub fn described_key(&self) -> String {
        described_key(&self.source, &self.version)
    }

    pub fn outline(&self) -> Value {
        outline_body(Value::Array(self.nodes.clone()))
    }
}

/// The two pulls of one announcement of `doc`, `(info, outline)`, held ones
/// first, each checked against its form (GH #950 § 1 and § 3): route `pull`,
/// no `caller`, `op_id` `lib:f:i:<part>:<source>:<v12>`, body `{op, file:
/// '<source>@<v12>', source, version, args: {version}}`, and the outline's
/// `limit` the announced node count, at least 1 and at most 2000.
pub async fn take_pulls(
    ports: &mut Ports,
    root: &std::path::Path,
    doc: &Doc,
    held: &mut Vec<Message>,
) -> (Message, Message) {
    let v = v12(&doc.version);
    let mut got = Vec::new();
    for part in ["info", "outline"] {
        let id = doc.op_id(part);
        let at = held.iter().position(|m| hop_str(m, "op_id") == id);
        let m = match at {
            Some(i) => held.remove(i),
            None => {
                next_matching(
                    &mut ports.pulls,
                    root,
                    &format!("the `{part}` pull of {}@{v}", doc.source),
                    |m| hop_str(m, "op_id") == id,
                    held,
                )
                .await
            }
        };
        assert_eq!(hop_str(&m, "route"), "pull", "{id}");
        assert_eq!(hop_str(&m, "op"), part, "{id}: the pull names its op");
        assert!(
            !m.headers.hop.contains_key("caller"),
            "{id}: a pull carries no `caller` (GH #950 § 1): {:?}",
            m.headers.hop
        );
        let b = body_of(&m);
        assert_eq!(b["op"], json!(part), "{id}: {b}");
        assert_eq!(b["file"], json!(format!("{}@{v}", doc.source)), "{id}: {b}");
        assert_eq!(b["source"], json!(doc.source), "{id}: {b}");
        assert_eq!(b["version"], json!(v), "{id}: {b}");
        assert_eq!(b["args"]["version"], json!(v), "{id}: {b}");
        if part == "outline" {
            assert_eq!(
                b["args"]["limit"],
                json!(doc.announced_nodes().clamp(1, 2000)),
                "{id}: the outline asks for the announced nodes, at least 1, at most 2000: {b}"
            );
        }
        got.push(m);
    }
    let outline = got.pop().expect("the outline pull");
    let info = got.pop().expect("the info pull");
    (info, outline)
}

/// Announce every doc, describe it, answer both pulls of each as the file
/// space would, and wait until the index wrote every description and every
/// answer.
///
/// The description is written BEFORE the `info` answer, which carries the
/// summary of the head before ([`Doc::info`]): an index that still took the
/// summary line or the tags from `info` would overwrite the description, and
/// every row a lock reads would say so. The two write disjoint columns, so a
/// correct index is indifferent to the order -- the space's model may finish
/// before or after the pulls are answered.
pub async fn index_files(
    h: &ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    docs: &[Doc],
) {
    let log = space::message_log(root);
    let before = written(&log);
    let mut described_want: BTreeMap<String, usize> = BTreeMap::new();
    let already = described_written(&log);
    for d in docs {
        let key = d.described_key();
        let n = described_want
            .entry(key.clone())
            .or_insert_with(|| already.get(&key).copied().unwrap_or(0));
        *n += 1;
    }
    for d in docs {
        h.send(d.announcement()).await;
    }
    let mut held = Vec::new();
    let mut pulls = Vec::new();
    // The pulls leave the index together with the announcement's own store
    // bundle: once they are here, the row's `announced` is on its way to the
    // store ahead of anything sent from now on.
    for d in docs {
        pulls.push(take_pulls(ports, root, d, &mut held).await);
    }
    for d in docs {
        h.send(d.description()).await;
    }
    let described_want: Vec<(String, usize)> = described_want.into_iter().collect();
    wait_described(root, "every announced file is described", &described_want).await;
    for (d, (info, outline)) in docs.iter().zip(&pulls) {
        h.send(answer_pull(info, d.info())).await;
        h.send(answer_pull(outline, d.outline())).await;
    }
    let want: Vec<(String, usize)> = docs
        .iter()
        .flat_map(|d| [d.op_id("info"), d.op_id("outline")])
        .map(|id| {
            let n = before.get(&id).copied().unwrap_or(0) + 1;
            (id, n)
        })
        .collect();
    wait_written(
        root,
        "every announced file is written into the catalog",
        &want,
    )
    .await;
}

// ════════════════════════════════════════════════════════════════════ the readers

/// The librarian's own store.
pub fn store_db(root: &std::path::Path) -> std::path::PathBuf {
    root.join("main/librarian/store/cell.db")
}

/// One row, by column name, every value as text.
pub type Row = BTreeMap<String, String>;

/// Every row of `sql` against the librarian's store. A store that has not
/// woken yet holds no table, which is the honest "nothing yet".
pub fn table(root: &std::path::Path, sql: &str) -> Vec<Row> {
    let db = store_db(root);
    if !db.is_file() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&db) else {
        return Vec::new();
    };
    // As the store opens every connection: `entries` carries a full-text
    // index whose tokenizer is the store's own.
    let _ = meclaw_cells::store::query::install_connection_extensions(&conn);
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
    };
    let names: Vec<String> = st.column_names().iter().map(|c| c.to_string()).collect();
    let out: Vec<Row> = st
        .query_map([], |r| {
            Ok(names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    let v = match r.get_ref(i) {
                        Ok(rusqlite::types::ValueRef::Text(t)) => {
                            String::from_utf8_lossy(t).to_string()
                        }
                        Ok(rusqlite::types::ValueRef::Integer(x)) => x.to_string(),
                        Ok(rusqlite::types::ValueRef::Real(x)) => x.to_string(),
                        _ => String::new(),
                    };
                    (n.clone(), v)
                })
                .collect::<Row>())
        })
        .map(|it| it.filter_map(Result::ok).collect())
        .unwrap_or_default();
    out
}

/// The catalog, one row per source, by source.
pub fn catalog_rows(root: &std::path::Path) -> Vec<Row> {
    table(root, "SELECT * FROM entries ORDER BY source")
}

/// The catalog row of one source.
pub fn catalog_row(root: &std::path::Path, source: &str) -> Option<Row> {
    catalog_rows(root)
        .into_iter()
        .find(|r| col(r, "source") == source)
}

/// One column of a row; '' when the column is missing, so a schema that lost
/// a column fails the comparison that reads it, with the row printed.
pub fn col<'a>(row: &'a Row, key: &str) -> &'a str {
    row.get(key).map(String::as_str).unwrap_or_default()
}

/// A column that holds a JSON list of strings (`tags`, `names`).
pub fn list(row: &Row, key: &str) -> Vec<String> {
    sj::from_str::<Vec<String>>(col(row, key))
        .unwrap_or_else(|e| panic!("`{key}` holds a JSON list of strings ({e}): {row:?}"))
}

/// The space-separated terms of `pterms` / `nterms`.
pub fn terms(row: &Row, key: &str) -> BTreeSet<String> {
    col(row, key)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Wait until `cond` holds; on the deadline, panic with the catalog and the
/// dead letters of the run.
pub async fn wait_for(root: &std::path::Path, what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + DEADLINE;
    while !cond() {
        if Instant::now() >= deadline {
            panic!(
                "{what}: not within {DEADLINE:?}. Catalog: {:#?}\nDead letters: {:#?}",
                catalog_rows(root),
                space::dead_letters(root)
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn kids<'a>(by: &BTreeMap<&'a str, Vec<&'a Logged>>, id: &str) -> Vec<&'a Logged> {
    by.get(id).cloned().unwrap_or_default()
}

/// Every delivery to `./index` on `lane` that the index answered with a store
/// bundle the store answered in turn, paired with that bundle.
fn handled<'a>(log: &'a [Logged], lane: &str) -> Vec<(&'a Logged, &'a Logged)> {
    let mut by: BTreeMap<&str, Vec<&Logged>> = BTreeMap::new();
    for r in log {
        if let Some(p) = r.parent.as_deref() {
            by.entry(p).or_default().push(r);
        }
    }
    let mut out = Vec::new();
    for got in log.iter().filter(|r| r.to == INDEX && r.route() == lane) {
        for b in kids(&by, &got.id)
            .into_iter()
            .filter(|b| b.from == INDEX && b.to == STORE)
        {
            if kids(&by, &b.id)
                .iter()
                .any(|s| s.from == STORE && s.to == INDEX)
            {
                out.push((got, b));
            }
        }
    }
    out
}

/// How often the index WROTE what it was answered, per `op_id`: a delivery to
/// `./index` on `in_pulled`, the bundle `./index` sent `./store` in reply to
/// that very delivery, and the store's answer to that bundle. A signal in the
/// colony's own log, not a quiet window -- a slow machine cannot read the
/// catalog before the answer was applied (the pattern of
/// `gh945_a_stale_pull_never_overwrites`). The compare-and-set of GH #950
/// § 3 sends its bundle whether or not it matches, so a stale answer counts
/// here too: handled, not necessarily applied.
pub fn written(log: &[Logged]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (got, _) in handled(log, "in_pulled") {
        if seen.insert(got.id.as_str()) {
            let id = got.hop["op_id"].as_str().unwrap_or_default().to_string();
            *out.entry(id).or_insert(0) += 1;
        }
    }
    out
}

/// The key of one described version: `<source>@<v12>`, as the index names it
/// in the `cur_call` of its `described` bundle.
pub fn described_key(source: &str, version: &str) -> String {
    format!("{source}@{}", v12(version))
}

/// How often the index WROTE a description, per [`described_key`]: a delivery
/// to `./index` on `source_described`, the `described` bundle `./index` sent
/// `./store` in reply, and the store's answer to that bundle -- the same
/// signal as [`written`]. The bundle goes out whether or not its
/// compare-and-set on `announced` matches, so a stale description counts
/// here too: handled, not necessarily applied. A description the index parks
/// (no source address, no version) sends no bundle and never counts; the end
/// of its run is the colony going quiet.
pub fn described_written(log: &[Logged]) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for (_, bundle) in handled(log, "source_described") {
        if bundle.hop["phase"].as_str() != Some("described") {
            continue;
        }
        let call: Value = bundle.hop["cur_call"]
            .as_str()
            .and_then(|c| sj::from_str(c).ok())
            .unwrap_or(Value::Null);
        let key = described_key(
            call["source"].as_str().unwrap_or_default(),
            call["version"].as_str().unwrap_or_default(),
        );
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

/// Wait until every `(op_id, n)` was written at least `n` times.
pub async fn wait_written(root: &std::path::Path, what: &str, want: &[(String, usize)]) {
    wait_for(root, what, || {
        let now = written(&space::message_log(root));
        want.iter()
            .all(|(id, n)| now.get(id).copied().unwrap_or(0) >= *n)
    })
    .await;
}

/// Wait until every `(key, n)` of [`described_written`] was written at least
/// `n` times.
pub async fn wait_described(root: &std::path::Path, what: &str, want: &[(String, usize)]) {
    wait_for(root, what, || {
        let now = described_written(&space::message_log(root));
        want.iter()
            .all(|(key, n)| now.get(key).copied().unwrap_or(0) >= *n)
    })
    .await;
}

/// Every delivery to the pull capture, in log order.
pub fn pulls_in(log: &[Logged]) -> Vec<&Logged> {
    log.iter().filter(|r| r.to == "/pulls").collect()
}

/// Ask the librarian one question on `in_lib` and return the body of its
/// one answer.
pub async fn ask(
    h: &ColonyHandle,
    ports: &mut Ports,
    root: &std::path::Path,
    op: &str,
    op_id: &str,
    args: Value,
) -> Value {
    h.send(ask_lib(op, op_id, args)).await;
    answer_of(ports, root, op, op_id).await
}

/// The one answer to the question `op_id` on `/lsink`.
pub async fn answer_of(ports: &mut Ports, root: &std::path::Path, op: &str, op_id: &str) -> Value {
    let mut seen = Vec::new();
    let m = next_matching(
        &mut ports.lsink,
        root,
        &format!("the answer to `{op}` ({op_id})"),
        |m| hop_str(m, "route") == "answer" && hop_str(m, "op_id") == op_id,
        &mut seen,
    )
    .await;
    assert_eq!(hop_str(&m, "op"), op, "the answer mirrors `op`");
    body_of(&m).clone()
}

/// The one `tool_result` of call `id` on `/tsink`: its hop and the JSON its
/// one turn carries (GH #950 § 5: `{origin: 'tool', type: 'tool_result', id,
/// text}`, the text without `op_id`, `caller` and `messages`).
pub async fn tool_result(
    ports: &mut Ports,
    root: &std::path::Path,
    id: &str,
) -> (Map<String, Value>, Value) {
    let mut seen = Vec::new();
    let m = next_matching(
        &mut ports.tsink,
        root,
        &format!("the tool result of call `{id}`"),
        |m| hop_str(m, "tool_call_id") == id,
        &mut seen,
    )
    .await;
    assert_eq!(hop_str(&m, "route"), "tool_result");
    let turns = body_of(&m)["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(turns.len(), 1, "a tool result is one turn: {turns:?}");
    assert_eq!(turns[0]["origin"], json!("tool"), "{turns:?}");
    assert_eq!(turns[0]["type"], json!("tool_result"), "{turns:?}");
    if !id.is_empty() {
        assert_eq!(turns[0]["id"], json!(id), "{turns:?}");
    }
    let answer = space::tool_answer(&m);
    for k in ["op_id", "caller", "messages", "header"] {
        assert!(
            answer.get(k).is_none(),
            "`{k}` is no part of what a model reads: {answer}"
        );
    }
    (m.headers.hop.clone(), answer)
}

/// Every delivery out of a code cell of the librarian carries what its
/// contract declares -- `hop.route` for `index`, `query` and `tools`,
/// `hop.operation` for `schemas` -- in the run, not on paper.
pub fn emissions_within_contract(log: &[Logged]) {
    for cell in ["index", "query", "tools", "schemas"] {
        let cfg = cell_config(cell);
        let hop = &cfg["contract"]["emits"]["hop"];
        let key = if hop["route"]["values"].is_array() {
            "route"
        } else {
            "operation"
        };
        let declared: Vec<String> = hop[key]["values"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        assert!(
            !declared.is_empty(),
            "templates/librarian/{cell} declares the values of the `hop.{key}` it emits"
        );
        let from = format!("{HIVE}/{cell}");
        for r in log.iter().filter(|r| r.from == from) {
            let got = r.hop[key].as_str().unwrap_or_default();
            assert!(
                declared.iter().any(|d| d.as_str() == got),
                "{from} emitted `{key}` `{got}`, which its contract does not declare \
                 ({declared:?}): {}",
                r.say()
            );
        }
    }
}
