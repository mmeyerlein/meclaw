#!/usr/bin/env python3
"""Capture what a rebuild of the curator asks the summarizer (GH #1465).

    python3 scripts/curator_bank.py capture --out <dir>

The bank of constructed windows lives in the cell tests
(`crates/meclaw-cells/tests/support/curator_bank.rs`); its walls are Rust
fixtures and the rebuild runs in the shipped curator hive there. This script
builds no wall and runs no rebuild of its own: it runs the one test that
does, `gh_d58_bank_the_summarizer_request_is_captured`, through the strand
kit (`scripts/strand.sh test`), reads the lines that test prints

    CURATOR-BANK-CAPTURE <file> <sha256 of the json> <json>

and writes each as `<dir>/<file>`: one file per wall, role and form of the
request, holding the request(s) the rebuild sent and the list of the planted
points with the keyword each is known by. Beside them it writes
`manifest.json` (every file with its counts and, per planted point, its
`tier` and `expected_in_request`) and `capture.log` (what the kit said of
the run). No model is called: a stub answers every request in the test.

What a request has to hold follows the tiers of the role, and the test says
it per point: `expected_in_request` is true for a point that left the raw
part of its window and lies in the tier `summary`; a point of the tier
`none` is dropped by the role and must not stand in a request sent to the
summarizer (`must_be_absent`). Whoever measures a summary against its
request counts the points with `expected_in_request` and no others.

Two kinds carry more (GH #1490): a `third_party` point (the person's fact
about a third person) names that person as `subject`; a `superseded` point
(a value a later row replaces) names the keyword of both rows, `old_key` and
`new_key`, and the capture holds the old row under `old` with its own tier
and `expected_in_request`. The manifest carries these fields per point and
judges the old row like a planted row of its own: `left_raw`,
`expected_in_request`, `missing` and `unexpected` count rows, `planted`
counts points.

The kit shows the end of a test's output only and keeps the whole of it in
the test log it names (`strand: test log <path>`); the capture is read from
there.

The run needs what every single test of a strand needs: the lane lease of
the strand (`scripts/strand.sh token take`). `--from-log <file>` reads the
output of an earlier run instead of starting one.

Exit: 0 = the test is green, every planted point expected in its request
stands in it and none that must be absent does; 1 = the test is red, a line
is damaged, an expected point is missing or a dropped one stands in a
request (the files are written all the same); 2 = no capture at all (the
run was refused, or the output holds no line).
"""

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

MARK = "CURATOR-BANK-CAPTURE"
TEST = "gh_d58_bank_the_summarizer_request_is_captured"
FILTER = "binary(/curator_/) & test(/%s/)" % TEST
# `<wall>.<role>.<form>.json`, nothing that could leave the directory.
FILE_NAME = re.compile(r"^[a-z0-9_]+\.[a-z0-9_]+\.[a-z0-9_]+\.json$")
# The line the kit names the full log of a single-test run with.
KIT_LOG = "strand: test log "
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def kit_command(root):
    """The command that runs the capture test through the strand kit; the
    output of a green test is shown, the capture is in it."""
    return [os.path.join(root, "scripts", "strand.sh"), "-C", root, "test", FILTER,
            "--success-output", "immediate"]


def test_log_of(lines):
    """The test log the kit named in its output, or None."""
    for line in lines:
        if line.startswith(KIT_LOG):
            return line[len(KIT_LOG):].strip()
    return None


def parse(lines):
    """The captures in `lines`: `({file: record}, [problem])`. A line whose
    json does not match its checksum, or that names a file twice with two
    contents, is a problem and no capture."""
    found, sums, problems = {}, {}, []
    for line in lines:
        at = line.find(MARK + " ")
        if at < 0:
            continue
        parts = line[at:].rstrip("\r\n").split(" ", 3)
        if len(parts) != 4:
            problems.append("a capture line without its four parts")
            continue
        _, name, digest, text = parts
        if not FILE_NAME.match(name):
            problems.append("a capture line names no file of a capture: %r" % name[:80])
            continue
        if hashlib.sha256(text.encode("utf-8")).hexdigest() != digest:
            problems.append("%s: the line does not match its checksum" % name)
            continue
        if name in sums:
            if sums[name] != digest:
                problems.append("%s: captured twice with two contents" % name)
            continue
        try:
            record = json.loads(text)
        except ValueError as err:
            problems.append("%s: no json (%s)" % (name, err))
            continue
        found[name], sums[name] = record, digest
    return found, problems


def rows_of(planted):
    """Every planted row: each point, and the old row of a `superseded` one."""
    for p in planted:
        yield p
        if isinstance(p.get("old"), dict):
            yield p["old"]


def point_of(p):
    """The manifest entry of one planted point."""
    entry = {"key": p.get("key"), "kind": p.get("kind"), "tier": p.get("tier"),
             "left_raw": bool(p.get("left_raw")),
             "expected_in_request": bool(p.get("expected_in_request")),
             "in_request": bool(p.get("text_in_request"))}
    if "subject" in p:
        entry["subject"] = p.get("subject")
    if isinstance(p.get("old"), dict):
        old = p["old"]
        entry.update({"old_key": p.get("old_key"), "new_key": p.get("new_key"),
                      "old_tier": old.get("tier"),
                      "old_left_raw": bool(old.get("left_raw")),
                      "old_expected_in_request": bool(old.get("expected_in_request")),
                      "old_in_request": bool(old.get("text_in_request"))})
    return entry


def summary_of(name, record):
    """The manifest row of one capture."""
    planted = record.get("planted") or []
    rows = list(rows_of(planted))
    left = [p for p in rows if p.get("left_raw")]
    expected = [p for p in rows if p.get("expected_in_request")]
    requests = record.get("requests") or []
    return {
        "file": name,
        "wall": record.get("wall"),
        "role": record.get("role"),
        "form": record.get("form"),
        "knobs": record.get("knobs") or {},
        "requests": len(requests),
        "routes": [r.get("route") for r in requests],
        "planted": len(planted),
        "left_raw": len(left),
        "tiers": record.get("tiers") or [],
        "expected_in_request": len(expected),
        # A planted point expected in the request must stand in it, whole.
        "missing": [p.get("key") for p in expected
                    if not requests or not p.get("text_in_request")],
        # A point the role drops must not stand in a request to the summarizer.
        "unexpected": [p.get("key") for p in rows
                       if p.get("must_be_absent")
                       and (p.get("key_in_request") or p.get("text_in_request"))],
        "points": [point_of(p) for p in planted],
    }


def write(out, found, problems, command, status):
    """Writes the captures and the manifest into `out`; returns the manifest."""
    os.makedirs(out, exist_ok=True)
    rows = []
    for name in sorted(found):
        with open(os.path.join(out, name), "w", encoding="utf-8") as fh:
            json.dump(found[name], fh, indent=1, ensure_ascii=False, sort_keys=True)
            fh.write("\n")
        rows.append(summary_of(name, found[name]))
    manifest = {"test": TEST, "command": command, "exit_of_the_run": status,
                "files": rows, "problems": problems}
    with open(os.path.join(out, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, indent=1, ensure_ascii=False, sort_keys=True)
        fh.write("\n")
    return manifest


def run_kit(root, log_path):
    """Runs the capture test through the kit; the output goes to `log_path`
    as it comes. Returns `(lines, exit status)`."""
    command = kit_command(root)
    lines = []
    with open(log_path, "w", encoding="utf-8") as log:
        proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                stdin=subprocess.DEVNULL, encoding="utf-8", errors="replace")
        for line in proc.stdout:
            log.write(line)
            lines.append(line)
            if MARK + " " not in line:
                sys.stderr.write(line)
        status = proc.wait()
    full = test_log_of(lines)
    if full and os.path.isfile(full):
        with open(full, encoding="utf-8", errors="replace") as fh:
            lines = lines + fh.readlines()
    return lines, status


def capture(args):
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    if args.from_log:
        with open(args.from_log, encoding="utf-8", errors="replace") as fh:
            lines = fh.readlines()
        command, status = ["--from-log", os.path.basename(args.from_log)], 0
    else:
        lines, status = run_kit(args.root, os.path.join(out, "capture.log"))
        command = ["scripts/strand.sh", "test", FILTER, "--success-output", "immediate"]
    found, problems = parse(lines)
    if not found:
        sys.stderr.write("curator_bank: no capture in the output of the run (exit %d)%s\n"
                         % (status, "".join("\n  " + p for p in problems)))
        return 2
    manifest = write(out, found, problems, command, status)
    missing = unexpected = 0
    for row in manifest["files"]:
        missing += len(row["missing"])
        unexpected += len(row["unexpected"])
        print("%s  requests=%d planted=%d left_raw=%d expected=%d missing=%d unexpected=%d"
              % (row["file"], row["requests"], row["planted"], row["left_raw"],
                 row["expected_in_request"], len(row["missing"]), len(row["unexpected"])))
    for problem in problems:
        print("PROBLEM %s" % problem)
    print("curator_bank: %d file(s) in %s, %d expected point(s) missing, %d dropped point(s) "
          "in a request, %d problem(s), the run ended with %d"
          % (len(found), out, missing, unexpected, len(problems), status))
    return 0 if status == 0 and not problems and not missing and not unexpected else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="verb", required=True)
    cap = sub.add_parser("capture", help="write the requests a rebuild sends the summarizer")
    cap.add_argument("--out", required=True, help="the directory the capture is written to")
    cap.add_argument("--from-log", help="read the output of an earlier run instead of running")
    cap.add_argument("--root", default=ROOT, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    return capture(args)


if __name__ == "__main__":
    sys.exit(main())
