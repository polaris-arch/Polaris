#!/usr/bin/env python3
"""Offline preparation and consistency checks. No transition or native observer exists here."""

import argparse
import copy
import hashlib
import json
import platform
import re
import sys
from pathlib import Path

SCHEMA = 1
MAX_JSON_BYTES = 1024 * 1024
ROLES = ("app", "helper", "core")
PRODUCERS = ("main", "login", "temp", "check")
SHA = re.compile(r"[0-9a-f]{64}\Z")
COMMIT = re.compile(r"[0-9a-f]{40}\Z")
NONCE = re.compile(r"[0-9a-f]{32,64}\Z")
STATES = ("OBSERVED", "UNKNOWN", "PENDING", "NOT_EXECUTED")
MISSING = (
    "device-transition-executor", "trusted-native-receipt-exporter",
    "four-producer-drain-observer", "helper-tail-observer",
    "independent-postcheck-adapter", "restore-executor", "artifact-build-provenance",
)

# Actions are descriptions for one later authorized window, never executable commands.
CASES = (
    ("baseline", [], "independent-device-observer", False),
    ("old-helper-upgrade", ["main"], "helper-compatibility-observer", True),
    ("main-direct-stop-restart", ["main"], "app-owned-child-native-wait", True),
    ("main-helper-stop-restart", ["main"], "helper-exact-birth-stop", True),
    ("login-stop-retry", ["login"], "app-owned-child-native-wait", True),
    ("temp-stop-retry", ["temp"], "app-owned-child-native-wait", True),
    ("check-cancel-retry", ["check"], "app-owned-child-native-wait", True),
    ("quit-all-four", list(PRODUCERS), "four-producer-drain", True),
    ("app-restart-all-four", list(PRODUCERS), "four-producer-drain", True),
    ("update-after-os-exit", list(PRODUCERS), "installer-old-app-os-exit", True),
    ("negative-wait-error", list(PRODUCERS), "controlled-fault-observer", True),
    ("negative-cancel-and-stale-birth", list(PRODUCERS), "controlled-fault-observer", True),
    ("negative-ack-only", ["main"], "controlled-fault-observer", True),
    ("restore", [], "independent-device-observer", True),
    ("independent-postcheck", [], "independent-device-observer", False),
)
CAPABILITIES = {
    "transitionExecutor": "NOT_IMPLEMENTED",
    "trustedNativeObserver": "NOT_IMPLEMENTED",
    "helperProtocolObserved": "UNKNOWN",
    "fourProducerDrainObserved": "UNKNOWN",
    "helperTailObserved": "UNKNOWN",
    "restoreObserved": "UNKNOWN",
    "independentPostcheckObserved": "UNKNOWN",
    "artifactBuildProvenance": "UNVERIFIED",
}
WINDOW = {
    "authorization": "EXISTING_SESSION_SCOPE_ONLY",
    "scopes": ["read-only-preflight", "matched-app-helper-core-update", "controlled-stop-restart", "quit-app-restart-update", "controlled-negative-tests", "restore", "independent-postcheck"],
    "changeSet": ["one matched candidate deployment within existing authorized window/scope; preparation grants no additional authority", "controlled lifecycle transitions within existing authorized scope", "fault injection remains a window dependency: executor not implemented; use only within authorized scope"],
    "restore": ["retain original artifact hashes and supported helper pairing", "record nonsecret configuration/state/log/path roles and permissions; do not collect credentials or full configuration", "restore the selected baseline through a reviewed executor within authorized scope", "independent after-restore comparison; Unknown stays blocked"],
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def exact(value, keys, label):
    require(isinstance(value, dict) and set(value) == set(keys), f"{label}: missing/extra fields")


def text(value, label):
    require(isinstance(value, str) and 0 < len(value) <= 512, f"{label}: nonempty text required")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def file_digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "duplicate JSON field")
        result[key] = value
    return result


def load(path):
    with Path(path).open("rb") as stream:
        data = stream.read(MAX_JSON_BYTES + 1)
    require(len(data) <= MAX_JSON_BYTES, "JSON input exceeds 1 MiB limit")
    return json.loads(data.decode("utf-8-sig"), object_pairs_hook=unique_pairs)


def validate_binding(binding):
    exact(binding, ("candidateSha", "deviceId", "nonce", "platform"), "binding")
    require(isinstance(binding["candidateSha"], str) and COMMIT.fullmatch(binding["candidateSha"]), "candidate SHA required")
    require(isinstance(binding["deviceId"], str) and re.fullmatch(r"[A-Za-z0-9._-]{1,96}", binding["deviceId"]), "nonsecret inventory device ID required")
    require(isinstance(binding["nonce"], str) and NONCE.fullmatch(binding["nonce"]), "fresh 128–256 bit nonce required")
    require(binding["platform"] in ("macos", "linux", "windows"), "unsupported platform")


def validate_artifacts(artifacts):
    exact(artifacts, ROLES, "actual artifacts")
    for role, item in artifacts.items():
        exact(item, ("sha256", "bytes"), role)
        require(isinstance(item["sha256"], str) and SHA.fullmatch(item["sha256"]), f"{role} SHA-256 required")
        require(type(item["bytes"]) is int and item["bytes"] > 0, f"{role} nonempty binary required")


def validate_versions(versions):
    exact(versions, ("app", "helper", "core", "source"), "versions")
    for role in ROLES:
        require(isinstance(versions[role], str) and re.fullmatch(r"[A-Za-z0-9._+-]{1,96}", versions[role]), f"{role}: nonsecret version label required")
    require(versions["source"] == "operator-declared-unverified", "version labels cannot attest runtime capability")


def validate_preflight(report):
    exact(report, ("schemaVersion", "kind", "binding", "artifacts", "versions", "host", "readOnly", "capabilities", "claims"), "preflight")
    require(report["schemaVersion"] == SCHEMA and report["kind"] == "READ_ONLY_PREFLIGHT", "wrong preflight schema")
    validate_binding(report["binding"])
    validate_artifacts(report["artifacts"])
    validate_versions(report["versions"])
    exact(report["host"], ("system", "release", "architecture", "hostnameSha256"), "host")
    for key, value in report["host"].items():
        text(value, f"host {key}")
    expected_system = {"macos": "Darwin", "linux": "Linux", "windows": "Windows"}[report["binding"]["platform"]]
    require(report["host"]["system"] == expected_system, "device platform differs")
    require(SHA.fullmatch(report["host"]["hostnameSha256"]), "host fingerprint hash required")
    require(report["readOnly"] is True and report["claims"] == [], "preflight cannot grant process/resource claims")
    require(report["capabilities"] == CAPABILITIES, "preflight cannot infer unsupported capabilities")
    return report


def preflight(args):
    actual = {"Darwin": "macos", "Linux": "linux", "Windows": "windows"}.get(platform.system())
    require(actual == args.platform, "preflight wrapper does not match this OS")
    files = {}
    for role in ROLES:
        path = Path(getattr(args, role)).resolve(strict=True)
        require(path.is_file() and path.stat().st_size > 0, f"{role}: installed binary file required")
        # Selecting a file and hashing bytes does not establish which binary a process ran.
        before = path.stat()
        files[role] = {"sha256": file_digest(path), "bytes": before.st_size}
        after = path.stat()
        require((before.st_ino, before.st_size, before.st_mtime_ns) == (after.st_ino, after.st_size, after.st_mtime_ns), f"{role}: selected artifact changed during read")
    return validate_preflight({
        "schemaVersion": SCHEMA, "kind": "READ_ONLY_PREFLIGHT",
        "binding": {"candidateSha": args.candidate, "deviceId": args.device_id, "nonce": args.nonce, "platform": actual},
        "artifacts": files,
        "versions": {**{role: getattr(args, role + "_version") for role in ROLES}, "source": "operator-declared-unverified"},
        "host": {"system": platform.system(), "release": platform.release(), "architecture": platform.machine(), "hostnameSha256": hashlib.sha256(platform.node().encode("utf-8")).hexdigest()},
        "readOnly": True, "capabilities": dict(CAPABILITIES), "claims": [],
    })


def phases():
    return [{"id": name, "producers": list(producers), "requiredSource": source, "requiresAuthorizedWindow": mutation}
            for name, producers, source, mutation in CASES]


def prepare(report):
    validate_preflight(report)
    body = {
        "schemaVersion": SCHEMA, "kind": "PREPARED_ONLY_PLAN", "preparedOnly": True,
        "binding": copy.deepcopy(report["binding"]), "artifacts": copy.deepcopy(report["artifacts"]), "versions": dict(report["versions"]),
        "preflightSha256": digest(report), "phases": phases(), "capabilities": dict(CAPABILITIES), "claims": [],
        "windowRequest": copy.deepcopy(WINDOW),
    }
    return {**body, "planSha256": digest(body)}


def validate_plan(plan):
    exact(plan, ("schemaVersion", "kind", "preparedOnly", "binding", "artifacts", "versions", "preflightSha256", "phases", "capabilities", "claims", "windowRequest", "planSha256"), "plan")
    require(plan["schemaVersion"] == SCHEMA and plan["kind"] == "PREPARED_ONLY_PLAN" and plan["preparedOnly"] is True, "plan is prepared-only")
    validate_binding(plan["binding"])
    validate_artifacts(plan["artifacts"])
    validate_versions(plan["versions"])
    require(isinstance(plan["preflightSha256"], str) and SHA.fullmatch(plan["preflightSha256"]), "preflight digest missing")
    require(plan["phases"] == phases() and plan["capabilities"] == CAPABILITIES and plan["claims"] == [], "required scope/capability/claim matrix altered")
    require(plan["windowRequest"] == WINDOW, "preparation cannot grant or widen window authority")
    body = {key: value for key, value in plan.items() if key != "planSha256"}
    require(plan["planSha256"] == digest(body), "plan digest differs")
    return plan


def checked_file(root, reference):
    exact(reference, ("path", "sha256", "bytes"), "evidence artifact")
    text(reference["path"], "evidence path")
    require(isinstance(reference["sha256"], str) and SHA.fullmatch(reference["sha256"]), "evidence SHA-256 required")
    require(type(reference["bytes"]) is int and reference["bytes"] > 0, "nonempty evidence required")
    relative = Path(reference["path"])
    require(not relative.is_absolute() and ".." not in relative.parts, "evidence must stay inside supplied bundle")
    path = root / relative
    require(not any(part.is_symlink() for part in (path, *path.parents) if part != root.parent), "symlink evidence refused")
    require(path.resolve(strict=True).is_relative_to(root.resolve(strict=True)), "evidence escaped bundle")
    require(path.is_file() and type(reference["bytes"]) is int and path.stat().st_size == reference["bytes"], "evidence size differs")
    require(reference["sha256"] == file_digest(path), "evidence hash differs")
    return path


def verify_witness(plan, witness, root):
    validate_plan(plan)
    exact(witness, ("schemaVersion", "kind", "synthetic", "binding", "planSha256", "artifacts", "versions", "records", "claims"), "witness")
    require(witness["schemaVersion"] == SCHEMA and witness["kind"] == "OFFLINE_EVIDENCE_BUNDLE", "wrong witness schema")
    require(witness["synthetic"] is False, "synthetic fixture cannot enter device evidence review")
    require(witness["binding"] == plan["binding"] and witness["planSha256"] == plan["planSha256"], "old/wrong candidate, device, nonce or plan")
    require(witness["artifacts"] == plan["artifacts"] and witness["versions"] == plan["versions"], "App/helper/core identity differs")
    require(witness["claims"] == [], "native/local observations cannot grant NoOwner/managed/release")
    require(isinstance(witness["records"], list), "records array required")
    records = witness["records"]
    require(len(records) == len(CASES) and all(isinstance(item, dict) for item in records), "exact case record count/types required")
    require([item.get("caseId") for item in records] == [item[0] for item in CASES], "missing/duplicate/out-of-order case receipts")
    retained = []
    for phase, record in zip(plan["phases"], witness["records"]):
        exact(record, ("caseId", "state", "capture"), "case record")
        require(record["state"] in STATES, "unknown result type")
        if record["state"] != "OBSERVED":
            retained.append({"caseId": record["caseId"], "state": record["state"]})
        if record["capture"] is None:
            require(record["state"] != "OBSERVED", "observed case lacks original capture")
            continue
        capture = load(checked_file(Path(root), record["capture"]))
        exact(capture, ("synthetic", "binding", "planSha256", "caseId", "state", "source", "identities", "original"), "capture envelope")
        require(capture["synthetic"] is False, "synthetic capture refused")
        require(capture["binding"] == plan["binding"] and capture["planSha256"] == plan["planSha256"] and capture["caseId"] == record["caseId"], "old/wrong capture binding")
        require(capture["state"] == record["state"], "original Unknown/Pending cannot be reclassified")
        require(capture["source"] == phase["requiredSource"], "PID list/Stop ACK/other source cannot replace required observer")
        require(isinstance(capture["identities"], dict) and set(capture["identities"]) == set(phase["producers"]), "same-generation producer identity missing")
        for identity in capture["identities"].values():
            exact(identity, ("generation", "nativeIdentity"), "producer identity")
            for value in identity.values():
                text(value, "opaque native generation/identity")
        checked_file(Path(root), capture["original"])
    return {
        "verdict": "NOT_READY", "witnessConsistent": True, "preparedOnly": True,
        "planSha256": plan["planSha256"], "retainedStates": retained,
        "missingCapabilities": list(MISSING), "claims": [],
        "note": "Files/bindings/shapes match only; no trusted native observer, device execution acceptance or global resource proof is implemented.",
    }


class RedactedArgumentParser(argparse.ArgumentParser):
    def error(self, message):
        # Do not echo an unrecognized argument that might contain a path or credential.
        self.exit(1, "EVIDENCE_REJECTED: invalid arguments; use --help for required fields\n")


def main(argv=None):
    parser = RedactedArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)
    prepare_parser = modes.add_parser("prepare")
    prepare_parser.add_argument("preflight")
    verify_parser = modes.add_parser("verify")
    verify_parser.add_argument("plan")
    verify_parser.add_argument("witness")
    verify_parser.add_argument("--bundle", required=True)
    observation = modes.add_parser("preflight")
    observation.add_argument("--platform", choices=("macos", "linux", "windows"), required=True)
    observation.add_argument("--candidate", required=True)
    observation.add_argument("--device-id", required=True)
    observation.add_argument("--nonce", required=True)
    for role in ROLES:
        observation.add_argument("--" + role, required=True)
        observation.add_argument("--" + role + "-version", required=True)
    args = parser.parse_args(argv)
    if args.mode == "preflight":
        result, code = preflight(args), 0
    elif args.mode == "prepare":
        result, code = prepare(load(args.preflight)), 2
    else:
        result, code = verify_witness(load(args.plan), load(args.witness), args.bundle), 2
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return code


if __name__ == "__main__":
    try:
        sys.exit(main())
    except OSError as error:
        # OS exception filenames may disclose the operator's absolute artifact paths.
        print(f"EVIDENCE_REJECTED: artifact IO failed ({type(error).__name__})", file=sys.stderr)
        sys.exit(1)
    except (UnicodeError, json.JSONDecodeError):
        print("EVIDENCE_REJECTED: invalid UTF-8 JSON", file=sys.stderr)
        sys.exit(1)
    except (ValueError, TypeError, KeyError) as error:
        print(f"EVIDENCE_REJECTED: {error}", file=sys.stderr)
        sys.exit(1)
