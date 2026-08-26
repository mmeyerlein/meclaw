//! W8 Task 11 (GH #382): the shipped `web` template — `templates/web/`.
//!
//! The first template in the library that is a `web` cell, and the first that
//! declares `contract.ingress` at all. What is pinned here is what the template
//! PROMISES, and every question is asked of the **substrate's own reader**
//! rather than of a second opinion written in this file:
//!
//! 1. **The descriptor resolves.** `web@1.1.0` — the reference a mutation
//!    writes down.
//! 2. **The config is a persistent `web` cell with a port of its own**, read
//!    through `meclaw_colony::ParsedConfig` (the reader every boot and every
//!    staging path goes through) and its params through `WebParams::parse`
//!    (the cell type's one parser). A `contract` block in a shape the cell
//!    reader cannot deserialize would be a boot refusal, not a documentation
//!    defect — so the reader is the judge.
//! 3. **The ingress claim is the one Task 10 needs**: `session_id`, which the
//!    entry edge promotes into `context.session_id`.
//! 4. **Every seed file passes the cell's own static check** —
//!    `web::seed::check_seed_files`, the function `validate_cell_dir` calls in
//!    the plan phase. A seed that survives validation loads at spawn.
//! 5. **The seed loads into the real schema and the page renders**, through
//!    `setup_web_schema` + `load_seed_if_present` + `materialize_all`. A seed
//!    that parses but renders to nothing would be a template that ships a
//!    blank display.
//! 6. **The stylesheet is the one the design asks for.** Four load-bearing
//!    strings, each of which is a whole behaviour: `backdrop-filter` (the
//!    glass), `@supports` (the opaque fallback where there is none),
//!    `prefers-reduced-transparency` and `forced-colors` (the two settings
//!    that turn the material off on purpose). A token sheet missing any of
//!    them is a decoration rather than a design system.
//!
//! `web` is a PUBLIC template, so there is no export guard: in the published
//! clone the directory is there and these reads resolve.

use meclaw_cells::web::db::setup_web_schema;
use meclaw_cells::web::params::WebParams;
use meclaw_cells::web::render::{materialize_all, parse_template};
use meclaw_cells::web::seed::{check_seed_files, load_seed_if_present};
use meclaw_colony::ParsedConfig;
use meclaw_colony::config::validate_contract_presence;
use meclaw_core::serde_json::Value;
use rusqlite::Connection;

/// The shipped template directory.
fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/web")
}

fn read(rel: &str) -> String {
    let path = template_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("templates/web/{rel} must ship: {e}"))
}

fn json(rel: &str) -> Value {
    meclaw_core::serde_json::from_str(&read(rel))
        .unwrap_or_else(|e| panic!("templates/web/{rel} must be JSON: {e}"))
}

/// The template's `config.json` through the reader every boot uses.
fn parsed_config() -> ParsedConfig {
    meclaw_core::serde_json::from_str(&read("config.json"))
        .expect("templates/web/config.json must deserialize through ParsedConfig")
}

/// A fresh database with the shipped seed loaded, exactly as a first spawn
/// (`OpenStatus::Created`) does it.
fn seeded_db() -> Connection {
    let conn = Connection::open_in_memory().expect("in-memory db");
    setup_web_schema(&conn).expect("the web schema applies");
    load_seed_if_present(&conn, &template_dir()).expect("the shipped seed loads");
    conn
}

#[test]
fn the_descriptor_names_the_template_at_the_version_it_ships() {
    let val = json("template.json");
    assert_eq!(val["name"].as_str(), Some("web"));
    // Derived from the shipped file rather than repeated here: the version is
    // a number in a public surface, and a second copy of it is one more place
    // a bump can half-land (`development-rules.md` § 2d). What this asserts is
    // the SHAPE — a version exists and is a three-part one a reference can
    // name; which version it is, is `template.json`'s to say.
    let version = val["version"]
        .as_str()
        .expect("template.json names a version");
    assert_eq!(
        version.split('.').count(),
        3,
        "the reference a mutation writes down is `web@{version}`"
    );
    assert!(
        version.split('.').all(|p| p.parse::<u32>().is_ok()),
        "each part of `web@{version}` is a number"
    );
    // The four description slots the library table and the builder read.
    for slot in ["purpose", "use_when", "not_in_scope"] {
        assert!(
            val["description"][slot]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "template.json description.{slot} is empty"
        );
    }
}

#[test]
fn the_config_is_a_persistent_web_cell_with_a_port_of_its_own() {
    let cfg = parsed_config();
    assert_eq!(cfg.cell.cell_type, "web");
    assert_eq!(
        cfg.cell.timeout, -1,
        "a display is persistent — an idling `web` cell would stop serving"
    );

    // The cell type's own parser, not a second reading of the same JSON.
    let params = WebParams::parse(&cfg.params).expect("params must parse as WebParams");
    assert_eq!(params.port, 7800);
    assert_eq!(
        params.bind, "127.0.0.1",
        "R-W8-2: the cell never authenticates, so its default bind stays off-host"
    );

    validate_contract_presence(&cfg.contract).expect("the contract block is complete");
}

#[test]
fn the_template_declares_the_ingress_context_it_mints() {
    let cfg = parsed_config();
    let context: Vec<&str> = cfg
        .contract
        .ingress
        .context
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        context,
        vec!["session_id"],
        "a browser event is born at this cell carrying the page load's session id; \
         promoting it into context.session_id is the entry edge's job"
    );
}

#[test]
fn the_port_is_a_declared_param_so_a_second_display_can_override_it() {
    // gh212's substrate question, asked of the one key the README tells a
    // reader to override: an `override_params` key must exist under `params`
    // in the template's own config, or the mutation is refused.
    let cfg = parsed_config();
    let params = cfg.params.as_object().expect("params is an object");
    for key in ["port", "bind"] {
        assert!(
            params.contains_key(key),
            "`{key}` must stand in params for an override to address it"
        );
    }
}

#[test]
fn every_seed_file_passes_the_cells_own_static_check() {
    // The function `validate_cell_dir` calls in the plan phase. What survives
    // it loads at spawn — that is the point of sharing the parse path.
    check_seed_files(&template_dir()).expect("the shipped seed must pass --validate");

    for table in ["components", "objects", "pages", "assets"] {
        let path = template_dir().join("seed").join(format!("{table}.jsonl"));
        assert!(path.is_file(), "templates/web/seed/{table}.jsonl must ship");
    }
}

#[test]
fn every_shipped_component_template_parses_with_the_renderers_parser() {
    // The one-parser rule: `component.define` refuses a template the renderer
    // cannot draw. A seeded row bypasses that gate, so it is asked here.
    let conn = seeded_db();
    let mut stmt = conn
        .prepare("SELECT name, template FROM components ORDER BY name")
        .expect("components");
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(!rows.is_empty(), "the template ships no components at all");
    for (name, template) in rows {
        parse_template(&template)
            .unwrap_or_else(|e| panic!("component `{name}` is not renderable: {e}"));
    }
}

#[test]
fn the_seed_renders_a_page_at_the_root_route() {
    let conn = seeded_db();
    let pages = materialize_all(&conn).expect("the shipped pages render");
    let home = pages
        .get("/")
        .expect("the template ships a page at `/` — the display's own front door");
    assert!(!home.title.is_empty(), "a page carries a title");

    let body = home.rendered_body();
    assert!(
        body.contains("/vision.css"),
        "the page must reference the token stylesheet it ships: {body}"
    );
    assert!(
        !body.trim().is_empty(),
        "a display that renders to nothing is a blank screen with extra steps"
    );
}

#[test]
fn the_stylesheet_ships_as_an_asset_the_page_can_name() {
    let conn = seeded_db();
    // Read as text, not as bytes: a JSONL seed can only ever put a TEXT value
    // into the `body` column (`json_to_sql` maps a JSON string to
    // `rusqlite::types::Value::Text`, and a BLOB column has no affinity that
    // would convert it), so anything reading a seeded asset has to accept text.
    let (content_type, body): (String, String) = conn
        .query_row(
            "SELECT content_type, body FROM assets WHERE path = '/vision.css'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("the template ships /vision.css as an asset row");
    assert!(
        content_type.starts_with("text/css"),
        "a stylesheet is served as text/css, got {content_type}"
    );
    assert!(
        body.lines().count() > 100,
        "the token sheet is the design system, not a stub: {} lines",
        body.lines().count()
    );
}

#[test]
fn the_stylesheet_carries_the_four_load_bearing_declarations() {
    let conn = seeded_db();
    let css: String = conn
        .query_row(
            "SELECT body FROM assets WHERE path = '/vision.css'",
            [],
            |r| r.get(0),
        )
        .expect("/vision.css");

    // Each of the four is a whole behaviour, not a keyword: the material, the
    // fallback where the material does not exist, and the two settings that
    // switch it off on purpose.
    for needle in [
        "backdrop-filter",
        "@supports",
        "prefers-reduced-transparency",
        "forced-colors",
    ] {
        assert!(
            css.contains(needle),
            "the token sheet does not carry `{needle}` — without it the design \
             loses the behaviour that string IS"
        );
    }

    // The concentric-radius rule, which is the one geometry token a component
    // author has to be able to reach for.
    assert!(
        css.contains("--r-inner: calc(var(--r-window) - var(--r-pad))"),
        "the concentric radius is derived, never re-typed per component"
    );
}
