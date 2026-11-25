//! A member without an app leaves no dead letter in its apps (GH #1598).
//!
//! GH #553 made the member carry the mutation receipt into `./apps` with a static
//! edge `. -> ./apps`, so that every app grown later hears that the graph moved. The
//! container ships empty and carries a lane only along the edges the mutation that
//! installs an app draws, so in a member that has no app hearing the receipt every
//! receipt ended in the container: one `hive_no_route` at `<member>/apps` per
//! mutation (measured in the orga lab on the soul2me set 0.73, twice at a member's
//! birth). Since GH #1598 the member draws no such edge; the builder draws the receipt
//! edge per app, from the member straight to that app, in the manifest that installs
//! it (`recipes`: `_app_member_edges`, `install_app`).
//!
//! What this file measures, on a colony whose `mutation_receipts` point at the member
//! stand-in `/alex`, which carries the SHIPPED member's receipt edges into its apps
//! container and has NO app that hears the receipt:
//!
//!   * a mutation commits (the outcome), and its receipt and the boot's are booked
//!     towards the member (the positive event);
//!   * once the colony settled, no dead letter names `/alex/apps`.
//!
//! The absence is read after the barrier (`Colony::dead_letters` settles first), never
//! after a wait on the clock.

// harness_fold: #[path = "support/display_colony.rs"]
#[allow(unused_imports)]
use crate::display_colony; // harness_fold: mod display_colony;

use display_colony::{Boot, boot_unborn, have_python, hop_of, library_ships, read_json, repo};
use meclaw_colony::MutationOutcome;
use meclaw_core::serde_json::{Value, json};

/// The member stand-in of the fixture and its apps container.
const MEMBER: &str = "/alex";
const APPS: &str = "/alex/apps";

/// The shipped member's edges for the receipt into its apps: every
/// `mutation_committed` door out of `.` into `./apps` or below it.
fn shipped_receipt_into_apps() -> Vec<Value> {
    let member = read_json(&repo("templates/member/config.json"));
    member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member draws edges")
        .iter()
        .filter(|e| {
            e["from"] == json!(".")
                && e["to"]
                    .as_str()
                    .is_some_and(|to| to == "./apps" || to.starts_with("./apps/"))
                && e["condition"]
                    .as_str()
                    .unwrap_or("")
                    .contains("'mutation_committed'")
        })
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_without_an_app_leaves_no_dead_letter_in_its_apps() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let shipped = shipped_receipt_into_apps();
    let colony = boot_unborn(Boot::default(), |tree| {
        tree.patch_json("colony.json", |v| {
            v["mutation_receipts"] = json!({"to": MEMBER});
        });
        // The member stand-in carries the shipped member's receipt edges into its apps.
        tree.patch_json("main/alex/config.json", |v| {
            v["params"]["graph"]["edges"]
                .as_array_mut()
                .expect("the fixture's member edges")
                .extend(shipped);
        });
        // The apps container as a member without an app hearing the receipt has it:
        // no edge inside it takes `mutation_committed`.
        tree.patch_json("main/alex/apps/config.json", |v| {
            v["params"]["graph"]["edges"] = json!([{
                "from": "./probe", "to": ".",
                "condition": "has(hop.route) && hop.route == 'view'"
            }]);
        });
    })
    .await;

    // A mutation that installs no app: one more edge inside the member's apps.
    let outcome = colony
        .apply_diff(
            APPS,
            json!({"add_edges": [{"from": "./probe", "to": ".",
                                  "condition": "has(hop.route) && hop.route == 'gh1598'"}]}),
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

    // The absence, after the barrier.
    let in_apps: Vec<String> = colony
        .dead_letters()
        .await
        .iter()
        .filter(|d| d.resolved_target.as_str().starts_with(APPS))
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
        in_apps,
        Vec::<String>::new(),
        "a member with no app hearing the receipt dead-letters it in its apps"
    );
    colony.shutdown().await;
}
