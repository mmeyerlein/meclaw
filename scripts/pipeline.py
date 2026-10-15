#!/usr/bin/python3 -I
"""Starter of the pipeline core; the commands live in scripts/pipeline/cli.py."""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from scripts.pipeline import gitenv  # noqa: E402

gitenv.enter_tree()
from scripts.pipeline.cli import main  # noqa: E402

sys.exit(main())
