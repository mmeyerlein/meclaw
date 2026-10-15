#!/usr/bin/env python3
"""Fold the flat integration tests of one crate into a few harness binaries.

    scripts/harness_fold.py --crate <c> --plan     print file -> harness -> module
    scripts/harness_fold.py --crate <c> --apply    move the files (git mv), write main.rs
    scripts/harness_fold.py --crate <c> --unfold   back to the flat form, byte for byte
    scripts/harness_fold.py --crate <c> --check    exit 1 unless the tree is the plan

WHY
===
Cargo builds every `crates/<c>/tests/<stem>.rs` as its own crate: its own
metadata resolution, its own front end over the shared helpers, its own link.
meclaw-cells has about a thousand of them, so one touch of its lib relinks a
thousand binaries. A harness is ONE test crate,
`crates/<c>/tests/<harness>/main.rs`, that declares the old files as modules
(`pub mod <stem>;`). nextest still runs every test in its own process; the
filter for an old file becomes
`(binary_id(=<c>::<harness>) & test(/^<stem>::/))`.

THE TABLE
=========
`crates/<c>/tests/HARNESS.toml` decides, and is committed:

    max_files = 60            # a harness above this splits into <name>_1.._n
    keep = ["some_stem"]      # stays its own binary, whatever the rules say
    [[rule]]
    match = "^phase_"         # a regex on the stem, first match wins
    harness = "phase"
    [[rule]]
    uses = "^support/x\\.rs$" # a regex on a module file the group declares
    harness = "x"             # (both keys given: both must hold)

A stem no rule matches goes to `misc`. Without a table every foldable file
goes to `misc`.

WHAT STAYS ITS OWN BINARY
=========================
The scenario class and the browser locks of `gate_plan.py` (stations address
them by binary name), `[[test]]` targets of the crate's Cargo.toml, and every
file the fold would change in meaning (BLOCK): a `#[path]` other than the
two-line form below, an `include!`, an
`include_str!`/`include_bytes!` with a path relative to the file, a `fn main`,
a `crate::` path (it would point at main.rs), a crate-root attribute
(`#![feature]`, `#![recursion_limit]`, `#![no_std]`, ...), a `#[macro_export]`,
an `#[ignore]` proof (a harness would widen `--run-ignored` to everything
in it), a test that re-executes its own binary by test name
(`current_exe()`), and a `mod x;` that resolves to nothing under `tests/`.

THE ONE TEXT CHANGE
===================
A file's bytes stay identical except each `mod <x>;` line that names a shared
helper (`tests/<x>/mod.rs`) or a sibling test file (`tests/<x>.rs`): main.rs
declares that module ONCE, and the line becomes
`use crate::<x>; // harness_fold: <the old line>` -- the comment is what makes
`--unfold` exact. Files linked by sibling `mod` lines share a harness; if one of
them must stay its own binary, all of them do.

The two-line form `#[path = "<p>"]` + `mod <x>;` (p relative to tests/, the
file exists under tests/) folds the same way: main.rs declares
`#[path = "../<p>"] pub mod <x>;` once, the attribute line becomes
`// harness_fold: <it>` and the mod line the `use` above. Helpers name each
other through the crate root (`crate::road`), so a module name keeps its file
inside a harness: two groups that mean different files by one name go to
different parts (`<name>_1.._n`), one group that does so stays its own binaries
(`block:module_name_clash`). Every folded `use` carries
`#[allow(unused_imports)]`: a file may declare a module only for a helper.

`--apply` is idempotent: it computes the target from the logical flat form, so
running it twice, or after a table edit, moves only what has to move.
Standard library only.
"""

import argparse
import os
import pathlib
import re
import subprocess
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
MARKER = "harness_fold.py"
DEFAULT_MAX = 60

_MOD_RE = re.compile(
    r"^(?P<indent>\s*)(?P<attrs>(?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(?P<name>[A-Za-z_][A-Za-z_0-9]*)\s*;\s*$")
_ALLOW = "#[allow(unused_imports)] "
# rustfmt puts an outer attribute on its own line; the fold writes it there (W5 delta, PRE fmt RED)
_ALLOW_LINE = "#[allow(unused_imports)]"
_FOLDED_RE = re.compile(
    r"^(?P<indent>\s*)(?:#\[allow\(unused_imports\)\] )?use crate::(?P<name>[A-Za-z_][A-Za-z_0-9]*);"
    r" // harness_fold: (?P<orig>.*)$")
_PATH_ATTR_RE = re.compile(r'^(?P<indent>\s*)#\[\s*path\s*=\s*"(?P<rel>[^"]+)"\s*\]\s*$')
_FOLDED_PATH_RE = re.compile(r"^(?P<indent>\s*)// harness_fold: (?P<orig>#\[\s*path\s*=.*)$")
_MAIN_MOD_RE = re.compile(r"^pub mod ([A-Za-z_][A-Za-z_0-9]*);$")
_IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z_0-9]*$")  # a stem must be a module name to fold

BLOCKS = (
    ("path", re.compile(r"#\[\s*path\s*=")),
    ("include", re.compile(r"\binclude!\s*\(")),
    ("relative_include", re.compile(r"\binclude_(?:str|bytes)!\s*\(\s*\"")),
    ("fn_main", re.compile(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+main\s*\(")),
    ("crate_path", re.compile(r"\bcrate::")),
    ("crate_attr", re.compile(
        r"#!\[\s*(?:recursion_limit|feature|no_std|no_main|no_core|type_length_limit|"
        r"test_runner|reexport_test_harness_main|crate_name|crate_type|cfg_attr\s*\(.*\b(?:feature|recursion_limit)\b)")),
    ("macro_export", re.compile(r"#\[\s*macro_export")),
    ("ignore_proof", re.compile(r"#\[\s*ignore\b")),
    ("self_exec", re.compile(r"\bcurrent_exe\s*\(")),
)


def _gate_plan_keeps():
    """(scenario regexes, browser-lock paths) from gate_plan.py -- the one copy."""
    sys.path.insert(0, HERE)
    try:
        import gate_plan as gp
    finally:
        sys.path.pop(0)
    regs = []
    for term in gp.SCENARIO_TERMS:
        # one binary operator only; a compound keeps every stem (tb-s10 D)
        m = gp._ONE_BINARY_RE.match(term)
        regs.append(re.compile(m.group(1) if m else ""))
    return regs, frozenset(gp.BROWSER_LOCKS)


_BINARY_ID_RE = re.compile(r"binary_id\(\s*(=[^)\s]+|/[^/)]*/)\s*\)")


def _nextest_keeps(repo, crate):
    """(exact stems, id regexes) of the binaries .config/nextest.toml names in
    a filter -- folded, the binary id is gone and the override parse fails."""
    path = os.path.join(repo, ".config", "nextest.toml")
    exact, regs = set(), []
    if not os.path.isfile(path):
        return exact, regs
    for arg in _BINARY_ID_RE.findall(_read(path)):
        if arg.startswith("=") and arg[1:].startswith(crate + "::"):
            exact.add(arg[1 + len(crate) + 2:])
        elif arg.startswith("/") and len(arg) > 2:
            regs.append(re.compile(arg[1:-1]))
    return exact, regs


_DOC_LINK_RE = re.compile(r"\]\((?:\.\./)+crates/(?P<crate>[^/)]+)/tests/(?P<stem>[A-Za-z0-9_]+)\.rs[)#]")


def _doc_linked_stems(repo, crate):
    """Stems a PUBLIC template README links as `../../crates/<crate>/tests/<stem>.rs`.

    templates/ stays untouched by the fold (OR-DT.W5.doc), so a folded target
    leaves a dead relative link in the exported README and the export audit's R10
    goes red (W5 delta B-C1: 17 links in 10 READMEs). Such a file stays flat.
    Without the export script nothing is public and nothing is kept.
    """
    path = os.path.join(repo, "plans", "export-fixtures", "make_export.py")
    if not os.path.isfile(path):
        return set()
    import ast
    public = []
    for node in ast.parse(_read(path)).body:
        if (isinstance(node, ast.Assign) and isinstance(node.value, (ast.List, ast.Tuple))
                and any(isinstance(t, ast.Name) and t.id == "PUBLIC_TEMPLATES" for t in node.targets)):
            public = [e.value for e in node.value.elts
                      if isinstance(e, ast.Constant) and isinstance(e.value, str)]
    out = set()
    for name in public:
        readme = os.path.join(repo, "templates", name, "README.md")
        if os.path.isfile(readme):
            out |= {m["stem"] for m in _DOC_LINK_RE.finditer(_read(readme)) if m["crate"] == crate}
    return out


# A test path a doc NAMES in a code span (`crates/<c>/tests/<stem>.rs`). R10
# and `_doc_linked_stems` see links only, so the fold left 151 dead names in
# template READMEs (W5 re-check N3). docs/superpowers/plans/ and specs/ are
# historical, like plans/: they record tests as they were (one names a test
# deleted in 4c9804e88, before the fold) and are never rewritten. They are
# excused by a FROZEN list, not by their directory (W5 re-check, Koordinator
# 09.10.): the 124 dead mentions of 09.10. as (doc, path) -> count. A mention
# beyond its count, a new path or an unlisted doc there is dead like any other.
_DOC_MENTION_RE = re.compile(r"(?<![\w/.<])((?:\.\./)*crates/[\w-]+/tests/[\w/]+\.rs)")
_CODE_SPAN_RE = re.compile(r"`([^`\n]+)`")
HISTORICAL_DOCS = ("docs/superpowers/plans/", "docs/superpowers/specs/")
# Any other text file under templates/ and docs/ (config.json, template.json,
# the librarian's docs.jsonl, canvy .js/.py) names a test path without a code
# span: it is read whole. A file that is not UTF-8 is not text and is skipped
# (W5 OR-DT.W5.tpl rest: 29 dead mentions in 22 such files, 09.10.).

# Generated from the tree of 09.10. -- shrink it when a doc is fixed, never grow it.
FROZEN_HISTORICAL = {
    ("docs/superpowers/plans/2026-08-20-consistency-remediation.md",
     "crates/meclaw-cells/tests/gh155_steward_loop.rs"): 1,
    ("docs/superpowers/plans/2026-08-21-w1-hygiene-pins.md",
     "crates/meclaw-cells/tests/gh155_steward_loop.rs"): 4,
    ("docs/superpowers/plans/2026-08-21-w1-hygiene-pins.md",
     "crates/meclaw-colony/tests/gh163_surface_into_a_running_colony.rs"): 1,
    ("docs/superpowers/plans/2026-08-21-w3-composition-substrate.md",
     "crates/meclaw-cells/tests/channel_composite.rs"): 3,
    ("docs/superpowers/plans/2026-08-21-w3-composition-substrate.md",
     "crates/meclaw-cells/tests/channel_template.rs"): 3,
    ("docs/superpowers/plans/2026-08-21-w3-composition-substrate.md",
     "crates/meclaw-colony/tests/gh163_surface_into_a_running_colony.rs"): 2,
    ("docs/superpowers/plans/2026-08-21-w5-per-turn-extraction.md",
     "crates/meclaw-cells/tests/f9_inline_contract.rs"): 3,
    ("docs/superpowers/plans/2026-08-21-w5-per-turn-extraction.md",
     "crates/meclaw-cells/tests/gh143_extract_error_budget.rs"): 1,
    ("docs/superpowers/plans/2026-08-21-w5-per-turn-extraction.md",
     "crates/meclaw-cells/tests/gh298_the_turn_writes_its_own_episode.rs"): 2,
    ("docs/superpowers/plans/2026-08-21-w5-per-turn-extraction.md",
     "crates/meclaw-cells/tests/w13_claim_lease.rs"): 1,
    ("docs/superpowers/plans/2026-08-21-w5-per-turn-extraction.md",
     "crates/meclaw-cells/tests/w9a_per_turn_episodes.rs"): 4,
    ("docs/superpowers/plans/2026-08-21-w5-per-turn-extraction.md",
     "crates/meclaw-colony/tests/gh243_a_memory_can_leave_a_hive_and_arrive_in_another.rs"): 3,
    ("docs/superpowers/plans/2026-08-21-w6-identity-steward.md",
     "crates/meclaw-cells/tests/gh155_steward_loop.rs"): 6,
    ("docs/superpowers/plans/2026-08-21-w6-identity-steward.md",
     "crates/meclaw-colony/tests/gh163_surface_into_a_running_colony.rs"): 2,
    ("docs/superpowers/plans/2026-08-21-w7-os-composition.md",
     "crates/meclaw-cells/tests/channel_composite.rs"): 1,
    ("docs/superpowers/plans/2026-08-21-w7-os-composition.md",
     "crates/meclaw-cells/tests/channel_template.rs"): 5,
    ("docs/superpowers/plans/2026-08-21-w7-os-composition.md",
     "crates/meclaw-colony/tests/gh163_surface_into_a_running_colony.rs"): 1,
    ("docs/superpowers/plans/2026-08-24-w8-web-cell.md",
     "crates/meclaw-cli/tests/gh325_g1_knows_the_types_the_registry_spawns.rs"): 1,
    ("docs/superpowers/plans/2026-08-24-w8-web-cell.md",
     "crates/meclaw-colony/tests/gh163_surface_into_a_running_colony.rs"): 1,
    ("docs/superpowers/plans/2026-08-26-lane1-tier1-bundles.md",
     "crates/meclaw-cells/tests/gh419_the_collector_round_is_one_bundle.rs"): 4,
    ("docs/superpowers/plans/2026-08-26-lane1-tier1-bundles.md",
     "crates/meclaw-cells/tests/w11_curator.rs"): 1,
    ("docs/superpowers/plans/2026-08-26-lane5-builder-intake.md",
     "crates/meclaw-colony/tests/gh425_a_manifest_can_register_a_local_template.rs"): 1,
    ("docs/superpowers/plans/2026-08-27-lane2-mutation-core.md",
     "crates/meclaw-cells/tests/gh437_the_readme_birth_promise_holds.rs"): 1,
    ("docs/superpowers/plans/2026-08-27-lane3-template-intake.md",
     "crates/meclaw-cells/tests/gh218_llm_unit_errors_table_has_a_writer.rs"): 1,
    ("docs/superpowers/plans/2026-08-27-lane3-template-intake.md",
     "crates/meclaw-cli/tests/gh325_g1_knows_the_types_the_registry_spawns.rs"): 1,
    ("docs/superpowers/plans/2026-08-27-lane5-manifest-chain.md",
     "crates/meclaw-colony/tests/gh438_two_submissions_do_not_swap_receipts.rs"): 1,
    ("docs/superpowers/plans/2026-08-27-lane6-surface-retire.md",
     "crates/meclaw-api/tests/gh159_surface_bundles.rs"): 2,
    ("docs/superpowers/plans/2026-09-02-aufraeum-welle-500er.md",
     "crates/meclaw-cells/tests/gh563_the_export_markers_land_whole.rs"): 1,
    ("docs/superpowers/plans/2026-09-04-vorwelle-prozess.md",
     "crates/meclaw-cells/tests/gh1_uses_hive.rs"): 1,
    ("docs/superpowers/plans/2026-09-04-vorwelle-prozess.md",
     "crates/meclaw-cells/tests/gh2_all.rs"): 1,
    ("docs/superpowers/plans/2026-09-04-vorwelle-prozess.md",
     "crates/meclaw-cells/tests/gh3_llm.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cells/tests/gate_contract.rs"): 3,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cells/tests/gate_listener.rs"): 6,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cells/tests/gate_params.rs"): 4,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cells/tests/gate_receipt.rs"): 3,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cells/tests/gate_template.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cells/tests/gate_wire.rs"): 3,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-cli/tests/gate_two_colonies.rs"): 3,
    ("docs/superpowers/plans/2026-09-08-colony-lane.md",
     "crates/meclaw-core/tests/ingress_sink.rs"): 3,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_a_blob_keeps_its_file.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_a_crash_does_not_end_an_erasure.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_a_database_may_refuse_to_forget.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_a_selector_names_rows.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_an_erase_needs_a_selector.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_an_erase_reaches_last_input.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_an_erasure_does_not_roll_back.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_an_erasure_is_shown_not_claimed.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_the_neighbours_are_byte_identical.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_the_ninth_key.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_the_receipt_is_the_record.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_the_receipt_names_a_date.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-erase-member.md",
     "crates/meclaw-colony/tests/gh618_the_tree_is_the_selector.rs"): 1,
    ("docs/superpowers/plans/2026-09-08-phone-dispatcher.md",
     "crates/meclaw-cells/tests/switchboard_routes_a_call_to_the_colony_it_belongs_to.rs"): 9,
    ("docs/superpowers/plans/2026-09-09-phone-dispatcher.md",
     "crates/meclaw-cells/tests/switchboard_routes_a_call_to_the_colony_it_belongs_to.rs"): 10,
    ("docs/superpowers/plans/2026-09-13-post-kleinkram-welle.md",
     "crates/meclaw-cells/tests/gh679_the_screen_curates_what_it_shows.rs"): 1,
    ("docs/superpowers/plans/2026-09-17-display-welle-h.md",
     "crates/meclaw-cells/tests/support/display.rs"): 1,
    ("docs/superpowers/plans/2026-09-19-welle-p-prozess.md",
     "crates/meclaw-cells/tests/7xx_one_pass_at_a_time.rs"): 1,
    ("docs/superpowers/specs/2026-09-06-phone-app-design.md",
     "crates/meclaw-cells/tests/phone_channel_places_a_call_and_hears_the_line.rs"): 1,
    ("docs/superpowers/specs/2026-09-08-phone-dispatcher-design.md",
     "crates/meclaw-cells/tests/phone_channel_places_a_call_and_hears_the_line.rs"): 1,
}


def _tree_files(root):
    """Sorted files under `root` (every suffix -- the non-md scan of Z-D P3
    needs them); an unreadable directory raises `OSError` (W5 re-check 4 M2:
    `rglob` skipped it silently -- a green over a hole)."""
    if not root.is_dir():
        return []

    def fail(exc):
        raise exc
    found = []
    for base, _dirs, files in os.walk(root, onerror=fail):
        found.extend(pathlib.Path(base, f) for f in files)
    return sorted(found)


def stale_frozen(repo, frozen=None):
    """`[(doc, path, frozen_count, dead_count)]`: entries of `frozen` (default
    `FROZEN_HISTORICAL`) the tree no longer needs -- the path lives again, the
    mention or the doc went away (W5 re-check 4 M1). Such an entry would later
    excuse a NEW dead mention of the same pair; shrink it."""
    frozen = FROZEN_HISTORICAL if frozen is None else frozen
    dead = {}
    for key in dead_path_mentions(repo, frozen={}):
        dead[key] = dead.get(key, 0) + 1
    return [(doc, path, n, dead.get((doc, path), 0))
            for (doc, path), n in sorted(frozen.items()) if dead.get((doc, path), 0) < n]


def dead_path_mentions(repo, frozen=None, unread=None):
    """`[(doc, path)]`: test paths named in code spans under `templates/**/*.md`
    and `docs/**/*.md`, or anywhere in another UTF-8 file under those two
    roots, that the tree does not hold (relative ones resolved against the
    file's directory). A historical doc's mention is excused up to its count in
    `frozen` (default `FROZEN_HISTORICAL`), never beyond. A file that is not
    UTF-8 cannot be read; its path goes to the list `unread`, when given, so
    the caller can say so (review Z-D M1). `__pycache__` holds byte code, not
    text, and is not counted."""
    frozen = FROZEN_HISTORICAL if frozen is None else frozen
    out, seen = [], {}
    for top in ("templates", "docs"):
        for doc in _tree_files(pathlib.Path(repo, top)):
            rel = doc.relative_to(repo).as_posix()
            historical = rel.startswith(HISTORICAL_DOCS)
            try:
                text = _read(str(doc))
            except UnicodeDecodeError:
                if unread is not None and "__pycache__" not in doc.parts:
                    unread.append(rel)
                continue
            if doc.suffix == ".md":
                spans = [m.group(1) for m in _CODE_SPAN_RE.finditer(text)]
            else:
                spans = [text] if "/tests/" in text else []
            for span in spans:
                for m in _DOC_MENTION_RE.finditer(span):
                    path = m.group(1)
                    base = doc.parent if path.startswith("../") else pathlib.Path(repo)
                    if os.path.isfile(os.path.normpath(base / path)):
                        continue
                    seen[(rel, path)] = seen.get((rel, path), 0) + 1
                    if historical and seen[(rel, path)] <= frozen.get((rel, path), 0):
                        continue
                    out.append((rel, path))
    return out


def _export_private_stems(repo, crate):
    """Stems of `crates/<crate>/tests/**` the export never ships (its BLOCKLIST).

    Folded, such a file leaves the public tree while its harness `main.rs` still
    declares `pub mod <stem>;` -- the exported crate would not compile (W5-2d:
    ten files, three colony drift locks among them). The export script lives in
    the private tree only; without it nothing is private and nothing is kept.
    """
    path = os.path.join(repo, "plans", "export-fixtures", "make_export.py")
    if not os.path.isfile(path):
        return set()
    import ast
    prefix = f"crates/{crate}/tests/"
    for node in ast.parse(_read(path)).body:
        if (isinstance(node, ast.Assign) and isinstance(node.value, ast.Dict)
                and any(isinstance(t, ast.Name) and t.id == "BLOCKLIST" for t in node.targets)):
            return {os.path.basename(k.value)[:-3] for k in node.value.keys
                    if isinstance(k, ast.Constant) and isinstance(k.value, str)
                    and k.value.startswith(prefix) and k.value.endswith(".rs")}
    return set()


def _code_lines(text):
    for line in text.splitlines():
        s = line.lstrip()
        if s.startswith("//"):
            continue
        yield line


def blocks_of(text):
    """The BLOCK reasons a file's text carries (empty = foldable)."""
    out = []
    lines = list(_code_lines(text))
    for name, rx in BLOCKS:
        if any(rx.search(line) for line in lines):
            out.append(name)
    return out


def _bare_mod(line):
    m = _MOD_RE.match(line)
    return m if m and not m.group("attrs") else None


def path_pairs(text):
    """[(attr line, mod line, module name, path)] of the two-line form
    `#[path = "<p>"]` + `mod <x>;` -- the path as written, relative to tests/."""
    lines = text.split("\n")
    out = []
    for i in range(len(lines) - 1):
        a = _PATH_ATTR_RE.match(lines[i])
        m = a and _bare_mod(lines[i + 1])
        if m:
            out.append((i, i + 1, m.group("name"), a.group("rel")))
    return out


def mod_lines(text):
    """[(line index, module name)] of the plain `mod x;` lines of a file
    (a `mod x;` under a `#[path]` line is a path pair, see path_pairs)."""
    paired = {j for _, j, _, _ in path_pairs(text)}
    out = []
    for i, line in enumerate(text.split("\n")):
        m = _MOD_RE.match(line)
        if m and i not in paired:
            out.append((i, m.group("name")))
    return out


def fold_text(text):
    lines = text.split("\n")
    for i, name in mod_lines(text):
        indent = _MOD_RE.match(lines[i]).group("indent")
        lines[i] = f"{indent}{_ALLOW_LINE}\n{indent}use crate::{name}; // harness_fold: {lines[i].strip()}"
    for i, j, name, _ in path_pairs(text):
        indent = _PATH_ATTR_RE.match(lines[i]).group("indent")
        lines[i] = f"{indent}// harness_fold: {lines[i].strip()}"
        indent = _MOD_RE.match(lines[j]).group("indent")
        lines[j] = f"{indent}{_ALLOW_LINE}\n{indent}use crate::{name}; // harness_fold: {lines[j].strip()}"
    return "\n".join(lines)


def unfold_text(text):
    out = []
    for line in text.split("\n"):
        m = _FOLDED_RE.match(line) or _FOLDED_PATH_RE.match(line)
        if m:
            if (m.re is _FOLDED_RE and out
                    and out[-1] == m.group("indent") + _ALLOW_LINE):
                out.pop()  # the attribute line the fold wrote above the `use`
            line = m.group("indent") + m.group("orig")
        out.append(line)
    return "\n".join(out)


def load_table(tests_dir):
    path = os.path.join(tests_dir, "HARNESS.toml")
    if not os.path.isfile(path):
        return {"max_files": DEFAULT_MAX, "keep": [], "rule": []}
    with open(path, "rb") as fh:
        data = tomllib.load(fh)
    data.setdefault("max_files", DEFAULT_MAX)
    data.setdefault("keep", [])
    data.setdefault("rule", [])
    return data


def _cargo_test_targets(crate_dir):
    path = os.path.join(crate_dir, "Cargo.toml")
    if not os.path.isfile(path):
        return set()
    with open(path, "rb") as fh:
        data = tomllib.load(fh)
    if data.get("package", {}).get("autotests") is False:
        raise SystemExit(f"harness_fold: {path} sets autotests = false -- refusing")
    names = set()
    for t in data.get("test", []):
        if "name" in t:
            names.add(t["name"])
        if "path" in t:
            names.add(os.path.splitext(os.path.basename(t["path"]))[0])
    return names


def _is_harness_main(path):
    try:
        with open(path, encoding="utf-8") as fh:
            head = fh.read(400)
    except OSError:
        return False
    return MARKER in head


class Crate:
    """The logical flat form of one crate's tests, wherever the files sit now."""

    def __init__(self, repo, crate):
        self.repo = repo
        self.crate = crate
        self.crate_dir = os.path.join(repo, "crates", crate)
        self.tests_dir = os.path.join(self.crate_dir, "tests")
        if not os.path.isdir(self.tests_dir):
            raise SystemExit(f"harness_fold: no {self.tests_dir}")
        self.files = {}      # stem -> (current path, logical flat text)
        self.harnesses = {}  # existing harness name -> main.rs path
        self.other_dirs = []  # binary dirs this script did not write
        self.helpers = set()  # tests/<x>/mod.rs helper modules
        for entry in sorted(os.listdir(self.tests_dir)):
            p = os.path.join(self.tests_dir, entry)
            if entry.endswith(".rs") and os.path.isfile(p):
                self.files[entry[:-3]] = (p, _read(p))
            elif os.path.isdir(p):
                main = os.path.join(p, "main.rs")
                if os.path.isfile(main) and _is_harness_main(main):
                    self.harnesses[entry] = main
                elif os.path.isfile(main):
                    self.other_dirs.append(entry)
                elif os.path.isfile(os.path.join(p, "mod.rs")):
                    self.helpers.add(entry)
        for h, main in self.harnesses.items():
            for line in _read(main).split("\n"):
                m = _MAIN_MOD_RE.match(line)
                if not m or m.group(1) in self.helpers:
                    continue
                stem = m.group(1)
                p = os.path.join(self.tests_dir, h, stem + ".rs")
                if not os.path.isfile(p):
                    continue  # a sibling declared once, it sits elsewhere
                if stem in self.files:
                    raise SystemExit(f"harness_fold: {stem} is both flat and in {h}")
                self.files[stem] = (p, unfold_text(_read(p)))
            # a module file main.rs forgot is still a test file: the plan
            # sees it, so `--check` finds the stale main.rs
            hdir = os.path.dirname(main)
            for entry in sorted(os.listdir(hdir)):
                stem = entry[:-3]
                p = os.path.join(hdir, entry)
                if entry.endswith(".rs") and entry != "main.rs" and stem not in self.files:
                    self.files[stem] = (p, unfold_text(_read(p)))

    def _resolve(self, rel):
        """tests/-relative path of a `#[path]` target, or None (missing, outside tests/)."""
        n = os.path.normpath(rel).replace(os.sep, "/")
        if os.path.isabs(n) or n == ".." or n.startswith("../"):
            return None
        if "/" not in n and n.endswith(".rs") and n[:-3] in self.files:
            return n  # a test file, logically flat wherever it sits now
        return n if os.path.isfile(os.path.join(self.tests_dir, n)) else None

    def decls(self, stem, _seen=None, kept=frozenset()):
        """{module name: tests/-relative file} a file needs at the crate root --
        with the modules of the test files it includes by `#[path]`, whose
        folded `use crate::<x>` lines need them at the root too. A file in
        `kept` is not rewritten: its own `mod` lines stay nested, and a root
        copy would load the file twice (clippy::duplicate_mod, W5 clippy)."""
        seen = _seen if _seen is not None else {stem}
        text = self.files[stem][1]
        out = {}
        for _, name in mod_lines(text):
            if name in self.helpers:
                out[name] = f"{name}/mod.rs"
            elif name in self.files:
                out[name] = f"{name}.rs"
        for _, _, name, rel in path_pairs(text):
            r = self._resolve(rel)
            t = r[:-3] if r and "/" not in r else None
            if t in self.files and t not in seen and t not in kept:
                seen.add(t)
                for k, v in self.decls(t, seen, kept).items():
                    out.setdefault(k, v)
        for _, _, name, rel in path_pairs(text):
            out[name] = self._resolve(rel)
        return out

    def _mask_paths(self, text):
        """The text with resolvable path-pair attribute lines blanked (they fold)."""
        lines = text.split("\n")
        for i, _, _, rel in path_pairs(text):
            if self._resolve(rel):
                lines[i] = ""
        return "\n".join(lines)

    def plan(self):
        """{stem: ("keep", reason) | (harness, None)} plus sibling groups."""
        table = load_table(self.tests_dir)
        scen, browser = _gate_plan_keeps()
        cargo_targets = _cargo_test_targets(self.crate_dir)
        nx_exact, nx_regs = _nextest_keeps(self.repo, self.crate)
        private = _export_private_stems(self.repo, self.crate)
        doc_linked = _doc_linked_stems(self.repo, self.crate)
        keep = {}
        links = {}  # stem -> set of sibling stems
        for stem, (_, text) in self.files.items():
            rel = f"crates/{self.crate}/tests/{stem}.rs"
            links.setdefault(stem, set())
            if stem in table["keep"]:
                keep[stem] = "table"
            elif any(r.search(stem) for r in scen):
                keep[stem] = "scenario"
            elif rel in browser:
                keep[stem] = "browser"
            elif stem in cargo_targets:
                keep[stem] = "cargo_target"
            elif stem in nx_exact or any(r.search(f"{self.crate}::{stem}") for r in nx_regs):
                keep[stem] = "nextest_override"
            elif stem in private:
                keep[stem] = "export_private"
            elif stem in doc_linked:
                keep[stem] = "doc_linked"
            elif not _IDENT_RE.match(stem):
                keep[stem] = "block:stem_not_ident"
            else:
                b = blocks_of(self._mask_paths(text))
                if b:
                    keep[stem] = "block:" + ",".join(b)
            for _, name in mod_lines(text):
                if name in self.files:
                    links[stem].add(name)
                    links.setdefault(name, set()).add(stem)
                elif name not in self.helpers and stem not in keep:
                    keep[stem] = f"block:mod_unresolved({name})"
        groups = _groups(links)
        sigs = {}
        for g in groups:
            sig = {s: f"{s}.rs" for s in g}
            clash = False
            for s in g:
                for name, rel in self.decls(s).items():
                    clash = clash or sig.setdefault(name, rel) != rel
            sigs[g[0]] = sig
            if clash:
                for s in g:
                    keep.setdefault(s, "block:module_name_clash")
        # a kept file is not rewritten: its sharers and the test files its
        # `#[path]` lines name stay where it finds them (to a fixpoint)
        changed = True
        while changed:
            changed = False
            for g in groups:
                kept = [s for s in g if s in keep]
                for s in g if kept else ():
                    if s not in keep:
                        keep[s] = f"sharer_of:{kept[0]}"
                        changed = True
            for stem in sorted(keep):
                for _, _, _, rel in path_pairs(self.files[stem][1]):
                    r = self._resolve(rel)
                    t = r[:-3] if r and "/" not in r else None
                    if t in self.files and t not in keep:
                        keep[t] = f"path_target_of:{stem}"
                        changed = True
        rules = [(re.compile(r.get("match", "")), re.compile(r["uses"]) if "uses" in r else None,
                  r["harness"]) for r in table["rule"]]
        by_harness = {}
        assigned = set()
        for g in groups:
            if g[0] in keep:
                continue
            first = g[0]
            files = set(sigs[first].values())
            name = next((h for rx, ux, h in rules if rx.search(first)
                         and (ux is None or any(f and ux.search(f) for f in files))), "misc")
            by_harness.setdefault(name, []).append(g)
            assigned.update(g)
        out = {s: ("keep", r) for s, r in keep.items()}
        cap = int(table["max_files"])
        for name in sorted(by_harness):
            gs = by_harness[name]
            total = sum(len(g) for g in gs)
            parts = -(-total // cap)
            even = -(-total // parts)  # balanced parts, none above the cap
            packed = []  # [size, {module name: file}, [groups]]
            for g in gs:
                sig = sigs[g[0]]
                fits = [p for p in packed if (not p[0] or p[0] + len(g) <= even)
                        and all(p[1].get(k, v) == v for k, v in sig.items())]
                last = packed[-1] if packed else None
                if last is not None and last in fits:
                    part = last
                elif last is not None and (not last[0] or last[0] + len(g) <= even) and fits:
                    part = fits[0]  # the last part fits by size but means another file by a name
                else:
                    part = [0, {}, []]
                    packed.append(part)
                part[0] += len(g)
                part[1].update(sig)
                part[2].append(g)
            for n, part in enumerate(packed, 1):
                label = name if len(packed) == 1 else f"{name}_{n}"
                for g in part[2]:
                    for s in g:
                        out[s] = (label, None)
        for h in {v[0] for v in out.values() if v[0] != "keep"}:
            # a harness named into a `binary_id(/re/)` override inherits it (W5 delta M5)
            if any(r.search(h) for r in scen) or h in self.files or h in self.helpers \
                    or any(r.search(f"{self.crate}::{h}") for r in _nextest_keeps(self.repo, self.crate)[1]) \
                    or h in self.other_dirs:
                raise SystemExit(f"harness_fold: harness name {h!r} collides")
        return out

    def _nested(self, need, stems, kept):
        """{root module: (helper, its name there, pub?)} for a root module whose
        file a helper of the same crate root already loads by its own, unfolded
        `#[path]` line -- one file twice in one crate is clippy::duplicate_mod.
        A `pub mod` there is re-exported; a private one cannot be."""
        by_file = {rel: n for n, rel in need.items() if not (n in stems and rel == f"{n}.rs")}
        out = {}
        for m, rel in sorted(need.items()):
            stem = rel[:-3] if "/" not in rel and rel.endswith(".rs") else None
            if stem in self.files:
                if stem not in kept:
                    continue  # a folded file: its nested lines became `use crate::<x>`
                text = self.files[stem][1]
            else:
                text = _read(os.path.join(self.tests_dir, rel))
            lines = text.split("\n")
            for _, j, inner, nrel in path_pairs(text):
                target = os.path.normpath(os.path.join(os.path.dirname(rel), nrel)).replace(os.sep, "/")
                n = by_file.get(target)
                if n and n != m and m not in out:
                    out.setdefault(n, (m, inner, lines[j].lstrip().startswith("pub mod ")))
        return out

    def _placed(self, rel, plan, flat):
        """Where a tests/-relative file sits in the target state: a test file
        folded into a harness moves to `<harness>/<stem>.rs`."""
        stem = rel[:-3] if "/" not in rel and rel.endswith(".rs") else None
        h = plan.get(stem, ("keep", None))[0] if stem else "keep"
        return rel if flat or h == "keep" else f"{h}/{rel}"

    def target(self, plan, flat=False):
        """{stem: (path, text)} of the target state, plus {harness: main.rs text}."""
        files, mains = {}, {}
        members = {}
        for stem, (h, _) in plan.items():
            text = self.files[stem][1]
            if flat or h == "keep":
                files[stem] = (os.path.join(self.tests_dir, stem + ".rs"), text)
            else:
                files[stem] = (os.path.join(self.tests_dir, h, stem + ".rs"), fold_text(text))
                members.setdefault(h, []).append(stem)
        kept = frozenset(s for s, (h, _) in plan.items() if h == "keep")
        for h, stems in members.items():
            need = {}
            for s in stems:
                need.update(self.decls(s, kept=kept))
            nested = self._nested(need, stems, kept)
            body = [f"//! Test harness `{h}` of `{self.crate}`: one binary, one module per old file.",
                    f"//! Written by scripts/{MARKER} from tests/HARNESS.toml -- edit the table, not this file.",
                    ""]
            # rustfmt's order without rustfmt (#1134): it sorts a run of `mod`
            # items by name (byte order, attributes travel along) and a run of
            # `use` items by path; a `//` comment ends a run. So: uses first,
            # then every plain `mod` in one sorted run, the commented ones last.
            uses, mods, commented = [], [], []
            for n in sorted(need):
                if n in stems and need[n] == f"{n}.rs":
                    continue  # a member file, declared with the others below
                m, inner, public = nested.get(n, (None, None, False))
                if public:
                    uses.append(((m, inner),
                                 f"pub use {m}::{inner};" if inner == n else f"pub use {m}::{inner} as {n};"))
                    continue
                item = [f'#[path = "../{self._placed(need[n], plan, flat)}"]', f"pub mod {n};"]
                if m:
                    commented.append([f"// `{n}` is also a private module inside `{m}`; folding puts both in one crate",
                                      "#[allow(clippy::duplicate_mod)]"] + item)
                else:
                    mods.append((n, item))
            mods += [(s, [f"pub mod {s};"]) for s in stems]
            body += [u for _, u in sorted(uses)]
            body += [line for _, item in sorted(mods) for line in item]
            body += [line for item in commented for line in item]
            mains[h] = "\n".join(body) + "\n"
        # No rustfmt pass (#1134): the text above is already what rustfmt
        # writes, so the fold and its lock give one answer on every host.
        return files, mains

    def realize(self, files, mains, use_git):
        for stem, (dst, text) in sorted(files.items()):
            src = self.files[stem][0]
            if src != dst:
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                _move(self.repo, src, dst, use_git)
            if _read(dst) != text:
                with open(dst, "w", encoding="utf-8", newline="") as fh:
                    fh.write(text)
        for h, main in self.harnesses.items():
            if h not in mains:
                if use_git and _tracked(self.repo, main):
                    _git(self.repo, "rm", "-q", "-f", main)
                elif os.path.exists(main):
                    os.remove(main)
                d = os.path.dirname(main)
                if os.path.isdir(d) and not os.listdir(d):
                    os.rmdir(d)
        for h, text in mains.items():
            main = os.path.join(self.tests_dir, h, "main.rs")
            os.makedirs(os.path.dirname(main), exist_ok=True)
            if not os.path.isfile(main) or _read(main) != text:
                with open(main, "w", encoding="utf-8", newline="") as fh:
                    fh.write(text)
            if use_git:
                _git(self.repo, "add", main)

    def binaries(self, plan=None):
        """Binary count now (plan None) or after the plan."""
        if plan is None:
            flat = sum(1 for p, _ in self.files.values() if os.path.dirname(p) == self.tests_dir)
            return flat + len(self.harnesses) + len(self.other_dirs)
        hs = {h for h, _ in plan.values() if h != "keep"}
        kept = sum(1 for h, _ in plan.values() if h == "keep")
        return kept + len(hs) + len(self.other_dirs)


def _groups(links):
    seen, out = set(), []
    for s in sorted(links):
        if s in seen:
            continue
        stack, g = [s], []
        while stack:
            x = stack.pop()
            if x in seen:
                continue
            seen.add(x)
            g.append(x)
            stack.extend(links.get(x, ()))
        out.append(sorted(g))
    return out


def _read(path):
    with open(path, encoding="utf-8", newline="") as fh:
        return fh.read()


def _git(repo, *args):
    return subprocess.run(["git", "-C", repo, *args], check=True,
                          capture_output=True, text=True).stdout


def _tracked(repo, path):
    r = subprocess.run(["git", "-C", repo, "ls-files", "--error-unmatch", path],
                       capture_output=True, text=True)
    return r.returncode == 0


def _move(repo, src, dst, use_git):
    if use_git and _tracked(repo, src):
        _git(repo, "mv", src, dst)
    else:
        os.rename(src, dst)


def _in_git(repo):
    r = subprocess.run(["git", "-C", repo, "rev-parse", "--is-inside-work-tree"],
                       capture_output=True, text=True)
    return r.returncode == 0 and r.stdout.strip() == "true"


def check(c, plan):
    """Mismatches between the tree and the plan ([] = locked)."""
    files, mains = c.target(plan)
    bad = []
    for stem, (dst, text) in sorted(files.items()):
        src = c.files[stem][0]
        if src != dst:
            bad.append(f"{stem}: at {os.path.relpath(src, c.repo)}, plan says {os.path.relpath(dst, c.repo)}")
        elif _read(src) != text:
            bad.append(f"{stem}: text differs from the fold of its flat form")
    for h, text in mains.items():
        main = os.path.join(c.tests_dir, h, "main.rs")
        if not os.path.isfile(main) or _read(main) != text:
            bad.append(f"{h}/main.rs: missing or stale")
    for h in c.harnesses:
        if h not in mains:
            bad.append(f"{h}/main.rs: harness not in the plan")
    return bad


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--crate", required=True)
    ap.add_argument("--repo", default=REPO_ROOT)
    mode = ap.add_mutually_exclusive_group(required=True)
    for m in ("plan", "apply", "unfold", "check"):
        mode.add_argument(f"--{m}", action="store_true")
    ap.add_argument("--no-git", action="store_true", help="plain renames instead of git mv")
    args = ap.parse_args(argv)
    repo = os.path.abspath(args.repo)
    c = Crate(repo, args.crate)
    plan = c.plan()
    use_git = not args.no_git and _in_git(repo)
    if args.plan:
        for stem in sorted(plan, key=lambda s: (plan[s][0], s)):
            h, why = plan[stem]
            print(f"{stem}\t{h}\t{why if why else stem}")
        hs = sorted({h for h, _ in plan.values() if h != "keep"})
        kept = sum(1 for h, _ in plan.values() if h == "keep")
        print(f"# {args.crate}: {len(plan)} files, {len(plan) - kept} folded into {len(hs)} harnesses"
              f" ({', '.join(hs)}), {kept} kept; binaries {c.binaries()} -> {c.binaries(plan)}")
        return 0
    if args.check:
        bad = check(c, plan)
        for b in bad:
            print(f"harness_fold: {args.crate}: {b}", file=sys.stderr)
        return 1 if bad else 0
    files, mains = c.target(plan, flat=args.unfold)
    c.realize(files, mains if args.apply else {}, use_git)
    print(f"harness_fold: {args.crate}: {'applied' if args.apply else 'unfolded'};"
          f" binaries {c.binaries()} -> {Crate(repo, args.crate).binaries()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
