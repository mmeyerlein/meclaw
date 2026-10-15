#!/usr/bin/python3 -I
"""Starter of the pipeline core; the commands live in scripts/pipeline/cli.py."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from scripts.pipeline import gitenv  # noqa: E402

# `-C <dir>` first, as git: an inherited GIT_DIR is then read from there (GH #1199 (b)).
sys.argv[1:] = gitenv.take_leading_dirs(sys.argv[1:], "pipeline")
gitenv.enter_tree()
from scripts.pipeline.cli import main  # noqa: E402

sys.exit(main())
