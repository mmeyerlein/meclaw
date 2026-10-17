#!/usr/bin/env python3
"""meclaw -- resolve a diff into the list of gate stations that must run.

WHY THIS EXISTS
===============
Until now every gate chain ran the full suite for every diff. A scripts-only
change fell through `scripts/test-tier.sh changed` ("no crate touched --
falling back to t1") and paid minutes of rustc for a Python edit. This module
is the ONE place that maps changed paths to stations; nothing downstream --
neither the shell runner nor CI -- may hard-code a station trigger again.

It plans, it does not run. The verdict semantics, the receipt JSON and the
gate-line format belong to the runner.

CLASSES (a path can carry several)
==================================
    rust_src      crates/<c>/{src/**,build.rs,Cargo.toml,tests/{common,support}/**}
    rust_test     crates/<c>/tests/<stem>.rs, crates/<c>/tests/<stem>/**
    crate_file    every other file of crates/<c>/ -- schemas/ (`include_str!`),
                  examples/, benches/, README.md, migrations/, templates/, a
                  toml beside Cargo.toml, build.rs inputs (rule 10 of
                  `test_filter`)
    workspace     Cargo.toml, Cargo.lock, .cargo/**, .config/nextest.toml,
                  crates/meclaw-testing/**, deny.toml, rust-toolchain*
    corridor      crates/meclaw-colony/src/colony.rs, .github/fixtures/**,
                  plans/**/expected_*_body.txt, .github/gates/corridor_byte_gates.sh
    template      templates/<name>/**          (templates/README.md -> catalogue)
                  -- selection by REFERENCE shape, see `_template_reference_re`
                     and `_names_the_shipped_root` (GH #713)
    catalogue     templates/README.md
    example       examples/<name>/**
    docs          docs/**, top-level *.md, plans/** (minus corridor/export fixtures)
                  -- "docs-only means no cargo" holds only while no test READS
                  the file -- by name or at runtime (a walk of `docs`, the
                  seed generator); see rule 8 in `test_filter`
    corpus_source see CORPUS_SOURCES below
    evals_memory  workshop/evals/scenarios/**, workshop/fixtures/**memory-hive**,
                  workshop/evals/p5-longmemeval/tools/**
    evals_recall  workshop/evals/p5-longmemeval/*.py -- the harness' own
                  scripts -- and workshop/evals/engine.py, the shared eval
                  engine they call through; plans `recall-harness` only
                  (GH #878, #879)
    evals_builder workshop/evals/builder-scenarios/**
    display_scenarios  templates/display/compose/scenarios/**
                  -- carries `template` as well (the path lies under
                     templates/), so it pulls the template stations with it.
                     The class of its own exists so the station is planned
                     when only the driver changes
    evals_guide   workshop/evals/conversation-guide/**,
                  workshop/fixtures/positive/conversation-guide/**,
                  templates/memory-hive/inline-contract.md,
                  workshop/evals/engine.py, workshop/evals/colony_log.py,
                  scripts/tests/test_colony_log.py,
                  workshop/tools/display-lab/fact_yield.py,
                  scripts/tests/test_replay_871.py,
                  workshop/tools/display-lab/sidecar_turns.py
                  -- the guide harness and what its self-test reads: the
                     fixture edge, the shipped block its drift checks compare,
                     the engine it runs on and the fact counter it records
                     (GH #881). A path may carry this class beside another
    display_browser  workshop/tools/display-*, the Rust locks that run
                  a browser driver of the station (BROWSER_LOCKS), and the
                  engine pin the drivers run on (BROWSER_ENGINE_PINS:
                  workshop/tools/package.json, package-lock.json). Never
                  `template`: the driver lies under workshop/, which does
                  not travel
    display_lab   workshop/tools/display-lab/**, scripts/tests/test_display_lab.py
                  -- the measuring library. Its prefix is the longer one and
                     wins: a tool that READS a screen is not the driver that
                     lays one out, and must not pull `browser:display`
    llm_conformance workshop/tools/llm-conformance/**,
                  scripts/tests/test_llm_conformance.py,
                  crates/meclaw-cells/src/llm/translate.rs
                  -- the provider conformance tool and its offline tests
                     against a fake provider (GH #998). translate.rs carries
                     the class beside `rust_src`: the tool mirrors its
                     SAMPLING_PARAMS, and the drift lock has to run when the
                     cell's list moves
    persona_source see PERSONA_SOURCES below: the prompt literals, seeds, model knobs
                  and versions a persona is made of, plus the engine seam it is measured with
    evals_persona workshop/evals/persona/** -- its committed report included; it is NOT in IGNORED
    gate_infra    scripts/gate.sh, scripts/gate_legacy.sh, scripts/gate_plan.py,
                  scripts/precheck.py, scripts/pipeline/pre.py, scripts/pipeline/locks.py,
                  scripts/tests/**, scripts/test-tier.sh, scripts/wave_retro.py,
                  scripts/retro/**, scripts/display_sync.py, scripts/strand.sh,
                  scripts/wave_receipt.py, scripts/slow_tests.py, scripts/flake_triage.py,
                  scripts/stop_watch.py, scripts/check_required_pins.py,
                  .config/required-pins.toml (the strand kit, the receipt builder, the stop
                  watch of long runs and the required-pin check carry their tests in
                  `gate-selftest`), scripts/pipeline.py,
                  scripts/pipeline/** and workshop/grafana/** (the pipeline core and
                  its dashboard), .config/nextest.toml
                  (the profile that decides what may be retried is gate
                  infrastructure too)
    unwrap_infra  .github/gates/unwrap_budget.{py,txt}, .github/gates/clock-bans/clippy.toml
    export_infra  every path outside FORBIDDEN_PREFIX of make_export.py (the
                  export self-tests read the shipping tree: ROOT_FILES, DOCS_MAP,
                  templates, real-file pins, MUST tests), plans/export-fixtures/**,
                  plans/p5.5-fixtures/german_scan.py and the R16 seed generator
                  workshop/tools/build_librarian_seed.py (V2-A13, GH #1199)
    shell         **/*.sh
    ci            .github/workflows/**

STATIONS (S strand, I integration, R release, C ci)
===================================================
    precheck        the cheap form checks of the diff, FIRST, beside `build`
                    and before every other cargo station (GH #1162):
                    `rustfmt --check` on the changed sources, the
                    sheet's byte ceiling, an ADR's anchor line, a `scripts/`
                    path a test reads without an export entry, a docs page
                    changed without its twin, an eval call to the local model
                    lane without its thinking depth (private tree only) --
                    plus five findings that are notes rather than judgements
                    (`scripts/precheck.py`). The cheap locks of
                    `scripts/pipeline/locks.py` run here as well, the same
                    functions `strand.sh pre` calls (W4, GH #1118):
                      pin-lock / scenario-pin-lock  RED: a `"<template>@<x.y.z>"`
                        pin no template provides, with path:line (GH #1108)
                      version-lock  RED in a strand diff: a template or
                        workspace version bump (assigned at the merge point)
                      test-line-lock  NOTE: a promise changed without a test
                      set-drift-lock RED / diff-size NOTE (> 400 lines)
                      bin-owner-lock  RED: not exactly one `meclaw` binary,
                        or a recipe (release.yml, install.sh) that builds
                        another crate -- on any diff at a Cargo.toml,
                        src/main.rs, src/bin/ or the two recipes, TOML and
                        text only (GH #1168; it planned `gate-selftest`
                        before, ~326 s for every manifest diff)
                    A RED stops the cargo stations: the runner ends a
                    running `build` or `build:browser`, a running
                    `browser:display` -- it starts on phase 1 without
                    waiting for precheck (GH #1162 Weg d) -- and in R a
                    running `export-tree-tests` or `export-audit`, which
                    start beside precheck as well (cargo 0, executor
                    `PRECHECK_STOP_ALSO`) (SKIP `precheck red`), skips every
                    station that needs a build, and runs the cheap rest on
                    (`gate.sh --no-precheck-stop` overrides).
                    Never in C: the published tree has no `plans/`, so three
                    of the checks have no input there, and ci's diff is often
                    the whole tree, where the artefact notes grade files
                    nobody changed. An empty diff plans nothing to check
    roadmap-anchors adr-anchors claims tree-rules   always
                    (`adr-anchors` runs `scripts/check_required_pins.py` as its
                    second command: the required-pin table against its
                    markers and the tree, GH #1199 -- rule 12 of `test_filter`)
    test-bans-inventory  I/R/C always: `scripts/testing_bans.py --inventory
                    --check` counts every testing ban over the whole tree;
                    NOTE with the count, RED when a ban's count rose over
                    `.config/testing-bans.baseline.json` (VORGABEN § 5.4,
                    R-TV-W8: lowered once at the wave end, never per strand).
                    A strand's added lines are graded in `precheck`
                    (`test-bans`), not here. In C the published tree has no
                    `workshop/`, so its counts sit at or under the baseline:
                    a rise in `crates/` shows in full, one in `scripts/tests/`
                    only beyond the share `workshop/evals/` holds locally
    test-bans       C only, always: the two testing checks of `precheck`
                    (`test-bans`, `testing-rules`) by themselves, because
                    precheck never runs in C -- `scripts/testing_bans.py
                    --files` with the diff's `.rs`/`.py`/`.mjs` files and the
                    files the change reservation reads (`test_bans_paths`).
                    The base is the module's own `merge_base`: on a pull
                    request `origin/main` (the gate job checks out the full
                    history -- a shallow clone has no base, and the change
                    reservation fails closed RED), on a push to main HEAD
                    itself, so an export grades nothing its strands' precheck
                    and the owner rulings of its passes did not already grade
    corpus-committed  the seed corpus against the tree AS IT IS, before
                    anything regenerates it. NEVER in C, for the same reason
                    as `corpus`. NOTE in S (the strand still owes the commit),
                    RED in I/R (a committed corpus that lies is a finding)
    corpus          corpus_source -> `regenerate+check+librarian`, else
                    `check+librarian`; NEVER in C. Two checks, not one: the
                    seed corpus AND `build_librarian.py` (the R11 pair)
    catalogue       template/catalogue class; I/R always
    shellcheck      shell/gate_infra; C always
    gate-selftest   gate_infra; C always (resolver, runner, strand
                    kit, wave retro, merge driver, form station, the source
                    mark of the display scenarios, the quarantine grammar of
                    nextest, the roadmap anchor gate and the tests of the
                    `meclaw` binary owner lock -- the lock itself runs in
                    `precheck`, a manifest diff plans no selftest)
    display-lab     display_lab (workshop/tools/display-lab/** and its test);
                    I/R always; never in C -- workshop/ does not travel
    llm-conformance llm_conformance (workshop/tools/llm-conformance/**, its
                    test, the cell's translate.rs); I/R always; never in C --
                    workshop/ does not travel
    guide-selftest  evals_guide (run_guide.py --self-test, the #871 replay's
                    unittest and the log pager's with the Python pin "one page
                    is not the log"); I/R always; never in C -- workshop/ does
                    not travel
    fmt             rust_src/rust_test/crate_file/workspace
    clippy          rust_src/rust_test/crate_file/workspace  (-p <crates> in S,
                    else --workspace)
    unwrap-budget   rust_src/crate_file/workspace/unwrap_infra; C always (cargo: it
                    shells out to `cargo clippy --workspace`)
    corridor        corridor; C always
    tests           see `test_filter`; planned but not run in C (shards run it);
                    the full suite in R whatever the diff (`test_select`, KR-TB-1 d).
                    Outside C never a browser unit (rule 11, `browser:display`).
                    An empty diff plans the t0 TIER, not the equivalent filter
                    expression -- see `T0_FLOOR`
    slow-tests      right after `tests`, wherever `tests` is planned; not run
                    in C (tests do not run there). A test over 1/3 of its
                    nextest budget is a NOTE in every mode and a finding of
                    the wave (TV-K5: a time moves with the host's load, a
                    perf signal, never a verdict); exit 2 (no JUnit, a filter
                    it cannot read, a debt without an issue) is RED in every mode
    doctests        I/R with rust_src/crate_file/workspace
    deny            workspace; R always. NOT in I for every diff (R-P2,
                    2026-09-19): the `workspace` class is what moves the
                    dependency graph it grades
    scenarios:*     see the template/example/evals triggers; I/R always --
                    EXCEPT `scenarios:memory`, which R plans always and I only
                    when the diff carries `evals_memory` or the memory-hive
                    template (R-P2, 2026-09-19: 43 integration runs,
                    16 317 s, no finding -- wave P, finding 04 § 3.4).
                    A station that books colonies (`pipeline/plan.py` NEEDS:
                    memory the rest beside browser:display 3 -- 3 on a lane
                    of 6 colonies, 5 on 8 -- and builder 4) is handed that
                    many slots of the port register by the lane executor
                    (MECLAW_COLONY_SLOTS); the runners take them as
                    `--slots`, one worker per slot (GH #1161).
                    `scenarios:display` forks its four booked threads as
                    `--workers 4` outside C.
                    `scenarios:display` runs the scenarios of display-hive.md
                    § 11 against the real curator AND the reference model
                    against itself (development-rules § 10). It travels: the
                    driver and its scenario file live under templates/, so ci
                    runs it too -- it is NOT in CI_EXCLUDED, and it is planned
                    in I, R and C for EVERY diff, not only when one names it.
                    Second command: the cheap mark check
                    `display_sync.py --check-source`, a NOTE on stdout and
                    never a verdict (the Rust lock is what goes red)
    browser:display the B-proofs of display-hive.md § 5-9 in Chromium AND
                    WebKit (§ 6.7, R-23-10). TWO shapes, one station name:
                    S runs the SHEET half (scope `sheet`) -- a page the driver
                    builds out of `compose.py`'s own parts, ~30 s, no colony;
                    I and R run that AND the COLONY half (scope
                    `sheet+colony`), six tests, one boot each, against three
                    exits in two engines, behind `#[ignore]` and
                    `--run-ignored all`. Two commands SIDE BY SIDE
                    (`parallel` by cmd, GH #1161): one union of every lock and
                    the sheet, split by engine -- every Chromium test in one
                    part, every WebKit test in the other, which runs them one
                    at a time (two WebKit pages never meet, GH #1048); each part
                    has its own nextest profile and half the booked threads.
                    The locks of ALONE_LOCKS (gh1002, GH #1191) leave both
                    parts after every narrowing and run in `browser:alone`,
                    alone on the lane (`pipeline/plan.py` `split_alone`).
                    In EVERY mode it also runs the CSP
                    lock (GH #867): one colony, one page under a strict
                    policy in Chromium, ~30 s, `#[ignore]`d so the `tests`
                    station never builds a browser run into its own budget
                    (the lesson of GH #763), and the motion lock (GH #961):
                    one colony, three pages per engine, the same reasons.
                    And in EVERY mode but C it runs the BROWSER UNITS the
                    diff's test selection reaches (rule 11 of `test_filter`,
                    GH #1161 re-check M-3): a unit that starts a browser --
                    stem `*_browser`, or a driver `display-*-browser.mjs`
                    named in code that starts a process (`browser_unit`) --
                    never runs in `tests`, where nothing booked the engines
                    and an absent one was SKIP-green; reaching one PLANS this
                    station (a `crates/meclaw-cells/src/**` diff does), and
                    the ones outside BROWSER_LOCKS join the union of both
                    parts. An `#[ignore]`d one outside BROWSER_LOCKS, or a
                    shared test module naming a driver, is RED in `precheck`
                    (`browser-station-lock`, `scripts/pipeline/locks.py`).
                    The colony half was release-only
                    until 0.39.0, and the first gate that ever ran it found it
                    red on three proofs (GH #746): a proof only the release
                    night reaches is a proof nobody sees, and the pass that
                    declares a wave done is where it belongs. In CI_EXCLUDED,
                    unlike `scenarios:display`: the driver lives under
                    workshop/, which never travels. A narrowed integration
                    (a wave decision term) keeps only the locks the wave can
                    reach, fail-closed -- touched map cover or lock file,
                    unmeasured lock, unfit map, a non-Rust station input,
                    a workspace file, a crate file outside the map or a
                    lock that spawns a workspace binary keeps them; release
                    and a full pass (`--full`, the mark of pipeline/
                    full_pass.py) run all (owner's ruling 10.10.,
                    pipeline/browser_plan.py)
    recall-harness  memory-hive template, recall sources, evals_memory,
                    evals_recall; R always. Four commands: the recall cases
                    (tools/), the answer-side self-check (GH #878), the
                    offline local-path cases of the eval engine
                    (`workshop/evals/engine.py`, GH #879) and the offline
                    pipeline cases of the ingest (per-turn annotation,
                    sidecar section, close pass, fact yield, overnight;
                    GH #880).
                    NOT in I for every diff (R-P2), same measure as
                    `scenarios:memory` and one trigger more: a recall source
                    under `crates/meclaw-cells/src/` reaches the harness and
                    not the scenario cases
    deny-advisories R always (the runner grades it NOTE, never RED)
    crate-isolation R always (#1166 re-check M-1): `cargo check -p <member>
                    --all-targets`, one call per workspace member, so each
                    crate is checked with ITS OWN feature set. The build
                    station merges `-p` sets (S6), and a merged set resolves
                    the shared dependencies with the union of the members'
                    features (S6-DIAG: colony beside cells gets tokio
                    `process`, reqwest `rustls-tls`, rusqlite `functions`), so
                    a crate that compiles only thanks to a sibling's feature
                    is seen nowhere else. Members are cargo's
                    (`workspace_members`: globs, `exclude`, a root
                    [package], path dependencies under the root); a list
                    the model cannot resolve plans scope `refused` and one
                    command that exits 1 naming `<manifest>:1: <cause>`, a
                    red call names its `-p` (fail-closed, re-check m-4/m-5).
                    Runs after `build` on 4 threads, beside
                    the stations without cargo; every variant
                    lands in the shared target/ next to the others. The
                    night run (S1b) plans it as well once it exists
    persona-cases   evals_persona; I/R always; never in C (workshop/ does not
                    travel). Free and offline: its own composition self-test,
                    then every part that is BUILT -- the privacy sweep and its
                    self-test, a re-grade of the COMMITTED transcripts, the
                    runner's --self-test, the fingerprint's self-test. A part
                    whose module and data are both absent is "not built yet"
    persona-receipt persona_source/evals_persona; I/R always; never in C.
                    `fingerprint.py --check` recomputes the persona fingerprint
                    from the tree and holds it against the committed report
                    (ADR-0029). NOTE in S, whatever it finds. In I/R: GREEN when
                    the report covers the tree and is green, RED when it covers
                    it and is red or when the checker breaks -- and when the
                    report is stale, incomplete or absent the pass does not turn
                    red, it ASKS the owner (ruling on GH #621, 2026-09-22; see
                    `gate.sh --decide`). No station talks to a provider
    export-selftest export_infra; I/R always (seconds, pure Python). Two
                    commands: the drift fixtures and the lock of
                    `export-tree-tests` (`test_export_tree_tests.py`)
    export-tree-tests  R always, right before `export-audit` (GH #1101); S gate_infra
                    or make_export (the kit half only, see below).
                    Builds the export tree of the gated tree dry (the index of
                    `make_export.py`), selects IN IT every integration test
                    that reads the shipped catalogue (`_is_catalogue_wide` or
                    `_names_the_shipped_root` -- this module's rules, no hand
                    list) and runs them with nextest in a fresh target, one
                    call per crate. cargo:1. A test green in the private tree
                    and red in the export -- a count or a floor over the
                    templates that ship (0.62.1) -- is RED here instead of in
                    the public CI. Second command `--kit` (GH #1199): the
                    export tree's own `gate.sh ci --only` over every station
                    of its ci plan without cargo (gate-selftest and the other
                    kit stations) -- a kit self-test that reads a file the
                    export does not carry is RED here, not in the public CI.
                    S on a diff at a kit input -- `gate_infra` (the kit and
                    its tests, `scripts/tests/**` included) or `make_export.py`
                    (what travels) -- the `--kit` command alone, cargo 0
                    (GH #1199, release 0.64.2 run 4: four kit self-tests
                    were green in the strand and merge gate and red in the
                    export tree; the release was the first gate that ran
                    them there; re-check RC-KX: a diff at gate_plan.py
                    alone turns unchanged kit tests red there). Every
                    command exports `--worktree`, HEAD plus the overlay:
                    the tree the gate gates and the cache keys the station
                    on (`--rev HEAD` judged the base of an uncommitted diff
                    and the cache took that GREEN over for the commit).
                    Never in C: plans/ does not travel, and the
                    public tree IS the export tree
    export-audit    I/R always, after the four stations whose receipt
                    verdicts it reads (`plan.AUDIT_READS`, GH #1162 -- no
                    longer last). TWO shapes, one station name:
                    R runs the FULL audit (scope `R1-R17`, cargo:1 here --
                    make_export.py runs `cargo check --workspace
                    --all-targets` in a target of its own, so the pipeline
                    books it no share of the pass's lock); I runs it DRY
                    (`--skip-cargo --rev HEAD`, scope `R1-R17 dry`, cargo:0
                    -- it builds nothing). `--rev HEAD` because the pass
                    judges the tree its receipt was written over; the release
                    audit keeps the `master` default. The dry run is seconds
                    and surfaces the cheap
                    findings of R2b/R2c/R5/R10 -- dead template references
                    in tests, tests reaching into a never-exported root,
                    name/domain patterns, relative links -- in the
                    integration pass instead of an hour later in the release.
                    S (V2-A18, GH #1199) on a diff at an exported test source
                    (`make_export._is_exported_test`: `crates/*/tests/**`,
                    `*_test.rs`), at `templates/**` or at `make_export.py`:
                    the dry audit over the gated tree (`--worktree`, HEAD plus
                    the overlay) with `--no-gate-receipt` -- a strand gate has
                    no I/R receipt, so R11/R12/R14/R17 are dropped (reported
                    as not run), every other rule judges; `--parent HEAD`, so
                    no stale or missing `github-main` (ls-remote check) can
                    turn it red; scope `R1-R17 dry,
                    no gate receipt`, cargo:0. The merge gate runs mode S and
                    plans it from its own diff

USAGE
=====
    gate_plan.py --mode {strand,integration,release,ci}
                 (--files-from FILE | --files PATH...) [--repo DIR]
                 [--format {json,tsv}]
    gate_plan.py --print scenario|ignored|persona-sources|env-stations|lane-local|lane-env
                 [--repo DIR]

`--files-from -` reads paths from stdin, one per line.

On a build host (`gate.sh --lane`, GH #942) every station runs, the env
stations against the secret-free `.env` of `--print lane-env`; `--print
lane-local`, what a lane leaves to the owner's machine, is empty.

JSON:  {"mode", "classes": [...], "crates": [...], "stations": [...]}
       A station carries one `cmd` (argv list) or several `cmds`; a command
       that needs a working directory carries `cwd` (repo-relative). PATHS IN
       AN ARGV ARE RELATIVE TO THAT `cwd`, not to the repo root -- the runner
       `cd`s into it before it execs. The `tests` station carries
       `"run": false` in ci mode.
TSV:   EXACTLY FIVE tab-separated columns on every row, in this order:

           name <TAB> scope <TAB> cargo(0|1) <TAB> shell-quoted cmd <TAB> cwd

       This five-column shape is the contract with the runner. The fifth
       column is EMPTY, not absent, when the command needs no working
       directory -- so a shell reader must name five variables:

           while IFS=$'\t' read -r name scope cargo cmd cwd; do ...; done

       Four variables would swallow the cwd into `cmd` on every station that
       carries one, and `read` would leave `cwd` unset on every station that
       does not. One line per command: a station with several commands emits
       several lines under the same name. A row with a non-empty fifth column
       carries an argv whose paths are relative to that directory.

       The TSV has no room for the `run` flag, so it says what to RUN. In ci
       mode -- the one mode where a planned station is deliberately not run --
       read the JSON, which carries `"run": false`.

--explain (JSON only; W3, GH #1117 -- additive, without it the JSON is byte-
       for-byte what it was, and the TSV never changes):
       every station gains `reason`: why it is planned, as `<label>: <path>`
       entries -- the label is a class of the path (`rust_src`, `docs`, ...)
       or one of `diff` (any path), `template:<name>`, `example:<name>`,
       `recall_src`, `make_export` (`plans/export-fixtures/make_export.py`),
       `read` (rule 8 of
       `test_filter`); a station the MODE
       plans says `mode: <mode>`, one every mode plans says `always`,
       `slow-tests` says `station: tests`, the empty-diff floor `floor: empty
       diff`. At most `REASON_CAP` path entries, then `+<n> more`. The table
       behind it is `STATION_TRIGGERS`; a test holds it to `plan()`.
       `tests` and `browser:display` also gain `select`, the structured
       selection the filterset is rendered from: `{binaries: ["c::stem"],
       rdeps: ["c"], all, floor, extra: ["c::stem"], minus: [<binary-name
       regex>], filter}` -- `minus` is the scenario class subtracted, `extra`
       the browser-lock binaries the station drives. A consumer reads these
       fields and never parses `filter`.
"""

import argparse
import collections
import fnmatch
import glob as globmod
import hashlib
import itertools
import json
import os
import pickle
import posixpath
import re
import shlex
import subprocess
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

MODES = ("strand", "integration", "release", "ci")

# The scenario class. This is the ONLY copy: `scripts/test-tier.sh` no longer
# spells it out, it asks for it with `gate_plan.py --print scenario`. Change it
# here and the tiers and the per-diff selection move together.
# The `*_live` binaries are here for a second reason beside cost: they open a
# billed session against a third party, so they never ride a diff.
# `scripts/tests/test_gate_plan.py` locks the suffix rule against the tree.
SCENARIO_TERMS = (
    "binary(/_demo$/)", "binary(/_demo_/)", "binary(/e2e/)",
    "binary(/^workshop_scenario$/)", "binary(/^slack_live$/)",
    "binary(/^gpt_live_live$/)",
    "binary(/^harness_real_cli_smoke$/)", "binary(/^audit_14_/)",
)
SCENARIO = " + ".join(SCENARIO_TERMS)

_TERM_RE = re.compile(r"^binary\(/(.*)/\)$")
_BINARY_TERM_RE = re.compile(r"binary\(/([^/]*)/\)")


def _test_targets(repo):
    """The names of the test binaries cargo autodiscovers in the tree.

    `crates/*/tests/<stem>.rs` and `crates/*/tests/<stem>/main.rs` -- the same
    shape `_test_binary_exists` reads. Nothing here compiles anything.
    """
    root = REPO_ROOT if repo is None else repo
    names = set()
    for path in globmod.glob(os.path.join(root, "crates", "*", "tests", "*.rs")):
        names.add(os.path.splitext(os.path.basename(path))[0])
    for path in globmod.glob(os.path.join(root, "crates", "*", "tests", "*", "main.rs")):
        names.add(os.path.basename(os.path.dirname(path)))
    return names


def scenario_for(repo=None):
    """The scenario class, narrowed to the operators the tree can match.

    nextest refuses a filterset whose `binary(...)` operator matches no binary
    ("operator didn't match any binary IDs") -- the same refusal
    `_test_binary_exists` guards against -- and that took the whole `tests`
    station down in a public clone, which has no `slack_live.rs` (GH #934,
    measured on a build host 2026-10-01). So every term of the class is kept
    only when a test target of the tree matches it. The real tree matches all
    of them; `scripts/tests/test_gate_plan.py` locks both directions.
    """
    return " + ".join(_scenario_terms(repo))


def _scenario_terms(repo=None):
    """The terms of `scenario_for`, as a list (the `--explain` selection)."""
    names = _test_targets(repo)
    kept = []
    for term in SCENARIO_TERMS:
        # one binary operator only: a compound is kept unread (tb-s10 D)
        m = _ONE_BINARY_RE.match(term)
        rx = re.compile(m.group(1)) if m else None
        if rx is None or any(rx.search(n) for n in names):
            kept.append(term)
    return kept


# KR-TB-1 (d), AGENTS.md t2: the release is the full suite. It subtracted the
# scenario class from `tests` (cost: a release must not pay for it twice) and
# planned it nowhere else -- 8 terms, 57 units no release ran since before the
# wave (OR-TB.S4.55, `release --plan-only` on 28c948e15). They run in a station
# of their own now, release only, on the nextest profile `scenario` (its own
# JUnit file: `test-tier.sh` clears the file of its profile before every run,
# and `slow-tests` reads `default`'s). No `--run-ignored`: the live units are
# `#[ignore]`d and skip there exactly as in the CI shards -- no new exception.
SCENARIO_STATION = "scenario-tests"


def scenario_station_terms(repo=None):
    """The terms the release's scenario station runs: the scenario class."""
    return _scenario_terms(repo)


def scenario_binaries(repo=None):
    """`["<crate>::<binary>"]` the scenario class selects in the tree, sorted."""
    rxs = [re.compile(m.group(1))
           for m in map(_ONE_BINARY_RE.match, scenario_station_terms(repo)) if m]
    return sorted({"%s::%s" % (c, u.split("/")[0]) for c, u, _t in _test_sources(repo)
                   if any(rx.search(u.split("/")[0]) for rx in rxs)})


# The stations whose runner reads `<repo>/.env` (the scenario runners copy
# keys out of it into every colony they boot; the guide harness runs the
# scenario engine). A build host never gets the owner's `.env` -- it gets the
# secret-free one below (GH #942). Published with `--print env-stations`.
ENV_STATIONS = ("scenarios:memory", "scenarios:builder", "guide-selftest")

# What a lane leaves to the owner's machine: NOTHING (GH #942). It used to
# list the stations above (no `.env` on a host), `display-lab` (no `node`)
# and `export-audit` (no `github-main` ref), and `strand.sh gate --host` ran
# them here after the remote run -- the scenario suites on this machine's
# binary, the release audit with a cargo build of its own. The owner's
# machine runs live and lab colonies, and a lab colony died at load 15 while
# strands built beside it (2026-10-02). So a lane now runs every station:
# the env stations get the `.env` below, `display-lab` finds `node` on the
# host (or is RED there), the kit pushes `github-main` along. The mechanism
# stays -- `gate.sh --lane` reports a listed station as `SKIP <reason>
# (lane)` and the kit runs it here -- with an empty list. Published with
# `--print lane-local` (`<name>\t<reason>` per line).
LANE_LOCAL = {}

# The `.env` of a lane: the names the suites read out of `.env`, each with a
# constant placeholder, and not one secret VALUE -- a build host is another
# machine, and the suites run only their keyless class `none` there
# (`gate.sh --lane` refuses a `--with`). Measured which names the three env
# stations really read (GH #942, L-0): `run_scenarios.py` copies the model
# names, the embedding model and dimension, the reasoning levels below and the
# provider key; `run_builder_scenarios.py` resolves `${MODEL_BUILDER}` and
# `${OPENROUTER_API_KEY}` in its cases (and declares every other bare name of
# the library empty on its own); the guide self-test reads no `.env` at all.
# The provider key NAME stands here because a colony with an `llm` cell
# resolves it at boot, in class `none` too: without it a lane ended
# `scenarios:memory` with 38 of 46 cases on `EnvVarMissing` (GH #942). Its
# value is the constant `lane-stub`, never a credential: a keyless case never
# calls a provider (its embedder points at a dead loopback port, its judge
# switch is off), and a paid class never runs on a lane. This table is the
# contract -- a suite that needs another name adds it HERE with a constant,
# and no value ever comes from a file: `--print lane-env` prints the
# constants. Reasoning `low` and dimension 8: harmless values a config parser
# accepts, the same on every host.
LANE_ENV = {
    "MODEL_EXTRACTOR": "lane-stub",
    "MODEL_DIALECTIC": "lane-stub",
    "MODEL_DREAMER": "lane-stub",
    "MODEL_JUDGE": "lane-stub",
    "MODEL_CLOSER": "lane-stub",
    "MODEL_BUILDER": "lane-stub",
    "MEMORY_EMBED_MODEL": "lane-stub",
    "MEMORY_EMBED_DIM": "8",
    "MEMORY_REASONING_EXTRACT": "low",
    "MEMORY_REASONING_DIALECTIC": "low",
    "MEMORY_REASONING_DREAM": "low",
    "MEMORY_REASONING_JUDGE": "low",
    "MEMORY_REASONING_CLOSE": "low",
    "OPENROUTER_API_KEY": "lane-stub",
}

# Run artefacts of the two suites. They are committed, they change on every
# run, and they never justify a station -- so they carry no diff class at all.
# `gate_plan.py --print ignored` publishes the list, and `scripts/gate.sh`
# subtracts it before it calls a tree dirty in the receipt: the scenario
# stations REWRITE these files while the run is in progress, so without the
# subtraction every release gate ends `dirty: true` and the export refuses.
#
# `workshop/evals/persona/last_run.json` is deliberately NOT here (L-12): it is
# not rewritten by a station -- the persona run is an artefact nobody's gate
# performs -- and it IS the measurement `persona-receipt` holds the tree
# against. Dropping it from the diff would let a new report travel without the
# station that reads it.
IGNORED = (
    "workshop/evals/scenarios/last_run.json",
    "workshop/evals/builder-scenarios/last_run.json",
)

# A SUPERSET of the source globs of `workshop/tools/build_librarian_seed.py`,
# not a mirror of them. Only the direction that matters is enforced: a source
# that feeds the librarian seed and is missing here would let a corpus edit
# travel without a regenerate, which is the one failure mode the seed gate
# exists against -- and that is all the drift test checks (coverage, not
# equality). The other direction is deliberate over-selection: the README
# entries feed `build_librarian.py`, the second half of the corpus station,
# rather than the seed generator, and an extra `regenerate` costs a second.
CORPUS_SOURCES = (
    "docs/meclaw-overview.en.md",
    "docs/cell-types.en.md",
    "docs/config.en.md",
    "docs/rewiring.en.md",
    "README.md",
    "templates/*/template.json",
    "templates/**/config.json",
    "templates/*/README.md",
    "workshop/cookbook/*.md",
    "workshop/references/README.md",
    "workshop/corpus/*/ITEM.md",
    "examples/organism/grow-*.json",
    "workshop/fixtures/negative/*/expected_error.json",
)

# What a persona is made of, for the persona gate (ADR-0029, GH #882): every
# file whose bytes can change what the measured models read, which tools they
# get, whether and where the sidecar writes -- also as the override of a ref
# marker. A SUPERSET, same discipline as CORPUS_SOURCES: only the direction
# that matters is enforced (a source missing here lets a prompt edit travel
# under a report that no longer covers it); over-selection costs one
# re-measurement. `_glob_re` knows `**`, `*` and `?` and nothing else -- no
# braces, no classes -- so every alternative is written out.
# `fingerprint.py` asks for the expanded list with `--print persona-sources`:
# one semantics for the class and the list.
PERSONA_SOURCES = (
    # memory-hive, taken WHOLE: every cell carries model text or a model knob.
    "templates/memory-hive/config.json",          # hive root: the edges between its cells
    "templates/memory-hive/inline-contract.md",   # the authority behind MEMORY_RULES
    "templates/memory-hive/predicate-core.json",
    "templates/memory-hive/*/config.json",
    "templates/memory-hive/store/seed/*.jsonl",   # the embedding identity
    "templates/memory-hive/template.json",
    # talky, cogny and collector, taken WHOLE as well: root, every sub-config
    # (a ref marker's override_params moves the menu, the sidecar switch, the
    # window -- talky/collector sets assemble.tools and assemble.sidecar) and
    # the version. Single-level `*`: none of the three nests deeper since
    # talky's brain seed went with GH #889 (it carried only the recall tool).
    "templates/collector/config.json",
    "templates/collector/*/config.json",          # assemble (system_order, menu, preamble), window
    "templates/collector/template.json",
    "templates/talky/config.json",                # the route that carries the sidecar
    "templates/talky/*/config.json",              # brain, splitter, schemas, collector, dispatcher, ...
    "templates/talky/template.json",
    # the curator, taken WHOLE (GH #888/#889): it decides what of the wall a
    # model reads -- intake's PACK_SLOTS, the policy's budgets, the rebuild --
    # and its summarizer cell carries a prompt and a model of its own. The
    # ledger seed is the durable state a rebuild starts from.
    "templates/curator/config.json",
    "templates/curator/*/config.json",            # intake, policy, summarizer, writer, ledger, clock, history
    "templates/curator/ledger/seed/*.jsonl",
    "templates/curator/template.json",
    "templates/cogny/config.json",
    "templates/cogny/*/config.json",
    "templates/cogny/template.json",
    "templates/affinity/brief/config.json",       # the identity brief
    "templates/llm-registry/**",                  # the model registry
    "templates/meclaw-os/config.json",
    "templates/egon/persona/config.json",         # the two persona-bearing templates
    "templates/slack-agent/persona/config.json",
    # How a persona is MEASURED: the one engine seam (local/paid, reasoning depth,
    # price) every eval runner imports. A path may carry several classes.
    "workshop/evals/engine.py",
)

# The shell sources shellcheck reads. CI runs on the published export mirror,
# which has neither `plans/` nor `workshop/`, so only the first
# `SHELL_GLOBS_PUBLIC` patterns are used there. `scripts/pipeline/*.sh` (the
# lane fingerprint, W1) travels and is checked in CI as well.
SHELL_GLOBS = ("scripts/*.sh", ".github/gates/*.sh", "scripts/pipeline/*.sh",
               "plans/meclaw-os/*.sh", "workshop/tools/display-lab/*.sh")
SHELL_GLOBS_PUBLIC = 3

# Stations that are never planned in ci mode. ci runs in the published tree:
# no workshop/, no plans/, and cargo-deny runs in its own job. Planning any of
# these there produces a red station or a silent duplicate run (GH #234).
CI_EXCLUDED = frozenset({
    # The form station reads what the published tree does not carry, and ci's
    # diff is often `git ls-files` -- see its entry in the docstring.
    "precheck",
    "corpus",            # workshop/tools/build_librarian*.py
    "corpus-committed",  # ... and its pre-check reads the same builder
    "scenarios:memory",  # workshop/evals/scenarios/
    "scenarios:builder",  # workshop/evals/builder-scenarios/
    "recall-harness",    # workshop/evals/p5-longmemeval/
    "guide-selftest",    # workshop/evals/conversation-guide/
    "browser:display",   # workshop/tools/display-layout-browser.mjs
    "display-lab",       # workshop/tools/display-lab/
    "llm-conformance",   # workshop/tools/llm-conformance/
    "persona-cases",     # workshop/evals/persona/
    "persona-receipt",   # workshop/evals/persona/
    "export-selftest",   # plans/export-fixtures/
    "export-tree-tests",  # plans/export-fixtures/
    "export-audit",      # plans/export-fixtures/
    "deny",              # the CI `deny` job runs the cargo-deny action itself
    "deny-advisories",   # ... and would double-run or skip here
})

# The order stations are reported in: cheap first, so a red anchor costs
# seconds instead of a full cargo build; the release audit is last because it
# reads the tree every earlier station just proved.
STATION_ORDER = (
    "precheck",
    "roadmap-anchors", "adr-anchors", "claims", "tree-rules", "test-bans-inventory",
    "test-bans", "corpus-committed", "corpus", "catalogue", "shellcheck", "gate-selftest",
    "display-lab", "llm-conformance", "guide-selftest",
    "fmt", "clippy", "unwrap-budget", "corridor",
    "tests", "slow-tests", SCENARIO_STATION, "doctests", "deny",
    "scenarios:memory", "scenarios:builder", "scenarios:display",
    "browser:display", "recall-harness",
    "deny-advisories", "crate-isolation", "persona-cases", "persona-receipt",
    "export-selftest", "export-tree-tests", "export-audit",
)


# The Rust locks the station `browser:display` runs. They are named here and
# nowhere else: `gate_plan.py` is THE place a station trigger may be written down,
# and a test that changes the way a browser proof is driven must plan the station
# that drives it -- the `tests` filter would build the binary without ever saying
# which station owns it. The third is the CSP lock of GH #867: a display page under
# `script-src 'self'` plus the hashes `templates/display/csp.json` publishes.
BROWSER_LOCKS = frozenset({
    "crates/meclaw-cells/tests/710_the_sheet_holds_in_both_engines_browser.rs",
    "crates/meclaw-cells/tests/710_the_colony_holds_in_both_engines_browser.rs",
    "crates/meclaw-cells/tests/gh867_a_display_runs_under_a_strict_csp_browser.rs",
    # GH #961: motion by target state, one colony, three pages per engine.
    "crates/meclaw-cells/tests/gh961_motion_by_target_state_browser.rs",
    # GH #1002: a large page connects on Slow 3G and Fast 3G + CPU 4x.
    "crates/meclaw-cells/tests/gh1002_slow_3g_connects_browser.rs",
    # GH #1003: a turn of the phone sets `data-orientation` and sends nothing.
    "crates/meclaw-cells/tests/gh1003_the_shell_marks_orientation_browser.rs",
    # GH #1028: a new window that ranks first moves none of the others in the DOM.
    "crates/meclaw-cells/tests/gh1028_a_new_window_keeps_the_others_in_place_browser.rs",
})

# The engine the drivers run on: the one npm dependency of the tree (Playwright)
# and its lock. Class `display_browser` -- the whole station runs on the new pin.
BROWSER_ENGINE_PINS = frozenset({
    "workshop/tools/package.json",
    "workshop/tools/package-lock.json",
})

# The `#[ignore]`d browser locks of the station, one nextest term each. They run
# as ONE nextest call over the union of their binaries (W7, `browser_cmds`), so
# nextest's own test groups decide how many run at once (`.config/nextest.toml`:
# `webkit` one at a time -- software rendering takes every core --, `colony-boot`
# four at a time) instead of seven calls in a row.
#
# The CSP lock (GH #867): a display page under `script-src 'self'`. `#[ignore]`d in
# the tree, like the colony half, so the `tests` station never runs a browser;
# unlike the colony half it costs one boot and one page, so a strand pays for it too.
CSP_LOCK = "binary(/gh867_a_display_runs_under_a_strict_csp/)"

# The motion lock (GH #961): blocks glide in, 200 marks move in one diff, one test
# per engine -- one boot and three pages each, so every mode that plans the station
# runs it.
MOTION_LOCK = "binary(/gh961_motion_by_target_state/)"

# The slow-link lock (GH #1002): one `web` cell with a 4 400-object page, loaded cold
# in Chromium on Slow 3G and on Fast 3G with the CPU slowed 4x -- two page loads,
# about 45 s of browser time, so every mode that plans the station runs it.
SLOW3G_LOCK = "binary(/gh1002_slow_3g_connects/)"

# The orientation lock (GH #1003): one small `web` cell, one page in Chromium as a
# phone, turned once -- a few seconds, so every mode that plans the station runs it.
ORIENTATION_LOCK = "binary(/gh1003_the_shell_marks_orientation/)"

# The keep lock (GH #1028): four windows, the last one ranking first, one test per
# engine -- one boot and one page each, so every mode that plans the station runs it.
KEEP_LOCK = "binary(/gh1028_a_new_window_keeps_the_others_in_place/)"

# The ignored locks every mode that plans the station runs, in this order.
IGNORED_LOCKS = (CSP_LOCK, MOTION_LOCK, SLOW3G_LOCK, ORIENTATION_LOCK, KEEP_LOCK)

# The locks that measure a bound in wall time under a slowed CPU, and so run with
# nothing beside them on the lane (GH #1191): gh1002 asserts 6 s to `phx-connected`
# on Fast 3G with the CPU slowed 4x, and in the shared Chromium part, beside `tests`
# and the WebKit part, it measured the lane's load instead -- 6 of 10 isolated runs
# green on build04 (welle-tb S1e, gate bd490c7ec). This station still plans them;
# `pipeline/plan.py` (`split_alone`) moves them, after every narrowing, out of the
# shared parts into the station `browser:alone`, which books the lane exclusively
# (`executor._needs`). The assert, the bound and the throttle stay as they are.
ALONE_LOCKS = (SLOW3G_LOCK,)

# The colony half of the B-proofs: six tests, one boot and one page each (three
# exits in two engines). Only the modes that declare work finished plan it.
COLONY_LOCK = "binary(/710_the_colony_holds/)"

# The sheet half: not ignored; it rides the same two parts as the ignored locks.
SHEET_LOCK = "binary(/710_the_sheet_holds/)"

# The sheet alone: the call a narrowed integration keeps when only the sheet's
# lock is left (`browser_cmds(keep=...)`, pipeline/browser_plan.py), with the same
# `--run-ignored all` as in the union.
SHEET_LOCK_CMD = ["scripts/test-tier.sh", "filter", SHEET_LOCK, "--run-ignored", "all"]

# The WebKit tests of the station's binaries: every one of them names its engine
# (`*_in_webkit*`), and `.config/nextest.toml` puts exactly these in the `webkit`
# group, one at a time (GH #1048).
WEBKIT_TESTS = "test(/webkit/)"

# One nextest profile per part: two calls on one profile delete each other's JUnit
# file (`scripts/test-tier.sh` clears it before every run).
BROWSER_PROFILES = ("browser-chromium", "browser-webkit")


def browser_cmds(colony, keep=None, moved=()):
    """The two parts of `browser:display`: every Chromium test, every WebKit test.

    `moved` (GH #1161, re-check M-3): the browser units outside BROWSER_LOCKS the
    diff reaches (`browser_moved`) -- they join the same union and the same split.

    The station booked one Chromium and one WebKit side by side and ran its two calls
    (the ignored locks, then the sheet) one after the other, every WebKit test holding
    the whole call: 469.6 s in the integration pass of 2026-10-10 (build01), over the
    466.3 s target on its own -- WebKit alone summed 327 s of it, Chromium 328 s at up
    to four at a time (GH #1161). Now ONE union of every lock and the sheet, split by
    engine into two parts that run side by side (`parallel` by cmd, each part at half
    the booked threads): every test is in exactly one part, and every WebKit test is in
    the one part that runs them one at a time -- two WebKit pages side by side starved
    each other (GH #1048), so they never meet. `--run-ignored all` on both: the locks
    are `#[ignore]`d, the sheet is not.

    `keep` (lock paths, the integration rule of `pipeline/browser_plan.py`) drops
    every lock term not in it, never a `moved` unit (the diff reaches it); None
    keeps all. What a narrowed plan keeps runs as ONE call over the kept union
    instead of the two engine parts: a part of a narrowed union can match no test
    at all (gh867, gh1002 and gh1003 name no WebKit test), and a filter without a
    test is RED (`test-tier.sh`, exit 6). In the one call nextest's `webkit` group
    still runs the WebKit tests one at a time (GH #1048). Nothing left: no call."""
    locks = ([COLONY_LOCK] if colony else []) + list(IGNORED_LOCKS) + [SHEET_LOCK]
    terms = locks + sorted(moved)
    if keep is not None:
        stems = [l.rsplit("/", 1)[1].removesuffix(".rs") for l in keep]

        def kept(term):
            # a form that is no single binary operator stays (tb-s10 D)
            m = _ONE_BINARY_RE.match(term)
            return not m or any(re.search(m.group(1), s) for s in stems)
        narrowed = [t for t in locks if kept(t)]
        if narrowed != locks:
            narrowed += sorted(moved)
            return ([["scripts/test-tier.sh", "filter", " + ".join(narrowed), "--run-ignored", "all"]]
                    if narrowed else [])
    union = " + ".join(terms)
    return [["scripts/test-tier.sh", "filter", "(%s) - %s" % (union, WEBKIT_TESTS),
             "--run-ignored", "all"],
            ["scripts/test-tier.sh", "filter", "(%s) & %s" % (union, WEBKIT_TESTS),
             "--run-ignored", "all"]]


# The workers of `scenarios:display`: its booking in `pipeline/plan.py` (`NEEDS`,
# four threads), held equal by scripts/tests/test_pipeline_lab_width.py.
DISPLAY_SCENARIO_WORKERS = 4


# The empty-diff floor. `plan()` turns it into `scripts/test-tier.sh t0`
# rather than a `filter` run: the tier passes `--lib --bins` and builds only the
# unit targets, while the identical filterset would build every test binary in
# the workspace before selecting from it.
T0_FLOOR = "kind(lib) + kind(bin)"


class GatePlanError(Exception):
    """A diff the resolver refuses to plan for (e.g. a crate name mismatch)."""


Station = collections.namedtuple(
    "Station", "name scope cargo cmds cwds run parallel", defaults=(None, True, None))


def station(name, scope, cargo, cmds, cwds=None, run=True, parallel=None):
    """Build a `Station`, defaulting the per-command cwd list to all-unset.

    `parallel` (V0 § 4, additive): how the executor may split the station,
    e.g. `{"by": "cmd"}` -- its commands run side by side. None: in order."""
    if cwds is None:
        cwds = [None] * len(cmds)
    if len(cwds) != len(cmds):
        # A caller bug inside this module, not a property of the diff.
        raise ValueError("station %s: %d commands but %d cwds"
                         % (name, len(cmds), len(cwds)))
    return Station(name, scope, cargo, cmds, cwds, run, parallel)


# --- path matching ----------------------------------------------------------

def _glob_re(pattern):
    """Compile one glob to a regex. `**` crosses `/`, `*` and `?` do not."""
    out, i = [], 0
    while i < len(pattern):
        ch = pattern[i]
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?")
            i += 3
        elif pattern.startswith("**", i):
            out.append(".*")
            i += 2
        elif ch == "*":
            out.append("[^/]*")
            i += 1
        elif ch == "?":
            out.append("[^/]")
            i += 1
        else:
            out.append(re.escape(ch))
            i += 1
    return re.compile("^" + "".join(out) + "$")


_GLOB_CACHE = {}


def _matches(path, pattern):
    rx = _GLOB_CACHE.get(pattern)
    if rx is None:
        rx = _GLOB_CACHE[pattern] = _glob_re(pattern)
    return rx.match(path) is not None


def _norm(paths):
    """Repo-relative, slash-separated, de-duplicated, run artefacts dropped."""
    out = []
    for p in paths:
        p = p.strip().replace(os.sep, "/")
        # Only a literal `./` prefix is noise. `lstrip("./")` would eat the
        # dot of `.github/` and `.cargo/` and quietly drop two classes.
        while p.startswith("./"):
            p = p[2:]
        if not p or p in IGNORED or p in out:
            continue
        out.append(p)
    return out


# --- classification ---------------------------------------------------------

def _crate_of(path):
    """`crates/<c>/...` -> `<c>`, else None."""
    parts = path.split("/")
    if len(parts) >= 3 and parts[0] == "crates":
        return parts[1]
    return None


GERMAN_SCAN = "plans/p5.5-fixtures/german_scan.py"
MAKE_EXPORT = "plans/export-fixtures/make_export.py"
_EXPORT_RULES = []
_EXPORT_MODULE = []


def _export_module():
    """`make_export.py` of this tree as a module, or None where the tree has
    no plans/ (the export tree, where ci drops the stations anyway). Loaded
    once (~20 ms, pure)."""
    if not _EXPORT_MODULE:
        import importlib.util
        path = os.path.join(REPO_ROOT, MAKE_EXPORT)
        mx = None
        if os.path.isfile(path):
            spec = importlib.util.spec_from_file_location("make_export_rules", path)
            mx = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(mx)
        _EXPORT_MODULE.append(mx)
    return _EXPORT_MODULE[0]


def _export_rules():
    """(FORBIDDEN_PREFIX, seed generator) of `make_export.py` of this tree --
    what never ships and the one private file `build_index` reads (R16). One
    list: loaded, not copied. None where the tree has no plans/."""
    if not _EXPORT_RULES:
        mx = _export_module()
        rules = None
        if mx is not None:
            rules = (mx.FORBIDDEN_PREFIX,
                     os.path.relpath(str(mx.SEED_GENERATOR), str(mx.REPO)).replace(os.sep, "/"))
        _EXPORT_RULES.append(rules)
    return _EXPORT_RULES[0]


def _feeds_the_export_selftest(path):
    """True when the export self-tests read `path` from the real tree: every
    path that may ship (outside FORBIDDEN_PREFIX) and the private inputs they
    load -- the fixtures, the string scan of p5.5, the seed generator."""
    if path.startswith("plans/export-fixtures/") or path == GERMAN_SCAN:
        return True
    rules = _export_rules()
    if rules is None:
        return False
    forbidden, seed_generator = rules
    return path == seed_generator or not forbidden.match(path)


def _is_exported_test(path):
    """The files R2b/R2c/R2e judge: `make_export._is_exported_test` of this
    tree (one rule, loaded); False where the tree has no plans/."""
    mx = _export_module()
    return mx is not None and mx._is_exported_test(path)


def _feeds_the_strand_audit(path):
    """V2-A18: what the cheap export rules judge -- an exported test source,
    anything under `templates/` -- or the rules themselves (`make_export.py`).
    S runs the dry `export-audit` on it."""
    return path == MAKE_EXPORT or path.startswith("templates/") or _is_exported_test(path)


def _feeds_the_kit_stations(path):
    """A kit input: `gate_infra` (the kit and its self-tests, which travel and
    run in the public CI) or `make_export.py` (what travels). S runs the kit
    half of `export-tree-tests` on it."""
    return path == MAKE_EXPORT or "gate_infra" in _classes_of_path(path)


# The strand form of `export-audit` (V2-A18; RC-KX I1, RC-XA I1): dry, no gate
# receipt, the working tree, `--parent HEAD`. `pipeline/plan.py` plans the same
# form when it cannot read the diff (GH #1199, re-check I2).
STRAND_AUDIT = ("python3", MAKE_EXPORT, "--keep-going", "--skip-cargo", "--worktree",
                "--no-gate-receipt", "--parent", "HEAD")


def strand_audit_cmd(files, kit):
    """The strand audit of `files`; `files=None` judges the whole tree.

    `kit`: the same plan carries the kit half of `export-tree-tests`, which runs
    the resolver module (`scripts.tests.test_gate_plan`) in the export tree.
    Only then does the audit drop it (`--no-public-resolver`, 90 of R15's 97 s).
    The module holds real-tree tests over the crates' test units and the
    templates as well, so a test or template diff without a kit station keeps
    it (GH #1199, re-check I1): a switch rides on the station that carries what
    it drops, in the same plan, or not at all."""
    cmd = list(STRAND_AUDIT) + (["--no-public-resolver"] if kit else [])
    if files is None:
        return cmd
    return cmd + ["--diff-paths"] + [p for p in files
                                     if p == MAKE_EXPORT or _is_exported_test(p)]


def _classes_of_path(path):
    """Every class this single path carries."""
    cls = set()
    parts = path.split("/")
    crate = _crate_of(path)

    if crate:
        rest = "/".join(parts[2:])
        if (rest.startswith("src/") or rest == "build.rs" or rest == "Cargo.toml"
                or rest.startswith("tests/common/") or rest.startswith("tests/support/")):
            cls.add("rust_src")
        elif rest.startswith("tests/"):
            cls.add("rust_test")
        elif rest:
            # Rule 10 (welle-tb S1b): every other file of the crate -- an
            # `include_str!` schema (`crates/meclaw-core/schemas/ubf-body.json`),
            # an example, a bench, a README a crate doc includes, a migration, a
            # template or a toml inside the crate, a build.rs input. Neither the
            # rules 2/3 nor the coverage map (`.rs` lines only) can name its test
            # binary, so it plans the crate whole (fail-closed).
            cls.add("crate_file")
        if crate == "meclaw-testing":
            cls.add("workspace")

    if path in ("Cargo.toml", "Cargo.lock", "deny.toml", ".config/nextest.toml"):
        cls.add("workspace")
    if path.startswith(".cargo/") or path.startswith("rust-toolchain"):
        cls.add("workspace")

    # The byte gates are corridor infrastructure: the script that diffs the
    # frozen bodies is as load-bearing as the fixtures it diffs against.
    if (path == "crates/meclaw-colony/src/colony.rs"
            or path.startswith(".github/fixtures/")
            or path == ".github/gates/corridor_byte_gates.sh"
            or (path.startswith("plans/") and _matches(path, "plans/**/expected_*_body.txt"))):
        cls.add("corridor")

    # The unwrap ratchet is its own gate: touching the budget file or the
    # checker must re-run it, and neither is a Rust source. The `scripts/
    # check_*.py` anchors need no such class -- their stations run in every
    # mode anyway.
    # The clock bans (tv-p1b) are read by the ratchet alone: a change there
    # moves its count, so it plans the station and moves its cache key.
    if path in (".github/gates/unwrap_budget.py", ".github/gates/unwrap_budget.txt",
                ".github/gates/clock-bans/clippy.toml"):
        cls.add("unwrap_infra")

    if path == "templates/README.md":
        cls.add("catalogue")
    elif path.startswith("templates/") and len(parts) >= 3:
        cls.add("template")

    if path.startswith("examples/") and len(parts) >= 3:
        cls.add("example")

    if path.startswith("docs/") or (len(parts) == 1 and path.endswith(".md")):
        cls.add("docs")
    if (path.startswith("plans/") and "corridor" not in cls
            and not path.startswith("plans/export-fixtures/")):
        cls.add("docs")

    if any(_matches(path, g) for g in CORPUS_SOURCES):
        cls.add("corpus_source")

    if (path.startswith("workshop/evals/scenarios/")
            or path.startswith("workshop/evals/p5-longmemeval/tools/")
            or (path.startswith("workshop/fixtures/") and "memory-hive" in path)):
        cls.add("evals_memory")
    # The recall harness' own scripts (judge, score, run_eval, the self-check
    # beside them). Their check is `answer_cases.py` in `recall-harness`. NOT
    # `evals_memory`: that would drag `scenarios:memory` (46 cases, ~390 s)
    # into every harness diff for a station that never reads them (GH #878).
    # The shared eval engine (GH #879) is how every harness call is built; its
    # offline check is `local_path_cases.py` in the same station.
    if _matches(path, "workshop/evals/p5-longmemeval/*.py") or path == "workshop/evals/engine.py":
        cls.add("evals_recall")
    if path.startswith("workshop/evals/builder-scenarios/"):
        cls.add("evals_builder")

    # The pins of the display hive travel with the template, so the path also
    # carries `template` above and pulls the template stations. The class of
    # its own is what plans `scenarios:display` when the driver alone changes.
    if path.startswith("templates/display/compose/scenarios/"):
        cls.add("display_scenarios")

    # The measuring library: the one copy of the tools that read a running
    # screen (befund `04-struktur.md` section 8). Its contract is a unittest,
    # and a unittest no station plans is not a lock. The prefix is asked FIRST
    # and the browser prefix below excludes it: the library lies under
    # `workshop/tools/display-`, so without this every one of its files dragged
    # the two Rust locks and the Chromium/WebKit run behind a 5 s unittest.
    lab = (path.startswith("workshop/tools/display-lab/")
           or path == "scripts/tests/test_display_lab.py")
    if lab:
        cls.add("display_lab")

    # The provider conformance tool (GH #998): probes a model at its real
    # endpoint; its contract is a unittest against a fake provider. The tool
    # mirrors the cell's SAMPLING_PARAMS, so the cell file plans the drift
    # lock as well -- beside its own `rust_src`.
    if (path.startswith("workshop/tools/llm-conformance/")
            or path in ("scripts/tests/test_llm_conformance.py",
                        "crates/meclaw-cells/src/llm/translate.rs")):
        cls.add("llm_conformance")

    # The conversation guide's self-test (GH #881): the harness, its fixture,
    # the shipped block its drift checks compare, and the two files it imports
    # from elsewhere. Those two keep their own classes too -- a break in the
    # engine or the fact counter has to plan the self-test that runs on them.
    if (path.startswith("workshop/evals/conversation-guide/")
            or path.startswith("workshop/fixtures/positive/conversation-guide/")
            or path in ("templates/memory-hive/inline-contract.md",
                        "workshop/evals/engine.py",
                        "workshop/evals/colony_log.py",
                        "scripts/tests/test_colony_log.py",
                        "workshop/tools/display-lab/fact_yield.py",
                        # L-7 (tb-s10): the #871 replay's unittest, and the
                        # splitter `replay_871.py` imports from the lab.
                        "scripts/tests/test_replay_871.py",
                        "workshop/tools/display-lab/sidecar_turns.py")):
        cls.add("evals_guide")

    # The browser half of the same document: the layout driver under workshop/ and
    # the two locks that run it. No `template` class comes with it -- workshop/ is a
    # FORBIDDEN_PREFIX of the export, which is why the station is in CI_EXCLUDED.
    # The Playwright pin and its lock: `lane_sync.sh browsers` provisions the
    # engine every driver runs on from it, and no test names either -- before,
    # a bump planned no browser at all (OR-V2.v2-r3-planbrowser.2).
    if ((path.startswith("workshop/tools/display-") and not lab) or path in BROWSER_LOCKS
            or path in BROWSER_ENGINE_PINS):
        cls.add("display_browser")

    # The persona gate (ADR-0029, GH #882): the suite itself -- its committed
    # report included, which is why that report is not in IGNORED -- and the
    # sources a persona is made of, with the same glob semantics as
    # CORPUS_SOURCES. `workshop/evals/engine.py` carries `persona_source` beside
    # whatever class its own suite gives it.
    if path.startswith("workshop/evals/persona/"):
        cls.add("evals_persona")
    if any(_matches(path, g) for g in PERSONA_SOURCES):
        cls.add("persona_source")

    # `.config/nextest.toml` is gate infrastructure as much as the runner is:
    # it decides which test may be retried, and `test_nextest_quarantine.py`
    # is the lock that keeps an entry from reaching nothing. It keeps
    # `workspace` as well -- a changed profile changes every test run.
    # The strand kit and the receipt builder as well: their tests run in
    # `gate-selftest`, and a diff that touched only `strand.sh` planned
    # `shellcheck` and nothing else -- an under-selection (GH #861).
    # Since W3 (#1117) `gate.sh` is a shell: the runner body is
    # `gate_legacy.sh` and the new path is `pipeline.py` + `scripts/pipeline/`,
    # whose tests run in `gate-selftest` as well.
    if (path in ("scripts/gate.sh", "scripts/gate_legacy.sh",
                 "scripts/gate_plan.py", "scripts/precheck.py",
                 "scripts/pipeline.py",
                 "scripts/pipeline/pre.py", "scripts/pipeline/locks.py",
                 "scripts/test-tier.sh", "scripts/wave_retro.py",
                 "scripts/display_sync.py", "scripts/strand.sh",
                 "scripts/wave_receipt.py", "scripts/slow_tests.py",
                 "scripts/flake_triage.py", "scripts/testing_bans.py",
                 "scripts/stop_watch.py",
                 "scripts/check_required_pins.py", REQUIRED_PINS,
                 ".config/nextest.toml", ".config/slow-debt.toml",
                 ".config/testing-bans.toml", ".config/testing-bans.baseline.json",
                 ".config/flaky-legacy.toml", ".config/testing-rulings.toml", "ruff.toml")
            or path.startswith("scripts/tests/")
            or path.startswith("scripts/retro/")
            or path.startswith("scripts/pipeline/")
            or path.startswith("workshop/grafana/")):
        cls.add("gate_infra")

    # `export-selftest` reads the real tree, not only its fixtures (V2-A13, GH
    # #1199): `test_drift_gate.py` scans ROOT_FILES and every tracked scripts/
    # file with its imports, pins real files by path and checks each declared
    # string exemption; `test_export_tree_tests.py` resolves the MUST tests and
    # ci.yml and runs `make_export.build_index("HEAD")` over ROOT_FILES,
    # DOCS_MAP, the public templates and R16. A list of those inputs would be
    # the next drift, so the rule is by construction: whatever may ship plans
    # the station. The plan sees paths, not statuses -- a new file, a rename, a
    # deletion and a new `import` in a kept file all reach it as a path here.
    if _feeds_the_export_selftest(path):
        cls.add("export_infra")

    if path.endswith(".sh"):
        cls.add("shell")
    if path.startswith(".github/workflows/"):
        cls.add("ci")

    return cls


def classify(paths):
    """The union of the diff classes over `paths`."""
    out = set()
    for p in _norm(paths):
        out |= _classes_of_path(p)
    return out


def _templates_of(paths):
    """The template names the diff touches (first path segment under templates/)."""
    out = set()
    for p in _norm(paths):
        parts = p.split("/")
        if parts[0] == "templates" and len(parts) >= 3:
            out.add(parts[1])
    return out


def _examples_of(paths):
    """The example names the diff touches."""
    out = set()
    for p in _norm(paths):
        parts = p.split("/")
        if parts[0] == "examples" and len(parts) >= 3:
            out.add(parts[1])
    return out


def crates_of(paths, repo=None):
    """The crate directories the diff touches (rust_src, rust_test or crate_file).

    A crate directory is the crate name. Where the manifest exists that
    assumption is VERIFIED rather than trusted: a `-p <dir>` built from a
    directory whose `Cargo.toml` names something else selects nothing and the
    gate would go green on an empty run.
    """
    root = REPO_ROOT if repo is None else repo
    out = set()
    for p in _norm(paths):
        crate = _crate_of(p)
        if crate and _classes_of_path(p) & {"rust_src", "rust_test", "crate_file"}:
            out.add(crate)
    for crate in sorted(out):
        manifest = os.path.join(root, "crates", crate, "Cargo.toml")
        if not os.path.isfile(manifest):
            continue
        with open(manifest, encoding="utf-8") as fh:
            m = re.search(r'^\s*name\s*=\s*"([^"]+)"', fh.read(), re.MULTILINE)
        if m and m.group(1) != crate:
            raise GatePlanError(
                "crates/%s/Cargo.toml declares name = %r. The resolver builds "
                "`-p %s` and `rdeps(%s)` from the directory name; a mismatch "
                "selects nothing and the gate would pass on an empty run."
                % (crate, m.group(1), crate, crate))
    return out


_DEP_TABLES = ("dependencies", "dev-dependencies", "dev_dependencies",
               "build-dependencies", "build_dependencies")
_GLOB_CHARS = re.compile(r"[*?\[]")


class _Unresolved(Exception):
    """A workspace the member model cannot resolve exactly as cargo does."""


def workspace_members(repo=None):
    """`(names, None)` -- the package names cargo makes workspace members of the
    root manifest -- or `([], "<manifest>:1: <cause>")` (#1166 re-check m-4).

    The model of cargo's `Workspace::find_members` (cargo/core/workspace.rs),
    since cargo never runs on the planning machine: the `members` entries,
    globs expanded to their directories and each match dropped by `exclude`
    (`is_excluded`: a path under an exclude entry, unless a literal member
    entry covers it); the root manifest's own `[package]`; and, transitively,
    every path dependency of a member (any dependency table, also
    target-specific, inherited from `[workspace.dependencies]`, renamed or
    optional) that lies under the root and is not excluded. Whatever cargo
    would reject, or this model does not cover (a glob directory without a
    manifest, a member outside the root, a nested or foreign workspace root,
    a path in `[patch]`/`[replace]`, two packages with one name), is refused:
    the cause names its manifest as `path:1:`, the RED line's first finding."""
    import tomllib
    root = os.path.normpath(os.path.abspath(REPO_ROOT if repo is None else repo))
    loaded = {}

    def rel(d):
        r = os.path.relpath(os.path.join(d, "Cargo.toml"), root)
        return r.replace(os.sep, "/")

    def load(d):
        if d not in loaded:
            try:
                with open(os.path.join(d, "Cargo.toml"), "rb") as fh:
                    loaded[d] = tomllib.load(fh)
            except OSError as exc:
                raise _Unresolved("%s:1: no readable manifest (%s)"
                                  % (rel(d), type(exc).__name__))
            except tomllib.TOMLDecodeError as exc:
                raise _Unresolved("%s:1: the manifest does not parse (%s)" % (rel(d), exc))
        return loaded[d]

    def strings(value, what):
        if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
            raise _Unresolved("Cargo.toml:1: [workspace] %s is not a list of paths" % what)
        return value

    def under(d, base):
        return d == base or d.startswith(base.rstrip(os.sep) + os.sep)

    try:
        top = load(root)
        ws = top.get("workspace")
        if not isinstance(ws, dict):
            raise _Unresolved("Cargo.toml:1: the root manifest has no [workspace] table")
        listed = strings(ws.get("members", []), "members")
        exclude = [os.path.normpath(os.path.join(root, e))
                   for e in strings(ws.get("exclude", []), "exclude")]
        literal = [os.path.normpath(os.path.join(root, m)) for m in listed]
        for table in ("patch", "replace"):
            for entry in _dep_entries(top.get(table) or {}, nested=table == "patch"):
                if "path" in entry:
                    raise _Unresolved("Cargo.toml:1: [%s] names a path; cargo's membership "
                                      "of it is not modelled here" % table)
        ws_deps = ws.get("dependencies") or {}

        def excluded(d):
            return (any(under(d, ex) for ex in exclude)
                    and not any(under(d, m) for m in literal))

        order = []  # member directories in discovery order
        for entry in listed:
            path = os.path.normpath(os.path.join(root, entry))
            if not _GLOB_CHARS.search(entry):
                order.append(path)
                continue
            dirs = [d for d in sorted(globmod.glob(path)) if os.path.isdir(d)]
            if not dirs:
                raise _Unresolved("Cargo.toml:1: the member glob %r matches no directory" % entry)
            for d in dirs:
                d = os.path.normpath(d)
                if excluded(d):
                    continue
                if not os.path.isfile(os.path.join(d, "Cargo.toml")):
                    raise _Unresolved("%s:1: the member glob %r matches a directory "
                                      "without a manifest" % (rel(d), entry))
                order.append(d)
        if isinstance(top.get("package"), dict):
            order.append(root)
        names, seen, queue = [], {}, list(order)
        while queue:
            d = queue.pop(0)
            if d in seen:
                continue
            if not under(d, root):
                raise _Unresolved("%s:1: a member outside the workspace root" % rel(d))
            m = load(d)
            pkg = m.get("package")
            if d != root and "workspace" in m:
                raise _Unresolved("%s:1: a member with its own [workspace] table" % rel(d))
            name = pkg.get("name") if isinstance(pkg, dict) else None
            if not isinstance(name, str) or not name:
                raise _Unresolved("%s:1: the member has no [package] name" % rel(d))
            if "workspace" in pkg and os.path.normpath(
                    os.path.join(d, str(pkg["workspace"]))) != root:
                raise _Unresolved("%s:1: `%s` names another workspace root (%r)"
                                  % (rel(d), name, pkg["workspace"]))
            if name in seen.values():
                raise _Unresolved("%s:1: a second package named `%s` in the workspace"
                                  % (rel(d), name))
            seen[d] = name
            names.append(name)
            for key, entry in _member_deps(m):
                if entry.get("workspace") is True:
                    base, entry = root, ws_deps.get(key)
                    if not isinstance(entry, dict):
                        continue
                else:
                    base = d
                if "path" not in entry:
                    continue
                if not isinstance(entry["path"], str):
                    raise _Unresolved("%s:1: dependency `%s` of `%s` has a path that is not "
                                      "a string" % (rel(d), key, name))
                dep = os.path.normpath(os.path.join(base, entry["path"]))
                if not under(dep, root):
                    try:
                        claims = "workspace" in (load(dep).get("package") or {})
                    except _Unresolved:
                        claims = False
                    if claims:
                        raise _Unresolved("%s:1: path dependency `%s` of `%s` lies outside "
                                          "the root and names a workspace" % (rel(dep), key, name))
                    continue
                if excluded(dep):
                    continue
                if not os.path.isfile(os.path.join(dep, "Cargo.toml")):
                    raise _Unresolved("%s:1: path dependency `%s` of `%s` has no manifest"
                                      % (rel(dep), key, name))
                queue.append(dep)
        if not names:
            raise _Unresolved("Cargo.toml:1: the workspace manifest names no members")
        return names, None
    except _Unresolved as exc:
        return [], str(exc)


def _dep_entries(table, nested=False):
    """The table-valued entries of a dependency table (`[patch.<src>]`: nested)."""
    if not isinstance(table, dict):
        return []
    if nested:
        return [e for t in table.values() for e in _dep_entries(t)]
    return [e for e in table.values() if isinstance(e, dict)]


def _member_deps(manifest):
    """`(key, entry)` of every table-valued dependency of a package manifest:
    the plain tables and those under each `[target.<cfg>]`."""
    tables = [manifest] + [t for t in (manifest.get("target") or {}).values()
                           if isinstance(t, dict)]
    out = []
    for t in tables:
        for kind in _DEP_TABLES:
            deps = t.get(kind)
            if isinstance(deps, dict):
                out.extend((k, e) for k, e in deps.items() if isinstance(e, dict))
    return out


def crate_isolation(repo=None):
    """`(scope, cmds)` of the release station `crate-isolation` (#1166 re-check
    M-1): `cargo check -p <member> --all-targets` for every workspace member,
    one call each -- a separate `-p` resolves the shared dependencies with that
    member's features only, which no merged call of the build station does.
    Members from `workspace_members`. Fail-closed (m-4/m-5): a member list the
    model refuses is ONE command that exits 1 with `crate-isolation: <cause>`,
    the cause starting with its manifest as `path:1:`, and the scope `refused`
    -- a release must not pass on a check that ran for nobody."""
    names, why = workspace_members(repo)
    if why:
        msg = "crate-isolation: %s; refusing to pass a check that ran for nobody" % why
        return "refused", [["python3", "-c", "import sys; sys.exit(%r)" % msg]]
    return ("%d members, own features" % len(names),
            [["cargo", "check", "-p", n, "--all-targets"] for n in names])


def crate_isolation_cmds(repo=None):
    """The commands of `crate_isolation`."""
    return crate_isolation(repo)[1]


# --- test sources -----------------------------------------------------------

_SOURCE_CACHE = {}
# `{root: (sources, {repo-relative file: text})}`: the text of every file
# `_test_sources` read, so `_test_files` and `_includers` read none of them
# again (re-check tb-s10 A1 13 d-4). Valid while `sources` is the cached list.
_SOURCE_TEXTS = {}


def _test_sources(repo):
    """`[(crate, unit, text)]` for every test source under `repo`.

    A flat `crates/*/tests/<stem>.rs` is the unit `<stem>`. A folded harness
    (`scripts/harness_fold.py`, GH #1119) is a directory with a `main.rs`:
    its `main.rs` is the unit `<harness>/main` (the whole binary, and the file
    that `#[path]`s the shared helpers in) and every other `.rs` in it is the
    unit `<harness>/<stem>` -- one module of that binary. `_binary_id` turns a
    unit into the nextest term that selects exactly that.
    """
    root = REPO_ROOT if repo is None else repo
    cached = _SOURCE_CACHE.get(root)
    if cached is not None:
        return cached
    out, texts = [], {}
    crates_dir = os.path.join(root, "crates")
    if os.path.isdir(crates_dir):
        for crate in sorted(os.listdir(crates_dir)):
            tests_dir = os.path.join(crates_dir, crate, "tests")
            if not os.path.isdir(tests_dir):
                continue
            for entry in sorted(os.listdir(tests_dir)):
                path = os.path.join(tests_dir, entry)
                if os.path.isfile(os.path.join(path, "main.rs")):
                    with open(os.path.join(path, "main.rs"), encoding="utf-8",
                              errors="replace") as fh:
                        main_text = fh.read()
                    files = [("%s/main" % entry if f == "main.rs"
                              else _folded_unit(main_text, entry, f[:-3]),
                              os.path.join(path, f))
                             for f in sorted(os.listdir(path)) if f.endswith(".rs")]
                elif entry.endswith(".rs"):
                    files = [(entry[:-3], path)]
                else:
                    continue
                for unit, fpath in files:
                    if not os.path.isfile(fpath):
                        continue
                    if fpath == os.path.join(path, "main.rs"):
                        text = main_text
                    else:
                        try:
                            with open(fpath, encoding="utf-8", errors="replace") as fh:
                                text = fh.read()
                        except OSError:
                            continue
                    out.append((crate, unit, text))
                    texts[os.path.relpath(fpath, root).replace(os.sep, "/")] = text
    _SOURCE_CACHE[root] = out
    _SOURCE_TEXTS[root] = (out, texts)
    return out


def _source_texts(repo):
    """`{repo-relative file: text}` of the files `_test_sources` read."""
    root = REPO_ROOT if repo is None else repo
    sources = _test_sources(repo)
    hit = _SOURCE_TEXTS.get(root)
    return hit[1] if hit is not None and hit[0] is sources else {}


_TEST_FILES = {}


def _test_files(repo):
    """`{repo-relative file: (crate, unit, helper)}` for every `.rs` below
    `crates/*/tests/`, plus the helpers' texts: a file of a `_test_sources`
    unit has its unit; any other file -- in a directory without main.rs
    (`tests/support/*.rs`, `tests/h5_support/mod.rs`, `tests/gh927/*.rs`) or
    in a subdirectory of a harness -- is a HELPER, unit = its path below
    tests/ without `.rs`, no binary of its own (`_helper_users`). Rebuilt with
    `_test_sources`."""
    root = REPO_ROOT if repo is None else repo
    sources = _test_sources(repo)
    hit = _TEST_FILES.get(root)
    if hit is not None and hit[0] is sources:
        return hit[1], hit[2]
    files, helpers = {}, []
    texts = _source_texts(repo)
    crates_dir = os.path.join(root, "crates")
    for crate in sorted(os.listdir(crates_dir)) if os.path.isdir(crates_dir) else ():
        tests_dir = os.path.join(crates_dir, crate, "tests")
        for dirpath, dirnames, filenames in os.walk(tests_dir):
            dirnames.sort()
            rel = os.path.relpath(dirpath, tests_dir).replace(os.sep, "/")
            harness = "/" not in rel and "main.rs" in filenames
            main_text = None
            for f in sorted(filenames):
                if not f.endswith(".rs"):
                    continue
                key = "crates/%s/tests/%s" % (crate, f if rel == "." else rel + "/" + f)
                if rel == ".":
                    files[key] = (crate, f[:-3], False)
                elif harness:
                    if main_text is None:
                        main_text = texts.get("crates/%s/tests/%s/main.rs" % (crate, rel))
                    if main_text is None:
                        with open(os.path.join(dirpath, "main.rs"), encoding="utf-8",
                                  errors="replace") as fh:
                            main_text = fh.read()
                    files[key] = (crate, "%s/main" % rel if f == "main.rs"
                                  else _folded_unit(main_text, rel, f[:-3]), False)
                else:
                    unit = "%s/%s" % (rel, f[:-3])
                    try:
                        with open(os.path.join(dirpath, f), encoding="utf-8",
                                  errors="replace") as fh:
                            helpers.append((crate, unit, fh.read()))
                    except OSError:
                        continue
                    files[key] = (crate, unit, True)
    _TEST_FILES[root] = (sources, files, helpers)
    return files, helpers


_READER_SOURCES = {}


def _reader_sources(repo):
    """`_test_sources` plus the helpers (`_test_files`): every text the reader
    rules (4, 5, 8) read. Before tb-s1c-c (#1177, F4) the 30 helpers of the
    tree were in none of them -- what they name or walk planned nothing for
    the binaries that compile them in. `_test_sources` stays the units WITH a
    binary (rule 9, `_sharers`, the graph's `bin:` nodes)."""
    root = REPO_ROOT if repo is None else repo
    sources = _test_sources(repo)
    hit = _READER_SOURCES.get(root)
    if hit is None or hit[0] is not sources:
        helpers = _test_files(repo)[1]
        hit = _READER_SOURCES[root] = (sources, sources + helpers,
                                       frozenset((c, u) for c, u, _t in helpers))
    return hit[1]


def _is_helper(repo, crate, unit):
    """Is `(crate, unit)` a helper of `_test_files` (no binary of its own)?"""
    _reader_sources(repo)
    return (crate, unit) in _READER_SOURCES[REPO_ROOT if repo is None else repo][2]


# `path = "x.rs"` in `#[path = ..]` and in `#[cfg_attr(.., path = ..)]`
_PATH_ATTR_RE = re.compile(r'\bpath\s*=\s*"([^"\\\n]+\.rs)"')
_MOD_DECL_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;", re.M)
_INCLUDERS = {}


def _includers(repo):
    """`{file: {file that compiles it in}}` over the `.rs` below `crates/*/tests/`
    (`_test_files`). Over-approximated on purpose, an extra edge only plans
    more: `#[path = "p"]` is resolved against the including file's directory
    and, for a non-mod-rs file, against `<dir>/<stem>/` too; `mod m;` names
    `m.rs` and `m/mod.rs` in both of those directories."""
    root = REPO_ROOT if repo is None else repo
    files, helpers = _test_files(repo)
    hit = _INCLUDERS.get(root)
    if hit is not None and hit[0] is files:
        return hit[1]
    texts = _source_texts(repo)
    helper_texts = {(c, u): t for c, u, t in helpers}
    out = collections.defaultdict(set)
    seen = {}   # GH #1183: `{prefix: is a symlink}`, one lstat per directory
    for f, (c, u, _h) in files.items():
        d, base = posixpath.split(f)
        bases = [d] if base in ("main.rs", "mod.rs", "lib.rs") else [d, d + "/" + base[:-3]]
        if not _h and "/" not in u:
            bases = [d]   # a flat test is a crate root (mod-rs)
        if _h:
            text = helper_texts.get((c, u), "")
        else:
            text = texts.get(f)
            if text is None:
                text = _read_text(os.path.join(root, f))
        if text:
            for m in _PATH_ATTR_RE.finditer(text):
                for b in bases:
                    out[posixpath.normpath(b + "/" + m.group(1))].add(f)
                    real = _real_key(root, b + "/" + m.group(1), seen)
                    if real:
                        out[real].add(f)
            for m in _MOD_DECL_RE.finditer(text):
                for b in bases:
                    for key in ("%s/%s.rs" % (b, m.group(1)), "%s/%s/mod.rs" % (b, m.group(1))):
                        out[key].add(f)
                        # re-check tb-s10 A1 9 L-b: `mod m;` through a link
                        # opens the file in the link's target, as `#[path]` does
                        real = _real_key(root, key, seen)
                        if real:
                            out[real].add(f)
    out = {k: frozenset(v) for k, v in out.items()}
    _INCLUDERS[root] = (files, out)
    return out


def _real_key(root, rel, seen):
    """The repo-relative file the compiler opens for the `#[path]` join `rel`
    when a directory on its way is a symlink, else None. GH #1183 (re-check
    tb-s1c L-2): `normpath` names the file next to the link -- one the walk of
    `_test_files` never lists --, the compiler the one in the link's target,
    so that includer was missing from its helper (fail-open). The same lstat
    of each prefix as `_resolve_dotdot`, the file itself included (re-check
    tb-s1e h2 L-a: with `far/link.rs` -> `deep/s2.rs`, `far/deep/s2.rs`
    lacked the includer of `far/link.rs`); out of the repository: None."""
    segs = rel.split("/")
    for j in range(1, len(segs) + 1):
        pre = posixpath.normpath("/".join(segs[:j]))
        if pre not in seen:
            seen[pre] = os.path.islink(os.path.join(root, pre))
        if seen[pre]:
            real = os.path.relpath(os.path.realpath(os.path.join(root, rel)),
                                   os.path.realpath(root)).replace(os.sep, "/")
            return None if real == ".." or real.startswith("../") else real
    return None


_TREE_LINKS = {}
_GIT_DIRS = {}


def _tree_links(root):
    """The repo-relative symlinks of the tree: those of its HEAD (mode 120000)
    -- the commit's links, deterministic. Re-check tb-s1e h2 m-2: a walk of
    the directories went through every untracked one, 0.34 s of each plan in
    the main tree (152k entries, 118k of them below the untracked workshop/),
    +12-15 %; measured there 362-404 ms for the walk, 34-44 ms for the index
    (3 runs each). A tree outside git keeps the walk. Re-check tb-s1e L-e:
    the memo follows HEAD (`_head_stamp`), a long-lived process that plans
    again after a commit reads the new links."""
    stamp = _head_stamp(root)
    hit = _TREE_LINKS.get(root)
    if hit is None or hit[0] != stamp:
        found = None if stamp is None else _index_links(root)
        links = frozenset(_walked_links(root) if found is None else found)
        hit = _TREE_LINKS[root] = (stamp, links)
    return hit[1]


def _git_env(root):
    """The environment of a git call on `root` alone: no repository variables
    of the caller, the ceiling at its parent (a directory inside another work
    tree is no git tree)."""
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env["GIT_CEILING_DIRECTORIES"] = os.path.dirname(os.path.realpath(root))
    return env


def _head_stamp(root):
    """Where HEAD of `root` stands, from a few stats -- no git call after the
    first per tree: the text of HEAD and the stat of the files a move of HEAD
    rewrites (HEAD, the branch it names, packed-refs, the reftable list).
    None when `root` is no git tree."""
    dirs = _GIT_DIRS.get(root)
    if dirs is None:
        try:
            res = subprocess.run(["git", "-C", root, "rev-parse", "--absolute-git-dir",
                                  "--git-common-dir"], capture_output=True, env=_git_env(root))
        except OSError:
            res = None
        lines = [] if res is None or res.returncode else os.fsdecode(res.stdout).splitlines()
        dirs = _GIT_DIRS[root] = (lines[0], os.path.join(root, lines[1])) if len(lines) == 2 else ()
    if not dirs:
        return None
    gitdir, common = dirs
    head = os.path.join(gitdir, "HEAD")
    try:
        with open(head, "rb") as fh:
            text = fh.read()
    except OSError:
        text = b""
    paths = [head, os.path.join(common, "packed-refs"),
             os.path.join(common, "reftable", "tables.list")]
    if text.startswith(b"ref:"):
        paths.append(os.path.join(common, os.fsdecode(text[4:].strip())))
    stats = []
    for path in paths:
        try:
            st = os.stat(path)
            stats.append((st.st_mtime_ns, st.st_size, st.st_ino))
        except OSError:
            stats.append(None)
    return text, tuple(stats)


def _index_links(root):
    """The symlinks of HEAD of `root` (`git ls-tree`, re-check tb-s1e L-f:
    the plan is `rev..HEAD`, so a staged link does not count and one dropped
    with `git rm --cached` still does), None when `root` is no git tree or
    HEAD names no commit (the walk then)."""
    try:
        res = subprocess.run(["git", "-C", root, "ls-tree", "-r", "-z", "--full-tree", "HEAD"],
                             capture_output=True, env=_git_env(root))
    except OSError:
        return None
    if res.returncode:
        return None
    return {os.fsdecode(path) for meta, _, path in
            (rec.partition(b"\t") for rec in res.stdout.split(b"\0"))
            if meta.startswith(b"120000 ")}


def _walked_links(root):
    """The symlinks below `root` on disk (`.git` and `target` aside)."""
    found, todo = set(), [""]
    while todo:
        rel = todo.pop()
        try:
            it = os.scandir(os.path.join(root, rel))
        except OSError:
            continue
        with it:
            for e in it:
                p = rel + "/" + e.name if rel else e.name
                if e.is_symlink():
                    found.add(p)
                elif e.is_dir() and p not in (".git", "target"):
                    todo.append(p)
    return found


def _linked_anywhere(root, pre):
    """Is `pre` a symlink below ANY directory of the tree? GH #1183 (re-check
    tb-s1c L-4): a test joins a literal to any base -- `tests/fixtures` as
    much as the crate --, and `_resolve_dotdot` looked for the link only at
    the root, the crate, its tests/ and the reader's directory. A prefix that
    climbs above its start (`a/../../lnk` is `../lnk`) lands below some
    directory too: its rest is matched the same way (re-check tb-s1e h2 L-c,
    it answered False there)."""
    pre = posixpath.normpath(pre)
    while pre.startswith("../"):
        pre = pre[3:]
    if pre in (".", ".."):
        return False
    return any(link == pre or link.endswith("/" + pre) for link in _tree_links(root))


def _read_text(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            return fh.read()
    except OSError:
        return ""


def _helper_users(repo, crate, unit):
    """The terms a helper (`_test_files`) reads for: rule 3's set
    (`_unit_users`) of every unit that compiles it in, through other helpers
    transitively (`support/warm_python.rs` is `#[path]`ed only by
    `support/curator_hive.rs` and `support/file_space_hive.rs`). Nobody found:
    `rdeps(<crate>)` -- the crate and its dependents (fail-closed, tb-s1c-c F4)."""
    files = _test_files(repo)[0]
    inc = _includers(repo)
    start = "crates/%s/tests/%s.rs" % (crate, unit)
    seen, todo, users = {start}, [start], set()
    while todo:
        for f in sorted(inc.get(todo.pop(), ())):
            if f in seen:
                continue
            seen.add(f)
            c, u, helper = files[f]
            if helper:
                todo.append(f)
            else:
                users |= _unit_users(repo, c, u)
    return users or {"rdeps(%s)" % crate}


def _folded_unit(main_text, harness, mod):
    """`<harness>/<mod>` when the harness `main.rs` declares `mod <mod>;` at its
    top level (the harness_fold shape), else `<harness>/main`: in any other
    directory target the module's test names carry no known prefix, so the
    whole binary is the only honest selection.

    Read over `crate_root.tokens` (OR-TB.S1.19): a `mod <mod>;` nested in a
    block, a macro or a comment is no fold, and a declaration carrying any
    attribute but a lint (`#[cfg]` may drop it, `#[path]` moves it off this
    file) or a `main.rs` the lexer doubts runs the whole harness -- never a
    `<mod>::` term that selects 0 tests in silence. The lexer runs once per
    main.rs (`_folded_mods`), not once per module (tb-s1c-d, #1177).
    """
    return harness + "/" + mod if mod in _folded_mods(main_text) else harness + "/main"


_FOLDED_MODS = {}
# The tree holds ~100 main.rs; the bound only stops a long-lived process from
# keeping every text it ever saw (re-check tb-s1c L-7). Content-addressed, so
# dropping the memo costs one lexer pass per text, never a stale answer.
_FOLDED_MODS_MAX = 1024


def _folded_mods(main_text):
    """Every module `_folded_unit` folds in `main_text`: each `mod <name>;` at
    the top level whose every declaration is a plain item (`_plain_mod_item`);
    empty when the lexer doubts or a bracket does not balance. Memoized per
    text -- one lexer pass per main.rs instead of one per module (#1177)."""
    names = _FOLDED_MODS.get(main_text)
    if names is None:
        if len(_FOLDED_MODS) >= _FOLDED_MODS_MAX:
            _FOLDED_MODS.clear()
        names = _FOLDED_MODS[main_text] = _scan_folded_mods(main_text)
    return names


def _scan_folded_mods(main_text):
    code = _rust_code(main_text)
    if code is None:
        return frozenset()
    hits, depth = {}, 0
    for i, t in enumerate(code):
        if t in _BRACKETS:
            depth += 1
        elif t in (")", "]", "}"):
            depth -= 1
            if depth < 0:
                return frozenset()
        elif depth == 0 and t == "mod" and code[i + 2:i + 3] == [";"]:
            hits.setdefault(code[i + 1], []).append(i)
    if depth:
        return frozenset()
    names = set()
    for name, at in hits.items():
        try:
            if all(_plain_mod_item(code, i) for i in at):
                names.add(name)
        except crate_root.Doubt:
            pass
    return frozenset(names)


_LINT_ATTRS = {"allow", "expect", "warn", "deny", "forbid"}


def _plain_mod_item(code, i):
    """True when the top-level `mod` at `code[i]` starts its item: only a
    visibility and lint attributes before it, then an item boundary."""
    j = i - 1
    if j >= 0 and code[j] == ")":
        j = _opening(code, j) - 1
        if j < 0 or code[j] != "pub":
            return False
        j -= 1
    elif j >= 0 and code[j] == "pub":
        j -= 1
    while j >= 0 and code[j] == "]":
        k = _opening(code, j)
        if k >= 1 and code[k - 1] == "!":
            break  # an inner attribute of the crate ends the run
        if k < 1 or code[k - 1] != "#" or code[k + 1] not in _LINT_ATTRS:
            return False
        j = k - 2
    return j < 0 or code[j] in (";", "}", "]")


def _binary_id(crate, stem):
    """The nextest term for a unit of `_test_sources`.

    A folded module `<harness>/<stem>` is its harness binary narrowed to the
    tests whose names start `<stem>::` -- harness_fold makes every old file one
    `pub mod <stem>;`, so that prefix is exactly the old binary's tests
    (measured: names SAME over all 153 cells binaries, W5-2c). The term is
    parenthesised so it composes under `+` and `-` like a bare `binary_id`.
    """
    if "/" in stem:
        harness, mod = stem.split("/", 1)
        if mod == "main":
            return "binary_id(=%s::%s)" % (crate, harness)
        return "(binary_id(=%s::%s) & test(/^%s::/))" % (crate, harness, mod)
    return "binary_id(=%s::%s)" % (crate, stem)


def _test_binary_exists(repo, crate, stem):
    """Is `<crate>::<stem>` an integration-test target that actually EXISTS?

    nextest rejects a `binary_id(=..)` that matches nothing -- not by ignoring
    the term, but with "operator didn't match any binary IDs" and "failed to
    parse filterset", which takes the WHOLE `tests` station down. So a term is
    only ever emitted for a file that is there to compile.

    `git diff --name-only` lists DELETIONS, so a diff naming a test file is no
    proof that the file is still in the tree; the same holds for any path whose
    stem is not a target (a leftover directory without a `main.rs`). Both are
    the shape cargo autodiscovers: `tests/<stem>.rs` or `tests/<stem>/main.rs`.
    """
    root = REPO_ROOT if repo is None else repo
    tests = os.path.join(root, "crates", crate, "tests")
    if "/" in stem:  # a folded unit `<harness>/<stem>`: a harness and the file
        harness, mod = stem.split("/", 1)
        return (os.path.isfile(os.path.join(tests, harness, "main.rs"))
                and os.path.isfile(os.path.join(tests, harness, mod + ".rs")))
    return (os.path.isfile(os.path.join(tests, stem + ".rs"))
            or os.path.isfile(os.path.join(tests, stem, "main.rs")))


# --- binaries that run no test ----------------------------------------------
#
# GH #997: rule 8 picked `meclaw-colony::gh850_bench_normal_path` for a diff to
# `scripts/test-tier.sh` -- its doc comment names the path -- and its only test
# is an `#[ignore]` benchmark. The `tests` station then ran 0 tests (nextest
# exit 4 on master; `no tests matched`, exit 6, since K1). A binary whose every
# test is ignored is no drift lock for anything, so NO rule may target it,
# and only that binary is dropped, in one place in `test_filter`. (The prose
# reference itself is no hit since #1160, `_reader_text`.)

# The lexer is `crate_root.tokens` (welle-tb S4, GH #1162) -- imported, not
# copied. The regex lexer before it lost its phase on a string with a
# `\`-newline continuation: its closing quote opened a "string", and from
# there on code read as literal and literal as code. `misc_1/cogny_template.rs`
# (4 tests) counted 0, and a `#[test]`, a `mod x;` or a macro past such a
# string was gone -- rule 9 dropped binaries that run tests. A comment of any
# kind goes (a doctest is no test of an integration binary), a literal is ONE
# token (a `#[test]` or `mod x;` inside it is no code, a `//` inside it no
# comment); a file the lexer doubts is kept.
_SCRIPTS_DIR = os.path.dirname(os.path.abspath(__file__))
if _SCRIPTS_DIR not in sys.path:
    sys.path.insert(0, _SCRIPTS_DIR)
from pipeline import crate_root  # noqa: E402

# An attribute that makes a test, read in the direction of doubt (K1 fix
# round 2): every attribute whose LAST path segment contains `test` --
# `#[test]`, `#[tokio::test(..)]`, `#[test_log::test]`, `#[rstest]`,
# `#[test_case(..)]`, `#[wasm_bindgen_test]` -- and `quickcheck`/`proptest`.
# A form this misses would count as "no test" and could drop a binary that
# runs tests; a form it over-counts only keeps a binary as target.
_TEST_ATTR_RE = re.compile(r'\w*test\w*|quickcheck|proptest')
# Only the unconditional `#[ignore]` / `#[ignore = ".."]` (the attribute's
# first token); a `#[cfg_attr(.., ignore)]` is conditional and leaves the test
# runnable.
_IGNORE_ATTR = "ignore"
# Unicode as Rust and the token lexer (`crate_root._IDENT`, tb-s10 A1 point 20).
_IDENT_RE = crate_root._IDENT
# Shapes that may carry tests the attribute count cannot see: `macro_rules!`,
# `include!`, an item macro (`gen!();`, `a::gen!{..}` where an item stands: at
# file level or directly in a `mod {}`), and a brace-bodied macro invocation at
# ANY depth (`quickcheck! { .. }`) -- except the expression macros
# `select!`/`join!`/`try_join!`, which never make an item:
# `gh1004_viewport_event_measure` (one `#[ignore]` test, a `select!` in its
# body) stayed a target of every `scripts/strand.sh` diff and the `tests`
# station ended RED on "no tests to run" (GH #1053 gate, 2026-10-07). A
# keyword before `!` is no macro (`impl !Send`, `if !x`).
_EXPR_MACROS = ("select", "join", "try_join")
_ITEM_BEFORE = (";", "}", "]", "{")
_KEYWORDS = frozenset((
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
    "enum", "extern", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
    "super", "trait", "type", "unsafe", "use", "where", "while", "yield"))
_BRACKETS = {"(": ")", "[": "]", "{": "}"}


def _rust_code(text):
    """The code tokens of `text` (`crate_root.tokens`, comments dropped); None in doubt.

    None when the lexer doubts -- a comment or literal left open, a character
    it does not know: every caller keeps the binary then.
    """
    try:
        toks = crate_root.tokens(text)
    except crate_root.Doubt:
        return None
    return [t for t in toks if not t.startswith(("//", "/*"))]


def _rust_code_aligned(text):
    """`text` with every comment (doc comments too) blanked to spaces, every
    literal whole, every offset and line where it was; None in doubt.

    For the readers that match text or report a line -- the browser marker
    rule (`browser_unit`, `browser_unplaced`, GH #1161) and
    `scripts/tests/test_browser_required.py`. They read the regex lexer's text
    before S4 (#1162) replaced it by `crate_root.tokens`; the spans of that
    lexer give the same text without the phase loss on a `\\`-newline string.
    """
    try:
        spans = crate_root.token_spans(text)
    except crate_root.Doubt:
        return None
    out = list(_NOT_NEWLINE_RE.sub(" ", text))
    for i, t in spans:
        if not t.startswith(("//", "/*")):
            out[i:i + len(t)] = t
    return "".join(out)


_NOT_NEWLINE_RE = re.compile(r"[^\n]")


def _closing(code, i):
    """Index of the bracket closing `code[i]`; Doubt when it does not close."""
    stack = []
    for j in range(i, len(code)):
        t = code[j]
        if t in _BRACKETS:
            stack.append(_BRACKETS[t])
        elif t in (")", "]", "}"):
            if not stack or stack.pop() != t:
                raise crate_root.Doubt("unbalanced %s" % t)
            if not stack:
                return j
    raise crate_root.Doubt("unclosed %s" % code[i])


# --- the text rule 8 scans: no comments (#1160, V2-R1-1) ----------------
#
# A path in a comment reads nothing: `gh617`/`gh623` named
# `scripts/gate_plan.py` in a `//!` paragraph about their file name and ran
# 22 tests (40.9 s) on every resolver diff. Rule 8 therefore scans code and
# literals and blanks every comment whole -- `//`, `///`, `//!`, `/* */`,
# `/** */`, `/*! */`, their code blocks too: a test file under
# `crates/*/tests/` is no library, rustdoc never runs its doc-comment code
# and nextest neither (V2-R1-1: `docs/development-rules.md` in comments
# planned 14 Rust tests for a docs-only diff). A hand lexer, not a regex: block comments nest, and a regex that
# closed `/* a /* b */ "x */` early would open a phantom string and could take
# a real `"http://.."` for a comment. Doubt keeps the text: an unterminated
# literal or comment returns it unchanged.
#
# The reason of `#[ignore = "..."]` is prose as well (re-check r1-kit M5):
# gh867/gh961/gh1028 named `scripts/gate_plan.py` in it and planned
# browser:display on every resolver diff. So is the message of an assert,
# panic or expect: nine tests named `docs/development-rules.md` only there.
# Such a literal is blanked like a comment -- only when it is the whole
# message argument; a literal in the condition or a format argument reads.
_READER_TOKEN_RE = re.compile(r'[bc]?r#*"|[bc]?"|b?\'|//|/\*')
_READER_STR_RE = re.compile(r'(?:\\.|[^"\\])*"', re.S)
_READER_CHAR_RE = re.compile(r"(?:\\(?:u\{[0-9A-Fa-f_]{1,8}\}|x[0-9A-Fa-f]{2}|.)|[^'\\\n])'")
_READER_BLOCK_RE = re.compile(r"/\*|\*/")
_READER_IGNORE_RE = re.compile(r"#\[\s*ignore\s*=\s*\Z")
# message argument index: macros (`name!(`) and methods (`.name(`)
_READER_MESSAGE_MACROS = {
    "panic": 0, "unreachable": 0, "todo": 0, "unimplemented": 0,
    "assert": 1, "debug_assert": 1,
    "assert_eq": 2, "assert_ne": 2, "debug_assert_eq": 2, "debug_assert_ne": 2,
}
_READER_MESSAGE_METHODS = {"expect": 0, "expect_err": 0}
_READER_MESSAGE_HINT_RE = re.compile(
    r"\b(?:%s)\b" % "|".join(sorted(set(_READER_MESSAGE_MACROS) | set(_READER_MESSAGE_METHODS))))
_READER_MACRO_NAME_RE = re.compile(r"(?<![\w:])(\w+)\s*!\s*\Z")
_READER_METHOD_NAME_RE = re.compile(r"\.\s*(\w+)\s*\Z")
_READER_CACHE = {}


def _reader_text(text):
    """`text` with every comment blanked -- what rule 8 scans.

    Offsets and newlines stay those of `text`. Kept: code and every literal but
    the reason of `#[ignore = "..."]` and the message of an assert, panic or
    expect (`_reader_message`). A comment of any form, doc comments and their
    code blocks included, is prose whole."""
    got = _READER_CACHE.get(text)
    if got is None:
        try:
            got = _reader_lex(text)
        except Exception:   # noqa: BLE001 -- doubt: the whole text counts
            got = text
        _READER_CACHE[text] = got
    return got


def _reader_lex(text):
    comments = []   # (start, end)
    reasons = []    # (start, end) of `#[ignore = "..."]` literals
    strings = []    # (start, end) of every string literal
    chars = []      # (start, end) of every char literal
    pos, n = 0, len(text)
    while True:
        m = _READER_TOKEN_RE.search(text, pos)
        if not m:
            break
        tok, i = m.group(0), m.start()
        if tok[0] in "bcr" and i > 0 and (text[i - 1].isalnum() or text[i - 1] == "_"):
            pos = i + 1   # `xb"`: the prefix belongs to an identifier
            continue
        if tok.endswith('"') and "r" in tok:
            hashes = tok.count("#")
            end = text.find('"' + "#" * hashes, m.end())
            if end < 0:
                return text
            pos = end + 1 + hashes
            strings.append((i, pos))
            if _READER_IGNORE_RE.search(text, max(0, i - 64), i):
                reasons.append((i, pos))
        elif tok.endswith('"'):
            s = _READER_STR_RE.match(text, m.end())
            if not s:
                return text
            pos = s.end()
            strings.append((i, pos))
            if _READER_IGNORE_RE.search(text, max(0, i - 64), i):
                reasons.append((i, pos))
        elif tok.endswith("'"):
            c = _READER_CHAR_RE.match(text, m.end())
            pos = c.end() if c else m.end()   # no char: a lifetime or a label
            if c:
                chars.append((i, pos))
        elif tok == "//":
            end = text.find("\n", i)
            end = n if end < 0 else end
            comments.append((i, end))
            pos = end
        else:
            depth, j = 1, m.end()
            while depth:
                b = _READER_BLOCK_RE.search(text, j)
                if not b:
                    return text
                depth += 1 if b.group(0) == "/*" else -1
                j = b.end()
            comments.append((i, j))
            pos = j
    if strings and _READER_MESSAGE_HINT_RE.search(text):
        mask = list(text)   # code only: literals and comments blanked
        for start, end in strings + chars:
            _reader_blank(mask, start, end)
        for start, end in comments:
            _reader_blank(mask, start, end)
        mask = "".join(mask)
        reasons += [(a, b) for a, b in strings if _reader_message(mask, a, b)]
    if not comments and not reasons:
        return text
    out = list(text)
    for start, end in reasons + comments:
        _reader_blank(out, start, end)
    return "".join(out)


def _reader_message(mask, start, end):
    """Is the literal at `start:end` the whole message argument of an assert,
    panic or expect? `mask` is the text with literals and comments blanked.
    Bounded walk back to the opening bracket; no bracket found is doubt (no)."""
    commas, depth, i = 0, 0, start - 1
    limit = max(0, start - 20000)
    while i >= limit:
        c = mask[i]
        if c in ")]}":
            depth += 1
        elif c in "([{":
            if depth == 0:
                break
            depth -= 1
        elif depth == 0:
            if c == ",":
                commas += 1
                if commas > 2:
                    return False
            elif commas == 0 and not c.isspace():
                return False   # the literal does not start its argument
        i -= 1
    else:
        return False
    head = mask[max(0, i - 64):i]
    m = _READER_MACRO_NAME_RE.search(head)
    want = _READER_MESSAGE_MACROS.get(m.group(1)) if m else None
    if want is None and mask[i] == "(":
        m = _READER_METHOD_NAME_RE.search(head)
        want = _READER_MESSAGE_METHODS.get(m.group(1)) if m else None
    if want is None or commas != want:
        return False
    j = end
    while j < len(mask) and mask[j].isspace():
        j += 1
    return j < len(mask) and mask[j] in ",)]}"   # the literal is the whole argument


def _reader_blank(out, start, end):
    for k in range(start, end):
        if out[k] != "\n":
            out[k] = " "


def _opening(code, i):
    """Index of the bracket opening the `code[i]` it closes; Doubt when none."""
    closers = {v: k for k, v in _BRACKETS.items()}
    stack = []
    for j in range(i, -1, -1):
        t = code[j]
        if t in closers:
            stack.append(closers[t])
        elif t in _BRACKETS:
            if not stack or stack.pop() != t:
                raise crate_root.Doubt("unbalanced %s" % t)
            if not stack:
                return j
    raise crate_root.Doubt("unopened %s" % code[i])


def _attribute_groups(code):
    """Runs of outer attributes standing back to back, as lists of bodies
    (the tokens between `#[` and `]`)."""
    groups, i, n = [], 0, len(code)
    while i < n - 1:
        if code[i] != "#" or code[i + 1] != "[":
            i += 1
            continue
        group = []
        while i < n - 1 and code[i] == "#" and code[i + 1] == "[":
            close = _closing(code, i + 1)
            group.append(code[i + 2:close])
            i = close + 1
        groups.append(group)
    return groups


def _is_test_attr(body):
    """Does the attribute's leading path end in a segment that makes a test?"""
    last = None
    for t in body:
        if t == "::":
            continue
        if not _IDENT_RE.fullmatch(t):
            break
        last = t
    return last is not None and bool(_TEST_ATTR_RE.fullmatch(last))


def _counts(code):
    """`(tests, runnable)` of the tokens `code`; Doubt on unbalanced brackets."""
    tests = runnable = 0
    for group in _attribute_groups(code):
        if not any(_is_test_attr(a) for a in group):
            continue
        tests += 1
        if not any(a[:1] == [_IGNORE_ATTR] for a in group):
            runnable += 1
    return tests, runnable


def _test_counts(text):
    """`(tests, runnable)` -- test attributes, and those not `#[ignore]`d.

    In doubt `(0, 0)`: 0 is never proof of "no test" for a reader of this
    count. `_runs_no_test` keeps a binary with 0 tests (and checks doubt
    before it counts), `pipeline/impact.py` `folded_form` plans rdeps for a
    module with 0 -- both fail closed.
    """
    code = _rust_code(text)
    if code is None:
        return 0, 0
    try:
        return _counts(code)
    except crate_root.Doubt:
        return 0, 0


def _hidden_tests(code):
    """Does `code` hold a shape that may make tests the count cannot see?"""
    item_scope = [True]     # per open bracket: may an item stand in it?
    for k, t in enumerate(code):
        if t in _BRACKETS:
            item_scope.append(t == "{" and k >= 2 and code[k - 2] == "mod"
                              and bool(_IDENT_RE.fullmatch(code[k - 1])))
            continue
        if t in (")", "]", "}"):
            if len(item_scope) == 1:
                raise crate_root.Doubt("unbalanced %s" % t)
            item_scope.pop()
            continue
        if t != "!" or k == 0:
            continue
        name = code[k - 1]
        if not _IDENT_RE.fullmatch(name) or name in _KEYWORDS:
            continue
        if name in ("macro_rules", "include"):
            return True
        if k + 1 < len(code) and code[k + 1] == "{" and name not in _EXPR_MACROS:
            return True
        start = k - 1           # the path `a::b::name` before the `!`
        while start >= 1 and code[start - 1] == "::":
            start -= 2 if start >= 2 and _IDENT_RE.fullmatch(code[start - 2]) else 1
        if item_scope[-1] and (start <= 0 or code[start - 1] in _ITEM_BEFORE):
            return True
    if len(item_scope) != 1:
        # m-hC-1: a file that ends inside a bracket is no proof of 0 tests.
        raise crate_root.Doubt("unclosed bracket at the end")
    return False


def _out_of_line_mods(code):
    """`(depth, attrs, name)` of every `[#[..]].. [pub[(..)]] mod <name>;`:
    the brace depth it stands at and the bodies of the attributes before it."""
    out, depth = [], 0
    for k, t in enumerate(code):
        if t == "{":
            depth += 1
        elif t == "}":
            depth -= 1
        elif (t == "mod" and k + 2 < len(code) and code[k + 2] == ";"
              and _IDENT_RE.fullmatch(code[k + 1])):
            j = k
            if j >= 1 and code[j - 1] == "pub":
                j -= 1
            elif j >= 1 and code[j - 1] == ")":
                o = _opening(code, j - 1)
                if o >= 1 and code[o - 1] == "pub":
                    j = o - 1
            attrs = []
            while j >= 2 and code[j - 1] == "]":
                o = _opening(code, j - 1)
                if o < 1 or code[o - 1] != "#":
                    break
                attrs.insert(0, code[o + 1:j - 1])
                j = o - 1
            out.append((depth, attrs, code[k + 1]))
    return out


def _module_texts(code, tests_dir):
    """The out-of-line modules of the tokens `code`, read one level deep; None in doubt.

    K1 fix round 2: the browser locks gh867/gh961 pull in
    `#[path = "support/display_colony.rs"] mod display_colony;`. Taken as
    doubt on sight, they stayed targets of the `tests` station and ran 0 tests
    there -- RED on every diff to their driver `.mjs` or to the lock. A module
    is looked up as `tests/<path>`, `tests/x.rs`, `tests/x/mod.rs`; one that
    is not found, or whose `#[path]` cannot be read, is doubt.
    """
    out = []
    for depth, attrs, name in _out_of_line_mods(code):
        # K1 fix round 3: a `mod` inside an inline module (`mod a { mod b; }`)
        # loads `tests/a/b.rs`, and a `cfg_attr(.., path = ..)` may swap the
        # file -- both are doubt.
        if depth != 0:
            return None
        if any(a[:1] == ["cfg_attr"] and "path" in a for a in attrs):
            return None
        paths = [a for a in attrs if a[:1] == ["path"]]
        if len(paths) > 1:
            return None
        if paths:
            a = paths[0]
            lit = a[2] if len(a) == 3 and a[1] == "=" else ""
            if len(lit) < 2 or lit[0] != '"' or lit[-1] != '"' or "\\" in lit:
                return None
            candidates = [lit[1:-1]]
        else:
            candidates = [name + ".rs", os.path.join(name, "mod.rs")]
        for rel in candidates:
            full = os.path.join(tests_dir, rel)
            if os.path.isfile(full):
                try:
                    with open(full, encoding="utf-8", errors="replace") as fh:
                        out.append(fh.read())
                except OSError:
                    return None
                break
        else:
            return None
    return out


def _runs_no_test(text, tests_dir=None):
    """True only when the binary SURELY runs 0 tests by default.

    In doubt it runs: a file the lexer doubts, no test attribute at all (a
    `harness = false` target, a helper compiled as its own binary), a local
    macro, an include, a module that is not found -- or a module (read one
    level deep from `tests_dir`) with test attributes or hidden shapes of its
    own.
    """
    code = _rust_code(text)
    if code is None:
        return False
    try:
        tests, runnable = _counts(code)
        if tests == 0 or runnable > 0:
            return False
        if _hidden_tests(code):
            return False
        if not _out_of_line_mods(code):
            return True
        modules = None if tests_dir is None else _module_texts(code, tests_dir)
        if modules is None:
            return False
        for mod in modules:
            mcode = _rust_code(mod)
            if (mcode is None or _counts(mcode)[0] or _hidden_tests(mcode)
                    or _out_of_line_mods(mcode)):
                return False
    except crate_root.Doubt:
        return False
    return True


_NO_TEST_CACHE = {}


def _drop_binaries_without_tests(repo, terms):
    """`terms` minus every `binary_id(=c::s)` whose source runs no test.

    Only the selected binaries are parsed, once per tree: the whole tree is
    ~1300 sources, and a plan names a handful.

    A term stands for every file of its unit: `<h>/main` is `main.rs` and each
    module it declares in a form that does not fold (`#[cfg]`, `#[path]`, ...).
    The term goes only when NONE of them runs a test (GH #1162 B-hC-1: one
    text per term let the last file judge the whole harness binary).
    """
    root = REPO_ROOT if repo is None else repo
    texts = None
    out = set()
    for t in terms:
        key = (root, t)
        if key not in _NO_TEST_CACHE:
            if texts is None:
                texts = {}
                for c, s, x in _test_sources(repo):
                    texts.setdefault(_binary_id(c, s), []).append((c, x))
            hits = texts.get(t) or []
            _NO_TEST_CACHE[key] = bool(hits) and all(
                _runs_no_test(x, os.path.join(root, "crates", c, "tests")) for c, x in hits)
        if not _NO_TEST_CACHE[key]:
            out.add(t)
    return out


def _folded_helper(repo, crate, harness, mod):
    """Is the folded module `<harness>/<mod>` anything but a pure test module?

    Other modules of the harness reach a helper by `use crate::<mod>;` (the
    harness_fold rewrite of `mod <mod>;`), `super::<mod>`, `mod`, `#[path]`, so
    their test names do not start `<mod>::` and the narrowed term misses them
    (W5 delta C1: `llm/mock_responses.rs` selected 0 tests and none of its 4
    users). Fail closed: the module's name as a whole word in any sibling, or a
    module without a test of its own, makes it a helper -- the caller then
    selects the whole harness binary.
    """
    hdir = os.path.join(REPO_ROOT if repo is None else repo, "crates", crate, "tests", harness)
    word = re.compile(r"(?<![A-Za-z0-9_])%s(?![A-Za-z0-9_])" % re.escape(mod))
    try:
        names = sorted(os.listdir(hdir))
        with open(os.path.join(hdir, mod + ".rs"), encoding="utf-8", errors="replace") as fh:
            own = fh.read()
    except OSError:
        return True
    if not re.search(r"#\[\s*(?:[\w:]+::)?test\b", own):
        return True
    for f in names:
        if not f.endswith(".rs") or f in ("main.rs", mod + ".rs"):
            continue
        with open(os.path.join(hdir, f), encoding="utf-8", errors="replace") as fh:
            if word.search(fh.read()):
                return True
    return False


def _sharers(repo, crate, stem):
    """Binaries that COMPILE `<stem>.rs` in as a module (shared test helpers).

    `mock_openai.rs` is its own binary and also `mod mock_openai;` in a dozen
    others; editing it changes all of them, so all of them are selected.
    """
    out = set()
    for c, s, text in _test_sources(repo):
        if c != crate or s == stem:
            continue
        # A harness `main.rs` names its shared helpers by path
        # (`#[path = "../support/road.rs"]`), not by a bare `"road.rs"`.
        if (("mod %s;" % stem) in text or ('"%s.rs"' % stem) in text
                or ('/%s.rs"' % stem) in text):
            out.add(_binary_id(c, s))
    return out


# How close a `read_dir(` may sit below the line that named the template root
# and still be read as iterating it. Three lines covers the idiomatic
# `let dir = templates_root();` / `let mut names = vec![];` / `for e in
# fs::read_dir(dir)` and stops well short of the next function.
CATALOGUE_WINDOW = 3

# Tokens that make a `read_dir`/`glob` line an iteration over the CATALOGUE
# rather than over some directory the test happens to hold.
CATALOGUE_TOKENS = ("templates_root", '"templates', "templates/")

# The SHIPPED catalogue root, however a test spells it: the helper that
# resolves it (`templates_root()`, `templates_dir()`) or the literal path that
# builds it (`PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")`
# -- any literal ENDING in `/templates`). A bare `"templates"` segment is
# deliberately NOT one: `tempdir.join("templates")` is a fixture library the
# test writes itself, and a shipped template that changes says nothing about
# it. This is the anchor of two rules -- the catalogue sweep below and
# `_names_the_shipped_root` -- and it was one spelling short in both: the two
# real sweeps of the tree name their root `templates_dir()` and
# `"../../templates"`, so neither was read as a sweep and neither travelled
# with any template diff (GH #713).
_SHIPPED_ROOT_RE = re.compile(r'templates_root\(|templates_dir\(|"[^"]*/templates"')

# A test's own recursive walker, `fn walk(dir, ..)` called on the root it just
# resolved. It is not in CATALOGUE_WALKS because the `read_dir(` is inside the
# helper, pages away from the line that names the root.
_LOCAL_WALK_RE = re.compile(r'\bwalk\w*\(')

# A template path built by a format string: `format!("templates/{dir}")`,
# `panic!("templates/{name} declares no contract")`. WHICH template it names is
# a runtime value, so such a file counts for EVERY template -- over-selection
# by construction, and the only alternative is missing it.
CATALOGUE_FORMAT = "templates/{"

# The catalogue ROOT as an expression -- `templates_root()`, `repo("templates")`,
# `root.join("templates")` -- but NOT one narrowed to a single template by a
# following `.join(...)`: `templates_root().join("talky")` copies one template
# and says nothing about the rest of the catalogue.
CATALOGUE_ROOT_RE = re.compile(
    r'(?:templates_root\(\)|"templates")(?!\s*\)?\s*\.join\()')

# A directory walk of any shape. `scan_templates_dir(` is in here because it
# IS the catalogue scan the colony itself uses.
CATALOGUE_WALKS = ("read_dir(", "glob(", "WalkDir", "scan_templates_dir(")

# A function signature: `fn templates_root() -> PathBuf` DEFINES the root, it
# does not point a walk at it.
_FN_RE = re.compile(r"\s*(?:pub\s+)?fn\s+[A-Za-z_][A-Za-z0-9_]*\s*[(<]")


_CATALOGUE_WIDE = {}


def _is_catalogue_wide(text):
    """`_catalogue_wide(text)`, once per source text: rule 4 asks it for every
    template name of the diff (65 names x 1 300 sources, 0.6 s per name)."""
    hit = _CATALOGUE_WIDE.get(text)
    if hit is None:
        hit = _CATALOGUE_WIDE[text] = _catalogue_wide(text)
    return hit


def _catalogue_wide(text):
    """Does this test enumerate the whole template catalogue?

    Such a test asserts something about EVERY shipped template, so any
    template diff must run it. Recognised in six forms:

    (a) it calls `shipped_templates(`;
    (b) one line carries `read_dir` together with a catalogue token
        (`templates_root`, `"templates`, `templates/`);
    (c) a `read_dir(` sits within `CATALOGUE_WINDOW` lines BELOW a line that
        named the catalogue root -- the two-statement form
        `let dir = templates_root(); ... for e in fs::read_dir(dir)`;
    (d) a `glob(` line spells `templates/*`;
    (e) it walks a directory anywhere AND names the catalogue ROOT as an
        expression somewhere -- `copy_tree(&repo("templates"), ..)`,
        `let root = templates_root();` followed 25 lines later by the helper
        that reads it. (b) and (c) see only one line at a time and miss both
        shapes, and missing them is UNDER-selection: such a test asserts over
        every shipped template. A root narrowed by a following `.join(...)`
        (`templates_root().join("talky")`) does NOT count -- that is one
        template -- and neither does the `fn templates_root()` signature;
    (g) a line names the SHIPPED root (`_SHIPPED_ROOT_RE`) and a walk of any
        shape -- `read_dir(`, `WalkDir`, or the file's own `walk(` -- follows
        within `CATALOGUE_WINDOW` lines. (b) and (c) know one spelling of the
        root and (e) another; `store_wake_failure_modes` (`let root = ...
        .join("../../templates"); walk(&root, ..)`) and
        `gh494_no_interior_marker_leaves_a_hive` (`fn templates_dir()`) read
        every shipped `config.json` through neither;
    (f) a line builds a template path with a format string --
        `format!("templates/{dir}")`, `panic!("templates/{name} ...")`. The
        name is a runtime value and cannot be resolved from the source, so the
        file counts for every template. Without this,
        `gh455_the_two_templates_ship` and `gh482_the_catalogue_says_no_by_name`
        fell out of every template diff -- under-selection.

    The earlier rule was "`read_dir(` anywhere AND `templates` anywhere in the
    file", which called 140 of 763 integration tests catalogue-wide. Almost all
    of those were a private `fn copy_cells(src, dst)` or `fn walk_rs(dir)` --
    a recursive helper over a directory PARAMETER -- in a file that elsewhere
    mentions its own template by name. Those files stay selected for their own
    template through the name rule; they were never enumerating the catalogue.
    """
    if "shipped_templates(" in text:
        return True
    lines = text.splitlines()
    for line in lines:
        if "read_dir" in line and (any(tok in line for tok in CATALOGUE_TOKENS)
                                   or _SHIPPED_ROOT_RE.search(line)):
            return True
        if "glob(" in line and "templates/*" in line:
            return True
        if CATALOGUE_FORMAT in line:
            return True
    for i, line in enumerate(lines):
        if "templates_root(" in line or "templates/" in line:
            for nxt in lines[i + 1:i + 1 + CATALOGUE_WINDOW]:
                if "read_dir(" in nxt:
                    return True
    if any(walk in text for walk in CATALOGUE_WALKS):
        for line in lines:
            if not _FN_RE.match(line) and CATALOGUE_ROOT_RE.search(line):
                return True
    for i, line in enumerate(lines):
        if _FN_RE.match(line) or not _SHIPPED_ROOT_RE.search(line):
            continue
        for nxt in lines[i + 1:i + 1 + CATALOGUE_WINDOW]:
            if any(walk in nxt for walk in CATALOGUE_WALKS) or _LOCAL_WALK_RE.search(nxt):
                return True
    return False


_TEMPLATE_REF_CACHE = {}


def _template_reference_re(name):
    """The shapes in which a test REFERENCES the template `name`.

    A bare `"<name>"` is NOT one of them. It used to be, and it made every
    template diff pull a third of the suite: `assistant`, `member`, `talky` and
    `display` are instance and cell names all over the example trees, and a
    tree that instantiates a cell called `assistant` says nothing about the
    template of that name. What counts is a path INTO the template, a versioned
    reference, or a declaration naming it as a template.

    Over-selection stays allowed -- these patterns deliberately match prose in
    comments too. To go back to the old behaviour, add `'"%s"' % n` as one more
    alternative below; that single line is the whole difference.
    """
    rx = _TEMPLATE_REF_CACHE.get(name)
    if rx is None:
        n = re.escape(name)
        parts = [
            # Anything but a name character ends the name: `/`, `"`, a space,
            # end of line -- and a closing backtick, because doc comments spell
            # it `templates/memory-hive` and a prose mention still counts.
            r'templates/%s(?![A-Za-z0-9_-])',
            r'"%s@',                           # a versioned reference
            r'join\("%s"\)',                   # a path built from the name
            r'\\?"template\\?"\s*:\s*\\?"%s(?=[@"\\])',  # a JSON declaration
            r'template\s*=\s*"%s(?=[@"])',     # attribute / TOML form
            r'template\("%s"\)',               # a helper call
        ]
        rx = _TEMPLATE_REF_CACHE[name] = re.compile(
            "|".join(part % n for part in parts), re.MULTILINE)
    return rx


# A test that reads a SHIPPED template without ever spelling its path. It
# resolves the catalogue root once -- `fn templates_root() -> PathBuf` over
# `CARGO_MANIFEST_DIR/../../templates` -- and joins a RUNTIME value onto it:
# `fn shipped(rel) { templates_root().join(rel) }`, called as
# `shipped("assistant/config.json")`. The literal `templates/assistant` is
# nowhere in the file, so `_template_reference_re` does not see it; the diff
# path `templates/assistant/config.json` is nowhere either, so rule 8 does
# not; and the root is narrowed by a `.join(`, so `_is_catalogue_wide` does
# not. `gh529_the_menu_merges_every_answerers_declarations` and
# `gh561_the_pack_rides_a_v_lane` read exactly that file and fell out of the
# strand gate that changed it -- both went red on the integration branch
# afterwards (GH #713).
#
# The root must be the SHIPPED one: `tempdir.join("templates").join(name)`
# builds a fixture library the test writes itself, and a shipped template that
# changes says nothing about it. So only a root spelled `templates_root(` or a
# literal path ENDING in `/templates` ("../../templates") counts -- a bare
# `"templates"` segment does not.
# What is left of the path below the root is not the resolver's business:
# `join(rel)`, `join(name)` and `join("memory-hive/embed/config.json")` are the
# same reach through the same helper. Requiring a RUNTIME join kept
# `gh204_the_shipped_embedding_generation_agrees` --
# `templates_root().join("memory-hive/embed/config.json")` -- out of that
# file's own diff, which is the defect of GH #713 one spelling further on.


def _names_the_shipped_root(text):
    """Does this test reach into the shipped catalogue at all?"""
    return bool(_SHIPPED_ROOT_RE.search(text))


_ROOT_RELATIVE_CACHE = {}


def _root_relative_reference_re(name):
    """The template `name` as it arrives at such a helper, root-relative.

    Two shapes, and only these two:

        "<name>/..."        a path INTO the template -- `"assistant/config.json"`
        ([,) "<name>"       the bare name HANDED to something -- `shipped(
                            "assistant", FILES)`, `&["collector"]`

    The second shape is why the bare name may not count on its own anywhere
    else: `{"origin": "assistant"}` is an LLM role, and it is in a third of
    the suite. A delimiter in front of it is a cheap filter, not a proof --
    `turn("assistant", ..)` is that same role handed to a helper, and
    `cfg["tools"]` is a Rust index. It may over-select; what it buys is that
    the JSON-body form, which is most of them, drops out: measured on the tree
    that is 3 binaries selected for `templates/assistant` instead of 18.
    """
    rx = _ROOT_RELATIVE_CACHE.get(name)
    if rx is None:
        n = re.escape(name)
        rx = _ROOT_RELATIVE_CACHE[name] = re.compile(
            r'"%s/|[(,\[]\s*&?"%s"' % (n, n))
    return rx


_TEMPLATE_READERS = {}


def _template_readers(repo, name):
    """`{(crate, unit)}` -- the `_test_sources` units that reference the
    template `name` (rule 4): a path into it, a versioned reference, a
    declaration, the whole catalogue, or a root-relative path in a test that
    names the shipped root. Once per tree and name (0.9 s a name on the tree;
    the rules-4/5 guard asks it twice for each of 65 names, tb-s1c)."""
    root = REPO_ROOT if repo is None else repo
    sources = _reader_sources(repo)
    hit = _TEMPLATE_READERS.get((root, name))
    if hit is not None and hit[0] is sources:
        return hit[1]
    ref = _template_reference_re(name)
    rel = _root_relative_reference_re(name)
    out = set()
    for crate, stem, text in sources:
        if ref.search(text) or _is_catalogue_wide(text):
            out.add((crate, stem))
        elif _names_the_shipped_root(text) and rel.search(text):
            out.add((crate, stem))
    out = frozenset(out)
    _TEMPLATE_READERS[(root, name)] = (sources, out)
    return out


def _tests_referencing_template(repo, name):
    """Binaries of rule 4: the rule-3 set (`_unit_users`) of every unit that
    references the template -- a helper references it for its users, like a
    rule-8 reader (welle-tb S1b OR-TB.S1b.34: 2215 template/example reader
    pairs of the tree ran only the referencing module, not its users)."""
    return {t for c, u in _template_readers(repo, name) for t in _unit_users(repo, c, u)}


def _path_literals(path):
    """The spellings under which a test may name `path`.

    `docs/X.en.md` also travels as `docs/X.md`: the export maps the English
    bytes onto the public name, and tests in this tree cite both (8 files name
    `docs/config.md`, one names `docs/config.en.md`). Selecting only the spelling
    the diff happens to use would miss the readers of the other.
    """
    out = [path]
    if path.endswith(".en.md"):
        out.append(path[:-len(".en.md")] + ".md")
    return out


def _tests_reading_path(repo, path):
    """Binaries that name `path` as a literal -- rule 8.

    A test that READS a file is that file's drift lock. `gh466_the_seed_gate_is
    _wired.rs` opens `.github/workflows/ci.yml` and asserts a wording; a diff to
    that workflow that did not run it left the lock unrun. The rule generalises
    the template and example rules to every path outside `crates/`: docs, plans,
    workflows, CONTRIBUTING.md, README.md, workshop material.
    A path a comment names reads nothing (`_reader_text`, #1160, V2-R1-1);
    in code it reads. A walk of an ancestor directory is the runtime rule's
    (`_tests_reading_at_runtime`), never a reason to read a comment.
    """
    literals = _path_literals(path)
    out = set()
    for crate, stem, text in _reader_sources(repo):
        if any(lit in text for lit in literals) \
                and any(lit in _reader_text(text) for lit in literals):
            out |= _unit_users(repo, crate, stem)   # a helper reads for its users (M-8)
    return out


# --- rule 8 runtime: paths a test reads without naming them (GH #1138) ------
#
# A coverage map sees the Rust lines a test executes, never the files it opens.
# Literal rule 8 sees a path the test NAMES. Between the two lies the runtime
# read: a directory the test walks, a generator it runs that reads its own
# sources, a crate fixture built from `CARGO_MANIFEST_DIR`. The W6 replay case
# R18 fell through exactly there: `librarian_seed_corpus` runs the seed
# generator, which reads `docs/cell-types.en.md`, and no test names that page.
#
# Three shapes, all derived from the test's text or this table, none guessed:
#   dir     a string literal of the test is an ANCESTOR directory of the path
#           (`root.join("docs")`, `"/tests/fixtures/x.json"` read relative to
#           the repo or, for `crates/<c>/tests/**`, to the crate; relative
#           to the repo at any depth, `crates` included, and then for EVERY
#           crate file below, `.rs` and src/ too -- a crate-relative literal
#           only for the non-`.rs` files of tests/). Paths under
#           templates/ and examples/ stay with rules 4 and 5: their readers are
#           selected by reference shape, and a walk of `templates` would pull
#           the catalogue-wide tests that rule 4 already names.
#   script  the test names a script of SCRIPT_READS (it runs it), and the
#           path matches one of that script's source globs.
#   unread  a non-`.rs` file under `crates/<c>/tests/` no test names in any of
#           these ways: the reader is unknown, so `rdeps(<c>)` -- fail-closed.
#           A repo-relative walk of `crates`, `crates/<c>` or
#           `crates/<c>/tests` plans along but names no file: beside it the
#           file stays unread (tb-s1c M-1).

# `(script, globs)`: a test that names `script` runs it, and the script reads
# every path the globs match. The globs are the ones the gate already keeps for
# the script (CORPUS_SOURCES is a checked superset of the seed generator's).
SCRIPT_READS = (
    ("workshop/tools/build_librarian_seed.py", CORPUS_SOURCES),
)

# `{dir: {unit: (suffixes, depth)}}`: what each NAMED reader of `dir` reads
# below it -- a property of the reader, not of the directory (fix round stapel,
# I1/I2). `depth`: "import" a PYTHONPATH root, the modules an `import` reaches
# (`_importable`: every name on the way a Python identifier -- `llm-conformance/`
# and `display-lab/` are no package; their modules, read by path by their own
# unittest stations, planned every reader of the root and the browser run
# behind it: OR-TM-8, K15, AN3-FIX 6bd5216f1, crit 10800 s on build03); "top"
# the directory's own files (one `read_dir`); "any" every depth, a hyphened
# directory included. The station is the reader's own (rule 11): a browser
# unit runs in `browser:display`, every other in `tests`.
#   gh661       imports the generators it runs from `workshop/tools` -> `tests`
#   708         `every_driver_keeps_the_answer_of_an_evidence_shot`: every
#               `.mjs` of the top level -> `tests`
#   744         every `.mjs`, node_modules aside -> `browser:display`
#   gh1209      `every_browser_driver_goes_through_the_switch`: every `.mjs`/
#               `.js`, node_modules aside -> `browser:display`
# A reader of `dir` the table does not name reads every file below it, any
# suffix, any depth (fail-closed, like a dir outside the table): a new walker
# runs for every diff there until its entry says what it keeps.
RUNTIME_DIR_READS_ONLY = {
    "workshop/tools": {
        "misc_5/gh661_the_briefing_states_the_operator_set_rule": ((".py",), "import"),
        "708_the_client_draws_the_tap_before_the_pass": ((".mjs",), "top"),
        "744_a_late_patch_does_not_take_the_drawing_back": ((".mjs",), "any"),
        "gh1209_no_stage_draws_a_filter_browser": ((".mjs", ".js"), "any"),
    },
}


def _importable(rel):
    """Does an `import` from a PYTHONPATH root reach the file `rel` (relative to
    the root)? Every directory between is a package and the file a module only
    when each name is a Python identifier."""
    parts = rel.split("/")
    return all(p.isidentifier() for p in parts[:-1]) \
        and parts[-1].rsplit(".", 1)[0].isidentifier()

_STRING_LITERAL_RE = re.compile(r'"([A-Za-z0-9_.@+\-/]+)"')
# The same literals, plus (group 2) the next non-space character after each, ""
# at the end: a lookahead, so it consumes nothing and finds exactly the
# literals `_STRING_LITERAL_RE` finds. `_bare_root` needs that character for
# every bare word; sliced off the text per literal it cost 0.8 s a build
# (205 545 literals), and a gate builds the index twice -- in the runner and
# in the resolver subprocess (tb-s1c-d, #1177).
_STRING_LITERAL_NEXT_RE = re.compile(r'"([A-Za-z0-9_.@+\-/]+)"(?=\s*(\S?))')
_QUOTED_RE = re.compile(r'"([^"]*)"')
_RUNTIME_INDEX = {}


# A literal followed by an existence probe (`"plans").is_dir()`) reads nothing.
_PROBE_RE = re.compile(r"\s*\)\s*\.\s*(?:is_dir|is_file|exists)\s*\(\s*\)")


def _bare_root(text, m, root, after, isdir, lists):
    """Is the bare word at `m` a path the test reads? Only where a path goes:
    the argument of a call (`root.join("docs")`), or a list whose every entry
    is a directory of the tree (`["templates", "examples", "docs"]`) -- not a
    JSON key or value (`"predicate": "plans"`, `["user", "plans"]`), not a
    constant (`MARKER: &str = "workshop"`).

    `after` is the next non-space character behind the literal (group 2 of
    `_STRING_LITERAL_NEXT_RE`), `isdir` a `{word: bool}` memo of one build,
    `lists` a `{(lo, hi): bool}` memo of one text (every entry of a list asks
    the same list). The character before is found by stepping back over
    whitespace, never by slicing the text (tb-s1c-d)."""
    if after not in (")", "]", ","):
        return False
    i = m.start() - 1
    while i >= 0 and text[i].isspace():
        i -= 1
    before = text[i] if i >= 0 else ""
    if before == "(" and after == ")":
        return True
    if before not in ("[", ",") or after == ")":
        return False
    lo, hi = text.rfind("[", 0, m.start()), text.find("]", m.end())
    if lo < 0 or hi < 0 or hi - lo > 400:
        return False
    hit = lists.get((lo, hi))
    if hit is None:
        seg = text[lo:hi]
        hit = False
        if "{" not in seg and "}" not in seg and ";" not in seg:
            words = _QUOTED_RE.findall(seg)
            for w in words:
                if w not in isdir:
                    isdir[w] = os.path.isdir(os.path.join(root, w))
            hit = bool(words) and all(isdir[w] for w in words)
        lists[(lo, hi)] = hit
    return hit


def _dir_literal(lit):
    """`lit` as a relative path: leading `/`, `./` and `../` off, trailing `/` off."""
    s = lit.lstrip("/")
    while s.startswith(("./", "../")):
        s = s[s.index("/") + 1:]
    s = s.rstrip("/")
    return s if s and s not in (".", "..") else None


def _unit_dir(crate, unit):
    """The repo-relative directory of the FILE of a `_test_sources` unit."""
    return "crates/%s/tests%s" % (crate, "/" + unit.rsplit("/", 1)[0] if "/" in unit else "")


def _resolve_dotdot(lit, crate, unit, root):
    """`lit` (a `_dir_literal` with a `..` segment) lexically normalised: the
    index keys it reads under, or None when no base pins it down.

    No climb above its own start (`sub/../data.json`): the normalised literal,
    read like any other (`_dir_literal`). A climb above its start
    (`sub/../../x`): where it lands depends on the base it is joined to, so it
    lands once per base of rule 8 -- the crate, its tests/, the reader file's
    directory -- as a repo-relative key; a base the climb leaves (the repo
    root always) is outside the repository. None -- the reader reads ANY
    path, fail-closed -- when a `..` follows a symlink of the tree (it climbs
    out of the link's target), or the climb leaves every base, or lands on the
    repository root itself (tb-s1c-b, #1177)."""
    segs = lit.split("/")
    last = max(i for i, s in enumerate(segs) if s == "..")
    bases = ["crates/%s" % crate, "crates/%s/tests" % crate, _unit_dir(crate, unit)]
    for j in range(1, last + 1):
        pre = "/".join(segs[:j])
        if any(os.path.islink(os.path.join(root, b, pre)) for b in [""] + bases):
            return None
        if _linked_anywhere(root, pre):   # GH #1183: every join base
            return None
    norm = posixpath.normpath(lit)
    parts = norm.split("/")
    climb = len(parts) - len(list(itertools.dropwhile(lambda s: s == "..", parts)))
    if not climb:
        norm = _dir_literal(norm)
        return [norm] if norm else []
    tail = "/".join(parts[climb:])
    keys = []
    for b in bases:
        bp = b.split("/")
        if climb > len(bp):
            continue   # out of the repository from this base
        key = "/".join(bp[:len(bp) - climb] + ([tail] if tail else []))
        if not key:
            return None   # the repository root itself: every path
        keys.append(key)
    return sorted(set(keys)) or None


def _runtime_index(repo):
    """`{normalised literal: {(crate, unit)}}` over every test source, once per tree."""
    return _runtime_tables(repo)[0]


def _runtime_spelled(repo):
    """`{path: {(crate, unit)}}` -- the keys of `_runtime_index` a `..`
    spelling produced (`docs/sub/../x.md` -> `docs/x.md`): the literal rule 8
    looks for the path as written and cannot see them, so the runtime rule
    plans them as a whole-file read too."""
    return _runtime_tables(repo)[1]


def _runtime_any(repo):
    """`{(crate, unit)}` -- readers with a `..` literal no base resolves
    (`_resolve_dotdot` None): they read every path, fail-closed."""
    return _runtime_tables(repo)[2]


# One build of the rule-8 index per gate (tb-s10 w6, A1 11 L-8): the resolver
# subprocess (`pipeline.plan.default_resolver`) and the runner (graph, impact)
# each built it. The runner names a file in this variable; whichever side
# builds first writes the tables there under `_runtime_key`, the other loads
# them. A key that does not fit, a directory a bare word asked about that
# changed, or a torn file: built anew (fail-closed).
RULE8_INDEX_ENV = "GATE_PLAN_RULE8_INDEX"
_CODE_DIGEST = []


def _runtime_key(root, sources):
    """What the index of `root` is built from: this code, the tree, HEAD (the
    links of `_tree_links`) and every reader text."""
    if not _CODE_DIGEST:
        with open(__file__, "rb") as fh:
            _CODE_DIGEST.append(hashlib.sha256(fh.read()).hexdigest())
    h = hashlib.sha256()
    for crate, unit, text in sources:
        h.update(("%s\0%s\0%d\0" % (crate, unit, len(text))).encode("utf-8", "surrogatepass"))
        h.update(text.encode("utf-8", "surrogatepass"))
    return (_CODE_DIGEST[0], os.path.realpath(root), repr(_head_stamp(root)), h.hexdigest())


def _runtime_load(path, key, root):
    """The tables in `path` when they were built under `key` and every
    directory a bare word asked about still answers the same, else None."""
    try:
        with open(path, "rb") as fh:
            got = pickle.load(fh)
        if got["key"] != key:
            return None
        if any(os.path.isdir(os.path.join(root, w)) != v for w, v in got["isdir"].items()):
            return None
        return got["tables"]
    except Exception:  # noqa: BLE001 -- any unreadable file: build anew
        return None


def _runtime_store(path, key, isdir, tables):
    tmp = "%s.%d.tmp" % (path, os.getpid())
    try:
        with open(tmp, "wb") as fh:
            pickle.dump({"key": key, "isdir": isdir, "tables": tables}, fh,
                        protocol=pickle.HIGHEST_PROTOCOL)
        os.replace(tmp, path)
    except OSError:
        pass   # no hand-over: the other side builds itself


def _runtime_tables(repo):
    root = REPO_ROOT if repo is None else repo
    idx = _RUNTIME_INDEX.get(root)
    if idx is None or idx[0] is not _reader_sources(repo):
        sources = _reader_sources(repo)
        path = os.environ.get(RULE8_INDEX_ENV)
        stamp = _runtime_key(root, sources) if path else None   # not `key`: the loop below uses it
        tables = _runtime_load(path, stamp, root) if path else None
        if tables is not None:
            idx = _RUNTIME_INDEX[root] = (sources, tables)
            return idx[1]
        lits = collections.defaultdict(set)
        spelled = collections.defaultdict(set)
        anywhere = set()
        isdir = {}
        links = _tree_links(root)
        for crate, unit, text in sources:
            lists = {}
            for m in _STRING_LITERAL_NEXT_RE.finditer(text):
                word, after = m.group(1, 2)
                if "/" not in word:
                    # a bare word is its own `_dir_literal` (`.`/`..`: none) and
                    # counts only as a call argument or a list entry
                    if after not in (")", "]", ",") or word in (".", "..") \
                            or not _bare_root(text, m, root, after, isdir, lists) \
                            or _PROBE_RE.match(text, m.end()):
                        continue
                    lits[word].add((crate, unit))
                    continue
                lit = _dir_literal(word)
                if not lit or _PROBE_RE.match(text, m.end()):
                    continue   # `root.join("plans").is_dir()` asks, it reads nothing
                if ".." in lit.split("/"):
                    keys = _resolve_dotdot(lit, crate, unit, root)
                    if keys is None:
                        anywhere.add((crate, unit))
                    for key in keys or ():
                        lits[key].add((crate, unit))
                        spelled[key].add((crate, unit))
                    continue
                if "/" not in lit and not _bare_root(text, m, root, after, isdir, lists):
                    continue
                lits[lit].add((crate, unit))
                for key in _linked_keys(root, lit, crate, links):
                    lits[key].add((crate, unit))
                    spelled[key].add((crate, unit))
        idx = _RUNTIME_INDEX[root] = (sources, (lits, spelled, anywhere))
        if path:
            _runtime_store(path, stamp, isdir, idx[1])
    return idx[1]


_LINK_HEADS = {}


def _link_heads(links):
    """Every directory on the way to a link of `links`, once per link set."""
    heads = _LINK_HEADS.get(links)
    if heads is None:
        _LINK_HEADS.clear()   # one tree's set at a time, no growth
        heads = _LINK_HEADS[links] = frozenset(
            "/".join(link.split("/")[:j]) for link in links
            for j in range(1, link.count("/") + 1))
    return heads


def _linked_keys(root, lit, crate, links):
    """The repo-relative paths the literal `lit` (no `..`) reads under when a
    directory on its way is a symlink of the tree (`links`, `_tree_links`):
    joined to each base of rule 8 -- the repo, the crate, its tests/ -- and
    resolved to the link's target. Re-check tb-s1e h2 L-d: `docs/lnk/x.md`
    with `docs/lnk -> far` stayed lexical, a diff at `docs/far/x.md` planned
    nothing. A target out of the repository is no key."""
    keys = set()
    heads = _link_heads(links)
    for base in ("", "crates/%s/" % crate, "crates/%s/tests/" % crate):
        rel, pre = base + lit, ""
        for seg in rel.split("/"):
            pre = pre + "/" + seg if pre else seg
            if pre in links or pre not in heads:
                break   # ~12k literals, ~300 links: mostly one lookup each
        if pre not in links:
            continue
        top = os.path.realpath(root)
        real = os.path.relpath(os.path.realpath(os.path.join(root, rel)), top).replace(os.sep, "/")
        if real != rel and real not in (".", "..") and not real.startswith("../"):
            keys.add(real)
    return keys


def is_catalogue_top(path):
    """A top-level file of templates/ (`templates/README.md`): no template owns
    it, so rules 4/5 do not plan it and the dir shape leaves templates/ to
    them. A test that enumerates the catalogue (`_is_catalogue_wide`) walks
    the directory and reads it -- gh408 sweeps every `.md` below templates/;
    until V2-R1-1 its only edge to the README was a `//!` paragraph."""
    return path.startswith("templates/") and path.count("/") == 1


def _runtime_dir_applies(lit, path, unit=None):
    """Does the walk of the directory `lit` by the test unit `unit` read `path`?
    (the dir shape's filter; RUNTIME_DIR_READS_ONLY per named reader, every
    file below for a reader the table does not name -- `unit` None too)"""
    if path.startswith(("templates/", "examples/")):
        return False
    rule = RUNTIME_DIR_READS_ONLY.get(lit, {}).get(unit)
    if rule is None:
        return True
    suffixes, depth = rule
    below = path[len(lit) + 1:] if path.startswith(lit + "/") else None
    if not path.endswith(suffixes):
        return False
    if below is None or depth == "any":
        return True
    if depth == "top":
        return "/" not in below
    return _importable(below)


def _runtime_roots(repo, targets):
    """`{(crate, unit): {path}}` -- the paths of `targets` (repo-relative: the
    tree's directories, and its crate test files) each test unit names in the
    dir shape's terms: relative to the repo, or (for the crate's own tests/)
    to the crate or to its tests/. The pipeline graph hangs `reads` edges on
    them (`dir:<d>`, or `file:<f>` for a crate fixture named whole)."""
    out = collections.defaultdict(set)
    for lit, units in _runtime_index(repo).items():
        for crate, unit in units:
            # the literal as written is repo-relative at any depth (`crates`,
            # `crates/<c>`, `crates/<c>/tests` walk every fixture below); the
            # crate-relative forms only below the crate's tests/ -- the bare
            # `tests` is every binary's home (tb-s1c, #1177)
            for d, crate_rel in ((lit, False), ("crates/%s/%s" % (crate, lit), True),
                                 ("crates/%s/tests/%s" % (crate, lit), True)):
                if d in targets and (not crate_rel or d.count("/") >= 3):
                    out[(crate, unit)].add(d)
    return out


def _ancestors(rel):
    parts = rel.split("/")
    return ["/".join(parts[:i]) for i in range(len(parts) - 1, 0, -1)]


def _runtime_readers(repo, path):
    """`{(crate, unit): reason}` -- the test units that read `path` at runtime.

    `(None, crate)` stands for `rdeps(crate)`: the `unread` shape. The units
    are those of `_test_sources`; the pipeline graph hangs its `reads` edges
    on them, `_tests_reading_at_runtime` turns them into filterset terms."""
    out = _runtime_readers_named(repo, path)
    # a `..` literal no base resolves reads every path (tb-s1c-b); after the
    # `unread` fallback, which asks whether a NAMING reader exists
    for unit in sorted(_runtime_any(repo)):
        out.setdefault(unit, "rule:8 runtime unresolved-dotdot file:%s" % path)
    return out


def _runtime_readers_named(repo, path):
    lits = _runtime_index(repo)
    out = {}

    def add(units, reason):
        for unit in sorted(units):
            out.setdefault(unit, reason)

    crate = _crate_of(path)
    if crate:
        rest = path[len("crates/%s/" % crate):]
        fixture = rest.startswith("tests/") and not path.endswith(".rs")
        if fixture:
            # relative to the crate (`CARGO_MANIFEST_DIR`) or to its tests/ --
            # the crate's own tests only; the bare `tests` is every binary's
            # home, and a crate-relative bare word is mostly a cell name
            # (`"voice"` -> tests/voice), so never for a `.rs` file
            rel_tests = rest[len("tests/"):]
            cands = [rest, rel_tests] + [a for a in _ancestors(rest) if a != "tests"] \
                + _ancestors(rel_tests)
            for lit in cands:
                if lit in lits:
                    add({(c, u) for c, u in lits[lit] if c == crate},
                        "rule:8 runtime dir:%s file:%s" % (lit, path))
        # relative to the repo, from any crate, at any depth, for EVERY crate
        # file: a test that walks `crates`, `crates/<c>` or `crates/<c>/src`
        # reads what lies below, `.rs` included (tb-s1c, #1177: misc_5/gh523
        # reads `.rs`/`.json`/`.manifest` below `crates`, gh_1/gh230 every
        # `.rs`; first they ran for none of the 83 json fixtures, then for no
        # `.rs` or src file of a crate they do not depend on -- 6 813 pairs)
        known = bool(out)
        for lit in [path] + _ancestors(path):
            if lit in lits:
                add(lits[lit], "rule:8 runtime dir:%s file:%s" % (lit, path))
                # a walk of `crates`, `crates/<c>` or `crates/<c>/tests` plans
                # along but says nothing of this one file (gh523 reads only
                # json|rs|manifest, gh230 skips tests/): with only such a
                # reader the file stays `unread` (tb-s1c M-1, KR-TB-1 (c))
                known = known or lit.count("/") >= 3
        if fixture and not known:
            out[(None, crate)] = "rule:8 runtime unread file:%s" % path
        return out
    # a whole file spelled with `..` (`docs/sub/../x.md`): the literal rule
    # looks for the path as written and misses it (tb-s1c-b)
    add(_runtime_spelled(repo).get(path, ()), "rule:8 runtime dotdot file:%s" % path)
    for lit in _ancestors(path):
        if lit in lits:
            # each reader by what IT reads (RUNTIME_DIR_READS_ONLY); a driver of
            # the class `display_browser` as well: its class plans the locks,
            # the walkers join them (stapel c2), 708 runs in `tests` (I1)
            add({(c, u) for c, u in lits[lit] if _runtime_dir_applies(lit, path, u)},
                "rule:8 runtime dir:%s file:%s" % (lit, path))
    if is_catalogue_top(path):
        add({(c, u) for c, u, text in _reader_sources(repo) if _is_catalogue_wide(text)},
            "rule:8 runtime catalogue file:%s" % path)
    for script, globs in SCRIPT_READS:
        if any(_matches(path, g) for g in globs):
            add({(c, u) for c, u, text in _reader_sources(repo)
                 if script in text and script in _reader_text(text)},
                "rule:8 runtime script:%s file:%s" % (script, path))
    return out


def _tests_reading_at_runtime(repo, path):
    """`{term: reason}` -- rule 8 runtime as filterset terms.

    Additive to the literal rule 8 (`_tests_reading_path`): a term both name
    keeps both reasons. See the block above for the three shapes. A reading
    unit stands for every binary its file runs in (`_unit_users`)."""
    out = {}
    for (c, u), r in _runtime_readers(repo, path).items():
        for t in ({"rdeps(%s)" % u} if c is None else sorted(_unit_users(repo, c, u))):
            out.setdefault(t, r)
    return out


_UNIT_USERS = {}


def _unit_users(repo, crate, unit):
    """The terms rule 3 plans for the FILE of a `_test_sources` unit: the unit,
    the whole harness for a folded helper (`_folded_helper`), and every binary
    that compiles the file in (`_sharers`).

    Rule 8 finds a reader by its unit. A helper module reads for every test of
    its harness that calls it, a `#[path]`-shared or `mod`-shared file for
    every binary it is compiled into -- the unit's own term ran only the
    reader's tests and left its users silently unrun (welle-tb S1b M-8:
    `support_14b.rs` reads `tests/fixtures/14a-tool-loop/README.md` for 20
    binaries, `misc_2/gh203_*` reads `docs/README.md` as a helper of misc_2).
    """
    root = REPO_ROOT if repo is None else repo
    sources = _test_sources(repo)
    hit = _UNIT_USERS.get((root, crate, unit))
    if hit is not None and hit[0] is sources:
        return hit[1]
    if _is_helper(repo, crate, unit):
        # no binary of its own: what it reads, it reads for its includers (F4)
        users = _helper_users(repo, crate, unit)
        _UNIT_USERS[(root, crate, unit)] = (sources, users)
        return users
    stem = unit
    users = set()
    if "/" in unit:
        harness, mod = unit.split("/", 1)
        if mod != "main" and _folded_helper(repo, crate, harness, mod):
            stem = harness + "/main"
        if mod == "main":
            # The unit `<h>/main` is no single file: `_test_sources` also names
            # every module main.rs does not declare at its top level `<h>/main`
            # (`_folded_unit`), and rule 3 plans such a FILE with its own
            # sharers (`#[path = "../h/x.rs"]` in another harness) -- the unit's
            # sharers alone left them unrun (tb-s1c m-16, family P12).
            for m in _undeclared_modules(repo, crate, harness):
                users |= _sharers(repo, crate, "%s/%s" % (harness, m))
    users |= {_binary_id(crate, stem)} | _sharers(repo, crate, unit)
    _UNIT_USERS[(root, crate, unit)] = (sources, users)
    return users


def _undeclared_modules(repo, crate, harness):
    """The modules of the harness `crates/<crate>/tests/<harness>/` that its
    main.rs does not declare at the top level (`_folded_unit` -> `<h>/main`)."""
    hdir = os.path.join(REPO_ROOT if repo is None else repo, "crates", crate, "tests", harness)
    try:
        with open(os.path.join(hdir, "main.rs"), encoding="utf-8", errors="replace") as fh:
            main = fh.read()
        names = sorted(os.listdir(hdir))
    except OSError:
        return []
    return [f[:-3] for f in names if f.endswith(".rs") and f != "main.rs"
            and _folded_unit(main, harness, f[:-3]) == harness + "/main"]


def _example_readers(repo, name):
    """`{(crate, unit)}` -- the `_test_sources` units that name `examples/<name>` (rule 5)."""
    needle = "examples/%s" % name
    return {(crate, stem) for crate, stem, text in _reader_sources(repo) if needle in text}


def _tests_referencing_example(repo, name):
    """Binaries of rule 5: the rule-3 set of every unit that names the example."""
    return {t for c, u in _example_readers(repo, name) for t in _unit_users(repo, c, u)}


# --- the test filter --------------------------------------------------------

def test_file_unit(path, repo=None):
    """`(crate, unit)` rule 3 runs for the integration-test file `path`: the
    flat stem for `tests/<stem>.rs`; for `tests/<harness>/<mod>.rs` (#1119)
    `<harness>/<mod>` when main.rs folds the module, else `<harness>/main` --
    also for main.rs itself and for a helper module, whose users run too
    (W5 delta C1). `_binary_id(crate, unit)` is its nextest term. The one
    mapping of a test file to its unit; `pipeline/pre.py` plans with it too
    (GH #1130)."""
    crate, parts = _crate_of(path), path.split("/")
    # `tests/<stem>.rs` flat, `tests/<harness>/<stem>.rs` folded (#1119)
    stem = (parts[3].removesuffix(".rs") if len(parts) == 4
            else parts[3] + "/" + parts[4].removesuffix(".rs"))
    if len(parts) == 5 and not stem.endswith("/main"):
        main_p = os.path.join(REPO_ROOT if repo is None else repo, *parts[:4], "main.rs")
        if os.path.isfile(main_p):
            with open(main_p, encoding="utf-8", errors="replace") as fh:
                stem = _folded_unit(fh.read(), parts[3], parts[4].removesuffix(".rs"))
            if not stem.endswith("/main") and _folded_helper(
                    repo, crate, parts[3], parts[4].removesuffix(".rs")):
                stem = parts[3] + "/main"  # a helper: its users run too (W5 delta C1)
    return crate, stem


def _term_rules(files, repo):
    """The ONE rule source behind `test_terms` and `test_select` (W6 x W3).

    `[(label, path, reason, terms)]` for rules 1-5, 8 and 10 over the normalised,
    non-empty diff `files`: `label` is the class `test_select` prints as
    `<label>: <path>`, `reason` the `rule:<n> ...` string `test_terms` keeps
    per term, `terms` the set that firing brought in. Rule 9 (drop binaries
    without tests) is NOT applied here -- both views apply it to their union.
    """
    out = []
    if "workspace" in classify(files):
        for p in files:
            if "workspace" in _classes_of_path(p):
                out.append(("workspace", p, "rule:1 file:%s" % p, {"all()"}))
        return out
    # Rule 12 first, so `--explain` never caps a pin away (REASON_CAP).
    out.extend(_required_pin_rules(files, repo))
    for crate in crates_of(files, repo):
        for p in files:
            if "rust_src" in _classes_of_path(p) and _crate_of(p) == crate:
                out.append(("rust_src", p, "rule:2 file:%s" % p, {"rdeps(%s)" % crate}))
    for p in files:
        if "rust_test" not in _classes_of_path(p):
            continue
        crate = _crate_of(p)
        parts = p.split("/")
        # The FILE's own unit, before any rewrite to `<h>/main`: a foreign
        # harness `#[path]`s the file, not the harness (re-check N-C1).
        file_stem = (parts[3].removesuffix(".rs") if len(parts) == 4
                     else parts[3] + "/" + parts[4].removesuffix(".rs"))
        stem = test_file_unit(p, repo)[1]
        # A path the diff names may be GONE -- `git diff --name-only`
        # lists deletions. An id for a binary that does not exist is not
        # an over-selection, it is a filterset nextest refuses to parse.
        # Rules 4, 5 and 8 are safe without this check: they derive every
        # id from a file they just READ out of the tree.
        exists = _test_binary_exists(repo, crate, stem)
        if exists:
            out.append(("rust_test", p, "rule:3 file:%s" % p, {_binary_id(crate, stem)}))
        # Sharers come from the sources that compile the file in, not from the
        # file's own binary: `tests/h5_support/mod.rs` and `tests/gh927/*.rs`
        # lie in directories without main.rs (no binary) and planned NOTHING
        # although misc_8 (6 units) and round_2 `#[path]` them in (tb-s1c m-17,
        # the full rule-8 guard; family P13).
        sharers = _sharers(repo, crate, file_stem)
        if sharers:
            out.append(("rust_test", p, "rule:3 shares file:%s" % p, set(sharers)))
    for p in files:
        # Rule 10: a crate file no rule above can assign -- the crate and its
        # reverse dependencies, like a Rust source. NOT `rule:2`: the coverage
        # narrowing replaces only rule-2 rdeps, and the map never sees this file.
        if "crate_file" in _classes_of_path(p):
            out.append(("crate_file", p, "rule:10 crate-file:%s" % p,
                        {"rdeps(%s)" % _crate_of(p)}))
    for name in sorted(_templates_of(files)):
        hit = set(_tests_referencing_template(repo, name))
        for p in files:
            if p.startswith("templates/%s/" % name):
                out.append(("template:" + name, p, "rule:4 template:%s" % name, hit))
    for name in sorted(_examples_of(files)):
        hit = set(_tests_referencing_example(repo, name))
        for p in files:
            if p.startswith("examples/%s/" % name):
                out.append(("example:" + name, p, "rule:5 example:%s" % name, hit))
    for p in files:
        if not p.startswith("crates/") or "crate_file" in _classes_of_path(p):
            out.append(("read", p, "rule:8 file:%s" % p, set(_tests_reading_path(repo, p))))
        by_reason = collections.defaultdict(set)
        for t, reason in _tests_reading_at_runtime(repo, p).items():
            by_reason[reason].add(t)
        for reason, hit in sorted(by_reason.items()):
            out.append(("read", p, reason, hit))
    return out


# --- required pins (GH #1199) ------------------------------------------------
#
# A test that must run whenever a file is touched, whatever the coverage map
# says. AUDIT-PRE14-001 did not run in the strand gate of v2-p1082 although
# `cell_task.rs` was in its diff: rule 2 planned `rdeps(meclaw-colony)`, the
# narrowing (`pipeline/narrow.py`) replaced it by the parts the map saw, and
# the pin was not among them. The mapping file -> test lives in ONE table,
# `REQUIRED_PINS`, and nowhere else (the prose that asks for a pin carries a
# marker `(required-pin: <id>)` instead of a copy). `_term_rules` reads it, so
# a pin is a floor term: `impact` plans its floor from `test_terms`, and the
# narrowing replaces `rdeps` only. `scripts/check_required_pins.py` holds the
# table against the markers and the tree.
REQUIRED_PINS = ".config/required-pins.toml"
_PIN_FN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def required_pins(repo=None):
    """The rows of `REQUIRED_PINS` (`[[pin]]`: `id`, `paths`, `tests`, `why`),
    or [] when the file is absent. A table that does not parse is a
    `GatePlanError`: a plan that dropped every pin in silence is the defect the
    table exists against."""
    import tomllib
    path = os.path.join(REPO_ROOT if repo is None else repo, REQUIRED_PINS)
    try:
        with open(path, "rb") as fh:
            rows = tomllib.load(fh).get("pin", [])
    except FileNotFoundError:
        return []
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise GatePlanError("%s: %s" % (REQUIRED_PINS, exc)) from exc
    return [r for r in rows if isinstance(r, dict)] if isinstance(rows, list) else []


def required_pin_term(test, repo=None):
    """The nextest term of one pinned test `{file, fn}`, or None when `file` is
    no test unit of the tree or `fn` no identifier.

    The term is derived from the file, never written into the table, so a
    harness fold (`harness_fold.py`) moves it along: a flat `tests/<s>.rs` is
    `binary_id(=<c>::<s>)`, a folded `tests/<h>/<m>.rs` the harness narrowed to
    `<m>::<fn>` (`_folded_unit`), an unfolded module the harness narrowed to
    the function name at any depth. Without `fn` the whole unit runs."""
    if not isinstance(test, dict):
        return None
    rel, name = str(test.get("file") or ""), test.get("fn")
    if name is not None and not (isinstance(name, str) and _PIN_FN_RE.match(name)):
        return None
    parts = rel.split("/")
    if (len(parts) not in (4, 5) or parts[0] != "crates" or parts[2] != "tests"
            or not rel.endswith(".rs")):
        return None
    crate, root = parts[1], REPO_ROOT if repo is None else repo
    if len(parts) == 4 or parts[4] == "main.rs":
        stem = parts[3].removesuffix(".rs")
        if not _test_binary_exists(repo, crate, stem):
            return None
        base = "binary_id(=%s::%s)" % (crate, stem)
        return "(%s & test(=%s))" % (base, name) if name else base
    harness, mod = parts[3], parts[4].removesuffix(".rs")
    if not _test_binary_exists(repo, crate, harness + "/" + mod):
        return None
    unit = _folded_unit(_read_text(os.path.join(root, *parts[:4], "main.rs")), harness, mod)
    if not name:
        return _binary_id(crate, unit)
    base = "binary_id(=%s::%s)" % (crate, harness)
    if unit.endswith("/main"):
        return "(%s & test(/(^|::)%s$/))" % (base, name)
    return "(%s & test(=%s::%s))" % (base, mod, name)


def _required_pin_rules(files, repo):
    """Rule 12 of `test_filter`: `[(label, path, reason, terms)]` -- one entry
    per touched path a pin's `paths` glob matches, label `required-pin:<id>`."""
    out = []
    for pin in required_pins(repo):
        pid = pin.get("id")
        globs = [g for g in pin.get("paths") or [] if isinstance(g, str)]
        hit = {t for t in (required_pin_term(x, repo) for x in pin.get("tests") or []) if t}
        if not hit:
            continue
        for p in files:
            # A changed table runs every row: a new or moved term has to
            # prove itself in nextest, not first in the next diff it fires on.
            if p == REQUIRED_PINS or any(_matches(p, g) for g in globs):
                out.append(("required-pin:%s" % pid, p,
                            "required-pin:%s file:%s" % (pid, p), hit))
    return out


def test_terms(paths, repo=None):
    """`{term: [reason, ...]}` -- the terms of `test_filter` and the rule that chose each.

    A view on `_term_rules`, the same source `test_select` renders: the keys
    are exactly the terms `test_filter` joins, the reasons are what the
    pipeline's `impact` (W6) reads. A reason names the rule and what fired
    it -- `rule:2 file:<path>`, `rule:4 template:<name>`. An empty diff
    (rule 7) and a diff no rule selects for (rule 6) give `{}`.
    """
    files = _norm(paths)
    if not files:
        return {}
    reasons = collections.defaultdict(set)
    for _label, _p, reason, hit in _term_rules(files, repo):
        for t in hit:
            reasons[t].add(reason)
    # Rule 9, over all rules above: a binary that runs 0 tests is no
    # target (GH #997). `all()` and `rdeps()` are untouched -- they never
    # stand on one binary.
    kept = _drop_binaries_without_tests(repo, set(reasons))
    return {t: sorted(reasons[t]) for t in sorted(kept)}


# Public names of the resolver functions the pipeline graph (W6) builds its
# `reads`/`references` edges from -- the same functions, so the same hits.
tests_reading_path = _tests_reading_path
runtime_readers = _runtime_readers
runtime_roots = _runtime_roots
runtime_dir_applies = _runtime_dir_applies
runtime_ancestors = _ancestors
runtime_index = _runtime_index
runtime_spelled = _runtime_spelled
runtime_any = _runtime_any
tests_referencing_template = _tests_referencing_template
tests_referencing_example = _tests_referencing_example
template_readers = _template_readers
example_readers = _example_readers
test_sources = _test_sources
reader_sources = _reader_sources
is_helper = _is_helper
test_files = _test_files
includers = _includers
reader_text = _reader_text


# --- browser units (GH #1161, re-check M-3) ----------------------------------
#
# `MECLAW_REQUIRE_BROWSERS` and the RED of an absent engine hang on the engine
# booking, and only `browser:display` books the engines. A test that starts a
# browser inside `tests` ran unbooked, beside the whole suite, and SKIP-green on
# a lane without the engine -- in every mode (gh703, the WebKit phone, among
# them). So a browser unit never runs in `tests` outside ci: `test_select`
# subtracts every one, `plan` hands the ones the diff reaches to
# `browser:display`, and `scripts/pipeline/locks.py` (`browser-station-lock`)
# is RED for one the station cannot take.

# A browser driver of the lab, as code names it (`workshop/tools/display-*`).
_BROWSER_DRIVER_RE = re.compile(r"display-[a-z0-9-]+-browser\.mjs")
# The unit starts a process, or reaches code this rule cannot read: then it is
# taken as a starter (fail-closed). A unit that names a driver and does neither
# only READS the driver's text (720, 722).
_BROWSER_START_RE = re.compile(
    r"\bCommand\b|\bsupport::|\bcommon::|#\s*\[\s*path\b|^\s*(?:pub\s+)?mod\s+\w+\s*;", re.M)


# GH #1189: a unit that drives Chromium in-process (`BrowserParams`/CDP, gh766)
# names no driver; it reads the engine's path from MECLAW_CHROMIUM.
_BROWSER_ENGINE_READ_RE = re.compile(r"\benv::var(?:_os)?\(\s*\"MECLAW_CHROMIUM\"\s*\)")


def browser_unit(unit, text):
    """True when the test unit `unit` (a `_test_sources` unit) starts a browser:
    its stem ends `_browser`, or its code (comments blanked, `_rust_code_aligned`)
    names a driver `display-*-browser.mjs` and starts a process
    (`_BROWSER_START_RE`), or reads `MECLAW_CHROMIUM` (GH #1189). A unit that names
    a driver or the engine and that the lexer doubts is code this rule cannot
    read: a starter (fail-closed)."""
    if unit.rsplit("/", 1)[-1].endswith("_browser") or unit.split("/")[0].endswith("_browser"):
        return True
    driver, engine = _BROWSER_DRIVER_RE.search(text), "MECLAW_CHROMIUM" in text
    if not (driver or engine):  # cheap: no driver, no engine anywhere, not even code
        return False
    code = _rust_code_aligned(text)
    if code is None:
        return True
    return bool(_BROWSER_DRIVER_RE.search(code) and _BROWSER_START_RE.search(code)
                or _BROWSER_ENGINE_READ_RE.search(code))


_BROWSER_CACHE = {}


def browser_units(repo=None):
    """`{term: (crate, unit, text)}` of every browser unit of the tree (once per
    tree, like `_test_sources`)."""
    root = REPO_ROOT if repo is None else repo
    if root not in _BROWSER_CACHE:
        _BROWSER_CACHE[root] = {_binary_id(c, u): (c, u, t) for c, u, t in _test_sources(repo)
                                if browser_unit(u, t)}
    return _BROWSER_CACHE[root]


def browser_terms(repo=None):
    """The nextest terms of every browser unit, sorted."""
    return sorted(browser_units(repo))


def _browser_lock_term(crate, unit):
    """True when the unit is one of BROWSER_LOCKS (the station drives it already)."""
    return "/" not in unit and "crates/%s/tests/%s.rs" % (crate, unit) in BROWSER_LOCKS


def browser_unplaced(repo=None):
    """`[(rel, why)]` -- the browser units `browser:display` cannot take.

    The station runs its union with `--run-ignored all`; an `#[ignore]`d proof
    outside BROWSER_LOCKS would run in every mode without anybody deciding
    which (the colony half runs in I/R only). And a test module outside the
    units (`tests/support/**`) that names a driver hides its callers from the
    marker rule. Both are RED in `browser-station-lock`, and so is a unit or
    module the lexer doubts: unread is no proof (fail-closed)."""
    root = REPO_ROOT if repo is None else repo
    out = []
    for term, (crate, unit, text) in sorted(browser_units(repo).items()):
        if _browser_lock_term(crate, unit):
            continue
        rel = "crates/%s/tests/%s.rs" % (crate, unit)
        # Not `_test_counts`: its doubt is (0, 0), which reads as "no #[ignore]".
        # With the token lexer (#1162) a comment left open is such a doubt, and
        # an #[ignore]d proof that S3's regex lexer flagged fell silent (re-check
        # h2 M-1, fixture `doubted_proof.rs`).
        code = _rust_code(text)
        try:
            tests, runnable = (None, None) if code is None else _counts(code)
            # an item macro may make an #[ignore]d proof the count cannot see
            # (tb-s10 A1 point 21; in the tree only the lock 710 holds one)
            hidden = code is not None and _hidden_tests(code)
        except crate_root.Doubt:
            tests, runnable = None, None
        if tests is None:
            out.append((rel, "a browser unit the lexer doubts outside BROWSER_LOCKS"))
        elif tests > runnable:
            out.append((rel, "an #[ignore]d browser proof outside BROWSER_LOCKS"))
        elif hidden:
            out.append((rel, "a browser unit whose tests a macro may make outside BROWSER_LOCKS"))
    units = {"crates/%s/tests/%s.rs" % (c, u) for c, u, _t in _test_sources(repo)}
    crates_dir = os.path.join(root, "crates")
    for crate in sorted(os.listdir(crates_dir)) if os.path.isdir(crates_dir) else ():
        tests_dir = os.path.join(crates_dir, crate, "tests")
        for dirpath, _dirs, names in os.walk(tests_dir):
            for name in sorted(names):
                rel = os.path.relpath(os.path.join(dirpath, name), root).replace(os.sep, "/")
                if not name.endswith(".rs") or rel in units or name == "main.rs":
                    continue
                try:
                    with open(os.path.join(dirpath, name), encoding="utf-8",
                              errors="replace") as fh:
                        text = fh.read()
                except OSError:
                    out.append((rel, "unreadable"))
                    continue
                if not _BROWSER_DRIVER_RE.search(text):
                    continue
                code = _rust_code_aligned(text)
                if code is None:   # it names a driver, the lexer doubts: fail-closed
                    out.append((rel, "a shared test module the lexer doubts names a "
                                     "browser driver"))
                elif _BROWSER_DRIVER_RE.search(code):
                    out.append((rel, "a shared test module names a browser driver"))
    return out


def _crate_users(repo):
    """`{crate: {crates that depend on it, transitively, itself included}}` from
    the `path` dependencies of `crates/*/Cargo.toml` (dev ones too: nextest's
    `rdeps` counts them). None when a manifest cannot be read (fail-closed)."""
    import tomllib
    root = REPO_ROOT if repo is None else repo
    crates_dir = os.path.join(root, "crates")
    deps = {}
    for crate in sorted(os.listdir(crates_dir)) if os.path.isdir(crates_dir) else ():
        manifest = os.path.join(crates_dir, crate, "Cargo.toml")
        if not os.path.isfile(manifest):
            continue
        try:
            with open(manifest, "rb") as fh:
                doc = tomllib.load(fh)
        except (OSError, tomllib.TOMLDecodeError):
            return None
        name = (doc.get("package") or {}).get("name") or crate
        found = set()
        for key in ("dependencies", "dev-dependencies", "build-dependencies"):
            for dep, spec in (doc.get(key) or {}).items():
                if isinstance(spec, dict) and "path" in spec:
                    found.add(spec.get("package") or dep)
        deps[name] = found
    def reach(c):
        seen, todo = set(), [c]
        while todo:
            for d in deps.get(todo.pop(), ()):
                if d not in seen:
                    seen.add(d)
                    todo.append(d)
        return seen
    below = {c: reach(c) for c in deps}
    return {x: {x} | {c for c, ds in below.items() if x in ds} for x in deps}


def _selects_unit(term, crate, unit, users):
    """True when the `tests` term `term` reaches the browser unit. An unknown
    term shape reaches it (fail-closed)."""
    if term == "all()":
        return True
    if term == T0_FLOOR:
        return False
    m = re.fullmatch(r"rdeps\(([^)]+)\)", term)
    if m:
        return users is None or crate in users.get(m.group(1), {m.group(1)})
    if term == _binary_id(crate, unit):
        return True
    m = re.fullmatch(r"binary_id\(=([^:)]+)::([^)]+)\)", term)
    if m:
        return m.group(1) == crate and m.group(2) == unit.split("/")[0]
    m = re.fullmatch(r"\(binary_id\(=([^:)]+)::([^)]+)\) & test\(/\^([^/]+)::/\)\)", term)
    if m:
        return (m.group(1) == crate and "/" in unit
                and unit.split("/") in ([m.group(2), m.group(3)], [m.group(2), "main"]))
    # A required pin (`required_pin_term`, GH #1199): one binary narrowed to
    # tests by name -- it reaches that binary's units at most. Read as unknown
    # it reached every browser unit and planned `browser:display` for a pin.
    m = re.fullmatch(r"\(binary_id\(=([^:)]+)::([^)]+)\) & test\((?:=[A-Za-z0-9_:]+"
                     r"|/\(\^\|::\)[A-Za-z0-9_]+\$/)\)\)", term)
    if m:
        return m.group(1) == crate and m.group(2) == unit.split("/")[0]
    return True


def _browser_reach(paths, mode, repo=None):
    """`(terms, paths)`: the browser-unit terms the diff's `tests` selection
    reaches (lock units included), and the diff paths whose rule reached one
    -- both sorted; ([], []) in ci, where they stay in `tests`. A release
    reaches every unit and names no path: its `tests` is `all()` by rule,
    whatever the diff (`test_select`, KR-TB-1 d, GH #1162)."""
    if mode == "release":
        return sorted(browser_units(repo)), []
    files = _norm(paths)
    if mode == "ci" or not files:
        return [], []
    units = browser_units(repo)
    users = _crate_users(repo) if units else None
    terms, why = set(), set()
    for _l, p, _r, hit in _term_rules(files, repo):
        got = {t for t, (c, u, _x) in units.items()
               if any(_selects_unit(s, c, u, users) for s in hit)}
        if got:
            terms |= got
            why.add(p)
    return sorted(terms), sorted(why)


def browser_reached(paths, mode, repo=None):
    """The browser-unit terms the diff's `tests` selection reaches (lock units
    included), sorted; [] in ci, where they stay in `tests`."""
    return _browser_reach(paths, mode, repo)[0]


def browser_moved(paths, mode, repo=None):
    """`browser_reached` minus BROWSER_LOCKS: the units `browser:display` adds."""
    units = browser_units(repo)
    return [t for t in browser_reached(paths, mode, repo)
            if not _browser_lock_term(units[t][0], units[t][1])]


def tests_minus_for(repo=None):
    """What `tests` subtracts outside ci at most: the scenario class and every
    browser unit. The narrowing (`pipeline/impact.py`) subtracts all of it; the
    resolver the browser units its terms reach (`test_select`), the same set
    for every crate-wide term."""
    return " + ".join(_scenario_terms(repo) + browser_terms(repo))


def test_filter(paths, mode, repo=None):
    """The nextest filterset for this diff, or None when no test must run.

    1. workspace class            -> all()  (a workspace file the diff does
                                     not hold but that changed since the
                                     coverage map's rev: all() unless it is on
                                     `pipeline/narrow.NO_SHIFT`, M-4, or the
                                     merge point's version bump alone, V2-A3)
    2. rust_src in crate X        -> rdeps(X)
    3. rust_test file             -> its binary_id, plus every binary that
                                     compiles it in as a module -- but ONLY if
                                     the target still exists in the tree
                                     (`_test_binary_exists`): a deleted test
                                     cannot run, and nextest treats an
                                     unmatched `binary_id(=..)` as a hard
                                     error that kills the whole station; the
                                     binaries that compile it in count either
                                     way (a `#[path]` target in a directory
                                     without main.rs is no binary, tb-s1c)
    4. template <name>            -> the binaries that REFERENCE it
                                     (`_template_reference_re` -- a bare
                                     `"<name>"` does not count), the ones that
                                     name the shipped root and `<name>`
                                     below it (`_names_the_shipped_root`),
                                     plus the catalogue-wide ones
                                     (`_is_catalogue_wide`)
    5. example <name>             -> the binaries that reference it
       4 and 5: like a rule-8 reader, a referencing unit stands for rule 3's
                                     set of its file (`_unit_users`)
    6. none of the above, diff not empty -> None (no `tests` station at all)
    7. empty diff                 -> the t0 floor, `T0_FLOOR`; `plan()` turns
                                     that one value into `test-tier.sh t0`
    8. any path outside `crates/`, and every crate_file
                                  -> the binaries that name it as a literal
                                     in code -- never in a comment, an ignore
                                     reason or an assert/panic/expect message
                                     (`_tests_reading_path`). A test that reads
                                     a file is that file's drift lock, so the
                                     file's diff must run it. Additive: it
                                     extends rules 4 and 5, it replaces neither.
       runtime (GH #1138), also for every crate file a repo-relative walk
       covers (`crates`, `crates/<c>/src`: `.rs` too, tb-s1c) and for the
       non-`.rs` files under `crates/<c>/tests/`
                                  -> the tests that read the path WITHOUT
                                     naming it (`_tests_reading_at_runtime`):
                                     an ancestor directory as a literal (a
                                     walk, `crates` and `crates/<c>` too;
                                     not under templates/ or examples/,
                                     rules 4/5 own those; below a
                                     directory of RUNTIME_DIR_READS_ONLY
                                     each NAMED reader only what it keeps,
                                     in its own station (rule 11):
                                     `workshop/tools` -- gh661 the modules
                                     an `import` reaches (`.py`, every
                                     name a Python identifier, so nothing
                                     below `llm-conformance/` or
                                     `display-lab/`: OR-TM-8), 708 every
                                     top-level `.mjs` (a driver diff runs
                                     it in `tests`), 744 every `.mjs` and
                                     gh1209_no_stage every `.mjs`/`.js` at
                                     any depth (`browser:display`; no `.py`
                                     plans a browser); a reader the table
                                     does not name reads every file below,
                                     fail-closed), for a top-level
                                     file of templates/ the catalogue-wide
                                     binaries (`is_catalogue_top`), a SCRIPT_READS
                                     generator they run whose sources match
                                     the path, a crate fixture named relative
                                     to the crate. A crate test file no test
                                     names -> `rdeps(<c>)` (reader unknown,
                                     fail-closed); the coverage narrowing
                                     never replaces that rdeps
       both shapes: a reader stands for rule 3's set of its file
                                  (`_unit_users`): a folded helper for its
                                     whole harness, a shared file for every
                                     binary that compiles it in (M-8)
    9. over rules 3, 4, 5 and 8   -> a binary whose every test is `#[ignore]`
                                     is dropped (`_runs_no_test`): it would
                                     run 0 tests, and a `tests` station of 0
                                     tests is RED (GH #997). In doubt it stays.
   10. crate_file in crate X      -> rdeps(X), reason `rule:10 crate-file:<p>`
                                     (welle-tb S1b): a crate file outside
                                     src/ and tests/ (an `include_str!` schema,
                                     an example, a bench, a README, a
                                     migration, a template or toml inside the
                                     crate, a build.rs input) that no rule
                                     above can assign. Never `rule:2`, so the
                                     coverage narrowing (`pipeline/impact.py`)
                                     never replaces it -- the map lists `.rs`
                                     lines only. Plans fmt/clippy/unwrap-budget
                                     and in I/R doctests like a Rust source.
   11. outside ci, every browser unit -> out of `tests` (GH #1161, re-check
                                     M-3): a unit that starts a browser
                                     (`browser_unit`: stem `*_browser`, or a
                                     driver `display-*-browser.mjs` named in
                                     code that starts a process) is dropped
                                     from the terms and subtracted from every
                                     wider one that reaches it (`_selects_unit`;
                                     the narrowing: `tests_minus_for`); `plan`
                                     gives the ones the diff reaches
                                     (`browser_reached`) to `browser:display`,
                                     which books the engines. In ci they stay.
   12. a path a row of `REQUIRED_PINS` (`.config/required-pins.toml`) matches
                                  -> that row's tests, reason
                                     `required-pin:<id> file:<p>` (GH #1199),
                                     the table itself -> every row's tests.
                                     A pin is a test the prose asks to run on
                                     every touch of a file; as a floor term it
                                     stands beside rule 2, so the coverage
                                     narrowing (`pipeline/narrow.py`), which
                                     replaces `rdeps` only, never drops it.
                                     AUDIT-PRE14-001 fell out of a `cell_task.rs`
                                     diff that way (v2-p1082)

    Over-selection is allowed here, under-selection is not: a missed binary is
    a gate that passes over an untested change.
    """
    select = test_select(paths, mode, repo)
    return None if select is None else select["filter"]


def test_select(paths, mode, repo=None):
    """The structured selection behind `test_filter`, or None (no `tests`).

    `{binaries: ["<crate>::<stem>"], rdeps: ["<crate>"], all, floor, extra: [],
    minus: [<binary-name regex>], filter, why: ["<label>: <path>"]}` -- the
    rules are the ones `test_filter` documents, applied once; `filter` is the
    string `test_filter` returns, rendered from the fields here and nowhere
    else. `why` names the paths that put a term into the selection.
    A release takes none of the rules: it plans the full suite whatever the
    diff, `all()` minus the scenario class (KR-TB-1 d).
    """
    if mode == "release":
        # KR-TB-1 (d), GH #1162 OR-TB.S4.39: the release runs the full suite by
        # rule, whatever its export diff -- a one-file patch release planned
        # `rdeps(<crate>)`, a docs-only one no `tests` at all. The scenario class
        # stays subtracted: the release plans every scenario station. So does
        # every browser unit (rule 11, GH #1161): `all()` reaches each one, and
        # in `tests` it ran unbooked, SKIP-green on a lane without the engine.
        minus = _scenario_terms(repo)
        browser = browser_units(repo)
        cut = minus + sorted(browser)
        return {"binaries": [], "rdeps": [], "all": True, "floor": False, "extra": [],
                "minus": [_ONE_BINARY_RE.match(m).group(1) for m in minus if _ONE_BINARY_RE.match(m)]
                + ["^%s$" % re.escape(u) for _c, u, _x in sorted(browser.values())
                   if "/" not in u],
                "filter": "all() - (%s)" % " + ".join(cut) if cut else "all()",
                "why": ["mode: release -- full suite (KR-TB-1 d)"]}
    files = _norm(paths)
    if not files:
        return {"binaries": [], "rdeps": [], "all": False, "floor": True, "extra": [],
                "minus": [], "filter": T0_FLOOR, "why": ["floor: empty diff"]}

    rules = _term_rules(files, repo)
    why = [(label, p, hit) for label, p, _reason, hit in rules]
    # Rule 9 (GH #997) over the union -- the same call `test_terms` makes.
    terms = _drop_binaries_without_tests(
        repo, set().union(*(hit for _l, _p, _r, hit in rules)))
    browser = browser_units(repo) if mode != "ci" else {}
    # Rule 11: a browser unit leaves `tests` (it runs in `browser:display`) --
    # dropped as a term, subtracted from the wider terms that reach it. Only
    # those: an unreached one changes the scope string (and the cache key),
    # never the run.
    terms = {t for t in terms if t not in browser}
    if browser and terms:
        users = _crate_users(repo)
        browser = {t: v for t, v in browser.items()
                   if any(_selects_unit(s, v[0], v[1], users) for s in terms)}

    if not terms:
        return None
    expr = " + ".join(sorted(terms))
    minus = []
    if mode != "ci":
        # CI shards run the tests; the expression is planned, not reduced.
        minus = _scenario_terms(repo)
        if minus or browser:
            expr = "%s - (%s)" % (expr, " + ".join(minus + sorted(browser)))
    return {
        # every term's binaries, a narrowed harness `(binary_id(=c::h) & test(..))`
        # too: the pipeline builds only these (`pipeline/plan.py` -> `--test`), and a
        # missing harness took the tests station down (W5 delta I1)
        "binaries": sorted({b for t in terms for b in re.findall(r"binary_id\(=([^)]+)\)", t)}),
        "rdeps": sorted(t[len("rdeps("):-1] for t in terms if t.startswith("rdeps(")),
        "all": "all()" in terms,
        "floor": False,
        "extra": [],
        # binary-name regexes (`pipeline/plan.py` drops these parts of an `all()`);
        # a folded browser module is a test-level cut, its harness still runs
        "minus": [_ONE_BINARY_RE.match(m).group(1) for m in minus if _ONE_BINARY_RE.match(m)]
        + ["^%s$" % re.escape(u) for _c, u, _x in sorted(browser.values()) if "/" not in u],
        "filter": expr,
        "why": list(dict.fromkeys("%s: %s" % (label, p) for label, p, hit in why
                                  if hit & terms)),
    }


# --- scope helpers ----------------------------------------------------------

def _case_scope(repo, rel, recursive=False):
    """`<n> cases` counted from the case directory, or `cases` when absent.

    `recursive=True` counts every `*.json` below it: a suite may keep its cases
    in block sub-folders (the persona scenarios do).
    """
    root = REPO_ROOT if repo is None else repo
    path = os.path.join(root, rel)
    if not os.path.isdir(path):
        return "cases"
    if recursive:
        return "%d cases" % sum(1 for _d, _s, names in os.walk(path)
                                for f in names if f.endswith(".json"))
    return "%d cases" % len([f for f in os.listdir(path) if f.endswith(".json")])


def _scenario_scope(repo, rel):
    """`<n> scenarios` counted from the scenario file, or `scenarios` when absent.

    The number stands in every GATE line and in every receipt, so a scenario that
    silently left the file is visible in the pass that ran without it.
    """
    root = REPO_ROOT if repo is None else repo
    path = os.path.join(root, rel)
    try:
        with open(path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, ValueError):
        return "scenarios"
    return "%d scenarios" % len(data.get("scenarios", ()))


def _shell_files(repo, globs):
    """Expand the shellcheck globs against the tree; patterns as a fallback.

    The expansion reads `repo`, but the argv it produces is repo-RELATIVE --
    the runner executes it from the repo root, which for a `--repo` pointing
    somewhere else is not the tree that was globbed. That combination only
    occurs in tests, so it stays a note rather than a second path model.
    """
    root = REPO_ROOT if repo is None else repo
    out = []
    for pattern in globs:
        for p in sorted(globmod.glob(os.path.join(root, pattern))):
            out.append(os.path.relpath(p, root).replace(os.sep, "/"))
    return out or list(globs)


def _testing_bans():
    """`testing_bans` from beside this file, when the C plan needs it. Lazy:
    a kit tree without it never plans C. Found by this module's own directory
    when the caller's sys.path no longer holds `scripts/` -- the export
    selftest (`ci_plan_scripts`) puts it there for its `import gate_plan` and
    takes it off again before it calls `plan(.., "ci")`."""
    try:
        import testing_bans
        return testing_bans
    except ModuleNotFoundError:
        pass
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        "testing_bans", os.path.join(_SCRIPTS_DIR, "testing_bans.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def test_bans_paths(files):
    """The paths of `files` the C station `test-bans` hands on: source files
    the bans can grade and the files the change reservation reads. The rest
    stays off the argv -- in C the diff is often the whole tree (`git
    ls-files`, 23 177 paths in the private tree: over ARG_MAX)."""
    testing_bans = _testing_bans()
    reserved = {testing_bans.REGISTER_FILE} | {rel for rel, _judge in testing_bans.RESERVED}
    suffixes = tuple(testing_bans.SUFFIX.values())
    return sorted(f for f in files if f.endswith(suffixes) or f in reserved)


def persona_source_files(repo=None):
    """PERSONA_SOURCES expanded against the tree: repo-relative files, sorted.

    Expanded with `_matches`, the SAME matcher `_classes_of_path` uses -- not
    with `glob.glob`, which without `recursive=True` reads `**` as `*` and
    would be a second semantics. A pattern without a wildcard names one file
    and is passed over when that file is absent (`workshop/` does not travel).
    """
    root = REPO_ROOT if repo is None else repo
    out = set()
    for pattern in PERSONA_SOURCES:
        cut = min([i for i in (pattern.find("*"), pattern.find("?")) if i >= 0],
                  default=-1)
        if cut < 0:
            if os.path.isfile(os.path.join(root, pattern)):
                out.add(pattern)
            continue
        prefix = pattern[:cut].rsplit("/", 1)[0] if "/" in pattern[:cut] else ""
        for dirpath, _dirs, names in os.walk(os.path.join(root, prefix)):
            for name in names:
                rel = os.path.relpath(os.path.join(dirpath, name), root).replace(os.sep, "/")
                if _matches(rel, pattern):
                    out.add(rel)
    return sorted(out)


# --- the plan ---------------------------------------------------------------

def test_hook_flags(root, selector):
    """`--features meclaw-colony/test-hooks` or nothing, for a cargo call with the
    package selection `selector` (GH #1208, R-TV-W18). The rule lives once, in
    `meclaw_test_features` of scripts/cargo-target.sh (the build station and
    scripts/test-tier.sh read it there too); the tree's own copy wins, this
    checkout's is the fallback. A failing probe is an error, never "no flags"."""
    lib = os.path.join(root, "scripts", "cargo-target.sh")
    if not os.path.isfile(lib):
        lib = os.path.join(REPO_ROOT, "scripts", "cargo-target.sh")
    res = subprocess.run(
        ["bash", "-c", '. "$1" || exit 2; shift; meclaw_test_features "$@"', "_", lib, root]
        + list(selector), capture_output=True, text=True, timeout=30)
    if res.returncode:
        raise GatePlanError("meclaw_test_features failed (exit %d): %s"
                            % (res.returncode, res.stderr.strip()))
    return [ln for ln in res.stdout.splitlines() if ln]


def plan(paths, mode, repo=None):
    """The stations this diff must run, cheapest first."""
    if mode not in MODES:
        raise GatePlanError("unknown mode: %s (expected %s)"
                            % (mode, "|".join(MODES)))
    files = _norm(paths)
    classes = classify(files)
    crates = sorted(crates_of(files, repo))
    templates = _templates_of(files)
    examples = _examples_of(files)
    ci = mode == "ci"
    irc = mode in ("integration", "release", "ci")
    ir = mode in ("integration", "release")
    check = ["--check"] if ci else []

    out = {}

    # --- form, before anything takes the lock. A red form station costs the
    # queue and the build for a `cargo fmt` somebody forgot; measured over
    # three waves, that was four of twenty-three red strand runs (5 771 s).
    # The paths travel on the ARGV, not on stdin: the runner runs every
    # station with stdin closed, because a station that reads it eats the
    # rest of its own command list.
    if files and not ci:
        out["precheck"] = station(
            "precheck", "form", False,
            [["python3", "scripts/precheck.py", "--files"] + list(files)])

    # --- always: the anchor and claim gates. Seconds, and they are the ones
    # that catch a doc that promised something the code no longer carries.
    out["roadmap-anchors"] = station(
        "roadmap-anchors", "ROADMAP.md", False,
        [["python3", "scripts/check_roadmap_anchors.py"] + check])
    out["adr-anchors"] = station(
        "adr-anchors", "plans/adr", False,
        [["python3", "scripts/check_adr_anchors.py"] + check,
         # GH #1199: the required pins are anchors of the same kind -- prose
         # (AGENTS.md) names a test the tree must carry; seconds, every mode
         ["python3", "scripts/check_required_pins.py"] + check])
    out["claims"] = station(
        "claims", "claims.tsv", False,
        [["python3", "scripts/check_claims.py"] + check])
    out["tree-rules"] = station(
        "tree-rules", "templates+examples", False,
        [["python3", "scripts/check_tree_rules.py"]])
    # The testing-bans inventory (VORGABEN § 5.4): a pass and the public CI
    # count the whole tree against the baseline; a strand's added lines are
    # `precheck`'s.
    if irc:
        out["test-bans-inventory"] = station(
            "test-bans-inventory", "tree", False,
            [["python3", "scripts/testing_bans.py", "--inventory", "--check"]])
    # ... and in C, where precheck never runs, its two testing checks on the
    # diff (tv-bn-ci, #1209): an outside pull request is graded nowhere else.
    if ci:
        out["test-bans"] = station(
            "test-bans", "diff", False,
            [["python3", "scripts/testing_bans.py", "--files"] + test_bans_paths(files)])

    # --- corpus. A touched source regenerates and then checks; otherwise the
    # check alone proves the committed seed still matches its sources. CI never
    # regenerates -- it has nowhere to put the result.
    # NEVER in ci. The CI runs on the published export mirror and `workshop/`
    # does not travel, so the seed builder is not there at all -- which is why
    # `.github/workflows/ci.yml` forbids the step and the Rust twin
    # `librarian_seed_corpus.rs` skips cleanly in the public tree (GH #234).
    # Planning it there would be red on the first run, and planning the
    # REGENERATE half would be red on a tree CI cannot write back to anyway.
    #
    # TWO checks, not one: the seed corpus AND `templates/builder-librarian`
    # against the source it was generated from. The old R11 stage ran both, and
    # a station that ran only the first would let a drifted librarian template
    # through. If `build_librarian.py --check` goes red after a corpus
    # regenerate, regenerate the template with
    # `python3 workshop/tools/build_librarian.py` in the same strand.
    seed = ["python3", "workshop/tools/build_librarian_seed.py"]
    librarian = ["python3", "workshop/tools/build_librarian.py", "--check"]
    if not ci:
        # The pre-check, and it must come BEFORE `corpus` in STATION_ORDER.
        # `corpus` regenerates first and checks second, so it grades the file
        # it has just written: a committed corpus that no longer describes its
        # sources was GREEN in every mode. Measured 2026-09-05, two strands
        # with version bumps in `templates/*/template.json`: `docs.jsonl`
        # travelled with the OLD versions and no station ever said so
        # (GH #596). `--check` regenerates into a temp file and byte-compares
        # against the committed path -- run against the tree as it is, it is
        # the only station that reads what would actually travel.
        #
        # It costs a quarter of a second, so it runs in every non-ci mode
        # regardless of the diff: a stale corpus is stale whoever last touched
        # it, and the strand that finds it is the cheapest place to say so.
        out["corpus-committed"] = station(
            "corpus-committed", "seed as committed", False,
            [seed + ["--check"]])
        if "corpus_source" in classes:
            out["corpus"] = station(
                "corpus", "regenerate+check+librarian", False,
                [list(seed), seed + ["--check"], librarian])
        else:
            out["corpus"] = station(
                "corpus", "check+librarian", False,
                [seed + ["--check"], librarian])

    if classes & {"template", "catalogue"} or ir:
        out["catalogue"] = station(
            "catalogue", "templates/README.md", False,
            [["python3", "scripts/check_catalogue.py"]])

    if classes & {"shell", "gate_infra"} or ci:
        globs = SHELL_GLOBS[:SHELL_GLOBS_PUBLIC] if ci else SHELL_GLOBS
        out["shellcheck"] = station(
            "shellcheck", " ".join(globs), False,
            [["shellcheck"] + _shell_files(repo, globs)])

    if "gate_infra" in classes or ci:
        out["gate-selftest"] = station(
            "gate-selftest", "unittest", False,
            # One command, every module: the resolver's own tests, the
            # runner's, the strand kit's, the wave retro's and the merge
            # driver's. They all arrive with the runner (ROOT_FILES), so this
            # list is the same in both trees; the retro's one private-tree
            # case skips itself there. `selftest_shard.py` runs the modules
            # in shards side by side (GH #1126); one shard is the plain
            # `python3 -m unittest` of before.
            [["/usr/bin/python3", "-I", "scripts/pipeline/selftest_shard.py", "unittest",
              "scripts.tests.test_gate_plan", "scripts.tests.test_gate_sh",
              "scripts.tests.test_strand_sh",
              "scripts.tests.test_pipeline_hygiene",
              "scripts.tests.test_wave_receipt",
              "scripts.tests.test_lane_fingerprint",
              "scripts.tests.test_lane_browser_deps",
              "scripts.tests.test_wave_retro",
              "scripts.tests.test_git_merge_display_sync",
              "scripts.tests.test_precheck",
              "scripts.tests.test_precheck_testing_bans",
              "scripts.tests.test_pipeline_locks",
              "scripts.tests.test_harness_fold",
              "scripts.tests.test_pipeline_pre",
              "scripts.tests.test_pipeline_cascade_dry",
              "scripts.tests.test_display_sync",
              "scripts.tests.test_nextest_quarantine",
              "scripts.tests.test_slow_tests",
              "scripts.tests.test_flake_triage",
              "scripts.tests.test_clock_bans",
              "scripts.tests.test_roadmap_anchors",
              "scripts.tests.test_required_pins",
              "scripts.tests.test_pipeline_store",
              "scripts.tests.test_pipeline_events",
              "scripts.tests.test_pipeline_receipt",
              "scripts.tests.test_pipeline_executor",
              "scripts.tests.test_pipeline_schedule",
              "scripts.tests.test_pipeline_lanes_cli",
              "scripts.tests.test_pipeline_plan",
              "scripts.tests.test_pipeline_cache",
              "scripts.tests.test_pipeline_cache_reads",
              "scripts.tests.test_pipeline_lane",
              "scripts.tests.test_fake_ssh",
              "scripts.tests.test_lane_engstelle",
              "scripts.tests.test_pipeline_gate_cli",
              "scripts.tests.test_pipeline_metrics",
              "scripts.tests.test_pipeline_backfill",
              "scripts.tests.test_pipeline_grafana",
              "scripts.tests.test_pipeline_retro",
              "scripts.tests.test_pipeline_night",
              "scripts.tests.test_pipeline_orga_export",
              "scripts.tests.test_wave_close",
              "scripts.tests.test_wave_live",
              "scripts.tests.test_pipeline_register",
              "scripts.tests.test_pipeline_unit_runner",
              "scripts.tests.test_store_isolation",
              "scripts.tests.test_kit_time",
              "scripts.tests.test_pipeline_measure",
              "scripts.tests.test_pipeline_ports",
              "scripts.tests.test_pipeline_lab_width",
              "scripts.tests.test_browser_required",
              "scripts.tests.test_gate_plan_browser_units",
              "scripts.tests.test_pipeline_pool",
              "scripts.tests.test_nextest_groups",
              "scripts.tests.test_display_scenarios_parallel",
              "scripts.tests.test_side_effect_guard",
              "scripts.tests.test_pipeline_merge",
              "scripts.tests.test_pipeline_versions",
              "scripts.tests.test_pipeline_revert",
              "scripts.tests.test_pipeline_flakes",
              "scripts.tests.test_pipeline_graph",
              "scripts.tests.test_pipeline_impact",
              "scripts.tests.test_pipeline_coverage",
              "scripts.tests.test_pipeline_questions",
              "scripts.tests.test_pipeline_finding",
              "scripts.tests.test_unittest_shape",
              "scripts.tests.test_pipeline_receipt_bytes",
              "scripts.tests.test_pipeline_narrow",
              "scripts.tests.test_pipeline_spawn_closure",
              "scripts.tests.test_pipeline_browser_cover",
              "scripts.tests.test_pipeline_browser_plan",
              "scripts.tests.test_pipeline_full_pass",
              "scripts.tests.test_pipeline_lanes_release",
              "scripts.tests.test_pipeline_selftest_shard",
              "scripts.tests.test_meclaw_bin_owner",
              # GH #1199: the one mechanism for private-tree inputs
              "scripts.tests.test_private_tree",
              # GH #1162: the I/R path's tests stood in no station (the kit
              # ran them, no gate did); every test_pipeline_* module is
              # listed here (test_every_runner_test_module_is_in_gate_selftest).
              "scripts.tests.test_pipeline_ir_path",
              "scripts.tests.test_pipeline_two_phase_build",
              # tb-s7 (#1167): the turnaround module, under the same lock
              "scripts.tests.test_pipeline_turnaround",
              # #1075 (lo-funde, taken over in r1-kit): the stop watch of long runs
              "scripts.tests.test_stop_watch",
              # GH #1199: the kit's progress lines, loud refusals and liveness
              "scripts.tests.test_pipeline_progress",
              # GH #1199 (v2-r3-mergegate): merge/strand plan parity and the
              # strand export audit narrowed to its diff
              "scripts.tests.test_pipeline_merge_gate_parity",
              "scripts.tests.test_export_audit_scope"]])

    # `ir` as well as the class: the library is what every display measurement
    # of the next wave is read with, it costs five seconds, and a pass that
    # declares a wave done is exactly where a broken instrument has to surface
    # (the same reasoning as `browser:display` below, GH #746).
    if (ir or "display_lab" in classes) and not ci:
        out["display-lab"] = station(
            "display-lab", "unittest", False,
            [["python3", "-m", "unittest", "scripts.tests.test_display_lab"]])

    # The conformance tool's offline tests (GH #998): a fake provider on
    # loopback, no key, no money, under a second. `ir` for the same reason as
    # display-lab: the pass that declares a wave done is where a broken
    # instrument has to surface.
    if (ir or "llm_conformance" in classes) and not ci:
        out["llm-conformance"] = station(
            "llm-conformance", "unittest", False,
            [["python3", "-m", "unittest", "scripts.tests.test_llm_conformance"]])

    # The guide harness pins its own wiring (GH #881): the fixture edge, the
    # local engine and the rules tail its arms carry. A self-test nothing
    # plans is not a lock; it costs seconds and needs no colony.
    if (ir or "evals_guide" in classes) and not ci:
        out["guide-selftest"] = station(
            "guide-selftest", "unittest", False,
            [["python3", "workshop/evals/conversation-guide/run_guide.py", "--self-test"],
             # The #871 replay's unittest (L-7, tb-s10): its tool lives in the
             # guide's directory, so it runs where the guide's self-test runs.
             ["python3", "-m", "unittest", "scripts.tests.test_replay_871"],
             # The workshop's one log pager and the Python class pin "a reader
             # takes one page for the whole" (a scan of workshop/**/*.py, tv-bn
             # M7b17): the guide is the first of its readers, and I/R always
             # is where a reader that reads a window has to surface.
             ["python3", "-m", "unittest", "scripts.tests.test_colony_log"]])

    # --- cargo work.
    if classes & {"rust_src", "rust_test", "crate_file", "workspace"}:
        out["fmt"] = station("fmt", "workspace", True,
                             [["cargo", "fmt", "--all", "--", "--check"]])
        if "workspace" in classes or irc or not crates:
            selector, scope = ["--workspace"], "workspace"
        else:
            selector = [a for c in crates for a in ("-p", c)]
            scope = " ".join(crates)
        # R-TV-W18 (GH #1208): lint what the test builds compile -- the
        # `test-hooks` code was never linted while no build turned it on. The
        # unwrap budget below keeps the product's view, without the feature.
        out["clippy"] = station(
            "clippy", scope, True,
            [["cargo", "clippy"] + selector + ["--all-targets"]
             + test_hook_flags(repo or REPO_ROOT, selector) + ["--", "-D", "warnings"]])

    if classes & {"rust_src", "crate_file", "workspace", "unwrap_infra"} or ci:
        # cargo:1 -- it is a Python script, but it runs
        # `cargo clippy --workspace` under the hood, so the runner must give it
        # the same nice/ionice/flock and build budget as any other cargo work.
        out["unwrap-budget"] = station(
            "unwrap-budget", "workspace", True,
            [["python3", ".github/gates/unwrap_budget.py"]])

    if "corridor" in classes or ci:
        out["corridor"] = station(
            "corridor", "colony.rs", False,
            [[".github/gates/corridor_byte_gates.sh"]])

    expr = test_filter(files, mode, repo)
    if expr == T0_FLOOR:
        # The floor is a TIER, not a filterset: `test-tier.sh t0` passes
        # `--lib --bins`, so nextest builds the unit targets only. Handing the
        # same expression to `filter` would compile all 650+ test binaries
        # first and then run a handful of them -- minutes of rustc for the
        # cheapest station in the table.
        out["tests"] = station(
            "tests", "t0 floor", True,
            [["scripts/test-tier.sh", "t0"]], run=not ci)
    elif expr is not None:
        out["tests"] = station(
            "tests", expr, True,
            [["scripts/test-tier.sh", "filter", expr]], run=not ci)

    if "tests" in out:
        # The slow lock (GH #1046): the times of the run above against a third of
        # each test's nextest budget, from the JUnit file `test-tier.sh` leaves.
        # The display scenarios grew to 213 s of their 240 s unseen and were
        # killed at 24 threads; nothing read the times of green tests. cargo:1
        # for the order only -- it runs where the tests ran (a lane included)
        # and is skipped with them when the form station is red.
        out["slow-tests"] = station(
            "slow-tests", "1/3 of terminate-after", True,
            [["python3", "scripts/slow_tests.py"]], run=not ci)

    if mode == "release":
        scen = " + ".join(scenario_station_terms(repo))
        if scen:
            out[SCENARIO_STATION] = station(
                SCENARIO_STATION, scen, True, [["scripts/test-tier.sh", "filter", scen]])

    if ir and classes & {"rust_src", "crate_file", "workspace"}:
        # with the test hooks, like every other build of the gate and the CI
        # doctests (GH #1208, re-check M1): one variant of meclaw-colony in target/
        out["doctests"] = station("doctests", "workspace", True,
                                  [["cargo", "test", "--workspace", "--doc"]
                                   + test_hook_flags(repo or REPO_ROOT, ["--workspace"])])

    # `workspace` only, plus the release (R-P2, 2026-09-19): cargo-deny grades
    # the dependency graph, and the `workspace` class is what moves that graph.
    # 39 integration runs over five waves, 0 RED (wave P, finding 04 § 3.2-3.4).
    # The release keeps it unconditional -- it is the run that ships the tree.
    if "workspace" in classes or mode == "release":
        out["deny"] = station(
            "deny", "bans licenses sources", True,
            [["cargo", "deny", "check", "bans", "licenses", "sources"]])

    # --- the offline suites. They are Python, they need no network and no
    # model, and they are the only place the memory-hive state machine and the
    # declaration lane run as a whole.
    # Triggered by the diff, not by the mode (R-P2, 2026-09-19): the two
    # memory stations answer only about the memory hive, its recall lane and
    # their own cases. 43 integration runs over five waves, 16 317 s, not one
    # finding (wave P, finding 04 § 3.4). `release` keeps both unconditional.
    if mode == "release" or "evals_memory" in classes or "memory-hive" in templates:
        cmd = ["python3", "workshop/evals/scenarios/run_scenarios.py"]
        # Case selection by impact (W7): when the memory-hive template is the
        # ONLY reason for the station, a strand or an integration pass runs the
        # cases that name that template. A diff on the suite itself and the
        # release run every case.
        if (mode in ("strand", "integration") and "memory-hive" in templates
                and "evals_memory" not in classes):
            cmd += ["--only-templates", "memory-hive"]
        cmds = [cmd]
        # The runner's own unit tests (W7: workers, slots, case selection)
        # ride with a diff on the suite -- a unittest no station plans is not
        # a lock. Seconds, fake colony, before the real cases.
        if "evals_memory" in classes:
            cmds.insert(0, ["python3", "-m", "unittest", "discover", "-s",
                            "workshop/evals/scenarios/tests"])
        out["scenarios:memory"] = station(
            "scenarios:memory",
            _case_scope(repo, "workshop/evals/scenarios/cases"), False, cmds)

    if (ir or "evals_builder" in classes
            or (templates - {"memory-hive"})
            or examples & {"meclaw-os", "organism"}):
        # The argv is relative to the `cwd` below, not to the repo root: the
        # runner `cd`s into it before it execs, so a repo-relative path would
        # resolve as <cwd>/<repo-relative path> and miss (measured 2026-09-04:
        # ".../builder-scenarios/workshop/evals/builder-scenarios/
        # run_builder_scenarios.py: No such file").
        out["scenarios:builder"] = station(
            "scenarios:builder",
            _case_scope(repo, "workshop/evals/builder-scenarios/cases"), False,
            ([["python3", "-m", "unittest", "discover", "-s", "tests", "-t", "."]]
             if "evals_builder" in classes else [])
            + [["python3", "run_builder_scenarios.py"]],
            cwds=["workshop/evals/builder-scenarios"]
            * (2 if "evals_builder" in classes else 1))

    # The pins of the one normative display document (development-rules § 10).
    # Pure Python, seconds, no colony and no browser -- so it runs in S as soon
    # as the diff touches the template or the scenario file, and in I, R and C
    # ALWAYS. C is where the other scenario suites cannot run at all, because
    # workshop/ does not travel; this one does, and two seconds is not a price
    # worth making conditional (wave H, plan § 10 "Produces": I/R/C always).
    if ir or mode == "ci" or "display" in templates or "display_scenarios" in classes:
        out["scenarios:display"] = station(
            "scenarios:display",
            _scenario_scope(repo, "templates/display/compose/scenarios/scenarios.json"),
            False,
            [["python3",
              "templates/display/compose/scenarios/run_display_scenarios.py"]
             # The four threads `pipeline/plan.py` books for the station are its
             # workers (GH #1161): it booked four and forked none, 164-170 s in
             # every I/R pass. ci runs the legacy runner, which books nothing.
             + ([] if mode == "ci" else ["--workers", str(DISPLAY_SCENARIO_WORKERS)]),
             # The cheap half of the drift question (GH #753): does the mark
             # `SOURCE` still describe the living description tree? Seconds,
             # no cargo, and it never fails the station -- it prints a NOTE
             # and exits 0, because the verdict belongs to the Rust lock
             # `710_the_scenarios_run_against_the_curator`. Where the living
             # tree is absent (ci, a foreign clone) there is nothing to
             # compare and it says so.
             ["python3", "scripts/display_sync.py", "--check-source"]])

    # The B-proofs of the same document, in a real engine (§ 6.7, R-23-10). Two
    # shapes of one station, like `export-audit`: the SHEET half builds its page out
    # of `compose.py`'s own parts, needs no colony and costs about half a minute, so
    # a strand pays for it; the COLONY half drives three exits in two engines, six
    # tests with a throwaway colony each -- 4 to 6 minutes in a row, about two when
    # the Chromium ones run side by side. It is `#[ignore]`d in the tree, and the
    # two modes that declare work finished take it off the shelf.
    #
    # Both of them, and not `release` alone (GH #746): the station was written with
    # the colony half in `release` only, the release gate of 0.39.0 was the first
    # gate of any mode to run it, and it was red on three proofs that had stood
    # unmeasured since the day the arm was added. An expensive proof nobody reaches
    # until the release night is a proof nobody reads, and the pass that says a wave
    # is done is exactly where that has to be paid.
    #
    # `cargo: True` -- both halves are nextest runs and build test binaries, so they
    # belong under the same nice/ionice/flock/build-width hygiene as any other cargo
    # station. Through `test-tier.sh`, which owns the nextest profile and knows not
    # to take the cargo lock twice.
    #
    # No browser on a developer's host is a SKIP: the driver leaves with 3 and the
    # Rust locks return green (R2b). In the gate it is not (GH #1161, OR-TB.S3.17):
    # the station books both engines, the planner gives it only a lane whose
    # fingerprint says they start (`schedule.browser_refusals`), the executor is
    # RED before the start when its own probe says otherwise, and the proofs run
    # with MECLAW_REQUIRE_BROWSERS=1 -- an engine that does not start fails the run.
    #
    # Two commands (W7): ONE nextest call over every ignored lock, the colony half
    # among them in I/R, and the sheet call. Inside the first call the groups of
    # `.config/nextest.toml` set the width -- `webkit` one at a time, `colony-boot`
    # four. The two calls are independent, but the station does not say so
    # yet: two WebKit runs side by side were never measured (review W7 I1).
    #
    # A narrowed integration cuts this row afterwards (owner's ruling 10.10.,
    # `pipeline/browser_plan.py`): a lock -- colony half and sheet included --
    # stays when the wave diff touches its map cover or the lock file, when it is
    # unmeasured, when the map does not fit, or when a non-Rust station input, a
    # workspace file or a crate file outside the map changed. `release`, a full pass
    # and an integration in doubt run all.
    #
    # Rule 11 of `test_filter` (GH #1161, re-check M-3): a browser unit the diff's
    # selection reaches runs HERE, never in `tests` -- so it plans the station in
    # every mode but ci, and the units outside BROWSER_LOCKS join its union.
    reached = browser_reached(files, mode, repo)
    if ir or "display" in templates or "display_browser" in classes or reached:
        colony = ir
        out["browser:display"] = station(
            "browser:display", "sheet+colony" if colony else "sheet", True,
            browser_cmds(colony, moved=browser_moved(files, mode, repo)),
            parallel={"by": "cmd", "profiles": list(BROWSER_PROFILES)})

    recall_src = any(p.startswith("crates/meclaw-cells/src/") and "recall" in p
                     for p in files)
    if (mode == "release" or "memory-hive" in templates
            or "evals_memory" in classes or "evals_recall" in classes or recall_src):
        # cwd-relative argv, same rule as `scenarios:builder` above. The second
        # command is the answer side of the harness -- rubric choice, abstention
        # axis, stratum -- offline, beside the scripts it imports (GH #878).
        out["recall-harness"] = station(
            "recall-harness", "tier-1+answers", False,
            [["python3", "recall_cases.py"], ["python3", "answer_cases.py"],
             ["python3", "local_path_cases.py"], ["python3", "pipeline_cases.py"]],
            cwds=["workshop/evals/p5-longmemeval/tools", "workshop/evals/p5-longmemeval",
                  "workshop/evals/p5-longmemeval", "workshop/evals/p5-longmemeval"])

    if mode == "release":
        out["deny-advisories"] = station(
            "deny-advisories", "advisories", True,
            [["cargo", "deny", "check", "advisories"]])
        # #1166 re-check M-1: the build station merges `-p` sets, so a crate is
        # no longer built in its own feature variant anywhere; this checks each
        # member alone (S6-DIAG: colony beside cells resolves tokio `process`).
        scope, iso = crate_isolation(repo)
        out["crate-isolation"] = station("crate-isolation", scope, True, iso)

    # --- the persona gate (ADR-0029, GH #882). Two FREE stations read a committed
    # measurement; the paid or long run is an artefact, never a station.
    persona = "workshop/evals/persona"
    if ir or "evals_persona" in classes:
        out["persona-cases"] = station(
            "persona-cases", _case_scope(repo, persona + "/scenarios", recursive=True), False,
            [["python3", "persona_cases.py", "--self-test"], ["python3", "persona_cases.py"]],
            cwds=[persona, persona])
    if ir or classes & {"persona_source", "evals_persona"}:
        out["persona-receipt"] = station(
            "persona-receipt", "fingerprint", False,
            [["python3", "fingerprint.py", "--check"]], cwds=[persona])

    if "export_infra" in classes or ir:
        # Seconds of pure Python, so the integration pass pays for it too --
        # it is the self-test of the very rules the audit below applies.
        out["export-selftest"] = station(
            "export-selftest", "drift-fixtures", False,
            [["python3", "plans/export-fixtures/test_drift_gate.py"],
             ["python3", "plans/export-fixtures/test_export_tree_tests.py"]])

    if mode == "release":
        # GH #1101: the tests whose answer depends on WHICH templates ship, run
        # in the tree that ships. The export audit scans test literals (R2b/R2e);
        # a count or a floor over the catalogue holds no literal to scan.
        # cargo:1 -- a fresh target inside the export tree, like R9.
        out["export-tree-tests"] = station(
            "export-tree-tests", "catalogue tests in the export tree", True,
            # `--worktree` (RC-KX I1): the tree the cache keys the station on
            [["python3", "plans/export-fixtures/export_tree_tests.py", "--worktree"],
             # GH #1199: the kit half -- the public CI's cargo-free stations
             # (gate-selftest first of all) in the export tree, with its own
             # gate.sh ci; 0.64.0's public CI was red there and nowhere else
             ["python3", "plans/export-fixtures/export_tree_tests.py", "--worktree", "--kit"]])
    elif mode == "strand" and any(_feeds_the_kit_stations(p) for p in files):
        # GH #1199 (release 0.64.2, run 4): four kit self-tests read a file the
        # export does not carry; the strand gate ran them only in the private
        # tree and the release found them red. A diff at a kit input runs the
        # cargo-free kit half where the public CI runs it -- over the working
        # tree the strand gate gates before the commit (RC-KX I1).
        out["export-tree-tests"] = station(
            "export-tree-tests", "kit stations in the export tree", False,
            [["python3", "plans/export-fixtures/export_tree_tests.py", "--worktree", "--kit"]])

    if ir:
        # `{receipt}` is a placeholder the runner substitutes with the path it
        # wants the audit receipt written to. The runner ALWAYS substitutes it,
        # in every mode -- so `make_export.py` never falls back to its own
        # default receipt lookup from here.
        #
        # TWO shapes of the same station. Release runs the full audit; cargo:1
        # -- it is a Python script, but it runs `cargo check --workspace
        # --all-targets` in a fresh target directory inside the export tree.
        # That is a cold build, and it belongs under the same
        # nice/ionice/flock/build-width hygiene as any other cargo station.
        #
        # Integration runs the SAME audit with `--skip-cargo`: R8/R9/R12-class
        # work drops out, nothing is built (cargo:0), and what remains are the
        # cheap rules -- R2b dead template references in tests, R2c tests
        # reaching into a never-exported root, R5 name/domain patterns, R10
        # relative links. Those used to surface only in the
        # release pass, an hour after the integration pass had declared the
        # wave done (v0.30.0).
        #
        # `--rev HEAD` is the second half of the dry shape. `make_export.py`
        # defaults to `--rev master`, and the receipt the audit reads belongs
        # to the revision the RUNNER gated -- HEAD. On master the two are the
        # same commit, which is why the release audit never noticed; from a
        # wave branch they differ and the audit answered with a rev mismatch
        # instead of a verdict (measured 2026-09-04). The integration pass
        # judges the tree it just gated, so it names it. Release keeps the
        # `master` default: an export is of master, not of whatever is checked
        # out.
        dry = mode == "integration"
        argv = ["python3", "plans/export-fixtures/make_export.py", "--keep-going"]
        if dry:
            argv += ["--skip-cargo", "--rev", "HEAD"]
        out["export-audit"] = station(
            "export-audit", "R1-R17 dry" if dry else "R1-R17", not dry,
            [argv + ["--gate-receipt", "{receipt}"]])
    elif mode == "strand" and any(_feeds_the_strand_audit(p) for p in files):
        # V2-A18 (GH #1199, release 0.64.2 run 5): two g855 tests with a bare
        # `../../templates` root reached master through strand and merge gates;
        # R2b saw them first in the release. A diff at what the cheap rules
        # judge plans the DRY audit here. `--no-gate-receipt`: a strand gate
        # writes no I/R receipt, and R11/R12/R14/R17 (read from one) would be
        # red on every strand -- the switch drops exactly them, visibly.
        # `--worktree` instead of `--rev HEAD` (RC-KX I1): the gate gates the
        # working tree before the commit. `--parent HEAD` (RC-XA I1): the
        # default `github-main` is checked against `ls-remote origin main` and
        # aborts on drift; that ref moves by hand after an export, so a stale
        # or missing one would turn the strand red regardless of its diff.
        # `--keep-going` builds no commit and never reads the parent. I/R keep
        # the default -- there the check is wanted.
        # GH #1199 (v2-r3-mergegate): the strand judges its diff -- R2b/R2c/R2d
        # over the exported tests it touches (make_export.py among them: the
        # whole tree). Profiled 16.10.: 162 s, of which R15 97 s and R2b-R2d 26 s.
        out["export-audit"] = station(
            "export-audit", "R1-R17 dry, no gate receipt", False,
            [strand_audit_cmd(files, kit="export-tree-tests" in out)])

    if ci:
        # One rule, not eight conditions spread through the builder above:
        # whatever the diff says, ci only plans what the published tree holds.
        for name in CI_EXCLUDED:
            out.pop(name, None)

    if mode == "release":
        gaps, twice = release_partition(out, repo)
        if gaps or twice:
            raise GatePlanError(
                "release plan does not split all() (KR-TB-1 d): in no station %s; "
                "in two stations %s" % (gaps[:20], twice[:20]))
    return [out[name] for name in STATION_ORDER if name in out]


_BINARY_ID_RE = re.compile(r"^binary_id\(=([^:()]+)::([^:()]+)\)$")
# `_TERM_RE` is greedy: `binary(/(a/) & test(/b)/)` -- a compound of two
# operators -- matched as ONE binary regex `(a/) & test(/b)`. The guard and
# every reader of a binary regex (scenario class, `minus`, lock narrowing,
# `harness_fold`; tb-s10 D) read a single operator only: no `/)` inside, so a
# compound is a form they cannot read.
_ONE_BINARY_RE = re.compile(r"^binary\(/((?:(?!/\)).)*)/\)$")


def _term_hits(term, units):
    """`{(crate, unit)}` the term selects, or None for a form this cannot read.

    The three forms the planner writes: `binary(/<regex>/)` over the binary
    name, `binary_id(=<crate>::<binary>)`, and `_binary_id` of a folded
    module. Any other form is None: the guard cannot vouch for it."""
    m = _ONE_BINARY_RE.match(term)
    if m:
        rx = re.compile(m.group(1))
        return {(c, u) for c, u in units if rx.search(u.split("/")[0])}
    m = _BINARY_ID_RE.match(term)
    if m:
        return {(c, u) for c, u in units if (c, u.split("/")[0]) == m.groups()}
    hit = {(c, u) for c, u in units if _binary_id(c, u) == term}
    return hit or None


def _planned_terms(st, prefix="", suffix=""):
    """The ` + `-joined terms of the first `test-tier.sh filter` call of `st`,
    the frame `prefix`...`suffix` taken off; None when the call or frame is
    missing."""
    for cmd in st.cmds:
        if cmd[:2] == ["scripts/test-tier.sh", "filter"] and len(cmd) > 2:
            f = cmd[2]
            if not (f.startswith(prefix) and f.endswith(suffix)):
                return None
            f = f[len(prefix):len(f) - len(suffix)]
            return f.split(" + ") if f else []
    return None


# The row `pipeline.plan.split_alone` moves `ALONE_LOCKS` into (GH #1191).
ALONE_STATION = "browser:alone"


def release_partition(stations, repo=None):
    """`(gaps, twice)` of a release plan (`{name: Station}`): the units of
    `all()` in no part and in two, the parts being `tests`, `browser:display`,
    `browser:alone` (once split) and the scenario station, each read from its
    planned filter.

    KR-TB-1 (d), fund tb-s1e re-check (Post-xhigh-8): the release stays the
    full suite, and only a pin of the wave held it so far -- `plan()` refuses
    a release this does not clear (fail-closed: a missing `tests`, a frame or
    a term form this cannot read is a gap, `("<station>", "<term>")`). A unit is
    `(crate, unit)` of `_test_sources`, plus `(crate, crate)` for each crate's
    lib/bin unit tests."""
    src = [(c, u) for c, u, _t in _test_sources(repo)]
    units = src + [(c, c) for c in sorted({c for c, _u in src})]
    everything = set(units)
    gaps, where = [], {u: [] for u in units}
    parts = (("tests", "all() - (", ")", True),
             ("browser:display", "(", ") - %s" % WEBKIT_TESTS, False),
             # fund 27 (re-check tb-s1e-alone): `pipeline.plan.split_alone` moves
             # the `ALONE_LOCKS` into a row of their own after this plan; read
             # behind the split, that row is a browser part of its own
             (ALONE_STATION, "", "", False),
             (SCENARIO_STATION, "", "", False))
    for name, prefix, suffix, minus in parts:
        st = stations.get(name)
        if st is None and not minus:
            continue   # a part not planned takes no unit; what it should hold is a gap
        terms = None if st is None else _planned_terms(st, prefix, suffix)
        if terms is None and name == "tests" and st is not None:
            terms = [] if _planned_terms(st) == ["all()"] else None
        if terms is None:
            gaps.append((name, "station or filter missing"))
            continue
        hit = set()
        for t in terms:
            h = _term_hits(t, units)
            if h is None:
                gaps.append((name, t))
                continue
            hit |= h
        for u in (everything - hit if minus else hit):
            where[u].append(name)
    gaps += [u for u in units if not where[u]]
    twice = [(c, u, w) for (c, u), w in where.items() if len(w) > 1]
    return gaps, twice


# --- explain (W3, GH #1117) --------------------------------------------------

IR_MODES = ("integration", "release")

# station -> (trigger labels, modes that plan it whatever the diff[, labels
# that do NOT trigger it]). A label is a class of `_classes_of_path` or a
# pseudo-label of `_trigger_labels`; `*` is a wildcard. MODES in the second
# slot means "every mode" (`always`). It mirrors the conditions of `plan()`
# and serves `--explain` only; `scripts/tests/test_gate_plan.py` (class
# `Explain`) holds the two together: every station `plan()` puts into a
# strand plan must name the path that planned it, or say `always`.
STATION_TRIGGERS = {
    "precheck": (("diff",), ()),
    "roadmap-anchors": ((), MODES),
    "adr-anchors": ((), MODES),
    "claims": ((), MODES),
    "tree-rules": ((), MODES),
    "test-bans-inventory": ((), IR_MODES + ("ci",)),
    "test-bans": ((), ("ci",)),
    "corpus-committed": ((), MODES),
    "corpus": (("corpus_source",), MODES),
    "catalogue": (("template", "catalogue"), IR_MODES),
    "shellcheck": (("shell", "gate_infra"), ("ci",)),
    "gate-selftest": (("gate_infra",), ("ci",)),
    "display-lab": (("display_lab",), IR_MODES),
    "llm-conformance": (("llm_conformance",), IR_MODES),
    "guide-selftest": (("evals_guide",), IR_MODES),
    "fmt": (("rust_src", "rust_test", "crate_file", "workspace"), ()),
    "clippy": (("rust_src", "rust_test", "crate_file", "workspace"), ()),
    "unwrap-budget": (("rust_src", "crate_file", "workspace", "unwrap_infra"), ("ci",)),
    "corridor": (("corridor",), ("ci",)),
    "doctests": (("rust_src", "crate_file", "workspace"), ()),
    "deny": (("workspace",), ("release",)),
    SCENARIO_STATION: ((), ("release",)),
    "scenarios:memory": (("evals_memory", "template:memory-hive"), ("release",)),
    "scenarios:builder": (("evals_builder", "template:*", "example:meclaw-os",
                           "example:organism"), IR_MODES, ("template:memory-hive",)),
    "scenarios:display": (("display_scenarios", "template:display"),
                          ("integration", "release", "ci")),
    "browser:display": (("display_browser", "template:display"), IR_MODES),
    "recall-harness": (("evals_memory", "evals_recall", "template:memory-hive",
                        "recall_src"), ("release",)),
    "deny-advisories": ((), ("release",)),
    "crate-isolation": ((), ("release",)),
    "persona-cases": (("evals_persona",), IR_MODES),
    "persona-receipt": (("persona_source", "evals_persona"), IR_MODES),
    "export-selftest": (("export_infra",), IR_MODES),
    "export-tree-tests": (("gate_infra", "make_export"), ("release",)),
    "export-audit": (("export_test", "template", "template_root", "catalogue", "make_export"),
                     IR_MODES),
}

# How many `<label>: <path>` entries one station lists before `+<n> more`: a ci
# plan is the whole tree, and a reason is read by a person.
REASON_CAP = 20


def _trigger_labels(path):
    """The classes of `path` plus the pseudo-labels `STATION_TRIGGERS` names."""
    labels = set(_classes_of_path(path)) | {"diff"}
    parts = path.split("/")
    if parts[0] == "templates" and len(parts) >= 3:
        labels.add("template:" + parts[1])
    if parts[0] == "templates" and len(parts) == 2 and path != "templates/README.md":
        # A file directly under templates/ is no template (`template` starts
        # at three segments), but the strand audit judges it (RC-XA M3).
        labels.add("template_root")
    if parts[0] == "examples" and len(parts) >= 3:
        labels.add("example:" + parts[1])
    if path.startswith("crates/meclaw-cells/src/") and "recall" in path:
        labels.add("recall_src")
    if path == MAKE_EXPORT:
        labels.add("make_export")
    if _is_exported_test(path):
        labels.add("export_test")
    return labels


def _capped(entries):
    if len(entries) <= REASON_CAP:
        return entries
    return entries[:REASON_CAP] + ["+%d more" % (len(entries) - REASON_CAP)]


def _station_reason(name, files, mode, select=None):
    """`reason` of one planned station (see `--explain` in the docstring)."""
    if name == "tests":
        return _capped(list(select["why"])) if select else []
    if name == "slow-tests":
        return ["station: tests"]
    spec = STATION_TRIGGERS[name]
    labels, modes = spec[0], spec[1]
    excluded = spec[2] if len(spec) > 2 else ()
    hits = []
    for p in files:
        for label in sorted(_trigger_labels(p)):
            if label in excluded:
                continue
            if any(fnmatch.fnmatchcase(label, want) for want in labels):
                hits.append("%s: %s" % (label, p))
    hits = _capped(hits)
    if modes == MODES:
        return hits or ["always"]
    if mode in modes:
        hits.append("mode: %s" % mode)
    return hits


def _browser_select(st):
    """`select` of `browser:display`: the lock binaries its commands drive."""
    extra, filters = set(), []
    for cmd in st.cmds:
        if cmd[:2] != ["scripts/test-tier.sh", "filter"] or len(cmd) < 3:
            continue
        filters.append(cmd[2])
        # `browser_cmds` joins the locks into ONE union and splits it by engine
        # (`(binary(/a/) + binary(/b/)) - test(/webkit/)`), so each binary term
        # is read on its own.
        for pattern in _BINARY_TERM_RE.findall(cmd[2]):
            rx = re.compile(pattern)
            for lock in BROWSER_LOCKS:
                stem = lock.rsplit("/", 1)[1].removesuffix(".rs")
                if rx.search(stem):
                    extra.add("%s::%s" % (_crate_of(lock), stem))
        # the moved browser units (rule 11), flat or a folded harness module
        extra.update(re.findall(r"binary_id\(=([^)]+)\)", cmd[2]))
    return {"binaries": [], "rdeps": [], "all": False, "floor": False,
            # Each part's filter in parentheses: a part is `(union) - test(..)`.
            "extra": sorted(extra), "minus": [],
            "filter": " + ".join("(%s)" % f for f in filters) if len(filters) > 1
            else "".join(filters)}


# --- rendering --------------------------------------------------------------

def to_json(paths, mode, repo=None, explain=False):
    stations = plan(paths, mode, repo)
    files = _norm(paths)
    select = None
    if explain and any(st.name == "tests" for st in stations):
        select = test_select(files, mode, repo)
    rows = []
    for st in stations:
        row = {"name": st.name, "scope": st.scope, "cargo": bool(st.cargo)}
        if len(st.cmds) == 1:
            row["cmd"] = st.cmds[0]
            if st.cwds[0]:
                row["cwd"] = st.cwds[0]
        else:
            row["cmds"] = st.cmds
            if any(st.cwds):
                row["cwds"] = list(st.cwds)
        if not st.run:
            row["run"] = False
        if st.parallel:
            row["parallel"] = st.parallel
        if st.name in ENV_STATIONS:
            row["env"] = True
        if explain:
            row["reason"] = _station_reason(st.name, files, mode, select)
            if st.name == "tests" and select is not None:
                row["select"] = {k: v for k, v in select.items() if k != "why"}
            elif st.name == SCENARIO_STATION:
                row["select"] = {"binaries": [], "rdeps": [], "all": False, "floor": False,
                                 "extra": scenario_binaries(repo), "minus": [],
                                 "filter": st.cmds[0][2]}
            elif st.name == "browser:display":
                row["select"] = _browser_select(st)
                # `<label>: <path>` with the diff path whose rule reached a
                # browser unit: the cache proves its inputs cover the trigger
                # (`cache.uncovered_triggers`); the units are in the cmds
                row["reason"] = _capped(row["reason"] + [
                    "browser unit: %s" % p for p in _browser_reach(files, mode, repo)[1]])
        rows.append(row)
    return {
        "mode": mode,
        "classes": sorted(classify(paths)),
        "crates": sorted(crates_of(paths, repo)),
        "stations": rows,
    }


def to_tsv(paths, mode, repo=None):
    lines = []
    for st in plan(paths, mode, repo):
        for cmd, cwd in zip(st.cmds, st.cwds):
            lines.append("\t".join([
                st.name, st.scope, "1" if st.cargo else "0",
                " ".join(shlex.quote(a) for a in cmd), cwd or ""]))
    return "\n".join(lines)


def main(argv=None):
    ap = argparse.ArgumentParser(
        prog="gate_plan.py",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="Resolve changed paths into the gate stations that must run.",
        epilog="TSV output has EXACTLY five tab-separated columns on every row:\n"
               "  name <TAB> scope <TAB> cargo(0|1) <TAB> shell-quoted cmd <TAB> cwd\n"
               "The fifth column is empty, not absent, when no working directory\n"
               "is needed, so a shell reader must name all five variables:\n"
               "  while IFS=$'\\t' read -r name scope cargo cmd cwd; do ...; done\n"
               "The `run` flag lives in the JSON only; ci mode plans the `tests`\n"
               "station with \"run\": false because the CI shards run it.")
    ap.add_argument("--mode", choices=MODES)
    ap.add_argument("--files-from", metavar="FILE",
                    help="read paths from FILE, one per line ('-' = stdin)")
    ap.add_argument("--files", nargs="*", default=[], metavar="PATH")
    ap.add_argument("--repo", metavar="DIR",
                    help="tree the test sources are read from (default: this repo)")
    ap.add_argument("--format", choices=("json", "tsv"), default="tsv")
    ap.add_argument("--explain", action="store_true",
                    help="JSON only: add `reason` to every station and `select` to "
                         "`tests` and `browser:display` (see the docstring)")
    ap.add_argument("--terms", action="store_true",
                    help="print the test terms and the rule behind each as JSON "
                         "(`{term: [reason, ...]}`, the view `test_terms`) instead of the plan")
    ap.add_argument("--print", dest="what", choices=("scenario", "ignored", "persona-sources",
                                                    "env-stations", "lane-local", "lane-env"),
                    help="print a constant and exit (`ignored`, `persona-sources`: "
                         "one path per line; `persona-sources` reads `--repo`)")
    args = ap.parse_args(argv)

    if args.what == "scenario":
        # Narrowed to the tree (`--repo`, default this one): `test-tier.sh`
        # builds `all() - ( <this> )`, and an empty class is `none()`.
        print(scenario_for(args.repo) or "none()")
        return 0

    if args.what == "env-stations":
        for name in ENV_STATIONS:
            print(name)
        return 0

    if args.what == "lane-local":
        for name, why in LANE_LOCAL.items():
            print("%s\t%s" % (name, why))
        return 0

    if args.what == "lane-env":
        # `KEY=value` per line: the `.env` the kit writes into a lane's tree.
        for name, value in LANE_ENV.items():
            print("%s=%s" % (name, value))
        return 0

    if args.what == "ignored":
        # One path per line, for `scripts/gate.sh`: a run artefact that the
        # suites rewrite while the gate runs must not make the tree look dirty
        # in the receipt, or every release refuses on its own scenario station.
        for path in IGNORED:
            print(path)
        return 0

    if args.what == "persona-sources":
        # One file per line, for `workshop/evals/persona/fingerprint.py`: the
        # files the persona fingerprint is computed over, expanded with the
        # matcher the class uses.
        for path in persona_source_files(args.repo):
            print(path)
        return 0

    if not args.mode:
        ap.error("--mode is required (or use --print)")

    paths = list(args.files)
    if args.files_from:
        if args.files_from == "-":
            paths += sys.stdin.read().splitlines()
        else:
            try:
                with open(args.files_from, encoding="utf-8") as fh:
                    paths += fh.read().splitlines()
            except OSError as exc:
                ap.error("--files-from %s: %s" % (args.files_from, exc.strerror))

    try:
        if args.terms:
            print(json.dumps(test_terms(paths, args.repo), indent=2, sort_keys=True))
        elif args.format == "json":
            print(json.dumps(to_json(paths, args.mode, args.repo, explain=args.explain),
                             indent=2))
        else:
            text = to_tsv(paths, args.mode, args.repo)
            if text:
                print(text)
    except GatePlanError as exc:
        print("gate_plan: %s" % exc, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
