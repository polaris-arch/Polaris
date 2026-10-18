#!/usr/bin/env python3
"""Read an ACC preparation catalogue; emit scope checks, never device acceptance.

Usage: python3 -B scripts/acc01-preflight/verify.py LOCAL_CASEPLAN_JSON
Exit 0 = catalogue structure matches; exit 2 = invalid input. Neither permits execution.
Only JSON is read. No ignored acceptance module is imported or executed.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

ORIGINAL_IDS = tuple(f"{prefix}-{number:02d}" for prefix, count in
                     (("V1", 9), ("V3", 30), ("V4", 6)) for number in range(1, count + 1))
WIN_SCENARIOS = ("direct-shutdown", "stop-then-shutdown", "exit-then-shutdown",
                 "restart", "logoff", "cancel", "boot-without-app")
PLATFORMS = ("windows", "linux", "macos")
COEX_MATRIX = (
    ("openvpn-split", "windows", ("tun", "system-proxy")),
    ("openvpn-split", "linux", ("tun",)),
    ("openvpn-split", "macos", ("tun", "system-proxy")),
    ("openvpn-full", "windows", ("tun",)),
    ("openvpn-full", "linux", ("tun",)),
    ("openvpn-full", "macos", ("tun", "system-proxy")),
    ("official-tailscale", "windows", ("tun",)),
    ("official-tailscale", "linux", ("tun",)),
    ("official-tailscale", "macos", ("tun",)),
    ("clash-verge-rev", "windows", ("tun",)),
    ("clash-verge-rev", "linux", ("tun",)),
    ("clash-verge-rev", "macos", ("tun",)),
    ("virtualization", "windows", ("tun",)),
    ("virtualization", "linux", ("tun",)),
)


class InvalidCatalogue(ValueError):
    pass


def need(condition, code):
    if not condition:
        raise InvalidCatalogue(code)


def unique_rows(value, code):
    need(type(value) is list and value, code)
    need(all(type(row) is dict and type(row.get("id")) is str for row in value), code)
    ids = [row["id"] for row in value]
    need(len(set(ids)) == len(ids), code)
    return {row["id"]: row for row in value}


def strings(value, code):
    need(type(value) is list and value and
         all(type(item) is str and item.strip() for item in value), code)
    need(len(value) == len(set(value)), code)


def supplements():
    """Derived obligations, outside the original 45; no new authoritative task IDs."""
    return {
        "B3b": {
            "source": "polaris-helper-core-install-root-cause-fix-2026-10-08.md#后续裁定/B3b",
            "cases": [{"platform": platform, "runningUser": "A", "actingUser": "B",
                       "action": action, "deviceStatus": "NOT_EXECUTED"}
                      for platform in PLATFORMS for action in ("install", "upgrade", "start", "stop")],
            "contract": ["authenticated slot owner; stop/status/restart only owner or administrator",
                         "other-user start reports occupied by another user, not Already",
                         "macOS/Windows resolve token and config by authenticated caller",
                         "installation/upgrade results must be observed while A remains running"],
            "blockers": ["B3 then B3b implementation and frozen candidate",
                         "two authenticated OS users and separate administrator controls",
                         "independent process/IPC/config observations and approved device window"],
        },
        "COEX-01": {
            "source": "polaris-coex01-multi-vpn-coexistence-governance-2026-10-08.md#9.2-9.3",
            "matrix": [{"object": obj, "platform": platform, "mode": mode,
                        "deviceStatus": "NOT_EXECUTED"}
                       for obj, platform, modes in COEX_MATRIX for mode in modes],
            "declaredMatrixCount": 15,
            "expandedMatrixCount": sum(len(modes) for _, _, modes in COEX_MATRIX),
            "countStatus": "SOURCE_CONFLICT_REQUIRES_OWNER_CONFIRMATION",
            "additionalObligations": [
                {"object": "windows-l2tp", "scope": "replacement default route; extra to matrix"},
                {"object": "anyconnect", "scope": "unified App acceptance only; no early environment setup"},
                {"object": "openvpn-def1-started-after-polaris", "scope": "all three desktop platforms; 9.2"},
                {"object": "other-tun-started-after-polaris", "scope": "notification and stop entry; 9.2"},
            ],
            "uncovered": ["EasyConnect", "aTrust", "GlobalProtect", "other enterprise clients", "Surge"],
            "blockers": ["COEX source batches and runtime observer contracts",
                         "17-vs-15 source count reconciliation by COEX owner",
                         "controlled peers, paired route/DNS/capture originals and approved windows",
                         "AnyConnect environment and credentials at unified acceptance"],
        },
    }


def check(catalogue):
    need(type(catalogue) is dict, "CatalogueObjectRequired")
    need(catalogue.get("schema") == "acc01-case-catalogue-v1", "UnknownSchema")
    need(type(catalogue.get("originalCaseCount")) is int and
         catalogue["originalCaseCount"] == 45, "OriginalCountMismatch")
    need(type(catalogue.get("additionalSpecialCount")) is int and
         catalogue["additionalSpecialCount"] == 1, "SpecialCountMismatch")
    need(catalogue.get("execution") == "UNAVAILABLE", "ExecutionClaimForbidden")
    need(type(catalogue.get("sourcePlanSha256")) is str and
         re.fullmatch(r"[0-9a-f]{64}", catalogue["sourcePlanSha256"]), "SourceDigestRequired")
    # A structure checker cannot consume a witness or grant device readiness.
    need(not ({"devicePassed", "ACC01Complete", "readyForVAL03", "claims", "assessment"} &
              catalogue.keys()), "AcceptanceClaimForbidden")
    cases = unique_rows(catalogue.get("cases"), "InvalidOrDuplicateCases")
    need(set(cases) == set(ORIGINAL_IDS) | {"WIN-EXIT-01"}, "CaseScopeMismatch")
    for row in cases.values():
        need(row.get("deviceStatus") == "NOT_EXECUTED" and
             row.get("deviceJudge") == "NOT_IMPLEMENTED", "DeviceClaimForbidden")
        need(not ({"devicePassed", "claims", "assessment", "result", "witness"} & row.keys()),
             "AcceptanceClaimForbidden")
        strings(row.get("windows"), "InvalidWindows")
        need(set(row["windows"]) <= {"W1", "W2", "W3", "W4"}, "InvalidWindows")
        if row["id"] != "WIN-EXIT-01":
            strings(row.get("observers"), "MissingObserverMapping")
    win = cases["WIN-EXIT-01"]
    need(win["windows"] == ["W1"], "WindowsSpecialOutsideW1")
    scenarios = unique_rows(win.get("scenarios"), "InvalidOrDuplicateScenarios")
    need(set(scenarios) == set(WIN_SCENARIOS), "WindowsScenarioScopeMismatch")
    for name, scenario in scenarios.items():
        strings(scenario.get("expected"), "ScenarioExpectedRequired")
        need(type(scenario.get("action")) is str and scenario["action"].strip(), "ScenarioActionRequired")
        need(not ({"devicePassed", "claims", "assessment", "result", "witness"} & scenario.keys()),
             "AcceptanceClaimForbidden")
        expected_control = "negative" if name in ("stop-then-shutdown", "exit-then-shutdown") else None
        need(scenario.get("control") == expected_control, "WindowsControlMismatch")
    strings(win.get("requiredOriginals"), "WindowsOriginalsRequired")
    gate = catalogue.get("windowGate")
    need(type(gate) is dict and gate.get("deviceActions") == "NOT_AUTHORIZED" and
         gate.get("networkChanges") == "NOT_AUTHORIZED" and
         type(gate.get("retryMax")) is int and gate["retryMax"] == 0, "WindowGateMismatch")
    return {
        "schema": "acc01-scope-preflight-v1", "scopeCheck": "MATCH",
        "semanticValidation": "NOT_PERFORMED", "execution": "UNAVAILABLE",
        "ACC01Complete": False, "readyForVAL03": False, "devicePassed": False,
        "originalCaseCount": len(ORIGINAL_IDS), "originalCaseIds": list(ORIGINAL_IDS),
        "additionalSpecialCount": 1, "windowsScenarioIds": list(WIN_SCENARIOS),
        "supplementalMappings": supplements(),
        "unfinishedObservers": ["S3", "S6", "S8", "S9", "S13", "S14"],
        "existingToolLimits": {
            "S1": "private copies and fixed synthetic restore only",
            "S2": "offline historical normalization/comparison; no live collector",
            "S10": "candidate/original binding; no device issuer or executor",
            "S11": "known credential scan in verified copy only",
            "M1": "execution remains unfrozen", "M2": "pure package contract only",
        },
        "limitations": ["scope structure only; textual criteria and source authenticity unverified",
                        "supplemental mappings are derived planning dimensions, not device cases passed",
                        "current-source completion of Q2 does not provide device observations",
                        "Q6 belongs to the separate production writer; not changed here",
                        "frozen candidate, VAL-01/VAL-02 evidence and approved windows still required"],
    }


def no_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        need(key not in result, "DuplicateJsonKey")
        result[key] = value
    return result


def load(raw):
    need(len(raw) <= 1024 * 1024, "CatalogueTooLarge")
    return json.loads(raw, object_pairs_hook=no_duplicate_keys,
                      parse_constant=reject_nonfinite)


def reject_nonfinite(_):
    raise InvalidCatalogue("NonFiniteJson")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("catalogue", type=Path)
    args = parser.parse_args()
    try:
        with args.catalogue.open("rb") as stream:
            raw = stream.read(1024 * 1024 + 1)
        report = check(load(raw))
        report["catalogueSha256"] = hashlib.sha256(raw).hexdigest()
    except (OSError, ValueError, TypeError, RecursionError) as error:
        # Do not echo paths or raw input. Invalid input cannot yield scope MATCH.
        print(json.dumps({"schema": "acc01-scope-preflight-v1", "scopeCheck": "INVALID",
                          "reason": str(error) if isinstance(error, InvalidCatalogue) else "InvalidInput",
                          "execution": "UNAVAILABLE", "ACC01Complete": False,
                          "readyForVAL03": False, "devicePassed": False}))
        return 2
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
