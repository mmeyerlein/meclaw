"""The owner's questions the gate prints on ASK (GH #1149).

Since W3 (#1117) `scripts/gate.sh` is a thin wrapper and the question block
lived only in `scripts/gate_legacy.sh`, so the pipeline runner answered ASK
with a summary line and no way forward. The text stays here as LITERAL lines,
never assembled from a variable: the persona runner's self-test
(`workshop/evals/persona/run_persona.py`, pin
`a_run_that_can_spend_asks_the_owner_first`) holds its `measure_commands()`
against this file line by line, so the command the owner is told to run is
the command that exists.
"""

PERSONA_RECEIPT = """\
  persona-receipt: The committed persona report does not describe this tree. Three answers:
    1. measure again on the local model (no model spend, about 1-2 hours; the hosted
       embedder costs cents, so the runner asks for a budget as well):
         LOCAL_LLM_MODEL=<model id served at LOCAL_LLM_BASE_URL> python3 workshop/evals/persona/run_persona.py --live --engine local --budget-eur <EUR>
       then, if it is green, commit workshop/evals/persona/last_run.json and transcripts/, and run this gate again
    2. measure again on a paid provider (costs money; design estimate 1.2-2.0 EUR a run,
       the runner prints its own estimate and refuses to spend without a budget):
         python3 workshop/evals/persona/run_persona.py --live --engine paid --budget-eur <EUR>
       then, if it is green, commit the report and run this gate again
    3. go on without a fresh measurement -- the owner's call, written into the receipt:
         scripts/gate.sh <mode> --decide persona-receipt=without:"OR-<id>: <reason>"   (with this run's other options)
"""

BY_STATION = {"persona-receipt": PERSONA_RECEIPT}


def block(stations, mode):
    """The question text for the asking stations of a run ('' when none asks)."""
    return "".join(BY_STATION[s].replace("<mode>", mode) for s in stations if s in BY_STATION)
