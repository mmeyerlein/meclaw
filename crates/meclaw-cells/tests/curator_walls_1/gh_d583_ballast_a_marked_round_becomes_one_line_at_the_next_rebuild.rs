//! GH #1528 (d58-3 ballast, R-583-2 form C) -- a round the judge of the
//! ballast marks `asked` becomes one line at the next rebuild.
//!
//! WHY this file exists: a person on the phone asks the time three times a
//! day and the weather twice. Later only the fact that it was asked matters
//! (the owner on the curator, 11.11.), yet every such round stays in the
//! window word for word. With `ballast_judge` "1" the policy asks a small
//! judge (`curator/relevance`) about every closed round -- the person's
//! words, the answer, and the lines of the marks of the day so far --, once,
//! beside the path, and keeps the judge's `asked` as a mark of the round.
//! The next rebuild shows a marked round as ONE line, `[#<id1> #<id2>
//! asked: <line>]` (`curator_bank::asked_form`, the pattern of
//! `EXPIRED_FORM`), whose ids are the way back through `history_read`.
//!
//! Pinned on the wall `phone_asked` (`support/curator_bank/phone_asked.rs`),
//! the judge a stub: every closed round of the wall is sent to the judge in
//! exactly one request, which holds the round's words and answer whole and
//! the line of every round the judge marked before it that day; after the
//! rebuild every marked round outside the newest `keep_recent` rounds stands
//! in the window as exactly one message, that line, in the place of the
//! round, and no keyword of the round is left; the `gc` mark counts both
//! blocks of each such round in the class `ballast`. Every request carries
//! the judge's contract, the prompt `BALLAST_PROMPT` of the policy as its
//! `system` instructions (review M-3: an empty `system` stays green
//! everywhere else and only a real model would show it).

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
use curator_hive::Msg;
use std::io::Write;
use std::process::{Command, Stdio};

/// The judge's prompt as the policy ships it: the literal of
/// `BALLAST_PROMPT` in the script, read by python's own parser.
fn ballast_prompt() -> String {
    let script = curator_hive::script_of("policy");
    let from = script
        .find("BALLAST_PROMPT = ")
        .expect("the policy names BALLAST_PROMPT")
        + "BALLAST_PROMPT = ".len();
    let to = from
        + script[from..]
            .find("\nBALLAST_ROUND_HEAD")
            .expect("BALLAST_ROUND_HEAD follows the prompt");
    let mut py = Command::new("python3")
        .args([
            "-I",
            "-c",
            "import ast, sys; sys.stdout.write(ast.literal_eval(sys.stdin.read()))",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    py.stdin
        .take()
        .expect("stdin")
        .write_all(&script.as_bytes()[from..to])
        .expect("the literal written");
    let out = py.wait_with_output().expect("python3 ran");
    assert!(out.status.success(), "the literal of BALLAST_PROMPT parses");
    let prompt = String::from_utf8(out.stdout).expect("utf-8");
    assert!(!prompt.trim().is_empty(), "BALLAST_PROMPT is no empty text");
    prompt
}

#[test]
fn a_marked_round_becomes_one_line_at_the_next_rebuild() {
    let (wall, keys) = phone_asked::wall();
    let (h, o) = rebuild_with_hive(&wall, &wall.runs[0]);

    // One request per closed round, holding the round and the lines of the
    // day before it.
    let words: Vec<usize> = (0..wall.pieces.len())
        .filter(|i| wall.pieces[*i].el["origin"] == "user")
        .collect();
    assert_eq!(words.len(), 11, "the wall holds eleven rounds");
    for i in words {
        let p = &wall.pieces[i];
        let round = round_of(&wall, &p.key);
        let reqs: Vec<&Msg> = h
            .judged
            .iter()
            .filter(|r| request_text(r).contains(&p.key))
            .collect();
        assert_eq!(
            reqs.len(),
            1,
            "{}: the judge is asked about the round {} exactly once: {} requests in all",
            o.who(),
            p.key,
            h.judged.len()
        );
        let text = request_text(reqs[0]);
        for j in &round {
            assert!(
                text.contains(wall.pieces[*j].text()),
                "{}: the request about {} holds {} whole: {text}",
                o.who(),
                p.key,
                wall.pieces[*j].key
            );
        }
        // The lines of the asked rounds that stand before this round on the
        // wall -- a later round's line cannot be known yet.
        let earlier = wall
            .asked
            .iter()
            .filter(|(k, _)| wall.pieces.iter().position(|q| q.key == *k) < Some(i));
        for (_, line) in earlier {
            assert!(
                text.contains(line.as_str()),
                "{}: the request about {} holds the line of the day `{line}`: {text}",
                o.who(),
                p.key
            );
        }
    }

    // Every request carries the judge's contract (review M-3).
    let prompt = ballast_prompt();
    for r in &h.judged {
        assert_eq!(
            r.body["system"]["instructions"]["text"].as_str(),
            Some(prompt.as_str()),
            "{}: the request carries BALLAST_PROMPT as its instructions: {}",
            o.who(),
            request_text(r)
        );
    }

    // After the rebuild: one line per marked round, in its place.
    let window = o.window();
    for (n, key) in keys.asked_out.iter().enumerate() {
        let round = round_of(&wall, key);
        let ids: Vec<String> = round.iter().map(|i| wall.pieces[*i].id()).collect();
        let line = &wall
            .asked
            .iter()
            .find(|(k, _)| k == key)
            .expect("an asked round has its line")
            .1;
        let want = asked_form(&ids, line);
        let naming: Vec<usize> = (0..window.len())
            .filter(|m| ids.iter().any(|id| window[*m].contains(&format!("#{id}"))))
            .collect();
        assert_eq!(
            naming
                .iter()
                .map(|m| window[*m].clone())
                .collect::<Vec<_>>(),
            vec![want.clone()],
            "{}: the asked round {} ({n}) stands as exactly one line\n{}",
            o.who(),
            key,
            o.view()
        );
        for i in &round {
            let p = &wall.pieces[*i];
            assert_eq!(o.shown(&wall, *i), Shown::Form(Form::Asked), "{}", p.key);
            assert!(
                window.iter().all(|t| !t.contains(&p.key)),
                "{}: the keyword {} of the asked round is gone",
                o.who(),
                p.key
            );
        }
        // In the place of the round: after the answer of the round before
        // it, in front of the words of the round after it.
        let at = naming[0];
        let last = round[round.len() - 1];
        if round[0] > 0 && o.shown(&wall, round[0] - 1) == Shown::Whole {
            let before = &wall.pieces[round[0] - 1];
            let b = (0..window.len())
                .find(|m| window[*m].ends_with(before.text()))
                .expect("the round before stands");
            assert!(b < at, "{}: {key} stands after {}", o.who(), before.key);
        }
        if last + 1 < wall.pieces.len() && o.shown(&wall, last + 1) == Shown::Whole {
            let after = &wall.pieces[last + 1];
            let a = (0..window.len())
                .find(|m| window[*m].ends_with(after.text()))
                .expect("the round after stands");
            assert!(at < a, "{}: {key} stands before {}", o.who(), after.key);
        }
    }

    // The count of the class: both blocks of every marked round.
    let mark = o.gc_mark();
    assert_eq!(
        mark["classes"][BALLAST],
        meclaw_core::serde_json::json!(2 * keys.asked_out.len()),
        "{}: the gc mark counts the blocks of the marked rounds: {mark}",
        o.who()
    );
}
