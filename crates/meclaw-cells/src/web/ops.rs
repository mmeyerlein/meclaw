//! W8 (GH #380): the `object.*` operations.
//!
//! Object CRUD is the hot path (R-W8-4c), so each op is one statement against
//! an indexed table and nothing else. What makes them safe rather than merely
//! fast is that each one validates against the component's own `prop_schema`
//! before it writes: props reach this cell from models and from browsers, and a
//! prop nobody declared is a typo that would otherwise render as silence.
//!
//! # Which slot a write touched
//!
//! Every write answers with the **root-child ancestor** of the object it
//! changed, per page. That is the diff granularity of [`crate::web::render`]:
//! the caller re-renders exactly that slot and pushes exactly that. Finding it
//! is a walk up `parent`, bounded like the render is, because the same cycle
//! that would hang a render would hang this walk.

use crate::web::output::OpOutcome;
use crate::web::render::parse_template;
use meclaw_core::serde_json::{Value, json};
use rusqlite::Connection;

/// Route segments this cell keeps for itself.
///
/// `live` is the phoenix client's: it appends exactly `/websocket` to the socket
/// URL, so a page at `/live` would shadow the transport. `@…` is ours — the
/// bundles live under `/@client/`. Refusing both at `page.set` is what makes it
/// impossible to shadow them **by construction** rather than by the router
/// happening to match in a lucky order.
const RESERVED_SEGMENTS: &[&str] = &["live"];

/// How far up the tree the ancestor walk goes before giving up. Same bound as
/// the renderer, for the same reason.
const MAX_DEPTH: usize = 64;

/// The closed set of class names that **are** the glass material.
///
/// They are the vocabulary `templates/web/seed/assets.jsonl` defines, and the
/// only way a component template can ask for the material — every other rule in
/// that sheet spends a custom property, so there is nothing else to check
/// against. A component that reaches for `backdrop-filter` in an inline style
/// instead is outside what this rule can see, and saying so plainly is better
/// than a check that pretends to cover CSS in general.
pub const GLASS_CLASSES: &[&str] = &["glass", "glass--thin", "glass--thick"];

/// Where a write landed: the pages that show it, and the root-child slot on
/// each that has to be re-rendered.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Touched {
    /// `(route, root_child_id)` — one per page showing the changed object.
    pub slots: Vec<(String, String)>,
    /// Set when the change was structural (a create, a move or a delete), so
    /// the page's slot list itself moved and a whole re-materialise is due
    /// rather than a single-slot patch.
    pub structural: bool,
}

/// Apply one op.
///
/// Returns the outcome and what it touched. A refusal touches nothing.
pub fn apply(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let Some(op) = args.get("op").and_then(Value::as_str) else {
        return (
            OpOutcome::refused("unknown", "invalid_input", "missing \"op\""),
            Touched::default(),
        );
    };

    match op {
        "object.create" => object_create(conn, args),
        "object.update" => object_update(conn, args),
        "object.move" => object_move(conn, args),
        "object.delete" => object_delete(conn, args),
        "component.define" => component_define(conn, args),
        "page.set" => page_set(conn, args),
        "query" => (query(conn, args), Touched::default()),
        other => (
            OpOutcome::refused(
                other,
                "unknown_op",
                format!(
                    "no such op {other:?} — this cell has object.create, object.update, \
                     object.move, object.delete, component.define, page.set and query"
                ),
            ),
            Touched::default(),
        ),
    }
}

/// A required string argument.
fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, OpOutcome> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| {
        OpOutcome::refused(
            args.get("op").and_then(Value::as_str).unwrap_or("unknown"),
            "invalid_input",
            format!("{key:?} is required and must be a string"),
        )
    })
}

/// The declared props of a component, plus whether it exists at all.
fn prop_schema_of(conn: &Connection, component: &str) -> Result<Value, OpOutcome> {
    conn.query_row(
        "SELECT prop_schema FROM components WHERE name = ?1",
        [component],
        |r| r.get::<_, String>(0),
    )
    .map(|s| meclaw_core::serde_json::from_str(&s).unwrap_or_else(|_| json!({})))
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => OpOutcome::refused(
            "object.create",
            "unknown_component",
            format!("no component {component:?} — define it before using it"),
        ),
        other => OpOutcome::refused("object.create", "invalid_input", other.to_string()),
    })
}

/// Check a props bag against a component's declared schema.
///
/// An undeclared prop is refused rather than stored. A template can only render
/// what it names, so an undeclared prop is invisible — and silently accepting it
/// would let a model believe it had set something.
fn check_props(operation: &str, schema: &Value, props: &Value) -> Result<(), OpOutcome> {
    let Some(obj) = props.as_object() else {
        return Err(OpOutcome::refused(
            operation,
            "invalid_input",
            "\"props\" must be a JSON object",
        ));
    };
    let Some(declared) = schema.as_object() else {
        return Ok(());
    };
    for key in obj.keys() {
        if !declared.contains_key(key) {
            let known: Vec<&str> = declared.keys().map(String::as_str).collect();
            return Err(OpOutcome::refused(
                operation,
                "invalid_input",
                format!(
                    "prop {key:?} is not declared by this component — it declares: {}",
                    if known.is_empty() {
                        "nothing".to_string()
                    } else {
                        known.join(", ")
                    }
                ),
            ));
        }
    }
    Ok(())
}

/// The class names a template writes, with the template's own tags removed
/// first.
///
/// Removing them is what makes the check honest. A class attribute may hold a
/// conditional — `class="text{{#if quiet}} text--secondary{{/if}}"` — so
/// splitting the raw source on whitespace would yield tokens like `text{{#if`
/// and would walk straight past a `class="{{#if x}}glass{{/if}}"`. Every `{{…}}`
/// becomes a space, and what is left is class names.
///
/// Comparing whole names rather than searching for a substring is the other
/// half: `--glass-tint` in an inline style is a custom property, and a class
/// called `glassy` is somebody else's class.
fn class_tokens(template: &str) -> Vec<String> {
    let mut plain = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        plain.push_str(&rest[..open]);
        plain.push(' ');
        match rest[open + 2..].find("}}") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => {
                // An unclosed tag is `parse_template`'s refusal to make, not
                // this one's. Stop scanning rather than guess where it ended.
                rest = "";
                break;
            }
        }
    }
    plain.push_str(rest);

    let mut out = Vec::new();
    let mut hay = plain.as_str();
    while let Some(at) = hay.find("class=") {
        let after = &hay[at + "class=".len()..];
        let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            hay = after;
            continue;
        };
        let body = &after[1..];
        let Some(end) = body.find(quote) else {
            break;
        };
        out.extend(body[..end].split_whitespace().map(str::to_string));
        hay = &body[end + 1..];
    }
    out
}

/// Whether a template asks for the glass material.
pub fn writes_glass(template: &str) -> bool {
    class_tokens(template)
        .iter()
        .any(|c| GLASS_CLASSES.contains(&c.as_str()))
}

/// The first adoption rule, as a check: **glass lives on the navigation layer.**
///
/// One function, two callers — `component.define` and the seed reader
/// ([`crate::web::seed`]), which never goes through an op at all. A rule
/// enforced on one of those two paths is a rule a shipped template walks past,
/// and the shipped template is exactly where a designed component set lands.
///
/// Returns the message rather than an [`OpOutcome`], because the seed path
/// reports strings and the op path wraps them.
pub fn check_glass_layer(name: &str, template: &str, layer: &str) -> Result<(), String> {
    if layer == "navigation" {
        return Ok(());
    }
    let Some(class) = class_tokens(template)
        .into_iter()
        .find(|c| GLASS_CLASSES.contains(&c.as_str()))
    else {
        return Ok(());
    };
    Err(format!(
        "component {name:?} declares layer {layer:?} and wears the class {class:?} — \
         glass lives on the navigation layer only. Either give it layer \"navigation\", \
         or drop the material and let it sit on the pane above it"
    ))
}

/// Whether a component is navigation glass.
///
/// Both halves matter: a navigation-layer component that is not glass — a bare
/// toolbar row — nests as freely as any content component.
fn is_navigation_glass(layer: &str, template: &str) -> bool {
    layer == "navigation" && writes_glass(template)
}

/// The second adoption rule, as a check: **glass never sits on glass.**
///
/// Asked of the edge a create makes, which is the one place it can be answered
/// with one indexed read (R-W8-4c) — and, in the common case, with none: a
/// parent that is not glass ends the question before the child is looked up.
///
/// What it does **not** claim: a pane moved under another pane by
/// `object.move`, or nested through a content component in between, is not
/// caught. The rule this cell enforces is about the edge in front of it, and a
/// check that walked every ancestor on every create would put a bounded tree
/// walk on the hot path in exchange for a promise it still could not keep
/// against moves.
fn check_glass_on_glass(
    conn: &Connection,
    parent: &str,
    child_component: &str,
) -> Result<(), OpOutcome> {
    let parent_row: Result<(String, String, String), _> = conn.query_row(
        "SELECT c.name, c.template, c.layer
           FROM objects o JOIN components c ON c.name = o.component
          WHERE o.id = ?1",
        [parent],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    );
    // No such parent, or a parent whose component is gone: not this rule's
    // business, and the create answers for itself either way.
    let Ok((parent_component, parent_template, parent_layer)) = parent_row else {
        return Ok(());
    };
    if !is_navigation_glass(&parent_layer, &parent_template) {
        return Ok(());
    }

    let child: Result<(String, String), _> = conn.query_row(
        "SELECT template, layer FROM components WHERE name = ?1",
        [child_component],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    );
    let Ok((child_template, child_layer)) = child else {
        return Ok(());
    };
    if !is_navigation_glass(&child_layer, &child_template) {
        return Ok(());
    }

    Err(OpOutcome::refused(
        "object.create",
        "invalid_input",
        format!(
            "{child_component:?} is glass and so is its parent {parent_component:?} \
             (object {parent:?}) — glass never sits on glass. Put a content-layer \
             component between them, or give this one a place of its own"
        ),
    ))
}

/// The root-child ancestor of `id` on every page that shows it.
fn touched_by(conn: &Connection, id: &str) -> Touched {
    // Walk to the top of this object's tree, remembering the last step before
    // the root — that is the slot.
    let mut chain: Vec<String> = vec![id.to_string()];
    let mut current = id.to_string();
    for _ in 0..MAX_DEPTH {
        let parent: Option<String> = conn
            .query_row(
                "SELECT parent FROM objects WHERE id = ?1",
                [&current],
                |r| r.get::<_, Option<String>>(0),
            )
            .ok()
            .flatten();
        match parent {
            Some(p) => {
                chain.push(p.clone());
                current = p;
            }
            None => break,
        }
    }

    let root = chain.last().cloned().unwrap_or_default();
    // The slot is the child of the root on the path down to `id`; when `id` IS
    // the root there is no slot and the whole page is affected.
    let slot = if chain.len() >= 2 {
        chain[chain.len() - 2].clone()
    } else {
        return Touched {
            slots: Vec::new(),
            structural: true,
        };
    };

    let mut stmt = match conn.prepare("SELECT route FROM pages WHERE root = ?1") {
        Ok(s) => s,
        Err(_) => return Touched::default(),
    };
    let routes: Vec<String> = stmt
        .query_map([&root], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();

    Touched {
        slots: routes.into_iter().map(|r| (r, slot.clone())).collect(),
        structural: false,
    }
}

fn object_create(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let component = match arg_str(args, "component") {
        Ok(c) => c,
        Err(e) => return (e, Touched::default()),
    };
    let schema = match prop_schema_of(conn, component) {
        Ok(s) => s,
        Err(e) => return (e, Touched::default()),
    };
    let props = args.get("props").cloned().unwrap_or_else(|| json!({}));
    if let Err(e) = check_props("object.create", &schema, &props) {
        return (e, Touched::default());
    }

    // An id the caller chose, or one derived from the row count. A caller that
    // wants to patch its object later has to name it, which is why `id` is
    // offered rather than always generated.
    let id = match args.get("id").and_then(Value::as_str) {
        Some(i) => i.to_string(),
        None => {
            let n: i64 = conn
                .query_row("SELECT count(*) FROM objects", [], |r| r.get(0))
                .unwrap_or(0);
            format!("obj-{}", n + 1)
        }
    };
    let parent = args.get("parent").and_then(Value::as_str);
    let ord = args.get("ord").and_then(Value::as_i64).unwrap_or(0);

    // The second adoption rule, checked where the edge is made.
    if let Some(parent) = parent
        && let Err(e) = check_glass_on_glass(conn, parent, component)
    {
        return (e, Touched::default());
    }

    let res = conn.execute(
        "INSERT INTO objects (id, parent, component, ord, props) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, parent, component, ord, props.to_string()],
    );
    match res {
        Ok(n) => {
            let mut touched = touched_by(conn, &id);
            // A create changes the page's shape, not just one slot's content.
            touched.structural = true;
            (OpOutcome::wrote("object.create", n as i64), touched)
        }
        Err(e) => (
            OpOutcome::refused("object.create", "invalid_input", e.to_string()),
            Touched::default(),
        ),
    }
}

fn object_update(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let id = match arg_str(args, "id") {
        Ok(i) => i,
        Err(e) => return (e, Touched::default()),
    };
    let current: Result<(String, String), _> = conn.query_row(
        "SELECT component, props FROM objects WHERE id = ?1",
        [id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    );
    let (component, existing) = match current {
        Ok(v) => v,
        Err(_) => {
            return (
                OpOutcome::refused(
                    "object.update",
                    "unknown_object",
                    format!("no object {id:?}"),
                ),
                Touched::default(),
            );
        }
    };
    let schema = match prop_schema_of(conn, &component) {
        Ok(s) => s,
        Err(mut e) => {
            e.operation = "object.update".to_string();
            return (e, Touched::default());
        }
    };
    let patch = args.get("props").cloned().unwrap_or_else(|| json!({}));
    if let Err(e) = check_props("object.update", &schema, &patch) {
        return (e, Touched::default());
    }

    // Merge per key, so a patch names only what it changes. Replacing the whole
    // bag would make every partial update a read-modify-write for the caller.
    let mut merged: Value =
        meclaw_core::serde_json::from_str(&existing).unwrap_or_else(|_| json!({}));
    if let (Some(target), Some(patch_obj)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in patch_obj {
            target.insert(k.clone(), v.clone());
        }
    }

    match conn.execute(
        "UPDATE objects SET props = ?1 WHERE id = ?2",
        rusqlite::params![merged.to_string(), id],
    ) {
        Ok(n) => (
            OpOutcome::wrote("object.update", n as i64),
            touched_by(conn, id),
        ),
        Err(e) => (
            OpOutcome::refused("object.update", "invalid_input", e.to_string()),
            Touched::default(),
        ),
    }
}

/// Move an object: a new parent, a new `ord`, or both.
///
/// **`ord` is a sort key, not a list index.** A move does not renumber
/// siblings, and two siblings may share an `ord` — the render then breaks the
/// tie by `id`, deterministically. The alternative, shifting everyone else up
/// or down, would make one caller's patch silently rewrite rows it never named,
/// and in a bundle it would make the result depend on the order the legs
/// happened to run in. A caller that wants a specific arrangement states it:
/// gaps (10, 20, 30) leave room to insert without touching anything else.
fn object_move(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let id = match arg_str(args, "id") {
        Ok(i) => i,
        Err(e) => return (e, Touched::default()),
    };
    let exists: i64 = conn
        .query_row("SELECT count(*) FROM objects WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    if exists == 0 {
        return (
            OpOutcome::refused("object.move", "unknown_object", format!("no object {id:?}")),
            Touched::default(),
        );
    }

    // Where it was, so both the old and the new page get their patch.
    let before = touched_by(conn, id);

    let mut sets: Vec<&str> = Vec::new();
    let mut params: Vec<rusqlite::types::Value> = Vec::new();
    if args.get("parent").is_some() {
        sets.push("parent = ?");
        params.push(match args.get("parent").and_then(Value::as_str) {
            Some(p) => rusqlite::types::Value::Text(p.to_string()),
            None => rusqlite::types::Value::Null,
        });
    }
    if let Some(ord) = args.get("ord").and_then(Value::as_i64) {
        sets.push("ord = ?");
        params.push(rusqlite::types::Value::Integer(ord));
    }
    if sets.is_empty() {
        return (
            OpOutcome::refused(
                "object.move",
                "invalid_input",
                "a move names \"parent\", \"ord\" or both",
            ),
            Touched::default(),
        );
    }
    params.push(rusqlite::types::Value::Text(id.to_string()));
    let sql = format!("UPDATE objects SET {} WHERE id = ?", sets.join(", "));
    let bound: Vec<&dyn rusqlite::ToSql> =
        params.iter().map(|v| v as &dyn rusqlite::ToSql).collect();

    match conn.execute(&sql, bound.as_slice()) {
        Ok(n) => {
            let mut after = touched_by(conn, id);
            // A move is structural on both ends, and the slot lists differ.
            for slot in before.slots {
                if !after.slots.contains(&slot) {
                    after.slots.push(slot);
                }
            }
            after.structural = true;
            (OpOutcome::wrote("object.move", n as i64), after)
        }
        Err(e) => (
            OpOutcome::refused("object.move", "invalid_input", e.to_string()),
            Touched::default(),
        ),
    }
}

fn object_delete(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let id = match arg_str(args, "id") {
        Ok(i) => i,
        Err(e) => return (e, Touched::default()),
    };

    // Children are NOT re-parented and NOT cascaded. Either would be this
    // cell guessing what a caller meant about content it cannot see; deleting
    // leaf-first is unambiguous, and the refusal names the children so the
    // caller knows exactly what to do.
    let mut stmt = match conn.prepare("SELECT id FROM objects WHERE parent = ?1 ORDER BY ord, id") {
        Ok(s) => s,
        Err(e) => {
            return (
                OpOutcome::refused("object.delete", "invalid_input", e.to_string()),
                Touched::default(),
            );
        }
    };
    let children: Vec<String> = stmt
        .query_map([id], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default();
    if !children.is_empty() {
        return (
            OpOutcome::refused(
                "object.delete",
                "invalid_input",
                format!(
                    "object {id:?} still has children ({}) — delete leaf-first; \
                     this cell does not re-parent or cascade, because either would be \
                     guessing what you meant",
                    children.join(", ")
                ),
            ),
            Touched::default(),
        );
    }
    drop(stmt);

    // Read the slot BEFORE the row is gone: afterwards the walk has nothing
    // to climb.
    let touched = touched_by(conn, id);

    match conn.execute("DELETE FROM objects WHERE id = ?1", [id]) {
        Ok(0) => (
            OpOutcome::refused(
                "object.delete",
                "unknown_object",
                format!("no object {id:?}"),
            ),
            Touched::default(),
        ),
        Ok(n) => {
            let mut t = touched;
            t.structural = true;
            (OpOutcome::wrote("object.delete", n as i64), t)
        }
        Err(e) => (
            OpOutcome::refused("object.delete", "invalid_input", e.to_string()),
            Touched::default(),
        ),
    }
}

/// Define or redefine a component.
///
/// **The template is parsed here**, and that is the whole point of doing it at
/// definition time: an unknown `{{…}}` is answered to whoever wrote it, at the
/// moment they write it. Parsing at render time instead would surface the same
/// mistake as a blank area on a page, to somebody who never saw the template.
///
/// This is also the one-parser rule in practice: the same
/// [`crate::web::render::parse_template`] the renderer uses. A second, laxer
/// parser here would let a component into the library that the renderer cannot
/// draw.
fn component_define(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let name = match arg_str(args, "name") {
        Ok(n) => n,
        Err(e) => return (e, Touched::default()),
    };
    let template = match arg_str(args, "template") {
        Ok(t) => t,
        Err(e) => return (e, Touched::default()),
    };
    if let Err(e) = parse_template(template) {
        return refuse_define("component.define", "invalid_input", e.0);
    }

    let prop_schema = args
        .get("prop_schema")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !prop_schema.is_object() {
        return refuse_define(
            "component.define",
            "invalid_input",
            "\"prop_schema\" must be a JSON object",
        );
    }

    let editable = args.get("editable").cloned().unwrap_or_else(|| json!([]));
    if !editable.is_array() {
        return refuse_define(
            "component.define",
            "invalid_input",
            "\"editable\" must be an array of prop names",
        );
    }
    // An `editable` prop the schema does not declare could never be written by
    // a browser anyway — the write would be refused as undeclared — so naming
    // one is a mistake worth reporting rather than a harmless extra.
    if let (Some(list), Some(declared)) = (editable.as_array(), prop_schema.as_object()) {
        for item in list {
            let Some(prop) = item.as_str() else {
                return refuse_define(
                    "component.define",
                    "invalid_input",
                    "\"editable\" holds prop names, which are strings",
                );
            };
            if !declared.contains_key(prop) {
                return refuse_define(
                    "component.define",
                    "invalid_input",
                    format!(
                        "editable names {prop:?}, which this component does not declare \
                         in prop_schema"
                    ),
                );
            }
        }
    }

    let layer = args
        .get("layer")
        .and_then(Value::as_str)
        .unwrap_or("content");
    if !matches!(layer, "navigation" | "content") {
        return refuse_define(
            "component.define",
            "invalid_input",
            format!("layer is \"navigation\" or \"content\", got {layer:?}"),
        );
    }
    // The first adoption rule. The seed reader calls the same function, because
    // a component set ships as seed data far more often than it is defined by
    // message.
    if let Err(why) = check_glass_layer(name, template, layer) {
        return refuse_define("component.define", "invalid_input", why);
    }

    let res = conn.execute(
        "INSERT INTO components (name, template, prop_schema, editable, layer)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(name) DO UPDATE SET
             template = excluded.template,
             prop_schema = excluded.prop_schema,
             editable = excluded.editable,
             layer = excluded.layer",
        rusqlite::params![
            name,
            template,
            prop_schema.to_string(),
            editable.to_string(),
            layer
        ],
    );
    match res {
        Ok(n) => {
            // Redefining a component changes how every object using it renders,
            // and this cell does not track which objects those are — so every
            // route is re-materialised. A component definition is rare and a
            // page count is small; guessing narrower here would risk a page
            // that quietly kept drawing the old template.
            (
                OpOutcome::wrote("component.define", n as i64),
                Touched {
                    slots: all_routes(conn),
                    structural: true,
                },
            )
        }
        Err(e) => refuse_define("component.define", "invalid_input", e.to_string()),
    }
}

/// A refusal from `component.define`, which touches nothing.
fn refuse_define(operation: &str, code: &str, text: impl Into<String>) -> (OpOutcome, Touched) {
    (
        OpOutcome::refused(operation, code, text),
        Touched::default(),
    )
}

/// Every route this cell serves, paired with its own root.
///
/// Used when a change is too broad to attribute to one slot.
fn all_routes(conn: &Connection) -> Vec<(String, String)> {
    let Ok(mut stmt) = conn.prepare("SELECT route, root FROM pages ORDER BY route") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Check a route against the grammar this cell serves.
///
/// One plain segment chain: `/`, or `/a`, or `/a/b`, with segments of
/// `[a-z0-9-]`. No `@`, no reserved segment, no traversal, no query string —
/// a route is a name, not a URL.
fn check_route(route: &str) -> Result<(), String> {
    if route == "/" {
        return Ok(());
    }
    if !route.starts_with('/') {
        return Err(format!("a route starts with \"/\", got {route:?}"));
    }
    if route.ends_with('/') {
        return Err(format!(
            "a route does not end with \"/\" (that would make {route:?} and its \
             trimmed form two names for one page)"
        ));
    }
    for seg in route.trim_start_matches('/').split('/') {
        if seg.is_empty() {
            return Err(format!("{route:?} has an empty segment"));
        }
        if RESERVED_SEGMENTS.contains(&seg) {
            return Err(format!(
                "{seg:?} is reserved — it is the transport\'s, and a page there would \
                 shadow the websocket"
            ));
        }
        if seg.starts_with('@') {
            return Err(format!(
                "{seg:?} starts with \"@\", which is reserved for the cell\'s own files"
            ));
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(format!(
                "segment {seg:?} is not a plain name — routes use [a-z0-9-]"
            ));
        }
    }
    Ok(())
}

/// Point a route at a root object.
///
/// **The `pages` table is the only route source this cell has.** There is no
/// `cell.surface` key any more — it was removed with the retired `/surface/*`
/// path it declared (GH #383), and the `CellHeader` now refuses it as an
/// unknown key. Two grammars for one thing was the spec risk this design was
/// asked to discharge; it is discharged twice over — one of the two grammars is
/// gone, and there is no code here that reads it.
fn page_set(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let route = match arg_str(args, "route") {
        Ok(r) => r,
        Err(e) => return (e, Touched::default()),
    };
    if let Err(why) = check_route(route) {
        return (
            OpOutcome::refused("page.set", "invalid_input", why),
            Touched::default(),
        );
    }
    let root = match arg_str(args, "root") {
        Ok(r) => r,
        Err(e) => return (e, Touched::default()),
    };
    let exists: i64 = conn
        .query_row("SELECT count(*) FROM objects WHERE id = ?1", [root], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    if exists == 0 {
        return (
            OpOutcome::refused(
                "page.set",
                "unknown_object",
                format!("no object {root:?} to use as this page\'s root"),
            ),
            Touched::default(),
        );
    }
    let title = args.get("title").and_then(Value::as_str).unwrap_or("");

    let res = conn.execute(
        "INSERT INTO pages (route, root, title) VALUES (?1, ?2, ?3)
         ON CONFLICT(route) DO UPDATE SET root = excluded.root, title = excluded.title",
        rusqlite::params![route, root, title],
    );
    match res {
        Ok(n) => (
            OpOutcome::wrote("page.set", n as i64),
            // A new page is structural for itself: the whole route has to be
            // materialised, and there is no previous slot list to patch.
            Touched {
                slots: vec![(route.to_string(), root.to_string())],
                structural: true,
            },
        ),
        Err(e) => (
            OpOutcome::refused("page.set", "invalid_input", e.to_string()),
            Touched::default(),
        ),
    }
}

/// Read object state back: one object by `id`, or a whole page by `route`.
fn query(conn: &Connection, args: &Value) -> OpOutcome {
    if let Some(id) = args.get("id").and_then(Value::as_str) {
        let row = conn.query_row(
            "SELECT id, parent, component, ord, props FROM objects WHERE id = ?1",
            [id],
            |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "parent": r.get::<_, Option<String>>(1)?,
                    "component": r.get::<_, String>(2)?,
                    "ord": r.get::<_, i64>(3)?,
                    "props": meclaw_core::serde_json::from_str::<Value>(&r.get::<_, String>(4)?)
                        .unwrap_or_else(|_| json!({})),
                }))
            },
        );
        return match row {
            Ok(v) => OpOutcome::read("query", json!({ "object": v })),
            Err(_) => OpOutcome::refused("query", "unknown_object", format!("no object {id:?}")),
        };
    }

    if let Some(route) = args.get("route").and_then(Value::as_str) {
        let root: Result<String, _> =
            conn.query_row("SELECT root FROM pages WHERE route = ?1", [route], |r| {
                r.get(0)
            });
        let Ok(root) = root else {
            return OpOutcome::refused(
                "query",
                "invalid_input",
                format!("no page declares the route {route:?}"),
            );
        };
        let mut stmt = match conn
            .prepare("SELECT id, parent, component, ord, props FROM objects ORDER BY parent, ord")
        {
            Ok(s) => s,
            Err(e) => return OpOutcome::refused("query", "invalid_input", e.to_string()),
        };
        let objects: Vec<Value> = stmt
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, String>(0)?,
                    "parent": r.get::<_, Option<String>>(1)?,
                    "component": r.get::<_, String>(2)?,
                    "ord": r.get::<_, i64>(3)?,
                    "props": meclaw_core::serde_json::from_str::<Value>(&r.get::<_, String>(4)?)
                        .unwrap_or_else(|_| json!({})),
                }))
            })
            .map(|rows| rows.filter_map(Result::ok).collect())
            .unwrap_or_default();
        return OpOutcome::read(
            "query",
            json!({ "route": route, "root": root, "objects": objects }),
        );
    }

    OpOutcome::refused(
        "query",
        "invalid_input",
        "a query names \"id\" or \"route\"",
    )
}

/// Write a browser-editable prop.
///
/// The `editable` declaration on the object's component is the authorisation,
/// and it is checked against the **component**, never against the message: a
/// browser says what it wants changed, and the component says what may be.
///
/// Returns the verdict the socket owes its client. Nothing is written on a
/// refusal — not a partial prop, not an audit row.
pub fn set_editable(
    conn: &Connection,
    id: &str,
    prop: &str,
    value: &Value,
) -> crate::web::cell::EventReply {
    use crate::web::cell::EventReply;

    let row: Result<(String, String, String), _> = conn.query_row(
        "SELECT o.component, o.props, c.editable
           FROM objects o JOIN components c ON c.name = o.component
          WHERE o.id = ?1",
        [id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    );
    let Ok((component, props_raw, editable_raw)) = row else {
        return EventReply::Error(format!("no object {id:?}"));
    };

    let editable: Vec<String> =
        meclaw_core::serde_json::from_str(&editable_raw).unwrap_or_default();
    if !editable.iter().any(|e| e == prop) {
        return EventReply::Error("not_editable".to_string());
    }

    let mut props: Value =
        meclaw_core::serde_json::from_str(&props_raw).unwrap_or_else(|_| json!({}));
    if let Some(obj) = props.as_object_mut() {
        obj.insert(prop.to_string(), value.clone());
    }

    match conn.execute(
        "UPDATE objects SET props = ?1 WHERE id = ?2",
        rusqlite::params![props.to_string(), id],
    ) {
        Ok(_) => EventReply::Ok,
        Err(e) => {
            tracing::error!(%component, %id, %prop, error = %e, "web: editable write failed");
            EventReply::Error("the write failed".to_string())
        }
    }
}
