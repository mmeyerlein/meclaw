//! D58 target (contract points 2 and 5, OR-d58-2.target.69): the registry's
//! push after the birth, for a test colony that has no registry.
//!
//! In a grown colony the registry pushes every model cell its package after
//! the birth, and the cell reports it -- its row and its window profile --
//! to the composite's curator before the first turn. A test colony boots
//! from the filesystem without a registry, so no cell reports, and the
//! curator cuts the first call with the shipped values of the instance (the
//! question for the profile goes beside the call, never instead of it,
//! OR-d58-2.target.15). [`report_packages`] sends that push through the
//! composite's door (`in_model`, addressed by `hop.subscriber`, the empty
//! params slot) to every model cell with `package_line` on, and returns
//! once the curator holds each cell's row.

use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::{ColonyHandle, wait};

fn read_config(p: &std::path::Path) -> Value {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| meclaw_core::serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

/// The keys of the curator's state that hold a catalogue row (`catalog:<cell>`),
/// read-only; no file or no table yet is "none yet".
fn catalog_keys(ledger: &std::path::Path) -> Vec<String> {
    if !ledger.is_file() {
        return Vec::new();
    }
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(ledger, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut st) = conn.prepare("SELECT key FROM state WHERE key LIKE 'catalog:%'") else {
        return Vec::new();
    };
    st.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// The model cells of the composite at `mount` (colony path, e.g.
/// `/talky`) under the colony directory `main` that report their package:
/// their `config.json` on disk says `package_line` true.
pub fn reporting_cells(main: &std::path::Path, mount: &str) -> Vec<String> {
    let dir = main.join(mount.trim_start_matches('/'));
    let mut cells: Vec<String> = std::fs::read_dir(&dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .filter(|e| {
                    read_config(&e.path().join("config.json"))["params"]["package_line"]
                        == json!(true)
                })
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    cells.sort();
    cells
}

/// The registry's push to every reporting model cell of the composite at
/// `mount` (colony directory `main`); waits until its curator
/// (`<mount>/curator/ledger`) holds the row of each.
pub async fn report_packages(h: &ColonyHandle, main: &std::path::Path, mount: &str) {
    let cells = reporting_cells(main, mount);
    assert!(
        !cells.is_empty(),
        "{mount}: no model cell with `package_line` under {}",
        main.display()
    );
    for cell in &cells {
        let hop: Map<String, Value> = json!({"route": "in_model",
                                             "subscriber": format!("{mount}/{cell}")})
        .as_object()
        .cloned()
        .expect("an object");
        h.send(
            MessageBuilder::new(Path::new(mount))
                .hop(hop)
                .body(Body::Inline(json!({"system": {}, "params": {}})))
                .ttl(400)
                .build(),
        )
        .await;
    }
    let ledger = main
        .join(mount.trim_start_matches('/'))
        .join("curator/ledger/cell.db");
    for cell in &cells {
        let key = format!("catalog:{cell}");
        wait::until_with(
            &format!("the curator of {mount} holds the package of {cell}"),
            || {
                let got = catalog_keys(&ledger).contains(&key);
                async move { got }
            },
            || format!("catalogue keys: {:?}", catalog_keys(&ledger)),
        )
        .await;
    }
}
