#!/usr/bin/env python3
"""Replay one pinned core/dependency graph for desktop and Android consumers.

Only fresh build checkouts are mutated. Local module overrides are explicit,
relative, patch-locked, and included in the source receipt; module cache is never
modified. Call provision() from a platform builder or use the JSON CLI.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def run(args, *, cwd=None, env=None, raw=False):
    result = subprocess.run(args, cwd=cwd, env=env, check=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return result.stdout if raw else result.stdout.decode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def pinned_commit(repository, commit):
    require(re.fullmatch(r"[0-9a-f]{40}", commit), "invalid source commit")
    require(run(["git", "-C", str(repository), "rev-parse", commit + "^{commit}"]).strip() == commit,
            "pinned source commit missing from repository")
    return run(["git", "-C", str(repository), "rev-parse", commit + "^{tree}"]).strip()


def patch_path(directory, specification, dependency=False):
    name = specification["patchFile" if dependency else "file"]
    require(Path(name).name == name and name.endswith(".patch"), "patch must remain in manifest directory")
    path = directory / name
    expected = specification["patchSha256" if dependency else "sha256"]
    require(re.fullmatch(r"[0-9a-f]{64}", expected), "invalid patch SHA256")
    require(digest(path.read_bytes()) == expected, "pinned patch hash mismatch: " + name)
    return path


def export_module(repository, commit, target):
    archive = run(["git", "-C", str(repository), "archive", "--format=tar", commit], raw=True)
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        # data_filter rejects absolute/traversing paths and escaping links.
        tar.extractall(target, filter="data")


def json_stream(data):
    decoder, position, values = json.JSONDecoder(), 0, []
    while position < len(data):
        while position < len(data) and data[position].isspace():
            position += 1
        if position == len(data):
            break
        value, position = decoder.raw_decode(data, position)
        values.append(value)
    return values


def provision(manifest_path, source_repo, checkout, module_repos=None, go_binary=None):
    manifest_path, source_repo, checkout = map(lambda value: Path(value).expanduser().resolve(),
                                               (manifest_path, source_repo, checkout))
    module_repos = module_repos or {}
    manifest_bytes = manifest_path.read_bytes()
    manifest, directory = json.loads(manifest_bytes), manifest_path.parent
    environment = os.environ.copy()
    environment.update({"GOWORK": "off", "GOFLAGS": "", "GOTOOLCHAIN": "local",
                        "GOPROXY": "off", "GOSUMDB": "off"})
    upstream_tree = pinned_commit(source_repo, manifest["sourceCommit"])
    require(not checkout.exists() or not any(checkout.iterdir()), "source checkout must be fresh and empty")
    patches = [patch_path(directory, patch) for patch in manifest["patches"]]
    dependencies = manifest.get("dependencyPatches", [])
    if dependencies:
        require(go_binary and Path(go_binary).is_file(), "pinned Go executable required")
        go_version = run([str(go_binary), "version"], env=environment)
        require(" go" + manifest["goVersion"] + " " in go_version, "source Go toolchain differs")
    seen_modules, seen_names = set(), set()
    # Validate all supplied inputs before creating or changing the checkout.
    for dependency in dependencies:
        name, module = dependency["name"], dependency["module"]
        require(re.fullmatch(r"[a-z0-9][a-z0-9-]*", name), "invalid dependency directory")
        require(module not in seen_modules and name not in seen_names, "duplicate dependency")
        seen_modules.add(module)
        seen_names.add(name)
        require(module in module_repos, "provide pinned dependency repository: " + module)
        require(re.fullmatch(r"[0-9a-f]{40}", dependency["patchedTree"]), "dependency patched tree missing")
        require(dependency["sourceURL"].startswith("https://github.com/"), "dependency upstream URL must be explicit")
        pinned_commit(module_repos[module], dependency["upstreamCommit"])
        patch_path(directory, dependency, dependency=True)
    run(["git", "clone", "--shared", "--no-checkout", str(source_repo), str(checkout)])
    # Lock raw compiler/manifests to their source bytes even when the native
    # runner's global Git configuration enables CRLF checkout conversion.
    run(["git", "config", "--local", "core.autocrlf", "false"], cwd=checkout)
    run(["git", "checkout", "--detach", manifest["sourceCommit"]], cwd=checkout)
    for patch in patches:
        run(["git", "apply", "--check", str(patch)], cwd=checkout)
        run(["git", "apply", str(patch)], cwd=checkout)
    run(["git", "add", "-A"], cwd=checkout)
    patched_source_tree = run(["git", "write-tree"], cwd=checkout).strip()
    if "patchedSourceTree" in manifest:
        require(patched_source_tree == manifest["patchedSourceTree"], "patched core source tree differs")
    receipt = {"schema": "polaris-core-source-v1", "sourceCommit": manifest["sourceCommit"],
               "sourceURL": manifest.get("sourceURL", "https://github.com/SagerNet/sing-box"),
               "upstreamTree": upstream_tree, "patchedSourceTree": patched_source_tree,
               "sourceManifestSha256": digest(manifest_bytes), "patches": manifest["patches"],
               "provisionerSha256": digest(Path(__file__).read_bytes()), "dependencies": []}
    receipt["sourceGraphState"] = "dependencies-patched" if dependencies else "source-only"
    receipt["graphScope"] = "declared-patched-modules" if dependencies else "core-source-only"
    receipt["moduleGraphQueries"] = sorted(dependency["module"] for dependency in dependencies)
    for dependency in dependencies:
        relative = Path("polaris-dependencies") / dependency["name"]
        target = checkout / relative
        target.mkdir(parents=True)
        repository = Path(module_repos[dependency["module"]]).expanduser().resolve()
        tree = pinned_commit(repository, dependency["upstreamCommit"])
        export_module(repository, dependency["upstreamCommit"], target)
        require(re.search(r"(?m)^module\s+" + re.escape(dependency["module"]) + r"\s*$", (target / "go.mod").read_text()), "dependency module identity differs")
        patch = patch_path(directory, dependency, dependency=True)
        # Seed exact upstream blobs and modes from the pinned local repository.
        # A Windows filesystem cannot supply Git's executable bits for new files.
        run(["git", "fetch", "--no-tags", str(repository), dependency["upstreamCommit"]], cwd=checkout)
        run(["git", "read-tree", "--prefix=" + relative.as_posix() + "/", dependency["upstreamCommit"]], cwd=checkout)
        run(["git", "update-index", "--refresh"], cwd=checkout)
        apply = ["git", "apply", "--index", "--directory=" + relative.as_posix()]
        run([*apply, "--check", str(patch)], cwd=checkout)
        run([*apply, str(patch)], cwd=checkout)
        patched_tree = run(["git", "write-tree", "--prefix=" + relative.as_posix() + "/"], cwd=checkout).strip()
        require(patched_tree == dependency["patchedTree"], "dependency patched tree differs: " + dependency["module"])
        require(go_binary, "Go executable required for explicit dependency overrides")
        # This generated go.mod visibly names the patched source. No mutation
        # occurs in the supplied upstream tree or shared module cache.
        run([str(go_binary), "mod", "edit", "-replace=" + dependency["module"] + "=./" + relative.as_posix()], cwd=checkout, env=environment)
        # The next dependency refresh must see the already generated manifest.
        # The final receipt still hashes and stages both main module manifests.
        run(["git", "add", "-f", "go.mod"], cwd=checkout)
        receipt["dependencies"].append({**dependency, "upstreamTree": tree,
                                         "replacement": "./" + relative.as_posix()})
    if dependencies:
        # Query exactly the manifest's patched modules. Optional unrelated module
        # metadata is outside this receipt's declared scope; the complete main
        # manifests and each binary's full build info are recorded separately.
        graph = json_stream(run([str(go_binary), "list", "-m", "-json", *receipt["moduleGraphQueries"]], cwd=checkout, env=environment))
        require(sorted(item["Path"] for item in graph) == receipt["moduleGraphQueries"], "queried module set differs")
        for dependency in receipt["dependencies"]:
            value = next((item for item in graph if item["Path"] == dependency["module"]), None)
            require(value and value.get("Version") == dependency["upstreamVersion"], "upstream module version differs")
            replacement = value.get("Replace", {})
            require(replacement.get("Path") == dependency["replacement"] and Path(replacement.get("Dir", "")).resolve() == (checkout / dependency["replacement"]).resolve(), "compiled dependency replacement differs")
        # Paths are build-directory dependent; the portable graph records module
        # versions/checksums and the explicit manifest replacement instead.
        portable = []
        for item in graph:
            portable.append({key: item[key] for key in ("Path", "Version", "Sum", "GoModSum") if key in item} |
                            ({"Replace": {key: item["Replace"][key] for key in ("Path", "Version", "Sum", "GoModSum") if key in item["Replace"]}} if "Replace" in item else {}))
        receipt["moduleGraph"] = portable
        receipt["moduleGraphSha256"] = digest(json.dumps(portable, sort_keys=True, separators=(",", ":")).encode())
    receipt["mainGoModSha256"] = digest((checkout / "go.mod").read_bytes())
    receipt["mainGoSumSha256"] = digest((checkout / "go.sum").read_bytes())
    run(["git", "add", "-f", "go.mod", "go.sum"], cwd=checkout)
    receipt["buildTree"] = run(["git", "write-tree"], cwd=checkout).strip()
    receipt["fingerprint"] = digest(json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode())
    (checkout / ".polaris-source-receipt.json").write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    return receipt


def source_linker_flag(receipt):
    require(re.fullmatch(r"[0-9a-f]{64}", receipt["fingerprint"]), "invalid source fingerprint")
    return "-buildid=polaris-source-v1-" + receipt["fingerprint"]


def verify_checkout(checkout, receipt):
    """Reject changes to staged source or compiler inputs before packaging."""
    require(run(["git", "write-tree"], cwd=checkout).strip() == receipt["buildTree"], "staged build source tree differs")
    run(["git", "diff", "--exit-code"], cwd=checkout)
    for filename, field in (("go.mod", "mainGoModSha256"), ("go.sum", "mainGoSumSha256")):
        require(digest((Path(checkout) / filename).read_bytes()) == receipt[field], "main module manifest differs: " + filename)
    extra = run(["git", "ls-files", "--others", "--exclude-standard"], cwd=checkout).splitlines()
    inputs = (".go", ".c", ".cc", ".cpp", ".h", ".s", ".S", ".syso")
    require(not any(path.endswith(inputs) or Path(path).name in {"go.mod", "go.sum", "go.work", "go.work.sum"} or path.startswith("vendor/") for path in extra), "untracked compiler input present")


def verify_binary(go_binary, binary, receipt, required_modules=("github.com/sagernet/sing-tun",)):
    info = run([str(go_binary), "version", "-m", str(binary)])
    build_id = run([str(go_binary), "tool", "buildid", str(binary)]).strip()
    require(build_id == source_linker_flag(receipt).removeprefix("-buildid="), "binary source fingerprint differs")
    modules, current = {}, None
    for line in info.splitlines():
        columns = line.strip().split("\t")
        if columns[0] == "dep" and len(columns) >= 3:
            current = columns[1]
            modules[current] = {"version": columns[2]}
        elif columns[0] == "=>" and current is not None and len(columns) >= 2:
            modules[current]["replacement"] = columns[1]
        elif columns[0] != "=>":
            current = None
    declared = {item["module"]: item for item in receipt["dependencies"]}
    for module in required_modules:
        require(module in declared and module in modules, "required patched module missing from binary: " + module)
    for module, dependency in declared.items():
        if module in modules:
            require(modules[module]["version"] == dependency["upstreamVersion"]
                    and modules[module].get("replacement") == dependency["replacement"], "binary dependency provenance differs: " + module)
    return {"sha256": digest(Path(binary).read_bytes()), "buildID": build_id,
            "sourceFingerprint": receipt["fingerprint"], "goBuildInfo": info,
            "linkedModules": modules}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--checkout", type=Path, required=True)
    parser.add_argument("--module-source", action="append", default=[], metavar="MODULE=REPOSITORY")
    parser.add_argument("--go", type=Path)
    args = parser.parse_args()
    repositories = {}
    for value in args.module_source:
        module, separator, path = value.partition("=")
        require(separator and module not in repositories and path, "invalid/duplicate module source")
        repositories[module] = path
    print(json.dumps(provision(args.manifest, args.source, args.checkout, repositories, args.go), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"core source provisioning failed: {error}", file=__import__("sys").stderr)
        raise SystemExit(1)
