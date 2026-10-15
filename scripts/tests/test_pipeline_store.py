"""Tests for `scripts/pipeline/store.py` -- the SQLite store of the pipeline core.

The store carries Orga's names (items, edges, counters, task_runs,
task_run_results, calibration, journal) so a pass can move to Orga later
without a translation table. These tests lock the column lists and the state
machines word for word against the contract (V0 section 2), so a renamed
column fails here and not in a strand that builds against it.

Every test works on a store in a TempDir -- never on the real one.
"""

import contextlib
import io
import os
import pathlib
import sqlite3
import sys
import tempfile
import unittest
from unittest import mock
from scripts.tests import store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO))

from scripts.pipeline import cli, store  # noqa: E402

AT = "2026-10-08T12:00:00Z"

# V0 section 2, word for word (Orga columns plus the additive ones).
V0_COLUMNS = {
    "items": ["id", "key", "type", "title", "state", "reason", "parent", "doc",
              "version", "created_at", "updated_at"],
    "edges": ["src", "dst", "kind", "at", "actor", "data"],
    "counters": ["name", "value"],
    "task_runs": ["id", "item", "key", "runner", "attempt", "ws", "state",
                  "started_at", "deadline_at", "ended_at", "reason", "evidence",
                  "results", "lost", "round", "verdict", "build_s", "run_s",
                  "cache_from", "fingerprint"],
    "task_run_results": ["run", "at", "call", "tool_name", "entry"],
    "runners": ["name", "party", "capacity", "host", "transport", "class", "state",
                "boot_s", "fingerprint", "updated_at"],
    "calibration": ["item", "type", "executing", "estimate_min", "actual_min",
                    "actual_s", "closed_at", "key", "estimate_s"],
    "journal": ["seq", "at", "item", "run", "kind", "event", "text", "data"],
    "cache": ["key", "station", "part", "verdict", "run", "runner", "at", "inputs"],
    "flakes": ["test", "station", "repo", "first_at", "last_at", "runs", "reds",
               "flaky", "quarantined_until", "issue", "evidence", "owner", "cause", "fixed"],
    "workspaces": ["id", "runner", "kind", "path", "state", "run", "bytes",
                   "created_at", "used_at", "closed_at", "collected_at", "ttl_s"],
    "leases": ["resource", "holder", "run", "taken_at", "ttl_s", "offered_to"],
    "merges": ["id", "repo", "strand", "base", "merge_sha", "state", "versions",
               "pins", "run", "at"],
    "minimum": ["stage", "runner", "repo", "seconds", "critical_path",
                "overhead_s", "x", "measured_at", "source"],
    "schema_version": ["version", "applied_at"],
    "migration_hash": ["n", "sha256", "recorded_at"],
}


# gate-selftest loads every module into one process: no fixture event of this
# module may reach the metrics target of the caller's environment.
_METRICS_URL = mock.patch.dict(os.environ, {"MECLAW_PIPELINE_METRICS_URL": ""})


def setUpModule():  # noqa: N802 -- unittest's name
    store_isolation.isolate()
    _METRICS_URL.start()


def tearDownModule():  # noqa: N802 -- unittest's name
    _METRICS_URL.stop()
    store_isolation.restore()


class StoreCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = pathlib.Path(self.tmp.name) / "pipeline.db"
        self.db = store.open(self.path)

    def tearDown(self):
        store.close(self.db)
        self.tmp.cleanup()


class SchemaTest(StoreCase):
    def test_store_schema_has_orga_tables_and_fields(self):
        for table, cols in V0_COLUMNS.items():
            got = [r[1] for r in self.db.execute(f"PRAGMA table_info({table})")]
            self.assertEqual(got, cols, table)
        version = self.db.execute(
            "SELECT version FROM schema_version ORDER BY rowid DESC LIMIT 1").fetchone()[0]
        self.assertEqual(version, store.SCHEMA_VERSION)
        self.assertEqual(store.SCHEMA_VERSION, "meclaw.pipeline.store/4")
        self.assertEqual(self.db.execute("PRAGMA journal_mode").fetchone()[0], "wal")

    def test_open_twice_does_not_reapply_migrations(self):
        store.close(self.db)
        self.db = store.open(self.path)
        n = self.db.execute("SELECT count(*) FROM schema_version").fetchone()[0]
        self.assertEqual(n, len(store.MIGRATIONS))

    def test_a_v1_store_gains_the_flake_evidence_column_and_keeps_its_rows(self):
        """#1135: migration 2 adds `flakes.evidence` to a store made by migration 1."""
        path = self.path.with_name("v1.db")
        db = sqlite3.connect(str(path))
        db.execute("CREATE TABLE schema_version (version TEXT, applied_at TEXT)")
        for sql in store.MIGRATIONS[0][1]:
            db.execute(sql)
        db.execute("INSERT INTO schema_version VALUES (?, ?)", ("meclaw.pipeline.store/1", AT))
        db.execute("INSERT INTO flakes (test, repo, runs) VALUES ('t', 'meclaw', 3)")
        db.commit()
        db.close()
        db = store.open(path)
        self.addCleanup(store.close, db)
        cols = [r[1] for r in db.execute("PRAGMA table_info(flakes)")]
        # Migration 2 adds `evidence`; migration 4 (VG TV-F3) adds the quarantine fields after it.
        self.assertEqual(cols[-4:], ["evidence", "owner", "cause", "fixed"])
        self.assertEqual(dict(db.execute("SELECT test, repo, runs FROM flakes").fetchone()),
                         {"test": "t", "repo": "meclaw", "runs": 3})
        self.assertEqual(store.check(db)["schema"], store.SCHEMA_VERSION)

    def test_newer_schema_is_never_refused(self):
        n = len(store.MIGRATIONS) + 1
        self.db.execute("INSERT INTO schema_version VALUES (?, ?)",
                        ("meclaw.pipeline.store/%d" % n, AT))
        self.db.commit()
        store.close(self.db)
        self.db = store.open(self.path)
        self.assertEqual(store.check(self.db)["schema"], "meclaw.pipeline.store/%d" % n)


class MigrationHashTest(StoreCase):
    """OR-DT.store-migrations: every applied migration carries its number and
    the hash of its statements; drift or a gap is refused loudly."""

    def hashes(self, db=None):
        return {r[0]: r[1] for r in (db or self.db).execute(
            "SELECT n, sha256 FROM migration_hash")}

    def reopen(self):
        store.close(self.db)
        self.db = store.open(self.path)

    def test_open_records_number_and_hash_of_every_migration(self):
        self.assertEqual(self.hashes(), {n: store.migration_hash(stmts)
                                         for n, stmts in store.MIGRATIONS})
        self.reopen()
        self.assertEqual(len(self.hashes()), len(store.MIGRATIONS))

    def test_known_number_with_another_hash_is_refused(self):
        self.db.execute("UPDATE migration_hash SET sha256 = ? WHERE n = 3", ("0" * 64,))
        store.close(self.db)
        with self.assertRaisesRegex(store.MigrationDrift, "migration 3"):
            self.db = store.open(self.path)
        self.db = store.open_ro(self.path)

    def test_a_gap_is_refused_and_nothing_is_skipped(self):
        self.db.execute("DELETE FROM schema_version WHERE version = ?",
                        ("meclaw.pipeline.store/2",))
        store.close(self.db)
        with self.assertRaisesRegex(store.MigrationDrift, "gap"):
            self.db = store.open(self.path)
        self.db = store.open_ro(self.path)
        versions = [r[0] for r in self.db.execute("SELECT version FROM schema_version")]
        self.assertNotIn("meclaw.pipeline.store/2", versions)

    def test_a_store_without_hashes_gets_them_once_without_red(self):
        """The live store applied 1-3 before hashes existed (a copy in a tempdir)."""
        path = self.path.with_name("legacy.db")
        db = sqlite3.connect(str(path))
        db.execute("CREATE TABLE schema_version (version TEXT, applied_at TEXT)")
        for n, stmts in store.MIGRATIONS:
            for sql in stmts:
                db.execute(sql)
            db.execute("INSERT INTO schema_version VALUES (?, ?)",
                       ("meclaw.pipeline.store/%d" % n, AT))
        db.commit()
        db.close()
        db = store.open(path)
        self.addCleanup(store.close, db)
        self.assertEqual(self.hashes(db), {n: store.migration_hash(stmts)
                                           for n, stmts in store.MIGRATIONS})
        self.assertEqual(db.execute("SELECT count(*) FROM schema_version").fetchone()[0],
                         len(store.MIGRATIONS))
        recorded = db.execute("SELECT recorded_at FROM migration_hash WHERE n = 1").fetchone()[0]
        store.close(db)
        db = store.open(path)
        self.assertEqual(db.execute("SELECT recorded_at FROM migration_hash WHERE n = 1")
                         .fetchone()[0], recorded)

    def test_hash_of_a_newer_unknown_migration_is_left_alone(self):
        n = len(store.MIGRATIONS) + 1
        self.db.execute("INSERT INTO schema_version VALUES (?, ?)",
                        ("meclaw.pipeline.store/%d" % n, AT))
        self.db.execute("INSERT INTO migration_hash VALUES (?, ?, ?)", (n, "f" * 64, AT))
        self.reopen()
        self.assertEqual(self.hashes()[n], "f" * 64)


class StateTest(StoreCase):
    def test_task_run_states_are_orga_states(self):
        self.assertEqual(store.TASK_RUN_STATES, ("dispatched", "done", "failed", "timed_out"))
        self.assertEqual(store.TRANSITIONS["workpackage"], {
            "planned": {"active"},
            "active": {"finished", "stopped"},
            "finished": {"active"},
            "stopped": {"active"},
        })
        self.assertEqual(store.TRANSITIONS["task"], {
            "open": {"in_progress", "done", "dropped"},
            "in_progress": {"waiting", "done", "dropped"},
            "waiting": {"in_progress", "done", "dropped"},
            "done": {"open", "dropped"},
            "dropped": {"open"},
        })
        mapping = {v: store.verdict_state(v, reason) for v, reason in [
            ("GREEN", None), ("CACHED", None), ("FLAKY", None), ("NOTE", None),
            ("SKIP", None), ("RED", None), ("ASK", None), ("ABORTED", "aborted_disk")]}
        # FLAKY is a finding (VG TV-F1); only a legacy FLAKY (VG TV-F4) is done.
        self.assertEqual(store.verdict_state("FLAKY", legacy=True), ("done", None))
        self.assertEqual(mapping, {
            "GREEN": ("done", None), "CACHED": ("done", None), "FLAKY": ("failed", "flaky"),
            "NOTE": ("done", None), "SKIP": ("done", None), "RED": ("failed", "red"),
            "ASK": ("failed", "ask"), "ABORTED": ("failed", "aborted_disk")})
        with self.assertRaises(ValueError):
            store.verdict_state("ABORTED", "whatever")
        # A run is opened `dispatched` and closed by its verdict.
        task = store.item_create(self.db, "task", "fmt", at=AT)
        tr = store.run_open(self.db, task["id"], "lane-a", at=AT)
        self.assertTrue(tr.startswith("tr-") and len(tr) == 15, tr)
        row = self.db.execute("SELECT state FROM task_runs WHERE id=?", (tr,)).fetchone()
        self.assertEqual(row[0], "dispatched")
        self.assertEqual(store.run_close(self.db, tr, "RED", at=AT), "failed")
        row = self.db.execute("SELECT state, reason, verdict FROM task_runs WHERE id=?",
                              (tr,)).fetchone()
        self.assertEqual(tuple(row), ("failed", "red", "RED"))
        tr2 = store.run_open(self.db, task["id"], "lane-a", attempt=2, at=AT)
        self.assertEqual(store.run_close(self.db, tr2, "GREEN", timed_out=True, at=AT),
                         "timed_out")
        self.assertEqual(store.TASK_AFTER_RUN, {"done": "done", "failed": "waiting",
                                                "timed_out": "waiting"})

    def test_item_keys_come_from_counters(self):
        p1 = store.item_create(self.db, "workpackage", "strand pass", at=AT)
        t1 = store.item_create(self.db, "task", "fmt", parent=p1["id"], at=AT)
        p2 = store.item_create(self.db, "workpackage", "merge pass", at=AT)
        self.assertEqual((p1["key"], t1["key"], p2["key"]), ("P-1", "T-1", "P-2"))
        self.assertEqual((p1["state"], t1["state"]), ("planned", "open"))
        names = dict(self.db.execute("SELECT name, value FROM counters"))
        self.assertEqual(names, {"key:P": 2, "key:T": 1})

    def test_item_transition_is_compare_and_set(self):
        p = store.item_create(self.db, "workpackage", "strand pass", at=AT)
        self.assertEqual(p["version"], 1)
        v2 = store.item_transition(self.db, p["id"], "planned", "active", 1, at=AT)
        self.assertEqual(v2, 2)
        # A stale version is refused, and so is a stale from-state.
        with self.assertRaises(store.Conflict):
            store.item_transition(self.db, p["id"], "active", "finished", 1, at=AT)
        with self.assertRaises(store.Conflict):
            store.item_transition(self.db, p["id"], "planned", "active", 2, at=AT)
        # A transition the automaton does not know is refused before the CAS.
        with self.assertRaises(ValueError):
            store.item_transition(self.db, p["id"], "active", "planned", 2, at=AT)
        self.assertEqual(store.item_transition(self.db, p["id"], "active", "finished", 2,
                                               reason="green", at=AT), 3)
        row = self.db.execute("SELECT state, reason, version FROM items WHERE id=?",
                              (p["id"],)).fetchone()
        self.assertEqual(tuple(row), ("finished", "green", 3))
        # item_update is CAS on the version as well.
        self.assertEqual(store.item_update(self.db, p["id"], 3, doc={"repo": "meclaw"}, at=AT), 4)
        with self.assertRaises(store.Conflict):
            store.item_update(self.db, p["id"], 3, title="stale", at=AT)

    def test_journal_is_append_only(self):
        seq = store.journal_add(self.db, AT, None, "run-000000000001", "note",
                                "station_log", "log", {"run": "run-000000000001"})
        self.assertEqual(seq, 1)
        with self.assertRaisesRegex(sqlite3.DatabaseError, "journal is append-only"):
            self.db.execute("UPDATE journal SET text='x' WHERE seq=?", (seq,))
        with self.assertRaisesRegex(sqlite3.DatabaseError, "journal is append-only"):
            self.db.execute("DELETE FROM journal WHERE seq=?", (seq,))
        with self.assertRaises(ValueError):
            store.journal_add(self.db, AT, None, None, "chatter", "x", "x", {})


class ResourceTest(StoreCase):
    def test_lease_take_release_who(self):
        self.assertTrue(store.lease_take(self.db, "lane:lane-a", "dt-w0", ttl_s=60, at=AT))
        self.assertFalse(store.lease_take(self.db, "lane:lane-a", "dt-w1", ttl_s=60,
                                          at="2026-10-08T12:00:30Z"))
        self.assertEqual(store.lease_who(self.db, "lane:lane-a")["holder"], "dt-w0")
        # An expired lease can be taken over.
        self.assertTrue(store.lease_take(self.db, "lane:lane-a", "dt-w1", ttl_s=60,
                                         at="2026-10-08T12:02:00Z"))
        self.assertFalse(store.lease_release(self.db, "lane:lane-a", "dt-w0"))
        self.assertTrue(store.lease_release(self.db, "lane:lane-a", "dt-w1"))
        self.assertIsNone(store.lease_who(self.db, "lane:lane-a"))

    def test_cache_holds_green_and_flaky_only(self):
        store.cache_put(self.db, "k1", "fmt", "", "GREEN", "run-1", "lane-a", ["docs"], at=AT)
        self.assertEqual(store.cache_get(self.db, "k1")["verdict"], "GREEN")
        with self.assertRaises(ValueError):
            store.cache_put(self.db, "k2", "fmt", "", "RED", "run-1", "lane-a", [], at=AT)
        self.assertIsNone(store.cache_get(self.db, "k2"))

    def test_workspace_set_and_results(self):
        store.workspace_set(self.db, "ws-1", "lane-a", "ws", "/srv/target/ws/x", "created", at=AT)
        store.workspace_set(self.db, "ws-1", "lane-a", "ws", "/srv/target/ws/x", "in_use",
                            run="run-1", at=AT)
        row = self.db.execute("SELECT state, run, created_at, used_at FROM workspaces").fetchone()
        self.assertEqual(tuple(row), ("in_use", "run-1", AT, AT))
        with self.assertRaises(ValueError):
            store.workspace_set(self.db, "ws-1", "lane-a", "ws", "/x", "gone", at=AT)
        store.result_append(self.db, "tr-1", "tests/0", "nextest-binary",
                            {"part": "0", "verdict": "GREEN", "secs": 1.5}, at=AT)
        with self.assertRaises(sqlite3.IntegrityError):
            store.result_append(self.db, "tr-1", "tests/0", "nextest-binary", {}, at=AT)
        store.edge_link(self.db, "T-2", "T-1", "depends_on", "core",
                        {"dep": "fs", "lag_min": 0}, at=AT)
        with self.assertRaises(ValueError):
            store.edge_link(self.db, "T-2", "T-1", "relates", "core", at=AT)



class RunnerStateTest(StoreCase):
    """GH #1129: the store's writer for `runners.state` (hygiene's idle
    power-off writes `off`; the schedule reads it and wakes the lane)."""

    def test_runner_set_state_inserts_then_updates(self):
        store.runner_set_state(self.db, "north", "off", at="2026-10-09T10:00:00Z")
        row = self.db.execute("SELECT * FROM runners WHERE name = 'north'").fetchone()
        self.assertEqual(("lane:north", "off", "2026-10-09T10:00:00Z"),
                         (row["party"], row["state"], row["updated_at"]))
        self.db.execute("UPDATE runners SET capacity = '{\"cpu\": 8}' WHERE name = 'north'")
        store.runner_set_state(self.db, "north", "idle", at="2026-10-09T11:00:00Z")
        row = self.db.execute("SELECT * FROM runners WHERE name = 'north'").fetchone()
        self.assertEqual(("idle", '{"cpu": 8}'), (row["state"], row["capacity"]))
        self.assertEqual("idle", store.runner_state(self.db, "north"))
        self.assertIsNone(store.runner_state(self.db, "south"))


class ReadOnlyTest(StoreCase):
    def test_read_commands_never_create_or_migrate_the_store(self):
        # `store check` and `receipt` are reads: a missing store is an error,
        # never a fresh one (with a migration) on the shared path.
        missing = pathlib.Path(self.tmp.name) / "nope" / "pipeline.db"
        for argv in (["store", "check", "--db", str(missing)],
                     ["receipt", "run-0123456789ab", "--db", str(missing)]):
            with contextlib.redirect_stdout(io.StringIO()), \
                    contextlib.redirect_stderr(io.StringIO()) as err:
                code = cli.main(argv)
            self.assertNotEqual(code, 0, argv)
            self.assertIn("no store", err.getvalue(), argv)
            self.assertFalse(missing.parent.exists(), argv)
        # An existing store opens read-only: a write is refused.
        ro = store.open_ro(self.path)
        try:
            self.assertEqual(store.check(ro)["schema"], store.SCHEMA_VERSION)
            with self.assertRaises(sqlite3.OperationalError):
                ro.execute("INSERT INTO counters VALUES ('x', 1)")
        finally:
            store.close(ro)
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            self.assertEqual(cli.main(["store", "check", "--db", str(self.path)]), 0)
        self.assertIn(store.SCHEMA_VERSION, out.getvalue())


class TheBoundedRetry(unittest.TestCase):
    """welle-tb S3 (#1161): a locked write is retried a bounded number of
    times, then it fails loudly -- it never vanishes."""

    def test_a_locked_write_is_retried_and_then_succeeds(self):
        calls, slept = [], []

        def write(x):
            calls.append(x)
            if len(calls) < store.WRITE_ATTEMPTS:
                raise sqlite3.OperationalError("database is locked")
            return "written"

        got = store.retry_write(write, 7, sleep=slept.append)
        self.assertEqual("written", got)
        self.assertEqual([7] * store.WRITE_ATTEMPTS, calls)
        self.assertEqual(store.WRITE_ATTEMPTS - 1, len(slept))

    def test_a_write_locked_every_time_fails_loudly(self):
        calls = []

        def write():
            calls.append(1)
            raise sqlite3.OperationalError("database is locked")

        with self.assertRaises(store.StoreWriteError) as ctx:
            store.retry_write(write, sleep=lambda s: None)
        self.assertEqual(store.WRITE_ATTEMPTS, len(calls))
        self.assertIn("database is locked", str(ctx.exception))
        self.assertIn("%d attempts" % store.WRITE_ATTEMPTS, str(ctx.exception))

    def test_other_errors_are_not_retried(self):
        calls = []

        def write():
            calls.append(1)
            raise sqlite3.IntegrityError("UNIQUE constraint failed")

        with self.assertRaises(sqlite3.IntegrityError):
            store.retry_write(write, sleep=lambda s: None)
        self.assertEqual(1, len(calls))

if __name__ == "__main__":
    unittest.main()
