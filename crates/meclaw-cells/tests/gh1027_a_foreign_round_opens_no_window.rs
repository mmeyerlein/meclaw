//! GH #1027 -- a turn of a foreign round opens no window on the member's screen.
//!
//! Measured (display before-run, scene "a guest asks"): a turn whose
//! round was `["agent:…", "person:guest"]` -- the member not in the conversation -- went
//! to the decider, and its sure verdict opened `show-calendar` with the working hint on
//! all three outputs of the member; only the data sets were gated afterwards (`no_data`).
//! The open window alone told the screen that a guest asked about the member's calendar.
//!
//! The rule now: the turn's round is checked against the screen's round BEFORE anything
//! happens. A round that does not cover the screen's asks no decider and opens nothing;
//! its journal row says `foreign_round`. Proven at the receiver with a sentinel: `stage`
//! handles one turn after the other, so when the decider's FIRST request is the member's
//! later turn, the guest's turn was handled completely -- and nothing of it reached the
//! screen.

#[path = "support/display_colony.rs"]
mod display_colony;
use crate::display_colony::presenter_colony;

use display_colony::{calls_of, hop_of};
use meclaw_colony::api_dto::MessageLogDto;
use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, PRESENTER, boot, decision, guard, guest_or_before, warm};

/// Whether one patch bundle creates an object whose id carries `needle`.
fn creates(patch: &[Value], needle: &str) -> bool {
    patch.iter().any(|c| {
        c["op"] == "object.create" && c["id"].as_str().is_some_and(|id| id.contains(needle))
    })
}

/// Pick (4): a patch at `web` (`web` is its path) that creates an object `show-sample*`.
fn creates_show_sample_at(r: &MessageLogDto, web: &str) -> bool {
    r.to_path == web
        && hop_of(r)["route"] == "patch"
        && calls_of(r).is_some_and(|c| creates(&c, "show-sample"))
}

/// Pick (5): a row at the screen whose body carries `show-sample`.
fn carries_show_sample(r: &MessageLogDto) -> bool {
    r.to_path.starts_with(display_colony::SCREEN)
        && r.body_payload
            .as_deref()
            .is_some_and(|b| b.contains("show-sample"))
}

/// Every row at the screen whose body the log does not hold inline, as `id from -> to`.
///
/// Re-check tv-bn M7b11 M9: a body over `blob_inline_max_bytes` (default 65 536) is logged
/// as `body_kind = "blob"` with the blob's id as `body_payload` (`build_message_log_row_from_msg`
/// in `colony.rs`), so picks (4) and (5) -- they read the inline body -- choose no such row
/// and judge none: fail open. The fixture has no blob reader, and the colony's blob store
/// is no business of a test, so such a row is reported (fail closed) instead of resolved.
fn blobs_at_the_screen(rows: &[MessageLogDto]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.to_path.starts_with(display_colony::SCREEN) && r.body_kind != "inline")
        .map(|r| {
            format!(
                "{} {} -> {} ({})",
                r.id, r.from_path, r.to_path, r.body_kind
            )
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_of_a_foreign_round_opens_no_window() {
    if !guard("a_turn_of_a_foreign_round_opens_no_window") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    // The guest's turn: the member is not part of the round.
    s.turn_in(
        "g1",
        "show me her sample",
        json!(["agent:g1", "person:guest"]),
    )
    .await;
    // Re-check tv-bn M7b5 C-M1: "the guest's turn reached the screen as a window" is judged
    // here, before the member's turn holds the decider -- `journal_of` and `drawn` settle,
    // and a settle under a held request hangs. The guest's turn asks no decider, so the
    // colony comes to rest with it handled completely; after the member's verdict
    // `show-sample` stands by right, so the claim has no later place.
    let row = s.journal_of("g1").await;
    assert_eq!(row["fallback"], json!("foreign_round"), "{row}");
    assert!(
        !row.to_string().contains("her sample"),
        "the journal carries no text: {row}"
    );
    assert!(
        !s.drawn("show-sample").await,
        "the guest's turn reached the screen as a window"
    );
    // The sentinel: the member's own turn on the same screen.
    s.turn_in(
        "m1",
        "show me my sample",
        json!(["agent:g1", "member:alex"]),
    )
    .await;

    let first = s.ask().await;
    assert_eq!(
        first.json()["state"],
        json!("show me my sample"),
        "the decider was asked about the guest's turn"
    );
    // And the member's turn on the same screen still opens its window.
    first.release(decision(&[
        ("topic", "sample", 0.9),
        ("sample.lead", "rows", 0.9),
        ("sample.also", "none", 0.9),
    ]));
    s.wait_drawn("show-sample-hint").await;
    // tv-bn M7b8 (TV-F7, 3/5 red at CPUQuota 80): the window's fresh second (§ 4.34)
    // strikes one second after its pass, and the strike's pass carries the window to the
    // screen again. The claims below judged that pass only when the run outlasted the
    // second; awaited here, they judge it in every run.
    s.c.wait_until(
        "the fresh second of the member's window carries it to the screen again",
        || async {
            let rows = s.c.log(Some(display_colony::SCREEN)).await;
            let strikes: Vec<&str> = rows
                .iter()
                .filter(|r| hop_of(r)["route"] == "in_tick")
                .map(|r| r.id.as_str())
                .collect();
            rows.iter().any(|r| {
                r.parent_message_id
                    .as_deref()
                    .is_some_and(|p| strikes.contains(&p))
                    && r.body_payload
                        .as_deref()
                        .is_some_and(|b| b.contains("show-sample"))
            })
        },
    )
    .await;

    // Re-check tv-bn M7b4 B-M1: judged while the member's request was held, "no view for
    // the guest" had the decide call as its fence, which orders none of the screen path.
    // Released and settled, every view that left stage descends from the member's verdict
    // -- a window of the guest's turn would have none among its ancestors.
    let stage = format!("{PRESENTER}/stage");
    let (views, early) = s
        .before_the_verdict("m1", |r| {
            r.from_path == stage && hop_of(r)["route"] == "view"
        })
        .await;
    assert!(views > 0, "the member's turn opens its window");
    assert!(
        early.is_empty(),
        "a view left for the guest's turn: {early:#?}"
    );
    // Re-check tv-bn M7b5 C-M1: the views of stage are one lane to the screen; a window of
    // the guest's turn on any other lane passed the claim above. So every row that reached
    // the screen carrying `show-sample`, and every patch at `web` that creates an object
    // `show-sample*`, descends from the member's verdict as well.
    let web = format!("{}/web", display_colony::SCREEN);
    let (created, early) = s
        .before_the_verdict("m1", |r| creates_show_sample_at(r, &web))
        .await;
    assert!(created > 0, "the member's turn creates its window at web");
    assert!(
        early.is_empty(),
        "a window `show-sample*` was created for the guest's turn: {early:#?}"
    );
    // tv-bn M7b9/M7b11: the curator is stateful -- a later pass that another event started
    // (a stroke the `warm` window ordered) carries `show-sample` too and descends from no
    // verdict. The claim on every such row: none stands before the member's verdict, none
    // descends from the guest's turn, and every cause of it is named -- the member's
    // verdict or the write of `warm` -- or it is reported (`of_the_turn_or_before`, fail
    // closed; pinned on hand-made logs below).
    let (reached, early) = s
        .of_the_turn_or_before("g1", "m1", &["warm"], carries_show_sample)
        .await;
    // Re-check tv-bn M7b11 M9: the picks of (4) and (5) read the inline body, so a row at
    // the screen whose body the log holds as a blob would be overlooked. The log only
    // grows: read after both picks, it holds every row they saw.
    let blobs = blobs_at_the_screen(&s.c.log(Some(display_colony::SCREEN)).await);
    assert!(
        blobs.is_empty(),
        "fail closed: a row at the screen holds its body as a blob, which the picks (4) and \
         (5) cannot read: {blobs:#?}"
    );
    assert!(reached > 0, "the member's window reaches the screen");
    assert!(
        early.is_empty(),
        "`show-sample` reached the screen for the guest's turn: {early:#?}"
    );
    s.c.shutdown().await;
}

/// A turn without any round on a screen with one is no member turn either: fail-closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_without_a_round_opens_no_window_on_a_screen_with_one() {
    if !guard("a_turn_without_a_round_opens_no_window_on_a_screen_with_one") {
        return;
    }
    let mut s = boot(Dials {
        screen_audience: json!(["member:alex"]),
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    s.turn_in("n1", "show me a sample", json!(null)).await;
    s.turn_in("m1", "show me my sample", json!(["member:alex"]))
        .await;
    let first = s.ask().await;
    assert_eq!(first.json()["state"], json!("show me my sample"));
    // Re-check tv-bn M7b4 B-M1: released first, so the claim reads the colony at rest; the
    // member's verdict is `none`, so no window may stand at all.
    first.release(decision(&[("topic", "none", 0.9)]));
    let row = s.journal_of("n1").await;
    assert_eq!(row["fallback"], json!("foreign_round"), "{row}");
    assert!(!s.drawn("show-sample").await);
    s.c.shutdown().await;
}

/// One hand-made log row: `id` sent `from -> to` with `hop` and `body`, under `parent`.
fn row(
    id: &str,
    parent: Option<&str>,
    from: &str,
    to: &str,
    hop: Value,
    body: Value,
) -> MessageLogDto {
    MessageLogDto {
        id: id.to_string(),
        trace_id: id.to_string(),
        parent_message_id: parent.map(str::to_string),
        correlation_id: None,
        ttl: 24,
        from_path: from.to_string(),
        to_path: to.to_string(),
        reply_to: None,
        headers_json: json!({ "hop": hop }).to_string(),
        body_kind: "inline".to_string(),
        body_payload: Some(body.to_string()),
        created_at: 0,
    }
}

/// Re-check tv-bn M7b9 A-M3 / C-M1: the screen claim of the test above is fail closed. On a
/// hand-made log -- the guest's turn and its journal row, the write of `warm` and the
/// second it ordered, the member's turn and verdict -- the rows the member's verdict or
/// the `warm` second carried are clean, and each mutant is reported: a row of the guest's
/// turn (straight, through a second it ordered, through one of two orders), a row whose
/// cause cannot be shown (a stroke without an order, a parent the log does not hold, a
/// root that is no named cause, a cycle), a row before the verdict, and a log in which the
/// search cannot see the guest.
#[test]
fn a_guest_cause_or_an_untraced_row_is_reported() {
    let screen = display_colony::SCREEN;
    let web = format!("{screen}/web");
    let compose = format!("{screen}/compose");
    let clock = format!("{screen}/clock");
    let (decide, stage) = (format!("{PRESENTER}/decide"), format!("{PRESENTER}/stage"));
    let turn = |id: &str, t: &str| {
        row(
            id,
            None,
            "@external",
            PRESENTER,
            json!({"route": "turn", "turn_id": t}),
            json!({}),
        )
    };
    let strike = |id: &str, sid: &str| {
        row(
            id,
            None,
            "@external",
            &compose,
            json!({"route": "in_tick", "schedule_id": sid}),
            json!({}),
        )
    };
    let order = |id: &str, parent: &str, sid: &str| {
        row(
            id,
            Some(parent),
            &compose,
            &clock,
            json!({"route": "schedule"}),
            json!({"op": "add", "schedule_id": sid}),
        )
    };
    let shown = |id: &str, parent: Option<&str>| {
        row(
            id,
            parent,
            &compose,
            &web,
            json!({"route": "patch"}),
            json!({"id": "show-sample"}),
        )
    };
    let base = vec![
        turn("g", "g1"),
        row(
            "gj",
            Some("g"),
            PRESENTER,
            "/alex/store",
            json!({"route": "journal"}),
            json!({"fallback": "foreign_round"}),
        ),
        row(
            "w",
            None,
            "@external",
            screen,
            json!({"route": "in_view"}),
            json!({"view_id": "warm"}),
        ),
        order("wo", "w", "s-warm"),
        turn("m", "m1"),
        row(
            "v",
            Some("m"),
            &decide,
            &stage,
            json!({"route": "decide", "show_id": "m1"}),
            json!({}),
        ),
        shown("r-verdict", Some("v")),
        strike("s1", "s-warm"),
        shown("r-warm", Some("s1")),
    ];
    let pick = |r: &MessageLogDto| r.to_path == web;
    let judge = |rows: &[MessageLogDto]| guest_or_before(rows, "g1", "m1", &["warm"], pick);
    let mut wrong = Vec::new();
    let (picked, apart) = judge(&base);
    if picked != 2 || !apart.is_empty() {
        wrong.push(format!("the clean log: {picked} picked, {apart:?}"));
    }
    let with = |more: Vec<MessageLogDto>| -> Vec<MessageLogDto> {
        base.iter().cloned().chain(more).collect()
    };
    let cases: Vec<(&str, Vec<MessageLogDto>, &str)> = vec![
        (
            "a row of the guest's turn",
            with(vec![shown("x", Some("gj"))]),
            "descends from the turn `g1`",
        ),
        (
            "a second the guest ordered",
            with(vec![
                order("go", "gj", "s-guest"),
                strike("s2", "s-guest"),
                shown("x", Some("s2")),
            ]),
            "descends from the turn `g1`",
        ),
        (
            "a second the guest and the member ordered",
            with(vec![
                order("go", "gj", "s-both"),
                order("mo", "v", "s-both"),
                strike("s2", "s-both"),
                shown("x", Some("s2")),
            ]),
            "descends from the turn `g1`",
        ),
        (
            "a stroke without an order",
            with(vec![strike("s2", "s-none"), shown("x", Some("s2"))]),
            "a root that is no named cause",
        ),
        (
            "a parent the log does not hold",
            with(vec![shown("x", Some("gone"))]),
            "a parent the log does not hold",
        ),
        (
            "a root that is no named cause",
            with(vec![
                row(
                    "i",
                    None,
                    "@external",
                    screen,
                    json!({"route": "in_view"}),
                    json!({"view_id": "install"}),
                ),
                shown("x", Some("i")),
            ]),
            "a root that is no named cause",
        ),
        (
            "a cycle",
            with(vec![
                row("c1", Some("c2"), &compose, &compose, json!({}), json!({})),
                row("c2", Some("c1"), &compose, &compose, json!({}), json!({})),
                shown("x", Some("c1")),
            ]),
            "no branch reaches a named cause",
        ),
    ];
    for (case, rows, needle) in cases {
        let (_, apart) = judge(&rows);
        if apart.len() != 1 || !apart[0].contains(needle) {
            wrong.push(format!(
                "{case}: want one report naming `{needle}`, got {apart:?}"
            ));
        }
    }
    // Before the verdict: a window the member's verdict did not open yet.
    let mut early = base.clone();
    early.insert(4, shown("x", Some("w")));
    let (_, apart) = judge(&early);
    if apart.len() != 1 || !apart[0].contains("before the verdict") {
        wrong.push(format!("a row before the verdict: {apart:?}"));
    }
    // The guest's branch must work: without its turn, and without a row that traces to it.
    for (case, gone, needle) in [
        ("no guest turn", "g", "holds no turn `g1`"),
        ("no row of the guest", "gj", "cannot see the guest"),
    ] {
        let rows: Vec<MessageLogDto> = base.iter().filter(|r| r.id != gone).cloned().collect();
        let (_, apart) = judge(&rows);
        if !apart.iter().any(|a| a.contains(needle)) {
            wrong.push(format!(
                "{case}: want a report naming `{needle}`, got {apart:?}"
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "the screen claim misses what it names:\n{}",
        wrong.join("\n")
    );
}

/// Re-check tv-bn M7b11 M9: a body over 64 KiB at the screen, logged as a blob, is reported.
/// Its inline form is picked by (4) and (5); the logged form -- the blob's id -- is chosen by
/// neither (that was the fail-open), and [`blobs_at_the_screen`] reports it, while a log of
/// inline rows is clean.
#[test]
fn a_row_at_the_screen_whose_body_is_a_blob_is_reported() {
    let screen = display_colony::SCREEN;
    let web = format!("{screen}/web");
    let compose = format!("{screen}/compose");
    let call = json!({"op": "object.create", "id": "show-sample",
                      "props": {"image": "x".repeat(70_000)}});
    let big = json!({"messages": [{"type": "tool_call", "text": call.to_string()}]});
    assert!(
        big.to_string().len() > 65_536,
        "the body is over the inline limit"
    );
    let inline = row(
        "i",
        None,
        &compose,
        &web,
        json!({"route": "patch"}),
        big.clone(),
    );
    assert!(
        carries_show_sample(&inline) && creates_show_sample_at(&inline, &web),
        "(4) and (5) pick the inline form"
    );
    let mut blob = inline.clone();
    blob.id = "b".to_string();
    blob.body_kind = "blob".to_string();
    blob.body_payload = Some("01928f3e-0000-7000-8000-000000000001".to_string());
    assert!(
        !carries_show_sample(&blob),
        "pick (5) cannot read the logged form"
    );
    // Re-check tv-bn M7b12 C-M1: pick (4) reads the calls through the fixture's
    // `calls_of`, which is red on a body held as a blob -- fail closed at the root.
    let pick4 = std::panic::catch_unwind(|| creates_show_sample_at(&blob, &web));
    assert!(pick4.is_err(), "pick (4) is red on the logged form");
    assert_eq!(
        blobs_at_the_screen(std::slice::from_ref(&inline)),
        Vec::<String>::new()
    );
    let found = blobs_at_the_screen(&[inline, blob]);
    assert_eq!(found.len(), 1, "the blob row is reported: {found:?}");
    assert!(found[0].starts_with("b "), "{found:?}");
}

/// Re-check tv-bn M7b12 C-M1 (class M9): the fixture's readers do not drop a patch the log
/// holds as a blob. `a_turn_without_a_round_opens_no_window_on_a_screen_with_one` judges
/// `!s.drawn("show-sample")` over the folded tree (`drawn` -> `tree` -> `patches` ->
/// `patches_of`); a blob patch that creates `show-sample` was dropped there and the
/// absence held green. Now every reader of a row (`patches_of`, `calls_of`, `body_of`, the
/// log readers through `inline_bodies`) is red on it, and a log of inline rows is read.
#[test]
fn a_patch_held_as_a_blob_is_red_in_every_reader_of_the_fixture() {
    let screen = display_colony::SCREEN;
    let web = format!("{screen}/web");
    let compose = format!("{screen}/compose");
    let call = json!({"op": "object.create", "id": "show-sample"});
    let body = json!({"messages": [{"type": "tool_call", "text": call.to_string()}]});
    let inline = row("i", None, &compose, &web, json!({"route": "patch"}), body);
    assert_eq!(
        display_colony::patches_of(std::slice::from_ref(&inline)).len(),
        1,
        "the inline patch is read"
    );
    let mut blob = inline.clone();
    blob.id = "b".to_string();
    blob.body_kind = "blob".to_string();
    blob.body_payload = Some("01928f3e-0000-7000-8000-000000000001".to_string());
    let rows = vec![inline, blob.clone()];
    let mut open = Vec::new();
    if std::panic::catch_unwind(|| display_colony::patches_of(&rows)).is_ok() {
        open.push("patches_of drops the blob patch");
    }
    if std::panic::catch_unwind(|| display_colony::calls_of(&blob)).is_ok() {
        open.push("calls_of reads the blob as no calls");
    }
    if std::panic::catch_unwind(|| display_colony::body_of(&blob)).is_ok() {
        open.push("body_of reads the blob as an empty body");
    }
    if std::panic::catch_unwind(|| display_colony::inline_bodies(&rows)).is_ok() {
        open.push("inline_bodies passes a blob row");
    }
    assert!(open.is_empty(), "fail open on a blob patch: {open:?}");
}
