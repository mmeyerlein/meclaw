//! A born screen serves its pages before any app has written (GH #1590).
//!
//! A fresh display has no page map until its resident `compose` cell runs its
//! first pass, and that cell wakes only on a message. Before this lock the first
//! message was whatever app happened to write first, so a colony reported
//! `/health` ok and its portal `ready` while `/` and `/monitor` of its screen
//! still answered `404`. The mutation door's boot receipt (ruling O-0904-2,
//! `docs/cell-types.md` § `web`, the colony-view pattern) is now the first
//! message: the display hive takes `mutation_committed` onto `./compose`, and a
//! cold cell boots on it.
//!
//! What this file measures, on a booted colony whose `mutation_receipts` point at
//! the screen and where NO app writes anything:
//!
//!   * the receipt of the boot reaches `compose` (the positive event);
//!   * `/` (the switch) and `/monitor` then serve a page;
//!   * no app write reached the screen -- the page came from the receipt alone.
//!
//! Every wait is an event under the HANG mark of `meclaw_testing::deadline`.

// harness_fold: #[path = "support/display_colony.rs"]
#[allow(unused_imports)]
use crate::display_colony; // harness_fold: mod display_colony;

use display_colony::{
    Boot, MOUNT, SCREEN, boot_growing, boot_unborn, have_python, hop_of, library_ships, read_json,
    repo, screens,
};
use meclaw_colony::MutationOutcome;
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_born_screen_serves_its_pages_before_any_app_writes() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = boot_growing(Boot::default(), |tree| {
        tree.patch_json("colony.json", |v| {
            v["mutation_receipts"] = json!({"to": SCREEN});
        })
    })
    .await;

    // The positive event: the boot receipt reached the pass.
    colony
        .wait_until("the boot receipt reaches compose", async || {
            colony
                .to_child("compose")
                .await
                .iter()
                .any(|row| hop_of(row)["route"] == "mutation_committed")
        })
        .await;

    // Both routes serve a page (`page` waits for a success under the HANG mark). The
    // output first: only compose's patch draws it, so once it answers the switch is
    // compose's too and no longer the web cell's seed page.
    let _monitor = colony.page("monitor").await;
    let switch = colony.page("").await;
    assert!(
        switch.contains("monitor"),
        "the switch names the default output: {switch}"
    );
    assert_eq!(colony.status("monitor").await, 200);
    assert_eq!(colony.status("").await, 200);

    // Nothing else drew it: no app wrote onto the screen.
    assert_eq!(
        colony.writes_taken().await,
        Vec::<String>::new(),
        "an app write reached the screen, so the page proves nothing about the boot"
    );
    colony.shutdown().await;
}

/// The two screen declarations the builder renders for a member wish (`recipes`,
/// `member_devices`), as they ship: the screen in the member's channels (the second
/// declaration of the one manifest) and the member's own edges to it (the last, GH #1597).
fn rendered_screen() -> (Value, Value) {
    let recipes = repo("templates/builder/recipes/config.json");
    let member =
        read_json(&repo("examples/organism/grow-member.json"))["diff"]["add_nodes"][0]["template"]
            .clone();
    let wish = json!({"recipe": "grow_level", "request": "grow a member named alex",
                      "params": {"scope": "/os/orgs/acme", "level": "member",
                                 "name": "alex", "template": member}});
    let out = emit_all(
        &shipped_script(recipes.to_str().expect("utf-8 path")),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": wish.to_string()}],
        }),
    );
    let manifest = out
        .iter()
        .find(|m| m["header"]["operation"] == json!("recipe"))
        .expect("a member wish renders a manifest");
    let screen = manifest["manifest"][1].clone();
    assert_eq!(
        screen["diff"]["add_nodes"][0]["name"],
        json!("display"),
        "the second declaration grows the member's screen: {screen}"
    );
    let mut member_half = manifest["manifest"][3].clone();
    assert_eq!(
        member_half["scope"],
        json!("/os/orgs/acme/members/alex"),
        "the last declaration draws the member's edges to its screen: {member_half}"
    );
    // GH #1598: the same declaration carries the app's receipt edge
    // (`. -> ./apps/colony-view`); this colony grows no app, so only the screen's
    // edges are applied.
    let edges = &mut member_half["diff"]["add_edges"];
    let screen_edges: Vec<Value> = edges
        .as_array()
        .expect("the member's edges")
        .iter()
        .filter(|e| {
            e["to"]
                .as_str()
                .is_some_and(|t| t.starts_with("./channels/"))
        })
        .cloned()
        .collect();
    assert_eq!(
        screen_edges.len(),
        2,
        "the member's two edges to its screen: {edges}"
    );
    *edges = Value::Array(screen_edges);
    (screen, member_half)
}

/// OR-d54s.schirmstart.3 -- the same claim over a REAL birth: the screen is grown by
/// the declaration the builder renders for a member wish, the receipt of that
/// mutation enters the member level, and only the shipped edges carry it on -- the
/// member's `. -> ./channels/display`, which the builder draws with the screen (GH #1597).
/// No message is sent to the screen directly, and no app writes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screen_the_builder_grows_serves_its_pages_before_any_app_writes() {
    if !library_ships()
        || !have_python()
        || !repo("templates/builder/recipes/config.json").is_file()
    {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    // The member level's receipt doors, read off the shipped member template: every
    // `. -> <child>` edge on `mutation_committed` whose child this tree carries.
    let member = read_json(&repo("templates/member/config.json"));
    let doors: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member draws edges")
        .iter()
        .filter(|e| e["from"] == json!("."))
        .filter(|e| {
            e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'mutation_committed'"))
        })
        .filter(|e| e["to"] == json!("./apps") || e["to"] == json!("./channels"))
        .cloned()
        .collect();
    let colony = boot_unborn(Boot::default(), |tree| {
        tree.patch_json("colony.json", |v| {
            v["mutation_receipts"] = json!({"to": "/alex"});
        });
        tree.patch_json("main/alex/config.json", |v| {
            let edges = v["params"]["graph"]["edges"]
                .as_array_mut()
                .expect("the fixture's member edges");
            edges.extend(doors);
        });
        tree.copy_tree(&repo("templates/display"), "templates/display");
    })
    .await;

    // The birth: the builder's declaration, its mount and outputs set the way a
    // colony's set (`06-screen.json`) sets them.
    let (mut screen, member_half) = rendered_screen();
    let over = &mut screen["diff"]["add_nodes"][0]["override_params"];
    over["web"]["mount"] = json!(MOUNT);
    over["compose"] = json!({"screens": screens(), "default_screen": "monitor"});
    let outcome = colony.birth("/alex/channels", screen["diff"].clone()).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the builder's screen declaration commits: {outcome:?}"
    );
    // The member's half, as the manifest rolls it forward behind the screen: its
    // receipt is the one that reaches the screen.
    let outcome = colony
        .apply_diff("/alex", member_half["diff"].clone())
        .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the member's edges to its screen commit: {outcome:?}"
    );

    // The positive event: the receipt of the birth reached the pass, over the edges.
    colony
        .wait_until("the birth's receipt reaches compose", async || {
            colony
                .to_child("compose")
                .await
                .iter()
                .any(|row| hop_of(row)["route"] == "mutation_committed")
        })
        .await;

    let _monitor = colony.page("monitor").await;
    let switch = colony.page("").await;
    assert!(
        switch.contains("monitor"),
        "the switch names the default output: {switch}"
    );
    assert_eq!(colony.status("monitor").await, 200);
    assert_eq!(colony.status("").await, 200);
    assert_eq!(
        colony.writes_taken().await,
        Vec::<String>::new(),
        "an app write reached the screen, so the page proves nothing about the birth"
    );
    colony.shutdown().await;
}
