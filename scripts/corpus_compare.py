#!/usr/bin/env python3
"""Compare the curator's window corpus of an old tree with the one recorded now.

    python3 scripts/corpus_compare.py record  [--root <tree>]
    python3 scripts/corpus_compare.py compare [--root <tree>] [--base <rev>] [--free <file>]

The cell test `gh_d38_c0_policy_corpus_is_byte_identical` drives the shipped
curator hive through a fixed set of cases and compares, step by step, every
message that leaves the hive and every mark it writes against fixtures under
`crates/meclaw-cells/tests/fixtures/d38_c0/`. It knows no partial green: a
strand that changes a label of a call (a route, a hop key) records the whole
corpus anew, and the test alone can no longer say whether a window moved.

This script is the judge for that case. `record` runs the corpus test through
the strand kit with its bless switch (`MECLAW_D38_C0_BLESS=1`): the fixtures
are written instead of compared and come back into the tree. `compare` reads
the fixtures of the old tree (`--base`, a git revision; default the merge
base of `master` and `HEAD`) and those in the working tree, and demands, for
every case and step:

- the body parts `system`, `messages` and `tool_scope` of every message out
  of the hive byte for byte -- nothing may free them;
- every other part of such a message (`route`, each recorded `hop.<key>`,
  each other `body.<key>`) and every request to the summarizer
  (`summarizer.<key>`) byte for byte, unless the free list names it;
- every mark byte for byte, unless the free list names its kind.

Within a step the messages, the requests and the marks are each a multiset
(the order of two deliveries is the depth of their chains in the hive, not
behaviour), compared after the corpus test's one normalisation `sub_ms` (the
microseconds below the millisecond of a corpus clock reading). Steps pair by
position; a different step label is said as a note, not a difference (it is
the test's wording). A case only in the old tree differs in every step; a
case only in the new tree is said as a note.

The free list (`--free`) is a JSON file of the strand:

    {"free": [{"field": "route", "why": "..."},
              {"field": "hop.target", "why": "..."},
              {"mark": "escalate", "why": "..."},
              {"extra": "catalog_ask", "without": ["hop.catalog_for"], "why": "..."}]}

Every entry names exactly one `field`, one `mark` kind or one `extra` route
and a non-empty reason. An `extra` entry lets a step of the new tree carry
additional messages out of the hive on that route which lack every field of
its list `without` (`route`, `hop.<key>` or `body.<key>`); a message of any
other route, one that carries such a field, and every message the new tree
lacks stay a difference. An entry that frees `body.system`, `body.messages`
or `body.tool_scope`, names anything else or has no reason is refused
(exit 2).

Output: one line per differing step with its first difference
(`DIFF <case> <step>: ...`), notes (`NOTE ...`), then the summary line
`CORPUS <n> steps <m> differ GREEN|RED`. Exit 0 = GREEN, 1 = RED, 2 = the
input could not be read (no fixtures, a bad free list, a bad revision).
"""

import argparse
import json
import os
import re
import subprocess
import sys

FIXTURES = "crates/meclaw-cells/tests/fixtures/d38_c0"
TEST = "gh_d38_c0_policy_corpus_is_byte_identical"
FILTER = "binary(/curator_/) & test(/%s/)" % TEST
BLESS = "MECLAW_D38_C0_BLESS"
LOCKED = ("system", "messages", "tool_scope")
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIELD = re.compile(r"^(route|(hop|body|summarizer)\.[A-Za-z0-9_]+)$")


class InputError(Exception):
    """The input could not be read: exit 2."""


def record_command(root):
    """The kit command that records the corpus anew (the bless switch is
    handed in the environment, never on the command line)."""
    return [os.path.join(root, "scripts", "strand.sh"), "-C", root, "test", FILTER]


def record(root):
    env = dict(os.environ)
    env[BLESS] = "1"
    proc = subprocess.run(record_command(root), env=env, stdin=subprocess.DEVNULL)
    return proc.returncode


def git(root, *args):
    proc = subprocess.run(["git", "-C", root] + list(args), capture_output=True,
                          encoding="utf-8", errors="replace")
    if proc.returncode != 0:
        raise InputError("git %s: %s" % (" ".join(args), proc.stderr.strip()))
    return proc.stdout


def base_of(root, base):
    if base:
        return base
    return git(root, "merge-base", "master", "HEAD").strip()


def old_fixtures(root, rev):
    names = git(root, "ls-tree", "--name-only", rev, FIXTURES + "/").split()
    out = {}
    for path in names:
        if not path.endswith(".json"):
            continue
        text = git(root, "show", "%s:%s" % (rev, path))
        out[os.path.basename(path)[:-5]] = json.loads(text)
    return out


def new_fixtures(root):
    d = os.path.join(root, FIXTURES)
    if not os.path.isdir(d):
        raise InputError("no fixtures at %s" % d)
    out = {}
    for name in sorted(os.listdir(d)):
        if name.endswith(".json"):
            with open(os.path.join(d, name), encoding="utf-8") as fh:
                out[name[:-5]] = json.load(fh)
    return out


def read_free(path):
    """`(fields, marks, extras)` of the free list; refuses what it may not
    free. `extras` is a list of `(route, without)` pairs."""
    if not path:
        return set(), set(), []
    try:
        with open(path, encoding="utf-8") as fh:
            doc = json.load(fh)
    except (OSError, ValueError) as e:
        raise InputError("free list %s: %s" % (path, e))
    entries = doc.get("free") if isinstance(doc, dict) else None
    if not isinstance(entries, list):
        raise InputError("free list %s: no list `free`" % path)
    fields, marks, extras = set(), set(), []
    for e in entries:
        if not isinstance(e, dict) or not str(e.get("why") or "").strip():
            raise InputError("free list entry without a reason: %r" % (e,))
        keys = set(e) - {"why"}
        if keys == {"field"}:
            f = str(e["field"])
            if not FIELD.match(f):
                raise InputError("free list: %r is no field of a message" % f)
            if f in ["body." + k for k in LOCKED]:
                raise InputError("free list: %s may never be freed" % f)
            fields.add(f)
        elif keys == {"mark"} and str(e["mark"]).strip():
            marks.add(str(e["mark"]))
        elif keys <= {"extra", "without"} and "extra" in keys and str(e["extra"]).strip():
            without = e.get("without", [])
            if not isinstance(without, list) or not all(
                    isinstance(f, str) and FIELD.match(f) for f in without):
                raise InputError("free list: `without` of %r is no list of fields" % (e,))
            extras.append((str(e["extra"]), list(without)))
        else:
            raise InputError("free list entry names neither one field, one mark nor one "
                             "extra route: %r" % (e,))
    return fields, marks, extras


def has_field(m, field):
    if field == "route":
        return "route" in m
    part, key = field.split(".", 1)
    return isinstance(m.get(part), dict) and key in m[part]


def freed_extra(m, extras):
    """Whether message `m` is one an `extra` entry lets the new tree add."""
    if not isinstance(m, dict):
        return False
    return any(m.get("route") == route and not any(has_field(m, f) for f in without)
               for route, without in extras)


def sub_ms(s):
    """The corpus test's normalisation (`sub_ms` there): the last three
    fraction digits of an ISO stamp `hh:mm:ss.ffffffZ` and of a sixteen-digit
    run starting with the corpus epoch `17724` become `uuu`."""
    b = list(s)
    n = len(s)

    def digit(i):
        return 0 <= i < n and s[i].isascii() and s[i].isdigit()

    i = 0
    while i < n:
        if i + 16 <= n and not digit(i - 1) and all(
                (s[i + k] == ":" if k in (2, 5) else s[i + k] == "." if k == 8
                 else s[i + k] == "Z" if k == 15 else digit(i + k)) for k in range(16)):
            b[i + 12:i + 15] = "uuu"
            i += 16
            continue
        if digit(i) and not digit(i - 1):
            j = i
            while digit(j):
                j += 1
            if j - i == 16 and s.startswith("17724", i):
                b[j - 3:j] = "uuu"
            i = j
            continue
        i += 1
    return "".join(b)


def masked(v):
    if isinstance(v, str):
        return sub_ms(v)
    if isinstance(v, list):
        return [masked(x) for x in v]
    if isinstance(v, dict):
        return {k: masked(x) for k, x in v.items()}
    return v


def canon(v):
    return json.dumps(masked(v), sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def strip_out(m, fields):
    """A message out of the hive without the parts the free list frees."""
    m = dict(m) if isinstance(m, dict) else {"": m}
    if "route" in fields:
        m.pop("route", None)
    for part in ("hop", "body"):
        if isinstance(m.get(part), dict):
            m[part] = {k: v for k, v in m[part].items() if "%s.%s" % (part, k) not in fields}
    return m


def strip_req(r, fields):
    if not isinstance(r, dict):
        return r
    return {k: v for k, v in r.items() if "summarizer.%s" % k not in fields}


def first_gap(a, b):
    """The first element of multiset `a` that `b` lacks and the first of `b`
    that `a` lacks, as canonical texts (None when there is none)."""
    rest = list(b)
    only_a = None
    for x in a:
        if x in rest:
            rest.remove(x)
        elif only_a is None:
            only_a = x
    return only_a, (rest[0] if rest else None)


def describe_out(old, new):
    """Which parts of two messages out differ, the locked body parts first."""
    o, n = json.loads(old), json.loads(new)
    parts = []
    ob, nb = o.get("body") or {}, n.get("body") or {}
    for k in LOCKED:
        if canon(ob.get(k)) != canon(nb.get(k)):
            parts.append("body.%s" % k)
    for k in sorted(set(ob) | set(nb)):
        if k not in LOCKED and canon(ob.get(k)) != canon(nb.get(k)):
            parts.append("body.%s" % k)
    if canon(o.get("route")) != canon(n.get("route")):
        parts.append("route")
    oh, nh = o.get("hop") or {}, n.get("hop") or {}
    for k in sorted(set(oh) | set(nh)):
        if canon(oh.get(k)) != canon(nh.get(k)):
            parts.append("hop.%s" % k)
    return parts


def short(text, width=240):
    return text if len(text) <= width else text[:width] + "..."


def step_diff(old, new, fields, marks, extras=()):
    """The first difference of one step, or None."""
    outs_o = [canon(strip_out(m, fields)) for m in old.get("out") or []]
    raw_n = list(new.get("out") or [])
    outs_n = [canon(strip_out(m, fields)) for m in raw_n]
    if extras:
        rest = list(outs_o)
        kept = []
        for m, c in zip(raw_n, outs_n):
            if c in rest:
                rest.remove(c)
                kept.append(c)
            elif not freed_extra(m, extras):
                kept.append(c)
        outs_n = kept
    a, b = first_gap(outs_o, outs_n)
    if a is not None or b is not None:
        if a is not None and b is not None:
            return "out differs in %s: old %s / new %s" % (
                ", ".join(describe_out(a, b)) or "?", short(a), short(b))
        if a is not None:
            return "out missing now: %s" % short(a)
        return "out new now: %s" % short(b)
    req_o = [canon(strip_req(r, fields)) for r in old.get("summarizer") or []]
    req_n = [canon(strip_req(r, fields)) for r in new.get("summarizer") or []]
    a, b = first_gap(req_o, req_n)
    if a is not None or b is not None:
        return "summarizer request differs: old %s / new %s" % (
            short(a or "-"), short(b or "-"))
    mk_o = [canon(m) for m in old.get("marks") or [] if m.get("kind") not in marks]
    mk_n = [canon(m) for m in new.get("marks") or [] if m.get("kind") not in marks]
    a, b = first_gap(mk_o, mk_n)
    if a is not None or b is not None:
        return "mark differs: old %s / new %s" % (short(a or "-"), short(b or "-"))
    return None


def compare_all(old, new, fields, marks, write=print, extras=()):
    """Compares every case; writes the lines and returns `(steps, differ)`."""
    steps = differ = 0
    for case in sorted(set(old) | set(new)):
        if case not in old:
            write("NOTE %s: a case only in the new tree (%d steps)"
                  % (case, len((new[case] or {}).get("steps") or [])))
            continue
        os_ = (old[case] or {}).get("steps") or []
        ns_ = (new.get(case) or {}).get("steps") or []
        if case not in new:
            write("DIFF %s: the case is gone (%d steps)" % (case, len(os_)))
        for i in range(max(len(os_), len(ns_))):
            steps += 1
            o = os_[i] if i < len(os_) else None
            n = ns_[i] if i < len(ns_) else None
            label = str((o or n).get("step") or i)
            if case not in new:
                differ += 1
                continue
            if o is None or n is None:
                differ += 1
                write("DIFF %s %s: the step is %s" % (
                    case, label, "new" if o is None else "gone"))
                continue
            if o.get("step") != n.get("step"):
                write("NOTE %s %s: the step now reads %r" % (case, label, n.get("step")))
            why = step_diff(o, n, fields, marks, extras)
            if why:
                differ += 1
                write("DIFF %s %s: %s" % (case, label, why))
    return steps, differ


def compare(root, base, free):
    fields, marks, extras = read_free(free)
    rev = base_of(root, base)
    old = old_fixtures(root, rev)
    if not old:
        raise InputError("no fixtures under %s at %s" % (FIXTURES, rev))
    new = new_fixtures(root)
    steps, differ = compare_all(old, new, fields, marks, extras=extras)
    print("CORPUS %d steps %d differ %s" % (steps, differ, "GREEN" if differ == 0 else "RED"))
    return 0 if differ == 0 else 1


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="verb", required=True)
    rec = sub.add_parser("record", help="record the corpus anew through the strand kit")
    rec.add_argument("--root", default=ROOT, help="the tree (default: this one)")
    cmp_ = sub.add_parser("compare", help="compare the old fixtures with the recorded ones")
    cmp_.add_argument("--root", default=ROOT, help="the tree (default: this one)")
    cmp_.add_argument("--base", default="", help="the old revision (default: merge-base "
                                                 "of master and HEAD)")
    cmp_.add_argument("--free", default="", help="the strand's free list (JSON)")
    args = parser.parse_args(argv)
    if args.verb == "record":
        return record(os.path.abspath(args.root))
    try:
        return compare(os.path.abspath(args.root), args.base, args.free)
    except InputError as e:
        sys.stderr.write("corpus_compare: %s\n" % e)
        return 2


if __name__ == "__main__":
    sys.exit(main())
