# Source

`scenarios.json`, `pass.py` and `run_model.py` are byte copies of
`meclaw-next/23-display/model/{scenarios.json,pass.py,run.py}` (private tree). The
description `display-hive.md` there is the one normative document of this template
(`docs/development-rules.md` § 10); these copies are its pins as they travel with the
template. `run_display_scenarios.py` runs them against the curator (`../compose.py`).

`SOURCE` beside this page is the mark: one line, `meclaw-next <sha> <date>`, naming the
commit of the description tree these bytes came from.

Renew: run `python3 scripts/display_sync.py`. It fetches the three files from
`${MECLAW_DISPLAY_HIVE:-~/projeks/MeClaw/meclaw-next}/23-display/model/`, writes the
mark, and refuses while that model carries uncommitted edits — the mark promises that
`git show <sha>:23-display/model/<file>` yields exactly these bytes, and only a commit
can keep that promise. Without the tree it touches nothing.

The drift lock
`crates/meclaw-cells/tests/710_the_scenarios_run_against_the_curator.rs` compares the
copies against the mark's commit in a strand and against the tree's working copy in
the integration and release passes (GH #753).
