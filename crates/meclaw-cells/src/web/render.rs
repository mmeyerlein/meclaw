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

/// Every route this cell serves, rendered, with the generation it was
/// published as.
///
/// A `BTreeMap` rather than a `HashMap` so a listing of routes is stable — an
/// operator comparing two dumps should not have to sort them first. It reads
/// as that map (`Deref`); the one thing beside it is [`PageMap::generation`].
#[derive(Debug, Clone, Default)]
pub struct PageMap {
    /// GH #1013: which publish this is. The handler, the only writer, counts
    /// it up by one per publish, and every diff it fans out carries the
    /// generation of the pages it leads to (`WebReconfig::Push`). A join
    /// remembers the generation of its snapshot, and a diff of that
    /// generation or older is not sent to it: the snapshot holds it already,
    /// and a keyed-list diff applied twice is not the same page (its moves
    /// copy entries the client holds).
    pub generation: u64,
    routes: BTreeMap<String, Materialized>,
}

impl PageMap {
    /// No routes, generation 0.
    pub fn new() -> Self {
        Self::default()
    }
}

impl std::ops::Deref for PageMap {
    type Target = BTreeMap<String, Materialized>;
    fn deref(&self) -> &Self::Target {
        &self.routes
    }
}

impl std::ops::DerefMut for PageMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.routes
    }
}

impl<'a> IntoIterator for &'a PageMap {
    type Item = (&'a String, &'a Materialized);
    type IntoIter = std::collections::btree_map::Iter<'a, String, Materialized>;
    fn into_iter(self) -> Self::IntoIter {
        self.routes.iter()
    }
}

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
    ///
    /// On the wire the root is two statics — the first and the last piece —
    /// around ONE dynamic, the children as a keyed list (GH #1013,
    /// [`Self::packed_tree`]); the empty separators are never sent.
    pub statics: Vec<String>,
    /// One `(object_id, part)` per **direct child** of the page root, in order.
    pub slots: Vec<(String, Part)>,
    /// The page title, for the shell's `<title>`.
    pub title: String,
}

impl Materialized {
    /// The LiveView packed tree: `{"s": [first, last], "0": <root children>,
    /// "p": shared}`.
    ///
    /// The root's children are one keyed list (`"k"`), keyed by object id like
    /// every `{{children}}` below the root (GH #1009). A create, delete or move
    /// directly under the root is then a list difference that keeps every
    /// other child's node: with one slot per child the root's statics changed
    /// with every such op, the page went out whole, and the browser re-created
    /// all of it (GH #1013: 1 002 of 1 002 nodes on a root of 1 000 children,
    /// frame 107 693 B). Every component's statics stand once in `"p"` and
    /// every part names them by number. A page without children is its one
    /// static.
    pub fn packed_tree(&self) -> Value {
        let mut shared = SharedStatics::default();
        let mut m = Map::new();
        m.insert("s".to_string(), self.wire_statics());
        if !self.slots.is_empty() {
            m.insert("0".to_string(), list_value(&self.slots, &mut shared));
        }
        shared.attach(&mut m);
        Value::Object(m)
    }

    /// The root's statics on the wire: the first and the last piece around
    /// the children, or the one piece of a page without children.
    fn wire_statics(&self) -> Value {
        match (
            self.slots.is_empty(),
            self.statics.first(),
            self.statics.last(),
        ) {
            (false, Some(first), Some(last)) if self.statics.len() > 1 => json!([first, last]),
            _ => Value::Array(self.statics.iter().map(|s| json!(s)).collect()),
        }
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

    /// GH #1002: the bytes of slot HTML this page carries, statics excluded.
    pub fn slot_bytes(&self) -> usize {
        self.slots.iter().map(|(_, part)| part.html_len()).sum()
    }

    /// GH #1002: whether this page is large enough to be cut at all — more
    /// than [`CUT_ABOVE_BYTES`] of slot HTML.
    ///
    /// The page's GET body and its join are cut only then. A display page is
    /// below, so not one byte of what a display serves moves: the inline
    /// budget alone cut a display's screen out of the GET (GH #553 twice: the
    /// topology picture never arrived in the page).
    pub fn is_large(&self) -> bool {
        self.slot_bytes() > CUT_ABOVE_BYTES
    }

    /// GH #1002: what the GET shell embeds — the first `inline` bytes of slot
    /// HTML for a large page ([`Self::rendered_body_cut`]), the whole body for
    /// every other one ([`Self::rendered_body`], byte for byte as before).
    pub fn page_body(&self, inline: usize) -> String {
        if self.is_large() {
            self.rendered_body_cut(inline)
        } else {
            self.rendered_body()
        }
    }

    /// GH #1002: the join of this page — head and pieces cut at `chunk` for a
    /// large page ([`Self::cut_frames`]), the whole tree in one frame for
    /// every other one, as before.
    pub fn join_frames(&self, chunk: usize) -> (Value, Vec<Value>) {
        if self.is_large() {
            self.cut_frames(chunk)
        } else {
            (self.packed_tree(), Vec::new())
        }
    }

    /// GH #1002: the page as one HTML string, with every root slot after the
    /// first `limit` bytes of slot HTML left empty.
    ///
    /// What the GET shell embeds. The join fills the rest (its pieces are
    /// cut at their own, larger limit), so the page carries the first screen
    /// and not the whole tree a second time — the measured city's page was
    /// 636 KB, the same tree its join carried again. A page whose slots fit is
    /// [`Self::rendered_body`] byte for byte.
    pub fn rendered_body_cut(&self, limit: usize) -> String {
        let mut out = String::new();
        let mut used = 0usize;
        let mut open = true;
        for (i, s) in self.statics.iter().enumerate() {
            out.push_str(s);
            if let Some((_, part)) = self.slots.get(i) {
                // A prefix in document order: once one slot is left out, every
                // later one is too, so the page never shows a hole between two
                // drawn slots.
                let len = part.html_len();
                open = open && used + len <= limit;
                if open {
                    used += len;
                    part.write_html(&mut out);
                }
            }
        }
        out
    }

    /// GH #1002: the packed tree cut into a head and pieces, none above
    /// `limit` encoded bytes (except a piece holding a single entry that alone
    /// is larger).
    ///
    /// The cut runs between entries of the root's keyed list (GH #1013): the
    /// **head** is the tree with the list's first entries, as many as fit,
    /// and `"kc"` their count; every **piece** is a list difference that
    /// appends the next entries (`{"0": {"k": {"<i>": …, "kc": <i + 1>}}}`).
    /// The client merges an entry it does not hold into a fresh one and sets
    /// the count from `"kc"` (`phoenix_live_view.min.js`, `mergeKeyed`), so the
    /// list grows piece by piece to the page's, every entry keyed like the
    /// writes after it. Every frame — head and each piece — carries its own
    /// `"p"`, numbered from 0 and holding only the statics its own parts name:
    /// the client resolves a numeric `"s"` against the table of the frame it
    /// arrives in and deletes the table after rendering it (GH #1001), so a
    /// piece cannot lean on the head's.
    ///
    /// A tree that fits comes back as [`Self::packed_tree`] with no pieces.
    /// The page handlers reach this only for a large page
    /// ([`Self::join_frames`]): a display's join never does.
    ///
    /// Measured on a large page: a 0.69 MB join reply in ONE frame does
    /// not arrive within the client's 10 s join timeout below ~70 KB/s; in
    /// pieces the reply is ≤ the limit and the rest streams behind it. Every
    /// viewer of a route gets the same cut — no per-viewer state (R-H4-1).
    pub fn cut_frames(&self, limit: usize) -> (Value, Vec<Value>) {
        let whole = self.packed_tree();
        if self.slots.is_empty() || whole.to_string().len() <= limit {
            return (whole, Vec::new());
        }
        let total = self.slots.len();

        // The head: the root's statics and the longest prefix of entries that
        // fits; a document-order prefix, so the page never shows a hole.
        let mut head = Frame::head(self.wire_statics(), total);
        let mut first_out = total;
        for (i, (_, part)) in self.slots.iter().enumerate() {
            if !head.try_add(i, part, limit) {
                first_out = i;
                break;
            }
        }

        // The pieces: greedy, in document order, each with a table of its own.
        let mut pieces: Vec<Value> = Vec::new();
        let mut piece = Frame::piece(total);
        for (i, (_, part)) in self.slots.iter().enumerate().skip(first_out) {
            if !piece.try_add(i, part, limit) {
                pieces.push(std::mem::replace(&mut piece, Frame::piece(total)).into_value());
                // Alone in a fresh piece an entry always goes in.
                let added = piece.try_add(i, part, limit);
                debug_assert!(added);
            }
        }
        if !piece.entries.is_empty() {
            pieces.push(piece.into_value());
        }
        (head.into_value(), pieces)
    }
}

/// GH #1002: one frame of a cut join, built entry by entry with its own `"p"`.
///
/// The head carries the root's statics and the list in full form; a piece the
/// list's difference. Either way the entries stand in the list's `"k"`
/// (GH #1013).
struct Frame {
    /// The root's statics, for the head; `None` for a piece.
    statics: Option<Value>,
    /// The list's `"k"` so far, without `"kc"`.
    entries: Map<String, Value>,
    /// One past the last entry: the list's `"kc"` once this frame is merged.
    count: usize,
    shared: SharedStatics,
    /// Encoded bytes so far, the frame's wrapper and its `"p"` included.
    used: usize,
}

impl Frame {
    /// The head: `{"s": statics, "0": {"s": <list>, "k": {"kc": n}}}`, its
    /// table already naming the list's statics (number 0).
    fn head(statics: Value, total: usize) -> Self {
        let mut shared = SharedStatics::default();
        let list = shared.id(&list_statics());
        let wrapper = json!({"s": statics, "0": {"s": list, "k": {"kc": total}}});
        Self {
            used: wrapper.to_string().len() + shared.encoded_from(0),
            statics: Some(statics),
            entries: Map::new(),
            count: 0,
            shared,
        }
    }

    /// A piece: `{"0": {"k": {"kc": n}}}`. `total` bounds the digits of the
    /// count it will carry.
    fn piece(total: usize) -> Self {
        Self {
            statics: None,
            entries: Map::new(),
            count: 0,
            used: json!({"0": {"k": {"kc": total}}}).to_string().len(),
            shared: SharedStatics::default(),
        }
    }

    /// Add entry `i` if the frame stays within `limit` (or, for a piece,
    /// holds no entry yet: an entry larger than the limit then goes alone).
    fn try_add(&mut self, i: usize, part: &Part, limit: usize) -> bool {
        let mark = self.shared.table.len();
        let key = i.to_string();
        let value = entry(full_value(part, &mut self.shared));
        let grow = slot_len(&key, &value) + self.shared.encoded_from(mark);
        let fits = self.used + grow <= limit;
        let alone = self.statics.is_none() && self.entries.is_empty();
        if fits || alone {
            self.used += grow;
            self.entries.insert(key, value);
            self.count = i + 1;
            true
        } else {
            self.shared.truncate(mark);
            false
        }
    }

    fn into_value(self) -> Value {
        let mut k = self.entries;
        k.insert("kc".to_string(), json!(self.count));
        let mut list = Map::new();
        let mut map = Map::new();
        if let Some(statics) = self.statics {
            list.insert("s".to_string(), json!(0));
            map.insert("s".to_string(), statics);
        }
        list.insert("k".to_string(), Value::Object(k));
        map.insert("0".to_string(), Value::Object(list));
        self.shared.attach(&mut map);
        Value::Object(map)
    }
}

/// GH #1002: a page with at most this many bytes of slot HTML is never cut —
/// neither its GET body nor its join.
///
/// The cut exists for the measured city (about 600 KB of slot HTML, a 645 KB
/// join on one route) and must not touch a display. Measured on the largest
/// shipped display page (`examples/display-colony-view`, lock
/// `gh1002_the_display_example_is_served_whole`): 130 968 bytes of slot HTML
/// in 4 slots, a 238 KB page and a 245 KB join reply. Factor 2 of headroom
/// (OR-H4-7) is 262 KB; 320 KB keeps that factor with room for the picture to
/// vary, and the city stays nearly twice above it.
pub const CUT_ABOVE_BYTES: usize = 320 * 1024;

/// The bytes `s` takes as a JSON string, quotes included, without building it.
///
/// The limit of a join piece is a limit on the frame, and a frame is JSON:
/// HTML is full of `"`, which the encoder doubles to `\"`, so the raw length
/// under-counts by about a tenth on the measured city.
fn json_str_len(s: &str) -> usize {
    let mut n = 2 + s.len();
    for b in s.bytes() {
        n += match b {
            b'"' | b'\\' | b'\n' | b'\r' | b'\t' | 0x08 | 0x0c => 1,
            0x00..=0x1f => 5,
            _ => 0,
        };
    }
    n
}

/// The encoded size of one slot value, key and separators included.
fn slot_len(key: &str, value: &Value) -> usize {
    let v = match value {
        Value::String(s) => json_str_len(s),
        other => meclaw_core::serde_json::to_string(other).map_or(0, |t| t.len()),
    };
    // `"<key>":<value>,`
    key.len() + 4 + v
}

/// One rendered object, or one piece of one: LiveView's "rendered" shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// A string on the wire: a substituted value, or the whole markup of an
    /// object whose template is not exactly one element.
    Text(String),
    /// Statics and dynamics.
    Node(Arc<Node>),
    /// An object's children, each under its object id: a keyed comprehension
    /// on the wire (GH #1009).
    List(Arc<List>),
}

/// The children of one object, in order.
///
/// On the wire a keyed comprehension (`"k"`): its statics are two empty
/// strings whatever the count, so a list that grows or shrinks is a
/// difference and not a new part, and an entry the client holds can be
/// moved to another index with everything it carries. The client keys every
/// root part's node by the `data-phx-id` it gave that part
/// (`phoenix_live_view.min.js`, `getNodeKey` in `DOMPatch`) and keeps the id
/// only while frames merge into the part: a list sent whole, as the n + 1
/// statics of GH #1001 forced on every create and delete, re-created every
/// child's node, and a reorder diffed by position handed one object's node
/// to the next (GH #1009: the display's motion lock, `replaced: 6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    /// `(object_id, part)`, in `ord` order.
    pub items: Vec<(String, Part)>,
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

    /// GH #1002: the length of [`Self::html`], without building it.
    pub fn html_len(&self) -> usize {
        match self {
            Part::Text(t) => t.len(),
            Part::Node(n) => {
                n.statics.iter().map(String::len).sum::<usize>()
                    + n.dynamics.iter().map(Part::html_len).sum::<usize>()
            }
            // A list's markup is its entries back to back (`write_html`).
            Part::List(l) => l.items.iter().map(|(_, part)| part.html_len()).sum(),
        }
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
            Part::List(l) => {
                for (_, part) in &l.items {
                    part.write_html(out);
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

    /// GH #1002: the encoded bytes the entries from `mark` on add to `"p"`,
    /// the `"p":{}` itself included when they are its first.
    fn encoded_from(&self, mark: usize) -> usize {
        let entries: usize = self.table[mark..]
            .iter()
            .enumerate()
            .map(|(k, statics)| {
                let key = (mark + k).to_string().len() + 4;
                key + 2 + statics.iter().map(|s| json_str_len(s) + 1).sum::<usize>()
            })
            .sum();
        let opened = if mark == 0 && entries > 0 { 7 } else { 0 };
        entries + opened
    }

    /// GH #1002: forget the entries from `mark` on (a slot that did not fit).
    fn truncate(&mut self, mark: usize) {
        for statics in self.table.drain(mark..) {
            self.index.remove(&statics);
        }
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
        Part::List(l) => list_value(&l.items, shared),
    }
}

/// A keyed list in full: `{"s": <list statics>, "k": {"0": {"0": …}, …,
/// "kc": n}}` — an object's children, or the root's (GH #1013).
fn list_value(items: &[(String, Part)], shared: &mut SharedStatics) -> Value {
    let mut k = Map::new();
    for (i, (_, part)) in items.iter().enumerate() {
        k.insert(i.to_string(), entry(full_value(part, shared)));
    }
    k.insert("kc".to_string(), json!(items.len()));
    let mut m = Map::new();
    m.insert("s".to_string(), json!(shared.id(&list_statics())));
    m.insert("k".to_string(), Value::Object(k));
    Value::Object(m)
}

/// The statics of every list entry: the child stands alone between them.
fn list_statics() -> Arc<[String]> {
    Arc::from(vec![String::new(), String::new()])
}

/// One list entry: its one dynamic, the child.
fn entry(child: Value) -> Value {
    let mut m = Map::new();
    m.insert("0".to_string(), child);
    Value::Object(m)
}

/// What a viewer holding the list `old` needs to hold `new`, by object id.
///
/// An index whose object is the one the viewer holds there carries the
/// child's difference, if any; an object the viewer holds at another index is
/// moved from there (`[from, diff]`, or `from` alone — the client clones the
/// entry it held, the part's `data-phx-id` with it, `mergeKeyed`); an object
/// new to the list comes in full. `"kc"` is the new length and is always
/// there: the client sets the count from it, and truncates what lies past it.
fn diff_list(
    old: &[(String, Part)],
    new: &[(String, Part)],
    shared: &mut SharedStatics,
) -> Option<Value> {
    let held: HashMap<&str, usize> = old
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id.as_str(), i))
        .collect();
    let mut k = Map::new();
    let mut moved = false;
    for (j, (id, part)) in new.iter().enumerate() {
        let v = match held.get(id.as_str()) {
            Some(&i) => {
                let d = diff_value(&old[i].1, part, shared);
                if i == j {
                    match d {
                        Some(d) => entry(d),
                        None => continue,
                    }
                } else {
                    moved = true;
                    match d {
                        Some(d) => json!([i, entry(d)]),
                        None => json!(i),
                    }
                }
            }
            None => entry(full_value(part, shared)),
        };
        k.insert(j.to_string(), v);
    }
    if k.is_empty() && old.len() == new.len() {
        return None;
    }
    k.insert("kc".to_string(), json!(new.len()));
    if moved {
        // The client reads a moved entry from a copy of the list it held, and
        // makes that copy only when the frame says so.
        k.insert("km".to_string(), json!(1));
    }
    let mut m = Map::new();
    m.insert("k".to_string(), Value::Object(k));
    Some(Value::Object(m))
}

/// What a viewer holding `old` needs to hold `new`, or `None` if nothing.
///
/// The same statics: only the dynamics that changed, by index, each again as
/// a difference — the client merges an object without `"s"` into the part it
/// has. Two lists: by object id ([`diff_list`]). Anything else: the new part
/// in full, which the client puts in place.
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
        (Part::List(a), Part::List(b)) if Arc::ptr_eq(a, b) => None,
        (Part::List(a), Part::List(b)) => diff_list(&a.items, &b.items, shared),
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
            if let Some(k) = m.get("k") {
                let n = k["kc"].as_u64().unwrap_or_default();
                for e in 0..n {
                    out.push_str(&wire_html(
                        &json!({"s": statics, "0": k[e.to_string()]["0"]}),
                        shared,
                    ));
                }
                return out;
            }
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
                    let mut items = Vec::with_capacity(ids.len());
                    for child in ids {
                        items.push((child.clone(), self.part(tree, child, depth + 1)?));
                    }
                    Part::List(Arc::new(List { items }))
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
/// - the root-object case (structural, no slot): every route whole;
/// - a route `old` does not have, or a write that names the page root
///   (`page.set`, `component.define`), or a root template that folds its
///   children into its statics: that route whole;
/// - everything else: the named slots re-rendered (a structural write re-reads
///   the root's child list; a slot new to it is rendered too), every other
///   slot taken over.
///
/// Every frame is [`page_frame`]: the root list's difference when the viewers
/// hold the same root statics, the packed tree otherwise.
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
            .map(|(route, page)| (route.clone(), page_frame(old.get(route), page)))
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
            Ok(None) => whole(&mut r, route, old.get(route)),
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

/// A route rendered whole after a write, and its frame against `prev`, the
/// page the viewers hold ([`page_frame`]).
fn whole(
    r: &mut Renderer<'_>,
    route: &str,
    prev: Option<&Materialized>,
) -> Result<(Materialized, Value), RenderError> {
    let page = r.page(route)?;
    let frame = page_frame(prev, &page);
    Ok((page, frame))
}

/// The frame that takes a viewer from `prev` to `page`.
///
/// When the viewer holds a page with the same root statics, the frame is the
/// difference of the root's keyed list ([`diff_list`]): a child the write did
/// not change is not in it, a child created, deleted or moved directly under
/// the root is an entry added, cut off or moved (GH #1013), and every part the
/// client holds is merged into, never replaced — the client keys a part's
/// node by the `data-phx-id` it gave the part and keeps it only then
/// (GH #1009). The root-object case is the common one: a curator pass writes
/// a root prop the shell does not even draw (`due`), and that sent every
/// viewer the packed tree, so the browser re-created every node on the page.
///
/// Anything else is the packed tree, with `"s"` at the top: the root's
/// statics changed (a root prop the template draws, a page that gains its
/// first child or loses its last), or the viewer holds no page.
fn page_frame(prev: Option<&Materialized>, page: &Materialized) -> Value {
    let Some(prev) = prev else {
        return page.packed_tree();
    };
    if prev.slots.is_empty() != page.slots.is_empty() || prev.wire_statics() != page.wire_statics()
    {
        return page.packed_tree();
    }
    let mut shared = SharedStatics::default();
    let mut m = Map::new();
    if let Some(v) = diff_list(&prev.slots, &page.slots, &mut shared) {
        m.insert("0".to_string(), v);
    }
    shared.attach(&mut m);
    Value::Object(m)
}

/// The frame of a root list difference that names entries by index:
/// `{"0": {"k": {"<i>": {"0": diff}, …, "kc": n}}}`, or `{}` with none.
/// `"kc"` is always there: the client sets the count from it unchecked.
fn root_entries(entries: Map<String, Value>, count: usize, shared: SharedStatics) -> Value {
    let mut m = Map::new();
    if !entries.is_empty() {
        let mut k = entries;
        k.insert("kc".to_string(), json!(count));
        let mut list = Map::new();
        list.insert("k".to_string(), Value::Object(k));
        m.insert("0".to_string(), Value::Object(list));
    }
    shared.attach(&mut m);
    Value::Object(m)
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
            m.insert(i.to_string(), entry(v));
        }
    }
    Ok(Some(root_entries(m, page.slots.len(), shared)))
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
    // The slots taken over are the parts the viewer holds (`Arc` for `Arc`),
    // so the difference walks only the named ones.
    let frame = page_frame(Some(prev), &page);
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

    /// A frame as the client holds it after rendering it (GH #1001): every
    /// numeric `"s"` replaced by the statics it names in that frame's own
    /// `"p"`, and the table gone.
    fn resolved(frame: &Value) -> Value {
        fn walk(v: &Value, p: &Value) -> Value {
            match v {
                Value::Object(m) => Value::Object(
                    m.iter()
                        .filter(|(k, _)| k.as_str() != "p")
                        .map(|(k, x)| {
                            let y = match (k.as_str(), x) {
                                ("s", Value::Number(n)) => {
                                    let t = p[n.to_string()].clone();
                                    assert!(t.is_array(), "statics {n} are not in this frame's p");
                                    t
                                }
                                ("s", other) => other.clone(),
                                _ => walk(x, p),
                            };
                            (k.clone(), y)
                        })
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        walk(frame, &frame["p"])
    }

    /// The client's merge of a cut join: each frame resolved against its own
    /// table, then every entry of a piece's root list goes into the list the
    /// head brought and the count is the piece's (`mergeKeyed`, GH #1013).
    fn merge_all(head: &Value, pieces: &[Value]) -> Value {
        let mut t = resolved(head);
        for p in pieces {
            let piece = resolved(p);
            assert!(piece.get("s").is_none(), "a piece is a diff");
            assert!(
                piece["0"].get("s").is_none(),
                "a piece merges into the list"
            );
            let k = t["0"]["k"].as_object_mut().expect("the head's list");
            for (key, v) in piece["0"]["k"].as_object().expect("a list diff") {
                k.insert(key.clone(), v.clone());
            }
        }
        t
    }

    /// The root list entries a frame carries, by index (`"kc"` left out).
    fn entries(frame: &Value) -> Vec<usize> {
        let mut out: Vec<usize> = frame["0"]["k"]
            .as_object()
            .expect("a root list")
            .keys()
            .filter(|k| *k != "kc")
            .map(|k| k.parse().expect("an index"))
            .collect();
        out.sort_unstable();
        out
    }

    /// The markup of a resolved tree.
    fn html_of(tree: &Value) -> String {
        wire_html(tree, &Value::Null)
    }

    fn page_of(sizes: &[usize]) -> Materialized {
        Materialized {
            statics: std::iter::repeat_n(String::new(), sizes.len() + 1).collect(),
            slots: sizes
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    (
                        format!("o{i}"),
                        format!("<p a=\"{i}\">{}</p>", "x".repeat(*n)).into(),
                    )
                })
                .collect(),
            title: "t".into(),
        }
    }

    /// Slots of nested parts: two components alternate, each slot holds a
    /// list (W1b, GH #1009: `{{children}}` is a keyed comprehension) of two
    /// children of a third, so every frame names shared statics.
    fn nested_page(slots: usize, text: usize) -> Materialized {
        let st = |v: &[&str]| -> Arc<[String]> { v.iter().map(|s| s.to_string()).collect() };
        let (a, b, kid) = (
            st(&["<div class=\"a\" data-k=\"", "\">", "</div>"]),
            st(&["<section data-k=\"", "\">", "</section>"]),
            st(&["<i>", "</i>"]),
        );
        let node = |statics: &Arc<[String]>, dynamics: Vec<Part>| {
            Part::Node(Arc::new(Node {
                statics: Arc::clone(statics),
                root: true,
                dynamics,
            }))
        };
        Materialized {
            statics: std::iter::once("<main>".to_string())
                .chain(std::iter::repeat_n(String::new(), slots - 1))
                .chain(std::iter::once("</main>".to_string()))
                .collect(),
            slots: (0..slots)
                .map(|i| {
                    let kids = Part::List(Arc::new(List {
                        items: ["a", "b"]
                            .iter()
                            .map(|k| {
                                let text = format!("{i}{k}{}", "y".repeat(text));
                                (format!("o{i}{k}"), node(&kid, vec![text.into()]))
                            })
                            .collect(),
                    }));
                    let outer = if i % 2 == 0 { &a } else { &b };
                    (
                        format!("o{i}"),
                        node(outer, vec![i.to_string().into(), kids]),
                    )
                })
                .collect(),
            title: "t".into(),
        }
    }

    /// Every piece fits, or holds one entry; and the pieces append in
    /// document order, each one's `"kc"` one past its last entry.
    fn assert_pieces_fit(pieces: &[Value], limit: usize) {
        for p in pieces {
            let n = p.to_string().len();
            let at = entries(p);
            assert!(
                n <= limit || at.len() == 1,
                "a piece of {n} bytes holds {} entries",
                at.len()
            );
            assert_eq!(
                p["0"]["k"]["kc"],
                json!(at.last().expect("an entry") + 1),
                "kc"
            );
        }
    }

    #[test]
    fn gh1002_the_client_builds_the_same_html_from_chunks() {
        for sizes in [
            vec![10; 3],
            vec![900; 40],
            vec![5000, 10, 10, 10],
            vec![10, 10, 5000, 10, 10],
            vec![300; 500],
        ] {
            let page = page_of(&sizes);
            let whole = page.packed_tree();
            let (head, pieces) = page.cut_frames(2048);
            assert_eq!(
                merge_all(&head, &pieces),
                resolved(&whole),
                "sizes {:?}",
                &sizes[..3]
            );
            assert_pieces_fit(&pieces, 2048);
            assert!(
                head.to_string().len() <= 2048
                    || pieces.is_empty()
                    || head.to_string().len() < whole.to_string().len()
            );
        }
    }

    /// W1 shares statics through a frame-local `"p"`, and the client deletes
    /// the table once it rendered the frame: a piece naming statics only the
    /// head's table holds is a piece the client cannot build. Every frame of
    /// a cut join carries the statics its own parts name, and only those.
    #[test]
    fn gh1002_every_piece_carries_its_own_statics() {
        // 200 slots: their 200 emptied keys alone are about 1.7 KB, so every
        // limit here leaves the head room for slots of its own.
        let page = nested_page(200, 40);
        let whole = page.packed_tree();
        for limit in [4096, 8192] {
            let (head, pieces) = page.cut_frames(limit);
            assert!(!pieces.is_empty(), "limit {limit}: the page is cut");
            assert!(head.to_string().len() <= limit, "limit {limit}: head");
            assert_pieces_fit(&pieces, limit);
            for f in std::iter::once(&head).chain(&pieces) {
                let used = f["p"].as_object().map_or(0, Map::len);
                // The three components and the list entries' statics.
                assert!(used <= 4, "a frame names at most four statics");
            }
            let tree = merge_all(&head, &pieces);
            assert_eq!(tree, resolved(&whole), "limit {limit}");
            assert_eq!(html_of(&tree), page.rendered_body(), "limit {limit}");
        }
    }

    #[test]
    fn gh1002_a_tree_that_fits_is_not_touched() {
        let page = page_of(&[10, 20, 30]);
        let (head, pieces) = page.cut_frames(96 * 1024);
        assert_eq!(head.to_string(), page.packed_tree().to_string());
        assert!(pieces.is_empty());
    }

    #[test]
    fn gh1002_the_head_is_a_prefix_and_never_over_the_limit() {
        for page in [page_of(&[300; 100]), nested_page(100, 300)] {
            let (head, _) = page.cut_frames(4096);
            assert!(head.to_string().len() <= 4096, "{}", head.to_string().len());
            // A prefix of the root list: entries 0..kc and nothing else.
            let kc = head["0"]["k"]["kc"].as_u64().expect("kc") as usize;
            assert!(kc < 100, "something was cut");
            assert_eq!(entries(&head), (0..kc).collect::<Vec<_>>(), "no hole");
        }
    }

    /// The inline budget and the join limit apply only above the cut
    /// threshold. One byte below it a page is served and joined whole -- a
    /// display's screen alone is larger than the 48 KB budget.
    #[test]
    fn gh1002_only_a_page_above_the_threshold_is_cut() {
        let page = |bytes: usize| Materialized {
            statics: vec!["<main>".into(), String::new(), "</main>".into()],
            slots: vec![
                ("a".into(), "x".repeat(bytes / 2).into()),
                ("b".into(), "y".repeat(bytes - bytes / 2).into()),
            ],
            title: "t".into(),
        };
        let below = page(CUT_ABOVE_BYTES);
        assert!(!below.is_large());
        assert_eq!(below.page_body(48 * 1024), below.rendered_body());
        let (head, pieces) = below.join_frames(96 * 1024);
        assert_eq!(head, below.packed_tree());
        assert!(pieces.is_empty());

        let above = page(CUT_ABOVE_BYTES + 1);
        assert!(above.is_large());
        assert_ne!(above.page_body(48 * 1024), above.rendered_body());
        assert!(!above.join_frames(96 * 1024).1.is_empty());
    }

    /// Slot HTML counts the markup of nested parts, statics and all.
    #[test]
    fn gh1002_the_html_length_of_a_part_is_its_markup() {
        let page = nested_page(7, 5);
        for (_, part) in &page.slots {
            assert_eq!(part.html_len(), part.html().len());
        }
        let cut = page.rendered_body_cut(usize::MAX);
        assert_eq!(cut, page.rendered_body());
    }

    #[test]
    fn gh1002_json_length_counts_the_escapes() {
        for s in ["", "abc", "a\"b", "x\\y", "line\nnext", "\u{1}", "ü"] {
            assert_eq!(
                json_str_len(s),
                meclaw_core::serde_json::to_string(s).expect("json").len(),
                "{s:?}"
            );
        }
    }

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

    fn list(items: &[(&str, &Part)]) -> Part {
        Part::List(Arc::new(List {
            items: items
                .iter()
                .map(|(id, p)| (id.to_string(), (*p).clone()))
                .collect(),
        }))
    }

    /// GH #1009: a list is diffed by object id, never by position, and never
    /// sent whole because it grew or shrank.
    #[test]
    fn a_list_diff_moves_what_the_viewer_holds_and_sends_only_what_is_new() {
        let p = |v: &str| node(&["<b>", "</b>"], vec![v.into()]);
        let (a, b, c, d) = (p("a"), p("b"), p("c"), p("d"));
        let old = list(&[("a", &a), ("b", &b), ("c", &c)]);
        let mut shared = SharedStatics::default();

        // A reorder with one changed value: moves, no statics.
        let new = list(&[("c", &c), ("b", &b), ("a", &p("A"))]);
        assert_eq!(
            diff_value(&old, &new, &mut shared),
            Some(json!({"k": {"0": 2, "2": [0, {"0": {"0": "A"}}], "kc": 3, "km": 1}}))
        );
        // Grown by one in the middle: the entries behind it move, the new one
        // comes in full.
        let new = list(&[("a", &a), ("d", &d), ("b", &b), ("c", &c)]);
        assert_eq!(
            diff_value(&old, &new, &mut shared),
            Some(
                json!({"k": {"1": {"0": {"s": 0, "0": "d", "r": 1}}, "2": 1, "3": 2,
                              "kc": 4, "km": 1}})
            )
        );
        // Shrunk at the end: the count alone.
        let new = list(&[("a", &a), ("b", &b)]);
        assert_eq!(
            diff_value(&old, &new, &mut shared),
            Some(json!({"k": {"kc": 2}}))
        );
        assert_eq!(diff_value(&old, &old.clone(), &mut shared), None);

        // In full: one statics pair for every entry, the markup unchanged.
        let mut shared = SharedStatics::default();
        let full = full_value(&old, &mut shared);
        let mut frame = Map::new();
        shared.attach(&mut frame);
        assert_eq!(wire_html(&full, &frame["p"]), "<b>a</b><b>b</b><b>c</b>");
        assert_eq!(old.html(), "<b>a</b><b>b</b><b>c</b>");
    }
}
