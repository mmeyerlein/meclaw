"""The Python suite of the kit.

Importing the package drops git's repository variables (`GIT_DIR`,
`GIT_WORK_TREE`, ...): a suite started with them set ran every test
repository's `git init`/`config`/`commit` against the caller's repository --
core.worktree and user.name landed in the shared config of meclaw-core
(pd-1155 kit find). Every subprocess of a test inherits the cleaned environment.

It drops the verdict variables as well (`lane_sync.sh VERDICT_ENV`,
MECLAW_GATE_MODE among them): the kit suite is a station whose cache key does
not carry them (orchestrator OR-T12-21, `pipeline/cache.py` `env:`), so its
verdict must not read them -- a case that needs one sets it itself.
"""

import os as _os

from ..pipeline import gitenv as _gitenv

for _name in _gitenv.GIT_REPO_VARS:
    _os.environ.pop(_name, None)

from ..pipeline import lane as _lane  # noqa: E402 -- after the git variables are gone

for _name in _lane.verdict_env():
    _os.environ.pop(_name, None)
