//! GH #1558 -- a presenter whose decider has no calibration takes its fallback.
//!
//! The llm cell refuses to answer on a raw scale: without `decision_calibration`
//! its `decisions` lane answers `decision_uncalibrated` before any request. The
//! presenter reads that answer like any failed decide call: the turn's journal row
//! says `error`, nothing leaves towards the screen or the app, and the answer stays
//! text. The route's twin is `gh1558_the_route_falls_back_on_an_uncalibrated_decider`
//! (review jevr_o1-r2, Minor 5: the presenter's fallback had no pin with the new code).
//!
//! The decider is born from the shipped template without a curve, as a colony's is
//! before the registry pushes its catalogue row.

#[path = "support/display_colony.rs"]
mod display_colony;
use crate::display_colony::presenter_colony;

use display_colony::hop_of;
use meclaw_core::serde_json::json;
use presenter_colony::{Dials, PRESENTER, boot, guard, stage_routes};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_uncalibrated_decider_leaves_the_turn_as_text() {
    if !guard("an_uncalibrated_decider_leaves_the_turn_as_text") {
        return;
    }
    let s = boot(Dials {
        calibrated: false,
        ..Dials::default()
    })
    .await;
    s.install_sample().await;

    s.turn("k1", "show me a sample").await;
    assert_eq!(s.journal_of("k1").await["fallback"], json!("error"));

    // The answer that reached `stage` is the lane's refusal, by name.
    let decide = format!("{PRESENTER}/decide");
    let stage = format!("{PRESENTER}/stage");
    let verdict =
        s.c.log(Some(&stage))
            .await
            .iter()
            .filter(|r| r.from_path == decide && r.to_path == stage)
            .map(hop_of)
            .find(|hop| hop["show_id"] == "k1")
            .expect("the decider answered the turn");
    assert_eq!(
        verdict["error_code"],
        json!("decision_uncalibrated"),
        "{verdict}"
    );

    let routes = stage_routes(&s).await;
    assert!(
        !routes.iter().any(|r| r == "view" || r == "in_show"),
        "nothing may leave towards the screen or the app: {routes:?}"
    );
    s.c.shutdown().await;
}
