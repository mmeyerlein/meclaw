//! Issue #56 — validate-equals-spawn for `store` seed files.
//!
//! A `seed/<table>.jsonl` whose line 1 is not the schema header
//! (`{"schema":{…}}`) used to pass `meclaw --validate --strict` with exit 0:
//! the validate path never parsed seed files. The parse error then struck on
//! the store cell's wake, behind an `.expect`, taking a whole production colony
//! process down instead of failing one cell.
//!
//! These tests pin the validate half: everything statically parseable must
//! parse during validation, so a purely syntactic seed mistake is a named
//! bootstrap error with exit != 0 — plain `--validate` as well as
//! `--validate --strict`.

use meclaw_cli::{Cli, run};

fn cli_validate(root: &std::path::Path, strict: bool) -> Cli {
    Cli {
        root: root.into(),
        log: None,
        log_level: "warn".into(),
        log_filter: None,
        env: None,
        templates: None,
        rescan_templates: false,
        api: None,
        daemon: false,
        validate: true,
        strict,
        blobs: None,
        tokio_console: false,
        tokio_console_port: 6669,
        stdio_format: meclaw_cli::StdioFormat::Text,
    }
}

/// Root hive + one `store` cell declaring `items(id, name)`, seeded with the
/// supplied JSONL body.
fn write_store_colony(root: &std::path::Path, seed_body: &str) {
    std::fs::create_dir_all(root.join("main/notes/seed")).unwrap();
    std::fs::write(
        root.join("main/config.json"),
        br#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("main/notes/config.json"),
        br#"{"cell":{"type":"store"},
             "params":{"schema":{"items":{"id":"int","name":"text"}}},
             "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    )
    .unwrap();
    std::fs::write(root.join("main/notes/seed/items.jsonl"), seed_body).unwrap();
}

const VALID_SEED: &str = r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":"a"}
{"id":2,"name":"b"}
"#;

/// The reproducer: line 1 of the seed file removed. `--validate` must fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_missing_header_fails_validate() {
    let td = tempfile::TempDir::new().unwrap();
    write_store_colony(
        td.path(),
        r#"{"id":1,"name":"a"}
{"id":2,"name":"b"}
"#,
    );
    let err = run(cli_validate(td.path(), false))
        .await
        .expect_err("a seed file without its schema header must fail --validate");
    let msg = format!("{err:?}").to_lowercase();
    assert!(msg.contains("validate"), "expected a validate error: {msg}");
}

/// Same reproducer under `--strict` — the builder's G1 gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_missing_header_fails_validate_strict() {
    let td = tempfile::TempDir::new().unwrap();
    write_store_colony(
        td.path(),
        r#"{"id":1,"name":"a"}
"#,
    );
    assert!(
        run(cli_validate(td.path(), true)).await.is_err(),
        "--validate --strict must fail on a headerless seed file"
    );
}

/// A header that does not cover every declared column is a schema mismatch —
/// the check `load_seed_if_present` already performs at wake, pulled forward.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_column_mismatch_fails_validate() {
    let td = tempfile::TempDir::new().unwrap();
    write_store_colony(
        td.path(),
        r#"{"schema":{"id":"int"}}
{"id":1}
"#,
    );
    assert!(
        run(cli_validate(td.path(), false)).await.is_err(),
        "a seed header missing a declared column must fail --validate"
    );
}

/// A data line that is not valid JSON is statically parseable garbage too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seed_broken_data_line_fails_validate() {
    let td = tempfile::TempDir::new().unwrap();
    write_store_colony(
        td.path(),
        r#"{"schema":{"id":"int","name":"text"}}
{"id":1,"name":
"#,
    );
    assert!(
        run(cli_validate(td.path(), false)).await.is_err(),
        "a truncated JSON data row must fail --validate"
    );
}

/// Control: a well-formed seed file still validates clean — the new check must
/// not turn every seeded colony red.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_seed_passes_validate_strict() {
    let td = tempfile::TempDir::new().unwrap();
    write_store_colony(td.path(), VALID_SEED);
    run(cli_validate(td.path(), true))
        .await
        .expect("a well-formed seed file must keep --validate --strict green");
}

/// Control: no seed file at all stays legal (seeding is optional).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn absent_seed_passes_validate_strict() {
    let td = tempfile::TempDir::new().unwrap();
    write_store_colony(td.path(), VALID_SEED);
    std::fs::remove_file(td.path().join("main/notes/seed/items.jsonl")).unwrap();
    run(cli_validate(td.path(), true))
        .await
        .expect("an absent seed file must keep --validate --strict green");
}
