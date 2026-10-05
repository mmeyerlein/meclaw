//! W8 (GH #380): server-side rendering, and the materialised tree.
//!
//! # The template language, and why it is closed
//!
//! Four forms, and no fifth:
//!
//! | Form | Meaning |
//! |---|---|
//! | `{{prop}}` | the prop's value, HTML-escaped |
//! | `{{&prop}}` | the prop's value **raw** — only for a prop the component's `prop_schema` types as `"html"` |
//! | `{{children}}` | the object's children, in `ord` order |
//! | `{{#if prop}}…{{/if}}` | the enclosed text, if the prop is present, non-empty and not `false` |
//!
//! Components are *data*: a model can define one at runtime by message. A
//! template language that grew by accident is therefore one a model would
//! discover by accident, and every accidental form becomes a compatibility
//! obligation the moment something renders with it. So the parser rejects
//! anything else between braces — and it rejects it at **definition** time
//! (Task 8's `component.define`), not at render time. A refusal at render time
//! would reach a person as a blank area on a page instead of as an answer to
//! whoever wrote the component.
//!
//! # Escaping
//!
//! Escaped by default, raw only where declared. Props are written by models and
//! by browsers (`editable` writes, Task 9), so they are untrusted for this
//! purpose. `{{&prop}}` on a prop whose schema does not say `"html"` silently
//! escapes rather than refuses: rendering markup that no schema promised was
//! markup is the worse of the two failures.
//!
//! # Where a value stands (GH #869)
//!
//! Escaping keeps a value inside the element or attribute it was put in; it
//! says nothing about what the value means there. So the parser records, for
//! every substitution, the place it stands in ([`Slot`]: text, a named
//! attribute value, the inside of a tag, the body of a `<script>`/`<style>`),
//! and the walk asks each value to fit its place — an event name in a
//! LiveView binding, a relative or `http(s)`/`mailto`/`tel` URL in a URL
//! attribute, a plain CSS value in `style`, a token inside a tag. A prop typed
//! `"int"` renders an integer and nothing else. What does not fit renders
//! **empty**, for the reason above: a missing word is the harmless failure.
//! A raw value is raw only between tags or in a script/style body; in an
//! attribute it is escaped and fitted like any other. The grammars live in
//! [`crate::web::markup`].
//!
//! # What "materialised" means
//!
//! [`materialize`] renders a whole page once and keeps the result. A GET then
//! answers from that, touching no database and making no cell call (R-W8-4a).
//! The result is already in LiveView's packed shape — statics plus one slot per
//! direct child of the page root — so a GET does no diff work either
//! (R-W8-4b). Diffs exist only as a consequence of writes.
//!
//! # Parts, and what a write costs (GH #1001)
//!
//! Each slot holds a [`Part`]: LiveView's "rendered" shape for one object —
//! the statics of its template once, the substituted values as dynamics, a
//! `{{children}}` marker as one nested part holding one part per child, and an
//! `{{#if}}` as either nothing or a nested part. Measured on a deployed page
//! (`MESSUNG.md` § 6 L-W2, G1): every write re-rendered every route (≈ 51 ms
//! on 1 000 figures, 129–150 ms on 4 000) and pushed the whole root-child
//! slot as one HTML string (≈ 20 KB for one child of a 128-child chunk). So:
//!
//! - a write re-renders only the root-child slots it touched ([`rerender`]),
//!   loading each slot's subtree in one recursive query, and reuses every
//!   other slot of the published page as it is;
//! - what goes out is the difference between the part the viewers hold and
//!   the new one ([`diff_value`]): a moved figure is its changed values, a
//!   child of a chunk is a path into the chunk;
//! - statics travel once per frame in the shared table `"p"` and parts name
//!   them by number;
//! - a part whose template is exactly one element carries `"r": 1`, which lets
//!   the client skip it when a frame leaves it alone ([`one_root_element`]).
//!   A template that is not one element renders its object as an HTML string,
//!   the form every slot had before.

use crate::web::markup::{Contexts, Slot, Sub, fits, int_text};
use crate::web::ops::Touched;
use meclaw_core::serde_json::{Map, Value, json};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Every route this cell serves, rendered.
///
/// A `BTreeMap` rather than a `HashMap` so a listing of routes is stable — an
/// operator comparing two dumps should not have to sort them first.
pub type PageMap = BTreeMap<String, Materialized>;

/// Render every declared route.
///
/// Called once when the cell starts (`on_start`), and again for one route at a
/// time as writes land. A route whose tree is broken is **skipped with its
/// reason logged** rather than failing the whole map: one bad page must not
/// take down every other page in the same cell.
pub fn materialize_all(conn: &Connection) -> Result<PageMap, RenderError> {
    Renderer::new(conn).all()
}

impl Renderer<'_> {
    /// Every route, rendered by this one renderer (one component cache).
    fn all(&mut self) -> Result<PageMap, RenderError> {
        let routes = {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT route FROM pages ORDER BY route")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut out = PageMap::new();
        for route in routes {
            match self.page(&route) {
                Ok(m) => {
                    out.insert(route, m);
                }
                Err(e) => {
                    tracing::error!(route = %route, error = %e, "web: route did not render");
                }
            }
        }
        Ok(out)
    }
}

/// How deep the object tree may nest before rendering gives up.
///
/// Nothing stops a patch from making an object its own ancestor. A renderer
/// that recursed into such a tree would take the cell down with a stack
/// overflow — which is a crash, not a diagnosis — so the depth is bounded and
/// the breach is reported with the object named.
const MAX_DEPTH: usize = 64;

/// Why a render did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// No `pages` row for this route.
    UnknownRoute(String),
    /// No `objects` row with this id.
    UnknownObject(String),
    /// An object names a component that is not in the library.
    UnknownComponent(String),
    /// The tree nests deeper than [`MAX_DEPTH`], which in practice means a
    /// cycle. The id named is where the bound was hit.
    TooDeep { at: String },
    /// The database refused a read.
    Db(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownRoute(r) => write!(f, "no page declares the route {r:?}"),
            Self::UnknownObject(id) => write!(f, "no object {id:?}"),
            Self::UnknownComponent(c) => write!(f, "no component {c:?}"),
            Self::TooDeep { at } => write!(
                f,
                "object tree nests deeper than {MAX_DEPTH} at {at:?} — this is a cycle"
            ),
            Self::Db(e) => write!(f, "database: {e}"),
        }
    }
}

impl From<rusqlite::Error> for RenderError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e.to_string())
    }
}

/// A page, rendered and kept.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Materialized {
    /// The root component's template, split at `{{children}}`. For a root with
    /// N children this holds N+1 pieces — the text before the marker, N−1 empty
    /// separators, and the text after it — because the static/dynamic format
    /// the client reads wants one more static than there are dynamics. A root
    /// with no children at all (no `{{children}}`, or a marker with nothing to
    /// show) holds exactly one piece, and the page is entirely static.
    pub statics: Vec<String>,
    /// One `(object_id, part)` per **direct child** of the page root, in order.
    pub slots: Vec<(String, Part)>,
    /// The page title, for the shell's `<title>`.
    pub title: String,
}

impl Materialized {
    /// The LiveView packed tree: `{"s": statics, "p": shared, "0": slot0, …}`.
    ///
    /// Every component's statics stand once in `"p"` and every part names them
    /// by number; a page whose slots are all strings has no `"p"`.
    pub fn packed_tree(&self) -> Value {
        let mut shared = SharedStatics::default();
        let mut m = Map::new();
        m.insert(
            "s".to_string(),
            Value::Array(self.statics.iter().map(|s| json!(s)).collect()),
        );
        for (i, (_id, part)) in self.slots.iter().enumerate() {
            m.insert(i.to_string(), full_value(part, &mut shared));
        }
        shared.attach(&mut m);
        Value::Object(m)
    }

    /// The index of the slot holding `object_id`, if it is a root child.
    pub fn slot_of(&self, object_id: &str) -> Option<usize> {
        self.slots.iter().position(|(id, _)| id == object_id)
    }

    /// The page as one HTML string: statics and slots interleaved.
    ///
    /// This is what the shell embeds on a GET. It is the same content the join
    /// reply carries in packed form, which is what lets the LiveView client
    /// attach to the served markup instead of replacing it on connect.
    pub fn rendered_body(&self) -> String {
        let mut out = String::new();
        for (i, s) in self.statics.iter().enumerate() {
            out.push_str(s);
            if let Some((_, part)) = self.slots.get(i) {
                part.write_html(&mut out);
            }
        }
        out
    }
}

/// One rendered object, or one piece of one: LiveView's "rendered" shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// A string on the wire: a substituted value, or the whole markup of an
    /// object whose template is not exactly one element.
    Text(String),
    /// Statics and dynamics.
    Node(Arc<Node>),
}

/// The statics of a template and the values that go between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// One more than there are dynamics.
    pub statics: Arc<[String]>,
    /// `"r": 1` — the statics are exactly one element, so the client may skip
    /// this part when a frame does not touch it.
    pub root: bool,
    /// The values, each a part of its own.
    pub dynamics: Vec<Part>,
}

impl From<String> for Part {
    fn from(s: String) -> Self {
        Part::Text(s)
    }
}

impl From<&str> for Part {
    fn from(s: &str) -> Self {
        Part::Text(s.to_string())
    }
}

impl std::fmt::Display for Part {
    /// The part's markup.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.html())
    }
}

impl Part {
    /// The part's markup.
    pub fn html(&self) -> String {
        let mut out = String::new();
        self.write_html(&mut out);
        out
    }

    fn write_html(&self, out: &mut String) {
        match self {
            Part::Text(t) => out.push_str(t),
            Part::Node(n) => {
                for (i, s) in n.statics.iter().enumerate() {
                    out.push_str(s);
                    if let Some(d) = n.dynamics.get(i) {
                        d.write_html(out);
                    }
                }
            }
        }
    }
}

/// The statics one frame shares by number (`"p"`).
///
/// The client resolves a numeric `"s"` against the table of the frame that
/// carries it and then deletes the table, so every frame numbers its own.
#[derive(Default)]
struct SharedStatics {
    index: HashMap<Arc<[String]>, usize>,
    table: Vec<Arc<[String]>>,
}

impl SharedStatics {
    fn id(&mut self, statics: &Arc<[String]>) -> usize {
        if let Some(&i) = self.index.get(statics) {
            return i;
        }
        let i = self.table.len();
        self.table.push(Arc::clone(statics));
        self.index.insert(Arc::clone(statics), i);
        i
    }

    /// Put the table on the frame, if any part used it.
    fn attach(self, frame: &mut Map<String, Value>) {
        if self.table.is_empty() {
            return;
        }
        let mut p = Map::new();
        for (i, s) in self.table.iter().enumerate() {
            p.insert(
                i.to_string(),
                Value::Array(s.iter().map(|x| json!(x)).collect()),
            );
        }
        frame.insert("p".to_string(), Value::Object(p));
    }
}

/// A part in full, for a viewer that holds nothing of it.
fn full_value(part: &Part, shared: &mut SharedStatics) -> Value {
    match part {
        Part::Text(t) => Value::String(t.clone()),
        Part::Node(n) => {
            let mut m = Map::new();
            m.insert("s".to_string(), json!(shared.id(&n.statics)));
            for (i, d) in n.dynamics.iter().enumerate() {
                m.insert(i.to_string(), full_value(d, shared));
            }
            if n.root {
                m.insert("r".to_string(), json!(1));
            }
            Value::Object(m)
        }
    }
}

/// What a viewer holding `old` needs to hold `new`, or `None` if nothing.
///
/// The same statics: only the dynamics that changed, by index, each again as
/// a difference — the client merges an object without `"s"` into the part it
/// has. Anything else: the new part in full, which the client puts in place.
fn diff_value(old: &Part, new: &Part, shared: &mut SharedStatics) -> Option<Value> {
    match (old, new) {
        (Part::Text(a), Part::Text(b)) => (a != b).then(|| Value::String(b.clone())),
        (Part::Node(a), Part::Node(b)) if Arc::ptr_eq(a, b) => None,
        (Part::Node(a), Part::Node(b)) if a.root == b.root && a.statics == b.statics => {
            let mut m = Map::new();
            for (i, (x, y)) in a.dynamics.iter().zip(&b.dynamics).enumerate() {
                if let Some(v) = diff_value(x, y, shared) {
                    m.insert(i.to_string(), v);
                }
            }
            (!m.is_empty()).then_some(Value::Object(m))
        }
        _ => (old != new).then(|| full_value(new, shared)),
    }
}

/// The markup of a wire part, resolving numeric statics against `shared` (the
/// `"p"` of the frame that carries it). For tests and diagnostics: the client
/// does the same when it renders.
pub fn wire_html(part: &Value, shared: &Value) -> String {
    match part {
        Value::String(s) => s.clone(),
        Value::Object(m) => {
            let statics: Vec<String> = match m.get("s") {
                Some(Value::Number(n)) => shared[n.to_string()]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|s| s.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|s| s.as_str().unwrap_or_default().to_string())
                    .collect(),
                _ => Vec::new(),
            };
            let mut out = String::new();
            for (i, s) in statics.iter().enumerate() {
                out.push_str(s);
                if let Some(d) = m.get(&i.to_string()) {
                    out.push_str(&wire_html(d, shared));
                }
            }
            out
        }
        _ => String::new(),
    }
}

/// One row of `objects`.
struct ObjectRow {
    component: String,
    props: Value,
}

/// A template cut into statics and dynamics, once per component per render.
#[derive(Debug, Default)]
struct Tpl {
    statics: Arc<[String]>,
    root: bool,
    dyns: Vec<Dyn>,
}

/// One dynamic of a [`Tpl`].
#[derive(Debug)]
enum Dyn {
    Prop { name: String, slot: Slot },
    Raw { name: String, slot: Slot },
    Children,
    If { prop: String, body: Tpl },
}

impl Tpl {
    fn compile(pieces: &[Piece], top: bool) -> Tpl {
        let mut statics = Vec::new();
        let mut current = String::new();
        let mut dyns = Vec::new();
        for piece in pieces {
            let d = match piece {
                Piece::Text(t) => {
                    current.push_str(t);
                    continue;
                }
                Piece::Prop { name, slot } => Dyn::Prop {
                    name: name.clone(),
                    slot: slot.clone(),
                },
                Piece::Raw { name, slot } => Dyn::Raw {
                    name: name.clone(),
                    slot: slot.clone(),
                },
                Piece::Children => Dyn::Children,
                Piece::If { prop, body } => Dyn::If {
                    prop: prop.clone(),
                    body: Tpl::compile(body, false),
                },
            };
            statics.push(std::mem::take(&mut current));
            dyns.push(d);
        }
        statics.push(current);
        Tpl {
            statics: statics.into(),
            root: top && one_root_element(pieces),
            dyns,
        }
    }
}

/// A component, ready to render.
struct Compiled {
    pieces: Vec<Piece>,
    tpl: Tpl,
    schema: Value,
}

/// The objects below one id, loaded in one query.
#[derive(Default)]
struct Subtree {
    rows: HashMap<String, ObjectRow>,
    kids: HashMap<String, Vec<String>>,
}

/// One render pass: a component cache and the count of objects it rendered.
struct Renderer<'c> {
    conn: &'c Connection,
    components: HashMap<String, Arc<Compiled>>,
    /// Objects rendered so far — the statistic [`rerender`] reports.
    objects: usize,
}

impl<'c> Renderer<'c> {
    fn new(conn: &'c Connection) -> Self {
        Self {
            conn,
            components: HashMap::new(),
            objects: 0,
        }
    }

    /// `(template, prop_schema)` of one component, parsed and cut, once.
    fn component(&mut self, name: &str) -> Result<Arc<Compiled>, RenderError> {
        if let Some(c) = self.components.get(name) {
            return Ok(Arc::clone(c));
        }
        let row = self
            .conn
            .prepare_cached("SELECT template, prop_schema FROM components WHERE name = ?1")?
            .query_row([name], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    RenderError::UnknownComponent(name.to_string())
                }
                other => RenderError::Db(other.to_string()),
            })?;
        // A template stored in the database was accepted by
        // `component.define`, so a parse failure here means the row was
        // written around that gate. Render it as nothing rather than failing
        // the page — and the definition path is where the message belongs.
        let pieces = parse_template(&row.0).unwrap_or_default();
        let compiled = Arc::new(Compiled {
            tpl: Tpl::compile(&pieces, true),
            pieces,
            schema: meclaw_core::serde_json::from_str(&row.1).unwrap_or_else(|_| json!({})),
        });
        self.components
            .insert(name.to_string(), Arc::clone(&compiled));
        Ok(compiled)
    }

    /// `id` and everything below it, in one recursive query over
    /// `idx_objects_parent`. `UNION` (not `UNION ALL`) ends a cycle; the
    /// depth bound of the walk then names it.
    fn subtree(&self, id: &str) -> Result<Subtree, RenderError> {
        let mut stmt = self.conn.prepare_cached(
            "WITH RECURSIVE sub(id) AS ( \
                 SELECT ?1 UNION SELECT o.id FROM objects o JOIN sub ON o.parent = sub.id \
             ) \
             SELECT o.id, o.parent, o.component, o.ord, o.props \
             FROM objects o JOIN sub ON o.id = sub.id",
        )?;
        let mut tree = Subtree::default();
        let mut kids: HashMap<String, Vec<(i64, String)>> = HashMap::new();
        let rows = stmt.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (oid, parent, component, ord, props) = row?;
            // The top object's own parent edge too: it lies outside the
            // subtree unless the tree is a cycle, and then it is the edge that
            // lets the depth bound find it.
            if let Some(p) = parent {
                kids.entry(p).or_default().push((ord, oid.clone()));
            }
            tree.rows.insert(
                oid,
                ObjectRow {
                    component,
                    // A props column that is not an object renders as no props
                    // at all rather than failing the page: a malformed prop bag
                    // costs its own values, not everybody else's.
                    props: meclaw_core::serde_json::from_str(&props).unwrap_or_else(|_| json!({})),
                },
            );
        }
        for (parent, mut list) in kids {
            // `ORDER BY ord, id`, as the page has always drawn them.
            list.sort();
            tree.kids
                .insert(parent, list.into_iter().map(|(_, id)| id).collect());
        }
        Ok(tree)
    }

    /// One object of `tree` and everything below it, as a part.
    fn part(&mut self, tree: &Subtree, id: &str, depth: usize) -> Result<Part, RenderError> {
        if depth > MAX_DEPTH {
            return Err(RenderError::TooDeep { at: id.to_string() });
        }
        let row = tree
            .rows
            .get(id)
            .ok_or_else(|| RenderError::UnknownObject(id.to_string()))?;
        self.objects += 1;
        let comp = self.component(&row.component)?;
        let node = self.node(tree, &comp.tpl, &row.props, &comp.schema, id, depth)?;
        let part = Part::Node(Arc::new(node));
        if comp.tpl.root {
            Ok(part)
        } else {
            // Not one element: the client could not skip it, and a part
            // without a single root reads as malformed to its skip path. The
            // markup as a string is the form every slot had before.
            Ok(Part::Text(part.html()))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        tree: &Subtree,
        tpl: &Tpl,
        props: &Value,
        schema: &Value,
        id: &str,
        depth: usize,
    ) -> Result<Node, RenderError> {
        let mut dynamics = Vec::with_capacity(tpl.dyns.len());
        for d in &tpl.dyns {
            dynamics.push(match d {
                Dyn::Prop { name, slot } => {
                    Part::Text(escape(&fitted_text(props, schema, name, slot)))
                }
                Dyn::Raw { name, slot } => Part::Text(raw_text(props, schema, name, slot)),
                Dyn::Children => {
                    let ids = tree.kids.get(id).map(Vec::as_slice).unwrap_or_default();
                    let mut parts = Vec::with_capacity(ids.len());
                    for child in ids {
                        parts.push(self.part(tree, child, depth + 1)?);
                    }
                    // Statics n+1 empty strings: the children stand side by
                    // side, and there is no single root to skip.
                    Part::Node(Arc::new(Node {
                        statics: vec![String::new(); parts.len() + 1].into(),
                        root: false,
                        dynamics: parts,
                    }))
                }
                Dyn::If { prop, body } => {
                    if prop_truthy(props, prop) {
                        Part::Node(Arc::new(self.node(tree, body, props, schema, id, depth)?))
                    } else {
                        Part::Text(String::new())
                    }
                }
            });
        }
        Ok(Node {
            statics: Arc::clone(&tpl.statics),
            root: tpl.root,
            dynamics,
        })
    }

    /// The root row of a page and its title.
    fn page_row(&self, route: &str) -> Result<(String, String), RenderError> {
        self.conn
            .prepare_cached("SELECT root, title FROM pages WHERE route = ?1")?
            .query_row([route], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    RenderError::UnknownRoute(route.to_string())
                }
                other => RenderError::Db(other.to_string()),
            })
    }

    /// Render a whole route into its packed form.
    fn page(&mut self, route: &str) -> Result<Materialized, RenderError> {
        let (root, title) = self.page_row(route)?;
        let tree = self.subtree(&root)?;
        let obj = tree
            .rows
            .get(&root)
            .ok_or_else(|| RenderError::UnknownObject(root.clone()))?;
        self.objects += 1;
        let comp = self.component(&obj.component)?;

        // Split the root's own template at `{{children}}`. Everything outside
        // the children marker is static for this page; each direct child
        // becomes one slot. `{{#if}}` around the children marker is not split
        // into — the conditional is evaluated and its result folded into the
        // surrounding static, because a slot that appears and disappears is
        // not a slot.
        let mut statics: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut slots: Vec<(String, Part)> = Vec::new();
        let mut split = false;

        for piece in &comp.pieces {
            match piece {
                Piece::Children if !split => {
                    split = true;
                    statics.push(std::mem::take(&mut current));
                    let ids = tree.kids.get(&root).cloned().unwrap_or_default();
                    for child in ids {
                        let part = self.part(&tree, &child, 1)?;
                        slots.push((child, part));
                    }
                    // One separator between each pair of adjacent slots, so the
                    // list ends up n+1 long once the trailing piece is pushed
                    // (GH #394). Two children used to produce two statics,
                    // which put the closing tag *between* them and dropped
                    // every child from the third on — `rendered_body` walks
                    // `statics`, and the wire format wants n+1 statics for n
                    // dynamics.
                    let separators = slots.len().saturating_sub(1);
                    statics.resize(statics.len() + separators, String::new());
                }
                other => {
                    let ids = tree.kids.get(&root).cloned().unwrap_or_default();
                    render_pieces_with(
                        std::slice::from_ref(other),
                        &obj.props,
                        &comp.schema,
                        &mut current,
                        &mut |out| {
                            for child in &ids {
                                self.part(&tree, child, 1)?.write_html(out);
                            }
                            Ok(())
                        },
                    )?;
                }
            }
        }
        statics.push(current);

        // A root with nothing in its children marker is entirely static: one
        // piece, no slots. That covers both a root with no `{{children}}` at
        // all and one whose marker has no children to show — n+1 statics for
        // n = 0 is one, and a tree with two statics and no dynamic is not a
        // shape the client reads. The served body is identical either way.
        if slots.is_empty() {
            statics = vec![statics.concat()];
        }

        Ok(Materialized {
            statics,
            slots,
            title,
        })
    }

    /// Whether a write below this page's root can be answered slot by slot:
    /// the root template shows its children exactly once, at its top level.
    /// A second `{{children}}`, or one inside `{{#if}}`, is folded into the
    /// statics, and only a whole render keeps those current.
    fn slotted(&mut self, root: &str) -> Result<bool, RenderError> {
        let component: String = match self
            .conn
            .prepare_cached("SELECT component FROM objects WHERE id = ?1")?
            .query_row([root], |r| r.get(0))
        {
            Ok(c) => c,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let comp = self.component(&component)?;
        let top = comp
            .pieces
            .iter()
            .filter(|p| matches!(p, Piece::Children))
            .count();
        Ok(top == 1 && !comp.pieces.iter().any(nests_children))
    }
}

/// Whether a piece is an `{{#if}}` with `{{children}}` somewhere inside.
fn nests_children(piece: &Piece) -> bool {
    match piece {
        Piece::If { body, .. } => body
            .iter()
            .any(|p| matches!(p, Piece::Children) || nests_children(p)),
        _ => false,
    }
}

/// The ids of an object's children, in `ord` order.
fn child_ids(conn: &Connection, parent: &str) -> Result<Vec<String>, RenderError> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM objects WHERE parent = ?1 ORDER BY ord, id")?;
    let ids = stmt
        .query_map([parent], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// What a re-render after a write produced: the pages to publish, the frames
/// to push (one per route), and how many objects it rendered.
#[derive(Debug, Default)]
pub struct Rerendered {
    /// The pages to publish.
    pub pages: PageMap,
    /// `(route, diff)`, one per route the write reached.
    pub frames: Vec<(String, Value)>,
    /// Objects rendered — the root-child slots the write touched, with
    /// everything below them; untouched slots are taken from `old` as they
    /// are (GH #1001 T1).
    pub objects: usize,
}

/// Re-render after a write: only the slots `touched` names, on their routes.
///
/// `old` is what the viewers hold — the pages last published, every frame
/// computed against it. Three answers:
///
/// - the root-object case (structural, no slot): every route whole, every
///   route its packed tree, as before;
/// - a route `old` does not have, or a write that names the page root
///   (`page.set`, `component.define`), or a root template that folds its
///   children into its statics: that route whole; the frame is the packed
///   tree for a structural write, a route `old` does not have or a slot the
///   page does not have, and the named slots in full otherwise;
/// - everything else: the named slots re-rendered (a structural write re-reads
///   the root's child list; a slot new to it is rendered too), every other
///   slot taken over. If the root's child list is the one the viewers hold,
///   the frame is each named slot's difference; otherwise the packed tree.
///
/// A route that does not render is left out of the pages with its reason
/// logged, as [`materialize_all`] does.
pub fn rerender(
    conn: &Connection,
    old: &PageMap,
    touched: &Touched,
) -> Result<Rerendered, RenderError> {
    let mut r = Renderer::new(conn);
    if touched.structural && touched.slots.is_empty() {
        let pages = r.all()?;
        let frames = pages
            .iter()
            .map(|(route, page)| (route.clone(), page.packed_tree()))
            .collect();
        return Ok(Rerendered {
            pages,
            frames,
            objects: r.objects,
        });
    }

    // Group by route, in first-seen order: one frame per route (GH #723).
    let mut by_route: Vec<(&str, HashSet<&str>)> = Vec::new();
    for (route, id) in &touched.slots {
        match by_route.iter_mut().find(|(r, _)| r == route) {
            Some((_, ids)) => {
                ids.insert(id);
            }
            None => by_route.push((route, HashSet::from([id.as_str()]))),
        }
    }

    let mut pages = old.clone();
    let mut frames = Vec::new();
    for (route, ids) in by_route {
        // An update: the slots are patched in place in the copy that will be
        // published, so the write costs its slots and not the page (T2 — a
        // rebuilt slot list grew 2 → 6 ms from 1 000 to 4 000 figures).
        if !touched.structural
            && let Some(page) = pages.get_mut(route)
        {
            match in_place(&mut r, route, page, &ids) {
                Ok(Some(frame)) => {
                    frames.push((route.to_string(), frame));
                    continue;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(route = %route, error = %e, "web: route did not render");
                    pages.remove(route);
                    continue;
                }
            }
        }
        let rendered = match old.get(route) {
            Some(prev) if touched.structural => incremental(&mut r, route, prev, &ids),
            _ => Ok(None),
        };
        let result = match rendered {
            Ok(Some(done)) => Ok(done),
            // A route the viewers hold no page for gets its packed tree: the
            // named slots alone would land beside neighbours from before the
            // render error that dropped the route (review M1, GH #1001).
            Ok(None) => whole(
                &mut r,
                route,
                &ids,
                touched.structural || !old.contains_key(route),
            ),
            Err(e) => Err(e),
        };
        match result {
            Ok((page, frame)) => {
                pages.insert(route.to_string(), page);
                frames.push((route.to_string(), frame));
            }
            Err(e) => {
                tracing::error!(route = %route, error = %e, "web: route did not render");
                pages.remove(route);
            }
        }
    }
    Ok(Rerendered {
        pages,
        frames,
        objects: r.objects,
    })
}

/// A route rendered whole after a write, and its frame: the packed tree when
/// `packed` (a structural write, or a route the viewers hold no page for) or
/// when a named slot is not on the page, the named slots in full otherwise.
fn whole(
    r: &mut Renderer<'_>,
    route: &str,
    ids: &HashSet<&str>,
    packed: bool,
) -> Result<(Materialized, Value), RenderError> {
    let page = r.page(route)?;
    let addressable = ids.iter().all(|id| page.slot_of(id).is_some());
    let frame = if packed || !addressable {
        page.packed_tree()
    } else {
        let mut shared = SharedStatics::default();
        let mut m = Map::new();
        for (i, (id, part)) in page.slots.iter().enumerate() {
            if ids.contains(id.as_str()) {
                m.insert(i.to_string(), full_value(part, &mut shared));
            }
        }
        shared.attach(&mut m);
        Value::Object(m)
    };
    Ok((page, frame))
}

/// Whether a write below `root` may be answered slot by slot (see
/// [`Renderer::slotted`]); a write that names the root itself may not.
fn slot_by_slot(
    r: &mut Renderer<'_>,
    root: &str,
    prev: &Materialized,
    ids: &HashSet<&str>,
) -> Result<bool, RenderError> {
    Ok(!ids.contains(root) && !prev.slots.is_empty() && r.slotted(root)?)
}

/// An update answered in place: the named slots re-rendered inside `page`,
/// the frame their differences — or `None` when the route has to be whole.
///
/// An update cannot change the root's child list, so the list is the one the
/// viewers hold; a slot it names that the page does not have is a picture the
/// page never drew, and that is a whole render.
fn in_place(
    r: &mut Renderer<'_>,
    route: &str,
    page: &mut Materialized,
    ids: &HashSet<&str>,
) -> Result<Option<Value>, RenderError> {
    let (root, title) = r.page_row(route)?;
    if !slot_by_slot(r, &root, page, ids)? {
        return Ok(None);
    }
    let at: Vec<usize> = page
        .slots
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| ids.contains(id.as_str()))
        .map(|(i, _)| i)
        .collect();
    if at.len() != ids.len() {
        return Ok(None);
    }
    page.title = title;
    let mut shared = SharedStatics::default();
    let mut m = Map::new();
    for i in at {
        let id = page.slots[i].0.clone();
        let tree = r.subtree(&id)?;
        let new = r.part(&tree, &id, 1)?;
        let old = std::mem::replace(&mut page.slots[i].1, new);
        if let Some(v) = diff_value(&old, &page.slots[i].1, &mut shared) {
            m.insert(i.to_string(), v);
        }
    }
    shared.attach(&mut m);
    Ok(Some(Value::Object(m)))
}

/// A structural write re-rendered slot by slot, or `None` when the route has
/// to be whole: the root's child list is read again, the named slots and any
/// slot new to the list are rendered, every other slot is taken over.
fn incremental(
    r: &mut Renderer<'_>,
    route: &str,
    prev: &Materialized,
    ids: &HashSet<&str>,
) -> Result<Option<(Materialized, Value)>, RenderError> {
    let (root, title) = r.page_row(route)?;
    if !slot_by_slot(r, &root, prev, ids)? {
        return Ok(None);
    }
    let kids = child_ids(r.conn, &root)?;
    if kids.is_empty() {
        return Ok(None);
    }
    let held: HashMap<&str, &Part> = prev
        .slots
        .iter()
        .map(|(id, part)| (id.as_str(), part))
        .collect();
    let mut slots = Vec::with_capacity(kids.len());
    for id in kids {
        let part = match held.get(id.as_str()) {
            Some(part) if !ids.contains(id.as_str()) => (*part).clone(),
            _ => {
                let tree = r.subtree(&id)?;
                r.part(&tree, &id, 1)?
            }
        };
        slots.push((id, part));
    }
    let mut statics = Vec::with_capacity(slots.len() + 1);
    statics.push(prev.statics[0].clone());
    statics.resize(slots.len(), String::new());
    statics.push(prev.statics[prev.statics.len() - 1].clone());
    let page = Materialized {
        statics,
        slots,
        title,
    };

    let same_list = page.slots.len() == prev.slots.len()
        && page
            .slots
            .iter()
            .zip(&prev.slots)
            .all(|((a, _), (b, _))| a == b);
    let frame = if same_list {
        let mut shared = SharedStatics::default();
        let mut m = Map::new();
        for (i, ((id, new), (_, old))) in page.slots.iter().zip(&prev.slots).enumerate() {
            if ids.contains(id.as_str())
                && let Some(v) = diff_value(old, new, &mut shared)
            {
                m.insert(i.to_string(), v);
            }
        }
        shared.attach(&mut m);
        Value::Object(m)
    } else {
        page.packed_tree()
    };
    Ok(Some((page, frame)))
}

/// Whether a template's markup is exactly one element at its top level.
///
/// The client's skip path (`"r": 1`) cuts a part's markup at its first tag
/// and its last `>` and stands an empty element with the part's id in for it
/// (`phoenix_live_view.min.js`, `Lt`). That is only the same page if the part
/// IS one element: no text, comment, value, children or conditional beside it.
/// The template's own text is walked as markup (start and end tags, quoted
/// attribute values, void elements, comments, raw-text bodies); every
/// substitution must stand inside the element, and a conditional's body must
/// leave the markup where it found it.
pub(crate) fn one_root_element(pieces: &[Piece]) -> bool {
    let mut scan = Scan::default();
    scan.pieces(pieces) && scan.done()
}

/// Where the walk of [`one_root_element`] stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum ScanState {
    #[default]
    Data,
    /// After `<`.
    Open,
    /// In a start tag's name.
    Name,
    /// In an end tag's name.
    EndName,
    /// Inside a start tag, between attributes.
    InTag,
    /// In a quoted attribute value.
    Quoted(char),
    /// After `<!`.
    Bang,
    /// In a comment, `<!--` … `-->`.
    Comment,
    /// In `<!…>` that is not a comment.
    Decl,
    /// In the body of a raw-text element, waiting for its end tag.
    Raw,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Scan {
    state: ScanState,
    depth: usize,
    roots: usize,
    name: String,
    self_closing: bool,
    raw: String,
    tail: String,
    broken: bool,
}

/// Elements without an end tag.
const VOID: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Elements whose body is text up to their end tag.
const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title"];

impl Scan {
    fn done(&self) -> bool {
        !self.broken && self.state == ScanState::Data && self.depth == 0 && self.roots == 1
    }

    fn pieces(&mut self, pieces: &[Piece]) -> bool {
        for piece in pieces {
            match piece {
                Piece::Text(t) => {
                    for c in t.chars() {
                        self.char(c);
                    }
                }
                Piece::Prop { .. } | Piece::Raw { .. } | Piece::Children => {
                    // A value between tags at the top level is text beside
                    // the element; anywhere else it stays inside it.
                    if self.top_level() {
                        return false;
                    }
                }
                Piece::If { body, .. } => {
                    if self.top_level() {
                        return false;
                    }
                    // In a tag or a quoted value the body is attribute text;
                    // between tags it has to close what it opens.
                    if self.state == ScanState::Data {
                        let before = self.clone();
                        if !self.pieces(body)
                            || self.broken
                            || self.state != before.state
                            || self.depth != before.depth
                            || self.roots != before.roots
                        {
                            return false;
                        }
                        *self = before;
                    }
                }
            }
            if self.broken {
                return false;
            }
        }
        true
    }

    fn top_level(&self) -> bool {
        self.depth == 0 && self.state == ScanState::Data
    }

    fn char(&mut self, c: char) {
        match self.state.clone() {
            ScanState::Data => {
                if c == '<' {
                    self.state = ScanState::Open;
                } else if self.depth == 0 && !c.is_whitespace() {
                    self.broken = true;
                }
            }
            ScanState::Open => match c {
                '/' => {
                    self.name.clear();
                    self.state = ScanState::EndName;
                }
                '!' => {
                    self.tail.clear();
                    self.state = ScanState::Bang;
                }
                c if c.is_ascii_alphabetic() => {
                    self.name.clear();
                    self.name.push(c.to_ascii_lowercase());
                    self.self_closing = false;
                    self.state = ScanState::Name;
                }
                _ => {
                    // A `<` that opens nothing is text.
                    if self.depth == 0 {
                        self.broken = true;
                    }
                    self.state = ScanState::Data;
                }
            },
            ScanState::Name => match c {
                '>' => self.start_tag(),
                '/' => {
                    self.self_closing = true;
                    self.state = ScanState::InTag;
                }
                c if c.is_whitespace() => self.state = ScanState::InTag,
                c => self.name.push(c.to_ascii_lowercase()),
            },
            ScanState::InTag => match c {
                '>' => self.start_tag(),
                '"' | '\'' => self.state = ScanState::Quoted(c),
                '/' => self.self_closing = true,
                _ => self.self_closing = false,
            },
            ScanState::Quoted(q) => {
                if c == q {
                    self.self_closing = false;
                    self.state = ScanState::InTag;
                }
            }
            ScanState::EndName => match c {
                '>' => {
                    if self.depth == 0 {
                        self.broken = true;
                    } else {
                        self.depth -= 1;
                    }
                    self.state = ScanState::Data;
                }
                c => self.name.push(c.to_ascii_lowercase()),
            },
            ScanState::Bang => {
                self.tail.push(c);
                if self.tail == "--" {
                    // A comment beside the element is a node of its own.
                    if self.depth == 0 {
                        self.broken = true;
                    }
                    self.tail.clear();
                    self.state = ScanState::Comment;
                } else if !"--".starts_with(self.tail.as_str()) {
                    if self.depth == 0 {
                        self.broken = true;
                    }
                    self.state = if c == '>' {
                        ScanState::Data
                    } else {
                        ScanState::Decl
                    };
                }
            }
            ScanState::Comment => {
                self.tail.push(c);
                if self.tail.ends_with("-->") {
                    self.tail.clear();
                    self.state = ScanState::Data;
                }
            }
            ScanState::Decl => {
                if c == '>' {
                    self.state = ScanState::Data;
                }
            }
            ScanState::Raw => {
                self.tail.push(c.to_ascii_lowercase());
                let end = format!("</{}", self.raw);
                if self.tail.ends_with(&end) {
                    self.tail.clear();
                    self.name = self.raw.clone();
                    self.state = ScanState::EndName;
                } else if self.tail.len() > 64 {
                    let keep = self.tail.len() - end.len();
                    let cut = (keep..self.tail.len())
                        .find(|&i| self.tail.is_char_boundary(i))
                        .unwrap_or(self.tail.len());
                    self.tail.drain(..cut);
                }
            }
        }
    }

    fn start_tag(&mut self) {
        if self.depth == 0 {
            self.roots += 1;
        }
        let name = std::mem::take(&mut self.name);
        if VOID.contains(&name.as_str()) || self.self_closing {
            self.state = ScanState::Data;
        } else {
            self.depth += 1;
            if RAW_TEXT.contains(&name.as_str()) {
                self.raw = name;
                self.tail.clear();
                self.state = ScanState::Raw;
            } else {
                self.state = ScanState::Data;
            }
        }
        self.self_closing = false;
    }
}

/// A `{{&prop}}` value: raw where its schema says `html` and its place takes
/// markup, escaped and fitted everywhere else.
fn raw_text(props: &Value, schema: &Value, name: &str, slot: &Slot) -> String {
    if is_html_prop(schema, name) && slot.takes_markup() {
        prop_text(props, name)
    } else {
        // Undeclared, or standing where markup cannot: escape and fit.
        // Emitting markup a schema never promised was markup is the worse
        // failure, and a raw value inside quotes is a way out of them.
        escape(&fitted_text(props, schema, name, slot))
    }
}

/// Escape for an HTML text node or a double-quoted attribute.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A prop's value as display text. `null` and absent are both empty.
fn prop_text(props: &Value, key: &str) -> String {
    match props.get(key) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Whether `{{#if key}}` should show its body.
///
/// Absent, `null`, `false`, `0` and the empty string are all "no". A
/// conditional that showed an empty string would render an empty box, which is
/// worse than showing nothing.
fn prop_truthy(props: &Value, key: &str) -> bool {
    match props.get(key) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// One piece of a parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Literal text.
    Text(String),
    /// `{{prop}}` — escaped substitution, fitted to the place it stands in.
    Prop {
        /// The prop's name.
        name: String,
        /// Where in the markup it stands (GH #869).
        slot: Slot,
    },
    /// `{{&prop}}` — raw substitution, honoured only for `"html"` props and
    /// only where markup may stand ([`Slot::takes_markup`]).
    Raw {
        /// The prop's name.
        name: String,
        /// Where in the markup it stands (GH #869).
        slot: Slot,
    },
    /// `{{children}}`.
    Children,
    /// `{{#if prop}}…{{/if}}`, already parsed.
    If { prop: String, body: Vec<Piece> },
}

/// Why a template was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError(pub String);

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parse a component template into pieces.
///
/// This is the gate the closed syntax rests on. `component.define` (Task 8)
/// calls it and refuses the definition when it errors, naming the offending
/// `{{…}}` — so an unknown form is answered to whoever wrote it, at the moment
/// they write it.
pub fn parse_template(src: &str) -> Result<Vec<Piece>, TemplateError> {
    let (pieces, rest, ctx) = parse_until_end(src, Contexts::start())?;
    if !rest.is_empty() {
        return Err(TemplateError(
            "unexpected {{/if}} without a matching {{#if …}}".to_string(),
        ));
    }
    // Review I2: what the page puts after a template is read on from where the
    // template ends, so it ends between tags (see `Contexts::unfinished`).
    if let Some(why) = ctx.refused().or_else(|| ctx.unfinished()) {
        return Err(TemplateError(why.to_string()));
    }
    Ok(pieces)
}

/// Parse pieces until end of input or an unconsumed `{{/if}}`.
///
/// Returns the pieces, whatever remains after a closing tag (so the `#if` arm
/// can pick up where its body ended), and the readings of the markup at that
/// point (GH #869): `ctx` is where the template stands when this call starts,
/// fed with every piece of the template's own text.
fn parse_until_end(
    src: &str,
    mut ctx: Contexts,
) -> Result<(Vec<Piece>, &str, Contexts), TemplateError> {
    let mut out = Vec::new();
    let mut rest = src;

    loop {
        let Some(open) = rest.find("{{") else {
            if !rest.is_empty() {
                ctx.feed(rest);
                refuse_text(&ctx)?;
                out.push(Piece::Text(rest.to_string()));
            }
            return Ok((out, "", ctx));
        };
        if open > 0 {
            ctx.feed(&rest[..open]);
            refuse_text(&ctx)?;
            out.push(Piece::Text(rest[..open].to_string()));
        }
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            return Err(TemplateError(format!(
                "unclosed {{{{ near {:?}",
                &after[..after.len().min(24)]
            )));
        };
        let tag = after[..close].trim();
        let tail = &after[close + 2..];

        if tag == "/if" {
            ctx.place(Sub::Branch).map_err(|why| misplaced(tag, why))?;
            return Ok((out, tail, ctx));
        } else if let Some(prop) = tag.strip_prefix("#if ") {
            let prop = prop.trim();
            check_name(prop)?;
            ctx.place(Sub::Branch).map_err(|why| misplaced(tag, why))?;
            let (body, after_body, body_ctx) = parse_until_end(tail, ctx.clone())?;
            // After the conditional the markup is where it was (the body did
            // not render) or where the body left it (it did).
            ctx.join(body_ctx);
            out.push(Piece::If {
                prop: prop.to_string(),
                body,
            });
            rest = after_body;
            // An `#if` whose body ran to end of input never closed.
            if rest.is_empty() && !matches!(out.last(), Some(Piece::If { .. })) {
                return Err(TemplateError("{{#if …}} without {{/if}}".to_string()));
            }
            continue;
        } else if tag == "children" {
            ctx.place(Sub::Children)
                .map_err(|why| misplaced(tag, why))?;
            out.push(Piece::Children);
        } else if let Some(name) = tag.strip_prefix('&') {
            let name = name.trim();
            check_name(name)?;
            let slot = ctx.place(Sub::Value).map_err(|why| misplaced(tag, why))?;
            out.push(Piece::Raw {
                name: name.to_string(),
                slot,
            });
        } else {
            check_name(tag)?;
            let slot = ctx.place(Sub::Value).map_err(|why| misplaced(tag, why))?;
            out.push(Piece::Prop {
                name: tag.to_string(),
                slot,
            });
        }
        rest = tail;
    }
}

/// A `{{…}}` at a place where the template's own text would run on through it
/// (review I1 of #868/#869; the places are [`Contexts::place`]'s).
///
/// The definition rules read the template's text with every `{{…}}` blanked;
/// the renderer puts the value, or nothing, in its place. Where that joins the
/// text on either side into a name — `<scr{{x}}ipt>`, `on{{x}}click=`,
/// `h{{n}}="…"` — or into a scheme — `href="java{{x}}script:…"` — the rules
/// judged a different page than the one that renders. Refused here, every
/// caller of the parser refuses it: `component.define`, the seed, the renderer.
fn misplaced(tag: &str, why: &str) -> TemplateError {
    TemplateError(format!(
        "{{{{{tag}}}}} stands in {why} — a value fills the text between tags or a quoted \
         attribute value, never a name, and a URL attribute takes one whole value or \
         starts with the template's own `/`, `?`, `#` or scheme"
    ))
}

/// The template's own text refused after it was read (see [`misplaced`]).
fn refuse_text(ctx: &Contexts) -> Result<(), TemplateError> {
    match ctx.refused() {
        Some(why) => Err(TemplateError(why.to_string())),
        None => Ok(()),
    }
}

/// A prop name is a plain identifier. Anything else is a form this language
/// does not have, and saying so early is the whole point of the closed syntax.
fn check_name(name: &str) -> Result<(), TemplateError> {
    if name.is_empty() {
        return Err(TemplateError("empty {{}}".to_string()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(TemplateError(format!(
            "{{{{{name}}}}} is not a form this template language has — \
             it knows {{{{prop}}}}, {{{{&prop}}}}, {{{{children}}}} and {{{{#if prop}}}}…{{{{/if}}}}"
        )));
    }
    Ok(())
}

/// Whether `prop_schema` types this prop as raw HTML.
fn is_html_prop(schema: &Value, name: &str) -> bool {
    schema.get(name).and_then(Value::as_str) == Some("html")
}

/// A prop's value as the text its place may hold, or empty (GH #869).
///
/// Typed first: an `"int"` prop is an integer or nothing, wherever it stands.
/// Then placed: the text has to fit the [`Slot`] the parser found for it.
/// Escaping happens after this, at the caller, as before.
fn fitted_text(props: &Value, schema: &Value, name: &str, slot: &Slot) -> String {
    let text = if schema.get(name).and_then(Value::as_str) == Some("int") {
        int_text(props.get(name)).unwrap_or_default()
    } else {
        prop_text(props, name)
    };
    if fits(slot, &text) {
        text
    } else {
        String::new()
    }
}

/// Render one object and everything below it.
pub fn render_object(conn: &Connection, id: &str) -> Result<String, RenderError> {
    let mut r = Renderer::new(conn);
    let tree = r.subtree(id)?;
    Ok(r.part(&tree, id, 0)?.html())
}

/// The test-facing form of the cell's walk: the same truthiness, so a test
/// never disagrees with the cell about what `{{#if}}` sees.
///
/// One component template with its props, the way the cell renders one object
/// with no children: the cell's own parser cuts the pieces (a template it would
/// refuse is refused here, as the error), and the walk is the cell's --
/// escaped `{{prop}}`, raw `{{&prop}}` only where `schema` says `html`,
/// `{{#if}}` on the cell's truthiness. Only `{{children}}` renders as nothing,
/// since there is no tree and no database here.
pub fn render_pieces_plain(
    template: &str,
    props: &Value,
    schema: &Value,
) -> Result<String, TemplateError> {
    let pieces = parse_template(template)?;
    let mut out = String::new();
    // The only error the walk can raise comes from the children source, and
    // this one raises none.
    let _ = render_pieces_with(&pieces, props, schema, &mut out, &mut |_| Ok(()));
    Ok(out)
}

/// The one walk over a template's pieces. `children` fills in `{{children}}`;
/// it is the only step that needs a tree, so it is the only step a caller
/// supplies.
fn render_pieces_with(
    pieces: &[Piece],
    props: &Value,
    schema: &Value,
    out: &mut String,
    children: &mut dyn FnMut(&mut String) -> Result<(), RenderError>,
) -> Result<(), RenderError> {
    for piece in pieces {
        match piece {
            Piece::Text(t) => out.push_str(t),
            Piece::Prop { name, slot } => {
                out.push_str(&escape(&fitted_text(props, schema, name, slot)))
            }
            Piece::Raw { name, slot } => out.push_str(&raw_text(props, schema, name, slot)),
            Piece::Children => children(out)?,
            Piece::If { prop, body } => {
                if prop_truthy(props, prop) {
                    render_pieces_with(body, props, schema, out, children)?;
                }
            }
        }
    }
    Ok(())
}

/// Render a whole route into its packed form.
pub fn materialize(conn: &Connection, route: &str) -> Result<Materialized, RenderError> {
    Renderer::new(conn).page(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_forms_parse() {
        let p = parse_template("a{{x}}b{{&y}}c{{children}}d{{#if z}}e{{/if}}").unwrap();
        assert_eq!(
            p,
            vec![
                Piece::Text("a".into()),
                Piece::Prop {
                    name: "x".into(),
                    slot: Slot::Text
                },
                Piece::Text("b".into()),
                Piece::Raw {
                    name: "y".into(),
                    slot: Slot::Text
                },
                Piece::Text("c".into()),
                Piece::Children,
                Piece::Text("d".into()),
                Piece::If {
                    prop: "z".into(),
                    body: vec![Piece::Text("e".into())]
                },
            ]
        );
    }

    #[test]
    fn a_fifth_form_is_refused_with_the_offender_quoted() {
        // The whole promise of the closed syntax: `component.define` can hand
        // this message straight back to whoever wrote the template.
        let err = parse_template("{{#each items}}{{/each}}").unwrap_err();
        assert!(err.0.contains("not a form"), "{}", err.0);

        let err = parse_template("{{user.name}}").unwrap_err();
        assert!(err.0.contains("user.name"), "{}", err.0);
    }

    #[test]
    fn an_unclosed_brace_is_refused() {
        assert!(parse_template("<p>{{body</p>").is_err());
    }

    #[test]
    fn a_stray_closing_tag_is_refused() {
        assert!(parse_template("a{{/if}}b").is_err());
    }

    #[test]
    fn a_template_with_no_tags_is_all_text() {
        assert_eq!(
            parse_template("<hr>").unwrap(),
            vec![Piece::Text("<hr>".into())]
        );
    }

    #[test]
    fn nested_conditionals_parse() {
        let p = parse_template("{{#if a}}x{{#if b}}y{{/if}}{{/if}}").unwrap();
        assert!(matches!(p.as_slice(), [Piece::If { .. }]));
    }

    /// The plain form sees `{{#if}}` exactly as the cell does: `0`, `[]` and
    /// `{}` are "no", which is where the three test copies it replaces
    /// disagreed with the cell.
    #[test]
    fn the_plain_form_shares_the_cells_truthiness() {
        use meclaw_core::serde_json::json;
        let t = "{{#if a}}A{{/if}}{{#if b}}B{{/if}}{{#if c}}C{{/if}}{{#if d}}D{{/if}}{{&raw}}{{p}}{{children}}";
        let out = render_pieces_plain(
            t,
            &json!({"a": 0, "b": [], "c": {}, "d": "x", "raw": "<i>", "p": "<b>"}),
            &json!({"raw": "html"}),
        )
        .unwrap();
        assert_eq!(out, "D<i>&lt;b&gt;");
        assert!(render_pieces_plain("{{user.name}}", &json!({}), &json!({})).is_err());
    }

    fn plain(t: &str, props: Value, schema: Value) -> String {
        render_pieces_plain(t, &props, &schema).unwrap()
    }

    /// GH #869: a binding carries an event name; a JSON command list renders
    /// empty. `phx-value-*` stays escaped text, because the client executes
    /// nothing there and the display writes object ids into it.
    #[test]
    fn a_binding_renders_an_event_name_and_never_a_command_list() {
        let t = r#"<button phx-click="{{e}}" phx-value-for="{{v}}">{{e}}</button>"#;
        assert_eq!(
            plain(t, json!({"e": "save", "v": "w/0"}), json!({})),
            r#"<button phx-click="save" phx-value-for="w/0">save</button>"#
        );
        let cmd = r#"[["exec",{"attr":"onclick"}]]"#;
        let out = plain(t, json!({"e": cmd, "v": cmd}), json!({}));
        assert!(
            out.starts_with(r#"<button phx-click="" phx-value-for="[[&quot;exec"#),
            "{out}"
        );
        assert!(
            out.ends_with(">[[&quot;exec&quot;,{&quot;attr&quot;:&quot;onclick&quot;}]]</button>"),
            "{out}"
        );
        // Inside a conditional, the way the display writes its bindings.
        let t = r#"<b type="button"{{#if e}} phx-keyup="{{e}}"{{/if}}>"#;
        assert_eq!(
            plain(t, json!({"e": cmd}), json!({})),
            r#"<b type="button" phx-keyup="">"#
        );
    }

    /// GH #869: a URL attribute takes a relative URL or an allowed scheme.
    #[test]
    fn a_url_attribute_renders_no_script_scheme() {
        let t = r#"<img src="{{src}}" alt="{{src}}">"#;
        assert_eq!(
            plain(t, json!({"src": "https://example.org/a.png"}), json!({})),
            r#"<img src="https://example.org/a.png" alt="https://example.org/a.png">"#
        );
        assert_eq!(
            plain(t, json!({"src": "javascript:window.__pwned=1"}), json!({})),
            r#"<img src="" alt="javascript:window.__pwned=1">"#
        );
    }

    /// GH #869: a value inside `style` cannot open a declaration, and an
    /// `int` prop renders an integer or nothing at all.
    #[test]
    fn a_style_value_and_an_int_stay_what_they_are() {
        let t = r#"<div style="--v: {{v}}; --n: {{n}}" data-n="{{n}}">{{n}}</div>"#;
        let schema = json!({"v": "text", "n": "int"});
        assert_eq!(
            plain(t, json!({"v": "c.p", "n": 42}), schema.clone()),
            r#"<div style="--v: c.p; --n: 42" data-n="42">42</div>"#
        );
        assert_eq!(
            plain(
                t,
                json!({"v": "0;background:url(//x)", "n": "0;background:url(//x)"}),
                schema.clone()
            ),
            r#"<div style="--v: ; --n: " data-n=""></div>"#
        );
        assert_eq!(
            plain(t, json!({"v": "1", "n": "-12"}), schema.clone()),
            r#"<div style="--v: 1; --n: -12" data-n="-12">-12</div>"#
        );
        assert_eq!(
            plain(t, json!({"n": 1.5}), schema),
            r#"<div style="--v: ; --n: " data-n=""></div>"#
        );
    }

    /// GH #869: a raw prop stands as markup only where markup may stand. In an
    /// attribute it is escaped and fitted like any other value.
    #[test]
    fn a_raw_prop_is_raw_only_between_tags_and_in_a_script_body() {
        let schema = json!({"h": "html"});
        assert_eq!(
            plain(
                "<p>{{&h}}</p><script>{{&h}}</script>",
                json!({"h": "<i>a</i>"}),
                schema.clone()
            ),
            "<p><i>a</i></p><script><i>a</i></script>"
        );
        assert_eq!(
            plain(
                r#"<a title="{{&h}}" href="{{&h}}">"#,
                json!({"h": "\"><script>x</script>"}),
                schema.clone()
            ),
            r#"<a title="&quot;&gt;&lt;script&gt;x&lt;/script&gt;" href="&quot;&gt;&lt;script&gt;x&lt;/script&gt;">"#
        );
        assert_eq!(
            plain(r#"<a href="{{&h}}">"#, json!({"h": "javascript:x"}), schema),
            r#"<a href="">"#
        );
    }

    /// GH #869: a value never stands inside a tag outside quotes -- there it
    /// would be an attribute name (review I1). And a conditional whose branches
    /// leave the markup in two different places puts the value under the
    /// narrowest grammar, a token.
    #[test]
    fn a_value_inside_a_tag_is_refused_and_one_in_doubt_is_a_token() {
        assert!(parse_template("<div {{a}}>").is_err());
        assert!(parse_template("<div a={{a}}>").is_err());
        // The template closes its quote and tag in both readings: it ends between
        // tags, as every template does (review I2).
        let t = r#"<b>{{#if x}}<a title="{{/if}}{{u}}">"#;
        let pieces = parse_template(t).unwrap();
        assert!(
            pieces.contains(&Piece::Prop {
                name: "u".into(),
                slot: Slot::Tag
            }),
            "{pieces:?}"
        );
        assert_eq!(
            plain(t, json!({"x": true, "u": "javascript:x"}), json!({})),
            r#"<b><a title="">"#
        );
        assert_eq!(
            plain(t, json!({"x": true, "u": "a-1"}), json!({})),
            r#"<b><a title="a-1">"#
        );
    }

    /// The parser reads the markup across conditionals and out of raw text:
    /// the display's own shell, a style block and a script block before its
    /// attributes, lands every value in the right place.
    #[test]
    fn the_parser_finds_the_place_of_every_value() {
        let t = concat!(
            r#"{{#if s}}<link rel="stylesheet" href="vision.css">{{/if}}"#,
            r#"<style>a[x="y"]>b{c:d}{{&faces}}</style>"#,
            r#"<div class="stack{{#if t}} thin{{/if}}" id="{{id}}" style="--scale: {{scale}}">"#,
            r#"{{children}}</div><script>{{&js}}</script><p>{{body}}</p>"#
        );
        let slots: Vec<(String, Slot)> = parse_template(t)
            .unwrap()
            .into_iter()
            .filter_map(|p| match p {
                Piece::Prop { name, slot } | Piece::Raw { name, slot } => Some((name, slot)),
                _ => None,
            })
            .collect();
        assert_eq!(
            slots,
            vec![
                ("faces".to_string(), Slot::RawText),
                ("id".to_string(), Slot::Attr("id".into())),
                ("scale".to_string(), Slot::Attr("style".into())),
                ("js".to_string(), Slot::RawText),
                ("body".to_string(), Slot::Text),
            ]
        );
    }

    fn one_root(t: &str) -> bool {
        one_root_element(&parse_template(t).expect("parses"))
    }

    #[test]
    fn one_element_is_one_root_and_anything_beside_it_is_not() {
        // GH #1001: `"r": 1` only where the client's skip path keeps the page.
        assert!(one_root("<div class=\"a {{x}}\">{{y}}<b>{{&z}}</b></div>"));
        assert!(one_root("  <p>{{#if on}}<i>on</i>{{/if}}</p>\n"));
        assert!(one_root("<div><br><img src=\"/a.png\"><input/></div>"));
        assert!(one_root("<div><!-- a > b --><style>p > i {}</style></div>"));
        assert!(one_root("<section>{{children}}</section>"));
        assert!(one_root("<hr>"));
        assert!(!one_root("<b>{{a}}</b><i>{{b}}</i>"), "two elements");
        assert!(!one_root("<p>x</p> tail"), "text beside the element");
        assert!(!one_root("{{x}}<p></p>"), "a value beside the element");
        assert!(!one_root("{{children}}"), "children at the top level");
        assert!(!one_root("<!-- c --><p></p>"), "a comment beside it");
        assert!(
            !one_root("{{#if a}}<p></p>{{/if}}"),
            "an element that may be absent"
        );
        assert!(
            !one_root("<p>{{#if a}}<i>{{/if}}</p>"),
            "a conditional that opens"
        );
        assert!(!one_root("plain text"), "no element at all");
    }

    fn node(statics: &[&str], dynamics: Vec<Part>) -> Part {
        Part::Node(Arc::new(Node {
            statics: statics
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .into(),
            root: true,
            dynamics,
        }))
    }

    #[test]
    fn a_diff_names_only_the_values_that_changed() {
        let old = node(&["<p a=\"", "\">", "</p>"], vec!["1".into(), "x".into()]);
        let new = node(&["<p a=\"", "\">", "</p>"], vec!["2".into(), "x".into()]);
        let mut shared = SharedStatics::default();
        assert_eq!(diff_value(&old, &new, &mut shared), Some(json!({"0": "2"})));
        assert!(
            shared.table.is_empty(),
            "no statics travel for a changed value"
        );
        assert_eq!(diff_value(&old, &old.clone(), &mut shared), None);

        let other = node(&["<i>", "</i>"], vec!["y".into()]);
        let mut shared = SharedStatics::default();
        assert_eq!(
            diff_value(&old, &other, &mut shared),
            Some(json!({"s": 0, "0": "y", "r": 1})),
            "other statics: the part in full, its statics by number"
        );
        let mut frame = Map::new();
        shared.attach(&mut frame);
        assert_eq!(frame["p"], json!({"0": ["<i>", "</i>"]}));
        assert_eq!(
            wire_html(&json!({"s": 0, "0": "y"}), &frame["p"]),
            "<i>y</i>"
        );
    }
}
