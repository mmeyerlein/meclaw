//! d58-3 tag (GH #1527, contract point 7, OR-583.tag.4) -- the window of
//! the running day is captured, whole with the floor and curated without it.
//!
//! WHY this file exists: whether the day as the starting point of every
//! curator (R-58-20) is worth its price is measured with real models
//! (measurement M1 of the wave): arm W with `day_floor` "1", the whole
//! running day raw, against arm K with "0", the running day curated by the
//! horizon of the role -- asked what was said earlier today. That
//! measurement needs the windows themselves, built by the shipped curator,
//! not by hand. This test is their one source: per wall, role and arm it
//! runs one rebuild on the wall's own day (`window_today`: the order of an
//! expired cache, no limit, so nothing but the age of a row decides) and
//! prints the window the next turn sends -- the messages of the call and the
//! plan snapshot the ledger keeps for it (`snapshot_window` "1") -- as one
//! line
//!
//!     CURATOR-BANK-CAPTURE <wall>.<role>.floor_<arm>.json <sha256> <json>
//!
//! `scripts/curator_bank.py capture --window` runs this test through the
//! strand kit and writes the lines as files. No model is called; a stub
//! answers every request of the summarizer.
//!
//! The walls: `phone_day` of the bank (four calls, each a topic of its own)
//! and `consult_day` below (one consultation day in four topics), each a
//! single day, the running one.
//!
//! The roles come from the caller (OR-583.tag.5): the window values of a
//! role live in the product's set, not in core, so the measurement names
//! them -- `scripts/curator_bank.py capture --window --roles <file>` writes
//! the file `ROLES_FILE` beside this test for the run, a JSON list of
//! `{"name", "set", "knobs"}`: the name of the role in the file names, the
//! example value set it runs on (`curator_profiles::VALUE_SETS`), and the
//! params of `./policy` set on top (`horizon`, `keep_recent`,
//! `window_target`, ...). Without the file the roles are the ones M1 names,
//! with the values of the product set 0.61: `task` (the set consult as it
//! is), `all` (the set consult with `keep_recent` 2 and `window_target`
//! 24000, the values the role at that horizon runs at) and `day` (the set
//! talky), the control arm. The control arm sends one window in both arms
//! because of its raw budget, not its age (re-check M-A): talky keeps 40
//! rounds and 80 000 tokens raw, both walls are smaller, so the whole day
//! stays raw whatever `ages_of` says -- right for M1, where the difference of
//! the two arms there is the noise of the judge.
//!
//! Pinned beside the capture: every run is a rebuild on the running day and
//! keeps its snapshot; with the floor every piece of the day is raw; at
//! `task` and `all` the arm without the floor folds pieces of the day and
//! sends the smaller window; at `day` the two arms send the same window; and
//! the switch is a declared param of the policy.
//!
//! Runs the shipped curator hive over the bank (`support/curator_bank.rs`).

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::*;
use curator_hive::{Hive, canonical, sha256_hex, shipped};
use meclaw_core::serde_json::{self as sj, Value, json};

/// The head of every line of the capture on stdout (the mark the reader
/// `scripts/curator_bank.py` looks for).
const MARK: &str = "CURATOR-BANK-CAPTURE";

/// The file the caller names the roles in (`curator_bank.py --roles`),
/// beside the tests of the crate; absent = `default_roles`. A file that
/// lies there and does not read -- any error but "not found" -- fails the
/// run, it never falls back to the defaults (re-check I-B); every record
/// names where its roles came from (`roles`: "file" or "defaults"), the
/// script compares them with the ones it ordered.
const ROLES_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/curator_window_roles.json"
);

/// One role of the capture: its name in the file names, the example value
/// set it runs on, the horizon it runs at and the knobs set on top.
#[derive(Clone)]
struct Role {
    name: String,
    set: &'static str,
    horizon: String,
    knobs: Vec<(&'static str, Value)>,
    /// Where the role came from: "file" (`ROLES_FILE`) or "defaults".
    source: &'static str,
}

/// The horizons of `./policy`; any other value is a mistake of the caller,
/// never a silent `all` (re-check M-C).
const HORIZONS: [&str; 3] = ["day", "task", "all"];

/// A role out of `{"name", "set", "knobs"}`; the horizon is the knob, else
/// the set's own; a horizon outside `HORIZONS` fails the run.
fn role_of(v: &Value, source: &'static str) -> Role {
    let name = v["name"].as_str().unwrap_or("").to_string();
    assert!(
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "{ROLES_FILE}: a role has a name of [a-z0-9_]: {v}"
    );
    let set = v["set"].as_str().unwrap_or("");
    let set = *curator_profiles::VALUE_SETS
        .iter()
        .find(|s| **s == set)
        .unwrap_or_else(|| panic!("{ROLES_FILE}: the role {name} names a known value set: {v}"));
    let knobs: Vec<(&'static str, Value)> = v["knobs"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, x)| (&*Box::leak(k.clone().into_boxed_str()), x.clone()))
                .collect()
        })
        .unwrap_or_default();
    let horizon = knobs
        .iter()
        .find(|(k, _)| *k == "horizon")
        .map(|(_, x)| x.clone())
        .unwrap_or_else(|| curator_profiles::value_set(set)["horizon"].clone());
    let horizon = horizon.as_str().unwrap_or("").to_string();
    assert!(
        HORIZONS.contains(&horizon.as_str()),
        "{ROLES_FILE}: the horizon of the role {name} is one of {HORIZONS:?}: {v}"
    );
    Role {
        name,
        set,
        horizon,
        knobs,
        source,
    }
}

/// The roles M1 names, with the values of the product set 0.61.
fn default_roles() -> Vec<Role> {
    [
        json!({"name": "task", "set": "consult", "knobs": {}}),
        json!({"name": "all", "set": "consult",
               "knobs": {"horizon": "all", "keep_recent": 2, "window_target": 24000}}),
        json!({"name": "day", "set": "talky", "knobs": {}}),
    ]
    .iter()
    .map(|v| role_of(v, "defaults"))
    .collect()
}

/// The roles of the run: the caller's file, else -- only when there is no
/// file -- the defaults.
fn roles() -> Vec<Role> {
    match std::fs::read_to_string(ROLES_FILE) {
        Ok(text) => {
            let v: Value = sj::from_str(&text)
                .unwrap_or_else(|e| panic!("{ROLES_FILE}: the roles as JSON: {e}"));
            let list: Vec<Role> = v
                .as_array()
                .unwrap_or_else(|| panic!("{ROLES_FILE}: a list of roles"))
                .iter()
                .map(|v| role_of(v, "file"))
                .collect();
            assert!(!list.is_empty(), "{ROLES_FILE}: at least one role");
            list
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => default_roles(),
        Err(e) => panic!("{ROLES_FILE}: the roles do not read: {e}"),
    }
}

/// The two arms of the switch.
const ARMS: [&str; 2] = ["1", "0"];

/// The session of the consultation day.
const CONSULT: &str = "s-consult";

/// The topics of the consultation day and the hour each opens at; four
/// rounds each.
const TOPICS: [(&str, i64); 4] = [
    ("the tax letter", 8),
    ("the move of the doctor's office", 10),
    ("the birthday of the grandson", 13),
    ("the repair of the heating", 15),
];

/// The turn id of round `r` of topic `t`.
fn consult_turn(t: usize, r: usize) -> String {
    format!("c{}-{}", t + 1, r + 1)
}

/// The instant of round `r` of topic `t` on the running day.
fn consult_at(t: usize, r: usize) -> chrono::DateTime<chrono::Utc> {
    day(0, TOPICS[t].1, 10 * r as i64)
}

/// A `topic` mark `start` on the first round of every topic but the first,
/// the way `./intake` writes them: at the horizon `task` the day falls into
/// four segments.
fn consult_topics(h: &mut Hive) {
    for (t, (name, _)) in TOPICS.iter().enumerate().skip(1) {
        let start = canonical(&json!({"movement": "start", "name": name}));
        h.mark(
            consult_at(t, 0),
            CONSULT,
            &consult_turn(t, 0),
            "topic",
            &start,
        );
    }
}

/// The consultation day: one session from the morning to the late
/// afternoon, four topics of four rounds, every answer with a paragraph of
/// advice; run at 18:00 of the same day.
fn consult_day() -> Wall {
    let mut w = WallBuilder::new("consult_day", "marlow");
    for (t, (name, _)) in TOPICS.iter().enumerate() {
        for r in 0..4 {
            let turn = consult_turn(t, r);
            w.at(consult_at(t, r));
            w.words(
                CONSULT,
                &turn,
                &format!("About {name}: what should I do next, {{k}}?"),
            );
            let advice = filler((4 * t + r) as u64 + 1, 6);
            w.answer(
                CONSULT,
                &turn,
                &format!("For {name} the next step is clear, {{k}}. {advice}"),
            );
        }
    }
    w.build(day(0, 18, 0), CONSULT).prepare(consult_topics)
}

/// A `topic` mark `start` on the first round of the second, third and
/// fourth call of `phone_day`: every call is a topic of its own.
fn call_topics(h: &mut Hive) {
    for (session, turn, at) in [
        ("phone-2026-03-02T11:30", "p2-1", day(0, 11, 30)),
        ("phone-2026-03-02T14:05", "p3-1", day(0, 14, 5)),
        ("phone-2026-03-02T17:20", "p4-1", day(0, 17, 20)),
    ] {
        let start = canonical(&json!({"movement": "start", "name": format!("call {turn}")}));
        h.mark(at, session, turn, "topic", &start);
    }
}

/// One run of the capture.
struct Job {
    wall: Wall,
    role: Role,
    arm: &'static str,
}

/// The run of `job`: the file name of its capture and the capture.
fn capture(job: &Job) -> (String, Value) {
    let mut run = Run::of(job.role.set);
    for (k, v) in &job.role.knobs {
        run = run.with(k, v.clone());
    }
    let run = run
        .with("snapshot_window", json!("1"))
        .with("day_floor", json!(job.arm));
    let w = window_today(&job.wall, &run);
    let messages: Vec<Value> = w
        .next
        .messages()
        .into_iter()
        .map(|m| json!({"origin": m["origin"], "type": m["type"], "text": m["text"]}))
        .collect();
    let chars: usize = messages
        .iter()
        .map(|m| m["text"].as_str().unwrap_or("").chars().count())
        .sum();
    let pieces = job.wall.pieces.len();
    let raw = (0..pieces).filter(|&i| w.raw(i)).count();
    let rebuilt = w.reason == "cache";
    let record = json!({
        "wall": job.wall.name, "role": job.role.name, "set": job.role.set,
        "horizon": job.role.horizon, "roles": job.role.source, "knobs": job.role.knobs.iter()
            .map(|(k, v)| (k.to_string(), v.clone())).collect::<sj::Map<String, Value>>(),
        "day_floor": job.arm, "mark": w.mark, "reason": w.reason,
        "rebuilt": rebuilt, "requests": w.requests, "cover": w.plan["cover"],
        "pieces": pieces, "raw_pieces": raw, "whole_day": raw == pieces,
        "window_chars": chars, "window": messages, "snapshot": w.snapshot});
    (
        format!("{}.{}.floor_{}.json", job.wall.name, job.role.name, job.arm),
        record,
    )
}

#[test]
fn the_day_window_is_captured() {
    if !shipped() {
        return;
    }
    let bank = bank();
    let walls = [
        bank.named("phone_day").clone().prepare(call_topics),
        consult_day(),
    ];
    let mut found = Findings::default();
    let mut jobs = Vec::new();
    for wall in &walls {
        for role in roles() {
            for arm in ARMS {
                jobs.push(Job {
                    wall: wall.clone(),
                    role: role.clone(),
                    arm,
                });
            }
        }
    }
    // Every run is its own hive; they run side by side.
    let results: Vec<_> = std::thread::scope(|s| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|job| {
                s.spawn(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| capture(job)))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a run of the capture"))
            .collect()
    });
    let mut records = Vec::new();
    for (job, result) in jobs.iter().zip(results) {
        let who = format!(
            "{} as {} ({}) at the horizon {} with day_floor {}",
            job.wall.name, job.role.name, job.role.set, job.role.horizon, job.arm
        );
        match result {
            Ok((file, record)) => {
                let text = sj::to_string(&record).expect("a capture as json");
                println!("{MARK} {file} {} {text}", sha256_hex(&text));
                if !record["snapshot"].is_array() {
                    found.note(format!("{who}: the ledger keeps no snapshot of the window"));
                }
                if record["mark"] != json!("none") || record["rebuilt"] != json!(true) {
                    found.note(format!(
                        "{who}: the rebuild runs on the running day without a line (curate_mark \
                         {}, reason {})",
                        record["mark"], record["reason"]
                    ));
                }
                if job.arm == "1" && record["whole_day"] != json!(true) {
                    found.note(format!(
                        "{who}: with the floor the whole running day stays raw ({} of {} pieces)",
                        record["raw_pieces"], record["pieces"]
                    ));
                }
                records.push((job, record));
            }
            Err(e) => {
                let said = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "a panic without words".to_string());
                found.note(format!(
                    "{who}: the run did not come about: {}",
                    said.lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(400)
                        .collect::<String>()
                ));
            }
        }
    }
    // The two arms side by side: at `task` and `all` the arm without the
    // floor curates the day (the measurement compares something); at `day`
    // the arms are one window -- the control arm of M1. That check guards the
    // capture, not `ages_of`: the switch changes nothing at `day`, and with
    // talky's raw budget (`keep_recent` 40, `keep_recent_tokens` 80 000) both
    // walls stay whole in either arm, so no age could turn it red (re-check
    // M-A).
    for (job, on) in records.iter().filter(|(j, _)| j.arm == "1") {
        let off = records.iter().find(|(j, _)| {
            j.arm == "0" && j.wall.name == job.wall.name && j.role.name == job.role.name
        });
        let Some((_, off)) = off else {
            continue;
        };
        let who = format!(
            "{} as {} at the horizon {}",
            job.wall.name, job.role.name, job.role.horizon
        );
        if job.role.horizon == "day" {
            if on["window"] != off["window"] {
                found.note(format!(
                    "{who}: the two arms send the same window ({} against {} chars)",
                    on["window_chars"], off["window_chars"]
                ));
            }
        } else {
            if off["whole_day"] != json!(false) {
                found.note(format!(
                    "{who}: without the floor the horizon folds pieces of the running day \
                     ({} of {} raw)",
                    off["raw_pieces"], off["pieces"]
                ));
            }
            if off["window_chars"].as_u64() >= on["window_chars"].as_u64() {
                found.note(format!(
                    "{who}: the curated day ({} chars) is smaller than the whole one ({} chars)",
                    off["window_chars"], on["window_chars"]
                ));
            }
        }
    }
    found.judge("the switch", || {
        assert!(
            declared("policy", "day_floor"),
            "templates/curator/policy/config.json declares the switch day_floor"
        )
    });
    found.finish();
}
