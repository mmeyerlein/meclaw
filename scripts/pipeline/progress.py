"""meclaw -- progress lines, loud refusals and the liveness of the kit's long runs (GH #1199).

Kit find V2-A8 (welle-validierung-2, 14.10.): a builder ran 17:45-21:40 without
a result. `flake_triage.py`, started without `-I`, refused at the lane's narrow
point with one stderr line that the background wrapper never kept (its log
stayed empty, exit 2), and a running series printed nothing until its end
(stdout is block-buffered when it goes to a file). `token who` showed a held
lane for four hours. Three rules, one place:

* `pass_line` + `emit`: one line per pass, `HH:MM:SS pass i/n done=k red=r`,
  flushed at once into every distinct output stream (stderr AND stdout when
  they are different files -- a wrapper that keeps only one of them still
  has the line). The gate keeps its own `GATE <station> ...` line per station.
* `refuse`: a refusal before (or instead of) the run is exit != 0 and one
  `<entry>: REFUSED -- <why>` line on every distinct stream and in the
  caller's result file, and the holder's lease notes it.
* `beat`: the last progress line (or the refusal) at the holder's lease, in
  `lease_progress` keyed by (pool, holder, since) like `lease_holds` -- a new
  lease starts without progress. `token who` reads it per holder on demand
  (`schedule._liveness`); nothing polls, no daemon.

A beat never ends a run: no store file, no lease or a busy store is `False`.
"""
import io
import os
import pathlib
import re
import sqlite3
import time

from . import schedule, store

TABLE = "lease_progress"

# A line a relay passes on at once (re-check v2-fix-e I1): a pass line or a
# refusal (`<entry>: REFUSED ...`, `<entry>: NO PASS ...`) of a child run.
RELAY = re.compile(r"^(\d\d:\d\d:\d\d pass \d+/\d+ done=\d+ red=\d+|[\w .-]+: (REFUSED|NO PASS)\b.*)$")


def pass_line(i, n, done, red, now=None):
    """`HH:MM:SS pass i/n done=k red=r` -- the kit clock `now` (epoch s) or the wall clock."""
    t = time.time() if now is None else now
    return "%s pass %d/%d done=%d red=%d" % (time.strftime("%H:%M:%S", time.localtime(t)),
                                            i, n, done, red)


def _key(stream):
    try:
        st = os.fstat(stream.fileno())
        return ("file", st.st_dev, st.st_ino)
    except (AttributeError, OSError, ValueError, io.UnsupportedOperation):
        return ("obj", id(stream))


def distinct(*streams):
    """The streams that are different files (stdout and stderr redirected into
    one log are one stream): a line written to each lands once per file."""
    seen, out = set(), []
    for s in streams:
        if s is None:
            continue
        k = _key(s)
        if k not in seen:
            seen.add(k)
            out.append(s)
    return out


def emit(text, *streams):
    """`text` as one flushed line on every distinct stream."""
    for s in distinct(*streams):
        print(text, file=s, flush=True)


def _write(db, holder, text, state, now):
    with store.tx(db):
        row = db.execute("SELECT since FROM lease_entries WHERE pool = ? AND holder = ? "
                         "AND resource IS NOT NULL", (schedule.POOL, holder)).fetchone()
        if row is None:
            return False
        db.execute("INSERT OR REPLACE INTO %s (pool, holder, since, at, state, line) "
                   "VALUES (?, ?, ?, ?, ?, ?)" % TABLE,
                   (schedule.POOL, holder, row["since"], schedule._iso(now), state, text))
    return True


def beat(holder, text, state="progress", db=None, now=None, path=None):
    """Note `text` at the current lease of `holder` (state `progress` or a
    refusal word); True when a lease took it. Without a store file nothing is
    created -- no store, no lease."""
    if not holder:
        return False
    own = db is None
    if own:
        p = pathlib.Path(path) if path else store.default_path()
        if not p.exists():
            return False
        try:
            db = store.open(p)
        except (sqlite3.Error, OSError, RuntimeError):
            return False
    try:
        schedule._ensure(db)
        return bool(store.retry_write(_write, db, holder, text, state, schedule._now(now)))
    except (sqlite3.Error, store.StoreWriteError):
        return False
    finally:
        if own:
            store.close(db)


def refuse(entry, msg, out=None, err=None, holder=None, also=None, db=None, now=None, code=2,
           word="REFUSED"):
    """The loud refusal: `<entry>: REFUSED -- <msg>` on stderr and stdout (once
    when they are one file), appended to `also` (the caller's result file),
    noted at the lease of `holder`; returns `code` (never 0)."""
    text = "%s: %s -- %s" % (entry, word, msg)
    emit(text, err, out)
    if also:
        try:
            with open(also, "a", encoding="utf-8") as fh:
                fh.write(text + "\n")
        except OSError:
            pass
    beat(holder, text, state=word.split(" ")[0].lower(), db=db, now=now)
    return code or 2


class Tail:
    """A stream that remembers its last non-empty line and whether a line
    started with one of `marks` -- the gate's entry reads both (`gate._cmd_gate`)."""

    def __init__(self, stream, marks=()):
        self._s, self._marks, self.last, self.marked, self._part = stream, tuple(marks), "", False, ""

    def write(self, text):
        self._part += text
        *lines, self._part = self._part.split("\n")
        for ln in lines:
            if ln.strip():
                self.last = ln.strip()
                self.marked = self.marked or (bool(self._marks) and ln.startswith(self._marks))
        return self._s.write(text)

    def __getattr__(self, name):
        return getattr(self._s, name)
