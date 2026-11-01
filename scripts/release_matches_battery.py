#!/usr/bin/env python3
"""Refuse a release commit that differs from the measured battery candidate in
anything but version and CHANGELOG places (wave live-2, OR-LV-12, GH #1358).

Why: the battery of real model tests runs against the frozen master candidate
before the release. A release commit that changes anything else ships code
nobody measured (retro 2026-10-29, item 2). This script is the lock the release
step runs before it tags.

Receipt form (Markdown, path as the first argument). The script reads three
kinds of line; each may stand as a plain line or as a list item, and the value
may be wrapped in backticks:

    core_commit: <sha>                       the measured meclaw-core commit
    lab_binary_sha256: <hex>                 sha256 of the lab binary
    manifest_sha256: <hex> <rendered path>   one per rendered manifest

A receipt is append-only: round 2 of a battery appends a new candidate below
round 1. The LAST `core_commit` line is the candidate, and the hash lines after
it are its own. The hashes are only read and printed for the release receipt;
this script never computes them (the lab binary and the rendered set are not in
this tree).

Check: `git diff <core_commit> <release>` (release defaults to HEAD), file by
file, renames split into delete + add. A file passes when it is one of the
places the release cut writes and the change stays inside what that place may
change:

- `CHANGELOG.md`, `ROADMAP.md`, `docs/status.en.md`, anything under `plans/`:
  any change. These are the release-cut records the last release merges wrote
  besides the bump (0.76.0, 0.76.4, 0.76.5, 0.76.6: CHANGELOG, ROADMAP, the
  status test count, the release strand's report and strand file); none of them
  reaches the binary or a rendered manifest.
- `Cargo.toml` and `crates/*/Cargo.toml`: only the `version = "x.y.z"` line of
  `[package]`/`[workspace.package]` and the version of a `meclaw-*` path
  dependency (exactly what `scripts/bump_version.py` writes).
- `Cargo.lock`: only the versions of `meclaw*` packages (the lock refresh of
  the same script); a third-party version is a dependency change, not a bump.
- `templates/**/template.json`: only the top-level `"version"` (the number the
  merge point assigns, docs/development-rules.md § 4).

Everything else, an added or deleted file outside the record places, and a
file-mode change are foreign: one line `foreign: <path> (<reason>)` each.

Exit: 0 clean, 1 foreign files, 2 unusable receipt or unknown commit.

Usage: release_matches_battery.py <receipt.md> [--release <rev>] [--repo <path>]
"""
import argparse
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

FIELD = re.compile(
    r"^\s*(?:[-*]\s+)?`?(core_commit|lab_binary_sha256|manifest_sha256)`?\s*:\s*(.+?)\s*$")
SHA = re.compile(r"[0-9a-f]{7,40}")

# Release-cut records: any change (see the docstring for the evidence).
RECORD_FILES = {"CHANGELOG.md", "ROADMAP.md", "docs/status.en.md"}
RECORD_PREFIXES = ("plans/",)

SEMVER = r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?"
PACKAGE_VERSION = re.compile(r'^version = "%s"$' % SEMVER)
MECLAW_PATH_DEP = re.compile(
    r'^(meclaw-[a-z-]+ = \{ path = "\.\./meclaw-[a-z-]+", version = ")%s(".*)$' % SEMVER)
LOCK_DEP_REF = re.compile(r'"(meclaw[a-z-]*) %s' % SEMVER)


class ReceiptError(Exception):
    pass


def read_receipt(path):
    """(core_commit, [pass-through lines]) of the last candidate in `path`."""
    try:
        lines = pathlib.Path(path).read_text().splitlines()
    except OSError as exc:
        raise ReceiptError("receipt not readable: %s" % exc)
    core, fields = None, []
    for line in lines:
        m = FIELD.match(line)
        if not m:
            continue
        key, value = m.group(1), m.group(2).replace("`", "").strip()
        if key == "core_commit":
            if not SHA.fullmatch(value):
                raise ReceiptError("core_commit is not a commit sha: %r" % value)
            core, fields = value, []
        elif core is not None:
            fields.append("%s: %s" % (key, value))
    if core is None:
        raise ReceiptError("receipt carries no core_commit line: %s" % path)
    return core, fields


def git(repo, *args, check=True):
    res = subprocess.run(["git", "-C", str(repo), *args], capture_output=True)
    if check and res.returncode != 0:
        raise ReceiptError("git %s: %s" % (" ".join(args), res.stderr.decode().strip()))
    return res


def commit_of(repo, rev):
    res = git(repo, "rev-parse", "--verify", "--quiet", rev + "^{commit}", check=False)
    if res.returncode != 0:
        raise ReceiptError("unknown commit: %s" % rev)
    return res.stdout.decode().strip()


def blob(repo, rev, path):
    return git(repo, "show", "%s:%s" % (rev, path)).stdout.decode("utf-8", "replace")


def norm_cargo_toml(text):
    """The manifest with the bump's own places blanked."""
    out, section = [], None
    for line in text.splitlines():
        s = line.strip()
        if s.startswith("[") and s.endswith("]"):
            section = s.strip("[]").strip()
        if section in ("package", "workspace.package") and PACKAGE_VERSION.match(s):
            out.append('version = "<bump>"')
            continue
        m = MECLAW_PATH_DEP.match(line)
        out.append(m.group(1) + "<bump>" + m.group(2) if m else line)
    return out


def norm_cargo_lock(text):
    """The lock with the versions of meclaw* packages blanked."""
    out, name = [], None
    for line in text.splitlines():
        if line == "[[package]]":
            name = None
        elif line.startswith("name = "):
            name = line[len("name = "):].strip('"')
        elif name and name.startswith("meclaw") and PACKAGE_VERSION.match(line):
            out.append('version = "<bump>"')
            continue
        out.append(LOCK_DEP_REF.sub(r'"\1 <bump>', line))
    return out


def norm_template_json(text):
    try:
        doc = json.loads(text)
    except ValueError:
        return text
    if isinstance(doc, dict):
        doc["version"] = "<bump>"
    return doc


def is_record(path):
    return path in RECORD_FILES or path.startswith(RECORD_PREFIXES)


def normaliser(path):
    if path == "Cargo.toml" or re.fullmatch(r"crates/[^/]+/Cargo\.toml", path):
        return norm_cargo_toml
    if path == "Cargo.lock":
        return norm_cargo_lock
    if path.startswith("templates/") and path.endswith("/template.json"):
        return norm_template_json
    return None


def changes(repo, core, release):
    """(status, old mode, new mode, path) per changed file, renames split."""
    raw = git(repo, "diff", "--raw", "--no-renames", "--no-abbrev", "-z", core, release).stdout
    parts = raw.decode("utf-8", "surrogateescape").split("\0")
    out = []
    for i in range(0, len(parts) - 1, 2):
        meta, path = parts[i], parts[i + 1]
        if not meta.startswith(":"):
            break
        old_mode, new_mode, _, _, status = meta[1:].split(" ")
        out.append((status[0], old_mode, new_mode, path))
    return out


def foreign_reason(repo, core, release, status, old_mode, new_mode, path):
    """None when the change is one the release cut may make, else the reason."""
    if is_record(path):
        return None
    if status in ("A", "D"):
        return "added" if status == "A" else "deleted"
    if old_mode != new_mode:
        return "mode %s -> %s" % (old_mode, new_mode)
    norm = normaliser(path)
    if norm is None:
        return "not a version or CHANGELOG file"
    if norm(blob(repo, core, path)) != norm(blob(repo, release, path)):
        return "changes more than the version"
    return None


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("receipt", help="battery receipt (Markdown)")
    ap.add_argument("--release", default="HEAD", help="release commit (default HEAD)")
    ap.add_argument("--repo", default=str(ROOT), help="meclaw-core tree (default: this one)")
    args = ap.parse_args(argv)
    try:
        core_ref, fields = read_receipt(args.receipt)
        core = commit_of(args.repo, core_ref)
        release = commit_of(args.repo, args.release)
        found = []
        for status, old_mode, new_mode, path in changes(args.repo, core, release):
            reason = foreign_reason(args.repo, core, release, status, old_mode, new_mode, path)
            if reason:
                found.append((path, reason))
    except ReceiptError as exc:
        print("release_matches_battery: ERROR %s" % exc)
        return 2
    print("core_commit: %s" % core)
    print("release: %s" % release)
    for line in fields:
        print(line)
    for path, reason in found:
        print("foreign: %s (%s)" % (path, reason))
    if found:
        print("release_matches_battery: FOREIGN %d file(s) changed after the battery" % len(found))
        return 1
    print("release_matches_battery: CLEAN")
    return 0


if __name__ == "__main__":
    sys.exit(main())
