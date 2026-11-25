#!/usr/bin/env python3
"""The params doors of sealed hives (GH #1353).

A params-only update travels on the lane `in_params` with `hop.subscriber` =
the absolute path of the cell it is for. A sealed hive (`params.ports`
present) is crossed at its path, so every sealed hive in or under which a cell
with a params overlay lies needs, all in its own template and always together:

  - `accepts` `in_params` (no `at`: the lane opens the hive's path, never a
    cell), and `emits` for the answer lanes;
  - one edge `{"from": "."}` per overlay cell and per inner sealed hive that
    has a door, on `hop.route == 'in_params'` and the END of
    `hop.subscriber` -- a template does not know where it is mounted;
  - the way back of every such target: a cell's answer names itself in
    `hop.params_answer` and carries no route, so one edge per answer sets its
    lane (a constant -- a computed route would count as an exit for EVERY lane
    of the contract and blind `judge_exit`); an inner hive's answer carries the
    lane already and needs one edge. Both delete what the hive's other exits
    delete (`working_keys`).

An `accepts` row without an inner edge makes `judge_door` refuse the next
mutation of a colony that carries it, so both are written in one go. Nested
seals get a door each: an edge from the outer hive to a cell of the inner one
would be a `hive_port_boundary`.

The fourth thing it writes is a GUARD. All matching edges of a sender fire, so
an out-edge of a target (or a `.` edge of the hive) that names no lane would
hand the cell's answer to its neighbour as a tick, a row or a model output. An
edge that may match gets a conjunct that excludes the answer (at the hive: the
update too). Whether it may match is read from its condition over the keys an
answer carries (`answer_hop`); a context key counts as absent (the update
entered from outside, so it carries no round of this hive), what cannot be read
counts as may match.

    python3 scripts/hive_doors.py [--root <templates>] [--lib <templates>]... \
        [--check | --dry-run]

`--root` is the template tree that is written (this repository's by default);
`--lib` is a tree a `ref` may point into, read and never written (another
repository passes this one's `templates/`). `--check` writes nothing and exits
1 when the tree differs from what a run would write; `--dry-run` prints the
list and exits 0. Exit 2: the tree cannot be read (an unresolved `ref`, a
config that does not survive a round trip through the writer), nothing is
written. A run is idempotent. A frozen version (`<name>@<x.y.z>/`) is never
written; a seal without a door (frozen, excepted, or of a `--lib` that does
not declare the lane) gets no edge from outside. Standard library only.

A limit: an edge knows the END of a path, not where its hive is mounted. A hive
mounted under the name of one of its own inner seals (`steward` as `access`)
has two edges for one path end, and an update may enter the wrong one.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

LANE = "in_params"
# The answer of a cell (`ParamsPush` in crates/meclaw-cells/src/params_overlay.rs):
# this hop key names it, its value is also the lane it leaves a hive on. The
# one place to change them.
ANSWER_KEY = "params_answer"
APPLIED, REFUSED = "params_applied", "params_refused"
ANSWER_LANES = (APPLIED, REFUSED)

# The nine cell types with an `OverlayParams` impl (crates/meclaw-cells).
OVERLAY_TYPES = frozenset({
    "llm", "web", "timer", "voice", "harness", "mcp", "proxy", "subcolony", "store"})

# Template classes that stay without a door, with the reason.
EXCEPTIONS = {
    "voice2vision": "lives in a repository of its own without this kit; "
                    "its templates are not written here",
}

ACCEPTS_BECAUSE = (
    "a params-only update for one cell of this hive or of a sealed hive in it (GH #1353): "
    "`hop.subscriber` is the cell's absolute path, the body carries `params` and nothing else. "
    "The inner edges carry it to that cell by the end of the path; a cell of another path applies "
    "nothing. Answered by exactly one `params_applied` or `params_refused`. Draw it only from "
    "the operator's road")
EMITS_BECAUSE = {
    "params_applied": "the cell named by `hop.subscriber` of an `in_params` update merged it into "
                      "its live params (GH #1353)",
    "params_refused": "the cell named by `hop.subscriber` of an `in_params` update refused it "
                      "whole: `hop.refused_subscriber` is its path, `hop.error_code` the code "
                      "and the body the detail (GH #1353)",
}
NAME = re.compile(r"^[A-Za-z0-9_-]+$")


class TreeError(Exception):
    """The tree cannot be read or written; nothing was changed."""


# ---------------------------------------------------------------- conditions

def lane_test(lanes):
    terms = " || ".join("hop.route == '%s'" % lane for lane in lanes)
    return "has(hop.route) && (%s)" % terms


CARRIED = lane_test(ANSWER_LANES)
ANSWER_GUARD = "!has(hop.%s)" % ANSWER_KEY
LANE_GUARD = "!(has(hop.route) && hop.route == '%s')" % LANE
DOOR_PREFIX = "has(hop.route) && hop.route == '%s' && has(hop.subscriber) && " % LANE
DOOR = re.compile(
    "^" + re.escape(DOOR_PREFIX)
    + r"hop\.subscriber\.(endsWith|matches)\('[^']*'\)"
    + r"( && !hop\.subscriber\.endsWith\('[^']*'\))*$")


def is_door(edge):
    return (edge.get("from") == "." and "modifier" not in edge
            and bool(DOOR.match(edge.get("condition") or "")))


def names_lane(edge):
    """An edge that asks for the lane by name, in a form that is not the door's."""
    condition = edge.get("condition") or ""
    return "hop.route == '%s'" % LANE in condition and LANE_GUARD not in condition


def working_keys(edges):
    """The context keys more than half of the standing exits delete, in their order.

    A hive deletes the working values it promotes on its own exits
    (`docs/development-rules.md` section 8c, pinned per hive by gh823 and
    gh1316); the answer of a cell is one more exit. The majority and not the
    union: a key one exit of nine takes (`turn_id` of the session-keeper) is
    correlation of that road, and the asker of the update may need it.
    """
    exits = [e for e in edges if e.get("to") == "." and not is_way_back(e)]
    lists = [(e.get("modifier") or {}).get("delete_context") or [] for e in exits]
    count = {}
    for keys in lists:
        for key in dict.fromkeys(keys):
            count[key] = count.get(key, 0) + 1
    return [key for key, n in count.items() if 2 * n > len(exits)]


def way_back(sender, target, drop):
    tail = {"delete_context": drop} if drop else {}
    if target.kind == "hive":
        edge = {"from": sender, "to": ".", "condition": CARRIED}
        return [dict(edge, modifier=tail) if tail else edge]
    return [{"from": sender, "to": ".",
             "condition": "has(hop.%s) && hop.%s == '%s'" % (ANSWER_KEY, ANSWER_KEY, lane),
             "modifier": {"set_hop": {"route": "'%s'" % lane}, **tail}} for lane in ANSWER_LANES]


def is_way_back(edge):
    condition = edge.get("condition") or ""
    return edge.get("to") == "." and (
        condition == CARRIED
        or condition.startswith("has(hop.%s) && hop.%s == '" % (ANSWER_KEY, ANSWER_KEY)))


class UnknownError(Exception):
    """A condition outside the grammar the reader knows."""


UNKNOWN = object()   # may be anything
ERROR = object()     # the evaluation fails: the edge is skipped
_TOKEN = re.compile(r"""\s*(?:(?P<num>\d+(?:\.\d+)?)|(?P<str>'(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*")
                        |(?P<id>[A-Za-z_]\w*)|(?P<op>\|\||&&|==|!=|<=|>=|[-+*/%<>!?:().,\[\]]))""",
                    re.X)


def tokens(text):
    out, at = [], 0
    text = text.rstrip()
    while at < len(text):
        m = _TOKEN.match(text, at)
        if not m:
            raise UnknownError(text[at:])
        out.append((m.lastgroup, m.group(m.lastgroup)))
        at = m.end()
    out.append(("end", ""))
    return out


class Reader:
    """Evaluates the CEL subset of an edge condition over a known hop.

    A hop value may be UNKNOWN; a hop key outside it and every context key are
    absent (`has` is false, reading one fails the evaluation)."""

    def __init__(self, text, hop):
        self.toks, self.at = tokens(text), 0
        self.hop = hop
        self.top_alternative = False
        self.depth = 0

    def peek(self):
        return self.toks[self.at][1] if self.toks[self.at][0] in ("op", "id") else None

    def take(self, want=None):
        kind, text = self.toks[self.at]
        if want is not None and (kind not in ("op", "id") or text != want):
            raise UnknownError("expected %s, got %s" % (want, text))
        self.at += 1
        return kind, text

    def run(self):
        value = self.ternary()
        if self.toks[self.at][0] != "end":
            raise UnknownError(self.toks[self.at][1])
        return value

    def ternary(self):
        cond = self.either()
        if self.peek() != "?":
            return cond
        self.top_alternative = self.top_alternative or self.depth == 0
        self.take("?")
        yes = self.ternary()
        self.take(":")
        no = self.ternary()
        if cond is True:
            return yes
        if cond is False:
            return no
        return ERROR if cond is ERROR else UNKNOWN

    def either(self):
        value = self.both()
        while self.peek() == "||":
            self.top_alternative = self.top_alternative or self.depth == 0
            self.take("||")
            right = self.both()
            if value is True or right is True:
                value = True
            elif value is False and right is False:
                value = False
            elif value in (False, ERROR) and right in (False, ERROR):
                value = ERROR
            else:
                value = UNKNOWN
        return value

    def both(self):
        value = self.compare()
        while self.peek() == "&&":
            self.take("&&")
            right = self.compare()
            if value is False or right is False:
                value = False
            elif value is True and right is True:
                value = True
            elif value in (True, ERROR) and right in (True, ERROR):
                value = ERROR
            else:
                value = UNKNOWN
        return value

    def compare(self):
        left = self.add()
        op = self.peek()
        while op in ("==", "!=", "<", ">", "<=", ">=", "in"):
            self.take()
            right = self.add()
            left = self.relation(op, left, right)
            op = self.peek()
        return left

    @staticmethod
    def relation(op, left, right):
        if ERROR in (left, right):
            return ERROR
        if op == "in" and isinstance(right, list) and left is not UNKNOWN:
            if any(x is not UNKNOWN and x == left for x in right):
                return True
            return UNKNOWN if any(x is UNKNOWN for x in right) else False
        if left is UNKNOWN or right is UNKNOWN or op not in ("==", "!="):
            return UNKNOWN
        return (left == right) == (op == "==")

    def add(self):
        value = self.unary()
        while self.peek() in ("+", "-", "*", "/", "%"):
            self.take()
            right = self.unary()
            value = ERROR if ERROR in (value, right) else UNKNOWN
        return value

    def unary(self):
        if self.peek() == "!":
            self.take()
            value = self.unary()
            return (not value) if isinstance(value, bool) else value
        if self.peek() == "-":
            self.take()
            self.unary()
            return UNKNOWN
        return self.postfix(self.primary())

    def args(self):
        self.take("(")
        self.depth += 1
        out = []
        while self.peek() != ")":
            out.append(self.ternary())
            if self.peek() == ",":
                self.take(",")
        self.depth -= 1
        self.take(")")
        return out

    def postfix(self, value):
        while True:
            if self.peek() == ".":
                self.take(".")
                kind, name = self.take()
                if kind != "id":
                    raise UnknownError(name)
                if self.peek() == "(":
                    value = self.method(value, name, self.args())
                else:
                    value = ERROR if value is ERROR else UNKNOWN
            elif self.peek() == "[":
                self.take("[")
                self.depth += 1
                self.ternary()
                self.depth -= 1
                self.take("]")
                value = ERROR if value is ERROR else UNKNOWN
            else:
                return value

    @staticmethod
    def method(value, name, args):
        if value is ERROR or ERROR in args:
            return ERROR
        if isinstance(value, str) and len(args) == 1 and isinstance(args[0], str):
            if name == "startsWith":
                return value.startswith(args[0])
            if name == "endsWith":
                return value.endswith(args[0])
            if name == "contains":
                return args[0] in value
        return UNKNOWN

    def primary(self):
        kind, text = self.take()
        if kind == "num":
            return float(text) if "." in text else int(text)
        if kind == "str":
            return UNKNOWN if "\\" in text else text[1:-1]
        if kind == "op" and text == "(":
            self.depth += 1
            value = self.ternary()
            self.depth -= 1
            self.take(")")
            return value
        if kind == "op" and text == "[":
            out = []
            self.depth += 1
            while self.peek() != "]":
                out.append(self.ternary())
                if self.peek() == ",":
                    self.take(",")
            self.depth -= 1
            self.take("]")
            return out
        if kind != "id":
            raise UnknownError(text)
        if text in ("true", "false"):
            return text == "true"
        if text == "has":
            return self.has()
        if text in ("hop", "context", "envelope"):
            if self.peek() == ".":
                self.take(".")
                key = self.take()
                if key[0] != "id":
                    raise UnknownError(key[1])
                if self.peek() == "(":
                    raise UnknownError(key[1])
                return self.member(text, key[1])
            return UNKNOWN
        if self.peek() == "(":
            args = self.args()
            return ERROR if ERROR in args else UNKNOWN
        raise UnknownError(text)

    def member(self, space, key):
        if space == "envelope":
            return UNKNOWN
        return self.hop[key] if space == "hop" and key in self.hop else ERROR

    def has(self):
        self.take("(")
        _, space = self.take()
        self.take(".")
        _, key = self.take()
        deeper = False
        while self.peek() == ".":
            self.take(".")
            self.take()
            deeper = True
        self.take(")")
        if space not in ("hop", "context"):
            return UNKNOWN
        if space == "hop" and key in self.hop:
            return UNKNOWN if deeper else True
        return False


def may_match(condition, hop):
    if condition is None:
        return True
    try:
        value = Reader(condition, hop).run()
    except (UnknownError, IndexError):
        return True
    return value is True or value is UNKNOWN


def answer_hop(lane, carried):
    """The hop of an answer; `carried` = after the first way back set its route."""
    hop = {ANSWER_KEY: lane, "operation": "params_update"}
    if lane == APPLIED:
        hop["applied_subscriber"] = UNKNOWN
    else:
        hop.update({"refused_subscriber": UNKNOWN, "finish_reason": "error",
                    "error_code": UNKNOWN})
    if carried:
        hop["route"] = lane
    return hop


def open_to(edge, sender):
    """The guards a standing edge lacks; `sender`: "cell", "seal" or "hive"."""
    if edge.get("default") and sender != "hive":
        return []  # the way back matched, so a default does not fire
    condition = edge.get("condition")
    out = []
    if sender == "hive" and may_match(condition, {"route": LANE, "subscriber": UNKNOWN}):
        out.append(LANE_GUARD)
    if any(may_match(condition, answer_hop(lane, sender != "cell")) for lane in ANSWER_LANES):
        out.append(ANSWER_GUARD)
    return out


def guarded(edge, guards):
    condition, guard = edge.get("condition"), " && ".join(guards)
    if condition is None:
        text = guard
    else:
        try:
            reader = Reader(condition, {})
            reader.run()
            wrap = reader.top_alternative
        except (UnknownError, IndexError):
            wrap = False
        text = "%s && %s" % ("(%s)" % condition if wrap else condition, guard)
    out = {}
    for key, value in edge.items():
        if key != "condition":
            out[key] = value
        if key == "to" or key == "condition":
            out["condition"] = text
    out.setdefault("condition", text)
    return out


# ---------------------------------------------------------------------- tree

class Node:
    """One directory of a template tree, its `ref` resolved."""

    def __init__(self, name, kind, home, config, own, frozen, children):
        self.name, self.kind, self.home, self.config = name, kind, home, config
        self.own, self.frozen, self.children = own, frozen, children

    @property
    def sealed(self):
        return self.kind == "hive" and "ports" in (self.config.get("params") or {})

    @property
    def excepted(self):
        return os.path.basename(self.home).split("@")[0] in EXCEPTIONS


class Tree:
    def __init__(self, root, libs):
        self.root = os.path.abspath(root)
        self.roots = [self.root] + [os.path.abspath(lib) for lib in libs]
        self.missing = set()
        self.configs = {}

    def config(self, home):
        if home not in self.configs:
            path = os.path.join(home, "config.json")
            try:
                with open(path, encoding="utf-8") as fh:
                    self.configs[home] = json.load(fh)
            except (OSError, ValueError) as err:
                raise TreeError("%s: %s" % (path, err))
        return self.configs[home]

    def provided(self, template):
        name = template.split("@")[0]
        for wanted in (template, name):
            for root in self.roots:
                home = os.path.join(root, wanted)
                if os.path.isfile(os.path.join(home, "config.json")):
                    return home
        return None

    def node(self, home, name=None, seen=()):
        config = self.config(home)
        block = config.get("cell") or {}
        name = name or os.path.basename(home)
        if block.get("type") == "ref":
            template = block.get("template") or ""
            target = self.provided(template)
            if target is None:
                self.missing.add("%s (%s)" % (template, home))
                return Node(name, "missing", home, {}, False, False, [])
            if target in seen:
                raise TreeError("%s: the references go in a circle" % home)
            return self.node(target, name, tuple(seen) + (target,))
        rel = os.path.relpath(home, self.root)
        own = not rel.startswith(os.pardir)
        children = []
        for entry in sorted(os.listdir(home)):
            child = os.path.join(home, entry)
            if os.path.isfile(os.path.join(child, "config.json")):
                children.append(self.node(child, entry, seen))
        frozen = own and "@" in rel.split(os.sep)[0]
        return Node(name, block.get("type"), home, config, own, frozen, children)

    def hives(self):
        """Every hive whose config stands under the root, references resolved."""
        for base, dirs, files in os.walk(self.root):
            dirs[:] = sorted(d for d in dirs if not d.startswith("."))
            if "config.json" in files:
                config = self.config(base)
                if isinstance(config, dict) and (config.get("cell") or {}).get("type") == "hive":
                    yield self.node(base)


def has_door(node):
    """Whether a sealed hive carries (or gets) the lane."""
    if node.excepted:
        return False
    if node.own and not node.frozen:
        return bool(targets(node))
    contract = (node.config.get("params") or {}).get("contract") or {}
    return any(row.get("route") == LANE for row in contract.get("accepts") or [])


def targets(node, prefix=""):
    """[(relative path, node)] of the cells and inner seals the door of `node` serves."""
    out = []
    for child in node.children:
        rel = prefix + child.name
        if child.kind in OVERLAY_TYPES:
            out.append((rel, child))
        elif child.kind == "hive" and child.sealed:
            if has_door(child):
                out.append((rel, child))
        elif child.kind == "hive":
            out.extend(targets(child, rel + "/"))
    return out


def cells(node, prefix=""):
    """Relative paths of every overlay cell the door of `node` reaches, seals included."""
    out = []
    for rel, target in targets(node):
        if target.kind == "hive":
            out.extend(cells(target, prefix + rel + "/"))
        else:
            out.append(prefix + rel)
    return out


def door_condition(rel, target, reached):
    for part in rel.split("/"):
        if not NAME.match(part):
            raise TreeError("%s: a name outside [A-Za-z0-9_-]" % rel)
    if target.kind != "hive":
        text = "hop.subscriber.endsWith('/%s')" % rel
        for other in sorted(reached):
            if other != rel and other.endswith("/" + rel):
                text += " && !hop.subscriber.endsWith('/%s')" % other
        return DOOR_PREFIX + text
    between = sorted({c.rpartition("/")[0] for c in cells(target)})
    deeper = "|".join(b + "/" for b in between if b)
    part = "(%s)%s" % (deeper, "?" if "" in between else "") if deeper else ""
    return DOOR_PREFIX + "hop.subscriber.matches('/%s/%s[^/]+$')" % (rel, part)


# ---------------------------------------------------------------------- plan

class Plan:
    def __init__(self, node):
        self.node = node
        self.cells = self.seals = self.backs = 0
        self.guards = []      # edges as they stand
        self.config = None    # the new config, None = unchanged

    @property
    def changed(self):
        return self.config is not None


def place(edges, mine, wanted):
    """`edges` with the managed ones replaced by `wanted`, in place when equal."""
    if [e for e in edges if mine(e)] == wanted:
        return list(edges)
    at = next((i for i, e in enumerate(edges) if mine(e)), len(edges))
    rest = [e for e in edges if not mine(e)]
    return rest[:at] + wanted + rest[at:]


def accepts_row(rows, wanted, foreign_door):
    out = []
    for row in rows:
        if row.get("route") != LANE:
            out.append(row)
        elif wanted:
            out.append({k: v for k, v in row.items() if k != "at"})
        elif foreign_door:
            out.append(row)
    if wanted and not any(r.get("route") == LANE for r in out):
        at = next((i + 1 for i, r in enumerate(out) if r.get("route") == "in_model"), len(out))
        out.insert(at, {"route": LANE, "because": ACCEPTS_BECAUSE})
    return out


def emits_rows(rows, wanted, had_door):
    out = list(rows)
    if wanted:
        for lane in ANSWER_LANES:
            if not any(r.get("route") == lane for r in out):
                out.append({"route": lane, "because": EMITS_BECAUSE.get(lane, "GH #1353")})
    elif had_door:
        out = [r for r in out if r.get("route") not in ANSWER_LANES]
    return out


def plan_hive(node):
    plan = Plan(node)
    found = [] if node.excepted or node.frozen else targets(node)
    config = json.loads(json.dumps(node.config))
    params = config.get("params") or {}
    edges = (params.get("graph") or {}).get("edges") or []
    contract = params.get("contract") or {}
    managed = any(is_door(e) or is_way_back(e) for e in edges)
    stands = any(r.get("route") == LANE for r in contract.get("accepts") or [])
    if not found and not managed and not stands:
        return plan
    reached = cells(node)
    doors = [{"from": ".", "to": "./" + rel, "condition": door_condition(rel, t, reached)}
             for rel, t in found]
    drop = working_keys(edges)
    backs = [edge for rel, t in found for edge in way_back("./" + rel, t, drop)]
    plan.seals = sum(t.kind == "hive" for _, t in found)
    plan.cells, plan.backs = len(found) - plan.seals, len(backs)

    new = place(edges, is_door, doors)
    new = place(new, is_way_back, backs)
    senders = {"./" + rel: "seal" if t.kind == "hive" else "cell" for rel, t in found}
    senders["."] = "hive"
    for i, edge in enumerate(new):
        sender = senders.get(edge.get("from"))
        if not found or not sender or is_door(edge) or is_way_back(edge):
            continue
        guards = open_to(edge, sender)
        if sender == "hive" and names_lane(edge):
            guards = [g for g in guards if g != LANE_GUARD]  # somebody's own edge of the lane
        if guards:
            plan.guards.append(edge)
            new[i] = guarded(edge, guards)

    foreign = any(e.get("from") == "." and not is_door(e) and names_lane(e) for e in new)
    accepts = accepts_row(contract.get("accepts") or [], bool(found), foreign)
    emits = emits_rows(contract.get("emits") or [], bool(found), managed or stands and not foreign)
    if found or "accepts" in contract:
        contract["accepts"] = accepts
    if found or "emits" in contract:
        contract["emits"] = emits
    params["contract"] = contract
    params.setdefault("graph", {})["edges"] = new
    config["params"] = params
    if config != node.config:
        plan.config = config
    return plan


def render(config, like):
    """The text of `config` in the form of the file it came from, None = unknown form."""
    old = json.loads(like)
    for ascii_only in (False, True):
        for end in ("\n", ""):
            def dump(doc):
                return json.dumps(doc, indent=2, ensure_ascii=ascii_only) + end
            if dump(old) == like:
                return dump(config)
    return None


def build(tree):
    plans = [plan_hive(node) for node in tree.hives() if node.sealed]
    if tree.missing:
        raise TreeError("no template root provides: %s" % ", ".join(sorted(tree.missing)))
    texts = {}
    for plan in plans:
        if plan.changed:
            path = os.path.join(plan.node.home, "config.json")
            with open(path, encoding="utf-8") as fh:
                text = render(plan.config, fh.read())
            if text is None:
                raise TreeError("%s: not in the writer's form (json.dumps indent=2)"
                                % os.path.relpath(path, tree.root))
            texts[path] = text
    return plans, texts


def show(plan, tree, out):
    rel = os.path.relpath(plan.node.home, tree.root)
    if plan.node.excepted:
        out.write("%s: no door: %s\n" % (rel, EXCEPTIONS[
            os.path.basename(plan.node.home).split("@")[0]]))
        return
    if not (plan.cells or plan.seals or plan.changed):
        return
    out.write("%s: %d cell(s), %d seal(s), %d way(s) back, %d guard(s)%s\n"
              % (rel, plan.cells, plan.seals, plan.backs, len(plan.guards),
                 "" if plan.changed else " -- stands"))


def main(argv=None):
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    ap = argparse.ArgumentParser(description="the params doors of sealed hives (GH #1353)")
    ap.add_argument("--root", default=os.path.join(here, "templates"),
                    help="the template tree that is written")
    ap.add_argument("--lib", action="append", default=[],
                    help="a template tree a ref may point into; read only")
    mode = ap.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="write nothing, exit 1 on a difference")
    mode.add_argument("--dry-run", action="store_true", help="write nothing, print the list")
    args = ap.parse_args(argv)
    for path in [args.root] + args.lib:
        if not os.path.isdir(path):
            sys.stderr.write("hive_doors: no such directory: %s\n" % path)
            return 2
    tree = Tree(args.root, args.lib)
    try:
        plans, texts = build(tree)
    except TreeError as err:
        sys.stderr.write("hive_doors: %s\n" % err)
        return 2
    for plan in plans:
        show(plan, tree, sys.stdout)
    if args.check:
        for path in texts:
            sys.stdout.write("hive_doors: %s differs -- run scripts/hive_doors.py --root %s\n"
                             % (os.path.relpath(path, tree.root), args.root))
        sys.stdout.write("hive_doors: %s\n" % ("RED" if texts else "ok"))
        return 1 if texts else 0
    if not args.dry_run:
        for path, text in texts.items():
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(text)
        sys.stdout.write("hive_doors: %d file(s) written\n" % len(texts))
    return 0


if __name__ == "__main__":
    sys.exit(main())
