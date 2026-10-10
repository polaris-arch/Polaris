#!/usr/bin/env python3
"""Download only reviewed origin archives; never execute their contents."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import tempfile
import zipfile


def require(condition, message):
    if not condition:
        raise ValueError(message)


def api(endpoint):
    return subprocess.check_output(['gh', 'api', endpoint], timeout=180)


def extract_archive(archive, destination, key):
    names = {f'{key}/' + ('sing-box.exe' if key == 'win' else 'sing-box')}
    names |= {name + '.source-receipt.json' for name in list(names)}
    with zipfile.ZipFile(archive) as z:
        files = [i for i in z.infolist() if not i.is_dir()]
        require(len(files) == 2 and {i.filename for i in files} == names, 'unexpected origin archive inventory')
        for info in files:
            mode = info.external_attr >> 16
            require(not stat.S_ISLNK(mode) and info.file_size <= 200_000_000, 'unsafe origin archive member')
            p = PurePosixPath(info.filename)
            require(not p.is_absolute() and '..' not in p.parts, 'unsafe origin archive path')
        for info in files:
            path = destination / info.filename
            require(not path.exists(), 'origin destination already present')
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(z.read(info))
            if not path.name.endswith('.json') and key != 'win':
                path.chmod(0o755)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--bundle-dir', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    subprocess.run(['node', '--input-type=module', '-e',
                    'import {validationContext} from "./scripts/desktop-core/validation-origin.mjs"; validationContext(process.env.GITHUB_SHA);'],
                   cwd=root, check=True, timeout=30)
    policy = json.loads((root / 'scripts/desktop-core/validation-origin.json').read_text())
    repo = policy['repository']
    run = json.loads(api(f'repos/{repo}/actions/runs/{policy["runId"]}/attempts/{policy["attempt"]}'))
    transport = {'repository': repo, 'run': run, 'artifacts': {}}
    # Check API identity before any archive download; the Node bridge checks it again.
    require(run['id'] == policy['runId'] and run['run_attempt'] == policy['attempt']
            and run['head_sha'] == policy['workflowHead'] and run['status'] == 'completed'
            and run['conclusion'] == 'success' and run['event'] == 'workflow_dispatch'
            and run['repository']['id'] == policy['repositoryId']
            and run['head_repository']['id'] == policy['repositoryId'], 'original run differs')
    args.bundle_dir.mkdir(parents=True, exist_ok=True)
    for key, pin in policy['platforms'].items():
        artifact = json.loads(api(f'repos/{repo}/actions/artifacts/{pin["artifactId"]}'))
        wr = artifact['workflow_run']
        require(artifact['id'] == pin['artifactId'] and artifact['name'] == pin['artifactName']
                and artifact['expired'] is False and artifact['digest'] == 'sha256:' + pin['archiveSha256']
                and wr['id'] == policy['runId'] and wr['head_sha'] == policy['workflowHead']
                and wr['repository_id'] == policy['repositoryId']
                and wr['head_repository_id'] == policy['repositoryId'], 'artifact identity differs')
        with tempfile.TemporaryDirectory(prefix='polaris-origin-') as tmp:
            archive = Path(tmp) / 'origin.zip'
            with archive.open('wb') as out:
                subprocess.run(['gh', 'api', f'repos/{repo}/actions/artifacts/{pin["artifactId"]}/zip'],
                               stdout=out, check=True, timeout=300)
            actual = hashlib.sha256(archive.read_bytes()).hexdigest()
            require(actual == pin['archiveSha256'], 'downloaded origin ZIP differs')
            extract_archive(archive, args.bundle_dir, key)
        transport['artifacts'][key] = {'api': artifact, 'archiveSha256': actual}
    output = args.bundle_dir / 'origin-transport.json'
    output.write_text(json.dumps(transport, indent=2) + '\n')
    subprocess.run(['node', '--input-type=module', '-e',
                    'import {readFileSync} from "node:fs"; import {verifyOriginTransport} from "./scripts/desktop-core/validation-origin.mjs"; '
                    'verifyOriginTransport(process.cwd(), JSON.parse(readFileSync(process.argv[1])));', str(output)],
                   cwd=root, check=True, timeout=30)


if __name__ == '__main__':
    main()
