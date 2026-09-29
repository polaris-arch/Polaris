#!/usr/bin/env python3
"""Private C4 CI artifact: exact-path source/candidate archive and local gate.

Build/pack is for an isolated Darwin arm64 CI runner. Import and verify are
local-only; none of these commands opens SSH or enables a product window.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import stat
import subprocess
import tarfile
import zipfile

import runner

prepare = runner.load_prepare()

HERE = Path(__file__).resolve().parent
SCHEMA = 'c4-mac-cgo1-proc-only-bundle-v2'
REPOSITORY = 'polaris-arch/Polaris'
WORKFLOW = '.github/workflows/c4-mac-masque-candidate.yml'
MAX_MEMBERS = 50000
MAX_FILE_BYTES = 256 * 1024 * 1024
MAX_TOTAL_BYTES = 2 * 1024 * 1024 * 1024
GO_ENV_KEYS = ('GOOS', 'GOARCH', 'CGO_ENABLED', 'GOTOOLCHAIN', 'GOWORK',
               'GOFLAGS', 'GOPROXY', 'GOSUMDB', 'GOPATH', 'GOMODCACHE')


def paths(nonce):
    runner.need(isinstance(nonce, str) and
                re.fullmatch(r'[0-9a-f]{8}', nonce) is not None,
                'CI bundle nonce invalid')
    base = Path('/var/tmp')
    return (base / ('polaris-mac-masque-source-' + nonce),
            base / ('polaris-mac-masque-fdtrace-' + nonce),
            base / ('polaris-mac-masque-ci-' + nonce + '.tar'),
            base / ('polaris-mac-masque-ci-' + nonce + '.json'))


def file_gate(path):
    row = path.lstat()
    runner.need(stat.S_ISREG(row.st_mode) and not path.is_symlink() and
                row.st_uid == os.getuid() and stat.S_IMODE(row.st_mode) == 0o600,
                'CI bundle file owner/mode/entity invalid')


def root_gate(path):
    row = path.lstat()
    runner.need(stat.S_ISDIR(row.st_mode) and not path.is_symlink() and
                row.st_uid == os.getuid() and stat.S_IMODE(row.st_mode) == 0o700,
                'CI bundle root owner/mode/entity invalid')


def member_name(name):
    runner.need(all(piece not in ('', '.', '..') for piece in name.split('/')),
                'CI archive path traversal or unexpected root')
    part = PurePosixPath(name)
    runner.need(not part.is_absolute() and
                len(part.parts) >= 1 and part.parts[0] in ('source', 'candidate')
                and all(piece not in ('', '.', '..') for piece in part.parts),
                'CI archive path traversal or unexpected root')
    return part


def tree_inventory(source, candidate):
    """Inventory extracted bytes; .git is checked by Git, not mutable index bytes."""
    result = {}
    for label, root in (('source', source), ('candidate', candidate)):
        root_gate(root)
        for path in [root, *sorted(root.rglob('*'))]:
            rel = path.relative_to(root)
            if label == 'source' and rel.parts and rel.parts[0] == '.git':
                continue
            key = label if not rel.parts else label + '/' + rel.as_posix()
            row = path.lstat()
            if stat.S_ISDIR(row.st_mode):
                result[key] = ('dir', '')
            elif stat.S_ISREG(row.st_mode):
                result[key] = ('file', runner.digest(path))
            else:
                raise ValueError('CI bundle contains symlink or special file')
    return result


def archive_inventory(archive):
    result = {}
    total = 0
    runner.need(archive.stat().st_size <= MAX_TOTAL_BYTES,
                'CI archive byte limit exceeded')
    with tarfile.open(archive, mode='r:') as stream:
        for member in stream:
            runner.need(len(result) < MAX_MEMBERS and
                        0 <= member.size <= MAX_FILE_BYTES,
                        'CI archive member/size limit exceeded')
            total += member.size
            runner.need(total <= MAX_TOTAL_BYTES,
                        'CI archive expanded byte limit exceeded')
            part = member_name(member.name)
            name = part.as_posix()
            runner.need(name not in result and (member.isdir() or member.isfile()) and
                        member.uid == member.gid == 0 and member.mtime == 0 and
                        not (member.mode & 0o7022),
                        'CI archive duplicate/special/metadata invalid')
            if member.isfile():
                h = hashlib.sha256()
                data = stream.extractfile(member)
                runner.need(data is not None, 'CI archive file unreadable')
                for block in iter(lambda: data.read(1048576), b''):
                    h.update(block)
                result[name] = ('file', h.hexdigest())
            else:
                result[name] = ('dir', '')
    runner.need('source' in result and 'candidate' in result and
                'source/.git' in result,
                'CI archive source/candidate Git roots missing')
    return result


def comparable_inventory(inventory):
    return {name: row for name, row in inventory.items()
            if not (name == 'source/.git' or name.startswith('source/.git/'))}


def ci_metadata(env):
    value = {
        'repository': env.get('GITHUB_REPOSITORY'),
        'workflow': WORKFLOW,
        'workflowCommit': env.get('GITHUB_SHA'),
        'ref': env.get('GITHUB_REF'),
        'runId': env.get('GITHUB_RUN_ID'),
        'runAttempt': env.get('GITHUB_RUN_ATTEMPT'),
        'runnerOS': env.get('RUNNER_OS'),
        'runnerArch': env.get('RUNNER_ARCH'),
        'event': env.get('GITHUB_EVENT_NAME'),
    }
    runner.need(env.get('GITHUB_ACTIONS') == 'true' and
                value['repository'] == REPOSITORY and
                value['runnerOS'] == 'macOS' and
                value['runnerArch'] == 'ARM64' and
                value['event'] == 'push' and
                isinstance(value['ref'], str) and
                value['ref'].startswith('refs/heads/c4-mac-masque-') and
                isinstance(value['workflowCommit'], str) and
                re.fullmatch(r'[0-9a-f]{40}', value['workflowCommit']) and
                all(isinstance(value[key], str) and value[key].isdigit() and
                    int(value[key]) > 0 for key in ('runId', 'runAttempt')),
                'CI build provenance environment incomplete')
    return value


def collect_toolchain():
    """Capture the actual CI Go and native SDK tools used by pack."""
    def output(argv):
        return subprocess.run(argv, check=True, capture_output=True,
                              text=True, timeout=20).stdout.strip()

    result = {
        'goVersion': output(['go', 'version']),
        'goEnv': json.loads(output(['go', 'env', '-json', *GO_ENV_KEYS])),
        'xcodeVersion': output(['xcodebuild', '-version']),
        'sdkVersion': output(['xcrun', '--sdk', 'macosx', '--show-sdk-version']),
        'sdkPath': output(['xcrun', '--sdk', 'macosx', '--show-sdk-path']),
        'clangPath': output(['xcrun', '--sdk', 'macosx', '--find', 'clang']),
        'socketStateABI': prepare.socket_state_abi(),
    }
    toolchain_gate(result)
    return result


def toolchain_gate(value):
    runner.need(isinstance(value, dict) and
                set(value) == {'goVersion', 'goEnv', 'xcodeVersion',
                               'sdkVersion', 'sdkPath', 'clangPath',
                               'socketStateABI'},
                'CI toolchain receipt schema incomplete')
    env = value['goEnv']
    runner.need(isinstance(env, dict) and set(env) == set(GO_ENV_KEYS) and
                value['goVersion'] == 'go version go1.26.8 darwin/arm64' and
                all(env.get(key) == expected for key, expected in {
                    'GOOS': 'darwin', 'GOARCH': 'arm64', 'CGO_ENABLED': '1',
                    'GOTOOLCHAIN': 'go1.26.8', 'GOWORK': 'off',
                    'GOFLAGS': '-mod=readonly',
                    'GOPROXY': 'https://proxy.golang.org',
                    'GOSUMDB': 'sum.golang.org',
                }.items()) and
                all(isinstance(env.get(key), str) and
                    Path(env[key]).is_absolute() and
                    len(env[key]) <= 1024 and '\n' not in env[key]
                    for key in ('GOPATH', 'GOMODCACHE')) and
                isinstance(value['xcodeVersion'], str) and
                re.fullmatch(r'Xcode [0-9]+(?:\.[0-9]+)*\nBuild version [A-Za-z0-9]+',
                             value['xcodeVersion']) is not None and
                isinstance(value['sdkVersion'], str) and
                re.fullmatch(r'[0-9]+(?:\.[0-9]+)*', value['sdkVersion']) is not None and
                all(isinstance(value[key], str) and
                    Path(value[key]).is_absolute() and
                    len(value[key]) <= 1024 and '\n' not in value[key]
                    for key in ('sdkPath', 'clangPath')),
                'CI Go/Xcode/SDK identity differs from reviewed build face')
    abi = value['socketStateABI']
    runner.need(isinstance(abi, dict) and
                set(abi) == {'sdkPath', 'sdkVersion', 'socketFdinfoSize',
                             'macroSize', 'flavor', 'cannotSendBit'} and
                abi['sdkPath'] == value['sdkPath'] and
                abi['sdkVersion'] == value['sdkVersion'] and
                type(abi['socketFdinfoSize']) is int and
                0 < abi['socketFdinfoSize'] <= 4096 and
                abi['socketFdinfoSize'] == abi['macroSize'] and
                abi['flavor'] == 3 and abi['cannotSendBit'] == 16,
                'CI public socket SDK ABI witness invalid')


def gh_json(endpoint):
    result = subprocess.run(['gh', 'api', '-H',
                             'Accept: application/vnd.github+json', endpoint],
                            capture_output=True, text=True, check=True,
                            timeout=30)
    return json.loads(result.stdout)


def provenance_gate(receipt_path, archive, receipt):
    """Independently bind the local files to a completed GitHub CI run.

    The installed gh must support attestation verify. An unavailable CLI,
    network, run, artifact, digest or attestation is a hard local stop.
    """
    ci = receipt['ci']
    run_id = ci['runId']
    run = gh_json('repos/' + REPOSITORY + '/actions/runs/' + run_id)
    runner.need(run.get('id') == int(run_id) and
                run.get('run_attempt') == int(ci['runAttempt']) and
                run.get('head_sha') == ci['workflowCommit'] and
                run.get('head_branch') == ci['ref'].removeprefix('refs/heads/') and
                run.get('path') == WORKFLOW and
                run.get('event') == 'push' and
                run.get('status') == 'completed' and
                run.get('conclusion') == 'success' and
                run.get('head_repository', {}).get('full_name') == REPOSITORY,
                'GitHub run metadata does not independently bind CI receipt')
    artifact_name = 'c4-mac-masque-' + receipt['nonce']
    listing = gh_json('repos/' + REPOSITORY + '/actions/runs/' + run_id +
                      '/artifacts?name=' + artifact_name)
    artifacts = listing.get('artifacts')
    runner.need(isinstance(artifacts, list) and len(artifacts) == 1,
                'GitHub run artifact missing or ambiguous')
    artifact = artifacts[0]
    runner.need(artifact.get('name') == artifact_name and
                artifact.get('expired') is False and
                artifact.get('workflow_run', {}).get('id') == int(run_id) and
                artifact['workflow_run'].get('head_sha') ==
                ci['workflowCommit'],
                'GitHub artifact not from pinned run')
    package = Path('/var/tmp/polaris-mac-masque-ci-' + receipt['nonce'] + '.zip')
    file_gate(package)
    runner.need(package.stat().st_size <= MAX_TOTAL_BYTES and
                artifact.get('size_in_bytes') == package.stat().st_size and
                artifact.get('digest') == 'sha256:' + runner.digest(package),
                'downloaded GitHub artifact ZIP digest differs')
    with zipfile.ZipFile(package) as zipped:
        names = zipped.namelist()
        runner.need(set(names) == {archive.name, receipt_path.name} and
                    len(names) == 2,
                    'GitHub artifact ZIP member set differs')
        for path in (archive, receipt_path):
            info = zipped.getinfo(path.name)
            limit = MAX_TOTAL_BYTES if path == archive else 1048576
            runner.need(0 < info.file_size <= limit and
                        not stat.S_ISLNK(info.external_attr >> 16) and
                        not info.is_dir(),
                        'GitHub artifact ZIP member invalid')
            h = hashlib.sha256()
            with zipped.open(info) as data:
                for block in iter(lambda: data.read(1048576), b''):
                    h.update(block)
            runner.need(h.hexdigest() == runner.digest(path),
                        'GitHub artifact ZIP contents differ from local files')
    signer = REPOSITORY + '/' + WORKFLOW
    for path in (archive, receipt_path):
        subprocess.run([
            'gh', 'attestation', 'verify', str(path), '--repo', REPOSITORY,
            '--signer-workflow', signer,
            '--signer-digest', ci['workflowCommit'],
            '--source-digest', ci['workflowCommit'],
            '--source-ref', ci['ref'], '--deny-self-hosted-runners'],
            capture_output=True, text=True, check=True, timeout=120)


def make_receipt(nonce, source, candidate, archive, manifest, ci, toolchain):
    toolchain_gate(toolchain)
    runner.need(manifest.get('socketStateABI') == toolchain['socketStateABI'],
                'candidate and CI public socket SDK ABI differ')
    runner.need(manifest.get('linkerFlagsSha256') ==
                prepare.PINS['release/LDFLAGS'] and
                manifest.get('linkerFlags') == prepare.LINK_FLAGS,
                'CI manifest linker flags differ from pinned source')
    runner.need(manifest.get('compiledSourceRoot') == '/private' + str(source),
                'CI compiled source physical path differs from Mac overlay contract')
    modes = {mode: {
        'binarySha256': runner.digest(candidate /
                                      ('sing-box-c4-' + mode + '-darwin-arm64')),
        'buildinfoSha256': manifest['modes'][mode]['buildinfoSha256']}
        for mode in runner.MODE}
    return {
        'schema': SCHEMA, 'nonce': nonce,
        'sourcePath': str(source), 'candidatePath': str(candidate),
        'compiledSourceRoot': manifest['compiledSourceRoot'],
        'sourceHead': runner.SOURCE_HEAD, 'sourcePins': prepare.PINS,
        'goModSha256': runner.digest(source / 'go.mod'),
        'goSumSha256': runner.digest(source / 'go.sum'),
        'quicReferenceTreeSha256': prepare.QUIC_TREE_SHA256,
        'singQuicReferenceTreeSha256': prepare.SING_QUIC_TREE_SHA256,
        'quicGoSum': prepare.QUIC_GO_SUM,
        'singQuicGoSum': prepare.SING_QUIC_GO_SUM,
        'buildProfile': prepare.BUILD_PROFILE,
        'buildTags': prepare.TAGS,
        'linkerFlagsSha256': prepare.PINS['release/LDFLAGS'],
        'linkerFlags': prepare.LINK_FLAGS,
        'prepareSha256': runner.digest(HERE / 'prepare.py'),
        'runnerSha256': runner.digest(HERE / 'runner.py'),
        'traceSourceSha256': runner.digest(HERE / 'polaris_trace_darwin.go'),
        'socketStateSourceSha256': {
            name: runner.digest(HERE / name)
            for name in prepare.SOCKET_STATE_SOURCES},
        'socketStateABI': manifest['socketStateABI'],
        'bundleToolSha256': runner.digest(Path(__file__)),
        'candidateManifestSha256': runner.digest(candidate / 'prepare.json'),
        'modes': modes, 'archivePath': str(archive),
        'archiveSha256': runner.digest(archive),
        'archiveBytes': archive.stat().st_size, 'ci': ci,
        'toolchain': toolchain,
    }


def write_exclusive(path, content):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 0o600)
    with os.fdopen(fd, 'wb') as out:
        out.write(content)
        out.flush()
        os.fsync(out.fileno())


def pack(nonce, quic_reference, sing_quic_reference, env=None):
    runner.need(platform.system() == 'Darwin' and
                platform.machine() in ('arm64', 'aarch64'),
                'CI bundle pack requires native Darwin arm64')
    ci = ci_metadata(os.environ if env is None else env)
    toolchain = collect_toolchain()
    source, candidate, archive, receipt = paths(nonce)
    runner.need(not archive.exists() and not archive.is_symlink() and
                not receipt.exists() and not receipt.is_symlink(),
                'CI archive/receipt already exists')
    root_gate(source)
    root_gate(candidate)
    for mode in runner.MODE:
        runner.candidate_gate(candidate, mode, quic_reference=quic_reference,
                              sing_quic_reference=sing_quic_reference,
                              require_bundle=False)
    # Reject symlinks/special files before tarfile can follow or encode them.
    tree_inventory(source, candidate)
    for root in (source, candidate):
        for path in root.rglob('*'):
            row = path.lstat()
            runner.need(stat.S_ISDIR(row.st_mode) or stat.S_ISREG(row.st_mode),
                        'CI source/candidate contains symlink or special file')
    fd = os.open(archive, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
                 0o600)
    with os.fdopen(fd, 'wb') as out, tarfile.open(fileobj=out, mode='w') as tar:
        for label, root in (('source', source), ('candidate', candidate)):
            for path in [root, *sorted(root.rglob('*'))]:
                name = label if path == root else label + '/' + path.relative_to(root).as_posix()
                row = tar.gettarinfo(str(path), arcname=name)
                row.uid = row.gid = row.mtime = 0
                row.uname = row.gname = ''
                if row.isfile():
                    with path.open('rb') as stream:
                        tar.addfile(row, stream)
                else:
                    tar.addfile(row)
        out.flush()
        os.fsync(out.fileno())
    manifest = json.loads((candidate / 'prepare.json').read_text())
    build_receipt = make_receipt(nonce, source, candidate, archive, manifest,
                                 ci, toolchain)
    write_exclusive(receipt, (json.dumps(build_receipt, indent=2,
                                          sort_keys=True) + '\n').encode())
    verify(candidate, manifest, require_provenance=False)
    return archive, receipt


def receipt_gate(root, manifest):
    nonce = root.name.removeprefix('polaris-mac-masque-fdtrace-')
    source, candidate, archive, receipt_path = paths(nonce)
    runner.need(root == candidate, 'CI bundle candidate path differs')
    file_gate(archive)
    file_gate(receipt_path)
    receipt = json.loads(receipt_path.read_text())
    ci = receipt.get('ci')
    runner.need(isinstance(ci, dict) and ci.get('repository') == REPOSITORY and
                ci.get('workflow') == WORKFLOW and
                ci.get('runnerOS') == 'macOS' and
                ci.get('runnerArch') == 'ARM64' and
                ci.get('event') == 'push' and
                isinstance(ci.get('ref'), str) and
                ci['ref'].startswith('refs/heads/c4-mac-masque-') and
                isinstance(ci.get('workflowCommit'), str) and
                re.fullmatch(r'[0-9a-f]{40}', ci['workflowCommit']) and
                all(isinstance(ci.get(key), str) and ci[key].isdigit() and
                    int(ci[key]) > 0 for key in ('runId', 'runAttempt')),
                'CI build provenance receipt incomplete')
    toolchain = receipt.get('toolchain')
    toolchain_gate(toolchain)
    expected = make_receipt(nonce, source, candidate, archive, manifest,
                            ci, toolchain)
    runner.need(receipt == expected and
                manifest.get('source') == str(source) and
                archive.stat().st_size > 0,
                'CI receipt/archive/source binding differs')
    return source, archive, receipt_path, receipt


def verify(root, manifest, *, require_provenance=True):
    source, archive, receipt_path, receipt = receipt_gate(root, manifest)
    if require_provenance:
        provenance_gate(receipt_path, archive, receipt)
    root_gate(source)
    root_gate(root)
    archived = comparable_inventory(archive_inventory(archive))
    actual = tree_inventory(source, root)
    runner.need(archived == actual,
                'CI archive and extracted source/candidate bytes differ')
    return receipt_path, archive, receipt


def unpack(receipt_path):
    receipt_path = Path(receipt_path)
    file_gate(receipt_path)
    receipt = json.loads(receipt_path.read_text())
    source, candidate, archive, expected_receipt = paths(receipt['nonce'])
    runner.need(receipt_path == expected_receipt and
                not source.exists() and not source.is_symlink() and
                not candidate.exists() and not candidate.is_symlink(),
                'CI bundle import path occupied or receipt path changed')
    file_gate(archive)
    runner.need(runner.digest(archive) == receipt.get('archiveSha256') and
                archive.stat().st_size == receipt.get('archiveBytes'),
                'CI archive bytes differ before import')
    archive_inventory(archive)
    provenance_gate(receipt_path, archive, receipt)
    # No extractall: every path/type is checked before opening an exact new
    # local name. On any error, leave partial roots for inspection, no remote.
    source.mkdir(mode=0o700)
    candidate.mkdir(mode=0o700)
    with tarfile.open(archive, mode='r:') as stream:
        for member in sorted(stream.getmembers(),
                             key=lambda row: (len(member_name(row.name).parts),
                                              not row.isdir(), row.name)):
            part = member_name(member.name)
            if len(part.parts) == 1:
                continue
            root = source if part.parts[0] == 'source' else candidate
            path = root.joinpath(*part.parts[1:])
            if member.isdir():
                path.mkdir(mode=0o700)
            else:
                data = stream.extractfile(member)
                runner.need(data is not None, 'CI archive extraction file absent')
                fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL |
                             os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, 'wb') as out:
                    for block in iter(lambda: data.read(1048576), b''):
                        out.write(block)
                    os.fchmod(out.fileno(), member.mode & 0o777)
    manifest = json.loads((candidate / 'prepare.json').read_text())
    verify(candidate, manifest, require_provenance=False)
    for mode in runner.MODE:
        runner.candidate_gate(candidate, mode)
    return candidate


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    pack_args = commands.add_parser('pack')
    pack_args.add_argument('--nonce', required=True)
    pack_args.add_argument('--quic-reference', type=Path, required=True)
    pack_args.add_argument('--sing-quic-reference', type=Path, required=True)
    import_args = commands.add_parser('import')
    import_args.add_argument('--receipt', type=Path, required=True)
    verify_args = commands.add_parser('verify')
    verify_args.add_argument('--candidate', type=Path, required=True)
    args = parser.parse_args()
    if args.action == 'pack':
        archive, receipt = pack(args.nonce, args.quic_reference,
                                args.sing_quic_reference)
        print(json.dumps({'archive': str(archive), 'archiveSha256':
                          runner.digest(archive), 'receipt': str(receipt),
                          'receiptSha256': runner.digest(receipt)}))
    elif args.action == 'import':
        print(json.dumps({'candidate': str(unpack(args.receipt))}))
    else:
        manifest = json.loads((args.candidate / 'prepare.json').read_text())
        verify(args.candidate, manifest)
        print(json.dumps({'verified': str(args.candidate)}))


if __name__ == '__main__':
    main()
