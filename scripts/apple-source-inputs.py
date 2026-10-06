#!/usr/bin/env python3
"""Explicit C2 source observations; no carrier, Framework, App or runtime admission."""
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent.parent
CONSUMERS = {
    'PacketTunnelProvider.swift': 'c122623edf2923984149314e531c74d8d109e64786b623daed2f6134d062527e',
    'TunnelLifecycle.swift': 'cd8dac4cd1fdf9b1d2c52d856c29e179023886b364ea24c348d3a385c58b4210',
    'TunnelMonitorSession.swift': '8d20d4a9f2a4a170ef5d553f2e10679993d4236fd409d1d9b9b803c51cae1ed3',
    'TunnelPlatform.swift': '76d8d689e7a59c1fb89cb051cac08e140c0f1196831a700a5bb83541b8865d5b',
}
GETTERS = ('RequestID', 'ConfigDigest', 'ContractVersion', 'Validation', 'Cleanup', 'ValidationError', 'CleanupError')
BOUND = 'github.com/sagernet/sing-box/experimental/libbox'
C3_REQUIRED = ('actual source carrierCount', 'actual parsed fullBuildInfo', 'actual native no-platform member hashes',
               'actual generated/support/reverse ObjC and complete generated-main Go/cgo c-archive compilation')


def require(ok, message):
    if not ok:
        raise RuntimeError('Apple source inputs: ' + message)


def json_stream(raw):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, 'duplicate JSON field')
            result[key] = value
        return result
    decoder = json.JSONDecoder(object_pairs_hook=unique)
    rows, offset = [], 0
    while offset < len(raw):
        if raw[offset:].isspace():
            break
        offset += len(raw[offset:]) - len(raw[offset:].lstrip())
        try:
            row, offset = decoder.raw_decode(raw, offset)
        except ValueError as error:
            raise RuntimeError('Apple source inputs: truncated/invalid JSON stream') from error
        require(isinstance(row, dict), 'non-object JSON record')
        rows.append(row)
    require(rows, 'empty JSON stream')
    return rows


def tree_identity(directory, helper):
    """C0 inventory algorithm: record symlinks without traversing external targets."""
    directory = Path(directory).resolve(strict=True)
    rows, total = [], 0
    for current, dirs, files in os.walk(directory, followlinks=False):
        dirs.sort()
        files.sort()
        for name in dirs + files:
            path = Path(current) / name
            relative = path.relative_to(directory).as_posix()
            if path.is_symlink():
                rows.append({'path': relative, 'symlink': os.readlink(path), 'resolved': str(path.resolve())})
            elif path.is_file():
                size = path.stat().st_size
                total += size
                rows.append({'path': relative, 'bytes': size, 'sha256': helper.file_hash(path),
                             'mode': path.stat().st_mode & 0o777})
    require(rows, 'empty directory inventory')
    return {'root': str(directory), 'entries': len(rows), 'regularBytes': total,
            'inventorySha256': helper.digest(helper.canonical(rows)), 'files': rows}


def effective_tags(policy, simulator):
    return policy['binding']['buildTags'] + ['ios'] + (['iossimulator'] if simulator else []) + policy['binding']['nonMacOSTags']


def target_env(base, target, sdk, clang, minimum, out):
    arch = 'x86_64' if target['goarch'] == 'amd64' else 'arm64'
    require(target['sdk'] == ('iphonesimulator' if target['variant'] == 'simulator' else 'iphoneos'), 'target SDK/variant differs')
    flag = '-mios-simulator-version-min=' if target['variant'] else '-miphoneos-version-min='
    flags = f'-isysroot {sdk} {flag}{minimum} -fembed-bitcode -arch {arch}'
    require(not any(c.isspace() for c in str(sdk)), 'SDK path unsupported by pinned gomobile flag quoting')
    env = dict(base)
    env.update({'GOOS': 'ios', 'GOARCH': target['goarch'], 'CGO_ENABLED': '1',
                'GOFLAGS': '-mod=readonly -tags=ios' + (',iossimulator' if target['variant'] else ''),
                'CC': str(clang), 'CXX': str(clang) + '++', 'CGO_CFLAGS': flags, 'CGO_CXXFLAGS': flags,
                'CGO_LDFLAGS': flags, 'CGO_CPPFLAGS': '', 'DARWIN_SDK': target['sdk'],
                'GOPATH': str(out) + os.pathsep + base['GOPATH']})
    return env


def validate_target_env(env, expected):
    for key in ('GOOS', 'GOARCH', 'CGO_ENABLED', 'GOFLAGS', 'CC', 'CXX', 'CGO_CFLAGS', 'CGO_CPPFLAGS',
                'CGO_CXXFLAGS', 'CGO_LDFLAGS', 'DARWIN_SDK', 'GOPATH', 'GOWORK', 'GOTOOLCHAIN'):
        require(env.get(key) == expected.get(key), 'target environment differs: ' + key)
    # Pinned Go reports the disabled GOENV file as an empty string, not the command input "off".
    require(expected.get('GOENV') == 'off' and env.get('GOENV') == '', 'target environment differs: GOENV')


def packed_version(value):
    require(isinstance(value, str) and re.fullmatch(r'\d+\.\d+(?:\.\d+)?', value), 'invalid SDK/minOS version')
    parts = [int(part) for part in value.split('.')]
    parts += [0] * (3 - len(parts))
    require(parts[0] <= 65535 and parts[1] <= 255 and parts[2] <= 255, 'SDK/minOS version overflow')
    return (parts[0] << 16) | (parts[1] << 8) | parts[2]


class Commands:
    """One bounded observer log. Only fixed source-tool operations are executable."""
    def __init__(self, evidence, env, executables):
        self.evidence, self.env, self.executables = evidence, env, executables
        self.rows = []
        self.started = time.monotonic()
        self.cleanup_allowed = True

    def group_exists(self, pgid):
        try:
            os.killpg(pgid, 0)
            return True
        except ProcessLookupError:
            return False

    def stop_group(self, child, row):
        # Only our own successful start_new_session spawn establishes this PGID.
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
        self.cleanup_allowed = all(item.get('groupDrained') is not False for item in self.rows)

    def run(self, name, args, cwd, env=None):
        args = [str(arg) for arg in args]
        require(len(args) >= 2, 'forbidden tool operation')
        kind = self.executables.get(args[0])
        go_args = args[1:]
        swift_sources = [str(ROOT / 'src-tauri/gen/apple/PacketTunnel' / name) for name in CONSUMERS]
        swift_allowed = (len(args) == 15 and args[1:3] == ['-typecheck', '-target']
                         and args[3] in ('arm64-apple-ios17.0', 'arm64-apple-ios17.0-simulator', 'x86_64-apple-ios17.0-simulator')
                         and args[4] == '-sdk' and args[6] == '-F' and args[8] == '-module-cache-path'
                         and args[10:14] == swift_sources and Path(args[14]).name == 'abi-witness.swift')
        allowed = ((kind == 'go' and (go_args == ['version'] or (len(go_args) == 3 and go_args[:2] == ['version', '-m'])
                   or go_args == ['env', '-json', 'GOROOT', 'GOPATH', 'GOMODCACHE', 'GOCACHE', 'GOHOSTOS', 'GOHOSTARCH', 'GOOS', 'GOARCH']
                   or go_args == ['env', '-json']
                   or go_args == ['list', '-m', '-json', 'github.com/sagernet/gomobile']
                   or go_args == ['list', '-json', 'github.com/sagernet/gomobile/bind', 'github.com/sagernet/gomobile/bind/objc']
                   or (len(go_args) == 5 and go_args[:3] == ['list', '-deps', '-json'] and go_args[3].startswith('-tags=') and go_args[4] == '.')))
                   or (kind == 'gobind' and len(args) == 5 and args[1] == '-lang=go,objc'
                       and args[2].startswith('-outdir=') and args[3].startswith('-tags=') and args[4] == BOUND)
                   or (kind == 'swiftc' and (swift_allowed or args[1:] == ['--version']))
                   or (kind == 'clang' and args[1:] == ['--version'])
                   or (kind == 'xcodebuild' and args[1:] == ['-version'])
                   or (kind == 'xcrun' and len(args) == 4 and args[1] == '--sdk'
                       and args[2] in ('iphoneos', 'iphonesimulator')
                       and args[3] in ('--show-sdk-path', '--show-sdk-version', '--show-sdk-build-version'))
                   or (kind == 'xcrun' and args[1:3] in (['--sdk', 'iphoneos'], ['--sdk', 'iphonesimulator'])
                       and args[3:] in (['--find', 'clang'], ['--find', 'swiftc'])))
        require(allowed, 'forbidden tool operation')
        require(self.cleanup_allowed, 'process group drain unknown; source tools stopped')
        require(time.monotonic() - self.started < 45 * 60, 'source window time budget exhausted')
        require(shutil.disk_usage(self.evidence).free >= 5 * 1024**3, 'less than 5 GiB free; source tools stopped')
        env = env or self.env
        index = len(self.rows) + 1
        row = {'name': name, 'argv': args, 'cwd': str(cwd), 'env': dict(env), 'exit': None,
               'stdout': f'{index:03d}.stdout', 'stderr': f'{index:03d}.stderr'}
        self.rows.append(row)
        try:
            with (self.evidence / row['stdout']).open('xb') as stdout, (self.evidence / row['stderr']).open('xb') as stderr:
                child = subprocess.Popen(args, cwd=cwd, env=env, stdout=stdout, stderr=stderr,
                                         close_fds=True, start_new_session=True)
                row['registeredProcessGroup'] = child.pid
                row['groupDrained'], self.cleanup_allowed = False, False
                try:
                    row['exit'] = child.wait(timeout=min(900, max(1, 2700 - int(time.monotonic() - self.started))))
                except BaseException as error:
                    self.stop_group(child, row)
                    if isinstance(error, subprocess.TimeoutExpired):
                        row['timeout'] = True
                        raise RuntimeError('Apple source inputs: tool failed: ' + name) from error
                    raise
                try:
                    remaining = self.group_exists(row['registeredProcessGroup'])
                except OSError as error:
                    row['groupObservationError'] = str(error)
                    remaining = True
                if remaining:
                    row['groupPresentAfterParentExit'] = True
                    self.stop_group(child, row)
                    raise RuntimeError('Apple source inputs: descendants remain after tool parent exit: ' + name)
                row['groupDrained'] = True
                self.cleanup_allowed = True
            require(row['exit'] == 0, 'tool failed: ' + name)
            return (self.evidence / row['stdout']).read_text()
        except (OSError, RuntimeError) as error:
            row['error'] = str(error)
            if isinstance(error, OSError):
                raise RuntimeError('Apple source inputs: tool failed: ' + name) from error
            raise
        finally:
            (self.evidence / 'commands.json').write_text(json.dumps(self.rows, sort_keys=True, indent=2) + '\n')


def observe_tools(commands, go, mobile, developer, shared, policy, helper, checkout):
    version = commands.run('go-version', [go, 'version'], checkout)
    require(f" go{shared['goVersion']} darwin/arm64" in version, 'pinned host Go differs')
    host = json.loads(commands.run('go-host', [go, 'env', '-json', 'GOROOT', 'GOPATH', 'GOMODCACHE', 'GOCACHE',
                                 'GOHOSTOS', 'GOHOSTARCH', 'GOOS', 'GOARCH'], checkout))
    require((host['GOHOSTOS'], host['GOHOSTARCH'], host['GOOS'], host['GOARCH']) == ('darwin', 'arm64', 'darwin', 'arm64'),
            'generation host/ambient architecture differs')
    commands.env.update({key: host[key] for key in ('GOPATH', 'GOMODCACHE', 'GOCACHE')})
    tools = {'go': {'path': str(go), 'sha256': helper.file_hash(go), 'version': version,
                    'distribution': tree_identity(host['GOROOT'], helper)}, 'host': host}
    for name in ('gomobile', 'gobind'):
        binary = mobile / name
        info = helper.parse_build_info(commands.run(name + '-buildinfo', [go, 'version', '-m', binary], checkout))
        module = info['modules'].get('github.com/sagernet/gomobile', {})
        require(info['goVersion'] == shared['goVersion'] and info['path'] == 'github.com/sagernet/gomobile/cmd/' + name
                and module.get('kind') == 'mod' and module.get('version') == policy['binding']['gomobileVersion']
                and not any('replacement' in value for value in info['modules'].values()), 'pinned mobile tool differs')
        tools[name] = {'path': str(binary), 'resolved': str(binary.resolve()), 'sha256': helper.file_hash(binary), 'buildInfo': info}
    raw = commands.run('gomobile-source', [go, 'list', '-m', '-json', 'github.com/sagernet/gomobile'], checkout)
    module = json_stream(raw)
    require(len(module) == 1 and module[0].get('Path') == 'github.com/sagernet/gomobile'
            and module[0].get('Version') == policy['binding']['gomobileVersion']
            and not module[0].get('Error') and 'Replace' not in module[0], 'gomobile support module differs')
    support = Path(module[0]['Dir']).resolve(strict=True)
    require(support.is_relative_to(Path(host['GOMODCACHE']).resolve())
            and Path(module[0]['GoMod']).is_file()
            and Path(module[0]['GoMod']).resolve().is_relative_to(Path(host['GOMODCACHE']).resolve()),
            'support source escapes actual module cache')
    require(module[0].get('Sum') == tools['gobind']['buildInfo']['modules']['github.com/sagernet/gomobile'].get('sum'),
            'support module/tool checksum differs')
    support_packages = json_stream(commands.run('actual-support-package-dirs', [go, 'list', '-json',
                                   'github.com/sagernet/gomobile/bind', 'github.com/sagernet/gomobile/bind/objc'], checkout))
    require(len(support_packages) == 2 and {row.get('ImportPath') for row in support_packages} == {
            'github.com/sagernet/gomobile/bind', 'github.com/sagernet/gomobile/bind/objc'}, 'support package inventory differs')
    for row in support_packages:
        package_module = row.get('Module', {})
        expected = support / row['ImportPath'].removeprefix('github.com/sagernet/gomobile/')
        require(not row.get('Error') and not row.get('DepsErrors') and Path(row['Dir']).resolve() == expected
                and package_module.get('Path') == 'github.com/sagernet/gomobile'
                and package_module.get('Version') == policy['binding']['gomobileVersion']
                and 'Replace' not in package_module, 'actual support package source differs')
    tools['gomobileSource'] = {'module': module[0], 'packageDirs': support_packages, 'inventory': tree_identity(support, helper)}
    tools['xcode'] = {'path': str(developer / 'usr/bin/xcodebuild'), 'sha256': helper.file_hash(developer / 'usr/bin/xcodebuild'),
                      'version': commands.run('xcode-version', [developer / 'usr/bin/xcodebuild', '-version'], checkout)}
    tools['xcrun'] = {'path': '/usr/bin/xcrun', 'sha256': helper.file_hash('/usr/bin/xcrun')}
    for sdk in ('iphoneos', 'iphonesimulator'):
        path = Path(commands.run(sdk + '-path', ['/usr/bin/xcrun', '--sdk', sdk, '--show-sdk-path'], checkout).strip())
        require(path.is_absolute() and path.resolve().is_relative_to(developer.resolve()), 'SDK escapes developer directory')
        sdk_version = commands.run(sdk + '-version', ['/usr/bin/xcrun', '--sdk', sdk, '--show-sdk-version'], checkout).strip()
        sdk_build = commands.run(sdk + '-build', ['/usr/bin/xcrun', '--sdk', sdk, '--show-sdk-build-version'], checkout).strip()
        require(re.fullmatch(r'\d+\.\d+(?:\.\d+)?', sdk_version) and sdk_build, 'invalid actual SDK identity')
        tools[sdk] = {'path': str(path), 'version': sdk_version, 'buildVersion': sdk_build,
                      'sysroot': tree_identity(path, helper)}
        for name in ('clang', 'swiftc'):
            binary = Path(commands.run(sdk + '-' + name, ['/usr/bin/xcrun', '--sdk', sdk, '--find', name], checkout).strip())
            require(binary.is_file() and binary.resolve().is_relative_to(developer.resolve()), 'compiler escapes developer directory')
            commands.executables[str(binary)] = name
            tools[sdk][name] = {'path': str(binary), 'resolved': str(binary.resolve()), 'sha256': helper.file_hash(binary)}
        tools[sdk]['clang']['version'] = commands.run(sdk + '-clang-version', [tools[sdk]['clang']['path'], '--version'], checkout)
        tools[sdk]['swiftc']['version'] = commands.run(sdk + '-swift-version', [tools[sdk]['swiftc']['path'], '--version'], checkout)
        cxx = Path(tools[sdk]['clang']['path'] + '++')
        require(cxx.is_file() and cxx.resolve().is_relative_to(developer.resolve()), 'CXX compiler missing/escapes')
        tools[sdk]['clang++'] = {'path': str(cxx), 'resolved': str(cxx.resolve()), 'sha256': helper.file_hash(cxx)}
    return tools, support


def graph(raw, checkout, generated, receipt, shared, core, helper, module_cache, goroot, platform_out):
    packages = json_stream(raw)
    by_path, selected, modules, inputs = {}, [], {}, {}
    main_path = re.search(r'^module\s+(\S+)\s*$', (checkout / 'go.mod').read_text(), re.M)
    require(main_path, 'main module declaration missing')
    main_path = main_path[1]
    require(helper.file_hash(checkout / 'go.mod') == receipt['mainGoModSha256']
            and helper.file_hash(checkout / 'go.sum') == receipt['mainGoSumSha256'], 'main GoMod/GoSum hash differs')
    replacements = {dep['module']: (checkout / 'polaris-dependencies' / dep['name']).resolve() for dep in shared['dependencyPatches']}
    roots = [checkout.resolve(), Path(module_cache).resolve(), Path(goroot).resolve(), platform_out.resolve()]
    for row in packages:
        require(not row.get('Error') and not row.get('DepsErrors') and not row.get('Incomplete'), 'package graph has errors')
        name = row.get('ImportPath')
        require(isinstance(name, str) and name and name not in by_path, 'duplicate/missing package path')
        by_path[name] = row
        directory = Path(row.get('Dir', '')).resolve(strict=True)
        require(any(directory.is_relative_to(root) for root in roots), 'package directory escapes source roots')
        if directory == generated.resolve():
            selected.append(row)
        module = row.get('Module')
        if module:
            require(isinstance(module, dict) and isinstance(module.get('Path'), str), 'invalid package module')
            path = module['Path']
            require(path not in modules or helper.canonical(modules[path]) == helper.canonical(module), 'duplicate module identity differs')
            modules[path] = module
            if module.get('Main'):
                require(path == main_path and Path(module['Dir']).resolve() == checkout.resolve()
                        and Path(module['GoMod']).resolve() == (checkout / 'go.mod').resolve(), 'main module binding differs')
            elif path in replacements:
                replace = module.get('Replace', {})
                require(module.get('Version') == next(dep['upstreamVersion'] for dep in shared['dependencyPatches'] if dep['module'] == path)
                        and replace.get('Path') == './polaris-dependencies/' + replacements[path].name
                        and Path(replace.get('Dir', '')).resolve() == replacements[path]
                        and Path(replace.get('GoMod', '')).resolve() == replacements[path] / 'go.mod', 'patched replacement differs')
                require(directory.is_relative_to(replacements[path]), 'replacement package escapes source')
            else:
                require('Replace' not in module and Path(module['Dir']).resolve().is_relative_to(Path(module_cache).resolve())
                        and directory.is_relative_to(Path(module['Dir']).resolve()), 'unexpected module replacement/source')
                if path in core['sourceBuild']['transportPins']:
                    require(module.get('Version') == core['sourceBuild']['transportPins'][path], 'transport version differs')
            mod_file = Path(module.get('Replace', module).get('GoMod', ''))
            require(mod_file.is_file(), 'module GoMod missing')
            inputs[str(mod_file.resolve())] = helper.file_hash(mod_file)
        else:
            require(row.get('Standard') or directory.is_relative_to(platform_out.resolve()), 'package module identity missing')
        for field in ('GoFiles', 'CgoFiles', 'CFiles', 'CXXFiles', 'MFiles', 'HFiles', 'SFiles', 'SysoFiles', 'IgnoredGoFiles'):
            files = row.get(field, [])
            require(isinstance(files, list), 'invalid selected source inventory')
            for filename in files:
                require(isinstance(filename, str) and Path(filename).name == filename, 'source file path escapes package')
                file = directory / filename
                require(file.is_file() and file.resolve().is_relative_to(directory), 'selected source file missing/escapes')
                inputs[str(file.resolve())] = helper.file_hash(file)
        require(isinstance(row.get('Imports', []), list), 'invalid package import edges')
    require(len(selected) == 1 and selected[0].get('Name') == 'main' and selected[0].get('Module', {}).get('Main'), 'generated main missing/duplicate')
    reachable, pending = set(), [selected[0]['ImportPath']]
    while pending:
        name = pending.pop()
        if name in reachable or name == 'C':
            continue
        require(name in by_path, 'truncated dependency graph')
        reachable.add(name)
        pending.extend(by_path[name].get('Imports', []))
    require(reachable == set(by_path) and BOUND in by_path, 'graph contains unreachable packages or misses bound package')
    fingerprint = helper.digest(helper.canonical(packages))
    partitions, absence = {}, {}
    for label, inventory, absent_key in [('patchedModules', core['sourceBuild']['dependencyModules'], 'allowedAbsent'),
                                         ('transportModules', sorted(core['sourceBuild']['transportPins']), 'confirmedAbsent')]:
        partition = {'requiredLinked': sorted(set(inventory) & set(modules)), absent_key: sorted(set(inventory) - set(modules))}
        helper.partition(partition, inventory, absent_key)
        partitions[label] = partition
        for path in partition[absent_key]:
            absence[path] = {'basis': 'absent from complete generated-main reachable package Module inventory',
                             'graphFingerprint': fingerprint, 'selectedSourceHashesFingerprint': helper.digest(helper.canonical(inputs))}
    libraries = {}
    for row in packages:
        flags = row.get('CgoLDFLAGS', [])
        require(isinstance(flags, list) and all(isinstance(flag, str) for flag in flags), 'invalid CgoLDFLAGS')
        for flag in flags:
            if flag.endswith('.a'):
                file = Path(flag)
                if not file.is_absolute():
                    file = Path(row['Dir']) / file
                require(file.is_file(), 'external static library missing')
                libraries[str(file.resolve())] = {'sha256': helper.file_hash(file), 'package': row['ImportPath']}
                inputs[str(file.resolve())] = libraries[str(file.resolve())]['sha256']
    return {'packages': packages, 'graphFingerprint': fingerprint, 'main': selected[0], **partitions,
            'absenceEvidence': absence, 'sourceHashes': inputs, 'externalStaticLibraries': libraries}


def clean_comments(text):
    return re.sub(r'/\*.*?\*/|//[^\n]*', '', text, flags=re.S)


def check_headers(header, selected_go_sources):
    """Finite generated declaration checks; real Swift compiler remains authoritative."""
    header = clean_comments(header)
    blocks = {}
    for kind, name, body in re.findall(r'@(interface|protocol)\s+(Libbox\w+)[^;\n]*\n(.*?)@end', header, re.S):
        require((kind, name) not in blocks, 'duplicate header declaration')
        blocks[kind, name] = body
    result = blocks.get(('interface', 'LibboxConfigValidationResult'), '')
    for getter in GETTERS:
        require(re.search(r'-\s*\(NSString\*\s+_Nonnull\)get' + getter + r'\s*;', result), 'getter type/nullability missing: ' + getter)
        require(not re.search(r'\bset' + getter + r'\s*:', result), 'validation setter forbidden')
    require(re.search(r'FOUNDATION_EXPORT\s+NSString\*\s+_Nonnull\s+const\s+LibboxConfigValidationContractVersion\s*;', header), 'contract constant differs')
    for factory in ('NewStrictCommandServer', 'NewTransientCommandServer'):
        declaration = re.search(r'FOUNDATION_EXPORT\s+([^;]+\bLibbox' + factory + r'\([^;]+\))\s*;', header)
        require(declaration and re.fullmatch(r'LibboxCommandServer\*\s+_Nullable\s+Libbox' + factory +
                r'\(id<LibboxCommandServerHandler>\s+_Nullable\s+\w+,\s*id<LibboxPlatformInterface>\s+_Nullable\s+\w+,\s*NSError\*\s+_Nullable\*\s+_Nullable\s+\w+\)', declaration[1]), 'factory type/nullability differs: ' + factory)
    require(re.search(r'FOUNDATION_EXPORT\s+LibboxConfigValidationResult\*\s+_Nullable\s+LibboxCheckConfigWithResult\(NSString\*\s+_Nullable\s+\w+,\s*NSString\*\s+_Nullable\s+\w+,\s*int64_t\s+\w+\)\s*;', header), 'validation factory differs')
    require(re.search(r'FOUNDATION_EXPORT\s+NSString\*\s+_Nonnull\s+LibboxInterfaceUpdateListenerIdentity\(id<LibboxInterfaceUpdateListener>\s+_Nullable\s+\w+\)\s*;', header), 'listener identity differs')
    go_source = clean_comments('\n'.join(selected_go_sources))
    for protocol in ('PlatformInterface', 'CommandServerHandler', 'LocalDNSTransport', 'Func', 'InterfaceUpdateListener'):
        go = re.search(r'type\s+' + protocol + r'\s+interface\s*\{([^}]+)\}', go_source)
        body = blocks.get(('protocol', 'Libbox' + protocol), '')
        require(go and body, 'complete protocol source/header missing: ' + protocol)
        methods = re.findall(r'^\s*([A-Z]\w*)\s*\(', go[1], re.M)
        require(methods, 'empty protocol method inventory')
        for method in methods:
            require(re.search(r'-\s*\([^)]*\)' + method[0].lower() + method[1:] + r'(?=[:;\s])', body), 'protocol method missing: ' + protocol + '.' + method)
    require(re.search(r'-\s*\(void\)onCancel:\(id<LibboxFunc>\s+_Nullable\)\w+\s*;', blocks.get(('interface', 'LibboxExchangeContext'), '')), 'OnCancel declaration differs')
    return {'readOnlyGetters': list(GETTERS), 'fullProtocolsCheckedFromSelectedSource': True,
            'scope': 'generated-header-declarations-only; Swift typecheck required'}


def render_headers(support_source):
    templates = {}
    for name in ('appleBindHeaderTmpl', 'appleModuleMapTmpl'):
        match = re.search(r'var ' + name + r' = template.Must\(template.New\("[^"]+"\).Parse\(`(.*?)`\)\)', support_source, re.S)
        require(match, 'pinned gomobile header template missing')
        templates[name] = match[1]
    umbrella = templates['appleBindHeaderTmpl'].replace('{{range .pkgs}}//\t{{.PkgPath}}\n{{end}}', '//\t' + BOUND + '\n')
    umbrella = umbrella.replace('{{range .bases}}#include "{{.}}.objc.h"\n{{end}}', '#include "Libbox.objc.h"\n#include "Universe.objc.h"\n').replace('{{.title}}', 'Libbox')
    module = templates['appleModuleMapTmpl'].replace('{{.Module}}', 'Libbox').replace('{{range .Headers}}    header "{{.}}"\n{{end}}', '    header "Libbox.objc.h"\n    header "Universe.objc.h"\n    header "Libbox.h"\n')
    require('{{' not in umbrella + module, 'unsupported pinned header template')
    return umbrella, module


def witness():
    getters = '\n'.join('    let _: String = result.get' + getter + '()' for getter in GETTERS)
    return '''import Foundation
import Libbox
func c2Witness(_ handler: LibboxCommandServerHandlerProtocol, _ platform: LibboxPlatformInterfaceProtocol,
               _ result: LibboxConfigValidationResult, _ exchange: LibboxExchangeContext,
               _ callback: LibboxFuncProtocol, _ transport: LibboxLocalDNSTransportProtocol,
               _ listener: LibboxInterfaceUpdateListenerProtocol) throws {
    var error: NSError?
    let _: LibboxCommandServer? = LibboxNewStrictCommandServer(handler, platform, &error)
    let _: LibboxCommandServer? = LibboxNewTransientCommandServer(handler, platform, &error)
    let _: LibboxConfigValidationResult? = LibboxCheckConfigWithResult("{}", "c2-typecheck", 1)
    let _: String = LibboxConfigValidationContractVersion
    let _: String = LibboxInterfaceUpdateListenerIdentity(listener)
    try platform.bindInterfaceControl(0, interfaceName: "en0")
    exchange.onCancel(callback)
    try callback.invoke()
    let _: Bool = transport.raw()
    try transport.lookup(exchange, network: "ip4", domain: "example.invalid")
    try transport.exchange(exchange, message: Data())
''' + getters + '\n}\n'


def verify_hashes(hashes, helper):
    for name, expected in hashes.items():
        require(Path(name).is_file() and helper.file_hash(name) == expected, 'input hash drift: ' + name)


def collect(final_preflight, source_helpers, checkout, source_receipt, go, mobile, developer, evidence):
    """Called only by explicit observe-source CLI. Never admit a carrier or cached product."""
    require(platform.system() == 'Darwin', 'observe-source requires existing Mac source compiler window')
    shared, core, policy = final_preflight()
    helper = source_helpers()
    provider = helper.provider()
    helper.validate_source_receipt(source_receipt, shared, core)
    checkout, go, mobile, developer = [Path(path).resolve(strict=True) for path in (checkout, go, mobile, developer)]
    helper.verify_checkout(checkout, source_receipt, provider)
    evidence = Path(evidence).expanduser().absolute()
    evidence = evidence.parent.resolve(strict=True) / evidence.name
    require(not evidence.exists() and not evidence.is_symlink()
            and not any(evidence.is_relative_to(path) for path in (checkout, ROOT, mobile, developer, go.parent)),
            'evidence must be a fresh directory outside source repositories/tools')
    require(go.is_file() and all((mobile / name).is_file() for name in ('gomobile', 'gobind')), 'source tool executable missing')
    consumers = [ROOT / 'src-tauri/gen/apple/PacketTunnel' / name for name in CONSUMERS]
    hashes = {str(path): CONSUMERS[path.name] for path in consumers}
    hashes.update({str(ROOT / name): helper.file_hash(ROOT / name) for name in
                   ('scripts/ios-libbox.py', 'scripts/apple-source-inputs.py', 'scripts/apple-carrier.py',
                    'scripts/core-source-provision.py', 'scripts/libbox-patches/android-source.py',
                    'scripts/libbox-patches/source-manifest.json', 'scripts/libbox-ios-patches/apple-source-policy.json')})
    verify_hashes(hashes, helper)
    evidence.mkdir(parents=True)
    scratch = Path(tempfile.mkdtemp(prefix='polaris-c2-owned-', dir=evidence))
    env = {key: os.environ[key] for key in ('HOME', 'PATH', 'GOCACHE', 'GOMODCACHE', 'GOPATH') if key in os.environ}
    env.update({'PATH': str(go.parent) + os.pathsep + env.get('PATH', '/usr/bin:/bin'), 'GOENV': 'off', 'GO111MODULE': 'on',
                'GOWORK': 'off', 'GOTOOLCHAIN': 'local', 'GOPROXY': 'off', 'GOSUMDB': 'off', 'GOFLAGS': '-mod=readonly',
                'TMPDIR': str(scratch), 'DEVELOPER_DIR': str(developer), 'POLARIS_NO_KERNEL_RUN': '1'})
    executables = {str(go): 'go', str(mobile / 'gobind'): 'gobind', '/usr/bin/xcrun': 'xcrun', str(developer / 'usr/bin/xcodebuild'): 'xcodebuild'}
    commands = Commands(evidence, env, executables)
    report = {'evidenceScope': 'C2-source-observations-only', 'status': 'active', 'sourceReceipt': source_receipt,
              'inputHashes': hashes, 'C3Required': list(C3_REQUIRED), 'carrierAdmission': False, 'targets': {}, 'cleanup': {}}
    created, parents = [], []
    try:
        tools, support = observe_tools(commands, go, mobile, developer, shared, policy, helper, checkout)
        report['tools'] = tools
        tool_hashes = {tools[name]['path']: tools[name]['sha256'] for name in ('go', 'gomobile', 'gobind', 'xcode', 'xcrun')}
        for sdk in ('iphoneos', 'iphonesimulator'):
            tool_hashes.update({tools[sdk][name]['path']: tools[sdk][name]['sha256'] for name in ('clang', 'clang++', 'swiftc')})
        umbrella, modulemap = render_headers((support / 'cmd/gomobile/bind_iosapp.go').read_text())
        generated = {}
        for platform_name in ('ios', 'iossimulator'):
            out = scratch / platform_name
            tags = effective_tags(policy, platform_name == 'iossimulator')
            gen_env = dict(commands.env, GOOS='ios', CGO_ENABLED='1')
            commands.run(platform_name + '-gobind', [mobile / 'gobind', '-lang=go,objc', '-outdir=' + str(out),
                         '-tags=' + ','.join(tags), BOUND], checkout, gen_env)
            require((out / 'src/gobind/go_main.go').is_file() and (out / 'src/gobind/go_libboxmain.go').is_file(), 'generated main sources missing')
            require(not any(path.name in ('go.mod', 'go.sum', 'go.work', 'go.work.sum', '_cgo_export.h')
                            or path.name == 'vendor' for path in out.rglob('*')), 'unexpected generated module/cgo header')
            for source, name in [('bind/seq.go.support', 'seq.go'), ('bind/objc/seq_darwin.go.support', 'seq_darwin.go'),
                                 ('bind/objc/seq_darwin.m.support', 'seq_darwin.m'), ('bind/objc/ref.h', 'ref.h'),
                                 ('bind/objc/seq_darwin.h', 'seq_darwin.h')]:
                require((out / 'src/gobind' / name).is_file()
                        and helper.file_hash(out / 'src/gobind' / name) == helper.file_hash(support / source),
                        'generated support source hash differs: ' + name)
            generated[platform_name] = out
            saved = evidence / ('generated-' + platform_name)
            shutil.copytree(out, saved, symlinks=True)
            report[platform_name + 'Generated'] = tree_identity(out, helper)
            layout = scratch / 'header-import' / platform_name / 'Libbox.framework'
            (layout / 'Headers').mkdir(parents=True)
            (layout / 'Modules').mkdir()
            for name in ('Libbox.objc.h', 'Universe.objc.h', 'ref.h'):
                src = out / 'src/gobind' / name
                require(src.is_file() and not src.is_symlink(), 'generated header source missing/redirected')
                shutil.copyfile(src, layout / 'Headers' / name)
            (layout / 'Headers/Libbox.h').write_text(umbrella)
            (layout / 'Modules/module.modulemap').write_text(modulemap)
            shutil.copytree(layout, evidence / ('header-import-' + platform_name))
        witness_file = scratch / 'abi-witness.swift'
        witness_file.write_text(witness())
        shutil.copyfile(witness_file, evidence / witness_file.name)
        for target_id, target in policy['targets'].items():
            platform_name = 'iossimulator' if target['variant'] else 'ios'
            out, sdk = generated[platform_name], tools[target['sdk']]
            copy = checkout / 'build' / (platform_name + '-' + target['goarch']) / 'Libbox'
            require(not copy.exists() and not copy.is_symlink(), 'generated target copy already exists')
            # Retain only the exact target copy we created; do not remove a pre-existing build parent.
            for parent in (checkout / 'build', copy.parent):
                if not parent.exists():
                    parent.mkdir()
                    parents.append((parent, parent.stat().st_ino))
                require(parent.is_dir() and not parent.is_symlink(), 'generated target parent redirected')
            copy.mkdir()
            created.append((copy, copy.stat().st_ino))
            shutil.copytree(out / 'src/gobind', copy, symlinks=True, dirs_exist_ok=True)
            expected_env = target_env(commands.env, target, sdk['path'], sdk['clang']['path'], policy['binding']['iosMinimumVersion'], out)
            tags = effective_tags(policy, bool(target['variant']))
            observed_env = json.loads(commands.run(target_id + '-effective-go-env', [go, 'env', '-json'], copy, expected_env))
            compared = dict(expected_env)
            for key in ('GOOS', 'GOARCH', 'CGO_ENABLED', 'GOFLAGS', 'CC', 'CXX', 'CGO_CFLAGS', 'CGO_CPPFLAGS',
                        'CGO_CXXFLAGS', 'CGO_LDFLAGS', 'GOWORK', 'GOTOOLCHAIN', 'GOENV'):
                compared[key] = observed_env.get(key)
            validate_target_env(compared, expected_env)
            raw = commands.run(target_id + '-graph', [go, 'list', '-deps', '-json', '-tags=' + ','.join(tags), '.'], copy, expected_env)
            observation = graph(raw, checkout, copy, source_receipt, shared, core, helper,
                                tools['host']['GOMODCACHE'], tools['host']['GOROOT'], out)
            bound = next(row for row in observation['packages'] if row['ImportPath'] == BOUND)
            bound_sources = [(Path(bound['Dir']) / filename).read_text() for filename in bound.get('GoFiles', []) + bound.get('CgoFiles', [])]
            header = (out / 'src/gobind/Libbox.objc.h').read_text()
            observation['headerDeclarations'] = check_headers(header, bound_sources)
            verify_hashes(hashes, helper)
            arch = 'x86_64' if target['goarch'] == 'amd64' else 'arm64'
            triple = arch + '-apple-ios' + policy['binding']['appMinimumVersion'] + ('-simulator' if target['variant'] else '')
            commands.run(target_id + '-swift-typecheck', [sdk['swiftc']['path'], '-typecheck', '-target', triple,
                         '-sdk', sdk['path'], '-F', scratch / 'header-import' / platform_name,
                         '-module-cache-path', scratch / 'clang-module-cache', *consumers, witness_file], checkout)
            verify_hashes(observation['sourceHashes'], helper)
            verify_hashes(tool_hashes, helper)
            observation.update({'effectiveTags': tags, 'effectiveEnvironment': expected_env,
                                'actualGoEnvironment': observed_env,
                                'externalInputs': {'sdk': target['sdk'], 'sdkVersion': sdk['version'], 'sdkBuild': sdk['buildVersion'],
                                                   'sysrootSha256': sdk['sysroot']['inventorySha256'],
                                                   'minOSRaw': packed_version(policy['binding']['iosMinimumVersion']),
                                                   'sdkRaw': packed_version(sdk['version']), 'environment': expected_env},
                                'headerSwiftTypecheckScope': 'generated-header-and-swift-typecheck', 'ObjCGoCgoCompilation': 'C3-not-observed'})
            report['targets'][target_id] = observation
        verify_hashes(hashes, helper)
        for name in ('ios', 'iossimulator'):
            require(tree_identity(generated[name], helper) == report[name + 'Generated'], 'generated source/header hash drift')
        require(tree_identity(tools['host']['GOROOT'], helper) == tools['go']['distribution'], 'Go distribution hash drift')
        require(tree_identity(support, helper) == tools['gomobileSource']['inventory'], 'gomobile support source hash drift')
        for name in ('iphoneos', 'iphonesimulator'):
            require(tree_identity(tools[name]['path'], helper) == tools[name]['sysroot'], 'SDK/sysroot hash drift')
        report['status'] = 'observed-C2-scopes-only'
    except Exception as error:
        report.update({'status': 'failed', 'error': str(error)})
        raise
    finally:
        removed, removed_parents = [], []
        report['cleanup'] = {'ownerPID': os.getpid(), 'removedTargetCopies': removed,
                             'removedParents': removed_parents, 'scratch': str(scratch), 'sharedCachesRemoved': False,
                             'processGroupsDrained': commands.cleanup_allowed, 'pendingCleanup': not commands.cleanup_allowed,
                             'registeredProcessGroups': [row['registeredProcessGroup'] for row in commands.rows
                                                         if 'registeredProcessGroup' in row],
                             'pendingProcessGroups': [row['registeredProcessGroup'] for row in commands.rows
                                                      if row.get('groupDrained') is False]}
        try:
            if not commands.cleanup_allowed:
                report['status'] = 'failed'
                report['cleanup'].update({'scratchRemoved': False,
                                         'retainedTargetCopies': [{'path': str(path), 'inode': inode} for path, inode in created],
                                         'retainedParents': [{'path': str(path), 'inode': inode} for path, inode in parents]})
            else:
                for copy, inode in reversed(created):
                    require(copy.is_dir() and not copy.is_symlink() and copy.stat().st_ino == inode, 'owned target copy changed; cleanup blocked')
                    shutil.rmtree(copy)
                    removed.append(str(copy))
                for parent, inode in reversed(parents):
                    require(parent.is_dir() and not parent.is_symlink() and parent.stat().st_ino == inode, 'owned target parent changed; cleanup blocked')
                    parent.rmdir()
                    removed_parents.append(str(parent))
                shutil.rmtree(scratch)
                report['cleanup']['scratchRemoved'] = True
                helper.verify_checkout(checkout, source_receipt, provider)
                verify_hashes(hashes, helper)
                report['cleanup']['providerCheckoutVerifiedAfterRemoval'] = True
        except Exception as error:
            report.update({'status': 'failed', 'cleanupError': str(error)})
            raise
        finally:
            report['commands'] = commands.rows
            report['rawLogHashes'] = {path.name: helper.file_hash(path) for path in evidence.iterdir()
                                      if path.is_file() and path.suffix in ('.stdout', '.stderr')}
            report['commandsLogSha256'] = helper.file_hash(evidence / 'commands.json') if (evidence / 'commands.json').is_file() else None
            (evidence / 'source-observations.json').write_text(json.dumps(report, sort_keys=True, indent=2) + '\n')
    return report
