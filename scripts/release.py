#!/usr/bin/env python3
"""One command from the release gate to the website, with one halt.

    scripts/release.sh prepare <x.y.z> --wave <wave> --strand <relgate> [--host <lane>] [--issue N]
    scripts/release.sh publish <x.y.z> --wave <wave> [--title <text>]
    scripts/release.sh asset <x.y.z>
    common: [--repo <main tree>] [--config <toml>] [--reason <text>] [--dry-run]

WHY THIS EXISTS
===============
Up to 0.77.0 a release was about twelve hand steps between the release gate
and the website (scan, gate, test count, export, request, push, github-main,
tag, two CI watches, release notes, discussion, website), each a turn of an
agent and each a gap on the clock. Three of them had each been forgotten at
least once (the website three times, github-main once, a piped `gh run watch`
that read a red run as green). Here they run as two phases without a turn in
between, and the one halt that stays is the release request to the
coordinator before anything goes public:

  prepare  master tip carries the release cut (version x.y.z in Cargo.toml and
           `## [x.y.z]` in CHANGELOG.md, made by `strand.sh merge --release`),
           else exit 2. A detached worktree on that tip, german_scan in both
           stages, `strand.sh gate release` (on master's tip: the gate's
           export audit exports `master`, GH #1533), `pipeline.py release
           count --write` plus its commit, the release's line under
           `ROADMAP.md` § Shipped plus its commit (the first sentence of the
           CHANGELOG intro, then `check_roadmap_anchors.py` green; GH #1518,
           after the gate since GH #1533), `make_export.py` on the release
           head (master plus those two record commits) with the release
           receipt, the release notes out of the CHANGELOG section, the
           website's new `Current version:` paragraph out of the CHANGELOG
           intro (the one before moves into the history of a file that keeps
           one, `Previous: ...`; a website commit, no push; GH #1592).
           Ends with the four-line release request and exit 0.
  publish  master and origin/main unchanged since prepare (else exit 2), push
           main, `github-main`, master fast-forward to the release head,
           `tag_release.sh`, `gh run watch` on ci and release (never through a
           pipe, each with a hang mark), `gh release edit --notes-file`, the
           discussion, the website (version, test count, sitemap date), its
           deploy as a file list, `publish_docs.sh`, a live check (`curl -L`:
           it follows the server's slash redirect, GH #1396) and the
           website push, then `asset` as its last step. Ends with one line
           `RELEASE x.y.z tag=... ci=... release=... website=...`.
  asset    `gh release download` of the release's binary tarball
           (`meclaw-<x.y.z>-<target>.tar.gz`, target `[asset] target`,
           default x86_64-unknown-linux-musl) with its `.sha256` sibling; a
           sum that differs is red and lands nothing. Else the binary goes to
           `<dir>/meclaw-<x.y.z>-asset` (`[asset] dir`, default ~/kb/bin)
           beside `meclaw-<x.y.z>-asset.origin.json` (version, commit of the
           tag, sha256 of the binary, source), and one JSON line names version,
           commit and sha256. A binary already there with the same sum stays;
           one with another sum is red. Up to 0.89.6 a hand command.

THE GATE'S LANE
===============
The release gate is bound by capacity: 39.7-41.7 min on a lane with 24
vCPU, about 51 min on one with 12 (GH #1392). `--host <lane>` goes to
`strand.sh gate release` as given. Without it, prepare picks the reachable
build lane (`lanes status`: `reachable`) with the most vCPU in its
fingerprint (`vcpu`), a tie by name; the owner's machine (`class=egon`) is
never picked, and with no reachable lane (or no host file) the gate gets no
`--host` and the admission picks as before. The lane lands in the state and
in the release request. One gate, one lane: no parallel gate here.

THE STATE FILE IS THE STOP WATCH
================================
Every step writes a start and an end line into
`plans/<wave dir>/release-<x.y.z>.state` (JSON, one line each) in the main
tree. A green step never runs again; a red step runs again only with
`--reason <text>`, and the reason is a line of its own. The same lines are the
station times of a release measurement -- there is no second tool for that.

WHAT IS NOT HERE
================
Hosts, paths and IDs: they live in a private config file
(`~/.config/meclaw/release.toml`, `--config`), never in this script. Text:
the release notes, the discussion and the website's `Current version:`
paragraph are taken from the CHANGELOG section; a paragraph already there for
the release (written by hand) stays as it is.

THE ONE RED THAT CAN BE ACCEPTED
================================
A red public CI run (`ci`) is accepted with
`--accept-red ci:"OR-<id>: <reason>"` (the ruling id is mandatory, else exit 2):
the step ends GREEN with `ci: failure-accepted-OR-<id>` and an `accepted` line
with the reason in the state; the acceptance is the reason of its retry. Up to
0.89.2 that line was written into the state by hand (GH #1592). The release run
builds the assets and takes no acceptance.
No `cd`, no environment prefixes: every git call is `git -C`, every tool is
called by its path.

Exit: 0 done (prepare: halted at the request), 1 a red step, 2 a refusal
(usage, missing cut, master moved, a red step again without a reason).
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import tomllib
from pathlib import Path

# `roadmap` after `gate` and `count` (GH #1533): the gate's export audit
# exports `master` and refuses a receipt on another tree, so a commit in the
# release worktree before the gate made the release gate of 0.87.0 red.
PREPARE = ("master", "worktree", "german-scan", "german-strings", "gate", "count", "roadmap",
           "export", "notes", "website-check")
PUBLISH = ("verify", "push", "github-main", "master-ff", "tag", "ci", "release-run",
           "release-notes", "discussion", "website", "deploy", "docs", "live-check",
           "website-push", "asset")
# Steps that check, never act: they run on every call and are never skipped.
ALWAYS = {"master", "verify"}

DEFAULT_CONFIG = Path.home() / ".config" / "meclaw" / "release.toml"
ASSET_DIR = Path.home() / "kb" / "bin"
ASSET_TARGET = "x86_64-unknown-linux-musl"
VERSION_RE = re.compile(r"^\d+\.\d+\.\d+$")
SEMVER = r"\d+\.\d+\.\d+"
# Every place on the website that names the current release (the 0.77.0 deploy:
# the pill text and link, the footer's `Changelog` link, the JSON-LD
# softwareVersion, the llms `[Changelog at vX](.../blob/vX/...)` link). Each
# pattern takes the whole marked place, never a bare link: a link to an older
# release in prose (llms-full.txt's `Previous: 0.61.4. ... blob/v0.61.4/
# CHANGELOG.md`, review I2) is history and stays byte-equal, as does a version
# in prose ("v0.29.0 means v0.29.0").
SITE_PATTERNS = (
    (re.compile(r'(/releases/tag/v)' + SEMVER + r'("[^>]*>v)' + SEMVER + '(<)'), r"\g<1>{v}\g<2>{v}\g<3>"),
    (re.compile(r'(/blob/v)' + SEMVER + r'(/CHANGELOG\.md"[^>]*>Changelog<)'), r"\g<1>{v}\g<2>"),
    (re.compile(r'("softwareVersion":\s*")[^"]*(")'), r"\g<1>{v}\g<2>"),
    (re.compile(r'(\[Changelog at v)' + SEMVER + r'(\]\([^)\s]*/blob/v)' + SEMVER + r'(/CHANGELOG\.md\))'),
     r"\g<1>{v}\g<2>{v}\g<3>"),
)
# One <url> block of a sitemap; its <loc> and <lastmod> need not be neighbours
# (the live sitemap carries two xhtml:link lines between them, review I1).
URL_BLOCK = re.compile(r"<url>.*?</url>", re.S)
SITE_MARKER = re.compile(r"/releases/tag/v" + SEMVER + r"|Current version: " + SEMVER)
TESTS_RE = re.compile(r"(The release gate passed )\d+( tests)")
CURRENT = "Current version: "
PREVIOUS = "Previous: "
# --accept-red: the steps a red can be accepted for, and the ruling it cites
# (OR-DT.ZI.decide-or: an owner's answer cites the ruling that carries it).
ACCEPTABLE = ("ci",)
ACCEPT_RE = re.compile(r"^(?P<step>[\w-]+):(?P<why>\s*(?P<ruling>OR-[\w.-]*\w)\s*:\s*\S.*)$")


class Refusal(Exception):
    """Exit 2: nothing ran, the caller has to change something first."""


class Red(Exception):
    """Exit 1: this step is red."""


def now():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class State:
    """The append-only line file of one release."""

    def __init__(self, path):
        self.path = Path(path)

    def lines(self):
        if not self.path.exists():
            return []
        return [json.loads(line) for line in self.path.read_text().splitlines() if line.strip()]

    def add(self, **entry):
        entry = {"t": now(), **entry}
        self.path.parent.mkdir(parents=True, exist_ok=True)
        with self.path.open("a") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")

    def last(self, step):
        """The verdict of the step's last end line, or None."""
        verdict = None
        for e in self.lines():
            if e.get("step") == step and e.get("event") == "end":
                verdict = e.get("verdict")
        return verdict

    def data(self):
        out = {}
        for e in self.lines():
            if e.get("event") == "end" and e.get("verdict") == "GREEN":
                out.update(e.get("data") or {})
        return out


def run(cmd, *, timeout=None, check_rc=True, what=None):
    """Run one command, its output passed through and kept. Never a shell."""
    print("$ " + " ".join(shlex.quote(str(c)) for c in cmd), flush=True)
    try:
        proc = subprocess.Popen([str(c) for c in cmd], stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, text=True)
    except OSError as e:
        raise Red(f"{what or cmd[0]}: cannot start: {e}") from e
    out = []

    def pump():
        for line in proc.stdout:
            sys.stdout.write(line)
            out.append(line)

    # The reader is a thread so the hang mark holds without a single line of
    # output: a stalled `gh run watch` printed nothing and the old in-loop
    # deadline was never checked (review I3). A mark, not a verdict on time.
    reader = threading.Thread(target=pump, daemon=True)
    reader.start()
    try:
        proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        reader.join(5)
        raise Red(f"HANG: {what or cmd[0]}: no verdict after {timeout}s (hang mark)")
    reader.join()
    sys.stdout.flush()
    text = "".join(out)
    if check_rc and proc.returncode != 0:
        raise Red(f"{what or cmd[0]}: exit {proc.returncode}")
    return proc.returncode, text


def read(cmd):
    """A read-only call whose output is data, not progress."""
    try:
        proc = subprocess.run([str(c) for c in cmd], capture_output=True, text=True)
    except OSError as e:  # a missing `gh` is a red step, not a traceback (review I4)
        raise Red(f"{cmd[0]}: cannot start: {e}") from e
    if proc.returncode != 0:
        raise Red(f"{' '.join(map(str, cmd[:4]))}: exit {proc.returncode}: {proc.stderr.strip()}")
    return proc.stdout.strip()


def git(repo, *args):
    return read(["git", "-C", repo, *args])


def sha256_of(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def fetch_asset(version, root, cfg):
    """The release's binary into the asset folder, checked against the release's sum.

    Returns {version, commit, sha256} and prints it as one JSON line."""
    repo = cfg.get("gh_repo")
    if not repo:
        raise Red("asset: the config carries no gh_repo")
    acfg = cfg.get("asset") or {}
    name = f"meclaw-{version}-{acfg.get('target', ASSET_TARGET)}"
    tag = f"v{version}"
    commit = git(root, "rev-parse", f"{tag}^{{commit}}")
    folder = Path(os.path.expanduser(str(acfg.get("dir", ASSET_DIR))))
    dest = folder / f"meclaw-{version}-asset"
    origin = folder / f"{dest.name}.origin.json"
    folder.mkdir(parents=True, exist_ok=True)
    # staged beside the target: a red check leaves nothing, a green one is one rename
    with tempfile.TemporaryDirectory(dir=folder, prefix=".asset-") as tmp:
        run(["gh", "release", "download", tag, "-R", repo, "-D", tmp, "-p", name + ".tar.gz*"],
            timeout=600, what="gh release download")
        tgz, sumfile = Path(tmp) / f"{name}.tar.gz", Path(tmp) / f"{name}.tar.gz.sha256"
        if not tgz.exists() or not sumfile.exists():
            raise Red(f"asset: {tag} has no {tgz.name} with its .sha256")
        want = (sumfile.read_text().split() or [""])[0]
        have = sha256_of(tgz)
        if have != want:
            raise Red(f"asset {tgz.name}: sha256 {have[:12]} differs from the release's {want[:12]}")
        with tarfile.open(tgz) as tf:
            try:
                member = tf.getmember(f"{name}/meclaw")
            except KeyError:
                raise Red(f"asset {tgz.name}: no {name}/meclaw inside") from None
            if not member.isfile():
                raise Red(f"asset {tgz.name}: {name}/meclaw is no file")
            data = tf.extractfile(member).read()
        sha = hashlib.sha256(data).hexdigest()
        if dest.exists():
            if sha256_of(dest) != sha:
                raise Red(f"asset: {dest} exists with another sha256; move it aside first")
        else:
            staged = Path(tmp) / "meclaw"
            staged.write_bytes(data)
            staged.chmod(0o755)
            os.replace(staged, dest)
        if not origin.exists():
            staged = Path(tmp) / "origin.json"
            staged.write_text(json.dumps({
                "version": version, "commit": commit, "sha256": sha, "tag": tag, "tgz_sha256": have,
                "source": f"GH release asset {tgz.name} of {repo} via gh release download "
                          f"(sha256 file checked)",
                "made_by": "scripts/release.sh asset", "made_at": now()}, sort_keys=True) + "\n")
            os.replace(staged, origin)
    out = {"version": version, "commit": commit, "sha256": sha}
    print(json.dumps(out), flush=True)
    return out


def lane_module():
    """The kit's lane module (host file, `lanes status`, fingerprint); a seam for the tests."""
    here = str(Path(__file__).resolve().parent)
    if here not in sys.path:
        sys.path.insert(0, here)
    from pipeline import lane
    return lane


def widest_lane(mod):
    """The reachable build lane with the most vCPU, a tie by name; None without one.

    The release gate took 39.7-41.7 min on 24 vCPU and about 51 on 12 (GH
    #1392). The owner's machine (`class=egon`) runs the live colonies and is
    never a release lane; a lane that does not answer `lanes status` or its
    fingerprint is left out, not waited for."""
    best = None
    for ln in mod.load_lanes():
        if ln.cls == "egon":
            continue
        if mod.status(ln).split()[:1] != ["reachable"]:
            continue
        try:
            vcpu = int((mod.fingerprint(ln) or {}).get("vcpu") or 0)
        except Exception:  # noqa: BLE001 -- no answer is no candidate
            continue
        if vcpu and (best is None or (-vcpu, ln.name) < (-best[0], best[1])):
            best = (vcpu, ln.name)
    return best


def changelog_section(text, version):
    """The body of `## [version]` (heading excluded) and its date."""
    lines = text.splitlines()
    head = re.compile(r"^## \[" + re.escape(version) + r"\](?:\s*[—-]\s*(\d{4}-\d{2}-\d{2}))?")
    for i, line in enumerate(lines):
        m = head.match(line)
        if m:
            body = []
            for rest in lines[i + 1:]:
                if rest.startswith("## ["):
                    break
                body.append(rest)
            return "\n".join(body).strip() + "\n", m.group(1)
    return None, None


def intro_of(section):
    """The section's first paragraph before its first sub-heading."""
    para = []
    for line in section.splitlines():
        if line.startswith("#"):
            break
        if not line.strip():
            if para:
                break
            continue
        para.append(line.strip())
    return " ".join(para)


# ---- the roadmap step (GH #1518) -------------------------------------------
# `docs/development-rules.md` § 1 and § 5b: between the CHANGELOG cut and the
# export every release appears once under `ROADMAP.md` § Shipped, and
# `scripts/check_roadmap_anchors.py` runs green. As a sentence in a prompt it
# was dropped from 0.77.1 to 0.86.1: § Shipped stood at v0.77.0 while fifteen
# releases shipped after it.
SHIPPED_HEAD = re.compile(r"^##\s+Shipped\b")
SHIPPED_BULLET = re.compile(r"^- v(\d+)\.(\d+)\.(\d+):")
# The width of the lines already there (117 at most up to v0.77.0).
SHIPPED_WIDTH = 116
# A parenthesis that holds issue links only: `([#7](...))`, `([#1](...), [#2](...))`.
ISSUE_LINK_GROUP = re.compile(r"\s*\((?:\[#\d+\]\([^)\s]*\)(?:,\s*)?)+\)")
ISSUE_LINK = re.compile(r"\[(#\d+)\]\([^)\s]*\)")
MD_LINK = re.compile(r"\[([^\]]*)\]\([^)\s]*\)")


def first_sentence(text):
    """Up to the first `.`, `!` or `?` outside parentheses and code spans that is
    followed by a blank and a word that does not start lower-case."""
    depth, code = 0, False
    for i, ch in enumerate(text):
        if ch == "`":
            code = not code
        elif code:
            continue
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth = max(0, depth - 1)
        elif ch in ".!?" and depth == 0:
            rest = text[i + 1:]
            if not rest.strip():
                return text[:i + 1]
            if rest[:1].isspace() and not rest.lstrip()[:1].islower():
                return text[:i + 1]
    return text


def shipped_line(section, version):
    """The § Shipped line of `version`: the first sentence of the CHANGELOG
    section's intro word for word, its issue links reduced to the number (a
    parenthesis of links alone goes), its first word lower-case unless it is an
    identifier or an acronym, wrapped like the lines already there."""
    import textwrap
    intro = intro_of(section or "")
    if not intro:
        raise Red(f"CHANGELOG.md `## [{version}]` has no intro paragraph to take the "
                  f"§ Shipped line from; write the line by hand, then run again with --reason")
    text = ISSUE_LINK_GROUP.sub("", first_sentence(intro))
    text = MD_LINK.sub(r"\1", ISSUE_LINK.sub(r"\1", text)).strip()
    word = text.split(" ", 1)[0].rstrip(":,;")
    if word[:1].isupper() and word.isalpha() and word[1:] == word[1:].lower():
        text = text[0].lower() + text[1:]
    return textwrap.fill(f"- v{version}: {text}", width=SHIPPED_WIDTH, subsequent_indent="  ",
                         break_long_words=False, break_on_hyphens=False)


def shipped_rows(text):
    """The lines of § Shipped (heading to the next `## `), none without it."""
    lines = text.split("\n")
    head = next((i for i, row in enumerate(lines) if SHIPPED_HEAD.match(row)), None)
    if head is None:
        return []
    end = next((i for i in range(head + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    return lines[head + 1:end]


def with_shipped_line(text, version, line):
    """`text` with `line` under § Shipped, newest on top: above the first line of
    an older release, else below the last release line. A release that already
    has its line leaves the text byte-equal (a second run adds nothing)."""
    lines = text.split("\n")
    head = next((i for i, row in enumerate(lines) if SHIPPED_HEAD.match(row)), None)
    if head is None:
        raise Red("ROADMAP.md has no `## Shipped`")
    end = next((i for i in range(head + 1, len(lines)) if lines[i].startswith("## ")), len(lines))
    new = tuple(int(x) for x in version.split("."))
    at = after = None
    for i in range(head + 1, end):
        m = SHIPPED_BULLET.match(lines[i])
        if not m:
            continue
        have = tuple(int(x) for x in m.groups())
        if have == new:
            return text
        if have < new and at is None:
            at = i
        after = i + 1
        while after < end and lines[after].startswith("  "):
            after += 1
    if at is None:
        at = after
    if at is None:  # no release line yet: below the section's text
        at = end
        while at > head + 1 and not lines[at - 1].strip():
            at -= 1
        line = "\n" + line
    return "\n".join(lines[:at] + line.split("\n") + lines[at:])


def workspace_version(cargo_toml):
    try:
        data = tomllib.loads(cargo_toml)
    except tomllib.TOMLDecodeError:
        return None
    return ((data.get("workspace") or {}).get("package") or {}).get("version") \
        or (data.get("package") or {}).get("version")


def sitemap_lastmod(text, loc):
    """The <lastmod> of the <url> block whose <loc> is exactly `loc`, or None."""
    for block in URL_BLOCK.findall(text):
        if f"<loc>{loc}</loc>" in block:
            m = re.search(r"<lastmod>([^<]*)</lastmod>", block)
            return m.group(1) if m else None
    return None


def page_url(base, rel):
    """`index.html` -> `<base>/`, `blog/index.html` -> `<base>/blog/`."""
    if rel == "index.html":
        return base.rstrip("/") + "/"
    if rel.endswith("/index.html"):
        return base.rstrip("/") + "/" + rel[: -len("index.html")]
    return base.rstrip("/") + "/" + rel


def rewrite_site_text(rel, text, version, tests, date, url):
    """One website file on the new release: every release place, the test count
    in the `Current version: <version>.` paragraph, the sitemap date of `/`."""
    for pattern, repl in SITE_PATTERNS:
        text = pattern.sub(repl.replace("{v}", version), text)
    if rel.endswith(".txt") and tests is not None:
        paras = text.split("\n\n")
        for i, para in enumerate(paras):
            if para.startswith(f"Current version: {version}."):
                paras[i] = TESTS_RE.sub(rf"\g<1>{tests}\g<2>", para)
        text = "\n\n".join(paras)
    if rel.endswith("sitemap.xml") and date:
        loc = f"<loc>{url.rstrip('/')}/</loc>"

        def stamp(m):
            block = m.group(0)
            if loc not in block:
                return block
            return re.sub(r"(<lastmod>)[^<]*(</lastmod>)", rf"\g<1>{date}\g<2>", block, count=1)
        text = URL_BLOCK.sub(stamp, text)
    return text


def site_paragraph(version, intro, tests):
    """The website's `Current version:` paragraph of `version` out of the
    CHANGELOG intro: code marks and issue links go (the hand text of 0.89.6 is
    the intro without them, llms.txt:80), a link keeps its text, the test count
    closes it (GH #1592; `step_website` moves the count as before)."""
    text = ISSUE_LINK_GROUP.sub("", intro or "")
    text = MD_LINK.sub(r"\1", ISSUE_LINK.sub(r"\1", text)).replace("`", "")
    text = " ".join(text.split())
    if text and text[-1] not in ".!?":
        text += "."
    return f"{CURRENT}{version}. {text} The release gate passed {tests} tests."


def write_current_paragraph(text, version, para):
    """`text` with `para` as its `Current version:` paragraph. One already on
    `version` stays byte-equal (a hand text wins); the one before goes into
    the history as `Previous: ...` where the file keeps one (llms-full.txt),
    else it is replaced (llms.txt). A file without the paragraph stays."""
    paras = text.split("\n\n")
    for i, old in enumerate(paras):
        if not old.startswith(CURRENT):
            continue
        if old.startswith(f"{CURRENT}{version}."):
            return text
        if any(p.startswith(PREVIOUS) for p in paras):
            paras[i:i + 1] = [para, PREVIOUS + old[len(CURRENT):]]
        else:
            paras[i] = para
        return "\n\n".join(paras)
    return text


class Release:
    def __init__(self, args, cfg):
        self.a = args
        self.cfg = cfg
        self.v = args.version
        self.root = Path(args.repo).resolve()
        self.wdir = self.wave_dir()
        self.state = State(self.root / "plans" / self.wdir / f"release-{self.v}.state")
        self.data = self.state.data()

    # ---- frame ------------------------------------------------------------
    def wave_dir(self):
        plans = self.root / "plans"
        if (plans / self.a.wave).is_dir():
            return self.a.wave
        hits = sorted(p.name for p in plans.glob(self.a.wave + "-*") if p.is_dir())
        if len(hits) != 1:
            raise Refusal(f"wave {self.a.wave}: {len(hits)} directories under plans/ (expected one)")
        return hits[0]

    def need(self, *keys):
        node = self.cfg
        for k in keys:
            if not isinstance(node, dict) or k not in node:
                raise Red(f"config {self.a.config}: missing {'.'.join(keys)}")
            node = node[k]
        return node

    def wt(self):
        return Path(self.data["worktree"])

    def steps(self, names):
        """Run the phase: green steps skip, red ones need a reason, stop at red."""
        for step in names:
            last = self.state.last(step)
            if self.a.dry_run:
                print(f"DRY {step} -- {'done' if last == 'GREEN' and step not in ALWAYS else 'would run'}")
                if step in ALWAYS:
                    getattr(self, "step_" + step.replace("-", "_"))()
                continue
            if last == "GREEN" and step not in ALWAYS:
                print(f"release: {step} done, skipped")
                continue
            if last == "RED" and step not in ALWAYS:
                accepted = self.a.accepted.get(step)
                if accepted and not self.a.reason:
                    self.a.reason = "accept-red " + accepted[1]
                if not self.a.reason:
                    raise Refusal(f"step {step} was red; fix it, then pass --reason <text> to run it again")
                self.state.add(step=step, event="retry", reason=self.a.reason)
            self.state.add(step=step, event="start")
            t0 = time.monotonic()
            try:
                got = getattr(self, "step_" + step.replace("-", "_"))() or {}
            except Red as e:
                self.state.add(step=step, event="end", verdict="RED", reason=str(e),
                               secs=round(time.monotonic() - t0, 1))
                print(f"release: {step} RED -- {e}", flush=True)
                return 1
            except Refusal as e:
                self.state.add(step=step, event="end", verdict="REFUSED", reason=str(e),
                               secs=round(time.monotonic() - t0, 1))
                raise
            except Exception as e:  # noqa: BLE001 -- every step ends with an end line (review I4)
                reason = f"{type(e).__name__}: {e}"
                self.state.add(step=step, event="end", verdict="RED", reason=reason,
                               secs=round(time.monotonic() - t0, 1))
                print(f"release: {step} RED -- {reason}", flush=True)
                return 1
            self.data.update(got)
            self.state.add(step=step, event="end", verdict="GREEN", data=got,
                           secs=round(time.monotonic() - t0, 1))
            print(f"release: {step} GREEN", flush=True)
        return 0

    # ---- prepare ----------------------------------------------------------
    def step_master(self):
        tip = git(self.root, "rev-parse", "master")
        if self.data.get("master") and self.data["master"] != tip:
            raise Refusal(f"master moved since prepare began ({self.data['master'][:12]} -> {tip[:12]}); "
                          f"a new release needs a new state file")
        have = workspace_version(git(self.root, "show", f"{tip}:Cargo.toml"))
        if have != self.v:
            raise Refusal(f"master {tip[:12]} carries version {have}, not {self.v}: "
                          f"cut it with `strand.sh merge --release {self.v}`")
        section, _ = changelog_section(git(self.root, "show", f"{tip}:CHANGELOG.md"), self.v)
        if section is None:
            raise Refusal(f"master {tip[:12]}: CHANGELOG.md has no `## [{self.v}]`")
        return {"master": tip}

    def step_worktree(self):
        base = os.environ.get("MECLAW_WORKTREE_ROOT")
        wave = re.sub(r"-\d{4}-\d{2}-\d{2}$", "", self.wdir)
        if base:
            path = Path(base) / self.root.name / wave / self.a.strand
        else:
            path = self.root.parent / f"{self.root.name}-wt-{self.a.strand}"
        if path.exists():
            if git(path, "rev-parse", "HEAD") != self.data["master"]:
                raise Red(f"{path} exists on another commit; remove it or pick another --strand")
        else:
            run(["git", "-C", self.root, "worktree", "add", "--detach", path, self.data["master"]],
                what="worktree add")
        return {"worktree": str(path), "strand": self.a.strand}

    def step_roadmap(self):
        """The release's line under `ROADMAP.md` § Shipped, committed on top of
        the test-count commit, then the anchor check green (GH #1518,
        development-rules § 1/§ 5b). Returns the release head the export takes.

        After the gate, beside the test count (GH #1533): the release gate's
        export audit exports `master` and refuses a receipt on any other tree;
        a roadmap commit before the gate put the release worktree one commit
        ahead of master, and the release gate of 0.87.0 was red on exactly
        that. Like the test-count commit, this one travels in the release head
        (`make_export.py --rev`), and the export's own audit checks the
        roadmap anchors there (R15). No language scan reads ROADMAP.md (R8 and
        the German-scan steps cover crates/tests/examples); the line is taken
        word for word from the English CHANGELOG intro. A line under § Shipped and
        nothing else is a release record (`turnaround.release_offending`), so
        the release receipt on master and a battery measured on the cut both
        cover the release head; any other ROADMAP.md change is foreign. A tree
        without ROADMAP.md writes nothing here; the gate's `roadmap-anchors`
        station is red on it.

        The line is the first sentence of the version's CHANGELOG intro, word
        for word: that sentence has to say what the release ships, not frame it
        ("a patch on 0.86.0" alone is a label). A line written by hand under
        § Shipped on the cut wins; the step then adds nothing.

        "Line already there" is decided at HEAD, never at the file in the
        worktree, and an uncommitted ROADMAP.md is red (review R1 M1): a run
        that wrote the line and then failed to commit it (a hook, a kill) left
        it dirty, and a rerun that read the file called it `present` and handed
        the export a release head without the line. Behind the gate nothing
        else catches that (before GH #1533 the gate's dirty receipt did)."""
        roadmap = self.wt() / "ROADMAP.md"
        if not roadmap.is_file():
            print("release: roadmap -- no ROADMAP.md in the release tree, no line to write", flush=True)
            return {"roadmap": "absent", "release_head": git(self.wt(), "rev-parse", "HEAD")}
        section, _ = changelog_section((self.wt() / "CHANGELOG.md").read_text(), self.v)
        if section is None:
            raise Red(f"CHANGELOG.md has no `## [{self.v}]`")
        dirty = git(self.wt(), "status", "--porcelain", "--", "ROADMAP.md")
        if dirty:
            raise Red(f"ROADMAP.md in the release worktree {self.wt()} has uncommitted changes "
                      f"({dirty.strip()}), a half run of this step or a hand edit: commit it or "
                      f"restore it (`git -C <worktree> checkout HEAD -- ROADMAP.md`), then run "
                      f"again with --reason")
        shown = subprocess.run(["git", "-C", str(self.wt()), "show", "HEAD:ROADMAP.md"],
                               capture_output=True, text=True)
        if shown.returncode != 0:
            raise Red(f"git show HEAD:ROADMAP.md: exit {shown.returncode}: {shown.stderr.strip()}")
        text = shown.stdout
        # a line already there (by hand, or a run before) needs no intro and gets no twin
        there = any(row.startswith(f"- v{self.v}:") for row in shipped_rows(text))
        got = text if there else with_shipped_line(text, self.v, shipped_line(section, self.v))
        state = "present"
        if got != text:
            roadmap.write_text(got)
            issue = f" (#{self.a.issue})" if self.a.issue else ""
            msg = (f"{self.a.wave} {self.a.strand}{issue}: ROADMAP § Shipped line for v{self.v} "
                   f"from its CHANGELOG section\n")
            with tempfile.NamedTemporaryFile("w", suffix=".msg", delete=False) as f:
                f.write(msg)
            try:
                run(["git", "-C", self.wt(), "add", "--", "ROADMAP.md"], what="git add")
                run(["git", "-C", self.wt(), "commit", "-q", "-F", f.name], what="git commit")
            finally:
                os.unlink(f.name)
            state = "written"
        run([sys.executable, "-I", self.wt() / "scripts/check_roadmap_anchors.py"],
            what="check_roadmap_anchors")
        return {"roadmap": state, "release_head": git(self.wt(), "rev-parse", "HEAD")}

    def step_german_scan(self):
        run([sys.executable, self.wt() / "plans/p5.5-fixtures/german_scan.py", "-C", self.wt(), "--strict"],
            what="german_scan --strict")

    def step_german_strings(self):
        run([sys.executable, self.wt() / "plans/p5.5-fixtures/german_scan.py", "-C", self.wt(),
             "--strings", "--strict"], what="german_scan --strings --strict")

    def gate_lane(self):
        """`--host` as given, else the widest reachable lane, else None (the admission picks)."""
        if self.a.host:
            return self.a.host
        best = widest_lane(lane_module())
        if best is None:
            print("release: no reachable build lane, the admission picks", flush=True)
            return None
        print(f"release: gate lane {best[1]} ({best[0]} vCPU, the widest reachable)", flush=True)
        return best[1]

    def step_gate(self):
        host = self.gate_lane()
        cmd = [self.wt() / "scripts/strand.sh", "-C", self.wt(), "gate", "release",
               "--strand", self.a.strand, "--wave", self.a.wave, "--wait-admission"]
        if host:
            cmd += ["--host", host]
        _, out = run(cmd, check_rc=False, what="gate release")
        summary = [line for line in out.splitlines() if line.startswith("GATE-SUMMARY ")]
        receipt = self.root / "plans" / self.wdir / "receipts" / self.a.strand / "latest" / "receipt.json"
        if not summary or not summary[-1].endswith(" GREEN"):
            raise Red("gate release: " + (summary[-1] if summary else "no summary line"))
        try:
            rec = json.loads(receipt.read_text())
        except (OSError, ValueError) as e:
            raise Red(f"gate release: no receipt at {receipt}: {e}") from e
        head = git(self.wt(), "rev-parse", "HEAD")
        if rec.get("verdict") != "GREEN" or rec.get("mode") != "release" or rec.get("rev") != head:
            raise Red(f"gate release: receipt {receipt} is not a green release receipt on {head[:12]}")
        return {"gate": summary[-1], "receipt": str(receipt.resolve()),
                "lane": rec.get("lane") or host or "-"}

    def step_count(self):
        _, out = run([self.wt() / "scripts/pipeline.py", "release", "count",
                      "--receipt", self.data["receipt"], "--write", self.wt()], what="release count")
        m = re.search(r"^release-count: (\d+) tests", out, re.M)
        if not m:
            raise Red("release count: no `release-count:` line")
        tests = int(m.group(1))
        if git(self.wt(), "status", "--porcelain", "--", "docs/status.en.md"):
            issue = f" (#{self.a.issue})" if self.a.issue else ""
            msg = (f"{self.a.wave} {self.a.strand}{issue}: Testzahl {tests} aus dem Release-Gate "
                   f"in docs/status.en.md\n")
            with tempfile.NamedTemporaryFile("w", suffix=".msg", delete=False) as f:
                f.write(msg)
            try:
                run(["git", "-C", self.wt(), "add", "--", "docs/status.en.md"], what="git add")
                run(["git", "-C", self.wt(), "commit", "-q", "-F", f.name], what="git commit")
            finally:
                os.unlink(f.name)
        return {"tests": tests, "release_head": git(self.wt(), "rev-parse", "HEAD")}

    def step_export(self):
        run(["git", "-C", self.root, "fetch", "-q", "origin", "main"], what="fetch origin main")
        public = git(self.root, "rev-parse", "origin/main")
        mirror = git(self.root, "rev-parse", "github-main")
        if public != mirror:
            raise Red(f"github-main {mirror[:12]} is not origin/main {public[:12]}: "
                      f"someone committed on public main; settle it before the export")
        _, out = run([sys.executable, self.wt() / "plans/export-fixtures/make_export.py",
                      "--rev", self.data["release_head"], "--parent", public,
                      "--gate-receipt", self.data["receipt"]], what="make_export")
        m = re.search(r"RECEIPT GRUEN -- Export-Commit: ([0-9a-f]{40})", out)
        if not m:
            raise Red("make_export: no export commit (tree identical to the parent, or red)")
        return {"export": m.group(1), "parent": public}

    def step_notes(self):
        section, date = changelog_section((self.wt() / "CHANGELOG.md").read_text(), self.v)
        if section is None:
            raise Red(f"CHANGELOG.md has no `## [{self.v}]`")
        notes = self.root / "plans" / self.wdir / f"release-{self.v}.notes.md"
        notes.write_text(section)
        return {"notes": str(notes), "date": date or datetime.date.today().isoformat(),
                "intro": intro_of(section)}

    def site_files(self):
        site = Path(self.need("website", "repo"))
        listed = (self.cfg.get("website") or {}).get("files")
        if listed:
            return site, list(listed)
        files = []
        for rel in git(site, "ls-files").splitlines():
            if rel.endswith(("sitemap.xml",)):
                files.append(rel)
                continue
            if not rel.endswith((".html", ".txt")):
                continue
            try:
                if SITE_MARKER.search((site / rel).read_text()):
                    files.append(rel)
            except (OSError, UnicodeDecodeError):
                continue
        return site, files

    def step_website_check(self):
        site, files = self.site_files()
        # GH #1592 (nr. 44): website-check was red on 0.84.0-0.89.6 seven times
        # until an agent wrote this paragraph by hand; now prepare writes it.
        para = site_paragraph(self.v, self.data.get("intro"), self.data.get("tests"))
        written = []
        for rel in files:
            if not rel.endswith(".txt"):
                continue
            old = (site / rel).read_text()
            new = write_current_paragraph(old, self.v, para)
            if new != old:
                (site / rel).write_text(new)
                written.append(rel)
        if written:
            run(["git", "-C", site, "add", "--", *written], what="website add")
            with tempfile.NamedTemporaryFile("w", suffix=".msg", delete=False) as f:
                f.write(f"v{self.v}: Absatz Current version aus dem CHANGELOG-Intro, "
                        f"Vorgänger in die Historie\n")
            try:
                run(["git", "-C", site, "commit", "-q", "-F", f.name], what="website commit")
            finally:
                os.unlink(f.name)
        missing = []
        for rel in files:
            if rel.endswith(".txt") and "Current version:" in (site / rel).read_text():
                if f"\nCurrent version: {self.v}." not in "\n" + (site / rel).read_text():
                    missing.append(rel)
        if missing:
            raise Red(f"website: no `Current version: {self.v}.` paragraph in {', '.join(missing)} "
                      f"(write it in {site}, then prepare again)")

    # ---- publish ----------------------------------------------------------
    def step_verify(self):
        for step in PREPARE:
            if self.state.last(step) != "GREEN":
                raise Refusal(f"prepare is not done (step {step}); run prepare first")
        tip = git(self.root, "rev-parse", "master")
        if tip not in (self.data["master"], self.data["release_head"]):
            raise Refusal(f"master moved since prepare ({self.data['master'][:12]} -> {tip[:12]}); "
                          f"prepare a new release")
        if self.state.last("push") != "GREEN":
            remote = read(["git", "-C", self.root, "ls-remote", "origin", "refs/heads/main"]).split()
            if not remote or remote[0] != self.data["parent"]:
                raise Refusal(f"origin/main is not the export's parent {self.data['parent'][:12]}; "
                              f"prepare a new release")

    def step_push(self):
        run(["git", "-C", self.root, "push", "origin", f"{self.data['export']}:main"], what="push main")

    def step_github_main(self):
        run(["git", "-C", self.root, "branch", "-f", "github-main", self.data["export"]],
            what="branch github-main")

    def step_master_ff(self):
        # The test-count and the roadmap commit sit on top of the cut; without
        # them master's docs/status.en.md would undo the count and ROADMAP.md
        # would lose the release's § Shipped line at the next export.
        head = self.data["release_head"]
        if git(self.root, "rev-parse", "master") == head:
            return
        if git(self.root, "symbolic-ref", "--short", "HEAD") != "master":
            raise Red(f"{self.root} is not on master; fast-forward master to {head[:12]} by hand")
        run(["git", "-C", self.root, "merge", "-q", "--ff-only", head], what="master fast-forward")

    def step_tag(self):
        try:
            run([self.wt() / "scripts/tag_release.sh", "-C", self.root, self.v, self.data["export"]],
                what="tag_release")
        except Red as e:  # review M2: a failed push leaves the tag local
            raise Red(f"{e} (if v{self.v} now exists locally, delete it before --reason)") from e
        return {"tag": git(self.root, "rev-parse", f"v{self.v}^{{commit}}")}

    def watch(self, workflow, accept=None):
        repo = self.need("gh_repo")
        ci = self.cfg.get("ci") or {}
        tries, pause = int(ci.get("find_tries", 30)), float(ci.get("find_interval_s", 10))
        run_id = None
        for i in range(max(1, tries)):
            out = read(["gh", "run", "list", "-R", repo, "--workflow", workflow,
                        "--commit", self.data["export"], "--json", "databaseId", "-L", "5"])
            runs = json.loads(out or "[]")
            if runs:
                run_id = str(runs[0]["databaseId"])
                break
            if i + 1 < tries:
                time.sleep(pause)
        if run_id is None:
            raise Red(f"no `{workflow}` run on {self.data['export'][:12]} (a tag push with a GitHub 500 "
                      f"starts none: delete and push the same tag object again)")
        # never through a pipe: the exit status is the watch's own
        run(["gh", "run", "watch", run_id, "-R", repo, "--exit-status"], check_rc=False,
            timeout=int(ci.get("watch_timeout_s", 3600)), what=f"gh run watch {workflow}")
        verdict = json.loads(read(["gh", "run", "view", run_id, "-R", repo, "--json", "conclusion"]))
        conclusion = verdict.get("conclusion") or "none"
        if conclusion != "success":
            if accept:
                # GH #1592 (nr. 46): the owner's acceptance, with its ruling
                ruling, why = accept
                self.state.add(step="ci", event="accepted", run=run_id, conclusion=conclusion,
                               reason=why)
                return run_id, f"failure-accepted-{ruling}"
            raise Red(f"{workflow} run {run_id}: {conclusion}")
        return run_id, conclusion

    def step_ci(self):
        run_id, verdict = self.watch((self.cfg.get("ci") or {}).get("workflow", "ci"),
                                     accept=self.a.accepted.get("ci"))
        return {"ci": verdict, "ci_run": run_id}

    def step_release_run(self):
        run_id, verdict = self.watch((self.cfg.get("ci") or {}).get("release_workflow", "release"))
        return {"release": verdict, "release_run": run_id}

    def step_release_notes(self):
        run(["gh", "release", "edit", f"v{self.v}", "-R", self.need("gh_repo"),
             "--notes-file", self.data["notes"]], what="gh release edit")

    def step_discussion(self):
        path = Path(self.a.config)
        disc = self.cfg.get("discussion") or {}
        if not path.exists() or not disc.get("repository_id") or not disc.get("category_id"):
            raise Red(f"discussion: {path} carries no [discussion] repository_id and category_id")
        repo = self.need("gh_repo")
        title = self.a.title or f"meclaw {self.v}"
        body = (f"meclaw {self.v} is out: https://github.com/{repo}/releases/tag/v{self.v}\n\n"
                f"{self.data.get('intro', '')}\n\n"
                f"The changelog has every detail: https://github.com/{repo}/blob/v{self.v}/CHANGELOG.md\n")
        # Look before posting: a Ctrl-C mid-step leaves `start` without `end`, and
        # the rerun ran the step again and posted a second discussion (review
        # e0f6f1ecf, minor). The look-up does not lean on --reason; a failed or
        # unreadable look-up is red and posts nothing (fail-closed).
        lookup = ("query($r:ID!,$c:ID!){node(id:$r){... on Repository{discussions(first:100,"
                  "categoryId:$c,orderBy:{field:CREATED_AT,direction:DESC}){nodes{title url}}}}}")
        out = read(["gh", "api", "graphql", "-f", f"query={lookup}", "-f", f"r={disc['repository_id']}",
                    "-f", f"c={disc['category_id']}"])
        try:
            nodes = json.loads(out)["data"]["node"]["discussions"]["nodes"]
            there = [n["url"] for n in nodes if n["title"] == title]
        except (ValueError, KeyError, TypeError) as e:
            raise Red(f"discussion: look-up answer unreadable, nothing posted: {out[:200]}") from e
        if there:
            print(f"discussion: `{title}` is there already: {there[0]}", flush=True)
            return {"discussion": there[0]}
        query = ("mutation($r:ID!,$c:ID!,$t:String!,$b:String!){createDiscussion(input:"
                 "{repositoryId:$r,categoryId:$c,title:$t,body:$b}){discussion{url}}}")
        out = read(["gh", "api", "graphql", "-f", f"query={query}", "-f", f"r={disc['repository_id']}",
                    "-f", f"c={disc['category_id']}", "-f", f"t={title}", "-f", f"b={body}"])
        try:
            url = json.loads(out)["data"]["createDiscussion"]["discussion"]["url"]
        except (ValueError, KeyError, TypeError) as e:
            raise Red(f"discussion: unexpected answer (a rerun looks before it posts): "
                      f"{out[:200]}") from e
        return {"discussion": url}

    def step_website(self):
        site, files = self.site_files()
        url = self.need("website", "url")
        changed = []
        for rel in files:
            path = site / rel
            old = path.read_text()
            new = rewrite_site_text(rel, old, self.v, self.data["tests"], self.data["date"], url)
            if rel.endswith(".txt") and "Current version:" in new \
                    and f"\nCurrent version: {self.v}." not in "\n" + new:
                raise Red(f"website: {rel} has no `Current version: {self.v}.` paragraph")
            if new != old:
                path.write_text(new)
                changed.append(rel)
        if changed:
            run(["git", "-C", site, "add", "--", *changed], what="website add")
            msg = (f"v{self.v}: Pill/Release-/Changelog-Links, softwareVersion, Testzahl "
                   f"{self.data['tests']} (Release-Gate), sitemap / {self.data['date']}\n")
            with tempfile.NamedTemporaryFile("w", suffix=".msg", delete=False) as f:
                f.write(msg)
            try:
                run(["git", "-C", site, "commit", "-q", "-F", f.name], what="website commit")
            finally:
                os.unlink(f.name)
        return {"site_files": files, "site_commit": git(site, "rev-parse", "HEAD")}

    def step_deploy(self):
        site = Path(self.need("website", "repo"))
        host, root = self.need("website", "ssh"), self.need("website", "root")
        backup = self.need("website", "backup_dir")
        stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
        name = f"{Path(root).name}-rel{self.v.replace('.', '')}-{stamp}"
        # The website host is no lane: its one ssh and scp per file stand outside the
        # kit's narrow point, as publish_docs.sh's rsync to the same host does.
        run(["ssh", host, f"mkdir -p {shlex.quote(backup)} && test -f {shlex.quote(root)}/index.html "  # lane-engstelle-ausnahme: the website host, no lane
             f"&& cp -a {shlex.quote(root)} {shlex.quote(backup)}/{shlex.quote(name)}"], what="backup")
        for rel in self.data["site_files"]:
            run(["scp", "-q", site / rel, f"{host}:{root}/{rel}"], what=f"scp {rel}")  # lane-engstelle-ausnahme: the website host, no lane
        return {"site_backup": f"{backup}/{name}"}

    def step_docs(self):
        site = Path(self.need("website", "repo"))
        target = f"{self.need('website', 'ssh')}:{self.need('website', 'root')}"
        run([self.wt() / "scripts/publish_docs.sh", "--source", "github-main", "--target", target,
             "--sitemap", site / "sitemap.xml"], what="publish_docs")

    def step_live_check(self):
        url = self.need("website", "url")
        bad = []
        for rel in self.data["site_files"]:
            # -L: nginx of the website answers `blog/` with 301 to `blog`; without
            # it curl read the redirect body, exit 0 (a 3xx is no -f error), and
            # 0.78.1 went red on four pages (GH #1396). Following beats dropping
            # the slash: it holds whichever form the server calls final.
            _, page = run(["curl", "-fsSL", page_url(url, rel)], what=f"curl {rel}")
            if rel.endswith(".html") and f"releases/tag/v{self.v}" not in page:
                bad.append(f"{rel}: no v{self.v}")
            if rel.endswith(".txt") and f"Current version: {self.v}." in page \
                    and f"passed {self.data['tests']} tests" not in page:
                bad.append(f"{rel}: test count is not {self.data['tests']}")
            if rel.endswith("sitemap.xml") \
                    and sitemap_lastmod(page, url.rstrip("/") + "/") != self.data["date"]:
                bad.append(f"{rel}: lastmod is not {self.data['date']}")
        if bad:
            raise Red("live check: " + "; ".join(bad))

    def step_website_push(self):
        site = Path(self.need("website", "repo"))
        branch = (self.cfg.get("website") or {}).get("branch", "master")
        run(["git", "-C", site, "push", "-q", "origin", branch], what="website push")
        return {"website": "ok"}

    # ---- ends -------------------------------------------------------------
    def step_asset(self):
        return {"asset": fetch_asset(self.v, self.root, self.cfg)["sha256"]}

    def request(self):
        d = self.data
        print(f"RELEASE-REQUEST {self.v} wave={self.a.wave}")
        print(f"master={d['master'][:12]} release-head={d['release_head'][:12]} "
              f"export={d['export'][:12]} parent={d['parent'][:12]}")
        print(f"{d['gate']} tests={d['tests']} lane={d.get('lane', '-')}")
        print(f"next: scripts/release.sh publish {self.v} --wave {self.a.wave}")

    def summary(self):
        d = self.data
        tag = (d.get("tag") or "-")[:12]
        print(f"RELEASE {self.v} tag={tag} ci={d.get('ci', '-')} release={d.get('release', '-')} "
              f"website={d.get('website', '-')}")


def parse(argv):
    ap = argparse.ArgumentParser(prog="release.sh", description=__doc__.split("\n\n")[0])
    ap.add_argument("phase", choices=("prepare", "publish", "asset"))
    ap.add_argument("version")
    ap.add_argument("--wave", help="the wave (prepare, publish)")
    ap.add_argument("--strand", help="name of the release gate (prepare)")
    ap.add_argument("--host", help="the lane of the release gate (prepare; default: the reachable "
                                   "build lane with the most vCPU)")
    ap.add_argument("--issue", help="issue number for the test-count and the roadmap commit")
    ap.add_argument("--title", help="discussion title (default: meclaw <x.y.z>)")
    ap.add_argument("--repo", help="the main tree (default: the main tree of this script's repo)")
    ap.add_argument("--config", default=str(DEFAULT_CONFIG))
    ap.add_argument("--reason", help="why a red step runs again")
    ap.add_argument("--dry-run", action="store_true", help="name the steps, run only the checks")
    ap.add_argument("--accept-red", action="append", default=[],
                    help='accept a red step (publish; only ci): ci:"OR-<id>: <reason>"')
    args = ap.parse_args(argv)
    args.accepted = {}
    for given in args.accept_red:
        m = ACCEPT_RE.match(given)
        if not m:
            ap.error(f'--accept-red wants <step>:"OR-<id>: <reason>" (the ruling id is mandatory), '
                     f"got: {given}")
        if m["step"] not in ACCEPTABLE:
            ap.error(f"--accept-red names {m['step']}, which takes no acceptance (only: "
                     f"{' '.join(ACCEPTABLE)}; the release run builds the assets)")
        args.accepted[m["step"]] = (m["ruling"], m["why"].strip())
    if not VERSION_RE.match(args.version):
        ap.error(f"{args.version} is not x.y.z")
    if args.phase != "asset" and not args.wave:
        ap.error(f"{args.phase} needs --wave <wave>")
    if args.phase == "prepare" and not args.strand:
        ap.error("prepare needs --strand <relgate name>")
    if not args.repo:
        common = read(["git", "-C", Path(__file__).resolve().parent, "rev-parse",
                       "--path-format=absolute", "--git-common-dir"])
        args.repo = str(Path(common).parent)
    return args


def main(argv=None):
    try:
        args = parse(argv if argv is not None else sys.argv[1:])
        cfg = {}
        if Path(args.config).exists():
            with open(args.config, "rb") as f:
                cfg = tomllib.load(f)
        if args.phase == "asset":
            if args.dry_run:
                print(f"DRY asset -- would download v{args.version}")
                return 0
            fetch_asset(args.version, Path(args.repo).resolve(), cfg)
            return 0
        rel = Release(args, cfg)
        if args.phase == "prepare":
            rc = rel.steps(PREPARE)
            if rc == 0 and not args.dry_run:
                rel.request()
            return rc
        rc = rel.steps(PUBLISH)
        if rc == 0 and not args.dry_run:
            rel.summary()
        return rc
    except Refusal as e:
        print(f"release: REFUSED -- {e}", file=sys.stderr)
        return 2
    except Red as e:
        print(f"release: RED -- {e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
