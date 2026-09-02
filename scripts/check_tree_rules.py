#!/usr/bin/env python3
"""Gate for the naming and wiring rules of the template tree.

Binding contract: `docs/development-rules.md` § 8a, filed as
[GH #550](https://github.com/mmeyerlein/meclaw/issues/550) out of the
2026-08-28 sweep of all template roots and every example
([GH #551](https://github.com/mmeyerlein/meclaw/issues/551)).

Three rules of the tree were enforced by reading it, which means they were
enforced when somebody remembered to look. Each of them had a live violation
when this gate was written, and every one of those violations entered the tree
past a green gate. A rule nobody can run is a habit, not a rule.

R5 joined them on 2026-08-31 with the v-lanes wave
([GH #559](https://github.com/mmeyerlein/meclaw/issues/559)) -- for the opposite
reason: it guards a form that does not exist in the tree yet, so that the first
one written is written right.

THE RULES
=========

R1 -- a ref is named after its template
---------------------------------------
Every `config.json` with `cell.type: "ref"` **inside a template** sits in a
directory whose name equals the referenced template's name (the part before
any `@`). The directory name of a ref is part of a published address space:
it is what an `override_params` path and a mutation address, so a ref that
carries a role name instead makes every reader learn a translation before they
can follow an edge.

Where the rule stops (the boundary ruled in #551):

    A `ref` marker inside a template is named after the template it
    references. An instance grown by a manifest is named by whoever grows it.

So `examples/` is INFORMATIONAL here and never a failure: an example that grows
an `org` called `acme` with a `member` called `alex` is teaching exactly that,
and `member/member/assistant` would teach nothing. The same boundary covers the
`ref` markers inside an example's own seed -- they are refs in the strict sense
but they stand in an instance, not in a template.

R3 -- no unwired cell in a shipped template
-------------------------------------------
Every cell directory of a template appears as `from` or `to` of at least one
edge in some `params.graph.edges[]` of that template. Edges live only there
(`docs/config.md` § `params.graph`), and `params.ports` is `[]` in every hive
that declares one, so there is no port-only reachability case to allow for.

A shipped template is a worked example. An occupant nobody can reach is not a
worked example; it is a placeholder with documentation attached.

An occupant that is deliberately unreachable **declares itself**, with
`"unwired": true` at the top level of its own `config.json` and the reason in
the `description.purpose` a reader will find anyway. The declaration is
deliberately in the file and NOT a path list in this script: a declaration is
visible in the diff that adds it, a checker exemption is visible to nobody --
the same argument `contract.transfer: "none"` was built on (`docs/cell-types.md`
§ *Content transfer*). The substrate ignores the key (only `cell.*` is a closed
key set, `docs/config.md` § Block definition), so it costs the boot nothing.

R4 -- one lane, one cell name
------------------------------
`LANE_ANSWERER` below is a small table: lane -> the name of the cell that
answers it. For every hive that declares such a lane in its contract, the cell
on the answering end of the `. -> ./X` edge must be named `X`.

The table is deliberately small and **grows by ruling, not by inference**. A
checker that guessed which cells "do the same thing" would be wrong more often
than the tree is. The families the sweep found but nobody has ruled on live in
`ROLE_FAMILIES` and are reported as WARNINGS -- they name a real inconsistency
and they are not a reason to stop a commit.

R5 -- a v-lane is a declared deep edge
--------------------------------------
A v-lane is an edge whose endpoint is more than one segment deep: it is drawn in
the graph of the lowest common ancestor and it lands on a rim *inside* that
ancestor's subtree, skipping the pass-through chain that used to carry the lane
level by level (ADR-0020, ruled 2026-08-31). The substrate has always
been able to deliver such an edge -- routing is a flat lookup -- so the only
thing that keeps a deep edge honest is a declaration, and the only place the
declaration can be READ is the tree. Hence this rule.

Two halves, both from the same ruling (R-V1):

    A deep edge names its lane. The validation cannot read a lane out of a CEL
    guard, so the edge says `"lane": "in_recall"` and Stage 6 checks the
    connect point and the mandatory hops against it.

    The target declares where the lane docks. A level's contract entry carries
    `"at": ["./talky", "./cogny"]` -- relative paths, from the declaring node
    down to the endpoint the lane may land on. `ports: []` stays literally true:
    a v-lane is the one opening the template itself pronounces.

An `at` entry is ALWAYS of the form `./…` and always strictly below the
declaring hive. Stage 6 compares it against the endpoint's path relative to the
level (`relative_to`, which yields `./…` and never `.`), so `"."` and a bare
`talky` match nothing at all -- an entry in either spelling is a finding here,
because the tree would otherwise carry a declaration that reads like a
permission and grants none. A lane meant to end on a hive's own RIM is declared
one level UP, as `./<hive>` in the parent's contract.

Walking one deep endpoint, level by level from the ancestor down (the rule table
of the wave brief, which is also the table Stage 6 runs -- this gate is its
static half):

    level                       contract says            verdict
    ------------------------------------------------------------------------
    unsealed                    nothing                  transparent, skipped
    unsealed                    the lane, no matching     `v_lane_mandatory_hop`
                                `at`                      -- the skip is refused
    sealed (`params.ports`)     `at` holds the relative   this level is open
                                path to the endpoint
    sealed                      nothing, or `at` without  the existing
                                a hit                     `hive_port_boundary`
    the endpoint's PARENT       no `at` naming the        `v_lane_no_connect_point`
                                endpoint

The walk never ends early on a hit: an `at` legitimates the level that declares
it and nothing else, so a sealed level BELOW a vouching one keeps its own seal
(*an ancestor may not open a subtree from outside* -- the same sentence the
no-connect-point finding says out loud). The endpoint itself is not a level the
lane traverses INTO: a v-lane lands on a rim, and a rim is an address, so seal
and mandatory-hop are asked only of the levels strictly above it.

The connect point is owed by ONE level: the endpoint's PARENT, the TARGET hive
of Stage 6. A vouching ancestor waives its own hop and its own seal and nothing
more -- it may not supply the connect point for a level further down, and the
endpoint may not supply it for itself (there is no `at` spelling that would say
so). The static half asks the same one level, so that a tree this gate passes is
a tree Stage 6 passes.

The gate is the STATIC half and says so: it resolves a `ref` level into
`templates/<name>/` (a tree read, not a registry read) and stops with a note
where the referenced template is not in this tree. Stage 6 in the colony is the
arbiter; what this rule buys is that a bad v-lane is caught in the diff that
writes it rather than in the mutation that submits it. A finding is a FAILURE,
not a warning: unlike R4's role families there is no open question here -- the
form was ruled before the first v-lane was drawn.

USAGE
=====
    python3 scripts/check_tree_rules.py [--check] [--root DIR] [--selftest]

`--check` is accepted for symmetry with `check_roadmap_anchors.py` /
`check_adr_anchors.py` / `check_claims.py` and changes nothing; there is no
--write mode, because this gate derives no travelling copy. It reads
`templates/` and `examples/` only, so it runs unchanged in the private tree and
in the published one -- unlike `claims.tsv` and the corridor fixtures there is
no reference file to keep in two places. No network, no cargo.

`--root` points the gate at another tree; `--selftest` builds a fixture tree
with one violation of each rule and asserts each one is caught (that is this
gate's own pin -- see `docs/development-rules.md` § 2).

Exit 0 = every rule held, or the finding stands in the dated transition list
below. Exit 1 = a finding.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# --- R4, the ruled half ------------------------------------------------------
# lane -> the name of the cell that answers it. Grows by ruling only.
# `in_schemas` -> `schemas`: GH #548. `schemas` names what comes back, it
# matches the lane, and it is the spelling of the hive that has the most of
# them.
LANE_ANSWERER = {
    "in_schemas": "schemas",
}

# --- R4, the unruled half: WARN only ----------------------------------------
# The families the 2026-08-28 sweep found (GH #551 § 2). None of them is wrong
# on its own; together they are the reason a reader cannot guess a cell's name
# from its job. They are WARNINGS and not failures on purpose: nobody has ruled
# on them, and a gate that fails on an open question gets switched off.
# A family leaves this table when it is ruled -- either into LANE_ANSWERER (if
# a lane names it) or out of the tree (if the names get unified).
ROLE_FAMILIES = (
    # (role, the name the sweep proposed as the survivor, the other spellings
    #  seen in the tree, issue)
    ("the timer", "clock", ("cron", "refresh", "night", "menu-clock"), 551),
    ("the fan-out", "dispatcher", ("dispatch",), 551),
    ("the error drain", "drain", ("errors",), 551),
    ("the outbound bridge", "proxy", ("notifier",), 551),
    ("the pipeline's own store", "state", ("memory",), 551),
)

# --- The dated transition list ----------------------------------------------
# ADDED 2026-08-30, and every row leaves again with the commit that lands its
# issue. These are the violations the sweep measured; three strands are fixing
# them right now, and this gate had to be able to go green against the tree as
# it stands or it could not be committed at all. A row that no longer matches
# anything is reported as a stale row to delete -- the gate says which line.
#
#   rule, path relative to the repo root      issue  what lands
TRANSITIONAL = {}
TRANSITIONAL_ADDED = "2026-08-30"

CONTROL_PLANE = "/colony/"


class Finding:
    def __init__(self, rule: str, path: str, text: str) -> None:
        self.rule = rule
        self.path = path
        self.text = text

    def line(self) -> str:
        return f"{self.rule} {self.path}: {self.text}"


def load(path: Path) -> dict:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        raise ValueError(f"{path}: unreadable ({exc})") from exc
    return data if isinstance(data, dict) else {}


def node_dirs(root: Path) -> list[Path]:
    """Every directory under (and including) `root` that holds a config.json."""
    return sorted({p.parent for p in root.rglob("config.json")})


def edge_endpoints(cfg_dir: Path, cfg: dict) -> tuple[set[Path], list[dict]]:
    """Resolve `.` / `./X` endpoints of one graph against the holding directory.

    Absolute endpoints (`/colony/graph` and friends) are the control plane and
    are not cells of this template; they resolve to nothing on purpose.
    """
    edges = ((cfg.get("params") or {}).get("graph") or {}).get("edges") or []
    hit: set[Path] = set()
    for edge in edges:
        for key in ("from", "to"):
            target = edge.get(key)
            if not isinstance(target, str) or target.startswith(CONTROL_PLANE):
                continue
            if target == ".":
                hit.add(cfg_dir)
            elif target.startswith("./"):
                hit.add(cfg_dir.joinpath(target[2:]))
    return hit, edges


def accepted_routes(cfg: dict) -> set[str]:
    """The routes a node's contract accepts, wherever the block sits.

    A hive carries its contract inside `params.contract`; a cell carries it at
    the top level. Both spellings are read, because R4 asks the same question
    of both.
    """
    routes: set[str] = set()
    for block in ((cfg.get("params") or {}).get("contract"), cfg.get("contract")):
        if not isinstance(block, dict):
            continue
        for entry in block.get("accepts") or []:
            if isinstance(entry, dict) and isinstance(entry.get("route"), str):
                routes.add(entry["route"])
    return routes


# --- R5, the v-lane walk -----------------------------------------------------


def contract_entries(cfg: dict) -> list[dict]:
    """Every `accepts` and `emits` entry of a node's contract, both spellings.

    A hive carries the block inside `params.contract`, a cell at the top level;
    `accepts` and `emits` are read together because the rule table asks the same
    question of both ends of a v-lane (a lane is declared where it is influenced,
    and which side of the arrow that is depends on the direction of the edge).
    """
    entries: list[dict] = []
    for block in ((cfg.get("params") or {}).get("contract"), cfg.get("contract")):
        if not isinstance(block, dict):
            continue
        for key in ("accepts", "emits"):
            for entry in block.get(key) or []:
                if isinstance(entry, dict):
                    entries.append(entry)
    return entries


def norm_rel(path: str) -> str | None:
    """The connect point an `at` entry names, or None if it names none.

    `./talky` and `./talky/` are the same point. Everything else is NOT a
    connect point and is not silently repaired into one: Stage 6 compares `at`
    against the endpoint's path relative to the level, which is always `./…`, so
    a bare `talky`, a `"."` and an absolute path can never match. Normalising
    them away would hide exactly the declaration that reads like a permission
    and grants none -- the caller turns a None into an R5 finding instead.
    """
    stripped = path.strip().rstrip("/")
    if not stripped.startswith("./") or len(stripped) < 3:
        return None
    return stripped


def lane_declaration(cfg: dict, lane: str) -> tuple[bool, set[str]]:
    """Does this node's contract speak about `lane`, and where does it dock?

    Only well-formed `at` entries become docks; a malformed one is reported once
    per node by `malformed_connect_points`, independently of whether any edge
    happens to walk through this level.
    """
    declared = False
    docks: set[str] = set()
    for entry in contract_entries(cfg):
        if entry.get("route") != lane:
            continue
        declared = True
        for at in entry.get("at") or []:
            if isinstance(at, str) and (rel := norm_rel(at)) is not None:
                docks.add(rel)
    return declared, docks


def malformed_connect_points(cfg: dict) -> list[tuple[str, str]]:
    """Every `at` entry of this node that names no connect point at all.

    Returns `(route, raw entry)` pairs. This is the half of R5 that needs no
    edge: a contract may carry `"at": ["."]` for a lane nobody draws yet, and it
    is just as wrong there -- `"."` is not the rim in this field, it is nothing.
    """
    bad: list[tuple[str, str]] = []
    for entry in contract_entries(cfg):
        route = entry.get("route")
        for at in entry.get("at") or []:
            if not isinstance(at, str) or norm_rel(at) is None:
                bad.append((str(route), repr(at)))
    return bad


def is_sealed(cfg: dict) -> bool:
    """A hive is sealed once it declares `params.ports` at all -- `[]` included."""
    params = cfg.get("params")
    return isinstance(params, dict) and "ports" in params


def follow_ref(node: Path, templates: Path) -> Path | None:
    """The directory that actually carries a level's contract and occupants.

    A `ref` marker holds overrides, not a contract: the contract and the
    children live in the template it references. Following it into
    `templates/<name>/` is a read of this tree, not of the registry -- the
    version behind the `@` is the registry's business and is dropped here, which
    is the approximation the docstring advertises.
    """
    seen: set[Path] = set()
    while True:
        cfg_path = node / "config.json"
        if not cfg_path.is_file():
            return None
        cell = load(cfg_path).get("cell") or {}
        if cell.get("type") != "ref":
            return node
        referenced = str(cell.get("template") or "").split("@")[0]
        target = templates / referenced
        if not referenced or target in seen or not (target / "config.json").is_file():
            return None
        seen.add(target)
        node = target


def v_lane_walk(
    cfg_dir: Path, lane: str, target: str, root: Path, repo_root: Path
) -> tuple[list[Finding], list[str]]:
    """Run the R5 rule table down one deep endpoint. Returns findings and notes."""
    templates = root.parent
    segments = target[2:].split("/")
    findings: list[Finding] = []
    here = cfg_dir
    for i, segment in enumerate(segments):
        node = follow_ref(here / segment, templates)
        if node is None:
            walked = "/".join(segments[: i + 1])
            return findings, [
                f"R5 (note) `{root.name}` cannot follow `{target}` past `./{walked}` "
                f"inside this tree -- the level refs a template that is not here, so "
                f"the connect point of lane `{lane}` is left to Stage 6."
            ]
        here = node
        rest = segments[i + 1:]
        if not rest:
            # The endpoint itself is not a level the lane traverses INTO: its
            # rim is the address, and Stage 6 judges the levels from its PARENT
            # upwards. It is walked only to prove the path exists in this tree.
            break
        cfg = load(node / "config.json")
        # The path from THIS level down to the endpoint -- the exact string
        # Stage 6 builds with `relative_to(level, endpoint)`, which is why it is
        # always `./…` and never `.`.
        wanted = "./" + "/".join(rest)
        # This level is the TARGET hive: the endpoint's parent, and the only
        # level that can owe a connect point.
        is_target = len(rest) == 1
        declared, docks = lane_declaration(cfg, lane)
        rel_node = node.relative_to(repo_root).as_posix()

        # Row 3. An `at` hit legitimates THIS level and nothing else. The walk
        # carries on to the endpoint: an ancestor may not open a subtree from
        # outside, so a sealed level below a vouching one still keeps its seal.
        if wanted in docks:
            continue
        if is_target:
            findings.append(Finding(
                "R5", rel_node,
                f"`{root.name}` draws the v-lane `{lane}` onto `{target}`, and the "
                f"hive this endpoint sits in declares no `at` reaching `{wanted}` -- "
                f"`v_lane_no_connect_point`. The connect point is owed by the "
                f"endpoint's PARENT and by nobody else: put `\"at\": [\"{wanted}\"]` "
                f"on this level's `{lane}` entry. An ancestor that vouched further "
                f"up does not pronounce it here, the endpoint cannot pronounce it "
                f"for itself, and `\".\"` is not a spelling that says so.",
            ))
            continue
        if is_sealed(cfg):
            findings.append(Finding(
                "R5", rel_node,
                f"`{root.name}` draws the v-lane `{lane}` onto `{target}`, but "
                f"this sealed level declares no `at` reaching `{wanted}`. A "
                f"sealed rim refuses it as `hive_port_boundary` -- the one "
                f"opening a seal accepts is the `at` the template itself "
                f"pronounces, and an ancestor that already vouched does not "
                f"pronounce it for this level.",
            ))
            return findings, []
        if declared:
            findings.append(Finding(
                "R5", rel_node,
                f"`{root.name}` draws the v-lane `{lane}` onto `{target}` past "
                f"this level, which declares `{lane}` in its own contract "
                f"without an `at` reaching `{wanted}` -- `v_lane_mandatory_hop`. "
                f"A level that takes influence on a lane may not be skipped: "
                f"give it the `at`, or drop the pass-through declaration in the "
                f"same commit that migrates the lane.",
            ))

    return findings, []


def check_template(root: Path, repo_root: Path) -> tuple[list[Finding], list[str]]:
    """R1, R3, R4 and R5 against one template root."""
    findings: list[Finding] = []
    notes: list[str] = []
    dirs = node_dirs(root)
    configs = {d: load(d / "config.json") for d in dirs}

    wired: set[Path] = set()
    for d, cfg in configs.items():
        hit, _ = edge_endpoints(d, cfg)
        wired |= hit

    for d, cfg in configs.items():
        rel = d.relative_to(repo_root).as_posix()
        cell = cfg.get("cell") or {}

        # --- R1 -------------------------------------------------------------
        # The template root is skipped: its own name is the template's name and
        # `templates/README.md` is the gate for that.
        if d != root and cell.get("type") == "ref":
            referenced = str(cell.get("template") or "").split("@")[0]
            if referenced and referenced != d.name:
                findings.append(Finding(
                    "R1", rel,
                    f"a ref onto `{referenced}` is named `{d.name}`. A ref "
                    f"inside a template is named after the template it "
                    f"references -- the directory name is the address an "
                    f"override_params path and a mutation use.",
                ))

        # --- R3 -------------------------------------------------------------
        if d != root and d not in wired and cfg.get("unwired") is not True:
            findings.append(Finding(
                "R3", rel,
                f"no edge in `{root.name}` names `./{d.name}` -- the cell is "
                f"reachable from nothing. An occupant that is deliberately "
                f"unreachable declares `\"unwired\": true` in its own "
                f"config.json, with the reason in description.purpose.",
            ))

        # --- R4 -------------------------------------------------------------
        _, edges = edge_endpoints(d, cfg)
        declared = accepted_routes(cfg)
        for lane, expected in LANE_ANSWERER.items():
            if lane not in declared:
                continue
            wanted = re.compile(r"'" + re.escape(lane) + r"'")
            for edge in edges:
                if edge.get("from") != ".":
                    continue
                if not wanted.search(str(edge.get("condition") or "")):
                    continue
                target = str(edge.get("to") or "")
                if not target.startswith("./"):
                    continue
                answerer = target[2:]
                if answerer == expected:
                    continue
                findings.append(Finding(
                    "R4", (d / answerer).relative_to(repo_root).as_posix(),
                    f"`{root.name}` answers the `{lane}` lane with "
                    f"`./{answerer}`; the ruled name of that answerer is "
                    f"`{expected}`. One lane, one cell name -- a caller should "
                    f"not have to know which word this hive chose.",
                ))

        # --- R5, the declaration half: an `at` that names no connect point ---
        # Independent of any edge. `at` is compared against the endpoint's path
        # relative to the declaring level, which is always `./…`, so `"."` and a
        # bare name match nothing -- a lane declared that way reads like a
        # permission and grants none, and the lane's rim-door duty is lifted
        # (`docks_below_the_rim`) for a connect point that does not exist.
        for route, raw in malformed_connect_points(cfg):
            findings.append(Finding(
                "R5", rel,
                f"the `{route}` entry of this contract carries the connect point "
                f"{raw}, which is not of the form `./…`. An `at` names a path "
                f"STRICTLY BELOW the declaring hive and is matched literally "
                f"against it -- `\".\"` and a bare name never match, so the lane "
                f"has no connect point at all. A lane meant to end on this hive's "
                f"own rim is declared one level UP, as `./{d.name}` in the "
                f"parent's contract.",
            ))

        # --- R5 -------------------------------------------------------------
        for edge in edges:
            deep = [
                str(edge[key]) for key in ("from", "to")
                if isinstance(edge.get(key), str)
                and edge[key].startswith("./") and "/" in edge[key][2:]
            ]
            if not deep:
                continue
            lane = edge.get("lane")
            if not isinstance(lane, str) or not lane.strip():
                findings.append(Finding(
                    "R5", rel,
                    f"the edge `{edge.get('from')} -> {edge.get('to')}` reaches "
                    f"{'a level' if len(deep) == 1 else 'levels'} below its own "
                    f"occupants and carries no `lane`. A deep edge is a v-lane and "
                    f"names its lane: the validation cannot read one out of a CEL "
                    f"guard, and without it neither the connect point nor the "
                    f"mandatory hops can be checked.",
                ))
                continue
            for target in deep:
                found, said = v_lane_walk(d, lane.strip(), target, root, repo_root)
                findings.extend(found)
                notes.extend(said)

    return findings, notes


def role_family_warnings(templates: Path, repo_root: Path) -> list[str]:
    """R4's unruled half: the same role under a second name (GH #551 § 2)."""
    warnings: list[str] = []
    for role, preferred, others, issue in ROLE_FAMILIES:
        seen: list[str] = []
        for d in sorted(templates.glob("*/*")):
            if not (d / "config.json").is_file() or d.name not in others:
                continue
            seen.append(d.relative_to(repo_root).as_posix())
        if seen:
            warnings.append(
                f"R4 (warn) {role}: {', '.join(seen)} -- the survivor the sweep "
                f"proposed is `{preferred}`. Unruled, see GH #{issue}."
            )
    return warnings


def example_instance_notes(examples: Path) -> list[str]:
    """The informational half of R1: instances an author named for themselves.

    Never a finding. It is here so the boundary is visible in the gate's own
    output rather than only in the docstring.
    """
    named: list[str] = []
    if not examples.is_dir():
        return named
    for cfg_path in sorted(examples.rglob("*.json")):
        try:
            data = json.loads(cfg_path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        if not isinstance(data, dict):
            continue
        if (data.get("cell") or {}).get("type") == "ref":
            referenced = str((data["cell"]).get("template") or "").split("@")[0]
            if referenced and referenced != cfg_path.parent.name:
                named.append(f"{cfg_path.parent.name} -> {referenced}")
            continue
        for block in data.get("manifest") or []:
            diff = (block or {}).get("diff") or {}
            for node in diff.get("add_nodes") or []:
                template = str((node or {}).get("template") or "").split("@")[0]
                name = str((node or {}).get("name") or "")
                if template and name and template != name:
                    named.append(f"{name} -> {template}")
    return named


def scan(repo_root: Path) -> tuple[list[Finding], list[str], list[str]]:
    templates = repo_root / "templates"
    findings: list[Finding] = []
    if not templates.is_dir():
        raise ValueError(f"{templates} is missing")
    roots = sorted(
        d for d in templates.iterdir()
        if d.is_dir() and not d.name.startswith("_")
    )
    lane_notes: list[str] = []
    for root in roots:
        found, said = check_template(root, repo_root)
        findings.extend(found)
        lane_notes.extend(said)
    warnings = role_family_warnings(templates, repo_root) + lane_notes
    notes = example_instance_notes(repo_root / "examples")
    return findings, warnings, notes


def run(repo_root: Path, quiet: bool = False) -> int:
    findings, warnings, instance_notes = scan(repo_root)

    hard: list[Finding] = []
    excused: list[Finding] = []
    for f in findings:
        if (f.rule, f.path) in TRANSITIONAL:
            excused.append(f)
        else:
            hard.append(f)

    matched = {(f.rule, f.path) for f in excused}
    stale = [key for key in TRANSITIONAL if key not in matched]

    out = [] if quiet else None

    def say(text: str) -> None:
        if out is None:
            print(text)
        else:
            out.append(text)

    for f in excused:
        issue, what = TRANSITIONAL[(f.rule, f.path)]
        say(
            f"  note: TRANSITIONAL (added {TRANSITIONAL_ADDED}, GH #{issue}: "
            f"{what}) -- {f.line()}"
        )
    for key in stale:
        issue, what = TRANSITIONAL[key]
        say(
            f"  note: STALE transition row -- ('{key[0]}', '{key[1]}') matches "
            f"nothing any more. GH #{issue} ({what}) landed: delete that line "
            f"from TRANSITIONAL in scripts/check_tree_rules.py."
        )
    for w in warnings:
        say(f"  note: {w}")
    if instance_notes:
        say(
            f"  note: R1 does not bind {len(instance_notes)} instance name(s) "
            f"under examples/ -- a ref inside a template is named after its "
            f"template, an instance grown by a manifest is named by whoever "
            f"grows it ({', '.join(sorted(set(instance_notes)))})."
        )

    if hard:
        print(f"\ntree rules RED ({len(hard)}):", file=sys.stderr)
        for f in hard:
            print(f"  - {f.line()}", file=sys.stderr)
        return 1

    say(
        f"tree rules OK: R1/R3/R4/R5 hold across templates/; "
        f"{len(excused)} transitional, {len(warnings)} note(s)."
    )
    return 0


# --- the gate's own pin ------------------------------------------------------

SELFTEST_TREE = {
    # R1: a ref named for its role instead of its template.
    "templates/fixture/rolename/config.json": {
        "cell": {"type": "ref", "template": "terminal@1.0.0"}
    },
    # R3: an occupant no edge names.
    "templates/fixture/island/config.json": {"cell": {"type": "echo"}},
    # R3: the same, declared -- and therefore not a finding.
    "templates/fixture/declared-island/config.json": {
        "cell": {"type": "echo"}, "unwired": True
    },
    # R4: the ruled lane answered under another name.
    "templates/fixture/declare/config.json": {"cell": {"type": "code"}},
    "templates/fixture/config.json": {
        "cell": {"type": "hive"},
        "params": {
            "ports": [],
            "contract": {"accepts": [{"route": "in_schemas"}]},
            "graph": {"edges": [
                {"from": ".", "to": "./rolename"},
                {"from": ".", "to": "./declare",
                 "condition": "has(hop.route) && hop.route == 'in_schemas'"},
            ]},
        },
    },
    # The clean control: a template with nothing wrong in it.
    "templates/clean/config.json": {
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [{"from": ".", "to": "./terminal"}]}},
    },
    "templates/clean/terminal/config.json": {
        "cell": {"type": "ref", "template": "terminal@1.0.0"}
    },

    # --- R5: one deep edge per row of the rule table -------------------------
    # Four failures and two silences, drawn from the graph of the same ancestor
    # so the walk starts where a real v-lane starts.
    "templates/vlane/config.json": {
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [
            {"from": ".", "to": "./bare"},
            {"from": ".", "to": "./bare/inner"},
            {"from": ".", "to": "./unlisted"},
            {"from": ".", "to": "./unlisted/inner", "lane": "in_recall"},
            {"from": ".", "to": "./gate"},
            {"from": ".", "to": "./gate/deep/inner", "lane": "in_recall"},
            {"from": ".", "to": "./sealed"},
            {"from": ".", "to": "./sealed/inner", "lane": "in_recall"},
            {"from": ".", "to": "./vouched"},
            {"from": ".", "to": "./vouched/inside/leaf", "lane": "in_recall"},
            {"from": ".", "to": "./anchor"},
            {"from": ".", "to": "./anchor/inner", "lane": "in_recall"},
            {"from": ".", "to": "./member"},
            {"from": ".", "to": "./member/talky", "lane": "in_recall"},
            {"from": ".", "to": "./dotty"},
            {"from": ".", "to": "./dotty/inner", "lane": "in_recall"},
            {"from": ".", "to": "./barename"},
            {"from": ".", "to": "./barename/inner", "lane": "in_recall"},
            {"from": ".", "to": "./farvouch"},
            {"from": ".", "to": "./farvouch/mid/leaf", "lane": "in_recall"},
            {"from": ".", "to": "./sealedmid"},
            {"from": ".", "to": "./sealedmid/mid/leaf", "lane": "in_recall"},
        ]}},
    },
    # (a) a deep edge with no lane at all.
    "templates/vlane/bare/config.json": {"cell": {"type": "hive"}},
    "templates/vlane/bare/inner/config.json": {"cell": {"type": "echo"}},
    # (b) a lane nobody on the way declares an `at` for.
    "templates/vlane/unlisted/config.json": {"cell": {"type": "hive"}},
    "templates/vlane/unlisted/inner/config.json": {"cell": {"type": "echo"}},
    # (c) a level that declares the lane without an `at`: it may not be skipped,
    #     even though the connect point one level further down is in order.
    "templates/vlane/gate/config.json": {
        "cell": {"type": "hive"},
        "params": {
            "contract": {"accepts": [{"route": "in_recall"}]},
            "graph": {"edges": [{"from": ".", "to": "./deep"}]},
        },
    },
    "templates/vlane/gate/deep/config.json": {
        "cell": {"type": "hive"},
        "params": {"contract": {"accepts": [
            {"route": "in_recall", "at": ["./inner"]}
        ]}},
    },
    "templates/vlane/gate/deep/inner/config.json": {"cell": {"type": "echo"}},
    # (d) a sealed level the lane is not declared through -- and it is the
    #     endpoint's parent, so the table answers it as row 5 (the seal check is
    #     a stage of its own); the genuine seal row is fixture (i) below.
    "templates/vlane/sealed/config.json": {
        "cell": {"type": "hive"}, "params": {"ports": []}
    },
    "templates/vlane/sealed/inner/config.json": {"cell": {"type": "echo"}},
    # (e) an ancestor vouches for the whole way down -- and may not: the level
    #     under it is the endpoint's parent and owes its own connect point
    #     (review 2026-09-01). Its seal is a second reason, not the reason.
    "templates/vlane/vouched/config.json": {
        "cell": {"type": "hive"},
        "params": {
            "contract": {"accepts": [
                {"route": "in_recall", "at": ["./inside/leaf"]}
            ]},
            "graph": {"edges": [{"from": ".", "to": "./inside"}]},
        },
    },
    "templates/vlane/vouched/inside/config.json": {
        "cell": {"type": "hive"}, "params": {"ports": []}
    },
    "templates/vlane/vouched/inside/leaf/config.json": {"cell": {"type": "echo"}},
    # The first silence: the target declares where the lane docks.
    "templates/vlane/anchor/config.json": {
        "cell": {"type": "hive"},
        "params": {
            "ports": [],
            "contract": {"emits": [{"route": "in_recall", "at": ["./inner"]}]},
        },
    },
    "templates/vlane/anchor/inner/config.json": {"cell": {"type": "echo"}},
    # (f) the endpoint declares `at: ["."]` -- the spelling the docs called "my
    #     own rim" until the review of 2026-09-01. It matches nothing: Stage 6
    #     compares `at` against `./…`, so the endpoint anchors nothing and the
    #     PARENT still owes the connect point. Two findings, and both are the
    #     point -- the malformed entry, and the consequence.
    "templates/vlane/dotty/config.json": {"cell": {"type": "hive"}},
    "templates/vlane/dotty/inner/config.json": {
        "cell": {"type": "hive"},
        "params": {"contract": {"accepts": [
            {"route": "in_recall", "at": ["."],
             "because": "I would distribute it myself -- except this says nothing"}
        ]}},
    },
    # (g) the same in the other bad spelling: a bare name where a `./…` belongs.
    "templates/vlane/barename/config.json": {
        "cell": {"type": "hive"},
        "params": {"contract": {"accepts": [
            {"route": "in_recall", "at": ["inner"]}
        ]}},
    },
    "templates/vlane/barename/inner/config.json": {"cell": {"type": "echo"}},
    # (h) an UNSEALED ancestor vouches for the whole way down and the endpoint's
    #     parent says nothing. Silent until the review of 2026-09-01, because
    #     the walk carried one `anchored` flag for the whole way; Stage 6 asks
    #     the parent alone (row 5, `is_target`), and now so does this half.
    "templates/vlane/farvouch/config.json": {
        "cell": {"type": "hive"},
        "params": {
            "contract": {"accepts": [
                {"route": "in_recall", "at": ["./mid/leaf"]}
            ]},
            "graph": {"edges": [{"from": ".", "to": "./mid"}]},
        },
    },
    "templates/vlane/farvouch/mid/config.json": {"cell": {"type": "hive"}},
    "templates/vlane/farvouch/mid/leaf/config.json": {"cell": {"type": "echo"}},
    # (i) a SEALED level that is a genuine intermediate rather than the target:
    #     the seal row of the table, which the two fixtures above it can no
    #     longer show now that a sealed TARGET is answered as row 5 instead.
    "templates/vlane/sealedmid/config.json": {
        "cell": {"type": "hive"},
        "params": {"ports": [], "graph": {"edges": [{"from": ".", "to": "./mid"}]}},
    },
    "templates/vlane/sealedmid/mid/config.json": {
        "cell": {"type": "hive"},
        "params": {"contract": {"accepts": [
            {"route": "in_recall", "at": ["./leaf"]}
        ]}},
    },
    "templates/vlane/sealedmid/mid/leaf/config.json": {"cell": {"type": "echo"}},
    # The second silence: the same, one `ref` hop away -- the walk reads the
    # referenced template's contract, which is where a v-lane meets it in the
    # real tree.
    "templates/vlane/member/config.json": {
        "cell": {"type": "ref", "template": "member@1.0.0"}
    },
    "templates/member/config.json": {
        "cell": {"type": "hive"},
        "params": {
            "ports": [],
            "contract": {"accepts": [{"route": "in_recall", "at": ["./talky"]}]},
            "graph": {"edges": [{"from": ".", "to": "./talky"}]},
        },
    },
    "templates/member/talky/config.json": {"cell": {"type": "echo"}},
}


def selftest() -> int:
    """Feed the checker a fixture tree with one violation of each kind."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for rel, body in SELFTEST_TREE.items():
            path = root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(body), encoding="utf-8")

        findings, _, _ = scan(root)
        got = sorted((f.rule, f.path) for f in findings)
        want = sorted([
            ("R1", "templates/fixture/rolename"),
            ("R3", "templates/fixture/island"),
            ("R4", "templates/fixture/declare"),
            # R5, one per failing row of the rule table.
            ("R5", "templates/vlane"),                  # no lane on a deep edge
            ("R5", "templates/vlane/unlisted"),         # v_lane_no_connect_point
            ("R5", "templates/vlane/gate"),             # v_lane_mandatory_hop
            ("R5", "templates/vlane/sealedmid"),        # hive_port_boundary
            ("R5", "templates/vlane/sealed"),           # ... a sealed TARGET: row 5
            ("R5", "templates/vlane/vouched/inside"),   # ... below a vouching level
            # R5, the two spellings that name no connect point (review 2026-09-01).
            ("R5", "templates/vlane/dotty/inner"),      # the `at: ["."]` entry
            ("R5", "templates/vlane/dotty"),            # and its consequence
            ("R5", "templates/vlane/barename"),         # the bare-name entry
            ("R5", "templates/vlane/barename"),         # and its consequence
            # R5, a vouching ANCESTOR is not the endpoint's parent.
            ("R5", "templates/vlane/farvouch/mid"),
        ])
        if got != want:
            print("selftest FAILED", file=sys.stderr)
            print(f"  expected: {want}", file=sys.stderr)
            print(f"  got:      {got}", file=sys.stderr)
            return 1

        # The declared island, the clean template and the two declared v-lanes
        # produce nothing -- a gate that fires on everything measures nothing.
        silent = ("templates/clean", "templates/vlane/anchor",
                  "templates/vlane/member", "templates/member")
        for rule, path in got:
            if "declared-island" in path or path.startswith(silent):
                print(f"selftest FAILED: {rule} fired on {path}", file=sys.stderr)
                return 1

    print("tree rules selftest OK: R1, R3, R4 fire exactly once and R5 once per "
          "failing row of its table -- including an `at` of `\".\"` or a bare "
          "name, and an ancestor that vouched but is not the endpoint's parent; "
          "a declared island, a clean template and a declared v-lane (also "
          "across a ref) stay silent.")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true",
                    help="accepted for symmetry with the sibling gates; a no-op")
    ap.add_argument("--root", default=None,
                    help="scan another tree instead of this repository")
    ap.add_argument("--selftest", action="store_true",
                    help="run the gate against its own fixture tree")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    root = Path(args.root).resolve() if args.root else REPO_ROOT
    try:
        return run(root)
    except ValueError as exc:
        print(f"tree rules: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
