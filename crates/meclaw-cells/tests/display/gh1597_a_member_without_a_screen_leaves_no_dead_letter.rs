//! A member without a screen leaves no dead letter in its channels (GH #1597).
//!
//! GH #1590 made the member carry the mutation receipt into `./channels` with a
//! static edge `. -> ./channels`, so that a screen grown later wakes on it. The
//! container ships empty and carries a lane only along the edges the mutation that
//! grows a channel draws, so in a member that has no screen every receipt ended in
//! the container: one `hive_no_route` at `<member>/channels` per mutation (measured in
//! the orga lab, fourteen of them in one fresh colony). Since GH #1597 the member
//! draws no such edge; the builder draws the receipt edge per screen, from the member
//! straight to that screen, in the manifest that grows it (`recipes`, `_screen_member_edges`).
//!
//! The same held for the view of an app (the orga lab, d54s: `apps/presenter/stage`,
//! `apps/ambient/compose`): the member carried every view with a `channel_node` into
//! `./channels` on a static edge, whether a screen of that name stood there or not.
//! Since GH #1597 that edge is the screen's too: `./apps -> ./channels/<screen>`, guarded
//! on the screen's own name, drawn with the screen.
//!
//! What this file measures, on a colony whose `mutation_receipts` point at the member
//! stand-in `/alex`, which carries the SHIPPED member's edges for both lanes into a
//! child this tree has (`./apps`, `./channels`) and has NO screen:
//!
//!   * a mutation commits (the outcome), and its receipt and the boot's are booked
//!     towards the member (the positive event);
//!   * an app writes a view for the member's screen `display`, which this member
//!     does not have (the positive event: the app's emission is in the log);
//!   * once the colony settled, no dead letter names `/alex/channels`.
//!
//! The absence is read after the barrier (`Colony::dead_letters` settles first), never
//! after a wait on the clock.

// harness_fold: #[path = "support/display_colony.rs"]
#[allow(unused_imports)]
use crate::display_colony; // harness_fold: mod display_colony;

use display_colony::{Boot, boot_unborn, have_python, hop_of, library_ships, read_json, repo};
use meclaw_colony::MutationOutcome;
use meclaw_core::serde_json::{Value, json};

/// The member stand-in of the fixture and its channels container.
const MEMBER: &str = "/alex";
const CHANNELS: &str = "/alex/channels";

/// The shipped member's edges for the two lanes the member sends a screen: every
/// `mutation_committed` door out of `.` and every `view`/`withdraw` edge out of
/// `./apps`, into a child this fixture carries (`./apps`, `./channels` or below it).
fn shipped_screen_lanes() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    let into_fixture =
        |to: &str| to == "./apps" || to == "./channels" || to.starts_with("./channels/");
    member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member draws edges")
        .iter()
        .filter(|e| e["to"].as_str().is_some_and(into_fixture))
        .filter(|e| {
            let c = e["condition"].as_str().unwrap_or("");
            (e["from"] == json!(".") && c.contains("'mutation_committed'"))
                || (e["from"] == json!("./apps") && c.contains("'view'"))
        })
        .cloned()
        .collect()
}

/// The shipped member's edges from its channels down into its apps: the screen's
/// `event` and `receipt`, addressed by owner.
fn shipped_way_into_apps() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member draws edges")
        .iter()
        .filter(|e| e["from"] == json!("./channels") && e["to"] == json!("./apps"))
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_without_a_screen_leaves_no_dead_letter_in_its_channels() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let shipped = shipped_screen_lanes();
    let colony = boot_unborn(Boot::default(), |tree| {
        tree.patch_json("colony.json", |v| {
            v["mutation_receipts"] = json!({"to": MEMBER});
        });
        // The member stand-in carries the shipped member's edges for both lanes in
        // place of the fixture's own way from the apps into the channels.
        tree.patch_json("main/alex/config.json", |v| {
            let edges = v["params"]["graph"]["edges"]
                .as_array_mut()
                .expect("the fixture's member edges");
            edges.retain(|e| !(e["from"] == json!("./apps") && e["to"] == json!("./channels")));
            edges.extend(shipped);
            // And the shipped way down into the apps (`event`/`receipt` by owner): since
            // GH #1598 no receipt edge of the member ends in `./apps`, and an app no edge
            // reaches is not spawned.
            edges.extend(shipped_way_into_apps());
        });
        // The probe app leaves its container the way an installed app does
        // (`install_app`): on the shipped lane, stamped with the screen it draws on --
        // the member's `display`, which this member does not have. And it is reached
        // the way an installed app is, on its owner edge back in: since GH #1598 no
        // shipped member edge ends in `./apps` on the receipt, and a cell no edge
        // reaches is not spawned.
        tree.patch_json("main/alex/apps/config.json", |v| {
            v["params"]["graph"]["edges"] = json!([{
                "from": "./probe", "to": ".",
                "modifier": {
                    "set_hop": {"route": "hop.route == 'in_withdraw' ? 'withdraw' : 'view'"},
                    "set_context": {"channel_node": "'display'", "channel": "'display'"}
                }
            }, {
                "from": ".", "to": "./probe",
                "condition": "has(hop.route) && (hop.route == 'event' || hop.route == 'receipt') \
                              && has(hop.owner) && hop.owner.contains('/apps/probe/')"
            }]);
        });
    })
    .await;

    // A mutation that grows no screen: one more edge inside the member's apps.
    let outcome = colony
        .apply_diff(
            "/alex/apps",
            json!({"add_edges": [{"from": "./probe", "to": ".",
                                  "condition": "has(hop.route) && hop.route == 'gh1597'"}]}),
        )
        .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the mutation commits: {outcome:?}"
    );

    // The positive event: the boot's receipt and the mutation's are both booked towards
    // the member.
    colony
        .wait_until("both receipts are booked towards the member", async || {
            colony
                .log(Some(MEMBER))
                .await
                .iter()
                .filter(|row| hop_of(row)["route"] == "mutation_committed")
                .count()
                >= 2
        })
        .await;

    // A view of an app for the screen this member does not have, and the positive
    // event: the app answered.
    colony
        .app(json!({"do": "chat", "text": "hello", "at": 1}))
        .await;
    colony
        .wait_until("the app's view leaves the app", async || {
            colony
                .log(None)
                .await
                .iter()
                .any(|row| row.from_path == "/alex/apps/probe")
        })
        .await;

    // The absence, after the barrier.
    let in_channels: Vec<String> = colony
        .dead_letters()
        .await
        .iter()
        .filter(|d| d.resolved_target.as_str().starts_with(CHANNELS))
        .map(|d| {
            format!(
                "{} -> {} route={:?} reason={:?}",
                d.sender_path.as_str(),
                d.resolved_target.as_str(),
                d.message.headers.hop.get("route"),
                d.reason
            )
        })
        .collect();
    assert_eq!(
        in_channels,
        Vec::<String>::new(),
        "a member with no screen dead-letters its receipts or its views in its channels"
    );
    colony.shutdown().await;
}
