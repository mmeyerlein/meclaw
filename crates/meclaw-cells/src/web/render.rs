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
//! That slot granularity is a deliberate v1 choice: a patch to any descendant
//! re-renders the slot of its root-child ancestor and pushes only that.
//! Finer granularity would mean tracking a slot per object and a much larger
//! static table; coarser would mean re-rendering the page on every keystroke.

use crate::web::markup::{Contexts, Slot, Sub, fits, int_text};
use meclaw_core::serde_json::{Map, Value, json};
use rusqlite::Connection;
use std::collections::BTreeMap;

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
    let mut stmt = conn.prepare("SELECT route FROM pages ORDER BY route")?;
    let routes = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut out = PageMap::new();
    for route in routes {
        match materialize(conn, &route) {
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
    /// One `(object_id, html)` per **direct child** of the page root, in order.
    pub slots: Vec<(String, String)>,
    /// The page title, for the shell's `<title>`.
    pub title: String,
}

impl Materialized {
    /// The LiveView packed tree: `{"s": statics, "0": slot0, "1": slot1, …}`.
    pub fn packed_tree(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "s".to_string(),
            Value::Array(self.statics.iter().map(|s| json!(s)).collect()),
        );
        for (i, (_id, html)) in self.slots.iter().enumerate() {
            m.insert(i.to_string(), json!(html));
        }
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
            if let Some((_, html)) = self.slots.get(i) {
                out.push_str(html);
            }
        }
        out
    }
}

/// One row of `objects`.
struct ObjectRow {
    component: String,
    props: Value,
}

fn load_object(conn: &Connection, id: &str) -> Result<ObjectRow, RenderError> {
    let row = conn
        .query_row(
            "SELECT component, props FROM objects WHERE id = ?1",
            [id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => RenderError::UnknownObject(id.to_string()),
            other => RenderError::Db(other.to_string()),
        })?;
    Ok(ObjectRow {
        component: row.0,
        // A props column that is not an object renders as no props at all
        // rather than failing the page: a malformed prop bag costs its own
        // values, not everybody else's.
        props: meclaw_core::serde_json::from_str(&row.1).unwrap_or_else(|_| json!({})),
    })
}

/// `(template, prop_schema)` of one component.
fn load_component(conn: &Connection, name: &str) -> Result<(String, Value), RenderError> {
    let row = conn
        .query_row(
            "SELECT template, prop_schema FROM components WHERE name = ?1",
            [name],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => RenderError::UnknownComponent(name.to_string()),
            other => RenderError::Db(other.to_string()),
        })?;
    Ok((
        row.0,
        meclaw_core::serde_json::from_str(&row.1).unwrap_or_else(|_| json!({})),
    ))
}

/// The ids of an object's children, in `ord` order.
fn child_ids(conn: &Connection, parent: &str) -> Result<Vec<String>, RenderError> {
    let mut stmt = conn.prepare("SELECT id FROM objects WHERE parent = ?1 ORDER BY ord, id")?;
    let ids = stmt
        .query_map([parent], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
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
    render_at(conn, id, 0)
}

fn render_at(conn: &Connection, id: &str, depth: usize) -> Result<String, RenderError> {
    if depth > MAX_DEPTH {
        return Err(RenderError::TooDeep { at: id.to_string() });
    }
    let obj = load_object(conn, id)?;
    let (template, schema) = load_component(conn, &obj.component)?;
    // A template stored in the database was accepted by `component.define`, so
    // a parse failure here means the row was written around that gate. Render
    // it as nothing rather than failing the page — and the definition path is
    // where the message belongs.
    let pieces = parse_template(&template).unwrap_or_default();
    let mut out = String::new();
    render_pieces(conn, &pieces, &obj.props, &schema, id, depth, &mut out)?;
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn render_pieces(
    conn: &Connection,
    pieces: &[Piece],
    props: &Value,
    schema: &Value,
    id: &str,
    depth: usize,
    out: &mut String,
) -> Result<(), RenderError> {
    render_pieces_with(pieces, props, schema, out, &mut |out| {
        for child in child_ids(conn, id)? {
            out.push_str(&render_at(conn, &child, depth + 1)?);
        }
        Ok(())
    })
}

/// The test-facing form of `render_pieces`: the same truthiness, so a test
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
            Piece::Raw { name, slot } => {
                if is_html_prop(schema, name) && slot.takes_markup() {
                    out.push_str(&prop_text(props, name));
                } else {
                    // Undeclared, or standing where markup cannot: escape and
                    // fit. Emitting markup a schema never promised was markup
                    // is the worse failure, and a raw value inside quotes is
                    // a way out of them.
                    out.push_str(&escape(&fitted_text(props, schema, name, slot)));
                }
            }
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
    let (root, title) = conn
        .query_row(
            "SELECT root, title FROM pages WHERE route = ?1",
            [route],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => RenderError::UnknownRoute(route.to_string()),
            other => RenderError::Db(other.to_string()),
        })?;

    let obj = load_object(conn, &root)?;
    let (template, schema) = load_component(conn, &obj.component)?;
    let pieces = parse_template(&template).unwrap_or_default();

    // Split the root's own template at `{{children}}`. Everything outside the
    // children marker is static for this page; each direct child becomes one
    // slot. `{{#if}}` around the children marker is not split into — the
    // conditional is evaluated and its result folded into the surrounding
    // static, because a slot that appears and disappears is not a slot.
    let mut statics: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut slots: Vec<(String, String)> = Vec::new();
    let mut split = false;

    for piece in &pieces {
        match piece {
            Piece::Children if !split => {
                split = true;
                statics.push(std::mem::take(&mut current));
                for child in child_ids(conn, &root)? {
                    let html = render_at(conn, &child, 1)?;
                    slots.push((child, html));
                }
                // One separator between each pair of adjacent slots, so the
                // list ends up n+1 long once the trailing piece is pushed
                // (GH #394). Two children used to produce two statics, which
                // put the closing tag *between* them and dropped every child
                // from the third on — `rendered_body` walks `statics`, and the
                // wire format wants n+1 statics for n dynamics.
                let separators = slots.len().saturating_sub(1);
                statics.resize(statics.len() + separators, String::new());
            }
            other => {
                render_pieces(
                    conn,
                    std::slice::from_ref(other),
                    &obj.props,
                    &schema,
                    &root,
                    0,
                    &mut current,
                )?;
            }
        }
    }
    statics.push(current);

    // A root with nothing in its children marker is entirely static: one piece,
    // no slots. That covers both a root with no `{{children}}` at all and one
    // whose marker has no children to show — n+1 statics for n = 0 is one, and
    // a tree with two statics and no dynamic is not a shape the client reads.
    // The served body is identical either way.
    if slots.is_empty() {
        statics = vec![statics.concat()];
    }

    Ok(Materialized {
        statics,
        slots,
        title,
    })
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
}
