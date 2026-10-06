#!/usr/bin/env python3
"""C3 compiler observations and immutable component publication; no App admission."""
from collections import Counter, defaultdict
from contextlib import contextmanager
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shlex
import shutil
import stat
import sys
import tempfile
import uuid

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location('component_' + name.replace('-', '_'), Path(__file__).with_name(name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


inputs = load('apple-source-inputs')
carrier = load('apple-carrier')


def driver():
    return load('ios-libbox')


def require(ok, message):
    if not ok:
        raise RuntimeError('Apple component: ' + message)


def fingerprint(value):
    helper = carrier.source_helpers()
    return helper.digest(helper.canonical(value))



def input_fingerprint(value):
    return fingerprint({key: item for key, item in value.items() if key not in ('appleInputFingerprint', 'cleanup')})


def read_json(path):
    rows = inputs.json_stream(Path(path).read_text())
    require(len(rows) == 1, 'one JSON object required')
    return rows[0]



def source_candidate(helper):
    provider = helper.provider()
    require(not provider.run(['git', 'status', '--porcelain=v1', '--untracked-files=all'], cwd=ROOT).strip(), 'producer candidate is dirty')
    candidate = provider.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT).strip()
    tree = provider.run(['git', 'rev-parse', 'HEAD^{tree}'], cwd=ROOT).strip()
    require(helper.match('[0-9a-f]{40}', candidate) and helper.match('[0-9a-f]{40}', tree), 'producer candidate/tree invalid')
    return {'candidate': candidate, 'tree': tree}


def apple_flags(receipt, core, ios):
    flags = ios.LINKER_FLAGS.format(version=core['sourceBuild']['version'])
    require(flags.count('-buildid=') == 1 and flags.endswith('-buildid='), 'Apple linker template differs')
    return flags[:-len('-buildid=')] + ios.source_helpers().provider().source_linker_flag(receipt)


class Commands(inputs.Commands):
    """C2 owns execution/drain; C3 only supplies fixed task-local compiler tuples."""
    def __init__(self, evidence, env, executables, checkout, scratch, source_receipt=None, go=None):
        super().__init__(evidence, env, executables)
        self.checkout, self.scratch = Path(checkout).resolve(), Path(scratch).resolve()
        self.archive_commands = ()
        self.target_inputs = {}
        self.external_inputs = {}
        if source_receipt is not None:
            ios = driver()
            _, core, policy = ios.final_preflight()
            flags = apple_flags(source_receipt, core, ios)
            rows = []
            for target_id, target in policy['targets'].items():
                args = [str(go), 'build', '-buildmode=c-archive', '-o', str(self.scratch / (target_id + '.a')),
                        '-tags=' + ','.join(inputs.effective_tags(policy, bool(target['variant']))), '-ldflags', flags,
                        '-trimpath', '-buildvcs=false', '-x', '-work', '.']
                rows.extend([tuple(args), tuple(args[:-1] + ['-a', '.'])])
            self.archive_commands = tuple(rows)

    def configure_targets(self, tools, generated):
        policy = driver().final_preflight()[2]
        for target_id, target in policy['targets'].items():
            name = 'iossimulator' if target['variant'] else 'ios'
            sdk = tools[target['sdk']]
            expected = inputs.target_env(self.env, target, sdk['path'], sdk['clang']['path'],
                                         policy['binding']['iosMinimumVersion'], generated[name])
            cwd = self.checkout / 'build' / (name + '-' + target['goarch']) / 'Libbox'
            self.target_inputs[target_id] = {'cwd': cwd, 'env': expected}

    def _owned(self, value):
        path = Path(value)
        return path.is_absolute() and path.resolve().is_relative_to(self.scratch) and not path.is_symlink()

    def _allowed_command(self, args, cwd, env):
        if super()._allowed_command(args, cwd, env):
            return True
        if tuple(args) in self.archive_commands:
            target_id = Path(args[4]).name.removesuffix('.a')
            expected = self.target_inputs.get(target_id)
            return bool(expected and Path(cwd).resolve() == expected['cwd'].resolve() and env == expected['env'])
        if self.executables.get(args[0]) == 'go':
            for target_id, expected in self.target_inputs.items():
                fixed = next(row for row in self.archive_commands if Path(row[4]).name == target_id + '.a')
                if (Path(cwd).resolve() == expected['cwd'].resolve() and env == expected['env'] and args[1:] ==
                    ['list', '-deps', '-f', '{{range .CgoLDFLAGS}}{{println .}}{{end}}', fixed[5], inputs.BOUND]):
                    return True
            return False
        if args[0] == '/usr/bin/xcrun' and len(args) == 3 and args[1] == '--find' and args[2] in ('libtool', 'lipo', 'ld'):
            return True
        if args[0] == '/usr/bin/xcrun' and len(args) >= 6:
            if args[1:4] == ['libtool', '-static', '-o']:
                target_id = Path(args[4]).name.removesuffix('.merged.a')
                expected = self.target_inputs.get(target_id)
                return (target_id in self.external_inputs and expected is not None
                        and Path(cwd).resolve() == expected['cwd'].resolve() and env == expected['env']
                        and args[4:6] == [str(self.scratch / (target_id + '.merged.a')), str(self.scratch / (target_id + '.a'))]
                        and args[6:] == self.external_inputs[target_id])
            if args[1] == 'lipo':
                return (len(args) == 7 and Path(cwd).resolve() == self.checkout and env == self.env
                        and args[-3:] == ['-create', '-output', str(self.scratch / 'simulator-fat.a')]
                        and args[2] in [str(self.scratch / ('ios-arm64-simulator' + ending)) for ending in ('.a', '.merged.a')]
                        and args[3] in [str(self.scratch / ('ios-x86_64-simulator' + ending)) for ending in ('.a', '.merged.a')])
        return False


class OwnedPaths:
    """Only this transaction's exact directory identities; no shared cache cleanup."""
    def __init__(self):
        self.rows = []

    def register(self, path, empty_only=False):
        path = Path(path)
        info = path.lstat()
        require(stat.S_ISDIR(info.st_mode), 'owned directory redirected')
        row = {'path': str(path), 'device': info.st_dev, 'inode': info.st_ino, 'emptyOnly': empty_only}
        require(not any(item['path'] == row['path'] for item in self.rows), 'owned directory registered twice')
        self.rows.append(row)
        return path

    def mkdir(self, path, empty_only=False):
        path = Path(path)
        path.mkdir()
        return self.register(path, empty_only)

    def cleanup(self, commands):
        require(commands.cleanup_allowed and all(row.get('groupDrained') is True for row in commands.rows
                if 'registeredProcessGroup' in row), 'group drain unknown; owned inputs retained')
        removed = []
        for row in reversed(self.rows):
            path = Path(row['path'])
            info = path.lstat()
            require(stat.S_ISDIR(info.st_mode) and (info.st_dev, info.st_ino) == (row['device'], row['inode']),
                    'owned directory identity changed; cleanup blocked')
            # Compile work/copies may be nonempty; parents are removed only if empty.
            if row['emptyOnly']:
                path.rmdir()
            else:
                shutil.rmtree(path)
            removed.append(str(path))
        return removed


def inventory(root):
    """Full modes, directories, regular bytes and safe symlinks; never follow links."""
    root = Path(root)
    require(root.is_dir() and not root.is_symlink(), 'inventory root redirected/missing')
    root = root.resolve(strict=True)
    helper, rows = carrier.source_helpers(), []
    for current, dirs, files in os.walk(root, followlinks=False):
        for name in sorted(dirs + files):
            path = Path(current) / name
            info = path.lstat()
            row = {'path': path.relative_to(root).as_posix(), 'mode': stat.S_IMODE(info.st_mode)}
            require(not info.st_mode & (stat.S_ISUID | stat.S_ISGID | stat.S_ISVTX), 'unsupported inventory mode')
            if stat.S_ISLNK(info.st_mode):
                target = os.readlink(path)
                require(target and not Path(target).is_absolute() and path.resolve(strict=True).is_relative_to(root),
                        'symlink escapes/missing')
                row.update(type='symlink', target=target)
            elif stat.S_ISDIR(info.st_mode):
                row['type'] = 'directory'
            elif stat.S_ISREG(info.st_mode):
                row.update(type='file', bytes=info.st_size, sha256=helper.file_hash(path))
            else:
                raise RuntimeError('Apple component: unknown inventory file type')
            rows.append(row)
    require(rows, 'empty component inventory')
    return sorted(rows, key=lambda row: row['path'])


def compiler_facts(log, observation, scratch, helper):
    """Read real -x/-work commands and objects; insufficient cache logs request -a."""
    work_rows = re.findall(r'^WORK=(.+)$', log, re.M)
    require(len(work_rows) == 1, 'actual WORK directory missing/duplicate')
    work = Path(work_rows[0]).resolve(strict=True)
    require(work.is_dir() and not work.is_symlink() and work.is_relative_to(Path(scratch).resolve()), 'WORK escapes own scratch')
    commands, cwd = [], None
    for line in log.splitlines():
        if line.startswith('cd '):
            values = shlex.split(line)
            require(len(values) == 2, 'ambiguous compiler cwd')
            cwd = values[1].replace('$WORK', str(work))
        if not line or line.startswith(('WORK=', 'mkdir ', 'cp ', 'mv ', 'cat ', '#', 'cd ')):
            continue
        try:
            argv = shlex.split(line.replace('$WORK', str(work)))
        except ValueError as error:
            raise RuntimeError('Apple component: truncated compiler argv') from error
        while argv and re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]*=.*', argv[0]):
            argv.pop(0)
        if argv and ('/cgo' in argv[0] or '/compile' in argv[0] or '/link' in argv[0]
                     or Path(argv[0]).name in ('clang', 'clang++', 'ar')):
            commands.append({'argv': argv, 'cwd': cwd})
    selected = [row for row in observation['packages'] if Path(row['Dir']).resolve().is_relative_to(Path(scratch).resolve())
                or row['ImportPath'] == observation['main']['ImportPath']]
    require(selected and observation['main'] in selected, 'complete generated compiler graph missing')
    sources, cgo_packages, implementations, generated_compiler = {}, {}, [], []
    main_package = observation['main']
    for package in selected:
        directory = Path(package['Dir'])
        for name in package.get('CgoFiles', []):
            sources[str(directory / name)] = helper.file_hash(directory / name)
        if package.get('CgoFiles'):
            matches = [row for row in commands if '/cgo' in row['argv'][0] and '-importpath' in row['argv']
                       and row['argv'][row['argv'].index('-importpath') + 1] == package['ImportPath']]
            require(len(matches) == 1, 'actual cgo command missing/duplicate: ' + package['ImportPath'])
            argv = matches[0]['argv']
            require('-objdir' in argv, 'actual cgo objdir missing')
            objdir = Path(argv[argv.index('-objdir') + 1]).resolve(strict=True)
            require(objdir.is_relative_to(work) and (objdir / '_cgo_export.h').is_file(), 'real _cgo_export.h missing')
            require(argv.count('--') == 1, 'ambiguous cgo argument boundary')
            boundary = argv.index('--')
            require(not any(value == '-srcdir' or value.startswith('-srcdir=') for value in argv[:boundary]),
                    'unsupported cgo source directory override')
            # Like cgo, select the final consecutive .go operands, never option values.
            source_start = len(argv)
            while source_start > boundary + 1 and argv[source_start - 1].endswith('.go'):
                source_start -= 1
            consumed_sources = []
            for value in argv[source_start:]:
                source = Path(value)
                if not source.is_absolute():
                    source_cwd = matches[0]['cwd']
                    require(source_cwd is not None and Path(source_cwd).is_absolute(), 'actual cgo source cwd missing/relative')
                    source_cwd = Path(source_cwd).resolve(strict=True)
                    require(source_cwd.is_dir(), 'actual cgo source cwd not a directory')
                    source = source_cwd / source
                source = source.resolve(strict=True)
                require(source.is_file(), 'actual cgo source is not a file')
                consumed_sources.append(source)
            for name in package['CgoFiles']:
                require((directory / name).resolve(strict=True) in consumed_sources, 'cgo source not consumed: ' + name)
            cgo_packages[package['ImportPath']] = {'command': matches[0], 'objdir': str(objdir),
                    'exportHeader': {'path': str(objdir / '_cgo_export.h'), 'sha256': helper.file_hash(objdir / '_cgo_export.h')}}
            for generated_file in sorted(objdir.iterdir()):
                if generated_file.suffix not in ('.go', '.c', '.h') or not generated_file.is_file():
                    continue
                generated_compiler.append({'path': str(generated_file), 'sha256': helper.file_hash(generated_file)})
                if generated_file.suffix == '.c':
                    consumed = []
                    for command in commands:
                        command_args = command['argv']
                        if Path(command_args[0]).name in ('clang', 'clang++') and '-c' in command_args and '-o' in command_args:
                            candidates = [Path(value) if Path(value).is_absolute() else Path(command['cwd'] or objdir) / value
                                          for value in command_args[1:] if value.endswith('.c')]
                            if any(path.resolve() == generated_file.resolve() for path in candidates):
                                consumed.append(command)
                    require(len(consumed) == 1, 'actual generated cgo C compile missing/duplicate: ' + str(generated_file))
                    object_file = Path(consumed[0]['argv'][consumed[0]['argv'].index('-o') + 1]).resolve(strict=True)
                    require(object_file.is_file() and object_file.is_relative_to(work), 'generated cgo C object missing')
                    generated_compiler[-1].update(command=consumed[0], object=str(object_file), objectSha256=helper.file_hash(object_file))
        gc_name = 'main' if package is main_package else package['ImportPath']
        gc = [row for row in commands if '/compile' in row['argv'][0] and '-p' in row['argv']
              and row['argv'][row['argv'].index('-p') + 1] == gc_name]
        require(len(gc) == 1, 'actual generated/reverse Go compile missing/duplicate: ' + package['ImportPath'])
        for name in package.get('GoFiles', []):
            source = str(directory / name)
            require(source in gc[0]['argv'], 'actual selected Go source not consumed: ' + source)
            sources[source] = helper.file_hash(source)
        if package.get('CgoFiles'):
            for path in sorted(objdir.iterdir()):
                if path.suffix == '.go':
                    require(str(path) in gc[0]['argv'], 'actual generated cgo Go source not consumed: ' + str(path))

        for name in package.get('MFiles', []):
            source = directory / name
            matches = []
            for row in commands:
                argv = row['argv']
                if Path(argv[0]).name not in ('clang', 'clang++') or '-c' not in argv or '-o' not in argv:
                    continue
                files = [Path(value) if Path(value).is_absolute() else Path(row['cwd'] or directory) / value
                         for value in argv[1:] if value.endswith('.m')]
                if any(path.resolve() == source.resolve() for path in files):
                    matches.append(row)
            require(len(matches) == 1, 'actual Objective-C implementation command missing/duplicate: ' + str(source))
            row, cgo = matches[0], cgo_packages.get(package['ImportPath'])
            require(cgo is not None, 'Objective-C package cgo header binding missing')
            argv = row['argv']
            included = [value[2:] for value in argv if value.startswith('-I') and len(value) > 2]
            included += [argv[i + 1] for i, value in enumerate(argv[:-1]) if value == '-I']
            require(any(Path(value).resolve() == Path(cgo['objdir']) for value in included), 'actual cgo header include chain missing')
            obj = Path(argv[argv.index('-o') + 1]).resolve(strict=True)
            require(obj.is_file() and obj.is_relative_to(work), 'actual Objective-C object missing')
            implementations.append({'source': str(source), 'sourceSha256': helper.file_hash(source), 'command': row,
                                    'object': str(obj), 'objectSha256': helper.file_hash(obj), 'exportHeader': cgo['exportHeader']})
            sources[str(source)] = helper.file_hash(source)
    main = observation['main']
    cgo = cgo_packages.get(main['ImportPath'])
    require(cgo and '-D__GOBIND_DARWIN__' in cgo['command']['argv'], 'actual Darwin support compiler define missing')
    seq = next((item for item in implementations if Path(item['source']).name == 'seq_darwin.m'), None)
    require(seq and all(flag in seq['command']['argv'] for flag in ('-fobjc-arc', '-fmodules', '-fblocks', '-Werror'))
            and '-x' in seq['command']['argv'] and seq['command']['argv'][seq['command']['argv'].index('-x') + 1] == 'objective-c',
            'actual support Objective-C flags missing')
    require(any('Foundation' == value for row in commands for value in row['argv']), 'actual Foundation linkage missing')
    reverse_sources = [path for path in sources if not Path(path).is_relative_to(Path(main['Dir']))]
    require(reverse_sources, 'actual reverse implementation inventory missing')
    require(any('@import ObjectiveC.message' in Path(item['source']).read_text() for item in implementations),
            'actual ObjectiveC.message reverse wrapper missing')
    return {'work': str(work), 'commands': commands, 'cgoPackages': cgo_packages,
            'implementations': implementations, 'generatedCompilerSources': generated_compiler,
            'sourceHashes': sources, 'workInventory': inventory(work)}


def code_members(container):
    slices = container['slices'] if container['kind'] == 'fat-ar' else [{'ordinal': 0, 'archive': container}]
    rows = []
    for slice_row in slices:
        body = slice_row['archive']
        require(body['kind'] == 'bsd-ar', 'component expected static ar')
        for member in body['members']:
            if member['classification'] == 'code':
                rows.append({'sliceOrdinal': slice_row['ordinal'], **member})
    return rows


def native_map(archive_bytes, external, compiler, helper):
    """Preserve multiplicity and ordinal identity; basename is never an identity."""
    origins = defaultdict(list)
    for row in compiler['workInventory']:
        if row['type'] == 'file' and row['path'].endswith('.o'):
            origins[row['sha256']].append({'kind': 'compiler-object', 'path': str(Path(compiler['work']) / row['path']),
                                         'sha256': row['sha256']})
    for library in external:
        for member in code_members(carrier.container(Path(library['path']).read_bytes(), helper)):
            origins[member['sha256']].append({'kind': 'external-static-member', 'archive': library['path'],
                        'archiveSha256': library['sha256'], 'sliceOrdinal': member['sliceOrdinal'],
                        'memberOrdinal': member['ordinal'], 'payloadOffset': member['payloadOffset'], 'sha256': member['sha256']})
    mapped, missing_platform, multiplicities = [], [], Counter()
    for member in code_members(carrier.container(archive_bytes, helper)):
        obj = member['machO']
        if any('buildInfo' in section for section in obj['sections']):
            continue
        choices = origins.get(member['sha256'], [])
        multiplicities[member['sha256']] += 1
        require(multiplicities[member['sha256']] <= len(choices), 'native member multiplicity exceeds actual origins')
        require(choices, 'native archive member has no compiler/static origin: ' + str(member['ordinal']))
        # Equal payloads are indistinguishable by content: keep every origin, never choose the first.
        mapped.append({'memberOrdinal': member['ordinal'], 'payloadOffset': member['payloadOffset'],
                       'sha256': member['sha256'], 'cpu': obj['cpu'], 'subCPU': obj['subCPU'], 'origins': choices})
        if not obj['platformCommands']:
            missing_platform.append(member['sha256'])
    return {'members': mapped, 'nativeMembersWithoutPlatform': sorted(set(missing_platform))}


def observed_contract(binary, target_id, observation, native, source_receipt, shared, core, helper):
    """Bootstrap observations only. This return value cannot authorize a producer."""
    actual = carrier.container(binary, helper)
    members = code_members(actual)
    infos = [(member, section['buildInfo']) for member in members for section in member['machO']['sections'] if 'buildInfo' in section]
    require(infos, 'actual compiler BuildInfo missing')
    expected_id = helper.provider().source_linker_flag(source_receipt).removeprefix('-buildid=')
    graph_modules = {row['Module']['Path']: row['Module'] for row in observation['packages'] if row.get('Module')}
    for member, info in infos:
        facts = info['parsedBuildInfo']
        require(facts['path'] == observation['main']['ImportPath'] and set(facts['modules']) == set(graph_modules),
                'actual archive main/modules differ from complete compiler graph')
        for path, module in graph_modules.items():
            actual_module = facts['modules'][path]
            require(actual_module['kind'] == ('mod' if module.get('Main') else 'dep')
                    and actual_module['version'] == ('(devel)' if module.get('Main') else module['Version'])
                    and actual_module.get('replacement') == module.get('Replace', {}).get('Path'), 'actual archive module binding differs: ' + path)
        ids = [note['value'] for section in member['machO']['sections'] for note in section.get('buildIDs', [])]
        require(ids == [expected_id], 'actual compiler source BuildID differs')
    require(all(info['parsedBuildInfo'] == infos[0][1]['parsedBuildInfo'] for _, info in infos), 'actual carriers metadata differs')
    external = dict(observation['externalInputs'], nativeMembersWithoutPlatform=native['nativeMembersWithoutPlatform'])
    row = {key: observation[key] for key in ('graphFingerprint', 'patchedModules', 'transportModules', 'effectiveTags')}
    row.update(carrierCount=len(infos), buildInfo=infos[0][1]['parsedBuildInfo'], externalInputs=external)
    candidate = {'schema': 'polaris-apple-carrier-contract-v1', 'evidenceScope': 'carrier-inspection-only',
                 'sourceReceiptFingerprint': source_receipt['fingerprint'], 'targets': {target_id: row}}
    # C1 cross-checks semantics against independent graph/env/source, not just raw extraction.
    checked = carrier.inspect_carrier(binary, [target_id], source_receipt, shared, core, candidate, {'targets': {target_id: external}})
    return row, checked



def validate_observation(observed, target, helper):
    packages, selected_sources, compiler = observed.get('packages'), observed.get('sourceHashes'), observed.get('compiler')
    require(isinstance(packages, list) and packages and all(isinstance(row, dict) and row.get('ImportPath')
            and row.get('Dir') and not any(row.get(key) for key in ('Error', 'DepsErrors', 'Incomplete')) for row in packages)
            and len({row['ImportPath'] for row in packages}) == len(packages)
            and observed.get('graphFingerprint') == fingerprint(packages)
            and isinstance(observed.get('main'), dict) and observed['main'] in packages and observed['main'].get('Name') == 'main',
            'complete actual compiler graph pending: ' + target)
    require(isinstance(selected_sources, dict) and selected_sources
            and all(Path(path).is_absolute() and helper.match('[0-9a-f]{64}', value) for path, value in selected_sources.items()),
            'complete selected compiler source hashes pending: ' + target)
    require(isinstance(compiler, dict) and isinstance(compiler.get('commands'), list) and compiler['commands']
            and all(isinstance(row, dict) and isinstance(row.get('argv'), list) and row['argv'] for row in compiler['commands'])
            and isinstance(compiler.get('cgoPackages'), dict) and compiler['cgoPackages']
            and isinstance(compiler.get('implementations'), list) and compiler['implementations']
            and isinstance(compiler.get('generatedCompilerSources'), list) and compiler['generatedCompilerSources']
            and isinstance(compiler.get('workInventory'), list) and compiler['workInventory'],
            'complete actual Go/cgo/object compiler facts pending: ' + target)
    for implementation in compiler['implementations']:
        require(isinstance(implementation, dict) and implementation.get('command') in compiler['commands']
                and implementation.get('source') in selected_sources
                and implementation.get('sourceSha256') == selected_sources[implementation['source']]
                and helper.match('[0-9a-f]{64}', implementation.get('objectSha256'))
                and isinstance(implementation.get('exportHeader'), dict)
                and helper.match('[0-9a-f]{64}', implementation['exportHeader'].get('sha256'))
                and Path(implementation['exportHeader'].get('path', '')).name == '_cgo_export.h',
                'actual implementation/header/object binding pending: ' + target)
    require(isinstance(observed.get('nativeMap'), dict) and isinstance(observed['nativeMap'].get('members'), list)
            and isinstance(observed.get('externalStaticInputs'), list)
            and isinstance(observed.get('actualGoEnvironment'), dict) and observed['actualGoEnvironment'],
            'complete actual native/environment inputs pending: ' + target)


def compiler_preflight(source_receipt, apple_input, build_policy, tools):
    ios = driver()
    shared, core, source_policy = ios.final_preflight()
    helper = ios.source_helpers()
    helper.validate_source_receipt(source_receipt, shared, core)
    require(isinstance(build_policy, dict) and build_policy.get('schema') == 'polaris-apple-build-policy-v1',
            'complete independently frozen component policy pending')
    require(set(build_policy) == {'schema', 'evidenceScope', 'sourceReceiptFingerprint', 'appleInputFingerprint',
            'reviewedObservationSha256', 'carrierContract', 'componentInventory', 'assembly', 'producer', 'sourceTests', 'excludedSourceTests'},
            'component policy fields differ')
    require(build_policy['evidenceScope'] == 'source-component-build-only'
            and build_policy['sourceReceiptFingerprint'] == source_receipt['fingerprint']
            and helper.match('[0-9a-f]{64}', build_policy['reviewedObservationSha256'])
            and build_policy['appleInputFingerprint'] == input_fingerprint(apple_input), 'independent component input freeze differs')
    require(isinstance(apple_input, dict) and apple_input.get('evidenceScope') == 'C3-source-compiler-observations-only'
            and apple_input.get('sourceReceiptFingerprint') == source_receipt['fingerprint']
            and set(apple_input.get('targets', {})) == set(source_policy['targets']), 'actual three-target compiler input pending')
    contract = build_policy['carrierContract']
    require(isinstance(contract, dict) and contract.get('schema') == 'polaris-apple-carrier-contract-v1'
            and contract.get('sourceReceiptFingerprint') == source_receipt['fingerprint']
            and set(contract.get('targets', {})) == set(source_policy['targets']), 'independently frozen carrier contract pending')
    require(isinstance(build_policy['componentInventory'], list) and isinstance(build_policy['assembly'], dict), 'component inventory/assembly field pending')
    require(isinstance(tools, dict) and tools.get('targets') == {target: contract['targets'][target]['externalInputs']
            for target in source_policy['targets']}, 'actual component SDK/CGO target input differs')
    for target, observed in apple_input['targets'].items():
        validate_observation(observed, target, helper)
        require(isinstance(observed, dict) and observed.get('compiler', {}).get('implementations')
                and observed['compiler'].get('cgoPackages') and observed.get('nativeMap')
                and observed.get('headerSwiftTypecheckScope') == 'generated-header-and-swift-typecheck',
                'complete implementation/compiler/header scope pending: ' + target)
        require(observed.get('graphFingerprint') == contract['targets'][target]['graphFingerprint']
                and observed.get('effectiveTags') == inputs.effective_tags(source_policy, bool(source_policy['targets'][target]['variant']))
                and observed.get('externalInputs') == {k: v for k, v in contract['targets'][target]['externalInputs'].items()
                                                      if k != 'nativeMembersWithoutPlatform'}, 'same compiler graph/target binding differs')
    producer = build_policy['producer']
    require(isinstance(producer, dict) and set(producer) == {'candidate', 'tree', 'codeHashes'}
            and helper.match('[0-9a-f]{40}', producer['candidate']) and helper.match('[0-9a-f]{40}', producer['tree']),
            'clean producer candidate/tree pending')
    code = producer['codeHashes']
    required_code = ('scripts/ios-libbox.py', 'scripts/apple-component.py', 'scripts/apple-carrier.py',
                     'scripts/apple-source-inputs.py', 'scripts/core-source-provision.py', 'scripts/libbox-patches/android-source.py',
                     'scripts/libbox-patches/source-manifest.json', 'scripts/libbox-ios-patches/apple-source-policy.json', 'src-tauri/core-manifest.json')
    require(isinstance(code, dict) and set(code) == set(required_code), 'complete producer/helper hashes pending')
    require(all(helper.match('[0-9a-f]{64}', value) for value in code.values()), 'frozen producer code hashes differ')
    tests = build_policy['sourceTests']
    require(isinstance(tests, list) and tests and all(isinstance(row, dict) and row.get('name')
            and row.get('passed') is True and row.get('sourceReceiptFingerprint') == source_receipt['fingerprint']
            and helper.match('[0-9a-f]{64}', row.get('logSha256')) for row in tests)
            and len({row['name'] for row in tests}) == len(tests)
            and isinstance(build_policy['excludedSourceTests'], list), 'applicable source tests/exclusions pending')
    return ios, helper, source_policy



def component_preflight(source_receipt, apple_input, build_policy, tools):
    facts = compiler_preflight(source_receipt, apple_input, build_policy, tools)
    require(build_policy['componentInventory'], 'full component inventory pending')
    assembly = build_policy['assembly']
    require(set(assembly) == {'commands', 'rawLogHashes'} and isinstance(assembly['commands'], list)
            and [row.get('name') for row in assembly['commands']] == ['ios-framework-lipo', 'iossimulator-framework-lipo', 'create-xcframework']
            and all(row.get('exit') == 0 and row.get('groupDrained') is True and isinstance(row.get('argv'), list)
                    and isinstance(row.get('env'), dict) and row.get('cwd') for row in assembly['commands'])
            and isinstance(assembly['rawLogHashes'], dict) and len(assembly['rawLogHashes']) == 6
            and all(carrier.source_helpers().match('[0-9a-f]{64}', value) for value in assembly['rawLogHashes'].values()),
            'actual complete assembly commands/logs pending')
    return facts


def inspect_component(framework, source_receipt, apple_input, build_policy, tools, scratch=None):
    ios, helper, source_policy = component_preflight(source_receipt, apple_input, build_policy, tools)
    framework = Path(framework)
    rows = inventory(framework)
    require(rows == build_policy['componentInventory'], 'complete Framework inventory differs')
    info = plistlib.loads((framework / 'Info.plist').read_bytes())
    require(set(info) == {'AvailableLibraries', 'CFBundlePackageType', 'XCFrameworkFormatVersion'}
            and info['CFBundlePackageType'] == 'XFWK' and info['XCFrameworkFormatVersion'] == '1.0'
            and isinstance(info['AvailableLibraries'], list) and len(info['AvailableLibraries']) == 2, 'XCFramework Info.plist differs')
    seen, slices, carriers = set(), [], {}
    for row in info['AvailableLibraries']:
        require(isinstance(row, dict) and set(row) in ({'LibraryIdentifier', 'LibraryPath', 'SupportedArchitectures', 'SupportedPlatform'},
                {'LibraryIdentifier', 'LibraryPath', 'SupportedArchitectures', 'SupportedPlatform', 'SupportedPlatformVariant'}),
                'XCFramework library fields differ')
        name = row['LibraryIdentifier']
        require(isinstance(name, str) and re.fullmatch(r'ios-[a-z0-9_\-]+', name) and name not in seen
                and row['LibraryPath'] == 'Libbox.framework' and row['SupportedPlatform'] == 'ios', 'XCFramework slice/path/platform differs')
        seen.add(name)
        variant = row.get('SupportedPlatformVariant')
        require(variant in (None, 'simulator'), 'XCFramework variant differs')
        expected = ['ios-arm64-simulator', 'ios-x86_64-simulator'] if variant else ['ios-arm64']
        arches = row['SupportedArchitectures']
        require(isinstance(arches, list) and len(set(arches)) == len(arches)
                and set(arches) == ({'arm64', 'x86_64'} if variant else {'arm64'}), 'XCFramework architectures differ')
        binary = framework / name / 'Libbox.framework/Libbox'
        require(binary.is_file() and binary.resolve(strict=True).is_relative_to(framework.resolve()), 'Framework static binary missing/escapes')
        actual = ios.inspect_source_carrier(binary, expected, source_receipt, build_policy['carrierContract'], tools, scratch)
        require(not set(carriers) & set(actual['targets']), 'duplicate Framework target')
        carriers.update(actual['targets'])
        slices.append({'identifier': name, 'architectures': arches, 'variant': variant, 'binarySha256': helper.file_hash(binary),
                       'container': actual['container']})
    require(set(carriers) == set(source_policy['targets']), 'Framework target inventory differs')
    expected_roots = {'Info.plist'} | seen
    require({path.name for path in framework.iterdir()} == expected_roots, 'extra Framework root file')
    headers = [row for row in rows if row['type'] == 'file' and '/Headers/' in row['path']]
    modulemaps = [row for row in rows if row['type'] == 'file' and row['path'].endswith('/module.modulemap')]
    require(len(modulemaps) == 2 and len(headers) == 8, 'complete Framework header/modulemap inventory differs')
    for name in seen:
        base = framework / name / 'Libbox.framework'
        require(set(path.name for path in (base / 'Headers').iterdir()) == {'Libbox.objc.h', 'Universe.objc.h', 'Libbox.h', 'ref.h'},
                'Framework public header set differs')
        platform_name = 'iossimulator' if any(row['identifier'] == name and row['variant'] for row in slices) else 'ios'
        generated = apple_input['generatedHeaders'][platform_name]
        for filename, expected_hash in generated.items():
            relative = 'Modules/module.modulemap' if filename == 'module.modulemap' else 'Headers/' + filename
            require(helper.file_hash(base / relative) == expected_hash, 'actual generated header/modulemap differs')
    return {'slices': sorted(slices, key=lambda row: row['identifier']), 'files': [row for row in rows if row['type'] == 'file'],
            'directories': [row for row in rows if row['type'] == 'directory'], 'symlinks': [row for row in rows if row['type'] == 'symlink'],
            'headers': headers, 'modulemaps': modulemaps, 'carriers': carriers,
            'externalStaticInputs': {target: apple_input['targets'][target]['externalStaticInputs'] for target in carriers},
            'generatedABI': {target: apple_input['targets'][target]['headerSwiftTypecheckScope'] for target in carriers}}


def verify_component(framework, receipt, apple_input, build_policy, tools, scratch=None):
    receipt = read_json(receipt) if not isinstance(receipt, dict) else receipt
    require(isinstance(receipt, dict) and set(receipt) == {'schema', 'evidenceScope', 'sourceReceipt', 'appleInput',
            'appleInputFingerprint', 'buildPolicySha256', 'assembly', 'producer', 'sourceTests', 'excludedSourceTests', 'component', 'outputFingerprint'},
            'component receipt fields differ')
    require(receipt['schema'] == 'polaris-apple-libbox-receipt-v1' and receipt['evidenceScope'] == 'source-component-build-only',
            'component receipt scope differs')
    source_receipt = receipt['sourceReceipt']
    component_preflight(source_receipt, apple_input, build_policy, tools)
    require(receipt['appleInput'] == apple_input and receipt['appleInputFingerprint'] == input_fingerprint(apple_input)
            and receipt['buildPolicySha256'] == fingerprint(build_policy) and receipt['assembly'] == build_policy['assembly'] and receipt['producer'] == build_policy['producer']
            and receipt['sourceTests'] == build_policy['sourceTests'] and receipt['excludedSourceTests'] == build_policy['excludedSourceTests'],
            'component receipt input/producer/tests differ')
    require(receipt['outputFingerprint'] == fingerprint({k: v for k, v in receipt.items() if k != 'outputFingerprint'}),
            'component output fingerprint differs')
    actual = inspect_component(framework, source_receipt, apple_input, build_policy, tools, scratch)
    require(receipt['component'] == actual, 'component receipt facts differ')
    return receipt


def make_receipt(framework, source_receipt, apple_input, build_policy, tools):
    actual = inspect_component(framework, source_receipt, apple_input, build_policy, tools)
    receipt = {'schema': 'polaris-apple-libbox-receipt-v1', 'evidenceScope': 'source-component-build-only',
            'sourceReceipt': source_receipt, 'appleInput': apple_input, 'appleInputFingerprint': input_fingerprint(apple_input),
            'buildPolicySha256': fingerprint(build_policy), 'assembly': build_policy['assembly'], 'producer': build_policy['producer'],
            'sourceTests': build_policy['sourceTests'], 'excludedSourceTests': build_policy['excludedSourceTests'], 'component': actual}
    receipt['outputFingerprint'] = fingerprint(receipt)
    return verify_component(framework, receipt, apple_input, build_policy, tools)


def safe_output_root(output_root):
    root = Path(output_root)
    require(root.is_absolute() and root.is_dir() and not root.is_symlink() and root == root.resolve(strict=True), 'output root redirected/missing')
    generations = root / '.libbox-generations'
    require(generations.is_dir() and not generations.is_symlink(), 'generation root redirected/missing')
    return root, generations


def selected_generation(root, target):
    require(re.fullmatch(r'\.libbox-generations/[0-9a-f]{64}', target or ''), 'unsafe current pointer target')
    selected = root / target
    require(selected.is_dir() and not selected.is_symlink() and selected.resolve(strict=True) == selected, 'selected generation redirected/missing')
    require({path.name for path in selected.iterdir()} == {'Libbox.xcframework', 'libbox-build-receipt.json'}, 'generation pair inventory differs')
    receipt_path = selected / 'libbox-build-receipt.json'
    require(receipt_path.is_file() and not receipt_path.is_symlink(), 'generation receipt redirected/missing')
    return selected


def snapshot(generation, build_policy, tools, scratch=None):
    receipt_path = generation / 'libbox-build-receipt.json'
    receipt = read_json(receipt_path)
    receipt = verify_component(generation / 'Libbox.xcframework', receipt, receipt['appleInput'], build_policy, tools, scratch)
    require(generation.name == receipt['outputFingerprint'], 'generation name/output fingerprint differs')
    return {'generationRoot': str(generation), 'frameworkPath': str(generation / 'Libbox.xcframework'),
            'receiptPath': str(receipt_path), 'receipt': receipt, 'componentOutputFingerprint': receipt['outputFingerprint'],
            'appleInputFingerprint': receipt['appleInputFingerprint'], 'sourceReceiptFingerprint': receipt['sourceReceipt']['fingerprint']}


def resolve_component(output_root, build_policy, tools, scratch=None):
    root, _ = safe_output_root(output_root)
    current = root / '.libbox-current'
    require(current.is_symlink(), 'current pointer missing/non-symlink')
    target = os.readlink(current)  # Exactly once: callers pin this generation through header/link use.
    return snapshot(selected_generation(root, target), build_policy, tools, scratch)


@contextmanager
def publisher_lock(root):
    path = root / '.libbox-publisher.lock'
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        require(stat.S_ISREG(os.fstat(descriptor).st_mode) and os.fstat(descriptor).st_nlink == 1, 'publisher lock redirected')
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        yield
    finally:
        os.close(descriptor)


def flush_directory(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def flush_generation(root):
    rows = inventory(root)
    for row in rows:
        if row['type'] == 'file':
            descriptor = os.open(root / row['path'], os.O_RDONLY | os.O_NOFOLLOW)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
    for row in reversed(rows):
        if row['type'] == 'directory':
            flush_directory(root / row['path'])
    flush_directory(root)



def verify_producer_code(build_policy, helper):
    inputs.verify_hashes({str(ROOT / name): value for name, value in build_policy['producer']['codeHashes'].items()}, helper)


def verify_compiler_inputs(apple_input, helper):
    for name in ('ios', 'iossimulator'):
        generated = apple_input[name + 'Generated']
        require(inputs.tree_identity(generated['root'], helper) == generated, 'selected generated origin drift: ' + name)
    inputs.verify_hashes(apple_input['inputHashes'], helper)
    for observed in apple_input['targets'].values():
        inputs.verify_hashes(observed['sourceHashes'], helper)
        inputs.verify_hashes({observed['archivePath']: observed['archiveSha256']}, helper)
    tools = apple_input['tools']
    hashes = {tools[name]['path']: tools[name]['sha256'] for name in ('go', 'gomobile', 'gobind', 'xcode', 'xcrun', 'libtool', 'lipo', 'ld')}
    for name in ('iphoneos', 'iphonesimulator'):
        hashes.update({tools[name][compiler]['path']: tools[name][compiler]['sha256'] for compiler in ('clang', 'clang++', 'swiftc')})
        require(inputs.tree_identity(tools[name]['path'], helper) == tools[name]['sysroot'], 'selected SDK/sysroot drift')
    inputs.verify_hashes(hashes, helper)
    require(inputs.tree_identity(tools['host']['GOROOT'], helper) == tools['go']['distribution'], 'selected Go distribution drift')
    require(inputs.tree_identity(tools['gomobileSource']['module']['Dir'], helper) == tools['gomobileSource']['inventory'], 'selected support source drift')


def publish_component(staged_generation, receipt, output_root, *, build_policy, tools, previous_policy=None, previous_tools=None):
    """Same verifier at stage/cache/readback. One pointer commit; generations never GC."""
    receipt = read_json(receipt) if not isinstance(receipt, dict) else receipt
    verify_component(Path(staged_generation) / 'Libbox.xcframework', receipt, receipt['appleInput'], build_policy, tools)
    helper = driver().source_helpers()
    expected_candidate = {key: build_policy['producer'][key] for key in ('candidate', 'tree')}
    require(source_candidate(helper) == expected_candidate, 'frozen publish candidate differs')
    verify_producer_code(build_policy, helper)
    verify_compiler_inputs(receipt['appleInput'], helper)
    root = Path(output_root)
    require(root.is_absolute() and root.is_dir() and not root.is_symlink() and root == root.resolve(strict=True), 'output root redirected/missing')
    generations = root / '.libbox-generations'
    if not generations.exists() and not generations.is_symlink():
        generations.mkdir()
        flush_directory(root)
    root, generations = safe_output_root(root)
    pending, pointer_temp, committed, old_target = None, None, False, None
    with publisher_lock(root):
        current = root / '.libbox-current'
        if current.exists() or current.is_symlink():
            require(current.is_symlink(), 'current pointer non-symlink')
            old_target = os.readlink(current)
            snapshot(selected_generation(root, old_target), previous_policy if previous_policy is not None else build_policy,
                     previous_tools if previous_tools is not None else tools)
        try:
            pending = Path(tempfile.mkdtemp(prefix='.pending-', dir=generations))
            pending_identity = (pending.stat().st_dev, pending.stat().st_ino)
            shutil.copytree(Path(staged_generation) / 'Libbox.xcframework', pending / 'Libbox.xcframework', symlinks=True)
            (pending / 'libbox-build-receipt.json').write_text(json.dumps(receipt, sort_keys=True, indent=2) + '\n')
            verify_component(pending / 'Libbox.xcframework', pending / 'libbox-build-receipt.json', receipt['appleInput'], build_policy, tools)
            flush_generation(pending)
            destination = generations / receipt['outputFingerprint']
            if destination.exists() or destination.is_symlink():
                existing = snapshot(selected_generation(root, '.libbox-generations/' + destination.name), build_policy, tools)
                require(existing['receipt'] == receipt and inventory(destination) == inventory(pending), 'existing generation differs; never overwritten')
            else:
                require(pending.stat().st_dev == generations.stat().st_dev, 'cross filesystem generation')
                os.rename(pending, destination)
                pending = None
            flush_directory(generations)
            # Recheck stage/source/tool policy before the unique selection commit.
            verified = snapshot(destination, build_policy, tools)
            require(source_candidate(helper) == expected_candidate, 'publish candidate drift')
            verify_producer_code(build_policy, helper)
            verify_compiler_inputs(receipt['appleInput'], helper)
            target = '.libbox-generations/' + destination.name
            pointer_temp = root / ('.libbox-pointer-' + uuid.uuid4().hex)
            os.symlink(target, pointer_temp)
            os.replace(pointer_temp, current)
            pointer_temp, committed = None, True
            flush_directory(root)
            require(source_candidate(helper) == expected_candidate, 'postcommit candidate drift')
            verify_producer_code(build_policy, helper)
            verify_compiler_inputs(receipt['appleInput'], helper)
            require(os.readlink(current) == target, 'postcommit current pointer differs')
            require(snapshot(selected_generation(root, target), build_policy, tools) == verified, 'postcommit generation differs')
            return verified
        except BaseException as error:
            if committed:
                try:
                    require(current.is_symlink() and os.readlink(current) == target, 'rollback current changed externally')
                    if old_target is None:
                        current.unlink()
                    else:
                        pointer_temp = root / ('.libbox-rollback-' + uuid.uuid4().hex)
                        os.symlink(old_target, pointer_temp)
                        os.replace(pointer_temp, current)
                        pointer_temp = None
                    flush_directory(root)
                    require((not current.exists() and not current.is_symlink()) if old_target is None
                            else current.is_symlink() and os.readlink(current) == old_target, 'rollback readback differs')
                except BaseException as rollback_error:
                    actual = os.readlink(current) if current.is_symlink() else ('non-symlink' if current.exists() else 'absent')
                    raise RuntimeError('Apple component: rollback pending; publish=' + str(error) + '; rollback='
                                       + str(rollback_error) + '; current=' + actual) from error
            raise
        finally:
            if pointer_temp is not None and pointer_temp.is_symlink():
                pointer_temp.unlink()
            if pending is not None:
                info = pending.lstat()
                require(stat.S_ISDIR(info.st_mode) and (info.st_dev, info.st_ino) == pending_identity, 'pending identity changed; retained')
                shutil.rmtree(pending)


def observe_component(checkout, source_receipt, go, mobile, developer, evidence):
    """Only full source/compiler observations; retain actual inputs for Root review."""
    require(platform.system() == 'Darwin', 'observe-component requires existing Mac source compiler window')
    ios = driver()
    shared, core, policy = ios.final_preflight()
    helper, provider = ios.source_helpers(), ios.source_helpers().provider()
    helper.validate_source_receipt(source_receipt, shared, core)
    checkout, go, mobile, developer = [Path(path).resolve(strict=True) for path in (checkout, go, mobile, developer)]
    helper.verify_checkout(checkout, source_receipt, provider)
    evidence = Path(evidence).expanduser().absolute()
    evidence = evidence.parent.resolve(strict=True) / evidence.name
    require(not evidence.exists() and not evidence.is_symlink() and not any(evidence.is_relative_to(path)
            for path in (checkout, ROOT, mobile, developer, go.parent)), 'evidence must be fresh outside source/tools')
    require(go.is_file() and all((mobile / name).is_file() for name in ('gomobile', 'gobind')), 'pinned tools missing')
    hashes = {str(ROOT / 'src-tauri/gen/apple/PacketTunnel' / name): value for name, value in inputs.CONSUMERS.items()}
    hashes.update({str(ROOT / name): helper.file_hash(ROOT / name) for name in ('scripts/ios-libbox.py', 'scripts/apple-component.py',
            'scripts/apple-source-inputs.py', 'scripts/apple-carrier.py', 'scripts/core-source-provision.py',
            'scripts/libbox-patches/android-source.py', 'scripts/libbox-patches/source-manifest.json',
            'scripts/libbox-ios-patches/apple-source-policy.json', 'src-tauri/core-manifest.json')})
    inputs.verify_hashes(hashes, helper)
    candidate = source_candidate(helper)
    evidence.mkdir()
    owned = OwnedPaths()
    scratch = owned.register(Path(tempfile.mkdtemp(prefix='polaris-c3-owned-', dir=evidence)))
    env = {key: os.environ[key] for key in ('HOME', 'PATH', 'GOCACHE', 'GOMODCACHE', 'GOPATH') if key in os.environ}
    env.update({'PATH': str(go.parent) + os.pathsep + env.get('PATH', '/usr/bin:/bin'), 'GOENV': 'off', 'GO111MODULE': 'on',
            'GOWORK': 'off', 'GOTOOLCHAIN': 'local', 'GOPROXY': 'off', 'GOSUMDB': 'off', 'GOFLAGS': '-mod=readonly',
            'TMPDIR': str(scratch), 'DEVELOPER_DIR': str(developer), 'POLARIS_NO_KERNEL_RUN': '1', 'GOEXPERIMENT': '',
            'GOARM64': 'v8.0', 'GOAMD64': 'v1'})
    flags = apple_flags(source_receipt, core, ios)
    executables = {str(go): 'go', str(mobile / 'gobind'): 'gobind', '/usr/bin/xcrun': 'xcrun',
                   str(developer / 'usr/bin/xcodebuild'): 'xcodebuild'}
    commands = Commands(evidence, env, executables, checkout, scratch, source_receipt, go)
    archive_commands = commands.archive_commands
    report = {'evidenceScope': 'C3-source-compiler-observations-only', 'status': 'active', 'sourceReceipt': source_receipt,
            'sourceReceiptFingerprint': source_receipt['fingerprint'], 'inputHashes': hashes, 'producerCandidate': candidate, 'targets': {},
            'carrierAdmission': False, 'pending': ['independent first actual carrier/native contract freeze',
            'complete Framework inventory and component policy', 'bound applicable source tests', 'C4 final static linkage'],
            'candidateCarrierContract': {'schema': 'polaris-apple-carrier-contract-v1', 'evidenceScope': 'carrier-inspection-only',
                'sourceReceiptFingerprint': source_receipt['fingerprint'], 'targets': {}}, 'generatedHeaders': {}}
    try:
        tools, support = inputs.observe_tools(commands, go, mobile, developer, shared, policy, helper, checkout)
        report['tools'] = tools
        for name in ('libtool', 'lipo', 'ld'):
            path = Path(commands.run(name + '-identity-path', ['/usr/bin/xcrun', '--find', name], checkout).strip())
            require(path.is_file() and path.resolve().is_relative_to(developer), 'assembly/linker tool escapes developer directory')
            tools[name] = {'path': str(path), 'resolved': str(path.resolve()), 'sha256': helper.file_hash(path)}
        tool_hashes = {tools[name]['path']: tools[name]['sha256'] for name in ('go', 'gomobile', 'gobind', 'xcode', 'xcrun', 'libtool', 'lipo', 'ld')}
        for sdk in ('iphoneos', 'iphonesimulator'):
            tool_hashes.update({tools[sdk][name]['path']: tools[sdk][name]['sha256'] for name in ('clang', 'clang++', 'swiftc')})
        umbrella, modulemap = inputs.render_headers((support / 'cmd/gomobile/bind_iosapp.go').read_text())
        generated = {}
        for name in ('ios', 'iossimulator'):
            out = scratch / name
            tags = inputs.effective_tags(policy, name == 'iossimulator')
            commands.run(name + '-gobind', [mobile / 'gobind', '-lang=go,objc', '-outdir=' + str(out),
                         '-tags=' + ','.join(tags), inputs.BOUND], checkout, dict(commands.env, GOOS='ios', CGO_ENABLED='1'))
            require((out / 'src/gobind/go_main.go').is_file() and (out / 'src/gobind/go_libboxmain.go').is_file(), 'generated main missing')
            require(not any(path.name in ('go.mod', 'go.sum', 'go.work', 'go.work.sum', '_cgo_export.h', 'vendor')
                            for path in out.rglob('*')), 'unexpected generated module/cgo header')
            for source, filename in [('bind/seq.go.support', 'seq.go'), ('bind/objc/seq_darwin.go.support', 'seq_darwin.go'),
                    ('bind/objc/seq_darwin.m.support', 'seq_darwin.m'), ('bind/objc/ref.h', 'ref.h'), ('bind/objc/seq_darwin.h', 'seq_darwin.h')]:
                require(helper.file_hash(out / 'src/gobind' / filename) == helper.file_hash(support / source), 'actual support source differs')
            generated[name] = out
            report[name + 'Generated'] = inputs.tree_identity(out, helper)
            layout = scratch / 'header-import' / name / 'Libbox.framework'
            (layout / 'Headers').mkdir(parents=True)
            (layout / 'Modules').mkdir()
            for filename in ('Libbox.objc.h', 'Universe.objc.h', 'ref.h'):
                shutil.copyfile(out / 'src/gobind' / filename, layout / 'Headers' / filename)
            (layout / 'Headers/Libbox.h').write_text(umbrella)
            (layout / 'Modules/module.modulemap').write_text(modulemap)
            report['generatedHeaders'][name] = {filename: helper.file_hash(layout / 'Headers' / filename)
                    for filename in ('Libbox.objc.h', 'Universe.objc.h', 'ref.h', 'Libbox.h')}
            report['generatedHeaders'][name]['module.modulemap'] = helper.file_hash(layout / 'Modules/module.modulemap')
        commands.configure_targets(tools, generated)
        witness = scratch / 'abi-witness.swift'
        witness.write_text(inputs.witness())
        consumers = [ROOT / 'src-tauri/gen/apple/PacketTunnel' / name for name in inputs.CONSUMERS]
        for index, (target_id, target) in enumerate(policy['targets'].items()):
            name = 'iossimulator' if target['variant'] else 'ios'
            out, sdk = generated[name], tools[target['sdk']]
            copy = checkout / 'build' / (name + '-' + target['goarch']) / 'Libbox'
            require(not copy.exists() and not copy.is_symlink(), 'target main copy already exists')
            for parent in (checkout / 'build', copy.parent):
                if not parent.exists() and not parent.is_symlink():
                    owned.mkdir(parent, empty_only=True)
                require(parent.is_dir() and not parent.is_symlink(), 'target parent redirected')
            owned.mkdir(copy)  # Pre-register before partial copy can fail.
            shutil.copytree(out / 'src/gobind', copy, symlinks=True, dirs_exist_ok=True)
            expected = inputs.target_env(commands.env, target, sdk['path'], sdk['clang']['path'], policy['binding']['iosMinimumVersion'], out)
            tags = inputs.effective_tags(policy, bool(target['variant']))
            observed = json.loads(commands.run(target_id + '-effective-go-env', [go, 'env', '-json'], copy, expected))
            compared = dict(expected)
            for key in ('GOOS', 'GOARCH', 'CGO_ENABLED', 'GOFLAGS', 'CC', 'CXX', 'CGO_CFLAGS', 'CGO_CPPFLAGS',
                    'CGO_CXXFLAGS', 'CGO_LDFLAGS', 'GOWORK', 'GOTOOLCHAIN', 'GOENV'):
                compared[key] = observed.get(key)
            inputs.validate_target_env(compared, expected)
            require(observed.get('GOARM64' if target['goarch'] == 'arm64' else 'GOAMD64') == ('v8.0' if target['goarch'] == 'arm64' else 'v1')
                    and observed.get('GOEXPERIMENT') == '', 'effective architecture/experiment default differs')
            raw = commands.run(target_id + '-graph', [go, 'list', '-deps', '-json', '-tags=' + ','.join(tags), '.'], copy, expected)
            graph = inputs.graph(raw, checkout, copy, source_receipt, shared, core, helper,
                                  tools['host']['GOMODCACHE'], tools['host']['GOROOT'], out)
            bound = next(row for row in graph['packages'] if row['ImportPath'] == inputs.BOUND)
            graph['headerDeclarations'] = inputs.check_headers((out / 'src/gobind/Libbox.objc.h').read_text(),
                    [(Path(bound['Dir']) / filename).read_text() for filename in bound.get('GoFiles', []) + bound.get('CgoFiles', [])])
            arch = 'x86_64' if target['goarch'] == 'amd64' else 'arm64'
            triple = arch + '-apple-ios' + policy['binding']['appMinimumVersion'] + ('-simulator' if target['variant'] else '')
            commands.run(target_id + '-swift-typecheck', [sdk['swiftc']['path'], '-typecheck', '-target', triple, '-sdk', sdk['path'],
                    '-F', scratch / 'header-import' / name, '-module-cache-path', scratch / 'clang-module-cache', *consumers, witness], checkout)
            graph.update(effectiveTags=tags, effectiveEnvironment=expected, actualGoEnvironment=observed,
                    headerSwiftTypecheckScope='generated-header-and-swift-typecheck', externalInputs={'sdk': target['sdk'],
                    'sdkVersion': sdk['version'], 'sdkBuild': sdk['buildVersion'], 'sysrootSha256': sdk['sysroot']['inventorySha256'],
                    'minOSRaw': inputs.packed_version(policy['binding']['iosMinimumVersion']), 'sdkRaw': inputs.packed_version(sdk['version']),
                    'environment': expected})
            report['targets'][target_id] = graph
            archive = scratch / (target_id + '.a')
            for attempt in range(2):
                commands.run(target_id + ('-archive' if attempt == 0 else '-archive-force-recompile'), archive_commands[index * 2 + attempt], copy, expected)
                log = (evidence / commands.rows[-1]['stderr']).read_text()
                for work in re.findall(r'^WORK=(.+)$', log, re.M):
                    path = Path(work).resolve(strict=True)
                    require(path.is_relative_to(scratch), 'WORK escaped own scratch')
                    if not any(row['path'] == str(path) for row in owned.rows):
                        owned.register(path)
                try:
                    compiled = compiler_facts(log, graph, scratch, helper)
                    break
                except RuntimeError as error:
                    if attempt:
                        raise
                    graph['forceRecompileReason'] = str(error)
            graph['compiler'] = compiled
            inputs.verify_hashes(graph['sourceHashes'], helper)
            extracted = commands.run(target_id + '-external-library-list', [go, 'list', '-deps', '-f',
                        '{{range .CgoLDFLAGS}}{{println .}}{{end}}', '-tags=' + ','.join(tags), inputs.BOUND], copy, expected)
            libraries = list(dict.fromkeys(row.strip() for row in extracted.splitlines() if row.strip().endswith('.a')))
            require(all(Path(path).is_absolute() for path in libraries), 'pinned external static path is relative; exact prerequisite pending')
            require(set(str(Path(path).resolve()) for path in libraries) == set(graph['externalStaticLibraries']),
                    'complete graph/bound-package static library set differs')
            commands.external_inputs[target_id] = libraries
            external = [{'path': str(Path(path).resolve()), 'sha256': helper.file_hash(path),
                         'container': carrier.container(Path(path).read_bytes(), helper)} for path in libraries]
            graph['externalStaticInputs'] = external
            thin = archive.read_bytes()
            graph['unmergedArchive'] = carrier.container(thin, helper)
            if external:
                merged = scratch / (target_id + '.merged.a')
                commands.run(target_id + '-merge', ['/usr/bin/xcrun', 'libtool', '-static', '-o', merged, archive, *libraries], copy, expected)
                # Ensure libtool conserved every code payload with multiplicity across all inputs.
                expected_members = Counter((m['sha256'], m['machO']['cpu'], m['machO']['subCPU']) for data in [thin] +
                    [Path(path).read_bytes() for path in libraries] for m in code_members(carrier.container(data, helper)))
                actual_members = Counter((m['sha256'], m['machO']['cpu'], m['machO']['subCPU']) for m in
                    code_members(carrier.container(merged.read_bytes(), helper)))
                require(expected_members == actual_members, 'libtool native member multiset differs')
                archive = merged
            inputs.verify_hashes({row['path']: row['sha256'] for row in external}, helper)
            native = native_map(archive.read_bytes(), external, compiled, helper)
            graph['nativeMap'] = native
            contract_row, checked = observed_contract(archive.read_bytes(), target_id, graph, native, source_receipt, shared, core, helper)
            graph['carrierFacts'] = checked
            target_contract = {'schema': 'polaris-apple-carrier-contract-v1', 'evidenceScope': 'carrier-inspection-only',
                         'sourceReceiptFingerprint': source_receipt['fingerprint'], 'targets': {target_id: contract_row}}
            target_tools = {'targets': {target_id: contract_row['externalInputs']}}
            original_path = scratch / (target_id + '.a')
            graph['unmergedCarrierFacts'] = carrier.inspect_carrier(original_path, [target_id], source_receipt, shared, core, target_contract, target_tools)
            require(carrier.inspect_carrier(archive, [target_id], source_receipt, shared, core, target_contract, target_tools) == checked,
                    'fresh/cache carrier facts differ')
            graph['archivePath'] = str(archive)
            graph['archiveSha256'] = helper.file_hash(archive)
            report['candidateCarrierContract']['targets'][target_id] = contract_row
            inputs.verify_hashes(tool_hashes, helper)
            inputs.verify_hashes(hashes, helper)
        simulator = scratch / 'simulator-fat.a'
        commands.run('simulator-fat', ['/usr/bin/xcrun', 'lipo', report['targets']['ios-arm64-simulator']['archivePath'],
                    report['targets']['ios-x86_64-simulator']['archivePath'], '-create', '-output', simulator], checkout)
        contract = report['candidateCarrierContract']
        report['simulatorCarrierFacts'] = carrier.inspect_carrier(simulator, ['ios-arm64-simulator', 'ios-x86_64-simulator'],
                    source_receipt, shared, core, contract, {'targets': {target: row['externalInputs'] for target, row in contract['targets'].items()}})
        for name in ('ios', 'iossimulator'):
            require(inputs.tree_identity(generated[name], helper) == report[name + 'Generated'], 'generated source drift')
        require(inputs.tree_identity(tools['host']['GOROOT'], helper) == tools['go']['distribution'], 'Go distribution drift')
        require(inputs.tree_identity(support, helper) == tools['gomobileSource']['inventory'], 'mobile support drift')
        for name in ('iphoneos', 'iphonesimulator'):
            require(inputs.tree_identity(tools[name]['path'], helper) == tools[name]['sysroot'], 'SDK drift')
        inputs.verify_hashes(hashes, helper)
        require(source_candidate(helper) == candidate, 'producer candidate drift')
        report['status'] = 'observed-awaiting-independent-contract'
    except BaseException as error:
        report.update(status='failed', error=str(error))
        raise
    finally:
        report['commands'] = commands.rows
        report['rawLogHashes'] = {path.name: helper.file_hash(path) for path in evidence.iterdir() if path.suffix in ('.stdout', '.stderr')}
        report['cleanup'] = {'registeredProcessGroups': [row['registeredProcessGroup'] for row in commands.rows if 'registeredProcessGroup' in row],
                'processGroupsDrained': commands.cleanup_allowed, 'retainedOwnedDirectories': owned.rows,
                'pendingIndependentReview': True, 'pendingCleanup': True, 'sharedCachesRemoved': False}
        report['appleInputFingerprint'] = input_fingerprint(report)
        (evidence / 'component-observations.json').write_text(json.dumps(report, sort_keys=True, indent=2) + '\n')
    return report


class AssemblyCommands(Commands):
    """M1-only fixed assembly allowance; source observer never constructs this class."""
    def _allowed_command(self, args, cwd, env):
        if super()._allowed_command(args, cwd, env):
            return True
        if Path(cwd).resolve() != ROOT or env != self.env:
            return False
        if args[0] == '/usr/bin/xcrun' and args[1] == 'lipo':
            for name, targets in [('ios', ['ios-arm64']), ('iossimulator', ['ios-arm64-simulator', 'ios-x86_64-simulator'])]:
                expected = ['/usr/bin/xcrun', 'lipo', *[str(self.scratch / (target + '.a')) for target in targets],
                            '-create', '-output', str(self.scratch / name / 'Libbox.framework/Versions/A/Libbox')]
                if args == expected:
                    return True
        return (self.executables.get(args[0]) == 'xcodebuild' and args[1:] == ['-create-xcframework', '-framework',
                str(self.scratch / 'ios/Libbox.framework'), '-framework', str(self.scratch / 'iossimulator/Libbox.framework'),
                '-output', str(self.scratch / 'Libbox.xcframework')])


def assemble_component(apple_input, source_receipt, build_policy, tools, evidence):
    """M1 source-only Framework assembly after Root's common freeze; never an App."""
    ios, helper, policy = compiler_preflight(source_receipt, apple_input, build_policy, tools)
    require(source_candidate(helper) == {key: build_policy['producer'][key] for key in ('candidate', 'tree')}, 'frozen M1 candidate differs')
    verify_producer_code(build_policy, helper)
    verify_compiler_inputs(apple_input, helper)
    tool_facts = apple_input['tools']
    go = Path(tool_facts['go']['path'])
    developer = Path(apple_input['commands'][0]['env']['DEVELOPER_DIR'])
    evidence = Path(evidence).absolute()
    evidence = evidence.parent.resolve(strict=True) / evidence.name
    require(not evidence.exists() and not evidence.is_symlink() and not evidence.is_relative_to(ROOT), 'M1 evidence must be fresh outside source')
    evidence.mkdir()
    scratch = Path(tempfile.mkdtemp(prefix='polaris-c3-assembly-', dir=evidence))
    env = dict(apple_input['commands'][0]['env'], TMPDIR=str(scratch))
    commands = AssemblyCommands(evidence, env, {str(developer / 'usr/bin/xcodebuild'): 'xcodebuild', '/usr/bin/xcrun': 'xcrun'},
                                ROOT, scratch)
    frameworks = []
    try:
        for name, target_ids in [('ios', ['ios-arm64']), ('iossimulator', ['ios-arm64-simulator', 'ios-x86_64-simulator'])]:
            layout = scratch / name / 'Libbox.framework'
            version = layout / 'Versions/A'
            for folder in ('Headers', 'Modules', 'Resources'):
                (version / folder).mkdir(parents=True)
            os.symlink('A', layout / 'Versions/Current')
            for filename in ('Libbox', 'Headers', 'Modules', 'Resources'):
                os.symlink('Versions/Current/' + filename, layout / filename)
            source = Path(apple_input[name + 'Generated']['root']) / 'src/gobind'
            template_source = (Path(tool_facts['gomobileSource']['module']['Dir']) / 'cmd/gomobile/bind_iosapp.go').read_text()
            umbrella, modulemap = inputs.render_headers(template_source)
            for filename in ('Libbox.objc.h', 'Universe.objc.h', 'ref.h'):
                shutil.copyfile(source / filename, version / 'Headers' / filename)
            (version / 'Headers/Libbox.h').write_text(umbrella)
            (version / 'Modules/module.modulemap').write_text(modulemap)
            template = re.search(r'const appleBindInfoPlist = `(.*?)`', template_source, re.S)
            require(template, 'pinned Framework resource template missing')
            (version / 'Resources/Info.plist').write_bytes(template[1].encode())
            # M1 copies verified retained archives into its own scratch; no archive writer touches these.
            archives = []
            for target_id in target_ids:
                archive = scratch / (target_id + '.a')
                shutil.copyfile(apple_input['targets'][target_id]['archivePath'], archive)
                require(helper.file_hash(archive) == apple_input['targets'][target_id]['archiveSha256'], 'retained archive copy differs')
                archives.append(archive)
            commands.run(name + '-framework-lipo', ['/usr/bin/xcrun', 'lipo', *archives, '-create', '-output', version / 'Libbox'], ROOT)
            frameworks.append(layout.resolve())
        output = scratch / 'Libbox.xcframework'
        commands.run('create-xcframework', [developer / 'usr/bin/xcodebuild', '-create-xcframework', '-framework', frameworks[0],
                     '-framework', frameworks[1], '-output', output], ROOT)
        verify_compiler_inputs(apple_input, helper)
        require(source_candidate(helper) == {key: build_policy['producer'][key] for key in ('candidate', 'tree')}, 'M1 candidate drift')
        verify_producer_code(build_policy, helper)
        assembly = {'commands': commands.rows, 'rawLogHashes': {path.name: helper.file_hash(path) for path in evidence.iterdir()
                    if path.suffix in ('.stdout', '.stderr')}}
        if not build_policy['componentInventory'] or not build_policy['assembly']:
            return {'stagedGeneration': str(scratch), 'componentInventory': inventory(output), 'assembly': assembly,
                    'pending': ['independent first actual complete Framework inventory freeze'], 'published': False}
        require(build_policy['assembly'] == assembly, 'actual M1 assembly differs from frozen policy')
        receipt = make_receipt(output, source_receipt, apple_input, build_policy, tools)
        (scratch / 'libbox-build-receipt.json').write_text(json.dumps(receipt, sort_keys=True, indent=2) + '\n')
        return {'stagedGeneration': str(scratch), 'receipt': receipt, 'commands': commands.rows}
    finally:
        # Retain final Framework/input pair for Root review, then explicit publisher uses it.
        (evidence / 'assembly-commands.json').write_text(json.dumps(commands.rows, sort_keys=True, indent=2) + '\n')
