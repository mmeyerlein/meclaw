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
    /// `{{prop}}` — escaped substitution.
    Prop(String),
    /// `{{&prop}}` — raw substitution, honoured only for `"html"` props.
    Raw(String),
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
    let (pieces, rest) = parse_until_end(src)?;
    if !rest.is_empty() {
        return Err(TemplateError(
            "unexpected {{/if}} without a matching {{#if …}}".to_string(),
        ));
    }
    Ok(pieces)
}

/// Parse pieces until end of input or an unconsumed `{{/if}}`.
///
/// Returns the pieces and whatever remains after a closing tag, so the `#if`
/// arm can pick up where its body ended.
fn parse_until_end(src: &str) -> Result<(Vec<Piece>, &str), TemplateError> {
    let mut out = Vec::new();
    let mut rest = src;

    loop {
        let Some(open) = rest.find("{{") else {
            if !rest.is_empty() {
                out.push(Piece::Text(rest.to_string()));
            }
            return Ok((out, ""));
        };
        if open > 0 {
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
            return Ok((out, tail));
        } else if let Some(prop) = tag.strip_prefix("#if ") {
            let prop = prop.trim();
            check_name(prop)?;
            let (body, after_body) = parse_until_end(tail)?;
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
            out.push(Piece::Children);
        } else if let Some(name) = tag.strip_prefix('&') {
            let name = name.trim();
            check_name(name)?;
            out.push(Piece::Raw(name.to_string()));
        } else {
            check_name(tag)?;
            out.push(Piece::Prop(tag.to_string()));
        }
        rest = tail;
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
    for piece in pieces {
        match piece {
            Piece::Text(t) => out.push_str(t),
            Piece::Prop(name) => out.push_str(&escape(&prop_text(props, name))),
            Piece::Raw(name) => {
                let text = prop_text(props, name);
                if is_html_prop(schema, name) {
                    out.push_str(&text);
                } else {
                    // Undeclared: escape. Emitting markup a schema never
                    // promised was markup is the worse failure.
                    out.push_str(&escape(&text));
                }
            }
            Piece::Children => {
                for child in child_ids(conn, id)? {
                    out.push_str(&render_at(conn, &child, depth + 1)?);
                }
            }
            Piece::If { prop, body } => {
                if prop_truthy(props, prop) {
                    render_pieces(conn, body, props, schema, id, depth, out)?;
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
                Piece::Prop("x".into()),
                Piece::Text("b".into()),
                Piece::Raw("y".into()),
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
}
