# shellcheck shell=bash
# The kit's one Python for its Bash entries (re-check tv-bn M7t B-M4, B-H3):
# `lane.PYTHON` of scripts/pipeline/lane.py -- the absolute interpreter,
# isolated (`-I`: no PYTHONPATH, no user site, no script directory taken from
# the caller). Every kit shell script sources this file and starts Python as
# `"${KIT_PY[@]}" ...`; a command string for a remote shell takes
# `$(kit_py_q)`. The words are pinned at run time against `lane.PYTHON`
# (OR-T11-27) and every kit shell entry is scanned for another spelling
# (scripts/tests/test_lane_engstelle.py, EntryIsolation). It lives under
# scripts/pipeline/ so it travels with the kit to every lane.
#
# Assigned unconditionally: a scalar KIT_PY in the caller's environment never
# survives the source.

KIT_PY=(/usr/bin/python3 -I)

# The same words, quoted for a command string a remote shell parses.
kit_py_q() {
    local out="" w
    for w in "${KIT_PY[@]}"; do out+="${out:+ }$(printf '%q' "$w")"; done
    printf '%s' "$out"
}
