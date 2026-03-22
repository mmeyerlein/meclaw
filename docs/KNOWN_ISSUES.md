# Known Issues & Workarounds

## ParadeDB BM25 `rt_fetch used out-of-bounds`

### Status: RESOLVED (v0.3.1)

### Problem
ParadeDB pg_search < 0.19.0 had a bug where the BM25 index's `custom_scan` 
feature crashed with `rt_fetch used out-of-bounds` when used with:
- CTEs containing FULL OUTER JOINs
- `paradedb.score()` in ORDER BY within complex queries
- Rapid TRUNCATE + INSERT cycles (benchmark pattern)

This caused 490/500 queries to crash in the benchmark, falling back to 
a naive `ORDER BY created_at DESC` retrieval.

### Root Cause
- GitHub Issue #2462: custom_scan evaluates Var nodes in wrong query context
- GitHub Issue #3135: OR EXISTS + multiple JOINs + ParadeDB functions

### Resolution
Upgraded pg_search from **0.15.10 → 0.22.2** (Commit d12a36b).

Also required:
- CPU with AVX2 support (host passthrough on KVM)
- `paradedb.score` → `pdb.score` (v0.19.0 breaking change, 29 occurrences)

### Previous Workarounds (REMOVED in v0.3.1)
These were attempted before the upgrade and are documented for reference:

1. **DROP+CREATE BM25 index after TRUNCATE** — Rebuilt index on every brain reset.
   Cost: ~0.3s per question, ~150s for 500 questions. Partially effective.

2. **VACUUM after embed batch** — Refreshed BM25 index after INSERT.
   Cost: ~50ms per call, held short table lock. Only partially effective.

3. **VACUUM before retrieve** — Second refresh after pipeline UPDATEs.
   Cost: ~50ms per call. Redundant with #2.

4. **`sanitize_bm25_query()`** — Strips special chars from queries before `@@@`.
   Still in place (sql/47) as it prevents ParseError from LLM-generated queries
   with apostrophes and parentheses. This is NOT a workaround but a real fix.

### If the bug returns
If `rt_fetch` errors reappear on future ParadeDB versions:
1. First try: `SET paradedb.enable_custom_scan = false;` (disables buggy codepath)
2. If persistent: Re-add VACUUM workarounds from git history (commit cf2a83d)
3. File bug report at https://github.com/paradedb/paradedb/issues

### Links
- https://github.com/paradedb/paradedb/issues/2462
- https://github.com/paradedb/paradedb/issues/3135
- MeClaw commits: d12a36b, 2050e4d, cf2a83d (workarounds), 0d04300 (workarounds)
