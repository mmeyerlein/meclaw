# Source

`scenarios.json`, `pass.py` and `run_model.py` are byte copies of
`meclaw-next/23-display/model/{scenarios.json,pass.py,run.py}` (private tree). The
description `display-hive.md` there is the one normative document of this template
(`docs/development-rules.md` § 10); these copies are its pins as they travel with the
template. Renew: copy the three files; the drift lock
`crates/meclaw-cells/tests/710_the_scenarios_run_against_the_curator.rs` compares them
with `${MECLAW_DISPLAY_HIVE:-$HOME/projeks/MeClaw/meclaw-next/23-display}/model/`.
`run_display_scenarios.py` runs them against the curator (`../compose.py`).
