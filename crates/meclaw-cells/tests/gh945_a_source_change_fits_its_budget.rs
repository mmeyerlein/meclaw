//! GH #945 -- an index job fits its budget, whatever the size of the file.
//!
//! The chain-length lock of the door `source_changed` (the seam table of
//! `docs/meclaw-overview.md` § Edge model, row "Door": an index job in a
//! graph space; the pattern of `gh933`'s close pass and of
//! `gh929_every_budget_segment_fits_its_reserve.rs`). The member layout
//! (`support/graph_space_colony.rs`) indexes one python file with as many
//! functions as the file space announces at most (`nodes_max`, read off the
//! shipped template, 2000 where it names none). The graph space pulls it with
//! a constant number of requests -- the large `outline` answer travels as one
//! blob -- and writes it in two bundles.
//!
//! Measured at the receiver, in the colony's own `message_log`: for every
//! delivery to `/graph-space/store`, the parent chain back to the delivery
//! behind the restoring door (`source_changed` entering the graph space). Its
//! longest segment spends at most `MESSAGE_DEFAULT_TTL - RESERVE` routing
//! decisions. One line `gh945 index-segment: start=<ttl> end=<ttl> used=<n>
//! nodes=<n> blobs=<n>` is the evidence.
//!
//! Free of a paid provider by construction. Guarded like every
//! template-reading test (GH #49).

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/graph_space_colony.rs"]
mod space;

use meclaw_core::MESSAGE_DEFAULT_TTL;
use meclaw_core::serde_json::Value;
use space::{Layout, Logged, create, graph_db, quiet, rows, wait_for};

/// What a segment keeps back from the colony budget (OR-BD-11), as in gh929.
const RESERVE: u32 = 16;
const SEGMENT_MAX: i64 = (MESSAGE_DEFAULT_TTL - RESERVE) as i64;

/// The most nodes the shipped file space announces for one file: the first
/// param named `nodes_max` of any of its cells, else 2000.
fn nodes_max() -> usize {
    let dir = space::repo("templates/file-space");
    let mut found = None;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for p in paths {
            let cfg = p.join("config.json");
            if !cfg.is_file() {
                continue;
            }
            let v: Value = space::read_json(&cfg);
            if let Some(n) = v["params"]["nodes_max"].as_u64() {
                found = Some(n as usize);
                break;
            }
        }
    }
    found.unwrap_or(2000)
}

fn blobs(root: &std::path::Path) -> usize {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    conn.query_row(
        "SELECT count(*) FROM message_log WHERE to_path LIKE '/graph-space%' AND body_kind = 'blob'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map_or(0, |n| n as usize)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_index_job_of_the_largest_file_fits_its_reserve() {
    if !space::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let n = nodes_max();
    let text: String = (0..n)
        .map(|i| format!("def f{i}():\n    return {i}\n\n\n"))
        .collect();
    let stubs = space::stubs().await;
    let td = tempfile::TempDir::new().expect("tempdir");
    space::build(&td, Layout::Member, &stubs);
    let root = td.path().to_path_buf();
    let (h, mut ports) = space::boot(&td).await;
    let db = graph_db(&root);

    let (file, _) = create(&h, &mut ports, &root, "/big/many.py", &text).await;
    let sql = format!("SELECT count(*) FROM nodes WHERE source = '{file}'");
    let want = n.to_string();
    wait_for(&root, "every node of the large file is indexed", || {
        rows(&db, &sql).first().and_then(|r| r.first()).cloned() == Some(want.clone())
    })
    .await;
    quiet(&root).await;
    let log = space::message_log(&root);
    let dead = space::dead_letters(&root);
    let blobbed = blobs(&root);
    h.shutdown().await;

    let seam = |r: &Logged| {
        r.route() == "source_changed"
            && r.to.starts_with("/graph-space")
            && !r.from.starts_with("/graph-space")
    };
    let mut worst: Option<(i64, i64, Vec<String>)> = None;
    let mut chains = 0usize;
    for closing in log.iter().filter(|r| r.to == "/graph-space/store") {
        let Some(chain) = space::chain_to_seam(&log, closing, seam) else {
            continue;
        };
        chains += 1;
        let start = chain[0].ttl;
        let used = start - closing.ttl;
        if worst.as_ref().is_none_or(|(_, u, _)| used > *u) {
            worst = Some((start, used, chain.iter().map(|r| r.say()).collect()));
        }
    }
    let (start, used, chain) =
        worst.expect("a store delivery of the graph space traced to its door");
    eprintln!(
        "gh945 index-segment: start={start} end={} used={used} nodes={n} blobs={blobbed} chains={chains}",
        start - used
    );
    assert!(
        used <= SEGMENT_MAX,
        "an index job spends {used} of {SEGMENT_MAX} routing decisions (reserve {RESERVE}):\n{}",
        chain.join("\n")
    );
    assert!(dead.is_empty(), "dead letters: {dead:#?}");
}
