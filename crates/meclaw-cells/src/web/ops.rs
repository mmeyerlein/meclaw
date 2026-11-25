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

/// GH #1006: the read-only op that reports every viewer's backlog. Answered
/// by the handler from the I/O half, never by [`apply`]: the counters are not
/// in the database.
pub const VIEWERS_OP: &str = "viewers";

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
        // Answered by the handler from the I/O half's viewer table before it
        // reaches the database (GH #1006); listed here so the op table names
        // every op a caller may send.
        VIEWERS_OP => (
            OpOutcome::refused(
                VIEWERS_OP,
                "unknown_op",
                "viewers is answered by the cell, not by its database",
            ),
            Touched::default(),
        ),
        other => (
            OpOutcome::refused(
                other,
                "unknown_op",
                format!(
                    "no such op {other:?} — this cell has object.create, object.update, \
                     object.move, object.delete, component.define, page.set, query and viewers"
                ),
            ),
            Touched::default(),
        ),
    }
}

/// One leg's result: its outcome, what it touched, and its own duration in
/// milliseconds.
pub type Applied = (OpOutcome, Touched, i64);

/// Apply the database legs of one message, in call order, in ONE transaction.
///
/// GH #1030: every leg used to be its own autocommit transaction on its own
/// trip to the blocking pool. Profiled on the gate host (release, 1 600 figure
/// updates on a pool of 1 600, `tests/gh1030_web_bundle_cost.rs`), one bundle
/// took 245 ms: 100 ms for the autocommitted legs — the same legs inside one
/// transaction took 23 ms — ≈ 110 ms for the trips, 34 ms for the re-render.
/// So the cost per leg was mostly the commit and the trip, both per leg.
///
/// The transaction is the write path, not an atomicity promise: every leg
/// keeps its own outcome, and a refused leg leaves its siblings written, as a
/// bundle always did. Two failures are the transaction's own, and both are
/// answered rather than hidden — SQLite ending it by itself (a full disk, an
/// I/O error) and a `COMMIT` that fails: every leg of the lost part that said
/// it wrote is turned into a refusal. Without a transaction (a refused
/// `BEGIN`), every leg commits on its own, as before.
///
/// GH #1264: an interrupt (`external_timeout_ms`, sent by the `DbConn`) ends
/// the bundle at the leg it hits. The transaction is rolled back, and every
/// leg that wrote, was running or had not run answers `query_timeout`; a read
/// that was done stays what it was.
pub fn apply_all(conn: &Connection, legs: &[Value]) -> Vec<Applied> {
    let mut out: Vec<Applied> = Vec::with_capacity(legs.len());
    let mut open = conn.execute_batch("BEGIN").is_ok();
    let mut first = 0;
    let mut cut = false;
    let committed = holding_interrupt(conn, || {
        for args in legs {
            let started = tokio::time::Instant::now();
            let (outcome, touched) = apply(conn, args);
            out.push((outcome, touched, started.elapsed().as_millis() as i64));
            if conn.is_interrupted() {
                cut = true;
                return Ok(());
            }
            if open && conn.is_autocommit() {
                unwritten(
                    &mut out[first..],
                    "invalid_input",
                    "the database rolled the bundle's transaction back",
                );
                open = conn.execute_batch("BEGIN").is_ok();
                first = out.len();
            }
        }
        if open {
            conn.execute_batch("COMMIT")
        } else {
            Ok(())
        }
    });
    let timed_out = cut || committed.as_ref().is_err_and(interrupted);
    if !timed_out && committed.is_ok() {
        return out;
    }
    if !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    if let (false, Err(e)) = (timed_out, &committed) {
        unwritten(
            &mut out[first..],
            "invalid_input",
            &format!("the bundle's transaction did not commit: {e}"),
        );
        return out;
    }
    if open {
        unwritten(&mut out[first..], "query_timeout", TIMED_OUT);
    }
    // What the interrupt hit has no answer of its own: a read may be cut short
    // and a refusal may be the interrupt's. Only a write that committed on its
    // own (no transaction) has landed.
    let landed = !open && out.last().is_some_and(|(o, ..)| o.rows_affected > 0);
    let at = out.len().saturating_sub(1);
    if cut
        && !landed
        && let (Some(args), Some((outcome, touched, _))) = (legs.get(at), out.last_mut())
    {
        *outcome = OpOutcome::refused(op_name(args), "query_timeout", TIMED_OUT);
        *touched = Touched::default();
    }
    for args in &legs[out.len()..] {
        out.push((
            OpOutcome::refused(op_name(args), "query_timeout", TIMED_OUT),
            Touched::default(),
            0,
        ));
    }
    out
}

/// The text of a `query_timeout` refusal.
const TIMED_OUT: &str = "the database call ran past external_timeout_ms and was interrupted — \
                         nothing of this leg is stored";

/// The op a leg names, for a refusal that is not the op's own.
fn op_name(args: &Value) -> &str {
    args.get("op").and_then(Value::as_str).unwrap_or("unknown")
}

/// Whether SQLite ended a statement because the connection was interrupted.
fn interrupted(e: &rusqlite::Error) -> bool {
    e.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted)
}

/// Run `f` so that an interrupt stays in force until it returns.
///
/// GH #1264: SQLite forgets an interrupt as soon as no statement is running. A
/// bundle and a re-render are many short statements, so the interrupt of the
/// operation timeout ended ONE of them, or none when it fell between two, and
/// the rest ran on without a clock. While the statement here is held
/// mid-way, every statement after the interrupt is refused, the `COMMIT`
/// included: whether the work landed is SQLite's verdict, not the clock's.
///
/// Call it inside a transaction, or let `f` begin one first (a `BEGIN` compiles
/// without the schema). Under autocommit a statement that does not
/// compile while this one is held leaves its read snapshot open, and the next
/// write after a foreign commit is refused with "database is locked".
fn holding_interrupt<R>(conn: &Connection, f: impl FnOnce() -> R) -> R {
    let mut stmt = conn.prepare_cached("SELECT 1 UNION ALL SELECT 2").ok();
    let mut rows = stmt.as_mut().and_then(|s| s.query([]).ok());
    if let Some(rows) = rows.as_mut() {
        let _ = rows.next();
    }
    f()
}

/// [`holding_interrupt`] for a read of many statements, in a transaction of its
/// own (see there), which also gives it one snapshot.
pub(crate) fn reading<R>(conn: &Connection, f: impl FnOnce() -> R) -> R {
    let open = conn.execute_batch("BEGIN").is_ok();
    let out = holding_interrupt(conn, f);
    if open {
        let _ = conn.execute_batch("ROLLBACK");
    }
    out
}

/// Turn every leg of `legs` that says it wrote into a refusal: its write is
/// gone with the transaction. A read (a payload) stays what it was.
fn unwritten(legs: &mut [Applied], code: &str, why: &str) {
    for (outcome, touched, _) in legs {
        if !outcome.is_error() && outcome.payload.is_null() {
            *outcome = OpOutcome::refused(&outcome.operation, code, why);
            *touched = Touched::default();
        }
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
    // GH #1001: `prepare_cached` — an update leg runs this once per leg, and
    // a fresh prepare per call was a share of the 400-leg bundle (lab).
    conn.prepare_cached("SELECT prop_schema FROM components WHERE name = ?1")
        .and_then(|mut stmt| stmt.query_row([component], |r| r.get::<_, String>(0)))
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

/// The definition rules on a component's markup and on what a browser may
/// write into it (GH #869): no event-handler attribute and no `javascript:`
/// in the template's static text, and no `editable` prop typed `"html"`.
///
/// One function, two callers, for the reason [`check_glass_layer`] gives:
/// `component.define` and the seed reader. A component that ships as seed data
/// never passes through an op, and a rule the shipped path walks past is no
/// rule. The checks themselves live in [`crate::web::markup`].
///
/// Those rules read the template's own text, so they hold only for a template
/// whose `{{…}}` all stand where that text cannot run on through them (review
/// I1 of #868/#869): the parser refuses the others, and is asked first — for
/// the seed too, which until then left the syntax to the shipped-template suite.
pub fn check_definition(
    name: &str,
    template: &str,
    prop_schema: &Value,
    editable: &Value,
) -> Result<(), String> {
    parse_template(template).map_err(|e| format!("component {name:?}: {e}"))?;
    crate::web::markup::check_template_markup(name, template)?;
    crate::web::markup::check_editable(name, prop_schema, editable)
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

/// The slot of `id` on every page that shows it: the child of the page's root
/// on the path down to `id`, or the root itself when `id` is that root.
fn touched_by(conn: &Connection, id: &str) -> Touched {
    // Walk to the top of this object's tree: every link may be a page's root.
    let mut chain: Vec<String> = vec![id.to_string()];
    let mut current = id.to_string();
    for _ in 0..MAX_DEPTH {
        let parent: Option<String> = conn
            .prepare_cached("SELECT parent FROM objects WHERE id = ?1")
            .and_then(|mut stmt| stmt.query_row([&current], |r| r.get::<_, Option<String>>(0)))
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

    // The top of a tree has no slot: the whole page is affected. GH #1264: if it
    // is a page's root at all. An object no page shows rendered every route
    // whole; a lookup that fails is no proof that nothing shows it.
    if chain.len() < 2 {
        let shown = conn
            .prepare_cached("SELECT 1 FROM pages WHERE root = ?1")
            .and_then(|mut stmt| stmt.exists([id]))
            .unwrap_or(true);
        return Touched {
            slots: Vec::new(),
            structural: shown,
        };
    }

    // GH #1264: `page.set` takes any object as a root, one with a parent too,
    // so the page that shows `id` is not only the one of the topmost ancestor.
    // Asking for that one alone left every nested page on its old picture. The
    // slot is the link below the page's root; a page rooted at `id` itself
    // names `id`, which the re-render answers with the whole route.
    let slots = all_routes(conn)
        .into_iter()
        .filter_map(|(route, root)| {
            let at = chain.iter().position(|link| *link == root)?;
            Some((route, chain[at.saturating_sub(1)].clone()))
        })
        .collect();
    Touched {
        slots,
        structural: false,
    }
}

/// The first `obj-N` past the row count that no object has, and that no object
/// or page still names: a new object takes over nothing left behind.
///
/// GH #1264: the count alone is a taken id after a delete, or when a caller
/// chose an `obj-N` itself, and it stays taken: every further create without
/// an id was refused.
fn free_id(conn: &Connection) -> rusqlite::Result<String> {
    conn.prepare_cached(
        "WITH RECURSIVE n(i) AS (
             SELECT count(*) + 1 FROM objects
             UNION ALL
             SELECT i + 1 FROM n
              WHERE EXISTS (SELECT 1 FROM objects WHERE id = 'obj-' || i)
                 OR EXISTS (SELECT 1 FROM objects WHERE parent = 'obj-' || i)
                 OR EXISTS (SELECT 1 FROM pages WHERE root = 'obj-' || i)
         )
         SELECT 'obj-' || max(i) FROM n",
    )
    .and_then(|mut stmt| stmt.query_row([], |r| r.get(0)))
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

    // An id the caller chose, or one nothing holds. A caller that wants to
    // patch its object later has to name it, which is why `id` is offered
    // rather than always generated.
    let id = match args.get("id").and_then(Value::as_str) {
        Some(i) => i.to_string(),
        None => match free_id(conn) {
            Ok(i) => i,
            Err(e) => {
                return (
                    OpOutcome::refused("object.create", "invalid_input", e.to_string()),
                    Touched::default(),
                );
            }
        },
    };
    let parent = args.get("parent").and_then(Value::as_str);
    let ord = args.get("ord").and_then(Value::as_i64).unwrap_or(0);

    // The second adoption rule, checked where the edge is made.
    if let Some(parent) = parent
        && let Err(e) = check_glass_on_glass(conn, parent, component)
    {
        return (e, Touched::default());
    }
    // GH #1264: a parent may be named before it exists, so a create can close
    // a cycle as well as a move can.
    if let Some(parent) = parent
        && let Err(e) = check_parent(conn, "object.create", &id, parent)
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

/// Stored props as the bag a patch is merged into.
///
/// GH #1264: JSON that is no object (a seed stores its text as it is) is an
/// empty bag, like text that is no JSON. A patch found no key to set there,
/// fell away and was answered `wrote 1`.
fn stored_props(raw: &str) -> Value {
    match meclaw_core::serde_json::from_str(raw) {
        Ok(Value::Object(bag)) => Value::Object(bag),
        _ => json!({}),
    }
}

fn object_update(conn: &Connection, args: &Value) -> (OpOutcome, Touched) {
    let id = match arg_str(args, "id") {
        Ok(i) => i,
        Err(e) => return (e, Touched::default()),
    };
    let current: Result<(String, String), _> = conn
        .prepare_cached("SELECT component, props FROM objects WHERE id = ?1")
        .and_then(|mut stmt| {
            stmt.query_row([id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
        });
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
    let mut merged = stored_props(&existing);
    if let (Some(target), Some(patch_obj)) = (merged.as_object_mut(), patch.as_object()) {
        for (k, v) in patch_obj {
            target.insert(k.clone(), v.clone());
        }
    }

    match conn
        .prepare_cached("UPDATE objects SET props = ?1 WHERE id = ?2")
        .and_then(|mut stmt| stmt.execute(rusqlite::params![merged.to_string(), id]))
    {
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

/// Whether `id` may hang under `parent`: not under itself or an object below it.
///
/// GH #1264: such a move or create was answered `wrote 1`. Its objects formed a
/// cycle no page root reaches: gone from every page, and not deletable either,
/// because each of them has a child. The walk up carries the renderer's bound: past it
/// the object could not be drawn, and a cycle above it never ends the walk.
fn check_parent(
    conn: &Connection,
    operation: &str,
    id: &str,
    parent: &str,
) -> Result<(), OpOutcome> {
    let refuse = |text: String| Err(OpOutcome::refused(operation, "invalid_input", text));
    let mut current = parent.to_string();
    for _ in 0..MAX_DEPTH {
        if current == id {
            return refuse(format!(
                "{parent:?} is {id:?} itself or stands below it; {operation} would cut both off \
                 from every page"
            ));
        }
        let up = conn
            .prepare_cached("SELECT parent FROM objects WHERE id = ?1")
            .and_then(|mut stmt| stmt.query_row([&current], |r| r.get::<_, Option<String>>(0)));
        match up {
            Ok(Some(p)) => current = p,
            Ok(None) | Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(()),
            Err(e) => return refuse(e.to_string()),
        }
    }
    refuse(format!(
        "{parent:?} stands {MAX_DEPTH} or more levels deep, or in a cycle; nothing below it \
         can be drawn"
    ))
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

    if let Some(parent) = args.get("parent").and_then(Value::as_str)
        && let Err(e) = check_parent(conn, "object.move", id, parent)
    {
        return (e, Touched::default());
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

    // GH #869: what the markup may carry, and what a browser may write.
    if let Err(why) = check_definition(name, template, &prop_schema, &editable) {
        return refuse_define("component.define", "invalid_input", why);
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
/// Used when a change is too broad to attribute to one slot, and by
/// [`touched_by`] to find the pages rooted on a path.
fn all_routes(conn: &Connection) -> Vec<(String, String)> {
    let Ok(mut stmt) = conn.prepare_cached("SELECT route, root FROM pages ORDER BY route") else {
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

/// Read object state back: one object by `id`, or, by `route`, ALL objects of
/// the cell, of every page, with the root of that route. Cutting the answer to
/// the route's subtree breaks a reader: `read_tree` in `display/compose`
/// mirrors the pages of every output from one query on `/` (GH #1264).
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
/// Returns the verdict the socket owes its client, and — on success — the
/// slot the write touched, so the push carries that one slot instead of the
/// whole packed tree (GH #414: the browser lane used to fake a structural
/// touch, and on a real colony every drag write broadcast ~220 KB; a group
/// drag flooded the viewer channels until frames dropped). Nothing is written
/// on a refusal — not a partial prop, not an audit row — and the touched set
/// is empty.
///
/// A prop typed `"html"` is never written, whatever `editable` says (review
/// M2 of #869): define and the seed refuse such a declaration since
/// `web@2.2.0`, but a component stored before keeps its row, and what one
/// viewer types would reach every other viewer as markup. The verdict is the
/// same `not_editable` a prop outside the list gets.
///
/// GH #1264: a write the operation timeout interrupts is answered
/// `query_timeout` and rolled back. The write and the lookup of its slot are
/// one transaction for that: an interrupt between the two would store a write
/// nobody publishes.
pub fn set_editable(
    conn: &Connection,
    id: &str,
    prop: &str,
    value: &Value,
) -> (crate::web::cell::EventReply, Touched) {
    use crate::web::cell::EventReply;

    let mut cut = false;
    let done = holding_interrupt(conn, || {
        // The lock from the start: a transaction that reads first cannot wait
        // for another writer at its write, SQLite refuses the step at once.
        // And inside the hold: an interrupt while this waits would be forgotten
        // with the `BEGIN`, and the write would land past its clock.
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let done = write_editable(conn, id, prop, value);
        cut = conn.is_interrupted();
        // A write SQLite refused may have ended the transaction itself.
        if !cut && !conn.is_autocommit() {
            conn.execute_batch("COMMIT")?;
        }
        Ok(done)
    });
    if !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    let refused = |why: &str| (EventReply::Error(why.to_string()), Touched::default());
    match done {
        Ok(_) if cut => refused("query_timeout"),
        Ok(done) => done,
        Err(e) if interrupted(&e) => refused("query_timeout"),
        Err(e) => {
            tracing::error!(%id, %prop, error = %e, "web: editable write failed");
            refused("the write failed")
        }
    }
}

/// The checks and the write of [`set_editable`].
fn write_editable(
    conn: &Connection,
    id: &str,
    prop: &str,
    value: &Value,
) -> (crate::web::cell::EventReply, Touched) {
    use crate::web::cell::EventReply;

    let row: Result<(String, String, String, String), _> = conn.query_row(
        "SELECT o.component, o.props, c.editable, c.prop_schema
           FROM objects o JOIN components c ON c.name = o.component
          WHERE o.id = ?1",
        [id],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        },
    );
    let Ok((component, props_raw, editable_raw, schema_raw)) = row else {
        return (
            EventReply::Error(format!("no object {id:?}")),
            Touched::default(),
        );
    };

    let editable: Vec<String> =
        meclaw_core::serde_json::from_str(&editable_raw).unwrap_or_default();
    let schema: Value = meclaw_core::serde_json::from_str(&schema_raw).unwrap_or_default();
    let raw = schema.get(prop).and_then(Value::as_str) == Some("html");
    if raw || !editable.iter().any(|e| e == prop) {
        return (
            EventReply::Error("not_editable".to_string()),
            Touched::default(),
        );
    }

    let mut props = stored_props(&props_raw);
    if let Some(obj) = props.as_object_mut() {
        obj.insert(prop.to_string(), value.clone());
    }

    match conn.execute(
        "UPDATE objects SET props = ?1 WHERE id = ?2",
        rusqlite::params![props.to_string(), id],
    ) {
        Ok(_) => (EventReply::Ok, touched_by(conn, id)),
        Err(e) => {
            tracing::error!(%component, %id, %prop, error = %e, "web: editable write failed");
            (
                EventReply::Error("the write failed".to_string()),
                Touched::default(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        crate::web::db::setup_web_schema(&c).unwrap();
        c
    }

    fn define(c: &Connection, template: &str, schema: Value, editable: Value) -> OpOutcome {
        apply(
            c,
            &json!({"op": "component.define", "name": "c", "template": template,
                    "prop_schema": schema, "editable": editable}),
        )
        .0
    }

    /// GH #869: a component's markup reaches the cell through a binding, never
    /// through an event-handler attribute, and a link never runs code. rev-3
    /// I-3: the rule is anchored at the start of a name, so the display's own
    /// `data-front`, `data-tone`, `data-cond` and `aria-controls` pass.
    #[test]
    fn define_refuses_handlers_and_script_urls_and_keeps_names_that_end_in_on() {
        let c = conn();
        for ok in [
            r#"<b data-front="{{f}}" data-tone="x" data-cond="y" aria-controls="z">{{f}}</b>"#,
            r#"<button phx-click="{{f}}">{{f}}</button>"#,
            r#"<script>{{&f}}</script>"#,
        ] {
            let out = define(&c, ok, json!({"f": "text"}), json!([]));
            assert_eq!(out.error_code, None, "{ok}: {out:?}");
        }
        for bad in [
            r#"<a onclick="x()">{{f}}</a>"#,
            r#"<svg onload=x()></svg>"#,
            r#"<a href="javascript:x()">{{f}}</a>"#,
        ] {
            let out = define(&c, bad, json!({"f": "text"}), json!([]));
            assert_eq!(
                out.error_code.as_deref(),
                Some("invalid_input"),
                "{bad}: {out:?}"
            );
        }
    }

    /// Review I1 (#868/#869): the rules above read the template's own text,
    /// so a value standing inside a NAME would join that text into a name the
    /// rules never saw. Each of these renders a script, a handler, a script URL
    /// or a frame with an empty or chosen value; define refuses the place.
    #[test]
    fn define_refuses_a_value_that_stands_in_a_name() {
        let c = conn();
        for bad in [
            "<scr{{f}}ipt>window.__pwned=1</scr{{f}}ipt>",
            r#"<a on{{f}}click="window.__pwned=1">x</a>"#,
            r#"<a href="java{{f}}script:window.__pwned=1">x</a>"#,
            r#"<ifr{{f}}ame src="https://example.org"></iframe>"#,
            r#"<a h{{f}}="{{f}}">x</a>"#,
        ] {
            let out = define(&c, bad, json!({"f": "text"}), json!([]));
            assert_eq!(
                out.error_code.as_deref(),
                Some("invalid_input"),
                "{bad}: {out:?}"
            );
        }
    }

    /// GH #869: what one viewer types is never markup for the next one.
    #[test]
    fn define_refuses_an_editable_html_prop() {
        let c = conn();
        let out = define(&c, "<p>{{&b}}</p>", json!({"b": "html"}), json!(["b"]));
        assert_eq!(out.error_code.as_deref(), Some("invalid_input"), "{out:?}");
        let out = define(&c, "<p>{{b}}</p>", json!({"b": "text"}), json!(["b"]));
        assert_eq!(out.error_code, None, "{out:?}");
    }

    /// Review M2 (#869): define and the seed refuse `editable` on an `"html"`
    /// prop, but a component stored before that rule keeps its row. The write
    /// asks the type too, so what a browser sends never lands in a prop the
    /// template renders raw.
    #[test]
    fn a_browser_never_writes_an_html_prop_whatever_the_stored_row_says() {
        use crate::web::cell::EventReply;
        let c = conn();
        c.execute_batch(
            r#"INSERT INTO components (name, template, prop_schema, editable)
                 VALUES ('old', '<p>{{&b}}{{t}}</p>', '{"b":"html","t":"text"}', '["b","t"]');
               INSERT INTO objects (id, parent, component, ord, props)
                 VALUES ('o', NULL, 'old', 0, '{"b":"kept","t":"x"}');"#,
        )
        .unwrap();
        let (reply, touched) = set_editable(&c, "o", "b", &json!("<img src=x onerror=alert(1)>"));
        assert_eq!(reply, EventReply::Error("not_editable".to_string()));
        assert_eq!(touched, Touched::default());
        let props: String = c
            .query_row("SELECT props FROM objects WHERE id = 'o'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(props, r#"{"b":"kept","t":"x"}"#);
        // The text prop beside it stays writable.
        let (reply, _) = set_editable(&c, "o", "t", &json!("y"));
        assert_eq!(reply, EventReply::Ok);
    }

    /// A pool for the bundle tests: one component, one object standing
    /// before the bundle (so a `query` leg has something to read).
    fn bundle_conn() -> Connection {
        let c = conn();
        c.execute_batch(
            r#"INSERT INTO components (name, template, prop_schema, editable)
                 VALUES ('p', '<p>{{t}}</p>', '{"t":"text"}', '[]');
               INSERT INTO objects (id, parent, component, ord, props)
                 VALUES ('pre', NULL, 'p', 0, '{"t":"x"}');"#,
        )
        .unwrap();
        c
    }

    fn create(id: &str) -> Value {
        json!({"op": "object.create", "component": "p", "id": id, "props": {"t": "y"}})
    }

    fn ids(c: &Connection) -> Vec<String> {
        let mut stmt = c.prepare("SELECT id FROM objects ORDER BY id").unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn refused_with(leg: &Applied, needle: &str) {
        let (outcome, touched, _) = leg;
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("invalid_input"),
            "{outcome:?}"
        );
        let text = outcome.error_text.as_deref().unwrap_or_default();
        assert!(text.contains(needle), "{needle:?} not in {text:?}");
        assert_eq!(*touched, Touched::default());
    }

    /// GH #1264: an interrupt that falls between two statements holds to the end
    /// of the work. SQLite forgets one as soon as no statement runs, so without
    /// the held statement the write and the `COMMIT` after it go through.
    #[test]
    fn gh1264_an_interrupt_between_two_statements_holds_to_the_end() {
        let c = bundle_conn();
        let clock = c.get_interrupt_handle();
        c.execute_batch("BEGIN").unwrap();
        let (write, commit) = holding_interrupt(&c, || {
            clock.interrupt();
            (
                c.execute("UPDATE objects SET ord = 7", []),
                c.execute_batch("COMMIT"),
            )
        });
        assert!(write.as_ref().is_err_and(interrupted), "{write:?}");
        assert!(commit.as_ref().is_err_and(interrupted), "{commit:?}");
        if !c.is_autocommit() {
            c.execute_batch("ROLLBACK").unwrap();
        }
        // Over with the work: the connection takes the next statement.
        let ord: i64 = c
            .query_row("SELECT ord FROM objects", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ord, 0);
    }

    /// GH #1264: the same through a bundle. The interrupt comes out of the last
    /// step of a leg's write, which SQLite completes; the rest is refused.
    #[test]
    fn gh1264_an_interrupt_between_two_legs_ends_the_bundle() {
        let c = bundle_conn();
        let clock = c.get_interrupt_handle();
        c.create_scalar_function(
            "clock_fires",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| {
                clock.interrupt();
                Ok(1)
            },
        )
        .unwrap();
        c.execute_batch(
            "CREATE TRIGGER clock AFTER INSERT ON objects WHEN NEW.id = 'one'
               BEGIN SELECT clock_fires(); END;",
        )
        .unwrap();
        let out = apply_all(&c, &[create("zero"), create("one"), create("two")]);
        assert_eq!(out.len(), 3);
        for leg in &out {
            assert_eq!(leg.0.error_code.as_deref(), Some("query_timeout"));
            assert_eq!(leg.1, Touched::default());
        }
        assert_eq!(ids(&c), ["pre"]);
        assert!(c.is_autocommit());
    }

    static BUSY_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// GH #1264: `object:set` reads before it writes. As a deferred transaction
    /// the step from the read to the write fails at once beside another writer;
    /// SQLite asks the busy handler only where a transaction starts.
    #[test]
    fn gh1264_an_editable_write_waits_for_another_writer() {
        use std::sync::atomic::Ordering;

        fn give_up(_: i32) -> bool {
            BUSY_ASKED.store(true, Ordering::SeqCst);
            false
        }

        let td = tempfile::TempDir::new().unwrap();
        let file = td.path().join("cell.db");
        let mut c = Connection::open(&file).unwrap();
        c.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        crate::web::db::setup_web_schema(&c).unwrap();
        c.execute_batch(
            r#"INSERT INTO components (name, template, prop_schema, editable)
                 VALUES ('p', '<p>{{t}}</p>', '{"t":"text"}', '["t"]');
               INSERT INTO objects (id, parent, component, ord, props)
                 VALUES ('o', NULL, 'p', 0, '{"t":"x"}');"#,
        )
        .unwrap();
        let other = Connection::open(&file).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        c.busy_handler(Some(give_up)).unwrap();
        let (reply, touched) = set_editable(&c, "o", "t", &json!("y"));
        assert!(
            matches!(reply, crate::web::cell::EventReply::Error(_)),
            "the other writer holds the lock"
        );
        assert_eq!(touched, Touched::default());
        assert!(BUSY_ASKED.load(Ordering::SeqCst), "the write did not wait");

        other.execute_batch("ROLLBACK").unwrap();
        let (reply, _) = set_editable(&c, "o", "t", &json!("y"));
        assert!(matches!(reply, crate::web::cell::EventReply::Ok));
        assert!(c.is_autocommit());
    }

    static WAITING: std::sync::Mutex<Option<(rusqlite::InterruptHandle, Connection)>> =
        std::sync::Mutex::new(None);

    /// GH #1264: an interrupt that falls while `object:set` waits for another
    /// writer ends it, also when the lock comes free after that. The wait is
    /// the `BEGIN` itself: not held, it ends well and SQLite forgets the
    /// interrupt before the write.
    #[test]
    fn gh1264_an_interrupt_while_an_editable_write_waits_is_kept() {
        use crate::web::cell::EventReply;

        /// The first call is the clock, then the other writer lets go.
        fn clock_then_free(_: i32) -> bool {
            let Some((clock, other)) = WAITING.lock().unwrap().take() else {
                return false;
            };
            clock.interrupt();
            other.execute_batch("ROLLBACK").unwrap();
            true
        }

        let td = tempfile::TempDir::new().unwrap();
        let file = td.path().join("cell.db");
        let mut c = Connection::open(&file).unwrap();
        c.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        crate::web::db::setup_web_schema(&c).unwrap();
        c.execute_batch(
            r#"INSERT INTO components (name, template, prop_schema, editable)
                 VALUES ('p', '<p>{{t}}</p>', '{"t":"text"}', '["t"]');
               INSERT INTO objects (id, parent, component, ord, props)
                 VALUES ('o', NULL, 'p', 0, '{"t":"x"}');"#,
        )
        .unwrap();
        let other = Connection::open(&file).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        *WAITING.lock().unwrap() = Some((c.get_interrupt_handle(), other));
        c.busy_handler(Some(clock_then_free)).unwrap();

        let (reply, touched) = set_editable(&c, "o", "t", &json!("y"));
        assert!(
            WAITING.lock().unwrap().is_none(),
            "the write did not wait for the other writer"
        );
        assert_eq!(reply, EventReply::Error("query_timeout".to_string()));
        assert_eq!(touched, Touched::default());
        assert!(c.is_autocommit());
        let props: String = c
            .query_row("SELECT props FROM objects WHERE id = 'o'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(props, r#"{"t":"x"}"#);

        // Over with the interrupted call: the next write lands.
        let (reply, _) = set_editable(&c, "o", "t", &json!("y"));
        assert_eq!(reply, EventReply::Ok);
    }

    /// GH #1030 review: a `COMMIT` that fails loses every write of the bundle,
    /// so every leg that said it wrote is answered as a refusal, the reads keep
    /// their payload, and the transaction is rolled back rather than left open.
    /// The failure is a deferred foreign key: the leg itself succeeds, the
    /// check runs at `COMMIT` and refuses it with the transaction still open.
    #[test]
    fn a_commit_that_fails_refuses_every_write_of_the_bundle_and_keeps_its_reads() {
        let c = bundle_conn();
        c.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE guard (id TEXT PRIMARY KEY);
             CREATE TABLE orphan (
               ref TEXT REFERENCES guard(id) DEFERRABLE INITIALLY DEFERRED);
             CREATE TRIGGER late AFTER INSERT ON objects WHEN NEW.id = 'late'
               BEGIN INSERT INTO orphan VALUES ('nowhere'); END;",
        )
        .unwrap();
        let out = apply_all(
            &c,
            &[
                create("a"),
                json!({"op": "query", "id": "pre"}),
                create("late"),
            ],
        );
        assert_eq!(out.len(), 3);
        refused_with(&out[0], "did not commit");
        assert!(!out[1].0.is_error(), "{:?}", out[1].0);
        assert_eq!(out[1].0.payload["object"]["id"], "pre");
        refused_with(&out[2], "did not commit");
        assert!(
            c.is_autocommit(),
            "the failed COMMIT left a transaction open"
        );
        assert_eq!(ids(&c), vec!["pre".to_string()]);
    }

    /// GH #1030 review: SQLite ending the transaction by itself mid-bundle (a
    /// full disk, an I/O error; here a trigger's `RAISE(ROLLBACK)`) takes the
    /// writes before it along, so those legs become refusals; the failing leg
    /// keeps its own refusal, a read keeps its payload, and the legs after it
    /// run in a new transaction and stay written.
    #[test]
    fn a_transaction_the_database_ends_itself_refuses_the_writes_before_it_only() {
        let c = bundle_conn();
        c.execute_batch(
            "CREATE TRIGGER boom BEFORE INSERT ON objects WHEN NEW.id = 'boom'
               BEGIN SELECT RAISE(ROLLBACK, 'the disk is gone'); END;",
        )
        .unwrap();
        let out = apply_all(
            &c,
            &[
                create("a"),
                json!({"op": "query", "id": "pre"}),
                create("boom"),
                create("c"),
            ],
        );
        assert_eq!(out.len(), 4);
        refused_with(&out[0], "rolled the bundle's transaction back");
        assert!(!out[1].0.is_error(), "{:?}", out[1].0);
        assert_eq!(out[1].0.payload["object"]["id"], "pre");
        refused_with(&out[2], "the disk is gone");
        assert!(!out[3].0.is_error(), "{:?}", out[3].0);
        assert_eq!(out[3].0.rows_affected, 1);
        assert!(c.is_autocommit(), "the bundle left a transaction open");
        assert_eq!(ids(&c), vec!["c".to_string(), "pre".to_string()]);
    }

    /// Objects `(id, parent)` of one component that draws its prop and its
    /// children, and pages `(route, root)` over them.
    fn tree_conn(objects: &[(&str, Option<&str>)], pages: &[(&str, &str)]) -> Connection {
        let c = conn();
        c.execute_batch(
            r#"INSERT INTO components (name, template, prop_schema, editable)
                 VALUES ('box', '<div data-t="{{t}}">{{children}}</div>', '{"t":"text"}', '[]');"#,
        )
        .unwrap();
        for (id, parent) in objects {
            c.execute(
                r#"INSERT INTO objects (id, parent, component, ord, props)
                   VALUES (?1, ?2, 'box', 0, '{"t":"old"}')"#,
                rusqlite::params![id, parent],
            )
            .unwrap();
        }
        for (route, root) in pages {
            c.execute(
                "INSERT INTO pages (route, root, title) VALUES (?1, ?2, '')",
                [route, root],
            )
            .unwrap();
        }
        c
    }

    fn parent_of(c: &Connection, id: &str) -> Option<String> {
        c.query_row("SELECT parent FROM objects WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// GH #1264: a page whose root hangs under another object hears every write
    /// that shows on it. The walk named only the routes of the topmost ancestor,
    /// so a nested page stayed on its old picture, and one under an object
    /// without a page was never published at all.
    #[test]
    fn gh1264_an_update_below_a_nested_page_root_reaches_that_page() {
        use crate::web::render::{materialize_all, rerender};
        let c = tree_conn(
            &[
                ("r", None),
                ("c", Some("r")),
                ("k", Some("c")),
                ("k2", Some("c")),
                ("u", None),
                ("d", Some("u")),
                ("leaf", Some("d")),
            ],
            &[("/", "r"), ("/detail", "c"), ("/island", "d")],
        );
        let mut held = materialize_all(&c).unwrap();
        let update = |id: &str| json!({"op": "object.update", "id": id, "props": {"t": format!("new-{id}")}});
        for (op, routes) in [
            (update("k"), &["/", "/detail"][..]),
            // The nested root itself: a slot of "/", the statics of "/detail".
            (update("c"), &["/", "/detail"][..]),
            // The top of a tree broadcasts to every route.
            (update("r"), &["/", "/detail", "/island"][..]),
            (update("leaf"), &["/island"][..]),
            (update("d"), &["/island"][..]),
            (
                json!({"op": "object.create", "id": "n", "component": "box", "parent": "c",
                       "props": {"t": "born"}}),
                &["/", "/detail"][..],
            ),
            (
                json!({"op": "object.move", "id": "n", "parent": "k"}),
                &["/", "/detail"][..],
            ),
            (
                json!({"op": "object.move", "id": "n", "parent": "d"}),
                &["/", "/detail", "/island"][..],
            ),
            (
                json!({"op": "object.delete", "id": "k2"}),
                &["/", "/detail"][..],
            ),
        ] {
            let (outcome, touched) = apply(&c, &op);
            assert!(!outcome.is_error(), "{op}: {outcome:?}");
            let done = rerender(&c, &held, &touched).unwrap();
            let mut framed: Vec<&str> = done.frames.iter().map(|(r, _)| r.as_str()).collect();
            framed.sort_unstable();
            assert_eq!(framed, routes, "{op}: one frame per page that shows it");
            let fresh = materialize_all(&c).unwrap();
            for (route, page) in fresh.iter() {
                assert_eq!(
                    done.pages[route.as_str()].rendered_body(),
                    page.rendered_body(),
                    "{op}: {route} is published as a whole render draws it"
                );
            }
            held = done.pages;
        }
    }

    /// GH #1264: a move under itself or under its own descendant is refused and
    /// moves nothing. It was answered `wrote 1`: the objects formed a cycle no
    /// root reaches, gone from every page, and no delete could take them (each
    /// has a child).
    #[test]
    fn gh1264_a_move_under_its_own_descendant_is_refused() {
        let c = tree_conn(
            &[("root", None), ("a", Some("root")), ("b", Some("a"))],
            &[("/", "root")],
        );
        for (id, parent) in [("a", "b"), ("a", "a"), ("root", "b")] {
            let before = parent_of(&c, id);
            let (outcome, touched) = apply(
                &c,
                &json!({"op": "object.move", "id": id, "parent": parent}),
            );
            assert_eq!(
                outcome.error_code.as_deref(),
                Some("invalid_input"),
                "{id} under {parent}: {outcome:?}"
            );
            assert_eq!(touched, Touched::default());
            assert_eq!(parent_of(&c, id), before, "{id} stays where it was");
        }
        // What is no cycle still moves: up, to the top, and a new ord alone.
        for op in [
            json!({"op": "object.move", "id": "b", "parent": "root"}),
            json!({"op": "object.move", "id": "b", "parent": null}),
            json!({"op": "object.move", "id": "a", "ord": 7}),
            json!({"op": "object.move", "id": "a", "parent": "b"}),
        ] {
            let (outcome, _) = apply(&c, &op);
            assert!(!outcome.is_error(), "{op}: {outcome:?}");
        }
    }

    /// GH #1264: the walk is bounded like the renderer's, and a parent at the
    /// bound is refused: below it the object could not be drawn, and a cycle
    /// the walk has not closed yet looks the same.
    #[test]
    fn gh1264_a_move_past_the_depth_bound_is_refused() {
        let names: Vec<String> = (0..=MAX_DEPTH).map(|i| format!("d{i}")).collect();
        let mut objects: Vec<(&str, Option<&str>)> = vec![("x", None)];
        for (i, name) in names.iter().enumerate() {
            objects.push((name, (i > 0).then(|| names[i - 1].as_str())));
        }
        let c = tree_conn(&objects, &[("/", "d0")]);
        let to = |parent: &str| json!({"op": "object.move", "id": "x", "parent": parent});
        let (outcome, _) = apply(&c, &to(&names[MAX_DEPTH]));
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("invalid_input"),
            "{outcome:?}"
        );
        assert_eq!(parent_of(&c, "x"), None);
        let (outcome, _) = apply(&c, &to(&names[MAX_DEPTH - 1]));
        assert!(!outcome.is_error(), "{outcome:?}");
        assert!(
            crate::web::render::materialize_all(&c)
                .unwrap()
                .contains_key("/")
        );
    }

    /// GH #1264: an id the cell makes up is free. It was `obj-{count + 1}`,
    /// which a delete or a caller's own `obj-N` makes a taken one, and then
    /// every create without an id was refused until the count moved. An id an
    /// object or a page still names is not free either: the new object would
    /// take over what was left behind.
    #[test]
    fn gh1264_a_generated_id_never_collides_after_a_delete() {
        let bare = json!({"op": "object.create", "component": "box"});
        let c = tree_conn(&[], &[]);
        for op in [
            bare.clone(),
            bare.clone(),
            json!({"op": "object.delete", "id": "obj-1"}),
            bare.clone(),
            json!({"op": "object.create", "component": "box", "id": "obj-4"}),
            bare.clone(),
        ] {
            let (outcome, _) = apply(&c, &op);
            assert!(!outcome.is_error(), "{op}: {outcome:?}");
        }
        assert_eq!(ids(&c), ["obj-2", "obj-3", "obj-4", "obj-5"]);

        // Two objects, so the count alone would say `obj-3`.
        let c = tree_conn(&[("y", Some("obj-3")), ("z", None)], &[("/", "obj-4")]);
        let (outcome, _) = apply(
            &c,
            &json!({"op": "object.create", "component": "box", "parent": "y"}),
        );
        assert!(!outcome.is_error(), "{outcome:?}");
        assert_eq!(ids(&c), ["obj-5", "y", "z"]);
    }

    /// GH #1264: a create closes no cycle either: not as its own parent, and
    /// not under an object whose chain already names the new id. It carries the
    /// move's depth bound as well.
    #[test]
    fn gh1264_a_create_that_closes_a_cycle_is_refused() {
        let c = tree_conn(
            &[("root", None), ("y", Some("a")), ("k", Some("y"))],
            &[("/", "root")],
        );
        let create = |id: &str, parent: &str| json!({"op": "object.create", "component": "box", "id": id, "parent": parent});
        for (id, parent) in [("a", "a"), ("a", "y"), ("a", "k")] {
            let (outcome, touched) = apply(&c, &create(id, parent));
            assert_eq!(
                outcome.error_code.as_deref(),
                Some("invalid_input"),
                "{id} under {parent}: {outcome:?}"
            );
            assert_eq!(touched, Touched::default());
            assert_eq!(ids(&c), ["k", "root", "y"], "nothing is written");
        }
        // What closes none is still created, the missing parent of `y` included.
        for (id, parent) in [("a", "root"), ("b", "k"), ("early", "later")] {
            let (outcome, _) = apply(&c, &create(id, parent));
            assert!(!outcome.is_error(), "{id} under {parent}: {outcome:?}");
        }

        let names: Vec<String> = (0..=MAX_DEPTH).map(|i| format!("d{i:02}")).collect();
        let mut objects: Vec<(&str, Option<&str>)> = Vec::new();
        for (i, name) in names.iter().enumerate() {
            objects.push((name, (i > 0).then(|| names[i - 1].as_str())));
        }
        let c = tree_conn(&objects, &[("/", "d00")]);
        let (outcome, _) = apply(&c, &create("x", &names[MAX_DEPTH]));
        assert_eq!(
            outcome.error_code.as_deref(),
            Some("invalid_input"),
            "{outcome:?}"
        );
        let (outcome, _) = apply(&c, &create("x", &names[MAX_DEPTH - 1]));
        assert!(!outcome.is_error(), "{outcome:?}");
        assert!(
            crate::web::render::materialize_all(&c)
                .unwrap()
                .contains_key("/"),
            "what a create lets through is drawn"
        );
    }

    /// GH #1264: stored props that are JSON but no object (a seed stores its
    /// text as it is) take a patch like an empty bag. The patch fell away and
    /// was answered `wrote 1`.
    #[test]
    fn gh1264_an_update_on_non_object_props_is_not_dropped() {
        let c = tree_conn(&[("root", None), ("a", Some("root"))], &[("/", "root")]);
        c.execute_batch(
            r#"UPDATE components SET editable = '["t"]';
               UPDATE objects SET props = '[]' WHERE id = 'a';
               UPDATE objects SET props = '7' WHERE id = 'root';"#,
        )
        .unwrap();
        let props = |id: &str| -> Value {
            let raw: String = c
                .query_row("SELECT props FROM objects WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .unwrap();
            meclaw_core::serde_json::from_str(&raw).unwrap()
        };
        let (outcome, _) = apply(
            &c,
            &json!({"op": "object.update", "id": "a", "props": {"t": "new"}}),
        );
        assert!(!outcome.is_error(), "{outcome:?}");
        assert_eq!(props("a"), json!({"t": "new"}));
        let (reply, _) = set_editable(&c, "root", "t", &json!("new"));
        assert!(matches!(reply, crate::web::cell::EventReply::Ok));
        assert_eq!(props("root"), json!({"t": "new"}));
    }

    /// GH #1264: an update to an object no page shows renders nothing. Having no
    /// parent was taken for being a page's root, so it rendered every route
    /// whole. The root of a page still does.
    #[test]
    fn gh1264_an_update_to_an_unpaged_object_renders_nothing() {
        let c = tree_conn(
            &[("root", None), ("loose", None), ("leaf", Some("loose"))],
            &[("/", "root")],
        );
        for id in ["loose", "leaf"] {
            let (outcome, touched) = apply(
                &c,
                &json!({"op": "object.update", "id": id, "props": {"t": "new"}}),
            );
            assert!(!outcome.is_error(), "{outcome:?}");
            assert_eq!(touched, Touched::default(), "{id}");
        }
        let (_, touched) = apply(
            &c,
            &json!({"op": "object.update", "id": "root", "props": {"t": "new"}}),
        );
        assert!(
            touched.structural && touched.slots.is_empty(),
            "{touched:?}"
        );
    }

    /// GH #1264: a `query` by route answers the objects of EVERY page of the
    /// cell, with the root of the route it named. That reads like a fault and
    /// is used: `read_tree` in `display/compose` asks `/` once and mirrors the
    /// pages of all outputs from the one answer. Cut to the route's subtree,
    /// the display would create the other outputs' objects again, be refused,
    /// and never delete the tree of an output that is gone.
    #[test]
    fn gh1264_a_route_query_answers_the_objects_of_every_page() {
        let c = tree_conn(
            &[
                ("root", None),
                ("a", Some("root")),
                ("wall.root", None),
                ("wall.a", Some("wall.root")),
            ],
            &[("/", "root"), ("/wall", "wall.root")],
        );
        for (route, root) in [("/", "root"), ("/wall", "wall.root")] {
            let (outcome, _) = apply(&c, &json!({"op": "query", "route": route}));
            assert!(!outcome.is_error(), "{outcome:?}");
            assert_eq!(outcome.payload["root"], root);
            let mut ids: Vec<&str> = outcome.payload["objects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|o| o["id"].as_str().unwrap())
                .collect();
            ids.sort();
            assert_eq!(ids, vec!["a", "root", "wall.a", "wall.root"], "{route}");
        }
    }
}
