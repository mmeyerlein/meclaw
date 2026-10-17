"""Tests for the conformance tool `workshop/tools/llm-conformance/`.

The `llm` cell's 958 tests pin its own contract against a mock; none of them
asked what a real endpoint takes or sends back. A chat request carried
`temperature` to a model whose endpoints do not list it: with
`provider.require_parameters` the provider answered 404, without it the
parameter was dropped silently (GH #993, OR-LP.ME.7 of wave Loop 13).

The tool probes a model at its endpoint against checkpoints K1-K9 and reports
every deviation as a finding. These tests run it offline against
`fake_provider.py` (a loopback server, no network, no money, no key):

  * a refused parameter is classified, not hidden (K3), and a catalogue
    parameter the model refuses is red,
  * a wrong parameter injected into every probe is a finding with exit 1,
  * a lenient local endpoint never yields `taken`, on either wire,
  * response-side breakage (tool arguments, usage, SSE, exhausted reasoning,
    a listing that lies) is red or named,
  * the cost guard stops BEFORE a call, and refuses a typo-sized budget,
  * the key never reaches a report, journal or recorded response,
  * `--record` writes the files of the contract (wave README section 6),
  * the cell's request fixture is sent as-is,
  * the tool's list of sampling parameters is the cell's list,
  * the three tool faults of the first live run M1 (05.10.) stay fixed: a 200
    in strict mode is `taken` only for a listed parameter, K5 sends no forced
    `tool_choice`, and K5 has room for a thinking model.
"""

import contextlib
import io
import json
import os
import pathlib
import re
import sys
import tempfile
import unittest
from scripts.tests import private_tree, store_isolation

REPO = pathlib.Path(__file__).resolve().parents[2]
TOOL_DIR = REPO / "workshop" / "tools" / "llm-conformance"

conformance = None
fake_provider = None


def setUpModule():
    # `workshop/` never travels with the export (R2c). This module does -- it
    # sits in ROOT_FILES because `scripts/tests/test_gate_plan.py` names it --
    # so in the published tree it must skip cleanly instead of failing on a
    # tool that was never shipped. Hence the import here, not at the top.
    global conformance, fake_provider
    # `require` (GH #1199): a skip in the export tree, a failure in the private
    # tree, where the directory can not go quiet (re-check RC-K3 I3).
    private_tree.require("workshop/tools/llm-conformance")
    store_isolation.isolate()
    sys.path.insert(0, str(TOOL_DIR))
    import conformance as _conformance
    import fake_provider as _fake_provider
    conformance, fake_provider = _conformance, _fake_provider

MEASURED_KEYS = {
    "model_id", "endpoint_class", "measured_at", "tool_version", "params",
    "listing", "reasoning_field", "checks", "cost_usd",
}
CHECKS = ["K1", "K2", "K3", "K4", "K5", "K6", "K7", "K8", "K9"]


class ConformanceCase(unittest.TestCase):
    def setUp(self):
        self.fake = fake_provider.FakeProvider()
        self.base = self.fake.start()
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.addCleanup(self.fake.stop)
        # The journal of a run without --record goes to the temp dir; keep
        # it inside this test's directory.
        self._old_tempdir = tempfile.tempdir
        tempfile.tempdir = self.tmp.name
        self.addCleanup(setattr, tempfile, "tempdir", self._old_tempdir)

    def run_tool(self, model, *extra, endpoint_class="openrouter"):
        argv = ["--model", model, "--base-url", self.base, "--no-key"]
        if endpoint_class:
            argv += ["--endpoint-class", endpoint_class]
        argv += list(extra)
        out = io.StringIO()
        err = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code, report = conformance.run(argv)
        return code, report, out.getvalue() + err.getvalue()


class TestConformance(ConformanceCase):
    def test_a_model_without_temperature_is_reported_refused(self):
        code, report, out = self.run_tool("fake/strict-no-temperature")
        self.assertEqual(report["params"]["temperature"], "refused")
        self.assertIn("No endpoints found", report["checks"]["K3"]["note"])
        self.assertEqual(report["checks"]["K3"]["status"], "green")
        self.assertEqual(code, 0)
        self.assertRegex(
            out, r"CONFORMANCE fake/strict-no-temperature \d+/\d+ [0-9.]+ GREEN")

    def test_a_catalogue_param_the_model_refuses_is_red(self):
        code, report, out = self.run_tool(
            "fake/strict-no-temperature", "--catalog-params", "temperature")
        self.assertEqual(report["checks"]["K3"]["status"], "red")
        self.assertIn("RED", out.strip().splitlines()[-1])

    def test_injected_wrong_param_is_a_finding_and_exit_nonzero(self):
        code, report, out = self.run_tool(
            "fake/strict-no-temperature", "--inject-param", "temperature=0.2",
            "--expect-green")
        self.assertEqual(report["checks"]["K1"]["status"], "red")
        self.assertIn("refused temperature", report["checks"]["K1"]["note"])
        self.assertIn("No endpoints found", report["checks"]["K1"]["note"])
        self.assertEqual(code, 1)
        # The injected value really was in the probe body.
        sent = json.loads(self.fake.bodies[0])
        self.assertEqual(sent["temperature"], 0.2)

    def test_local_acceptance_is_never_taken(self):
        code, report, _ = self.run_tool(
            "fake/local-lenient", endpoint_class="local")
        self.assertEqual(set(report["params"]), set(conformance.SAMPLING_PARAMS))
        self.assertEqual(set(report["params"].values()), {"accepted_unverified"})
        self.assertEqual(report["checks"]["K4"]["status"], "n/a")
        self.assertEqual(report["reasoning_field"], "reasoning_content")
        # No strict field ever reaches a local endpoint.
        for raw in self.fake.bodies:
            self.assertNotIn("provider", json.loads(raw))
            self.assertNotIn("usage", json.loads(raw))

    def test_local_acceptance_is_never_taken_on_the_messages_wire(self):
        # A 200 proves a parameter only on api.anthropic.com, which refuses
        # every unknown field; a local Messages server (vLLM, LiteLLM) may drop
        # it silently -- the GH #993 class (re-check AN3 I2).
        _, report, out = self.run_tool(
            "claude-fake-ok", "--wire", "messages", "--only", "K1,K3",
            endpoint_class="local")
        self.assertEqual(report["checks"]["K3"]["status"], "green", out)
        self.assertNotIn("taken", report["params"].values())
        for name in ("reasoning", "reasoning_effort"):
            self.assertEqual(report["params"][name], "accepted_unverified", name)

    def test_tool_arguments_must_be_a_json_string(self):
        _, report, _ = self.run_tool("fake/broken-tools")
        self.assertEqual(report["checks"]["K5"]["status"], "red")
        self.assertEqual(report["checks"]["K1"]["status"], "green")

    def test_missing_usage_is_red(self):
        _, report, _ = self.run_tool("fake/no-usage")
        self.assertEqual(report["checks"]["K2"]["status"], "red")
        self.assertEqual(report["checks"]["K1"]["status"], "green")

    def test_a_200_without_usage_books_the_worst_case(self):
        # A sent call answered 200 without `usage` was paid for all the same;
        # booking 0 $ would blind the guard for the rest of the run.
        record = pathlib.Path(self.tmp.name) / "rec"
        _, report, _ = self.run_tool("fake/no-usage", "--record", str(record))
        self.assertGreater(report["cost_usd"], 0.0)
        lines = [json.loads(l) for l in
                 next(record.rglob("cost.jsonl")).read_text().splitlines()]
        sent = [l for l in lines if l["status"] == "ok"]
        self.assertEqual(len(sent), self.fake.calls)
        for line in sent:
            self.assertEqual(line["cost_source"], "worst_case")
            self.assertEqual(line["cost"], line["worst"])

    def test_a_refused_call_costs_nothing(self):
        # A3 shape: the injected parameter is refused with a 404 -- no answer,
        # no bill. The worst case is booked only for a 200.
        code, report, out = self.run_tool(
            "fake/strict-no-temperature", "--inject-param", "temperature=0.2",
            "--only", "K1", "--expect-green")
        self.assertEqual(code, 1)
        self.assertEqual(report["cost_usd"], 0.0)
        self.assertRegex(out.strip().splitlines()[-1], r" 0\.000000 RED$")

    def test_only_rejects_an_unknown_checkpoint(self):
        # A typo in a proof run (`k1`, `K10`) must not select nothing and pass.
        for name in ("k1", "K10", "K1,K3x"):
            with self.subTest(only=name):
                code, report, out = self.run_tool(
                    "fake/strict-all", "--only", name, "--expect-green")
                self.assertEqual(code, 2)
                self.assertEqual(report, {})
                self.assertIn("--only", out)
        self.assertEqual(self.fake.calls, 0)
        self.assertEqual(self.fake.gets, 0)

    def test_nothing_counted_is_never_green(self):
        # K9 without --record is n/a: a run that counted nothing proved nothing.
        code, _, out = self.run_tool("fake/strict-all", "--only", "K9",
                                     "--expect-green")
        last = out.strip().splitlines()[-1]
        self.assertIn(" 0/0 ", last)
        self.assertTrue(last.endswith("RED"), last)
        self.assertEqual(code, 1)

    def test_sse_on_the_chat_wire_is_red(self):
        code, report, _ = self.run_tool("fake/sse-on-chat", "--expect-green")
        self.assertEqual(report["checks"]["K1"]["status"], "red")
        self.assertEqual(report["checks"]["K7"]["status"], "red")
        self.assertEqual(code, 1)

    def test_reasoning_exhausted_is_named_not_hidden(self):
        _, report, _ = self.run_tool("fake/reasoning-exhausted")
        self.assertEqual(report["checks"]["K6"]["status"], "green")
        self.assertIn("reasoning exhausted", report["checks"]["K6"]["note"])
        self.assertEqual(report["reasoning_field"], "reasoning")

    def test_listing_and_measurement_disagree(self):
        _, report, _ = self.run_tool("fake/listing-lies")
        self.assertEqual(report["checks"]["K4"]["status"], "red")
        self.assertIn("temperature", report["checks"]["K4"]["note"])

    def test_budget_stops_before_the_call(self):
        code, report, out = self.run_tool(
            "fake/strict-all", "--budget-usd", "0.0002")
        self.assertEqual(self.fake.calls, 2, "the third call must not be sent")
        self.assertEqual(code, 2)
        self.assertIn("budget", out)
        self.assertLessEqual(report["cost_usd"], 0.0002 + 1e-12)

    def test_budget_hard_ceiling(self):
        code, _, out = self.run_tool("fake/strict-all", "--budget-usd", "5")
        self.assertEqual(code, 2)
        self.assertEqual(self.fake.calls, 0)
        self.assertEqual(self.fake.gets, 0, "not even the free listing")
        self.assertIn("0.25", out)

    def test_the_key_never_leaves(self):
        secret = "h5-fake-credential-4f1c9e27b8d3a6"
        name = "H5_CONFORMANCE_TEST_KEY"
        os.environ[name] = secret
        self.addCleanup(os.environ.pop, name, None)
        record = pathlib.Path(self.tmp.name) / "rec"
        argv = ["--model", "fake/mirror-auth", "--base-url", self.base,
                "--key-env", name, "--endpoint-class", "openrouter",
                "--record", str(record), "--verbose"]
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            conformance.run(argv)
        # The fake did get it, and did mirror it -- or the test proves nothing.
        self.assertIn("Bearer " + secret, self.fake.auth_headers)
        self.assertTrue(self.fake.mirrored)
        # ... and no listing call carried a header (P1 section 3 K4: the
        # listing is free and public, the key stays off it even with --key-env).
        self.assertGreater(self.fake.gets, 0, "the listing was not fetched")
        self.assertEqual(self.fake.get_auth_headers, [])
        self.assertNotIn(secret, out.getvalue())
        files = [p for p in record.rglob("*") if p.is_file()]
        self.assertTrue(any(p.name == "cost.jsonl" for p in files))
        self.assertTrue(any(p.name == "basic.json" for p in files))
        for path in files:
            self.assertNotIn(secret, path.read_text(), str(path))

    def test_record_writes_the_contract_files(self):
        record = pathlib.Path(self.tmp.name) / "rec"
        code, _, _ = self.run_tool(
            "fake/strict-no-temperature", "--record", str(record))
        slug = record / "fake__strict-no-temperature"
        measured = json.loads((slug / "measured.json").read_text())
        self.assertEqual(set(measured), MEASURED_KEYS)
        self.assertEqual(sorted(measured["checks"]), CHECKS)
        for check in measured["checks"].values():
            self.assertIn(check["status"], ("green", "red", "n/a"))
            self.assertIn("note", check)
        self.assertEqual(measured["checks"]["K9"]["status"], "green")
        self.assertEqual(measured["endpoint_class"], "openrouter")
        self.assertEqual(measured["tool_version"], "1")
        self.assertNotIn("temperature", measured["listing"])
        basic = json.loads((slug / "responses" / "basic.json").read_text())
        self.assertIn("choices", basic)
        self.assertNotIn("headers", basic)
        self.assertTrue((slug / "responses" / "tool.json").is_file())
        self.assertTrue((slug / "responses" / "reasoning.json").is_file())
        text = "".join(p.read_text() for p in slug.rglob("*") if p.is_file())
        self.assertNotIn("127.0.0.1", text, "no address in a recorded file")

    def test_cell_request_is_sent_as_is(self):
        requests = pathlib.Path(self.tmp.name) / "requests"
        requests.mkdir()
        body = {
            "model": "fake/local-lenient",
            "messages": [
                {"role": "system", "content": "Answer with the single word OK."},
                {"role": "user", "content": "Ping."},
            ],
            "max_tokens": 512,
            "reasoning": {"effort": "low"},
            "provider_extra_marker": [1, 2, 3],
        }
        (requests / "local__fake__local-lenient.json").write_text(json.dumps({
            "model_id": "fake/local-lenient", "endpoint_class": "local",
            "wire_dialect": "chat", "body": body, "expected_dropped": [],
        }))
        _, report, _ = self.run_tool(
            "fake/local-lenient", "--requests-dir", str(requests),
            endpoint_class="local")
        self.assertEqual(report["checks"]["K8"]["status"], "green")
        expected = dict(body)
        expected["max_tokens"] = 32
        self.assertEqual(self.fake.bodies[-1], json.dumps(expected).encode())
        self.assertIn("max_tokens", report["checks"]["K8"]["note"])

    def test_sampling_params_mirror(self):
        src = (REPO / "crates/meclaw-cells/src/llm/translate.rs").read_text()
        block = re.search(
            r"const SAMPLING_PARAMS: &\[&str\] = &\[(.*?)\];", src, re.S)
        self.assertIsNotNone(block, "SAMPLING_PARAMS moved -- update the mirror")
        cell = re.findall(r'"([a-z_]+)"', block.group(1))
        self.assertEqual(list(conformance.SAMPLING_PARAMS), cell)


class TestM1Findings(ConformanceCase):
    """The first live run M1 (05.10., wave Haertung 5) met three tool faults.

    `plans/welle-haertung-provider-2026-10-05/messung/M1/`: all seven OpenRouter
    models were red at K4 for `thinking_token_budget` (strict 200, unlisted);
    two were red at K5 for the forced `tool_choice` (404), three for
    `finish_reason` `length` at a 32-token K5.
    """

    def k5_body(self):
        bodies = [json.loads(raw) for raw in self.fake.bodies]
        return [b for b in bodies if b.get("tools")][-1]

    # F-M1-1 ---------------------------------------------------------------
    def test_strict_200_for_an_unlisted_param_is_not_taken(self):
        code, report, _ = self.run_tool("fake/strict-ignores-unknown",
                                        "--expect-green")
        params = report["params"]
        self.assertEqual(params["thinking_token_budget"], "accepted_unverified")
        for name in ("temperature", "top_p", "reasoning", "reasoning_effort"):
            self.assertEqual(params[name], "taken", name)
        self.assertIn("not in the listing", report["checks"]["K3"]["note"])
        self.assertEqual(report["checks"]["K4"]["status"], "green",
                         report["checks"]["K4"]["note"])
        self.assertIn("thinking_token_budget", report["checks"]["K4"]["note"])
        self.assertEqual(code, 0)

    def test_without_a_listing_nothing_is_taken_on_openrouter(self):
        fake = fake_provider.FakeProvider(models=["fake/local-lenient"])
        self.base = fake.start()
        self.addCleanup(fake.stop)
        _, report, _ = self.run_tool("fake/local-lenient", "--price-in", "1",
                                     "--price-out", "1")
        self.assertNotIn("taken", report["params"].values())
        self.assertEqual(set(report["params"].values()), {"accepted_unverified"})
        self.assertEqual(report["checks"]["K4"]["status"], "red")
        self.assertIn("listing", report["checks"]["K4"]["note"])

    def test_a_listed_param_refused_in_strict_mode_stays_red(self):
        _, report, _ = self.run_tool("fake/listing-lies")
        self.assertEqual(report["params"]["temperature"], "refused")
        self.assertEqual(report["checks"]["K4"]["status"], "red")
        self.assertIn("listed but not taken: ['temperature']",
                      report["checks"]["K4"]["note"])

    # F-M1-2 ---------------------------------------------------------------
    def test_k5_sends_no_forced_tool_choice(self):
        _, report, _ = self.run_tool("fake/no-forced-tool-choice")
        self.assertEqual(report["checks"]["K5"]["status"], "green",
                         report["checks"]["K5"]["note"])
        body = self.k5_body()
        self.assertNotIn("tool_choice", body)
        self.assertIn("echo", json.dumps(body["messages"]))

    def test_k5_without_a_tool_call_is_red(self):
        _, report, _ = self.run_tool("fake/ignores-tools")
        self.assertEqual(report["checks"]["K5"]["status"], "red")
        self.assertEqual(report["checks"]["K1"]["status"], "green")

    # F-M1-3 ---------------------------------------------------------------
    def test_k5_has_room_for_a_thinking_model(self):
        _, report, _ = self.run_tool("fake/thinks-before-tools", "--expect-green")
        self.assertEqual(report["checks"]["K5"]["status"], "green",
                         report["checks"]["K5"]["note"])
        self.assertGreaterEqual(self.k5_body()["max_tokens"],
                                conformance.TOOL_MIN_TOKENS)

    def test_k5_keeps_reasoning_low_only_when_k3_took_it(self):
        self.run_tool("fake/thinks-before-tools")
        self.assertEqual(self.k5_body().get("reasoning"), {"effort": "low"})
        self.fake.bodies.clear()
        self.run_tool("fake/local-lenient", endpoint_class="local")
        self.assertNotIn("reasoning", self.k5_body())
        self.fake.bodies.clear()
        self.run_tool("fake/thinks-before-tools", "--only", "K1,K5")
        self.assertNotIn("reasoning", self.k5_body())

    def test_length_without_a_tool_call_stays_red(self):
        _, report, _ = self.run_tool("fake/thinks-forever")
        self.assertEqual(report["checks"]["K5"]["status"], "red")
        self.assertIn("length", report["checks"]["K5"]["note"])

    def test_the_bigger_k5_still_fits_the_budget_at_opus_prices(self):
        # Opus 5.5 lists 20 USD per million completion tokens (M1 05.10.):
        # even if EVERY probe of a full run cost its worst case, 0.05 USD holds.
        record = pathlib.Path(self.tmp.name) / "rec"
        code, _, out = self.run_tool(
            "fake/thinks-before-tools", "--price-in", "5", "--price-out", "20",
            "--budget-usd", "0.05", "--record", str(record), "--expect-green")
        self.assertEqual(code, 0, out)
        lines = [json.loads(l) for l in
                 next(record.rglob("cost.jsonl")).read_text().splitlines()]
        self.assertLessEqual(sum(l["worst"] for l in lines), 0.05)


class TestMessagesWire(ConformanceCase):
    """The native Messages wire (R-T10-1) and the time to the first tool call.

    A direct Claude row is measured on the wire the cell will speak, not on a
    compatibility endpoint; the latency probe answers R-WP-3 (answer start =
    wall clock to the tool call, no SSE: K7 forbids `stream`).
    """

    def run_messages(self, model, *extra):
        return self.run_tool(model, "--wire", "messages", "--price-in", "1",
                             "--price-out", "5", *extra, endpoint_class="anthropic")

    # T10-12 ---------------------------------------------------------------
    def test_messages_wire_sends_the_native_minimal_request(self):
        secret = "an3-fake-credential-7c1d52e9"
        name = "AN3_CONFORMANCE_TEST_KEY"
        os.environ[name] = secret
        self.addCleanup(os.environ.pop, name, None)
        record = pathlib.Path(self.tmp.name) / "rec"
        argv = ["--model", "claude-fake-ok", "--base-url", self.base,
                "--key-env", name, "--endpoint-class", "anthropic",
                "--wire", "messages", "--only", "K1,K2,K7,K9",
                "--price-in", "1", "--price-out", "5", "--record", str(record),
                "--expect-green"]
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code, report = conformance.run(argv)
        self.assertEqual(code, 0, out.getvalue())
        for k in ("K1", "K2", "K7", "K9"):
            self.assertEqual(report["checks"][k]["status"], "green", k)
        self.assertEqual(self.fake.gets, 0, "no listing probe on the Messages wire")
        path, headers = self.fake.requests[0]
        self.assertTrue(path.endswith("/messages"), path)
        self.assertEqual(headers.get("x-api-key"), secret)
        self.assertEqual(headers.get("anthropic-version"), "2023-06-01")
        self.assertNotIn("authorization", headers)
        body = json.loads(self.fake.bodies[0])
        self.assertEqual(body["system"], conformance.SYSTEM)
        self.assertEqual(body["messages"], [{"role": "user", "content": conformance.USER}])
        for field in ("stream", "usage", "provider"):
            self.assertNotIn(field, body)
        measured = json.loads((record / "claude-fake-ok" / "measured.json").read_text())
        self.assertEqual(measured["endpoint_class"], "anthropic")
        self.assertNotIn(secret, out.getvalue())
        for p in record.rglob("*"):
            if p.is_file():
                self.assertNotIn(secret, p.read_text(), str(p))

    # T10-13 ---------------------------------------------------------------
    def test_messages_wire_measures_native_sampling_thinking_and_tool_use(self):
        code, report, out = self.run_messages("claude-fake-ok", "--expect-green")
        self.assertEqual(report["params"], {
            "temperature": "refused", "top_p": "refused", "reasoning": "taken",
            "reasoning_effort": "taken", "thinking_token_budget": "refused"})
        for k, status in (("K3", "green"), ("K4", "n/a"), ("K5", "green"),
                          ("K6", "green")):
            self.assertEqual(report["checks"][k]["status"], status,
                             f"{k}: {report['checks'][k]['note']}")
        self.assertEqual(report["reasoning_field"], "thinking")
        self.assertEqual(code, 0, out)
        bodies = [json.loads(raw) for raw in self.fake.bodies]
        # Native forms only: the API refuses every foreign top-level field.
        for b in bodies:
            for name in ("reasoning", "reasoning_effort", "thinking_token_budget"):
                self.assertNotIn(name, b)
        self.assertIn({"effort": "low"}, [b.get("output_config") for b in bodies])
        tool_body = [b for b in bodies if b.get("tools")][-1]
        self.assertIn("input_schema", tool_body["tools"][0])
        self.assertNotIn("function", tool_body["tools"][0])

    # App id (owner rule 16.10.: every LLM request names the app) ----------
    def test_every_request_carries_the_app_id_on_every_wire(self):
        self.run_tool("fake/strict-all", "--only", "K1,K5")
        self.run_messages("claude-fake-ok", "--only", "K1,K5")
        self.assertEqual(len(self.fake.requests), 4)
        # The referer is the tool's own constant, not a literal here: this
        # module travels with the export, and the audit's R5 reads the project
        # domain as a name hit (OR-programm-tm.AN3.orch-3). Equality stays
        # the check; the constant itself must still be an https origin.
        referer = conformance.APP_HEADERS["HTTP-Referer"]
        self.assertTrue(referer.startswith("https://"), referer)
        for path, headers in self.fake.requests:
            self.assertEqual(headers.get("x-title"), "meclaw-lab conformance", path)
            self.assertEqual(headers.get("http-referer"), referer, path)

    # T4-02 ----------------------------------------------------------------
    def test_latency_probe_times_the_tool_call_on_both_wires(self):
        # n below 20 is refused before any call.
        code, _, out = self.run_tool("fake/tool-picker", "--latency", "19")
        self.assertEqual(code, 2, out)
        self.assertEqual(self.fake.calls + self.fake.gets, 0)
        cases = conformance.TOOL_CASES
        for wire, model, cls in (("chat", "fake/tool-picker", "openrouter"),
                                 ("messages", "claude-fake-tool-picker", "anthropic")):
            with self.subTest(wire=wire):
                fake = fake_provider.FakeProvider()
                self.base = fake.start()
                self.addCleanup(fake.stop)
                record = pathlib.Path(self.tmp.name) / wire
                code, report, out = self.run_tool(
                    model, "--wire", wire, "--only", "K1", "--latency", "20",
                    "--price-in", "1", "--price-out", "5", "--record", str(record),
                    endpoint_class=cls)
                self.assertEqual(code, 0, out)
                lat = report["latency"]
                # The fake misses every fifth call: 4 of 20 answer in text.
                self.assertEqual((lat["wire"], lat["n"], lat["sent"],
                                  lat["tool_calls"], lat["hits"]),
                                 (wire, 20, 20, 16, 16))
                self.assertLessEqual(lat["p50_ms"], lat["p95_ms"])
                self.assertIn("LATENCY", out)
                # Under --only K1 the run still probes `reasoning` once (K3's
                # probe): the switch is sent only where it is proven.
                sent = [json.loads(raw) for raw in fake.bodies]
                probe = (conformance.MESSAGES_NATIVE["reasoning"] if wire == "messages"
                         else {"reasoning": conformance.NEUTRAL["reasoning"]})
                self.assertEqual(sum(1 for b in sent if not b.get("tools")
                                     and probe.items() <= b.items()), 1)
                self.assertEqual(lat["reasoning"], "off")
                self.assertIsNone(lat["switch_note"])
                switch = conformance.REASONING_OFF[wire]
                # The switch is tried once before the run, not in every call.
                self.assertEqual(self.switch_trials(sent, switch), 1)
                bodies = self.latency_bodies(fake)
                self.assertEqual(len(bodies), 20)
                for i, b in enumerate(bodies):
                    self.assertNotIn("stream", b)
                    self.assertGreaterEqual(len(json.dumps(b)),
                                            conformance.WINDOW_TOKENS * 3)
                    self.assertEqual(b["messages"][-1]["content"],
                                     cases[i % len(cases)][0])
                    self.assertLessEqual(switch.items(), b.items())
                    for name in conformance.SAMPLING_PARAMS:
                        if name not in switch:
                            self.assertNotIn(name, b)
                slug = model.replace("/", "__")
                saved = json.loads((record / slug / "latency.json").read_text())
                self.assertEqual(saved["hits"], 16)
                self.assertEqual(saved["reasoning"], "off")

    @staticmethod
    def switch_trials(sent, switch):
        return sum(1 for b in sent if not b.get("tools") and switch.items() <= b.items())

    @staticmethod
    def latency_bodies(fake):
        bodies = [json.loads(raw) for raw in fake.bodies]
        return [b for b in bodies
                if len(b.get("tools") or []) == len(conformance.LATENCY_TOOLS)]

    def test_latency_reports_reasoning_off_only_when_the_switch_was_sent(self):
        # Re-check AN3 I1: a model that reasons by default must not be timed
        # with reasoning and filed as "off" -- T4-03 picks the quick tier from
        # it (R-WP-2). Not switchable (K3 refused `reasoning`) or unproven
        # (a local 200) sends no switch and says so.
        for wire, model, cls, label in (
                ("chat", "fake/tool-picker-no-reasoning", "openrouter", "not_switchable"),
                ("chat", "fake/tool-picker", "local", "unknown"),
                ("messages", "claude-fake-tool-picker", "local", "unknown")):
            with self.subTest(wire=wire, cls=cls):
                fake = fake_provider.FakeProvider()
                self.base = fake.start()
                self.addCleanup(fake.stop)
                code, report, out = self.run_tool(
                    model, "--wire", wire, "--only", "K1", "--latency", "20",
                    "--price-in", "1", "--price-out", "5", endpoint_class=cls)
                self.assertEqual(code, 0, out)
                self.assertEqual(report["latency"]["reasoning"], label)
                bodies = self.latency_bodies(fake)
                self.assertEqual(len(bodies), 20)
                for b in bodies:
                    for field in ("reasoning", "thinking", "reasoning_effort"):
                        self.assertNotIn(field, b)

    def test_latency_on_a_model_that_must_reason_measures_at_low_effort(self):
        # T4-03 (16.10.): gemini-3.8-flash takes `reasoning` in K3 but refuses
        # the switch with 400 "Reasoning is mandatory" -- every timed call was
        # an error and the probe still said `off`. The switch is tried once;
        # refused, the run measures at K5's low form and says `mandatory_low`.
        fake = fake_provider.FakeProvider()
        self.base = fake.start()
        self.addCleanup(fake.stop)
        code, report, out = self.run_tool(
            "fake/reasoning-mandatory", "--only", "K1", "--latency", "20",
            "--price-in", "1", "--price-out", "5", endpoint_class="openrouter")
        self.assertEqual(code, 0, out)
        lat = report["latency"]
        self.assertEqual(lat["reasoning"], "mandatory_low")
        self.assertIn("HTTP 400", lat["switch_note"])
        self.assertEqual(lat["tool_calls"], 16)
        sent = [json.loads(raw) for raw in fake.bodies]
        self.assertEqual(self.switch_trials(sent, conformance.REASONING_OFF["chat"]), 1)
        bodies = self.latency_bodies(fake)
        self.assertEqual(len(bodies), 20)
        self.assertTrue(all(b.get("reasoning") == {"effort": "low"} for b in bodies))

    def test_latency_downgrades_only_on_a_refusal_of_the_switch(self):
        # Delta check AN3-FIX I1: a 503 on the switch's trial is no refusal --
        # the run keeps the switch K3 proved and says `off`, the trial's
        # answer stands in `switch_note` (OR-programm-tm.AN3-FIX.orch-1).
        fake = fake_provider.FakeProvider()
        self.base = fake.start()
        self.addCleanup(fake.stop)
        code, report, out = self.run_tool(
            "fake/reasoning-switch-503", "--only", "K1", "--latency", "20",
            "--price-in", "1", "--price-out", "5", endpoint_class="openrouter")
        self.assertEqual(code, 0, out)
        lat = report["latency"]
        self.assertEqual(lat["reasoning"], "off")
        self.assertTrue(lat["switch_note"].startswith("HTTP 503"), lat["switch_note"])
        self.assertEqual(lat["tool_calls"], 16)
        switch = conformance.REASONING_OFF["chat"]
        sent = [json.loads(raw) for raw in fake.bodies]
        self.assertEqual(self.switch_trials(sent, switch), 1)
        bodies = self.latency_bodies(fake)
        self.assertEqual(len(bodies), 20)
        self.assertTrue(all(b.get("reasoning") == switch["reasoning"] for b in bodies))

    def test_latency_sends_no_unproven_low_form_on_the_messages_wire(self):
        # Delta check AN3-FIX M1: on the Messages wire the low form is
        # `reasoning_effort`, which --only K1 never proved. Refused switch and
        # no proven low form: nothing is sent, the label says the model was
        # timed at its default reasoning.
        fake = fake_provider.FakeProvider()
        self.base = fake.start()
        self.addCleanup(fake.stop)
        code, report, out = self.run_tool(
            "claude-fake-reasoning-mandatory", "--wire", "messages", "--only", "K1",
            "--latency", "20", "--price-in", "1", "--price-out", "5",
            endpoint_class="anthropic")
        self.assertEqual(code, 0, out)
        lat = report["latency"]
        self.assertEqual(lat["reasoning"], "mandatory_default")
        self.assertIn("HTTP 400", lat["switch_note"])
        self.assertEqual(lat["tool_calls"], 16)
        bodies = self.latency_bodies(fake)
        self.assertEqual(len(bodies), 20)
        for b in bodies:
            for field in ("thinking", "output_config"):
                self.assertNotIn(field, b)


def tearDownModule():
    store_isolation.restore()


if __name__ == "__main__":
    unittest.main()
