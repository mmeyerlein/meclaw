"""Unit tests for the wave retro (`scripts/wave_retro.py`, ruling R-P3).

The retro reads three kinds of evidence -- strand reports, gate receipts and
session transcripts -- and turns them into ten numbers. Every test here builds
a throw-away wave that carries all three, so the expectations stay stable and
no test ever touches a real wave, a real transcript or the machine's home.

The fixture wave is deliberately small and deliberately BAD: one strand needs
two gate runs, one of them red for a reason it did not cause, and the
orchestrator reads more than its share. A retro that reports everything green
over this wave is not measuring.
"""

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SCRIPTS = HERE.parent
REPO = SCRIPTS.parent
sys.path.insert(0, str(SCRIPTS))

import wave_retro as wr  # noqa: E402
from retro import gates  # noqa: E402

WAVE = "welle-t-2026-01-01"
SESSION = "11111111-2222-3333-4444-555555555555"


def _turn(ts, request_id, inp, cc, cr, out, blocks=None):
    """One assistant line of a transcript, in the shape Claude Code writes."""
    return json.dumps({
        "type": "assistant",
        "timestamp": ts,
        "requestId": request_id,
        "message": {
            "usage": {
                "input_tokens": inp,
                "cache_creation_input_tokens": cc,
                "cache_read_input_tokens": cr,
                "output_tokens": out,
            },
            "content": blocks or [],
        },
    })


def _user(ts, text):
    return json.dumps({
        "type": "user",
        "timestamp": ts,
        "message": {"content": [{"type": "text", "text": text}]},
    })


def fixture_wave(case):
    """A throw-away repo + transcript root holding one measurable wave."""
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    root = pathlib.Path(tmp.name)
    wave = root / "plans" / WAVE
    (wave / "berichte").mkdir(parents=True)
    (wave / "receipts" / "alpha").mkdir(parents=True)

    (wave / "berichte" / "alpha.md").write_text(
        "---\n"
        "strang: alpha\n"
        "branch: welle-t/alpha\n"
        "issues: [#1]\n"
        "basis: abc1234\n"
        'gate: "GATE-SUMMARY strand abc1234 12/12 600s GREEN"\n'
        "commits: [abc1234, deadbee]\n"
        "---\n\n"
        "## Auftrag\nEine Kachel.\n\n"
        "## Gate\n"
        "GATE-SUMMARY strand abc1234 11/12 900s RED\n"
        "Rot war `710_the_scenarios_run_against_the_curator::the_copy_is_the_document`\n"
        "-- Basis-Drift aus einer fremden Session.\n"
        "GATE lock-wait [60s behind other runs] 60s NOTE\n"
        "GATE-SUMMARY strand abc1234 12/12 600s GREEN\n\n"
        "## Review\n"
        "- C1 Der Lock prueft die falsche Zeile (alpha.rs:10).\n"
        "- N2 Wortlaut im CHANGELOG stimmt nicht (CHANGELOG.md:3).\n",
        encoding="utf-8")
    (wave / "berichte" / "alpha-review.md").write_text(
        "# Review alpha\nVerdikt: Fix needed\n\n"
        "| Fund | Schwere |\n|---|---|\n| C1 | Critical |\n| N2 | Nit |\n\n"
        "## Critical\n\n"
        "### C1 — Der Lock prueft die falsche Zeile\n\n"
        "`alpha.rs:10` vergleicht die zweite statt der ersten Kachel.\n\n"
        "## Nit\n\n"
        "### N2 — Die Zeile im Verzeichnis passt nicht\n\n"
        "Der Wortlaut im CHANGELOG nennt die alte Fassung.\n",
        encoding="utf-8")
    (wave / "berichte" / "beta-report.md").write_text(
        "# beta\nGATE-SUMMARY strand abc1234 12/12 300s GREEN\n",
        encoding="utf-8")
    (wave / "berichte" / "beta-review.md").write_text(
        "# Review beta\nVerdikt: Approved\n\n## Minor\n\n"
        "### N1 — Die Abweisung wird nicht geprueft\n",
        encoding="utf-8")

    # An analysis document inside the wave that QUOTES a run of another
    # wave. It must not become a run of this one.
    (wave / "befund").mkdir()
    (wave / "befund" / "01-zeit.md").write_text(
        "# Befund\nDie Welle davor fuhr\n"
        "GATE-SUMMARY strand ffff999 9/12 4444s RED\n"
        "und das war ein Fremdbefund.\n", encoding="utf-8")

    with open(wave / "receipts" / "alpha" / "last-strand.json", "w") as fh:
        json.dump({
        "mode": "strand",
        "rev": "abc1234",
        "verdict": "GREEN",
        "lock_wait_secs": 60,
        "started": "2026-01-01T10:00:00",
        "finished": "2026-01-01T10:10:00",
        "stations": [{"name": "tests", "scope": "alpha", "secs": 540,
                      "verdict": "GREEN", "log": "tests.log"}],
        }, fh)

    # --- transcripts -----------------------------------------------------
    troot = root / "transcripts"
    subs = troot / SESSION / "subagents"
    subs.mkdir(parents=True)
    (troot / (SESSION + ".jsonl")).write_text("\n".join([
        _user("2026-01-01T09:00:00Z", "Du faehrst die Welle T."),
        _turn("2026-01-01T09:00:10Z", "req_o1", 10, 20_000, 0, 500),
        _turn("2026-01-01T09:30:00Z", "req_o2", 10, 5_000, 25_000, 500),
    ]) + "\n", encoding="utf-8")

    # alpha: two API turns per request id (the duplicate-usage trap), one
    # long gap (Liegezeit), one repeated log read (a poll), one fix round.
    (subs / "agent-aaa1.jsonl").write_text("\n".join([
        _user("2026-01-01T10:00:00Z",
              "Du bist der Bauer des Strangs alpha. Nie `pkill` auf meclaw. "
              "Nie `.env` lesen. Bericht nach plans/" + WAVE + "/berichte/alpha.md."),
        _turn("2026-01-01T10:00:30Z", "req_a1", 5, 120_000, 0, 1,
              [{"type": "tool_use", "id": "t1", "name": "Bash",
                "input": {"command": "tail -40 target/gate/wt/logs/strand-tests.log"}}]),
        _turn("2026-01-01T10:00:31Z", "req_a1", 5, 120_000, 0, 900),
        _turn("2026-01-01T10:05:00Z", "req_a2", 5, 1_000, 120_000, 100,
              [{"type": "tool_use", "id": "t2", "name": "Bash",
                "input": {"command": "sleep 60"}}]),
        _turn("2026-01-01T10:06:00Z", "req_a3", 5, 1_000, 121_000, 100,
              [{"type": "tool_use", "id": "t3", "name": "Bash",
                "input": {"command": "tail -40 target/gate/wt/logs/strand-tests.log"}}]),
        _user("2026-01-01T11:30:00Z",
              "The coordinator sent a message: Fix-Runde 1, bitte C1 beheben."),
        _turn("2026-01-01T13:30:00Z", "req_a4", 5, 1_000, 122_000, 100),
    ]) + "\n", encoding="utf-8")
    (subs / "agent-aaa1.meta.json").write_text(
        json.dumps({"description": "Strang alpha", "agentType": "claude",
                    "model": "opus"}), encoding="utf-8")
    (subs / "agent-bbb1.jsonl").write_text("\n".join([
        _user("2026-01-01T10:00:00Z",
              "Du bist der Bauer des Strangs beta. Nie `pkill` auf meclaw. "
              "Nie `.env` lesen."),
        _turn("2026-01-01T10:10:00Z", "req_b1", 5, 30_000, 0, 400),
    ]) + "\n", encoding="utf-8")
    (subs / "agent-bbb1.meta.json").write_text(
        json.dumps({"description": "Strang beta", "agentType": "claude",
                    "model": "opus"}), encoding="utf-8")
    return root, troot


def run_retro(case, root, troot, *extra):
    argv = [WAVE, "--root", str(root), "--transcripts", str(troot),
            "--sessions", SESSION] + list(extra)
    return wr.main(argv)


class ThresholdTests(unittest.TestCase):
    def test_every_metric_has_a_threshold_from_the_one_file(self):
        """Ten metrics, one file, and the file is what the code reads."""
        spec = wr.load_thresholds()
        ids = [m["id"] for m in spec["metrics"]]
        self.assertEqual(ids, [f"Q{n}" for n in range(1, 11)])
        for m in spec["metrics"]:
            self.assertIn("threshold", m)
            self.assertIn("title", m)
            self.assertIn("unit", m)
        self.assertEqual(spec["ruling"], "R-P3")

    def test_thresholds_file_lives_next_to_the_library(self):
        self.assertTrue((SCRIPTS / "retro" / "thresholds.json").is_file())


def wave_with(case, **files):
    """A throw-away wave directory holding the given `berichte/` files."""
    tmp = tempfile.TemporaryDirectory()
    case.addCleanup(tmp.cleanup)
    wave = pathlib.Path(tmp.name) / "plans" / WAVE
    (wave / "berichte").mkdir(parents=True)
    for name, text in files.items():
        (wave / "berichte" / name).write_text(text, encoding="utf-8")
    return wave


class ForeignClassificationTests(unittest.TestCase):
    """Q2: whose fault a red run was (finding 01 section 3.2).

    The reason of a run stands UNDER that run, and the reason of its
    neighbour stands under the neighbour. A window of fixed size around the
    summary line reads both and calls every red run in a busy report
    foreign."""

    def _by_secs(self, wave):
        return {r["secs"]: r for r in gates.runs_of(wave, "strand")}

    def test_a_red_run_is_not_foreign_because_the_next_run_names_a_flake(self):
        wave = wave_with(self, **{"alpha-report.md":
            "# alpha\n\n## Gate\n\n"
            "```\nGATE-SUMMARY strand aaa1111 10/12 500s RED\n```\n\n"
            "Zwei Rote, beide meine: `fmt` und drei goldene Manifeste.\n"
            "Fix-Commits `abc1234` und `def5678`, danach der Gate noch einmal.\n\n"
            "```\nGATE-SUMMARY strand bbb2222 11/12 400s RED\n```\n\n"
            "Die eine rote Station ist `scenarios:builder`,\n"
            "`registry lacks '/os/builder/eyes'` — die Flake #721.\n"})
        runs = self._by_secs(wave)
        self.assertEqual(runs[500]["foreign"], [])
        self.assertTrue(runs[400]["foreign"])

    def test_a_run_that_also_failed_for_its_own_reason_is_the_strands_fault(self):
        """A red run with a genuine station of its own is the strand's, even
        when a second station was somebody else's -- that is how the hand
        count in section 3.2 classifies every mixed run."""
        wave = wave_with(self, **{"alpha-report.md":
            "# alpha\n\n```\nGATE-SUMMARY strand aaa1111 10/12 500s RED\n```\n\n"
            "Zwei Rote:\n\n"
            "1. **Echt.** `gh703_the_chat_shows_its_newest_line` — elf Zeilen "
            "umgestellt.\n"
            "2. **Kontention, kein Befund.** `scenarios:builder`, "
            "`registry lacks '/os/builder/eyes'`.\n"})
        self.assertEqual(self._by_secs(wave)[500]["foreign"], [])

    def test_a_run_that_only_failed_for_a_foreign_reason_is_foreign(self):
        wave = wave_with(self, **{"alpha-report.md":
            "# alpha\n\n```\nGATE-SUMMARY strand aaa1111 11/12 900s RED\n```\n\n"
            "Rot ist genau eine Station, und sie gehoert nicht diesem Diff:\n"
            "`710_the_scenarios_run_against_the_curator::the_copy_is_the_document`\n"
            "— der Baum zog waehrend des Laufs weiter.\n"})
        self.assertEqual(self._by_secs(wave)[900]["foreign"], ["Basis-Drift"])

    def test_a_station_rerun_and_its_green_repeat_are_two_runs(self):
        """`0/1 63s RED` and `1/1 63s GREEN` are the rerun of one station and
        its repeat: same commit, same second, different outcome. A key of
        (mode, rev, secs) alone merges them, red wins, and Q1 loses a run
        while Q2 gains a red one (review M5, turn-id-report.md:379/380)."""
        wave = wave_with(self, **{"alpha-report.md":
            "# alpha\n\n```\n"
            "GATE-SUMMARY strand aaa1111 0/1 63s RED\n"
            "GATE-SUMMARY strand aaa1111 1/1 63s GREEN\n```\n"})
        runs = gates.runs_of(wave, "strand")
        self.assertEqual(len(runs), 2)
        self.assertEqual(sorted(r["verdict"] for r in runs), ["GREEN", "RED"])

    def test_one_run_quoted_in_two_documents_is_still_one_run(self):
        """The dedup over documents stays: the report and the review of a
        strand carry the same line, stations and all."""
        wave = wave_with(self, **{
            "alpha-report.md": "# alpha\nGATE-SUMMARY strand aaa1111 12/13 600s GREEN\n",
            "alpha-review.md": "# review\nGATE-SUMMARY strand aaa1111 12/13 600s GREEN\n"})
        self.assertEqual(len(gates.runs_of(wave, "strand")), 1)


class ForeignCorpusTests(unittest.TestCase):
    """Q2 against the three waves finding 01 section 3.2 counted by hand.

    The share is what the hand count says for H3 (8 of 11) and what its
    per-run classes say for H2 (the two `scenarios:builder` runs of turn-id
    and the one of one-frame). Two runs of H3 are classified the other way
    round than by hand -- `taps` 860 s (a genuine failure next to a drift and
    a flake) and `verdict` 503 s (whose reason stands outside its section) --
    and they cancel out. The test pins the mechanism, not a coincidence.
    """

    def _share(self, wave):
        runs = gates.runs_of(REPO / "plans" / wave, "strand")
        red = [r for r in runs if r["verdict"] == "RED"]
        return len([r for r in red if r["foreign"]]), len(red)

    def setUp(self):
        if not (REPO / "plans" / "welle-h2-2026-09-18").is_dir():
            self.skipTest("the measured waves are not in this tree")

    def test_h2_counts_the_three_builder_runs_and_no_mixed_one(self):
        self.assertEqual(self._share("welle-h2-2026-09-18"), (3, 6))

    def test_h3_counts_eight_of_eleven_as_the_hand_count_does(self):
        self.assertEqual(self._share("welle-h3-2026-09-18"), (8, 11))

    # There was a third arm here, over `welle-g-2026-09-15`, pinned at (0, 3).
    # H2 and H3 are CLOSED waves: their receipts and reports stand still, so a
    # hand count over them pins a mechanism. Wave G was live when the arm was
    # written, and `runs_of` reads the untracked gate archive and the strand
    # reports as well -- so every further strand of the wave moved the number,
    # and the arm was red for whoever ran a gate next (measured at (1, 9) on
    # 2026-09-19, before this strand's own run was archived). A count over a
    # directory that is still being written to is not a measurement.


class GateDirectoryTests(unittest.TestCase):
    """The second receipt source of the contract: `target/gate/**`.

    A wave measures itself as its last step, BEFORE its receipts are copied
    into `plans/<wave>/receipts/`. Until then the only receipt of the run
    that just finished lies under `target/gate/<tree>/` (review M2)."""

    def _receipt(self, path, rev, secs):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps({
            "mode": "strand", "rev": rev, "verdict": "GREEN",
            "lock_wait_secs": 30,
            "started": "2026-01-01T10:00:00",
            "finished": f"2026-01-01T10:0{secs // 60}:{secs % 60:02d}",
            "stations": [{"name": "tests", "verdict": "GREEN"}],
        }), encoding="utf-8")

    def test_the_gate_directory_is_read_while_the_wave_kept_no_receipts(self):
        wave = wave_with(self, **{"alpha-report.md": "# alpha\nKein Gate zitiert.\n"})
        gate_dir = wave.parent.parent / "target" / "gate"
        self._receipt(gate_dir / "wt-alpha" / "last-strand.json", "ccc3333", 120)
        self.assertEqual(gates.runs_of(wave, "strand"), [])
        runs = gates.runs_of(wave, "strand", gate_dir=gate_dir)
        self.assertEqual([(r["rev"], r["secs"]) for r in runs], [("ccc3333", 120)])

    def test_an_archived_wave_ignores_the_gate_directory_of_this_tree(self):
        """A wave that kept its receipts is measured from them alone --
        otherwise every old wave inherits the runs of the tree measuring it."""
        wave = wave_with(self, **{"alpha-report.md": "# alpha\n"})
        (wave / "receipts" / "alpha").mkdir(parents=True)
        self._receipt(wave / "receipts" / "alpha" / "last-strand.json", "aaa1111", 60)
        gate_dir = wave.parent.parent / "target" / "gate"
        self._receipt(gate_dir / "wt-fremd" / "last-strand.json", "ccc3333", 120)
        runs = gates.runs_of(wave, "strand", gate_dir=gate_dir)
        self.assertEqual([r["rev"] for r in runs], ["aaa1111"])


class LowerBoundTests(unittest.TestCase):
    """Q1 and Q3 count what is documented, and say so when that is less.

    Finding 01 section 3.2 says it in one sentence -- "die Berichts-Zählung
    ist die Untergrenze" -- and names three runs of H2 that carry no seconds
    and several queue waits written as prose. The numbers cannot catch them;
    the report can say that they exist (review M4)."""

    def _retro(self, root, troot):
        run_retro(self, root, troot)
        return (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")

    def test_a_run_without_seconds_makes_q1_a_lower_bound(self):
        root, troot = fixture_wave(self)
        bericht = root / "plans" / WAVE / "berichte" / "alpha.md"
        bericht.write_text(bericht.read_text(encoding="utf-8") +
                           "\nEin dritter Lauf, im Bericht ohne Sekunden:\n"
                           "GATE-SUMMARY strand abc1234 11/12 RED\n",
                           encoding="utf-8")
        self.assertTrue(wr.metric_row(self._retro(root, troot), "Q1")["wert"]
                        .startswith("≥"))

    def test_a_queue_wait_written_as_prose_makes_q3_a_lower_bound(self):
        root, troot = fixture_wave(self)
        bericht = root / "plans" / WAVE / "berichte" / "beta-report.md"
        bericht.write_text(bericht.read_text(encoding="utf-8") +
                           "\nDie Schlange davor: lock_wait 1352s.\n",
                           encoding="utf-8")
        self.assertTrue(wr.metric_row(self._retro(root, troot), "Q3")["wert"]
                        .startswith("≥"))

    def test_a_wave_that_documented_everything_keeps_the_plain_number(self):
        root, troot = fixture_wave(self)
        retro = self._retro(root, troot)
        for metric in ("Q1", "Q3"):
            self.assertFalse(wr.metric_row(retro, metric)["wert"].startswith("≥"))


class ProvisionalTests(unittest.TestCase):
    """A wave measured while it still runs says so.

    The retro of the wave it belongs to is measured in ONE worktree, and the
    other strands are not merged yet: Q1, Q3 and Q9 are then the numbers of
    the strands that ARE there, standing in a table next to finished waves
    (review M1). The run says how many strands it saw, and the wave's own
    orchestrator makes the line again at the end."""

    def _line(self, root):
        text = (root / "plans" / "retro" / "RETRO.md").read_text(encoding="utf-8")
        rows = [l for l in text.splitlines() if WAVE in l]
        self.assertEqual(len(rows), 1, text)
        return rows[0]

    def test_the_history_line_names_the_strands_it_was_measured_over(self):
        root, troot = fixture_wave(self)
        run_retro(self, root, troot, "--provisional")
        self.assertIn("vorläufig (2 Stränge)", self._line(root))
        retro = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.assertIn("vorläufig (2 Stränge)", retro)
        self.assertLessEqual(len(retro.splitlines()), 40)

    def test_the_final_run_replaces_the_provisional_line(self):
        root, troot = fixture_wave(self)
        run_retro(self, root, troot, "--provisional")
        run_retro(self, root, troot)
        self.assertNotIn("vorläufig", self._line(root))


class MetricTests(unittest.TestCase):
    def setUp(self):
        self.root, self.troot = fixture_wave(self)
        self.assertEqual(run_retro(self, self.root, self.troot), 0)
        self.retro = (self.root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.verlauf = (self.root / "plans" / "retro" / "RETRO.md").read_text(encoding="utf-8")

    def test_all_ten_metrics_are_in_the_report(self):
        for n in range(1, 11):
            self.assertRegex(self.retro, rf"\|\s*Q{n}\s*\|")

    def test_report_stays_under_forty_lines(self):
        self.assertLessEqual(len(self.retro.splitlines()), 40)

    def test_table_carries_value_threshold_verdict_and_suggestion(self):
        head = [l for l in self.retro.splitlines() if l.startswith("| Q |")]
        self.assertEqual(len(head), 1)
        for col in ("Wert", "Schwelle", "Verdikt", "Vorschlag"):
            self.assertIn(col, head[0])

    def test_q1_counts_three_strand_runs_over_two_strands(self):
        """alpha ran twice, beta once: 3/2 = 1.5 -- exactly at the threshold."""
        q = wr.metric_row(self.retro, "Q1")
        self.assertEqual(q["wert"], "1,5")
        self.assertEqual(q["verdikt"], "OK")

    def test_a_run_quoted_in_an_analysis_document_is_not_a_run_of_this_wave(self):
        """A wave that measures other waves carries their GATE-SUMMARY lines
        in its findings. Runs count only where a wave RAN them: the strand
        reports, the wave receipt and the receipt files."""
        self.assertEqual(wr.metric_row(self.retro, "Q1")["wert"], "1,5")
        self.assertIn("3 Strang-Gate-Läufe", self.retro)

    def test_q2_names_the_foreign_red_run(self):
        """One red run, and its cause was a foreign session -- 100 %."""
        q = wr.metric_row(self.retro, "Q2")
        self.assertEqual(q["wert"], "100 %")
        self.assertEqual(q["verdikt"], "VERSTOSS")

    def test_q5_counts_a_finding_once_although_the_review_lists_it_twice(self):
        """alpha's review names C1 and N2 in its summary table AND as a
        heading each. Counting both shapes doubles every review that has a
        table, and the form share then measures the table, not the review."""
        self.assertEqual(self.retro.count("| Q5 |"), 1)
        q = wr.metric_row(self.retro, "Q5")
        self.assertEqual(q["wert"], "33 %")

    def test_q5_reads_the_body_of_a_finding_not_only_its_heading(self):
        """N2's heading says nothing about wording; the sentence under it
        does. A classifier that stops at the heading calls it substance."""
        root, troot = fixture_wave(self)
        run_retro(self, root, troot)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.assertEqual(wr.metric_row(text, "Q5")["wert"], "33 %")

    def test_q5_splits_form_findings_from_real_ones(self):
        """Three findings, one of them wording -- 33 %. The third is written
        as a heading (`### N1 — ...`), the shape every H2/H3 review uses; a
        finder that only reads bullets would count two and report 50 %."""
        q = wr.metric_row(self.retro, "Q5")
        self.assertEqual(q["wert"], "33 %")

    def test_q6_counts_a_cache_creation_once_per_request_id(self):
        """Two assistant lines share req_a1 and carry the same usage; counting
        both would report two cache creations over 100k where there was one."""
        q = wr.metric_row(self.retro, "Q6")
        self.assertEqual(q["wert"], "123k / 1")

    def test_q4_measures_the_fix_round_against_the_build_not_the_wait(self):
        """alpha built for 6 min and fixed for 2 h; the 84 min it lay idle
        waiting for the review belong to neither section (finding 01 § 3.1)."""
        q = wr.metric_row(self.retro, "Q4")
        self.assertEqual(q["wert"], "10,0")
        self.assertEqual(q["verdikt"], "VERSTOSS")

    def test_q7_counts_every_poll_not_only_the_repeated_one(self):
        """Two reads of the same gate log and one `sleep` are three polls.

        The first look at a log is a poll like the second: the run is in the
        background, and every look costs a full turn over the agent's whole
        context (finding 02 section 3.4). Counting only the repeats reported
        46 for wave P where the transcripts hold over 2 000."""
        q = wr.metric_row(self.retro, "Q7")
        self.assertEqual(q["wert"], "3")
        self.assertEqual(q["verdikt"], "VERSTOSS")

    def test_q9_reports_the_two_hour_idle_gap(self):
        q = wr.metric_row(self.retro, "Q9")
        self.assertEqual(q["verdikt"], "VERSTOSS")

    def test_q10_measures_the_orchestrator_share(self):
        q = wr.metric_row(self.retro, "Q10")
        self.assertTrue(q["wert"].endswith("%"))

    def test_verlauf_line_carries_the_wave_and_all_ten_columns(self):
        row = [l for l in self.verlauf.splitlines() if WAVE in l]
        self.assertEqual(len(row), 1)
        self.assertEqual(row[0].count("|"), 14)

    def test_verlauf_line_is_idempotent(self):
        run_retro(self, self.root, self.troot)
        run_retro(self, self.root, self.troot)
        text = (self.root / "plans" / "retro" / "RETRO.md").read_text(encoding="utf-8")
        self.assertEqual(sum(1 for l in text.splitlines() if WAVE in l), 1)


class MessartefaktTests(unittest.TestCase):
    """The four numbers wave P measured as artefacts of its own bookkeeping.

    The first real run over wave P reported 8,3 gate runs per strand, a gate
    share of 141 percent, a fix round of 0,0 and 46 polls -- none of them a
    property of the wave. Every test here is one of those four, written over
    a fixture that carries the shape that produced it.
    """

    # --- Q1: whose run a quoted GATE-SUMMARY line is -------------------
    def _quoting_wave(self, commits="[aaa1111]"):
        """A report that QUOTES a foreign run next to its own, and keeps the
        receipt of its own. Wave P's strands reviewed other waves, so their
        reports carry dozens of GATE-SUMMARY lines that were never theirs."""
        wave = wave_with(self, **{"alpha.md":
            "---\n"
            "strang: alpha\n"
            "branch: welle-t/alpha\n"
            "issues: [#1]\n"
            "basis: 9999999\n"
            'gate: "GATE-SUMMARY strand aaa1111 12/12 600s GREEN"\n'
            f"commits: {commits}\n"
            "---\n\n"
            "## Gate\n"
            "GATE-SUMMARY strand aaa1111 12/12 600s GREEN\n\n"
            "## Befund zur Welle davor\n"
            "Ihr Bauer zitierte\n"
            "GATE-SUMMARY strand ffff999 9/12 4444s RED\n"
            "und einmal mehr\n"
            "GATE-SUMMARY strand eeee888 11/12 2222s RED\n"})
        (wave / "receipts" / "alpha").mkdir(parents=True)
        (wave / "receipts" / "alpha" / "last-strand.json").write_text(json.dumps({
            "mode": "strand",
            "rev": "aaa1111222233334444555566667777888899990",
            "verdict": "GREEN", "lock_wait_secs": 0,
            "started": "2026-01-01T10:00:00",
            "finished": "2026-01-01T10:10:00",
            "stations": [{"name": "tests", "verdict": "GREEN"}],
        }), encoding="utf-8")
        return wave

    def test_a_quoted_foreign_run_is_not_a_run_of_this_strand(self):
        """Only a run whose rev is a commit of the strand, or a receipt in
        the strand's own archive, is the strand's."""
        runs = gates.runs_of(self._quoting_wave(), "strand")
        self.assertEqual([r["rev"][:7] for r in runs], ["aaa1111"])

    def test_the_archive_receipt_and_its_quoted_line_are_one_run(self):
        """The receipt carries the full sha, the report the short one. Keyed
        by the full string they are two runs of the same commit."""
        runs = gates.runs_of(self._quoting_wave(), "strand")
        self.assertEqual(len(runs), 1)
        self.assertEqual(runs[0]["secs"], 600)

    def test_a_strand_without_commits_in_its_head_keeps_every_quoted_line(self):
        """H2, H3 and G predate the head block. Without a commit list there
        is nothing to filter against, so their reports count as before."""
        wave = wave_with(self, **{"alpha-report.md":
            "# alpha\n"
            "GATE-SUMMARY strand aaa1111 12/12 600s GREEN\n"
            "GATE-SUMMARY strand bbb2222 11/12 400s RED\n"})
        self.assertEqual(len(gates.runs_of(wave, "strand")), 2)

    def test_a_receipt_of_a_throwaway_repo_is_not_a_run_of_the_strand(self):
        """`receipts/<strand>/` of a strand that worked in the main tree
        collected 33 one-second receipts of throw-away repos -- a test that
        pointed its archive at the real one. Their revs are commits of no
        strand, so the commit list drops them."""
        wave = self._quoting_wave()
        (wave / "receipts" / "alpha" / "strand-dead.json").write_text(json.dumps({
            "mode": "strand", "rev": "dead0000beef", "verdict": "GREEN",
            "started": "2026-01-01T11:45:03", "finished": "2026-01-01T11:45:03",
            "stations": [{"name": "ok", "verdict": "GREEN"}],
        }), encoding="utf-8")
        self.assertEqual([r["rev"][:7] for r in gates.runs_of(wave, "strand")],
                         ["aaa1111"])

    # --- Q3: gate seconds against the wall clock of a strand -----------
    def test_q3_is_na_when_the_gate_seconds_pass_the_wall_clock(self):
        """A share above 100 percent is not a measurement, it is a run that
        belongs to no strand -- wave P reported 141 percent."""
        root, troot = fixture_wave(self)
        bericht = root / "plans" / WAVE / "berichte" / "beta-report.md"
        bericht.write_text(
            "# beta\nGATE-SUMMARY strand abc1234 12/12 99000s GREEN\n",
            encoding="utf-8")
        run_retro(self, root, troot)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        q = wr.metric_row(text, "Q3")
        self.assertEqual(q["wert"], "n/a")
        self.assertIn("Wanduhr", q["vorschlag"])

    def test_q3_counts_the_wall_clock_up_to_the_review_of_the_same_strand(self):
        """The strand's wall clock ends with the last turn of its review or
        its fix round, not with the last turn of its builder."""
        root, troot = fixture_wave(self)
        subs = troot / SESSION / "subagents"
        (subs / "agent-ccc1.jsonl").write_text("\n".join([
            _user("2026-01-01T14:00:00Z", "Du bist der Reviewer von alpha."),
            _turn("2026-01-01T20:00:00Z", "req_c1", 5, 1_000, 2_000, 100),
        ]) + "\n", encoding="utf-8")
        (subs / "agent-ccc1.meta.json").write_text(
            json.dumps({"description": "Review Strang alpha"}), encoding="utf-8")
        run_retro(self, root, troot)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.assertEqual(wr.metric_row(text, "Q3")["wert"], "5 %")

    # --- Q4: a fix round is an agent of its own ------------------------
    def test_q4_measures_a_separate_fix_round_agent_against_its_builder(self):
        """Wave P gave every fix round a fresh agent (`Fix-Runde <strand>`),
        so no builder had a second section and Q4 read 0,0."""
        root, troot = fixture_wave(self)
        subs = troot / SESSION / "subagents"
        (subs / "agent-ddd1.jsonl").write_text("\n".join([
            _user("2026-01-01T12:00:00Z", "Du bist der Bauer des Strangs gamma."),
            _turn("2026-01-01T12:10:00Z", "req_d1", 5, 2_000, 0, 100),
        ]) + "\n", encoding="utf-8")
        (subs / "agent-ddd1.meta.json").write_text(
            json.dumps({"description": "P9 gamma bauen"}), encoding="utf-8")
        (subs / "agent-ddd2.jsonl").write_text("\n".join([
            _user("2026-01-01T13:00:00Z", "Du bist der Bauer des Strangs gamma."),
            _turn("2026-01-01T13:05:00Z", "req_d2", 5, 2_000, 0, 100),
        ]) + "\n", encoding="utf-8")
        (subs / "agent-ddd2.meta.json").write_text(
            json.dumps({"description": "Fix-Runde P9 gamma"}), encoding="utf-8")
        run_retro(self, root, troot)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        # gamma: 600 s built, 300 s fixed -> 0,5; alpha 20,0; beta 0,0.
        self.assertEqual(wr.metric_row(text, "Q4")["wert"], "0,5")

    def test_a_fix_round_agent_is_not_counted_as_a_builder(self):
        root, troot = fixture_wave(self)
        subs = troot / SESSION / "subagents"
        (subs / "agent-ddd2.jsonl").write_text("\n".join([
            _user("2026-01-01T13:00:00Z", "Du bist der Bauer des Strangs alpha."),
            _turn("2026-01-01T13:05:00Z", "req_d2", 5, 900_000, 0, 100),
        ]) + "\n", encoding="utf-8")
        (subs / "agent-ddd2.meta.json").write_text(
            json.dumps({"description": "Fix-Runde alpha"}), encoding="utf-8")
        run_retro(self, root, troot)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.assertEqual(wr.metric_row(text, "Q6")["wert"], "123k / 1")
        self.assertIn("(2 Bauer)", text)

    # --- Q7: every poll, whatever shape it takes -----------------------
    def test_q7_counts_a_task_output_a_gate_log_and_an_idle_echo(self):
        """Three shapes the guard of 19.09. names, and none of them is a
        repeated read of a `.log` file: a background task's output, a gate
        station log under `target/gate/**/logs`, and `echo idle`."""
        root, troot = fixture_wave(self)
        subs = troot / SESSION / "subagents"
        (subs / "agent-eee1.jsonl").write_text("\n".join([
            _user("2026-01-01T15:00:00Z", "Du bist der Bauer des Strangs delta."),
            _turn("2026-01-01T15:01:00Z", "req_e1", 5, 1_000, 0, 10,
                  [{"type": "tool_use", "id": "p1", "name": "Bash",
                    "input": {"command": "tail -2 /tmp/x/tasks/b3l3m4gqm.output"}}]),
            _turn("2026-01-01T15:02:00Z", "req_e2", 5, 1_000, 0, 10,
                  [{"type": "tool_use", "id": "p2", "name": "Read",
                    "input": {"file_path": "/tmp/x/tasks/bpdeeck41.output"}}]),
            _turn("2026-01-01T15:03:00Z", "req_e3", 5, 1_000, 0, 10,
                  [{"type": "tool_use", "id": "p3", "name": "Bash",
                    "input": {"command": "echo idle"}}]),
            _turn("2026-01-01T15:04:00Z", "req_e4", 5, 1_000, 0, 10,
                  [{"type": "tool_use", "id": "p4", "name": "Bash",
                    "input": {"command": "cat /home/x/target/gate/wt/logs/"
                                         "strand-tests.log"}}]),
            _turn("2026-01-01T15:05:00Z", "req_e5", 5, 1_000, 0, 10,
                  [{"type": "tool_use", "id": "p5", "name": "Bash",
                    "input": {"command": "git diff --stat"}}]),
        ]) + "\n", encoding="utf-8")
        (subs / "agent-eee1.meta.json").write_text(
            json.dumps({"description": "P9 delta bauen"}), encoding="utf-8")
        run_retro(self, root, troot)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.assertEqual(wr.metric_row(text, "Q7")["wert"], "7")


class MissingSourceTests(unittest.TestCase):
    def test_a_wave_without_transcripts_still_produces_a_report(self):
        """No transcript is a `n/a` with a reason, never a crash."""
        root, _troot = fixture_wave(self)
        rc = wr.main([WAVE, "--root", str(root), "--transcripts",
                      str(root / "nowhere")])
        self.assertEqual(rc, 0)
        text = (root / "plans" / WAVE / "retro.md").read_text(encoding="utf-8")
        self.assertIn("n/a", text)
        for n in (6, 7, 9, 10):
            self.assertEqual(wr.metric_row(text, f"Q{n}")["wert"], "n/a")

    def test_an_empty_wave_directory_is_not_an_error(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        (root / "plans" / "welle-leer-2026-01-01").mkdir(parents=True)
        self.assertEqual(
            wr.main(["welle-leer-2026-01-01", "--root", str(root)]), 0)

    def test_an_unknown_wave_is_reported_and_exits_zero(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.assertEqual(wr.main(["welle-gibtsnicht", "--root", tmp.name]), 0)


class CheckAndReadmeTests(unittest.TestCase):
    def test_check_is_red_while_the_retro_is_missing_and_green_after(self):
        root, troot = fixture_wave(self)
        self.assertEqual(wr.main(["--check", WAVE, "--root", str(root)]), 1)
        run_retro(self, root, troot)
        self.assertEqual(wr.main(["--check", WAVE, "--root", str(root)]), 0)

    def test_readme_is_generated_from_the_thresholds_file(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        self.assertEqual(wr.main(["--readme", "--root", str(root)]), 0)
        text = (root / "plans" / "retro" / "README.md").read_text(encoding="utf-8")
        spec = wr.load_thresholds()
        for m in spec["metrics"]:
            self.assertIn(m["id"], text)
            self.assertIn(m["title"], text)
        self.assertIn(spec["decided"], text)

    def test_the_committed_readme_matches_the_thresholds_file(self):
        """The README is generated, so a hand edit to either half is drift."""
        committed = REPO / "plans" / "retro" / "README.md"
        if not committed.is_file():
            self.skipTest("plans/retro/README.md not in this tree")
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        wr.main(["--readme", "--root", tmp.name])
        fresh = pathlib.Path(tmp.name) / "plans" / "retro" / "README.md"
        self.assertEqual(committed.read_text(encoding="utf-8"),
                         fresh.read_text(encoding="utf-8"))


class CliTests(unittest.TestCase):
    def test_the_script_runs_as_a_file_and_exits_zero(self):
        root, troot = fixture_wave(self)
        r = subprocess.run(
            [sys.executable, str(SCRIPTS / "wave_retro.py"), WAVE,
             "--root", str(root), "--transcripts", str(troot),
             "--sessions", SESSION],
            capture_output=True, text=True, timeout=120)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("retro.md", r.stdout)


if __name__ == "__main__":
    unittest.main()
