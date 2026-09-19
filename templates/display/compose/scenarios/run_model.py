"""Runs every scenario of scenarios.json against pass.py.

    python3 run.py            all scenarios
    python3 run.py S-012 Q-03 only those ids

Prints `PASS <id> <title>` or `FAIL <id> <title>: <field> expected X got Y`, a summary line
`N/N`, exit code 1 on any FAIL. Stdlib only.

Scenario form:
    {"id", "title", "source",
     "given": "<fixture name>" | {"fixture"?: name, "settings"?: {...}, "screens"?: {...}, "setup"?: [step, ...]},
     "steps": [{"event": {...}, "now": ms, "expect"?: {...}}, ...],
     "expect": {"<helper>" | "<helper>(<arg>)" : value, ...}}
A fixture is {"settings", "screens", "setup": [step, ...]}; a state is built by running the
setup steps from an empty state. Expectations name the helpers of pass.py; a helper with an
argument is written `rung(view.ambient.weather)`; floats compare rounded to 4 places.
"""

import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def load_model():
    spec = importlib.util.spec_from_file_location("hivepass", os.path.join(HERE, "pass.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def resolve(fixtures, given):
    """A fixture may extend another (`"fixture": parent`): settings and screens merge,
    the parent's setup runs first."""
    if isinstance(given, str):
        given = {"fixture": given}
    settings, screens, setup = {}, {}, []
    if given.get("fixture"):
        p_settings, p_screens, p_setup = resolve(fixtures, fixtures[given["fixture"]])
        settings.update(p_settings)
        screens.update(p_screens)
        setup.extend(p_setup)
    settings.update(given.get("settings") or {})
    screens.update(given.get("screens") or {})
    setup.extend(given.get("setup") or [])
    return settings, screens, setup


def build_state(model, fixtures, given):
    settings, screens, setup = resolve(fixtures, given)
    state = model.empty_state(settings, screens)
    for step in setup:
        state = model.run_pass(state, step["event"], step["now"])
    return state


def evaluate(model, state, key):
    name, arg = key, None
    if "(" in key and key.endswith(")"):
        name, arg = key[:key.index("(")], key[key.index("(") + 1:-1]
    fn = getattr(model, name)
    if arg is None or arg == "":
        return fn(state)
    args = [a.strip() for a in arg.split(",")]
    return fn(state, *args)


def norm(value):
    if isinstance(value, float):
        return round(value, 4)
    if isinstance(value, list):
        return [norm(v) for v in value]
    if isinstance(value, tuple):
        return [norm(v) for v in value]
    if isinstance(value, dict):
        return {k: norm(v) for k, v in value.items()}
    return value


def check(model, state, expect, failures, where=""):
    for key, want in (expect or {}).items():
        try:
            got = evaluate(model, state, key)
        except Exception as e:  # a helper that raises is a failure with a reason
            failures.append("%s%s raised %s: %s" % (where, key, type(e).__name__, e))
            continue
        if norm(got) != norm(want):
            failures.append("%s%s expected %s got %s" % (where, key, json.dumps(norm(want)), json.dumps(norm(got))))


def run_scenario(model, fixtures, sc):
    failures = []
    try:
        state = build_state(model, fixtures, sc.get("given") or {})
        for i, step in enumerate(sc.get("steps") or []):
            state = model.run_pass(state, step["event"], step["now"])
            check(model, state, step.get("expect"), failures, "step %d: " % (i + 1))
        check(model, state, sc.get("expect"), failures)
    except Exception as e:
        failures.append("raised %s: %s" % (type(e).__name__, e))
    return failures


def main(argv):
    model = load_model()
    with open(os.path.join(HERE, "scenarios.json")) as f:
        doc = json.load(f)
    fixtures = doc.get("fixtures") or {}
    only = set(argv[1:])
    total = passed = 0
    for sc in doc["scenarios"]:
        if only and sc["id"] not in only:
            continue
        total += 1
        failures = run_scenario(model, fixtures, sc)
        if failures:
            print("FAIL %s %s: %s" % (sc["id"], sc["title"], "; ".join(failures)))
        else:
            passed += 1
            print("PASS %s %s" % (sc["id"], sc["title"]))
    print("%d/%d" % (passed, total))
    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
