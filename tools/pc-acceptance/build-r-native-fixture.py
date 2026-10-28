#!/usr/bin/env python3
"""Explicit compile-only R fixture observations. No native execution or G ready plan."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
TARGET = 'github.com/sagernet/sing-tun'
TEST_FILE = 'polaris-dependencies/sing-tun/route_native_receipt_linux_test.go'
OVERLAY = ROOT / 'tools/pc-acceptance/r-native-fixture-overlay.patch'
OVERLAY_SHA = '2ca6559a6546e4639c6eeee141d3e90849b52671ed7465121245ccd31f20622d'
BEFORE_SHA = 'da784771f37e2a022afd3d6f333c1a8fc4cc2a0ff8c2e6401fba00bf6ba1be34'
AFTER_SHA = '0ab03175b50ede1f46124718332a3cc367f2bb294d842abf874568e37a7b015e'
FLAGS = ['-mod=readonly', '-tags=polaris_r_native']
FILE_FIELDS = ('GoFiles', 'CgoFiles', 'CFiles', 'CXXFiles', 'MFiles', 'HFiles', 'FFiles',
               'SFiles', 'SwigFiles', 'SwigCXXFiles', 'SysoFiles', 'EmbedFiles',
               'TestGoFiles', 'XTestGoFiles', 'TestEmbedFiles', 'XTestEmbedFiles')


def require(ok, message):
    if not ok:
        raise RuntimeError('R fixture source: ' + message)


def helper_module():
    spec = importlib.util.spec_from_file_location('r_source_helper', ROOT / 'scripts/libbox-patches/android-source.py')
    helper = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helper)
    return helper


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON field')
        result[key] = value
    return result


def json_object(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 4 * 1024**2,
            'missing/oversized/redirected JSON input')
    result = json.loads(path.read_bytes(), object_pairs_hook=unique)
    require(isinstance(result, dict) and result, 'empty/non-object JSON input')
    return result


def stream(raw, provider):
    # Keep the original provider's stream parser; reject duplicate fields first.
    decoder, offset = json.JSONDecoder(object_pairs_hook=unique), 0
    while offset < len(raw):
        offset += len(raw[offset:]) - len(raw[offset:].lstrip())
        if offset == len(raw):
            break
        _, offset = decoder.raw_decode(raw, offset)
    rows = provider.json_stream(raw)
    require(rows and all(isinstance(row, dict) and row for row in rows), 'empty/non-object JSON stream')
    return rows


def source_inventory(root, helper):
    rows = {}
    for directory, dirs, files in os.walk(root, followlinks=False):
        if Path(directory) == root:
            dirs[:] = [name for name in dirs if name != '.git']
            files = [name for name in files if name != '.git']
        for name in sorted(dirs + files):
            path = Path(directory) / name
            info = path.lstat()
            if stat.S_ISDIR(info.st_mode):
                continue
            key = path.relative_to(root).as_posix()
            if path.is_symlink():
                require(path.resolve(strict=True).is_relative_to(root), 'source symlink escapes checkout')
                link = os.readlink(path)
                row = {'link': link, 'sha256': helper.digest(link.encode()), 'bytes': len(link.encode())}
            else:
                require(stat.S_ISREG(info.st_mode), 'non-regular source input')
                row = {'sha256': helper.file_hash(path), 'bytes': info.st_size}
            rows[key] = {**row, 'mode': stat.S_IMODE(info.st_mode)}
    require(rows, 'empty source inventory')
    return dict(sorted(rows.items()))


def input_paths(checkout, receipt, go, output):
    values = []
    for raw in (checkout, receipt, go, output):
        require(isinstance(raw, (str, Path)) and str(raw).strip() and str(raw) != '.', 'empty path')
        path = Path(raw).expanduser().absolute()
        require('..' not in path.parts and path.resolve() == path, 'redirected/traversing path')
        values.append(path)
    checkout, receipt, go, output = values
    require(checkout.is_dir() and receipt.is_file() and go.is_file() and not go.is_symlink(), 'input missing')
    require(output.parent.is_dir() and not output.exists() and not output.is_symlink(), 'output must be fresh, even if empty')
    roots = [ROOT, checkout, go.parent.parent, Path.home() / 'go',
             Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache'))) / 'go-build']
    for key in ('GOCACHE', 'GOMODCACHE', 'GOPATH'):
        if os.environ.get(key):
            roots.extend(Path(value).expanduser().resolve() for value in os.environ[key].split(os.pathsep) if value)
    require(not any(output.is_relative_to(root) or root.is_relative_to(output) for root in roots),
            'output overlaps source/tools/cache')
    return checkout, receipt, go, output


def load_inputs(receipt_path, helper, provider):
    source_path = ROOT / 'scripts/libbox-patches/source-manifest.json'
    source = json_object(source_path)
    core = json_object(ROOT / 'src-tauri/core-manifest.json')
    receipt = json_object(receipt_path)
    helper.validate_source_receipt(receipt, source, core)
    require(helper.file_hash(source_path) == core['sourceBuild']['sourceManifestSha256']
            and helper.file_hash(helper.PROVIDER) == core['sourceBuild']['provisionerSha256'], 'manifest/provider drift')
    patches = [provider.patch_path(source_path.parent, row) for row in source['patches']]
    patches += [provider.patch_path(source_path.parent, row, dependency=True) for row in source['dependencyPatches']]
    require(helper.file_hash(OVERLAY) == OVERLAY_SHA, 'overlay differs, including extra/partial diff')
    require(next(row for row in receipt['dependencies'] if row['module'] == TARGET)['replacement']
            == './polaris-dependencies/sing-tun', 'fixture replacement differs')
    paths = patches + [source_path, ROOT / 'src-tauri/core-manifest.json', helper.PROVIDER,
                       ROOT / 'scripts/libbox-patches/android-source.py', OVERLAY, Path(__file__), receipt_path,
                       ROOT / 'scripts/native-netns-harness.py', ROOT / 'scripts/verify-r-native-receipts.py']
    return source, receipt, {str(path): helper.file_hash(path) for path in paths}


class Commands:
    def __init__(self, output, env):
        self.output, self.env, self.rows, self.started = output, env, [], time.monotonic()
        self.cleanup_allowed = True

    def group_exists(self, pgid):
        try:
            os.killpg(pgid, 0)
            return True
        except ProcessLookupError:
            return False

    def stop_group(self, child, row):
        # Only the PGID registered at our own successful start_new_session spawn.
        # Parent exit never closes this obligation; no unknown-process scan occurs.
        try:
            try:
                os.killpg(row['registeredProcessGroup'], signal.SIGKILL)
            except ProcessLookupError:
                pass
            row['terminationWaitExit'] = child.wait(timeout=10)
            deadline = time.monotonic() + 1
            while self.group_exists(row['registeredProcessGroup']) and time.monotonic() < deadline:
                time.sleep(0.01)
            row['groupDrained'] = not self.group_exists(row['registeredProcessGroup'])
        except (OSError, subprocess.TimeoutExpired) as error:
            row['groupDrainError'], row['groupDrained'] = str(error), False
        if not row['groupDrained']:
            self.cleanup_allowed = False

    def run(self, args, *, cwd, env=None, raw=False):
        require(time.monotonic() - self.started < 2700, 'compiler window exhausted')
        argv = [str(arg) for arg in args]
        index = len(self.rows) + 1
        row = {'argv': argv, 'cwd': str(cwd), 'env': dict(env or self.env), 'exit': None,
               'stdout': f'{index:03d}.stdout', 'stderr': f'{index:03d}.stderr'}
        self.rows.append(row)
        try:
            with (self.output / row['stdout']).open('xb') as stdout, (self.output / row['stderr']).open('xb') as stderr:
                child = subprocess.Popen(argv, cwd=cwd, env=row['env'], stdout=stdout, stderr=stderr,
                                         close_fds=True, start_new_session=True)
                row['registeredProcessGroup'] = child.pid
                try:
                    row['exit'] = child.wait(timeout=min(900, max(1, 2700 - (time.monotonic() - self.started))))
                except BaseException as error:
                    self.stop_group(child, row)
                    if isinstance(error, subprocess.TimeoutExpired):
                        row['timeout'] = True
                        raise RuntimeError('R fixture source: compiler command timed out') from error
                    raise
                try:
                    remaining = self.group_exists(row['registeredProcessGroup'])
                except OSError:
                    remaining = True
                if remaining:
                    row['groupPresentAfterParentExit'] = True
                    self.stop_group(child, row)
                    raise RuntimeError('R fixture source: descendants remain after compiler parent exit')
                row['groupDrained'] = True
            require(row['exit'] == 0, 'compiler command failed')
            data = self.output / row['stdout']
            require(data.stat().st_size <= 64 * 1024**2, 'compiler observation too large')
            return data.read_bytes() if raw else data.read_text()
        except (OSError, RuntimeError) as error:
            row['error'] = str(error)
            raise
        finally:
            (self.output / 'commands.json').write_text(json.dumps(self.rows, indent=2, sort_keys=True) + '\n')


def module_graph(raw, root, receipt, provider, cache, helper):
    rows = stream(raw, provider)
    require(all(helper.match('[A-Za-z0-9._/-]+', row.get('Path')) and not row.get('Error') for row in rows)
            and len({row['Path'] for row in rows}) == len(rows),
            'duplicate/invalid MVS module')
    main_name = re.search(r'^module\s+(\S+)\s*$', (root / 'go.mod').read_text(), re.M)
    require(main_name, 'main module declaration missing')
    mains = [row for row in rows if row.get('Main')]
    require(len(mains) == 1 and mains[0]['Path'] == main_name[1]
            and mains[0].get('Main') is True and Path(mains[0].get('Dir', '')).resolve() == root
            and Path(mains[0].get('GoMod', '')).resolve() == root / 'go.mod', 'MVS main binding differs')
    declared = {row['module']: row for row in receipt['dependencies']}
    for row in rows:
        name = row['Path']
        if name in declared:
            dep = declared[name]
            replace = row.get('Replace', {})
            expected = root / dep['replacement']
            require(row.get('Version') == dep['upstreamVersion'] and replace.get('Path') == dep['replacement']
                    and Path(replace.get('Dir', '')).resolve() == expected
                    and Path(replace.get('GoMod', '')).resolve() == expected / 'go.mod', 'MVS replacement differs')
        elif not row.get('Main'):
            require('Replace' not in row and isinstance(row.get('Version'), str) and row['Version'], 'unreviewed MVS replacement/version')
            if row.get('Dir'):
                require(Path(row['Dir']).resolve().is_relative_to(cache), 'MVS source escapes module cache')
    require(set(declared) <= {row['Path'] for row in rows}, 'MVS misses declared replacement')
    return rows


def package_graph(raw, root, receipt, mvs, provider, helper, host, temporary):
    rows, by_path, hashes = stream(raw, provider), {}, {}
    roots = [root, Path(host['GOMODCACHE']), Path(host['GOROOT']), Path(host['GOCACHE']), temporary]
    modules = {row['Path']: row for row in mvs}
    for row in rows:
        name = row.get('ImportPath')
        require(isinstance(name, str) and name and name not in by_path
                and not any(row.get(key) for key in ('Error', 'DepsErrors', 'Incomplete')), 'duplicate/incomplete package graph')
        # Go's full ImportPath is the instance identity; ForTest/bracket variants stay distinct.
        by_path[name] = row
        require(row.get('Dir') or name == TARGET + '.test', 'package directory missing')
        directory = Path(row.get('Dir') or str(root / 'polaris-dependencies/sing-tun')).resolve(strict=True)
        require(any(directory.is_relative_to(path) for path in roots), 'package source escapes observed roots')
        module = row.get('Module')
        if module:
            actual = modules.get(module.get('Path'))
            require(actual and all(module.get(key) == actual.get(key) for key in ('Version', 'Replace', 'Dir', 'GoMod', 'Main')),
                    'package module differs from complete MVS')
            effective = module.get('Replace', module)
            require(directory.is_relative_to(Path(effective['Dir']).resolve()), 'package escapes its actual module')
        else:
            require((row.get('Standard') and directory.is_relative_to(Path(host['GOROOT']))) or name == TARGET + '.test',
                    'package has no module identity')
        for field in FILE_FIELDS:
            names = row.get(field, [])
            require(isinstance(names, list) and all(isinstance(filename, str) for filename in names)
                    and len(set(names)) == len(names), 'duplicate/invalid source file inventory')
            for filename in names:
                require(isinstance(filename, str) and filename and '..' not in Path(filename).parts, 'source filename escapes package')
                path = Path(filename)
                if path.is_absolute():
                    require(name == TARGET + '.test' and any(path.resolve().is_relative_to(p) for p in roots[3:]),
                            'unreviewed absolute generated source')
                else:
                    path = directory / path
                    require(path.resolve().is_relative_to(directory), 'source link escapes package')
                require(path.is_file(), 'selected source missing')
                hashes[str(path.resolve())] = helper.file_hash(path)
        require(isinstance(row.get('Imports', []), list) and all(isinstance(edge, str) and edge for edge in row.get('Imports', []))
                and isinstance(row.get('ImportMap', {}), dict)
                and all(isinstance(key, str) and isinstance(value, str) and value for key, value in row.get('ImportMap', {}).items()),
                'invalid import edges')
    require(TARGET in by_path and TARGET + '.test' in by_path and by_path[TARGET + '.test'].get('Name') == 'main',
            'original package or actual generated test main absent')
    require(by_path[TARGET].get('Module', {}).get('Path') == TARGET
            and Path(by_path[TARGET]['Dir']).resolve() == root / 'polaris-dependencies/sing-tun',
            'original test target module/directory differs')
    reachable, pending = set(), [TARGET, TARGET + '.test']
    while pending:
        name = pending.pop()
        if name in reachable or name == 'C':
            continue
        require(name in by_path, 'truncated test dependency graph')
        reachable.add(name)
        row = by_path[name]
        pending.extend(row.get('ImportMap', {}).get(edge, edge) for edge in row.get('Imports', []))
    require(reachable == set(by_path), 'unreachable package records')
    for module in mvs:
        for candidate in (module, module.get('Replace', {})):
            if candidate.get('GoMod'):
                path = Path(candidate['GoMod']).resolve(strict=True)
                require(path.is_file() and any(path.is_relative_to(p) for p in roots[:3]), 'module manifest escapes roots')
                hashes[str(path)] = helper.file_hash(path)
    return rows, hashes


def elf_identity(path, helper):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size >= 64
            and path.stat().st_mode & 0o111, 'ELF absent/nonexecutable/redirected')
    with path.open('rb') as file:
        header = file.read(64)
    require(header[:7] == b'\x7fELF\x02\x01\x01' and int.from_bytes(header[16:18], 'little') in (2, 3)
            and int.from_bytes(header[18:20], 'little') == 62, 'compiled artifact is not Linux amd64 ELF')
    return {'path': str(path), 'bytes': path.stat().st_size, 'sha256': helper.file_hash(path), 'elfClass': 2, 'elfMachine': 62}


def build(checkout, receipt_path, go, output, *, source_compiler=False, runner_factory=Commands):
    require(source_compiler is True, 'explicit source compiler mode required')
    checkout, receipt_path, go, output = input_paths(checkout, receipt_path, go, output)
    helper = helper_module()
    provider = helper.provider()
    source, receipt, input_hashes = load_inputs(receipt_path, helper, provider)
    before = source_inventory(checkout, helper)
    require(before.get(TEST_FILE, {}).get('sha256') == BEFORE_SHA, 'original fixture preimage differs')
    output.mkdir(mode=0o700)
    output_id = (output.stat().st_dev, output.stat().st_ino)
    commands = None
    created, report, success = [], {'scope': 'R-compile-only-observations-NoRuntime', 'status': 'active',
                                  'sourceReceipt': receipt, 'inputHashes': input_hashes,
                                  'selectedTopLists': None, 'GPlanIssued': False, 'runtimeAdmission': False}, False
    binary = output / 'r-native.test'
    try:
        env = {key: os.environ[key] for key in ('HOME', 'PATH', 'GOPATH', 'GOMODCACHE', 'GOCACHE') if key in os.environ}
        env.update({'GOENV': 'off', 'GOWORK': 'off', 'GOTOOLCHAIN': 'local', 'GOPROXY': 'off', 'GOSUMDB': 'off',
                    'GOFLAGS': '', 'GOOS': 'linux', 'GOARCH': 'amd64', 'GOAMD64': 'v1', 'CGO_ENABLED': '0',
                    'GOEXPERIMENT': '', 'GOTELEMETRY': 'off', 'POLARIS_NO_KERNEL_RUN': '1'})
        commands = runner_factory(output, env)
        provider.run = commands.run
        helper.verify_checkout(checkout, receipt, provider)
        for name in ('source', 'tmp'):
            path = output / name
            path.mkdir(mode=0o700)
            created.append((path, (path.stat().st_dev, path.stat().st_ino)))  # register before copy can fail
        copy, temporary = output / 'source', output / 'tmp'
        shutil.copytree(checkout, copy, symlinks=True, dirs_exist_ok=True,
                        ignore=lambda directory, names: ['.git'] if Path(directory) == checkout else [])
        require(source_inventory(copy, helper) == before, 'partial/changed source copy')
        commands.run(['git', 'apply', '--whitespace=error-all', '--check', str(OVERLAY)], cwd=copy / 'polaris-dependencies/sing-tun')
        commands.run(['git', 'apply', '--whitespace=error-all', str(OVERLAY)], cwd=copy / 'polaris-dependencies/sing-tun')
        after = source_inventory(copy, helper)
        expected = dict(before)
        expected[TEST_FILE] = {**before[TEST_FILE], 'sha256': AFTER_SHA, 'bytes': 61263}
        require(after == expected, 'overlay changed source outside the sole fixture delta')
        report.update({'canonicalInventory': before, 'fixtureInventory': after,
                       'fixtureTree': {'algorithm': 'sha256-canonical-path-mode-link-byte-inventory',
                                       'sha256': helper.digest(helper.canonical(after)), 'files': len(after)},
                       'productionTreeUnchanged': receipt['buildTree'], 'overlaySha256': OVERLAY_SHA})
        env['TMPDIR'] = str(temporary)
        version = commands.run([go, 'version'], cwd=copy)
        require(version.strip() == f"go version go{source['goVersion']} linux/amd64", 'pinned Go host/version differs')
        host = json.loads(commands.run([go, 'env', '-json'], cwd=copy), object_pairs_hook=unique)
        for key in ('GOOS', 'GOARCH', 'GOAMD64', 'CGO_ENABLED', 'GOFLAGS', 'GOWORK', 'GOTOOLCHAIN'):
            require(host.get(key) == env[key], 'effective compiler environment differs: ' + key)
        # Go 1.25.5 reports disabled GOENV as empty, while command input remains off.
        require(host.get('GOENV') == '', 'effective compiler environment differs: GOENV must be disabled')
        require(host.get('GOHOSTOS') == 'linux' and host.get('GOHOSTARCH') == 'amd64'
                and Path(host['GOROOT']).resolve() == go.parent.parent, 'actual Go distribution differs')
        for key in ('GOROOT', 'GOTOOLDIR', 'GOMODCACHE', 'GOCACHE'):
            path = Path(host[key]).resolve(strict=True)
            require(path.is_dir() and not path.is_relative_to(output), 'tool/cache directory missing or overlaps output')
            require(not output.is_relative_to(path), 'output lives inside actual cache/tool directory')
        tool_hashes = {str(go): helper.file_hash(go)}
        for name in ('compile', 'link', 'asm'):
            path = Path(host['GOTOOLDIR']) / name
            require(path.is_file() and path.resolve().is_relative_to(go.parent.parent), 'compiler tool escapes pinned distribution')
            tool_hashes[str(path)] = helper.file_hash(path)
        report['toolchain'] = {'version': version, 'env': host, 'executableHashes': tool_hashes}
        mvs = module_graph(commands.run([go, 'list', '-m', '-json', '-mod=readonly', 'all'], cwd=copy),
                           copy, receipt, provider, Path(host['GOMODCACHE']), helper)
        packages, hashes = package_graph(commands.run([go, 'list', '-deps', '-test', '-json', *FLAGS, TARGET], cwd=copy),
                                          copy, receipt, mvs, provider, helper, host, temporary)
        hashes.update({str(copy / path): row['sha256'] for path, row in after.items() if 'link' not in row})
        report.update({'moduleGraph': mvs, 'moduleGraphSha256': helper.digest(helper.canonical(mvs)),
                       'testPackageGraph': packages, 'testPackageGraphSha256': helper.digest(helper.canonical(packages)),
                       'sourceManifest': hashes, 'sourceFilesSha256': helper.digest(helper.canonical(hashes)),
                       'goModSha256': helper.file_hash(copy / 'go.mod'), 'goSumSha256': helper.file_hash(copy / 'go.sum')})
        require(report['goModSha256'] == receipt['mainGoModSha256'] and report['goSumSha256'] == receipt['mainGoSumSha256'],
                'copy module manifests differ')
        argv = [go, 'test', '-c', *FLAGS, '-ldflags=-checklinkname=0', '-o', binary, TARGET]
        report['buildFlags'] = [str(arg) for arg in argv[1:]]
        commands.run(argv, cwd=copy)
        report['elf'] = elf_identity(binary, helper)
        require(source_inventory(copy, helper) == after and source_inventory(checkout, helper) == before, 'source changed during compiler window')
        helper.verify_checkout(checkout, receipt, provider)
        for filename, expected_sha in {**hashes, **input_hashes, **tool_hashes}.items():
            require(helper.file_hash(filename) == expected_sha, 'compiler input drift: ' + filename)
        report['status'] = 'CompiledIncomplete_NoRuntime_NoSelectedTopLists'
        success = True
        return report
    except BaseException as error:
        report['status'], report['error'] = 'FailedIncomplete_NoRuntime', str(error)
        raise
    finally:
        cleanup = []
        root_matches = output.exists() and not output.is_symlink() and (output.stat().st_dev, output.stat().st_ino) == output_id
        groups_drained = getattr(commands, 'cleanup_allowed', True)
        for path, identity in reversed(created):
            matches = root_matches and groups_drained and path.exists() and not path.is_symlink() and (path.stat().st_dev, path.stat().st_ino) == identity
            row = {'path': str(path), 'deviceInode': list(identity), 'removedOwnedDirectory': False}
            if matches:
                try:
                    shutil.rmtree(path)
                    row['removedOwnedDirectory'] = not path.exists()
                except OSError as error:
                    row['error'] = str(error)
            cleanup.append(row)
        if root_matches:
            if groups_drained and not success and (binary.is_file() or binary.is_symlink()):
                binary.unlink()
            report['cleanup'] = cleanup
            if not all(row['removedOwnedDirectory'] for row in cleanup):
                report['status'] = 'FailedOwnedCleanup_NoRuntime'
            (output / 'observations.json').write_text(json.dumps(report, indent=2, sort_keys=True) + '\n')
        require(root_matches and groups_drained and all(row['removedOwnedDirectory'] for row in cleanup),
                'compiler group unresolved or owned directory identity changed; source/foreign paths retained')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-compiler', action='store_true')
    for name in ('checkout', 'receipt', 'go', 'output'):
        parser.add_argument('--' + name)
    args = parser.parse_args(argv)
    if not args.source_compiler:
        print('NOT_EXECUTED: explicit --source-compiler required; no tools or output touched')
        return 0
    if not all(getattr(args, name) for name in ('checkout', 'receipt', 'go', 'output')):
        parser.error('source compiler requires --checkout --receipt --go --output')
    report = build(args.checkout, args.receipt, args.go, args.output, source_compiler=True)
    print(report['status'])
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1)
