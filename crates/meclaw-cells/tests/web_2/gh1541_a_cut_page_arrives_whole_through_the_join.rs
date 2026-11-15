//! GH #1541: a page above the cut arrives whole in the browser -- the GET
//! carries its first slots, the join every slot of the picture.
//!
//! Since GH #1002 a page with more than `CUT_ABOVE_BYTES` (320 KiB) of slot
//! HTML is cut, and that holds for EVERY page (ruling OR-54g-142): there is
//! no display branch, `RenderedPage::is_large` is the one switch. The
//! colony-view picture of a colony with the full talky and its tools crossed
//! the line -- the GET of the screen lost the picture's tail while the join
//! still carried it. What this file pins is the half the cut leans on: the
//! browser's road (GET, then the join's head and pieces, merged as the client
//! merges them) shows the whole picture, not one slot missing.
//!
//! The page is the city of GH #1002 (`support/gh1002_city.rs`), served by a
//! `web` cell exactly as a display's screen is: same cell, same page map,
//! same cut. Read through the shared browser reader
//! (`support/gh1541_browser.rs`), which `gh553` reads the colony view with.

// harness_fold: #[path = "support/gh1002_city.rs"]
#[allow(unused_imports)]
use crate::city_fixture; // harness_fold: mod city_fixture;
// harness_fold: #[path = "support/gh1541_browser.rs"]
#[allow(unused_imports)]
use crate::gh1541_browser; // harness_fold: mod gh1541_browser;

use city_fixture::{MOUNT, seed_city, start};
use meclaw_core::serde_json::json;
use tempfile::TempDir;

/// Every `data-k="<id>"` the HTML carries, in order: one per drawn object.
fn objects(html: &str) -> Vec<String> {
    html.split("data-k=\"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh1541_a_page_above_the_cut_arrives_whole_through_get_and_join() {
    let td = TempDir::new().expect("tempdir");
    let dir = td.path().join("web");
    std::fs::create_dir_all(&dir).expect("cell dir");
    // The plan's city without its stylesheet weight: the stylesheet is an
    // asset, not slot HTML, and plays no part in the cut.
    seed_city(&dir, 560, 30, 128, 1024);

    // The page as the cell holds it: above the cut, so its GET is cut.
    let conn = rusqlite::Connection::open_in_memory().expect("memory db");
    meclaw_cells::web::db::setup_web_schema(&conn).expect("schema");
    meclaw_cells::web::seed::load_seed_if_present(&conn, &dir).expect("seed");
    let page = meclaw_cells::web::render::materialize(&conn, "/").expect("materialize");
    let whole = page.rendered_body();
    let cut_above = meclaw_cells::web::render::CUT_ABOVE_BYTES;
    assert!(
        page.slot_bytes() > cut_above,
        "precondition: the page carries {} bytes of slot HTML, not above the cut at {cut_above}",
        page.slot_bytes()
    );
    let drawn = objects(&whole);
    assert!(
        drawn.len() > 4000,
        "precondition: the city draws its objects"
    );

    let live = start(&dir, json!({})).await;
    let seen = gh1541_browser::read_page(live.port, MOUNT)
        .await
        .expect("the page answers its GET and its join");
    println!(
        "NOTE gh1541 slot_bytes={} cut_above={} get={} shown={} whole={} join_frames={}",
        page.slot_bytes(),
        cut_above,
        seen.get.len(),
        seen.html.len(),
        whole.len(),
        seen.frames
    );
    assert_eq!(seen.status, 200, "the page is served");
    assert!(
        objects(&seen.get).len() < drawn.len(),
        "precondition: the GET of a page above the cut carries its first slots only"
    );
    assert!(
        seen.frames > 1,
        "precondition: the join of a page above the cut comes in pieces"
    );

    // The claim: through GET and join, not one object of the picture is missing.
    let shown: std::collections::BTreeSet<String> = objects(&seen.html).into_iter().collect();
    let missing: Vec<&String> = drawn.iter().filter(|k| !shown.contains(*k)).collect();
    assert!(
        missing.is_empty(),
        "{} of {} objects never arrive through GET and join, first {:?}",
        missing.len(),
        drawn.len(),
        missing.iter().take(5).collect::<Vec<_>>()
    );
    assert_eq!(
        seen.html, whole,
        "the browser builds a different page from the join than the cell holds"
    );
}
