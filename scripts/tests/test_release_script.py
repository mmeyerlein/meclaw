"""`scripts/release.py`: one command from the release gate to the website, with
one halt for the release request (wave release-tempo, RT4, GH #1387).

Every case builds a throw-away world in a temporary directory: a private repo
with a release cut on master, a bare `origin` that stands for the public repo,
a website repo with its own bare remote, and stub tools. The repo's own tools
(`strand.sh`, `pipeline.py`, `make_export.py`, `german_scan.py`,
`tag_release.sh`, `publish_docs.sh`) are committed into the throw-away repo as
stubs at their real paths, so the release worktree carries them as the real
one does; `gh`, `ssh`, `scp` and `curl` are stubs in front of `PATH`. Every
stub appends its argv to one log, which is how the order of the steps is read.
No network, no real GitHub, no real host.

Run: `scripts/strand.sh test --unittest scripts.tests.test_release_script`.
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

from scripts.tests import fake_ssh, store_isolation

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(SCRIPTS))
import release  # noqa: E402

SCRIPT = SCRIPTS / "release.py"
VERSION = "0.0.2"
WAVE = "w"
WDIR = "w-2026-11-01"
STRAND = "relgate-002"

STUB = r'''#!/usr/bin/env python3
import json, os, subprocess, sys
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
with open(os.environ["RELEASE_STUB_LOG"], "a") as f:
    f.write(json.dumps([name] + args) + "\n")
fail = os.path.join(os.environ["RELEASE_STUB_FAIL"], name)
if os.path.exists(fail):
    os.remove(fail)
    print(name + ": stub red")
    sys.exit(1)
def git(*a):
    return subprocess.run(["git", *a], check=True, capture_output=True, text=True).stdout.strip()
def opt(flag):
    return args[args.index(flag) + 1]
root = os.environ["RELEASE_STUB_ROOT"]
if name == "strand.sh":
    wt, strand = opt("-C"), opt("--strand")
    rev = git("-C", wt, "rev-parse", "HEAD")
    base = os.path.join(root, "plans", os.environ["RELEASE_STUB_WDIR"], "receipts", strand)
    os.makedirs(os.path.join(base, "run1"), exist_ok=True)
    with open(os.path.join(base, "run1", "receipt.json"), "w") as f:
        json.dump({"mode": "release", "rev": rev, "verdict": "GREEN"}, f)
    if not os.path.lexists(os.path.join(base, "latest")):
        os.symlink("run1", os.path.join(base, "latest"))
    print("GATE-SUMMARY release %s 3/3 9s GREEN" % rev[:7])
elif name == "pipeline.py":
    with open(opt("--receipt")) as f:
        rec = json.load(f)
    if "--write" in args:
        with open(os.path.join(opt("--write"), "docs", "status.en.md"), "w") as f:
            f.write("0.0.2 release gate ran 42 tests.\n")
    print("release-count: 42 tests (release %s)" % rec["rev"][:12])
elif name == "make_export.py":
    tree = git("-C", root, "rev-parse", opt("--rev") + "^{tree}")
    commit = subprocess.run(["git", "-C", root, "commit-tree", tree, "-p", opt("--parent")],
                            input="export\n", capture_output=True, text=True, check=True).stdout.strip()
    print("RECEIPT GRUEN -- Export-Commit: " + commit)
elif name == "tag_release.sh":
    git("-C", args[1], "tag", "-a", "v" + args[2], "-m", "v" + args[2], args[3])
elif name == "gh":
    hang = os.path.join(os.environ["RELEASE_STUB_FAIL"], "gh-hang")
    if args[:2] == ["run", "watch"] and os.path.exists(hang):
        import signal
        signal.pause()  # a stalled watch: no output, no exit, until killed
    if args[:2] == ["run", "list"]:
        print(json.dumps([{"databaseId": 7, "headSha": opt("--commit")}]))
    elif args[:2] == ["run", "view"]:
        print(json.dumps({"conclusion": "success"}))
    elif args[:2] == ["api", "graphql"] and "createDiscussion" not in " ".join(args):
        # the look-up before the create: red, or a discussion already there
        red = os.path.join(os.environ["RELEASE_STUB_FAIL"], "gh-lookup-red")
        if os.path.exists(red):
            print("gh: lookup red", file=sys.stderr)
            sys.exit(1)
        there = os.path.join(os.environ["RELEASE_STUB_FAIL"], "gh-discussion-there")
        nodes = []
        if os.path.exists(there):
            with open(there) as f:
                nodes = [{"title": f.read().strip(), "url": "https://example.invalid/d/0"}]
        nodes.append({"title": "meclaw 0.0.1", "url": "https://example.invalid/d/old"})
        print(json.dumps({"data": {"node": {"discussions": {"nodes": nodes}}}}))
    elif args[:2] == ["api", "graphql"]:
        print(json.dumps({"data": {"createDiscussion": {"discussion": {"url": "https://example.invalid/d/1"}}}}))
elif name == "curl":
    path = args[-1].split("//", 1)[1].partition("/")[2]
    if path == "" or path.endswith("/"):
        path += "index.html"
    with open(os.path.join(os.environ["RELEASE_STUB_SITE"], path)) as f:
        sys.stdout.write(f.read())
'''

# Stubs at the tools' paths inside the THROW-AWAY repo (never the real tools,
# so no path here is one the published tree has to carry).
REPO_TOOLS = ["/".join(p) for p in (
    ("scripts", "strand.sh"), ("scripts", "pipeline.py"), ("scripts", "tag_release.sh"),
    ("scripts", "publish_docs.sh"), ("plans/export-fixtures", "make_export.py"),
    ("plans/p5.5-fixtures", "german_scan.py"))]
# The fake `ssh` comes from its one maker (`fake_ssh.install_inert`, re-check
# M7e): it runs nothing of its arguments and appends `ssh <args>` as a text
# line to the same log, which `World.calls` reads beside the JSON lines.
PATH_TOOLS = ["gh", "scp", "curl"]

PILL = ('<a class="pill" href="https://github.com/o/r/releases/tag/v{v}">v{v}</a>\n'
        '<a href="https://github.com/o/r/blob/v{v}/CHANGELOG.md">Changelog</a>\n')

SITE = {
    "index.html": '<script>{{"softwareVersion": "0.0.1"}}</script>\n' + PILL.format(v="0.0.1"),
    "404.html": PILL.format(v="0.0.1"),
    "blog/index.html": PILL.format(v="0.0.1"),
    "blog/post/index.html": PILL.format(v="0.0.1") + "<p>v0.0.1 means v0.0.1</p>\n",
    "llms.txt": ("## Releases\n\nCurrent version: 0.0.2. A new thing. The release gate passed 7 tests."
                 " Every release before it is in the history.\n\n"
                 "- [Changelog at v0.0.1](https://github.com/o/r/blob/v0.0.1/CHANGELOG.md): truth.\n"),
    "llms-full.txt": ("## Releases\n\nCurrent version: 0.0.2. A new thing. The release gate passed 7 tests.\n\n"
                      "Previous: 0.0.1. Old. The release gate passed 5 tests.\n\n"
                      "Previous: 0.0.0. Older. Changelog: https://github.com/o/r/blob/v0.0.0/CHANGELOG.md\n"),
    # the live sitemap's form: two alternate links between <loc> and <lastmod>
    "sitemap.xml": ('<?xml version="1.0" encoding="UTF-8"?>\n'
                    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"\n'
                    '        xmlns:xhtml="http://www.w3.org/1999/xhtml">\n'
                    "  <url>\n    <loc>https://site.invalid/</loc>\n"
                    '    <xhtml:link rel="alternate" hreflang="en" href="https://site.invalid/"/>\n'
                    '    <xhtml:link rel="alternate" hreflang="x-default" href="https://site.invalid/"/>\n'
                    "    <lastmod>2026-10-01</lastmod>\n    <changefreq>weekly</changefreq>\n"
                    "    <priority>1.0</priority>\n  </url>\n  <url>\n"
                    "    <loc>https://site.invalid/blog</loc>\n"
                    '    <xhtml:link rel="alternate" hreflang="en" href="https://site.invalid/blog"/>\n'
                    "    <lastmod>2026-09-01</lastmod>\n  </url>\n</urlset>\n"),
    "robots.txt": "User-agent: *\n",
}

CHANGELOG = ("# Changelog\n\n## [Unreleased]\n\n## [0.0.2] — 2026-11-01\n\n"
             "A new thing ships.\n\n### Fixed\n\n- a fix ([#1](https://example.invalid/1))\n\n"
             "## [0.0.1] — 2026-10-01\n\n- first\n")


def git(repo, *args, stdin=None):
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True,
                          check=True, input=stdin).stdout.strip()


def write(base, files):
    for rel, text in files.items():
        path = base / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)


def setUpModule():
    store_isolation.isolate()


def tearDownModule():
    store_isolation.restore()


class World:
    """The throw-away repos, stubs, config and environment of one case."""

    def __init__(self, tmp, *, version=VERSION, discussion=True, watch_timeout_s=60, site_files=None):
        self.tmp = tmp
        self.root = tmp / "meclaw-core"
        self.site = tmp / "site"
        self.log = tmp / "stub.log"
        self.fail = tmp / "fail"
        self.fail.mkdir()
        bindir = tmp / "bin"
        bindir.mkdir()
        for name in PATH_TOOLS:
            (bindir / name).write_text(STUB)
            (bindir / name).chmod(0o755)
        self.env = dict(os.environ)
        fake_ssh.install_inert(bindir, "release chain: the website backup is logged, nothing runs",
                               env=self.env, log=self.log, rc=0)
        self.env.update({
            "PATH": f"{bindir}:{self.env['PATH']}",
            "RELEASE_STUB_LOG": str(self.log), "RELEASE_STUB_FAIL": str(self.fail),
            "RELEASE_STUB_ROOT": str(self.root), "RELEASE_STUB_WDIR": WDIR,
            "RELEASE_STUB_SITE": str(self.site),
            "MECLAW_WORKTREE_ROOT": str(tmp / "wellen"),
            "GIT_AUTHOR_NAME": "kit", "GIT_AUTHOR_EMAIL": "kit@example.invalid",
            "GIT_COMMITTER_NAME": "kit", "GIT_COMMITTER_EMAIL": "kit@example.invalid",
            "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null",
        })

        # the public repo and the private one; github-main = origin/main = the last export
        self.origin = tmp / "origin.git"
        subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(self.origin)], check=True)
        subprocess.run(["git", "init", "-q", "-b", "master", str(self.root)], check=True)
        write(self.root, {"README.md": "public\n"})
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "first export")
        git(self.root, "remote", "add", "origin", str(self.origin))
        git(self.root, "push", "-q", "origin", "master:main")
        git(self.root, "branch", "github-main", "master")
        files = {
            "Cargo.toml": f'[workspace]\n\n[workspace.package]\nversion = "{version}"\n',
            "CHANGELOG.md": CHANGELOG,
            "docs/status.en.md": "0.0.2 integration pass ran 40 tests.\n",
            f"plans/{WDIR}/README.md": "wave\n",
        }
        files.update({rel: STUB for rel in REPO_TOOLS})
        write(self.root, files)
        for rel in REPO_TOOLS:
            (self.root / rel).chmod(0o755)
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "Release 0.0.2")

        self.site_origin = tmp / "site.git"
        subprocess.run(["git", "init", "-q", "--bare", "-b", "master", str(self.site_origin)], check=True)
        subprocess.run(["git", "init", "-q", "-b", "master", str(self.site)], check=True)
        write(self.site, SITE)
        git(self.site, "add", "-A")
        git(self.site, "commit", "-q", "-m", "site")
        git(self.site, "remote", "add", "origin", str(self.site_origin))
        git(self.site, "push", "-q", "origin", "master")

        self.config = tmp / "release.toml"
        text = (f'gh_repo = "o/r"\n\n[ci]\nwatch_timeout_s = {watch_timeout_s}\nfind_tries = 1\n'
                'find_interval_s = 0\n\n'
                f'[website]\nrepo = "{self.site}"\nbranch = "master"\nurl = "https://site.invalid"\n'
                'ssh = "web@203.0.113.1"\nroot = "/srv/site"\nbackup_dir = "/srv/backup"\n')
        if site_files is not None:
            text += "files = " + json.dumps(site_files) + "\n"
        if discussion:
            text += '\n[discussion]\nrepository_id = "R_x"\ncategory_id = "DIC_y"\n'
        self.config.write_text(text)

    def run(self, phase, *extra):
        args = [sys.executable, "-I", str(SCRIPT), phase, VERSION, "--wave", WAVE,
                "--repo", str(self.root), "--config", str(self.config)]
        if phase == "prepare":
            args += ["--strand", STRAND]
        proc = subprocess.run(args + list(extra), capture_output=True, text=True, env=self.env)
        return proc.returncode, proc.stdout + proc.stderr

    def calls(self):
        if not self.log.exists():
            return []
        return [json.loads(line) if line.startswith("[") else line.split(" ", 1)
                for line in self.log.read_text().splitlines()]

    def names(self):
        return [c[0] if c[0] != "gh" else "gh " + " ".join(c[1:3]) for c in self.calls()]

    def creates(self):
        return [c for c in self.calls() if c[0] == "gh" and "createDiscussion" in " ".join(c)]

    def lookups(self):
        return [c for c in self.calls() if c[0] == "gh" and c[1:3] == ["api", "graphql"]
                and "createDiscussion" not in " ".join(c)]

    def state(self):
        path = self.root / "plans" / WDIR / f"release-{VERSION}.state"
        return [json.loads(line) for line in path.read_text().splitlines()]


class ReleaseScript(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.tmp = pathlib.Path(self._tmp.name)
        # the test's own git calls commit too; patched for the case only, the
        # gate-selftest process is shared (review M5)
        ident = mock.patch.dict(os.environ, {
            "GIT_AUTHOR_NAME": "kit", "GIT_AUTHOR_EMAIL": "kit@example.invalid",
            "GIT_COMMITTER_NAME": "kit", "GIT_COMMITTER_EMAIL": "kit@example.invalid"})
        ident.start()
        self.addCleanup(ident.stop)

    def tearDown(self):
        self._tmp.cleanup()

    def test_prepare_runs_in_order_and_halts_with_the_request(self):
        w = World(self.tmp)
        rc, out = w.run("prepare")
        self.assertEqual(rc, 0, out)
        self.assertEqual(w.names(), ["german_scan.py", "german_scan.py", "strand.sh",
                                     "pipeline.py", "make_export.py"])
        request = [line for line in out.splitlines() if line.startswith("RELEASE-REQUEST")]
        self.assertEqual(len(request), 1, out)
        tail = out[out.index("RELEASE-REQUEST"):].strip().splitlines()
        self.assertEqual(len(tail), 4, out)
        self.assertIn("publish 0.0.2 --wave w", tail[3])
        # the halt: nothing public happened
        self.assertEqual(git(w.origin, "rev-parse", "main"), git(w.root, "rev-parse", "github-main"))
        ends = [e["step"] for e in w.state() if e["event"] == "end" and e["verdict"] == "GREEN"]
        self.assertEqual(ends, list(release.PREPARE))
        # the test count landed as a commit on top of the cut, master untouched
        data = release.State(w.root / "plans" / WDIR / f"release-{VERSION}.state").data()
        wt = pathlib.Path(data["worktree"])
        self.assertEqual(git(wt, "rev-parse", "HEAD~1"), git(w.root, "rev-parse", "master"))
        self.assertIn("42 tests", (wt / "docs/status.en.md").read_text())
        notes = pathlib.Path(data["notes"]).read_text()
        self.assertTrue(notes.startswith("A new thing ships."), notes)
        self.assertNotIn("0.0.1", notes)

    def test_prepare_refuses_a_master_without_the_cut(self):
        w = World(self.tmp, version="0.0.1")
        rc, out = w.run("prepare")
        self.assertEqual(rc, 2, out)
        self.assertIn("0.0.1", out)
        self.assertEqual(w.calls(), [])

    def test_a_red_step_stops_and_resumes_there_with_a_reason(self):
        w = World(self.tmp)
        (w.fail / "make_export.py").write_text("")
        rc, out = w.run("prepare")
        self.assertEqual(rc, 1, out)
        self.assertIn("export", out)
        rc, out = w.run("prepare")
        self.assertEqual(rc, 2, out)
        self.assertIn("--reason", out)
        rc, out = w.run("prepare", "--reason", "stub fixed")
        self.assertEqual(rc, 0, out)
        names = w.names()
        self.assertEqual(names.count("strand.sh"), 1, names)
        self.assertEqual(names.count("pipeline.py"), 1, names)
        self.assertEqual(names.count("make_export.py"), 2, names)
        self.assertIn({"step": "export", "event": "retry", "reason": "stub fixed"},
                      [{k: e[k] for k in ("step", "event", "reason")} for e in w.state()
                       if e["event"] == "retry"])

    def test_publish_runs_the_chain_and_rewrites_every_pill(self):
        w = World(self.tmp)
        self.assertEqual(w.run("prepare")[0], 0)
        before = len(w.calls())
        rc, out = w.run("publish")
        self.assertEqual(rc, 0, out)
        names = w.names()[before:]
        self.assertEqual(names[:9], ["tag_release.sh", "gh run list", "gh run watch", "gh run view",
                                     "gh run list", "gh run watch", "gh run view",
                                     "gh release edit", "gh api graphql"], names)
        self.assertEqual(names[9], "gh api graphql", names)  # look-up, then the one create
        self.assertEqual(len(w.creates()), 1, names)
        self.assertEqual(names[10], "ssh", names)
        self.assertIn("publish_docs.sh", names)
        self.assertEqual(names[-1], "curl", names)
        data = release.State(w.root / "plans" / WDIR / f"release-{VERSION}.state").data()
        export = data["export"]
        self.assertEqual(git(w.origin, "rev-parse", "main"), export)
        self.assertEqual(git(w.root, "rev-parse", "github-main"), export)
        self.assertEqual(git(w.root, "rev-parse", "master"), data["release_head"])
        self.assertRegex(out.strip().splitlines()[-1],
                         r"^RELEASE 0\.0\.2 tag=[0-9a-f]{12} ci=success release=success website=ok$")
        # every pill, release link and changelog link on the new version
        for rel in ("index.html", "404.html", "blog/index.html", "blog/post/index.html"):
            text = (w.site / rel).read_text()
            self.assertEqual(text.count("v0.0.2"), 3, rel)
            self.assertNotIn("releases/tag/v0.0.1", text, rel)
        self.assertIn("v0.0.1 means v0.0.1", (w.site / "blog/post/index.html").read_text())
        self.assertIn('"softwareVersion": "0.0.2"', (w.site / "index.html").read_text())
        llms = (w.site / "llms.txt").read_text()
        self.assertIn("The release gate passed 42 tests.", llms)
        self.assertIn("Changelog at v0.0.2](https://github.com/o/r/blob/v0.0.2/CHANGELOG.md)", llms)
        full = (w.site / "llms-full.txt").read_text()
        self.assertIn("The release gate passed 42 tests.", full)
        self.assertIn("Previous: 0.0.1. Old. The release gate passed 5 tests.", full)
        self.assertIn("Previous: 0.0.0. Older. Changelog: https://github.com/o/r/blob/v0.0.0/CHANGELOG.md",
                      full)
        sitemap = (w.site / "sitemap.xml").read_text()
        self.assertEqual(release.sitemap_lastmod(sitemap, "https://site.invalid/"), "2026-11-01", sitemap)
        self.assertIn("<lastmod>2026-09-01</lastmod>", sitemap)
        self.assertNotIn("2026-10-01", sitemap)
        self.assertEqual(git(w.site_origin, "rev-parse", "master"), git(w.site, "rev-parse", "HEAD"))
        scp = [c for c in w.calls() if c[0] == "scp"]
        shipped = {c[-1].split(":", 1)[1] for c in scp}
        self.assertEqual(shipped, {"/srv/site/" + r for r in (
            "index.html", "404.html", "blog/index.html", "blog/post/index.html",
            "llms.txt", "llms-full.txt", "sitemap.xml")})
        # a second publish repeats nothing
        again = len(w.calls())
        rc, out = w.run("publish")
        self.assertEqual(rc, 0, out)
        self.assertEqual(len(w.calls()), again)

    def test_publish_refuses_a_moved_master(self):
        w = World(self.tmp)
        self.assertEqual(w.run("prepare")[0], 0)
        public = git(w.origin, "rev-parse", "main")
        write(w.root, {"x.txt": "late\n"})
        git(w.root, "add", "x.txt")
        git(w.root, "commit", "-q", "-m", "late merge")
        before = len(w.calls())
        rc, out = w.run("publish")
        self.assertEqual(rc, 2, out)
        self.assertIn("master", out)
        self.assertEqual(git(w.origin, "rev-parse", "main"), public)
        self.assertEqual(len(w.calls()), before)

    def test_publish_dry_run_touches_nothing(self):
        w = World(self.tmp)
        self.assertEqual(w.run("prepare")[0], 0)
        public = git(w.origin, "rev-parse", "main")
        before = len(w.calls())
        state_before = len(w.state())
        rc, out = w.run("publish", "--dry-run")
        self.assertEqual(rc, 0, out)
        self.assertEqual(len(w.calls()), before)
        self.assertEqual(len(w.state()), state_before)
        self.assertEqual(git(w.origin, "rev-parse", "main"), public)
        steps = [line.split()[1] for line in out.splitlines() if line.startswith("DRY ")]
        self.assertEqual(steps, list(release.PUBLISH))
        self.assertEqual(git(w.site, "status", "--porcelain"), "")

    def test_discussion_without_config_is_red_and_writes_nothing(self):
        w = World(self.tmp, discussion=False)
        self.assertEqual(w.run("prepare")[0], 0)
        rc, out = w.run("publish")
        self.assertEqual(rc, 1, out)
        self.assertIn("discussion", out)
        self.assertNotIn("gh api graphql", w.names())
        self.assertNotIn("ssh", w.names())

    def test_a_discussion_already_there_is_reported_not_created_again(self):
        # Ctrl-C mid-step left `start` without `end`; the rerun must not post twice
        w = World(self.tmp)
        self.assertEqual(w.run("prepare")[0], 0)
        (w.fail / "gh-discussion-there").write_text("meclaw 0.0.2")
        rc, out = w.run("publish")
        self.assertEqual(rc, 0, out)
        self.assertEqual(len(w.lookups()), 1, w.names())
        self.assertEqual(w.creates(), [], w.names())
        self.assertIn("https://example.invalid/d/0", out)
        end = [e for e in w.state() if e["step"] == "discussion" and e["event"] == "end"]
        self.assertEqual([e["verdict"] for e in end], ["GREEN"], out)
        self.assertEqual(end[0]["data"]["discussion"], "https://example.invalid/d/0")

    def test_a_failed_lookup_is_red_and_creates_nothing(self):
        w = World(self.tmp)
        self.assertEqual(w.run("prepare")[0], 0)
        (w.fail / "gh-lookup-red").write_text("")
        rc, out = w.run("publish")
        self.assertEqual(rc, 1, out)
        self.assertEqual(len(w.lookups()), 1, w.names())
        self.assertEqual(w.creates(), [], w.names())
        end = [e for e in w.state() if e["step"] == "discussion" and e["event"] == "end"]
        self.assertEqual([e["verdict"] for e in end], ["RED"], out)
        self.assertNotIn("ssh", w.names())

    def test_a_silent_watch_meets_the_hang_mark(self):
        # a stalled `gh run watch` prints nothing; the mark must still end it
        w = World(self.tmp, watch_timeout_s=1)
        self.assertEqual(w.run("prepare")[0], 0)
        (w.fail / "gh-hang").write_text("")
        rc, out = w.run("publish")
        self.assertEqual(rc, 1, out)
        end = [e for e in w.state() if e["step"] == "ci" and e["event"] == "end"]
        self.assertEqual([e["verdict"] for e in end], ["RED"], out)
        self.assertIn("HANG", end[0]["reason"])
        self.assertNotIn("gh run view", w.names())

    def test_an_unexpected_error_ends_the_step_red(self):
        w = World(self.tmp, site_files=["llms.txt", "gone.txt"])
        rc, out = w.run("prepare")
        self.assertEqual(rc, 1, out)
        last = w.state()[-1]
        self.assertEqual((last["step"], last["event"], last["verdict"]), ("website-check", "end", "RED"), out)
        self.assertIn("gone.txt", last["reason"])
        self.assertNotIn("Traceback", out)


class Rewrite(unittest.TestCase):
    def test_every_pill_place_takes_the_new_version(self):
        text = SITE["index.html"] + SITE["blog/post/index.html"]
        out = release.rewrite_site_text("index.html", text, "0.3.0", 9, "2026-11-02", "https://s")
        self.assertNotIn("0.0.1/", out)
        self.assertNotIn(">v0.0.1<", out)
        self.assertEqual(out.count("releases/tag/v0.3.0"), 2)
        self.assertEqual(out.count("blob/v0.3.0/CHANGELOG.md"), 2)
        self.assertIn("v0.0.1 means v0.0.1", out)

    def test_historic_release_links_stay_byte_equal(self):
        text = ("Current version: 0.0.2. New.\n\n"
                "Previous: 0.0.1. Old: https://github.com/o/r/blob/v0.0.1/CHANGELOG.md and "
                "https://github.com/o/r/releases/tag/v0.0.1 and "
                "https://github.com/o/r/releases/download/v0.0.1/x.tgz\n")
        self.assertEqual(release.rewrite_site_text("llms-full.txt", text, "0.0.2", None, None, "https://s"),
                         text)
        html = '<p>See <a href="https://github.com/o/r/releases/tag/v0.0.1">the old one</a></p>\n'
        self.assertEqual(release.rewrite_site_text("blog/post/index.html", html, "0.0.2", None, None,
                                                   "https://s"), html)

    def test_sitemap_date_lands_in_the_root_block(self):
        out = release.rewrite_site_text("sitemap.xml", SITE["sitemap.xml"], "0.0.2", None, "2026-11-02",
                                        "https://site.invalid")
        self.assertEqual(release.sitemap_lastmod(out, "https://site.invalid/"), "2026-11-02")
        self.assertEqual(release.sitemap_lastmod(out, "https://site.invalid/blog"), "2026-09-01")


class Read(unittest.TestCase):
    def test_a_missing_tool_is_red_not_a_traceback(self):
        with self.assertRaisesRegex(release.Red, "cannot start"):
            release.read(["/nonexistent/release-tool"])


if __name__ == "__main__":
    unittest.main()
