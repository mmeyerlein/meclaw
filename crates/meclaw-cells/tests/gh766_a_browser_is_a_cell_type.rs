//! GH #766 (wave G, T3) — the cell type `browser` exists, and its surface is closed.
//!
//! One browser per member, one cell. What this file measures is the smallest
//! thing that can be true about it before a single CDP byte moves: the params
//! surface with the numbers the contract names, the refusals that make the
//! surface a boundary rather than a suggestion, and the factory validating
//! through the same parser it spawns through.
//!
//! Two of the refusals are rulings rather than hygiene. `chromium_path` has no
//! default and no search path (OR-G9): the browser is a prerequisite out of the
//! distribution's package system, and a substrate that went looking for one
//! would eventually find something it never vetted. And `--no-sandbox` is
//! refused in `extra_args` (R-G11): the sandbox knob was struck, and an
//! operator flag is the door it would come back through.

use meclaw_cells::browser::{BrowserCellFactory, BrowserParams, ERROR_CODES};
use meclaw_colony::CellFactory;
use meclaw_core::serde_json::json;
use std::sync::Arc;

/// The params a working browser ships with, plus whatever `extra` adds.
fn params(extra: meclaw_core::JsonValue) -> meclaw_core::JsonValue {
    let mut v = json!({
        "chromium_path": "/usr/bin/chromium",
        // Required since OR-G54, so the shipped shape carries it.
        "sandbox": {
            "trust": "restricted",
            "network": "allow",
            "limits": {"memory_max_bytes": 2_000_000_000u64, "cpu_max_percent": 200,
                       "pids_max": 512}
        },
    });
    if let Some(obj) = extra.as_object() {
        for (k, val) in obj {
            v.as_object_mut()
                .expect("an object")
                .insert(k.clone(), val.clone());
        }
    }
    v
}

#[test]
fn the_defaults_are_the_numbers_the_contract_names() {
    let p = BrowserParams::parse(&params(json!({}))).expect("the shipped shape parses");
    assert_eq!(p.mount, "browser");
    assert_eq!(p.chromium_path, "/usr/bin/chromium");
    assert!(
        p.user_data_dir.is_none(),
        "the profile directory is the cell's to name, not the template's"
    );
    assert!(p.extra_args.is_empty());
    assert_eq!(p.max_pages, 8);
    assert_eq!(p.suspend_after_ms, 300_000);
    assert_eq!(p.throttle_after_ms, 30_000);
    assert_eq!(p.external_timeout_ms, 10_000);
    assert_eq!(p.startup_timeout_ms, 20_000);
    assert_eq!(p.screencast.format, "jpeg");
    assert_eq!(p.screencast.quality, 60);
    assert_eq!(p.screencast.max_width, 1280);
    assert_eq!(p.screencast.max_height, 800);
    assert_eq!(p.screencast.max_fps, 20);
    assert!(
        p.sandbox.is_some(),
        "and the ceiling is not a default anywhere: it was written (OR-G54)"
    );
}

#[test]
fn a_binary_that_was_not_named_is_a_refusal_and_not_a_search() {
    for shape in [json!({}), json!({"chromium_path": ""})] {
        let e = BrowserParams::parse(&shape).expect_err("no binary, no browser");
        assert!(
            e.contains("params.chromium_path"),
            "the refusal names the key: {e}"
        );
        assert!(
            e.contains("package"),
            "and says where a browser comes from — there is no search path \
             and no cache of somebody's tool to look in (OR-G9): {e}"
        );
    }
}

#[test]
fn the_flags_the_cell_owns_are_not_an_operators_to_write() {
    for flag in [
        "--headless=old",
        "--remote-debugging-port=9222",
        "--remote-debugging-pipe",
        "--user-data-dir=/tmp/elsewhere",
    ] {
        let e = BrowserParams::parse(&params(json!({"extra_args": [flag]})))
            .expect_err("a flag the cell owns is not an operator's");
        assert!(
            e.contains("params.extra_args") && e.contains(flag.split('=').next().unwrap()),
            "the refusal names the flag: {e}"
        );
    }
    let e = BrowserParams::parse(&params(json!({"extra_args": ["--no-sandbox"]})))
        .expect_err("the struck knob does not come back as a flag");
    assert!(
        e.contains("--no-sandbox") && e.contains("sandbox"),
        "and this one says why: {e}"
    );
}

#[test]
fn a_key_nobody_knows_is_a_typo_and_says_so() {
    let e = BrowserParams::parse(&params(json!({"max_page": 4})))
        .expect_err("a closed surface is closed");
    assert!(
        e.contains("max_page") && e.contains("max_pages"),
        "the refusal names the typo and the keys it could have meant: {e}"
    );
}

#[test]
fn the_sandbox_block_is_the_substrates_and_its_keys_are_closed() {
    let good = params(json!({"sandbox": {
        "trust": "restricted",
        "network": "allow",
        "limits": {"memory_max_bytes": 2_000_000_000u64, "cpu_max_percent": 200, "pids_max": 512}
    }}));
    let p = BrowserParams::parse(&good).expect("the shipped cap parses");
    assert!(p.sandbox.is_some(), "the cap reaches the cell");
    let e = BrowserParams::parse(&params(json!({"sandbox": {
        "trust": "restricted",
        "limits": {"pids_max": 512},
        "seatbelt": true
    }})))
    .expect_err("a sixth key is a boot error like everywhere else");
    assert!(e.contains("seatbelt"), "{e}");
    // And there is no knob beside it: the only way to say anything about the
    // sandbox is the substrate's own block (R-G11).
    let e = BrowserParams::parse(&params(json!({"sandbox_mode": "off"})))
        .expect_err("there is no second way to talk about the sandbox");
    assert!(e.contains("sandbox_mode"), "{e}");
}

#[test]
fn the_factory_validates_through_the_same_parser_it_spawns_through() {
    let factory = Arc::new(BrowserCellFactory::default());
    assert_eq!(factory.type_name(), "browser");
    assert!(
        factory.owns_schema(),
        "the pages table is this type's own code, so no seeder goes near it"
    );
    factory
        .validate_params(&params(json!({})))
        .expect("the shipped shape validates");
    let e = factory
        .validate_params(&json!({}))
        .expect_err("and the refusal is the parser's own");
    assert_eq!(
        e,
        BrowserParams::parse(&json!({})).expect_err("the same refusal"),
        "one parser, one sentence"
    );
}

#[test]
fn the_error_codes_are_nine_and_closed() {
    assert_eq!(
        ERROR_CODES,
        &[
            "invalid_input",
            "unknown_page",
            "too_many_pages",
            "spawn_failed",
            "startup_timeout",
            "browser_crashed",
            "navigate_failed",
            "cdp_timeout",
            "client_too_slow",
        ]
    );
}

#[test]
fn the_pages_table_is_the_cells_own_and_idempotent() {
    let conn = rusqlite::Connection::open_in_memory().expect("a database");
    meclaw_cells::browser::setup_browser_schema(&conn).expect("the ddl runs");
    meclaw_cells::browser::setup_browser_schema(&conn).expect("and runs again");
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('pages') ORDER BY cid")
        .expect("prepare")
        .query_map([], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("rows");
    assert_eq!(
        columns,
        vec![
            "page",
            "url",
            "context",
            "viewport_w",
            "viewport_h",
            "viewport_dpr",
            "mobile",
            "state",
            "opened_at",
            "updated_at",
        ]
    );
}

#[test]
fn a_browser_without_a_declared_ceiling_does_not_start() {
    // OR-G54. `params.sandbox` is optional everywhere else in the substrate,
    // and for this cell type that meant an `override_params` which simply left
    // it out got a browser — a process tree with a renderer per site — with no
    // cap at all, while the shipped `config.json` quietly made the common case
    // look safe. Contract § 2: `limits` stays required, and a cap that
    // cannot be enforced is fail-closed.
    let e = BrowserParams::parse(&json!({"chromium_path": "/usr/bin/chromium"}))
        .expect_err("a cell without a declared ceiling does not start");
    assert!(
        e.contains("params.sandbox") && e.contains("required"),
        "the refusal names the key and says it has no default: {e}"
    );
    assert!(
        e.contains("memory_max_bytes") && e.contains("trusted"),
        "and it writes out both answers, so an operator does not have to guess \
         the shape: {e}"
    );

    // And a `restricted` profile that caps nothing is the same refusal by
    // another road: it would be `trusted` under a name that says otherwise.
    let e = BrowserParams::parse(&json!({
        "chromium_path": "/usr/bin/chromium",
        "sandbox": {"trust": "restricted", "network": "allow"}
    }))
    .expect_err("a restricted profile with no cap is not a cap");
    assert!(
        e.contains("params.sandbox.limits"),
        "the refusal names the key that is missing: {e}"
    );

    // The two shapes that DO start: the shipped cap, and the escape hatch an
    // operator writes on purpose.
    BrowserParams::parse(&params(json!({}))).expect("the shipped cap starts");
    BrowserParams::parse(&json!({
        "chromium_path": "/usr/bin/chromium",
        "sandbox": {"trust": "trusted"}
    }))
    .expect("and so does a ceiling somebody declined in writing");

    // The factory refuses it in the same words, before a browser exists.
    let factory = Arc::new(BrowserCellFactory::default());
    let e = factory
        .validate_params(&json!({"chromium_path": "/usr/bin/chromium"}))
        .expect_err("validate_params is the same parser");
    assert!(e.contains("params.sandbox"), "{e}");
}
