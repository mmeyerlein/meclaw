//! GH #1465 (D58, the bank) -- what a rebuild asks the summarizer is
//! captured, and what was planted in the wall stands in it.
//!
//! WHY this file exists: what a summary keeps can only be measured against
//! what the summarizer was handed. The wall `over_aim` holds planted
//! promises, open points and facts, each known by a keyword of its own; the
//! wall `coding` holds the person's instructions and a test run that
//! failed. This test runs `over_aim`, its two variants and `coding` through
//! the rebuild once per form of the request and per role, and prints every
//! request the rebuild sent -- to the summarizer, or on the role's own way
//! where the role condenses itself -- as one line of JSON, with the list of
//! the planted points and where each of them stands.
//!
//! It is the one source of the capture: `scripts/curator_bank.py capture`
//! runs this test and writes the lines it prints as files. No model is
//! called; a stub answers every request.
//!
//! WHAT A REQUEST HOLDS follows the tiers of the role: a row is raw, goes
//! into the summary, or is dropped (`none`), by its age at the role's
//! horizon. One role keeps today raw, hands yesterday to the summary and
//! drops what is older; that is its preset and wanted, what is older is the
//! memory's to cover, not the window's. Another has no tier `none`: there
//! every row that leaves the raw part goes into the request. The walls of
//! talk are captured under both, the second with a `window_target` set by
//! knob, so the same wall is shown under both tier lists.
//!
//! Pinned: every run of the list sends a request; a planted point that left
//! the raw part of the window and whose row lies in the tier `summary`
//! stands in that request, the sentence whole; a planted point in the tier
//! `none` does not stand in a request sent to the summarizer (a silent
//! condensing of old days would show here); and no run leaves every planted
//! point raw. Where the role condenses itself the request carries the old
//! window in front, so a point of the tier `none` is recorded there and not
//! judged. Every point is printed with its `tier` and `expected_in_request`.
//! D58 plant (GH #1490): `over_aim` plants two kinds more, a fact of the
//! person about a third person (`third_party`, printed with its `subject`)
//! and a value a later row replaces (`superseded`, printed with `old_key`,
//! `new_key` and the old row under `old`). The old row is judged like any
//! planted row: expected in the request where it left the raw part and lies
//! in the tier `summary`, whole. The capture holds at least one point of
//! each of the two kinds with these fields.
//! The forms and the roles are parameters of the runs (`FORMS`,
//! `TALK_RUNS`, `TASK_RUNS`); a role the policy does not know is said, never
//! replaced by another.
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
use curator_hive::{cell_config, sha256_hex, shipped};
use curator_profiles::VALUE_SETS;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The head of every line of the capture on stdout:
/// `<MARK> <file> <sha256 of the json> <json>`.
const MARK: &str = "CURATOR-BANK-CAPTURE";

/// The forms of the request: the name of the capture and the knobs of
/// `./policy` a run sets for it.
const FORMS: [(&str, &[(&str, &str)]); 4] = [
    ("prose", &[]),
    ("brief", &[("summary_form", "brief")]),
    (
        "brief_state_cond",
        &[("summary_form", "brief"), ("summary_state_cond", "1")],
    ),
    ("summarizer_self", &[("summarizer_self", "1")]),
];

/// A role a wall is captured with, and the knobs of `./policy` set for it
/// beside its presets.
type RoleRun = (&'static str, &'static [(&'static str, i64)]);

/// The roles the walls of talk are captured with. The second presets no
/// `window_target`; it is set by knob to the target the first presets, so
/// both rebuild the same wall at the same size.
const TALK_RUNS: [RoleRun; 2] = [("talky", &[]), ("consult", &[("window_target", 100_000)])];

/// The roles the wall of a task is captured with.
const TASK_RUNS: [RoleRun; 1] = [("coding", &[])];

/// What the stub answers a request for a brief with: one line of a brief,
/// dated with a day its transcript shows (`{day}`, filled in by `condense`,
/// GH #1496).
const BRIEF_STUB: &str = "- topic ({day}): what came before, condensed by the stub of the bank";

/// One planted point of a wall: a row known by its keyword.
#[derive(Clone)]
struct Point {
    kind: String,
    key: String,
    text: String,
    piece: usize,
    /// How many days of the wall lie behind the day of the point, when the
    /// wall says its days. The walls of talk age by the day at either
    /// horizon: a day is a segment of its own at the horizon of a task too
    /// (a segment opens at a day change, and the variant with topics opens
    /// each topic with its day).
    age: Option<i64>,
    /// `third_party`: the third person the fact is about.
    subject: Option<String>,
    /// `superseded`: the row of the value the point replaces.
    old: Option<Box<Point>>,
}

/// What a run tells of the rows of its planted points.
struct Judge<'a> {
    o: &'a Outcome,
    said: &'a str,
    tiers: &'a [String],
    own_way: bool,
    sent: bool,
    who: &'a str,
    lines: Vec<String>,
    condensed: usize,
    dropped: usize,
}

impl Judge<'_> {
    /// The record of the row of `p`: where it stands and what is expected of
    /// it; a violation becomes a line.
    fn row(&mut self, p: &Point) -> Value {
        let who = self.who;
        let tiers = self.tiers;
        let left_raw = !self.o.raw(p.piece);
        let (key_in, text_in) = (self.said.contains(&p.key), self.said.contains(&p.text));
        let tier = tier_of(tiers, p.age, left_raw);
        if tier.is_none() {
            self.lines.push(format!(
                "{who}: the planted {} {} has no tier the wall could name (tiers {tiers:?})",
                p.kind, p.key
            ));
        }
        let tier = tier.unwrap_or_default();
        let expected = left_raw && tier == "summary";
        // A point of the tier `none` is judged where the request went to
        // the summarizer; on the role's own way the old window stands in
        // front of the request, the point with it.
        let must_be_absent = tier == "none" && !self.own_way;
        if left_raw {
            self.condensed += 1;
        }
        if expected && self.sent && !text_in {
            self.lines.push(format!(
                "{who}: the planted {} {} left the raw part, lies in the tier summary and is \
                 not whole in the request (its keyword is {})",
                p.kind,
                p.key,
                if key_in { "there" } else { "missing too" }
            ));
        }
        if tier == "none" {
            self.dropped += 1;
        }
        if must_be_absent && (key_in || text_in) {
            self.lines.push(format!(
                "{who}: the planted {} {} lies in the tier none and stands in the request \
                 sent to the summarizer",
                p.kind, p.key
            ));
        }
        json!({
            "kind": p.kind, "key": p.key, "text": p.text, "piece": p.piece,
            "seq": self.o.seqs[p.piece], "left_raw": left_raw, "age_days": p.age,
            "tier": tier, "expected_in_request": expected,
            "must_be_absent": must_be_absent,
            "key_in_request": key_in, "text_in_request": text_in})
    }

    /// The record of `p`: its row, and the fields of its kind.
    fn point(&mut self, p: &Point) -> Value {
        let mut rec = self.row(p);
        if let Some(s) = &p.subject {
            rec["subject"] = json!(s);
        }
        if let Some(old) = &p.old {
            rec["old_key"] = json!(old.key);
            rec["new_key"] = json!(p.key);
            rec["old"] = self.row(old);
        }
        rec
    }
}

/// The tier list of a run as the policy reports it (`tiers`).
fn tiers_of(knobs: &Knobs) -> Vec<String> {
    match &knobs.all["tiers"] {
        Value::Array(a) => a
            .iter()
            .map(|t| t.as_str().unwrap_or("").trim().to_string())
            .collect(),
        other => other
            .as_str()
            .unwrap_or("")
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
    }
}

/// The tier of a planted point: by its age where the wall says it, the
/// oldest tier of the list for all that is older. A wall that names no days
/// has its points raw while they stand in the raw part, and in the one tier
/// behind `raw` once they left it; a list with more than one such tier
/// cannot say which, and the point has no tier then.
fn tier_of(tiers: &[String], age: Option<i64>, left_raw: bool) -> Option<String> {
    match age {
        Some(a) => tiers
            .get((a.max(0) as usize).min(tiers.len().saturating_sub(1)))
            .cloned(),
        None if !left_raw => Some("raw".to_string()),
        None => {
            let rest: Vec<&String> = tiers.iter().skip(1).collect();
            match rest.first() {
                Some(first) if rest.iter().all(|t| t == first) => Some((*first).clone()),
                _ => None,
            }
        }
    }
}

/// One run of the capture.
struct Job {
    wall: Wall,
    points: Vec<Point>,
    role: &'static str,
    /// The knobs set for the role beside its presets.
    role_knobs: &'static [(&'static str, i64)],
    form: &'static str,
    knobs: &'static [(&'static str, &'static str)],
}

/// Every string a value holds, one per line.
fn strings_of(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings_of(x, out)),
        _ => {}
    }
}

/// The run of `job`: the file name of its capture, the capture, and what it
/// violates.
fn capture(job: &Job) -> (String, Value, Vec<String>) {
    let mut wall = job.wall.clone();
    let mut run = Run::of(job.role);
    for (k, v) in job.role_knobs {
        run = run.with(k, json!(*v));
    }
    for (k, v) in job.knobs {
        run = run.with(k, json!(*v));
        if (*k, *v) == ("summary_form", "brief") {
            wall.summary = BRIEF_STUB.to_string();
        }
    }
    let who = format!("{} as {} in the form {}", wall.name, job.role, job.form);
    let o = rebuild(&wall, &run);
    let mut lines = Vec::new();
    let mut said = String::new();
    let mut requests = Vec::new();
    // Whether every request left on the role's own way (it condenses
    // itself): such a request carries the old window in front.
    let mut own_way = true;
    for m in &o.requests {
        strings_of(&Value::Object(m.body.clone()), &mut said);
        // D58 target (contract point 8): a request the role condenses itself
        // leaves as a model call (`model_call`), not on the summarizer's way.
        own_way = own_way && m.route() == "model_call";
        requests.push(json!({"route": m.route(), "hop": m.hop, "body": m.body}));
    }
    let tiers = tiers_of(&o.knobs);
    if tiers.first().map(String::as_str) != Some("raw") {
        lines.push(format!(
            "{who}: the tier list of the run opens with raw: {tiers:?}"
        ));
    }
    if requests.is_empty() {
        lines.push(format!("{who}: the rebuild sent no request"));
    }
    let mut judge = Judge {
        o: &o,
        said: &said,
        tiers: &tiers,
        own_way,
        sent: !requests.is_empty(),
        who: &who,
        lines: Vec::new(),
        condensed: 0,
        dropped: 0,
    };
    let planted: Vec<Value> = job.points.iter().map(|p| judge.point(p)).collect();
    let (condensed, dropped) = (judge.condensed, judge.dropped);
    lines.append(&mut judge.lines);
    // A tier list with `none` over a wall that says its days: the statement
    // about the dropped points is judged on a point.
    if tiers.iter().any(|t| t == "none")
        && job.points.iter().any(|p| p.age.is_some())
        && dropped == 0
    {
        lines.push(format!(
            "{who}: the role drops old rows (tiers {tiers:?}) and no planted point lies in \
             the tier none"
        ));
    }
    if condensed == 0 {
        lines.push(format!(
            "{who}: no planted point left the raw part, the request shows none"
        ));
    }
    let record = json!({
        "wall": wall.name, "role": job.role, "form": job.form,
        "knobs": job.role_knobs.iter().map(|(k, v)| (k.to_string(), json!(v)))
            .chain(job.knobs.iter().map(|(k, v)| (k.to_string(), json!(v))))
            .collect::<sj::Map<String, Value>>(),
        "tiers": tiers, "horizon": o.knobs.all["horizon"], "own_way": own_way,
        "aim": o.aim, "cover": o.cover(), "window_target": o.knobs.window_target,
        "requests": requests, "planted": planted});
    (
        format!("{}.{}.{}.json", wall.name, job.role, job.form),
        record,
        lines,
    )
}

#[test]
fn the_summarizer_request_is_captured() {
    if !shipped() {
        return;
    }
    let mut found = Findings::default();

    // The forms are knobs the shipped policy declares.
    let settings = cell_config("policy")["contract"]["settings"].clone();
    for (form, knobs) in FORMS {
        for (k, _) in knobs {
            if settings.get(*k).is_none() {
                found.note(format!("form {form}: the policy declares no knob {k}"));
            }
        }
    }
    // The example value sets a run's target may report (the curator keeps no
    // roles since D58 target, R-58-15).
    let known: Vec<String> = VALUE_SETS.iter().map(|r| r.to_string()).collect();

    // The walls and what is planted in each.
    let bank = bank();
    let talk_points: Vec<Point> = over_aim::planted()
        .into_iter()
        .map(|p| Point {
            kind: p.kind.to_string(),
            key: p.key,
            text: p.text,
            piece: p.piece,
            // The rebuild of the wall plans on its last day, day 0.
            age: Some(-p.day),
            subject: p.subject.map(str::to_string),
            old: p.old.map(|r| {
                Box::new(Point {
                    kind: p.kind.to_string(),
                    key: r.key,
                    text: r.text,
                    piece: r.piece,
                    age: Some(-r.day),
                    subject: None,
                    old: None,
                })
            }),
        })
        .collect();
    // D58 plant: the capture carries both new kinds, each with its own
    // field (review plant m5: either field for either kind let a
    // `third_party` with an old row and no subject pass): a `third_party`
    // names its subject and carries no old row, a `superseded` carries its
    // old row.
    for kind in ["third_party", "superseded"] {
        let of_kind: Vec<&Point> = talk_points.iter().filter(|p| p.kind == kind).collect();
        if of_kind.is_empty() {
            found.note(format!("over_aim: the capture holds no planted {kind}"));
        }
        for p in of_kind {
            let own_field = match kind {
                "third_party" => p.subject.is_some() && p.old.is_none(),
                _ => p.old.is_some(),
            };
            if !own_field {
                found.note(format!(
                    "over_aim: the captured {kind} {} carries the field of its kind \
                     (subject {:?}, old row {})",
                    p.key,
                    p.subject,
                    p.old.is_some()
                ));
            }
        }
    }
    let task = bank.named("coding").clone();
    let task_points: Vec<Point> = task
        .pieces
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let kind = match p.what {
                What::Words => "instruction",
                What::Error => "failed_run",
                _ => return None,
            };
            Some(Point {
                kind: kind.to_string(),
                key: p.key.clone(),
                text: p.text().to_string(),
                piece: i,
                age: None,
                subject: None,
                old: None,
            })
        })
        .collect();
    let mut walls: Vec<(Wall, &Vec<Point>, &[RoleRun])> =
        vec![(bank.named("over_aim").clone(), &talk_points, &TALK_RUNS)];
    for v in over_aim::variants() {
        walls.push((v, &talk_points, &TALK_RUNS));
    }
    walls.push((task, &task_points, &TASK_RUNS));

    let mut jobs = Vec::new();
    for (wall, points, roles) in &walls {
        for (role, role_knobs) in roles.iter() {
            if !known.iter().any(|k| k.as_str() == *role) {
                found.note(format!(
                    "{}: no example value set {role:?} (the sets are {known:?}), nothing is \
                     captured for it",
                    wall.name
                ));
                continue;
            }
            for (form, knobs) in FORMS {
                jobs.push(Job {
                    wall: wall.clone(),
                    points: (*points).clone(),
                    role,
                    role_knobs,
                    form,
                    knobs,
                });
            }
        }
    }
    assert!(!jobs.is_empty(), "the capture holds a run");

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
    for (job, result) in jobs.iter().zip(results) {
        match result {
            Ok((file, record, lines)) => {
                let text = sj::to_string(&record).expect("a capture as json");
                println!("{MARK} {file} {} {text}", sha256_hex(&text));
                lines.into_iter().for_each(|l| found.note(l));
            }
            Err(e) => {
                let said = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "a panic without words".to_string());
                found.note(format!(
                    "{} as {} in the form {}: the rebuild did not come about: {}",
                    job.wall.name,
                    job.role,
                    job.form,
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
    found.finish();
}
