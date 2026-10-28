"""Synthetic offline positive/negative controls; no device, subprocess or network APIs."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("acc_preflight", Path(__file__).with_name("verify.py"))
preflight = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preflight)


def fixture():
    cases = [{"id": name, "windows": ["W1"], "observers": ["synthetic-observer"],
              "deviceStatus": "NOT_EXECUTED", "deviceJudge": "NOT_IMPLEMENTED"}
             for name in preflight.ORIGINAL_IDS]
    cases.append({"id": "WIN-EXIT-01", "windows": ["W1"], "deviceStatus": "NOT_EXECUTED",
                  "deviceJudge": "NOT_IMPLEMENTED", "requiredOriginals": ["synthetic-original"],
                  "scenarios": [{"id": name, "action": "synthetic action", "expected": ["synthetic criterion"],
                                 **({"control": "negative"} if name in
                                    ("stop-then-shutdown", "exit-then-shutdown") else {})}
                                for name in preflight.WIN_SCENARIOS]})
    return {"schema": "acc01-case-catalogue-v1", "sourcePlanSha256": "a" * 64,
            "originalCaseCount": 45, "additionalSpecialCount": 1, "execution": "UNAVAILABLE",
            "cases": cases, "windowGate": {"deviceActions": "NOT_AUTHORIZED",
                                          "networkChanges": "NOT_AUTHORIZED", "retryMax": 0}}


class ScopeTests(unittest.TestCase):
    def reject(self, mutate):
        value = fixture()
        mutate(value)
        with self.assertRaises(preflight.InvalidCatalogue):
            preflight.check(value)

    def test_positive_control_stays_unobserved(self):
        report = preflight.check(fixture())
        self.assertEqual(report["scopeCheck"], "MATCH")
        self.assertEqual(report["semanticValidation"], "NOT_PERFORMED")
        for key in ("ACC01Complete", "readyForVAL03", "devicePassed"):
            self.assertIs(report[key], False)
        self.assertEqual(len(report["originalCaseIds"]), 45)
        self.assertEqual(len(report["windowsScenarioIds"]), 7)

    def test_every_original_omission_rejected(self):
        for name in preflight.ORIGINAL_IDS:
            with self.subTest(name=name):
                self.reject(lambda d: d["cases"].__setitem__(slice(None),
                            [row for row in d["cases"] if row["id"] != name]))

    def test_every_windows_scenario_omission_rejected(self):
        for name in preflight.WIN_SCENARIOS:
            with self.subTest(name=name):
                self.reject(lambda d: d["cases"][-1]["scenarios"].__setitem__(slice(None),
                            [row for row in d["cases"][-1]["scenarios"] if row["id"] != name]))

    def test_duplicate_and_unknown_cases_rejected(self):
        self.reject(lambda d: d["cases"].append(copy.deepcopy(d["cases"][0])))
        self.reject(lambda d: d["cases"][0].update(id="V1-00"))

    def test_duplicate_and_unknown_scenarios_rejected(self):
        self.reject(lambda d: d["cases"][-1]["scenarios"].append(d["cases"][-1]["scenarios"][0]))
        self.reject(lambda d: d["cases"][-1]["scenarios"][0].update(id="ordinary-exit"))

    def test_negative_controls_cannot_be_relabelled(self):
        self.reject(lambda d: d["cases"][-1]["scenarios"][1].pop("control"))
        self.reject(lambda d: d["cases"][-1]["scenarios"][0].update(control="negative"))

    def test_acceptance_claims_rejected_at_all_consumed_levels(self):
        for field in ("devicePassed", "claims", "assessment"):
            for level in ("catalogue", "case", "scenario"):
                with self.subTest(field=field, level=level):
                    def mutate(d):
                        target = d if level == "catalogue" else (d["cases"][0] if level == "case"
                                 else d["cases"][-1]["scenarios"][0])
                        target[field] = True
                    self.reject(mutate)
        self.reject(lambda d: d["cases"][0].update(deviceStatus="PASS"))
        self.reject(lambda d: d["cases"][0].update(deviceJudge="IMPLEMENTED"))
        self.reject(lambda d: d.update(readyForVAL03=True))

    def test_window_authorization_and_execution_cannot_be_granted(self):
        self.reject(lambda d: d.update(execution="AVAILABLE"))
        self.reject(lambda d: d["windowGate"].update(deviceActions="AUTHORIZED"))
        self.reject(lambda d: d["windowGate"].update(networkChanges="AUTHORIZED"))
        self.reject(lambda d: d["windowGate"].update(retryMax=False))
        self.reject(lambda d: d["cases"][-1].update(windows=["W2"]))

    def test_counts_types_and_observer_mapping_required(self):
        self.reject(lambda d: d.update(originalCaseCount=44))
        self.reject(lambda d: d.update(additionalSpecialCount=True))
        self.reject(lambda d: d["cases"][0].update(windows=["W1", "W1"]))
        self.reject(lambda d: d["cases"][0].update(windows=["W5"]))
        self.reject(lambda d: d["cases"][0].update(observers=[]))
        self.reject(lambda d: d["cases"][-1].update(requiredOriginals=[]))
        self.reject(lambda d: d["cases"][-1]["scenarios"][0].update(expected=[]))

    def test_malformed_shapes_rejected(self):
        for value in (None, [], {"schema": "future"}):
            with self.subTest(value=value), self.assertRaises(preflight.InvalidCatalogue):
                preflight.check(value)
        self.reject(lambda d: d.update(cases=[None]))

    def test_duplicate_json_nonfinite_and_bounded_input(self):
        for raw in (b'{"x":1,"x":2}', b'{"x":NaN}', b'{"x":Infinity}', b' ' * (1024 * 1024 + 1)):
            with self.subTest(raw=raw[:30]), self.assertRaises(preflight.InvalidCatalogue):
                preflight.load(raw)
        self.assertEqual(preflight.check(preflight.load(json.dumps(fixture()).encode()))["scopeCheck"], "MATCH")

    def test_b3b_preserves_three_platforms_four_distinct_actions(self):
        cases = preflight.supplements()["B3b"]["cases"]
        self.assertEqual(len(cases), 12)
        self.assertEqual({(r["platform"], r["action"]) for r in cases},
                         {(p, a) for p in ("windows", "linux", "macos")
                          for a in ("install", "upgrade", "start", "stop")})
        self.assertTrue(all(r["deviceStatus"] == "NOT_EXECUTED" for r in cases))

    def test_coex_table_expands_to_seventeen_without_dropping_cells(self):
        coex = preflight.supplements()["COEX-01"]
        cells = {(r["object"], r["platform"], r["mode"]) for r in coex["matrix"]}
        self.assertEqual(len(cells), 17)
        self.assertEqual(coex["expandedMatrixCount"], 17)
        self.assertEqual(coex["declaredMatrixCount"], 15)
        self.assertEqual(coex["countStatus"], "SOURCE_CONFLICT_REQUIRES_OWNER_CONFIRMATION")
        self.assertNotIn(("virtualization", "macos", "tun"), cells)
        self.assertIn(("openvpn-full", "macos", "system-proxy"), cells)
        self.assertIn(("openvpn-split", "windows", "system-proxy"), cells)
        extra = {r["object"] for r in coex["additionalObligations"]}
        self.assertIn("windows-l2tp", extra)
        self.assertIn("anyconnect", extra)


if __name__ == "__main__":
    unittest.main()
