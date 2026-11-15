#!/usr/bin/env python3
"""Capture what a rebuild of the curator asks the summarizer (GH #1465).

    python3 scripts/curator_bank.py capture --out <dir>
    python3 scripts/curator_bank.py capture --window --out <dir>

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

`--window` (GH #1527, d58-3 tag, contract point 7, OR-583.tag.4) captures
the other test of the bank, `gh_d583_tag_bank_the_day_window_is_captured`,
the same way: the window after one rebuild on the running day, per wall,
role and arm of the switch `day_floor`, one file
`<wall>.<role>.floor_<arm>.json` each -- the messages of the call and the
plan snapshot the ledger keeps for it. Arm "1" is the whole running day raw,
arm "0" the running day curated by the horizon of the role: the two arms of
the measurement with real models (M1). The manifest names per window the
wall, role, horizon, arm, the mark of the turn and the reason of the
rebuild, whether every piece of the day stayed raw (`whole_day`), the
requests to the summarizer and the size of the window. Exit: 0
= the test is green and every line reads; 1 = the test is red or a line is
damaged (the files are written all the same); 2 = no capture at all.

    python3 scripts/curator_bank.py capture --window --roles <roles.json> --out <dir>

`--roles` (OR-583.tag.5) names the roles of the window capture: the window
values of a role live in the product's set, so the measurement hands them
in. The file is a JSON list, one entry per role:

    [{"name": "role_a", "set": "consult",
      "knobs": {"horizon": "all", "keep_recent": 2, "window_target": 24000}}]

`name` ([a-z0-9_]+, unique) stands in the file names, `set` is the example
value set of the bank the role runs on (talky, quick, consult, coding,
research), `knobs` are params of `./policy` set on top -- the horizon among
them (one of day, task, all), else the set's own. The capture copies the
file beside the test for the run (`ROLES_PATH`, the kit carries it to the
lane) and removes it after. The test fails on a file that lies there and
does not read; after the run the capture compares the roles of the windows
(name, set, knobs, and that they came from the file) with the ordered ones
and is exit 1 when they differ -- a run on the defaults never passes for
the ordered roles.
Without `--roles` the test runs its defaults, the values of the product set
0.61: `task` (consult), `all` (consult, `keep_recent` 2, `window_target`
24000), `day` (talky, the control arm); a window whose roles did not come
from the defaults (a roles file left beside the test) is exit 1. A file that
does not read is exit 2, nothing is run.
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
# The test of `--window`: the windows of a day change (GH #1527).
WINDOW_TEST = "gh_d583_tag_bank_the_day_window_is_captured"
WINDOW_FILTER = "binary(/curator_/) & test(/%s/)" % WINDOW_TEST
# `<wall>.<role>.<form>.json`, nothing that could leave the directory.
FILE_NAME = re.compile(r"^[a-z0-9_]+\.[a-z0-9_]+\.[a-z0-9_]+\.json$")
# The line the kit names the full log of a single-test run with.
KIT_LOG = "strand: test log "
# Where the window test reads the roles of `--roles` (relative to the tree).
ROLES_PATH = os.path.join("crates", "meclaw-cells", "tests", "curator_window_roles.json")
ROLE_NAME = re.compile(r"^[a-z0-9_]+$")
VALUE_SETS = ("talky", "quick", "consult", "coding", "research")
# The horizons of `./policy`; any other is refused, never a silent `all`.
HORIZONS = ("day", "task", "all")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def kit_command(root, window=False):
    """The command that runs the capture test through the strand kit -- the
    test of the windows of a day change with `window` --; the output of a
    green test is shown, the capture is in it."""
    return [os.path.join(root, "scripts", "strand.sh"), "-C", root, "test",
            WINDOW_FILTER if window else FILTER, "--success-output", "immediate"]


def roles_of(path):
    """The roles of `--roles` as a list, or ValueError with the reason."""
    try:
        with open(path, encoding="utf-8") as fh:
            roles = json.load(fh)
    except (OSError, ValueError) as e:
        raise ValueError("%s does not read: %s" % (path, e))
    if not isinstance(roles, list) or not roles:
        raise ValueError("%s: a non-empty list of roles" % path)
    names = set()
    for role in roles:
        if not isinstance(role, dict) or set(role) - {"name", "set", "knobs"}:
            raise ValueError("%s: a role is {name, set, knobs}: %r" % (path, role))
        name = role.get("name")
        if not isinstance(name, str) or not ROLE_NAME.match(name) or name in names:
            raise ValueError("%s: a unique name of [a-z0-9_]: %r" % (path, role))
        names.add(name)
        if role.get("set") not in VALUE_SETS:
            raise ValueError("%s: the set of %s is one of %s" % (path, name, ", ".join(VALUE_SETS)))
        knobs = role.get("knobs", {})
        if not isinstance(knobs, dict) or not all(isinstance(k, str) and k for k in knobs):
            raise ValueError("%s: the knobs of %s are an object of params" % (path, name))
        if "horizon" in knobs and knobs["horizon"] not in HORIZONS:
            raise ValueError("%s: the horizon of %s is one of %s: %r"
                             % (path, name, ", ".join(HORIZONS), knobs["horizon"]))
    return roles


def roles_not_taken(roles, found):
    """What the windows in `found` say against the ordered `roles`: a list
    of reasons, empty when every window ran an ordered role from the file
    and every ordered role has a window."""
    ordered = {r["name"]: r for r in roles}
    reasons, seen = [], set()
    for name in sorted(found):
        rec = found[name]
        role = ordered.get(rec.get("role"))
        if rec.get("roles") != "file":
            reasons.append("%s: the roles came from %r, not from the file"
                           % (name, rec.get("roles")))
        if role is None:
            reasons.append("%s: the role %r was not ordered" % (name, rec.get("role")))
            continue
        seen.add(role["name"])
        if rec.get("set") != role["set"]:
            reasons.append("%s: the set %r, ordered %r" % (name, rec.get("set"), role["set"]))
        if rec.get("knobs") != role.get("knobs", {}):
            reasons.append("%s: the knobs %r, ordered %r"
                           % (name, rec.get("knobs"), role.get("knobs", {})))
    for missing in sorted(set(ordered) - seen):
        reasons.append("the role %s has no window" % missing)
    return reasons


def roles_not_defaults(found):
    """What the windows in `found` say against a run without `--roles`: a
    list of reasons, empty when every window ran the test's defaults. A
    roles file left beside the test (a capture killed before it removed it)
    would otherwise pass for the defaults (re-check M-E)."""
    return ["%s: the roles came from %r, not from the defaults" % (name, found[name].get("roles"))
            for name in sorted(found) if found[name].get("roles") != "defaults"]


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


def window_summary_of(name, record):
    """The manifest row of one captured window (`--window`)."""
    window = record.get("window") or []
    return {
        "file": name,
        "wall": record.get("wall"),
        "role": record.get("role"),
        "horizon": record.get("horizon"),
        "day_floor": record.get("day_floor"),
        "mark": record.get("mark"),
        "reason": record.get("reason"),
        "rebuilt": bool(record.get("rebuilt")),
        "pieces": record.get("pieces") or 0,
        "raw_pieces": record.get("raw_pieces") or 0,
        "whole_day": bool(record.get("whole_day")),
        "requests": record.get("requests") or 0,
        "window_chars": record.get("window_chars") or 0,
        "messages": len(window),
        "snapshot": isinstance(record.get("snapshot"), list),
    }


def write(out, found, problems, command, status, window=False):
    """Writes the captures and the manifest into `out`; returns the manifest."""
    os.makedirs(out, exist_ok=True)
    rows = []
    for name in sorted(found):
        with open(os.path.join(out, name), "w", encoding="utf-8") as fh:
            json.dump(found[name], fh, indent=1, ensure_ascii=False, sort_keys=True)
            fh.write("\n")
        rows.append((window_summary_of if window else summary_of)(name, found[name]))
    manifest = {"test": WINDOW_TEST if window else TEST, "command": command,
                "exit_of_the_run": status,
                "files": rows, "problems": problems}
    with open(os.path.join(out, "manifest.json"), "w", encoding="utf-8") as fh:
        json.dump(manifest, fh, indent=1, ensure_ascii=False, sort_keys=True)
        fh.write("\n")
    return manifest


def run_kit(root, log_path, window=False):
    """Runs the capture test through the kit; the output goes to `log_path`
    as it comes. Returns `(lines, exit status)`."""
    command = kit_command(root, window)
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
    roles = None
    if args.roles:
        if not args.window:
            sys.stderr.write("curator_bank: --roles belongs to --window\n")
            return 2
        try:
            roles = roles_of(args.roles)
        except ValueError as e:
            sys.stderr.write("curator_bank: %s\n" % e)
            return 2
    out = os.path.abspath(args.out)
    os.makedirs(out, exist_ok=True)
    if args.from_log:
        with open(args.from_log, encoding="utf-8", errors="replace") as fh:
            lines = fh.readlines()
        command, status = ["--from-log", os.path.basename(args.from_log)], 0
    else:
        placed = None
        if args.roles:
            placed = os.path.join(args.root, ROLES_PATH)
            with open(placed, "w", encoding="utf-8") as fh:
                json.dump(roles, fh, indent=1, ensure_ascii=False)
                fh.write("\n")
        try:
            lines, status = run_kit(args.root, os.path.join(out, "capture.log"), args.window)
        finally:
            if placed and os.path.exists(placed):
                os.remove(placed)
        command = ["scripts/strand.sh", "test", WINDOW_FILTER if args.window else FILTER,
                   "--success-output", "immediate"]
    found, problems = parse(lines)
    if not found:
        sys.stderr.write("curator_bank: no capture in the output of the run (exit %d)%s\n"
                         % (status, "".join("\n  " + p for p in problems)))
        return 2
    if args.window:
        status = report_windows(write(out, found, problems, command, status, window=True),
                                out, status)
        if roles is not None:
            reasons = roles_not_taken(roles, found)
            for reason in reasons:
                print("ROLES %s" % reason)
            if reasons:
                print("curator_bank: the run did not take the roles of %s" % args.roles)
                return 1
        else:
            reasons = roles_not_defaults(found)
            for reason in reasons:
                print("ROLES %s" % reason)
            if reasons:
                print("curator_bank: the run took roles nobody ordered; a roles file may lie at %s"
                      % ROLES_PATH)
                return 1
        return status
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


def report_windows(manifest, out, status):
    """The lines of a window capture; the exit of `capture --window`."""
    for row in manifest["files"]:
        print("%s  mark=%s reason=%s whole_day=%s raw=%d/%d requests=%d messages=%d chars=%d"
              % (row["file"], row["mark"], row["reason"], row["whole_day"], row["raw_pieces"],
                 row["pieces"], row["requests"], row["messages"], row["window_chars"]))
    problems = manifest["problems"]
    for problem in problems:
        print("PROBLEM %s" % problem)
    print("curator_bank: %d window(s) in %s, %d problem(s), the run ended with %d"
          % (len(manifest["files"]), out, len(problems), status))
    return 0 if status == 0 and not problems else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="verb", required=True)
    cap = sub.add_parser("capture", help="write the requests a rebuild sends the summarizer")
    cap.add_argument("--out", required=True, help="the directory the capture is written to")
    cap.add_argument("--from-log", help="read the output of an earlier run instead of running")
    cap.add_argument("--window", action="store_true",
                     help="capture the windows of the running day instead, whole and curated (GH #1527)")
    cap.add_argument("--roles", help="with --window: a JSON list of the roles to capture "
                     "({name, set, knobs}, OR-583.tag.5)")
    cap.add_argument("--root", default=ROOT, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    return capture(args)


if __name__ == "__main__":
    sys.exit(main())
