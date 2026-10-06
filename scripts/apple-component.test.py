#!/usr/bin/env python3
"""Finite synthetic C3 predicates and mocked file/process transactions; no tools."""
import copy
from contextlib import ExitStack
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from unittest import mock

sys.dont_write_bytecode = True
SPEC = importlib.util.spec_from_file_location('apple_component', Path(__file__).with_name('apple-component.py'))
component = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(component)
SPEC = importlib.util.spec_from_file_location('carrier_fixtures', Path(__file__).with_name('apple-carrier.test.py'))
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)
COUNT = 0


def check(ok, reason):
    global COUNT
    if not ok:
        raise AssertionError(reason)
    COUNT += 1


def rejected(action, reason):
    global COUNT
    try:
        action()
    except (RuntimeError, OSError, ValueError, KeyError) as error:
        if reason and reason not in str(error):
            raise AssertionError('wrong rejection: ' + str(error) + '; expected ' + reason) from error
    else:
        raise AssertionError('counterexample accepted: ' + reason)
    COUNT += 1


def framework_fixture(root):
    f, ios = fixtures.fixture(), component.driver()
    helper = ios.source_helpers()
    source_policy = ios.final_preflight()[2]
    apple_input = {'evidenceScope': 'C3-source-compiler-observations-only', 'sourceReceiptFingerprint': f['receipt']['fingerprint'],
                   'targets': {}, 'generatedHeaders': {}}
    for target, row in f['policy']['targets'].items():
        tags = component.inputs.effective_tags(source_policy, target != 'ios-arm64')
        raw = f['raw'][target].replace('-tags=' + ','.join(row['effectiveTags']), '-tags=' + ','.join(tags))
        row.update(effectiveTags=tags, buildInfo=helper.parse_build_info('fixture-section: go1.25.5\n' +
                    ''.join('\t' + line + '\n' for line in raw.splitlines())))
        f['objects'][target] = fixtures.mach_object(target, fixtures.inline(raw), f['id'])
        f['archives'][target] = fixtures.archive([('__.SYMDEF SORTED', bytes(8)), ('go-origin.o', f['objects'][target]),
                       ('same.o', fixtures.mach_object(target)), ('same.o', fixtures.mach_object(target, extra=[('__TEXT', '__cstring', b'B')]))])
        main = {'ImportPath': row['buildInfo']['path'], 'Name': 'main', 'Dir': '/synthetic-test-only/main'}
        packages = [main]
        row['graphFingerprint'] = component.fingerprint(packages)
        source_path = '/synthetic-test-only/main/bridge.m'
        command = {'argv': ['/synthetic-test-only/clang', '-c', source_path], 'cwd': '/synthetic-test-only/main'}
        implementation = {'source': source_path, 'sourceSha256': 'a' * 64, 'command': command, 'objectSha256': 'b' * 64,
                          'exportHeader': {'path': '/synthetic-test-only/work/_cgo_export.h', 'sha256': 'c' * 64}}
        apple_input['targets'][target] = {'graphFingerprint': row['graphFingerprint'], 'effectiveTags': tags,
                'externalInputs': copy.deepcopy(row['externalInputs']), 'headerSwiftTypecheckScope': 'generated-header-and-swift-typecheck',
                'compiler': {'implementations': [implementation], 'cgoPackages': {'main': 'synthetic-test-only'}, 'commands': [command],
                    'generatedCompilerSources': [{'path': '/synthetic-test-only/work/_cgo_export.c', 'sha256': 'a' * 64}],
                    'workInventory': [{'path': 'object.o', 'type': 'file', 'sha256': 'b' * 64}]},
                'nativeMap': {'members': []}, 'externalStaticInputs': [], 'packages': packages, 'main': main,
                'sourceHashes': {source_path: 'a' * 64}, 'actualGoEnvironment': {'GOENV': ''}}
    framework = root / 'Libbox.xcframework'
    framework.mkdir()
    available = []
    for name, targets, platform_name in [('ios-arm64', ['ios-arm64'], 'ios'),
            ('ios-arm64_x86_64-simulator', ['ios-arm64-simulator', 'ios-x86_64-simulator'], 'iossimulator')]:
        layout = framework / name / 'Libbox.framework'
        version = layout / 'Versions/A'
        for folder in ('Headers', 'Modules', 'Resources'):
            (version / folder).mkdir(parents=True)
        os.symlink('A', layout / 'Versions/Current')
        for filename in ('Libbox', 'Headers', 'Modules', 'Resources'):
            os.symlink('Versions/Current/' + filename, layout / filename)
        data = f['archives'][targets[0]] if len(targets) == 1 else fixtures.fat(f['archives'][targets[1]], f['archives'][targets[0]])
        (version / 'Libbox').write_bytes(data)
        for filename in ('Libbox.objc.h', 'Universe.objc.h', 'Libbox.h', 'ref.h'):
            (version / 'Headers' / filename).write_text('synthetic test header ' + filename + '\n')
        (version / 'Modules/module.modulemap').write_text('synthetic test module map\n')
        (version / 'Resources/Info.plist').write_bytes(plistlib.dumps({}))
        apple_input['generatedHeaders'][platform_name] = {filename: helper.file_hash(version / 'Headers' / filename)
                for filename in ('Libbox.objc.h', 'Universe.objc.h', 'Libbox.h', 'ref.h')}
        apple_input['generatedHeaders'][platform_name]['module.modulemap'] = helper.file_hash(version / 'Modules/module.modulemap')
        row = {'LibraryIdentifier': name, 'LibraryPath': 'Libbox.framework', 'SupportedArchitectures': ['arm64'] if len(targets) == 1 else ['arm64', 'x86_64'], 'SupportedPlatform': 'ios'}
        if len(targets) == 2:
            row['SupportedPlatformVariant'] = 'simulator'
        available.append(row)
    (framework / 'Info.plist').write_bytes(plistlib.dumps({'CFBundlePackageType': 'XFWK', 'XCFrameworkFormatVersion': '1.0', 'AvailableLibraries': available}))
    names = ['scripts/ios-libbox.py', 'scripts/apple-component.py', 'scripts/apple-carrier.py', 'scripts/apple-source-inputs.py',
             'scripts/core-source-provision.py', 'scripts/libbox-patches/android-source.py', 'scripts/libbox-patches/source-manifest.json',
             'scripts/libbox-ios-patches/apple-source-policy.json', 'src-tauri/core-manifest.json']
    policy = {'schema': 'polaris-apple-build-policy-v1', 'evidenceScope': 'source-component-build-only',
            'sourceReceiptFingerprint': f['receipt']['fingerprint'], 'appleInputFingerprint': component.input_fingerprint(apple_input),
            'reviewedObservationSha256': 'a' * 64, 'carrierContract': f['policy'], 'componentInventory': component.inventory(framework),
            'producer': {'candidate': 'a' * 40, 'tree': 'a' * 40, 'codeHashes': {name: helper.file_hash(component.ROOT / name) for name in names}},
            'sourceTests': [{'name': 'synthetic-test-only', 'passed': True, 'sourceReceiptFingerprint': f['receipt']['fingerprint'], 'logSha256': 'a' * 64}],
            'excludedSourceTests': [], 'assembly': {'commands': [{'name': name, 'exit': 0, 'groupDrained': True,
                'argv': ['synthetic-test-only'], 'env': {}, 'cwd': 'synthetic-test-only'}
                for name in ('ios-framework-lipo', 'iossimulator-framework-lipo', 'create-xcframework')],
                'rawLogHashes': {str(i): 'a' * 64 for i in range(6)}}}
    ios.final_preflight = lambda: (f['source'], f['core'], source_policy)
    return f, ios, apple_input, policy


def predicate_tests(root):
    f, ios, apple_input, policy = framework_fixture(root)
    framework = root / 'Libbox.xcframework'
    with mock.patch.object(component, 'driver', return_value=ios):
        receipt = component.make_receipt(framework, f['receipt'], apple_input, policy, f['tools'])
        path = root / 'libbox-build-receipt.json'
        path.write_text(json.dumps(receipt))
        check(component.verify_component(framework, path, apple_input, policy, f['tools']) == receipt, 'bytes/path predicates diverged')
        check(set(receipt['component']['carriers']) == set(component.carrier.TARGETS), 'three targets missing')
        check(len(receipt['component']['headers']) == 8 and len(receipt['component']['modulemaps']) == 2, 'complete headers missing')
        for key, value in [('schema', 'history'), ('evidenceScope', 'build-only'), ('outputFingerprint', '0' * 64),
                           ('producer', {}), ('sourceTests', []), ('appleInputFingerprint', '0' * 64), ('buildPolicySha256', '0' * 64)]:
            changed = copy.deepcopy(receipt); changed[key] = value
            rejected(lambda: component.verify_component(framework, changed, apple_input, policy, f['tools']), '')
        for key in list(policy):
            changed = copy.deepcopy(policy); changed.pop(key)
            rejected(lambda: component.verify_component(framework, receipt, apple_input, changed, f['tools']), '')
        for key, value in [('reviewedObservationSha256', None), ('sourceTests', []), ('excludedSourceTests', None),
                           ('componentInventory', []), ('carrierContract', {}), ('producer', {}), ('appleInputFingerprint', '0' * 64)]:
            changed = copy.deepcopy(policy); changed[key] = value
            rejected(lambda: component.inspect_component(framework, f['receipt'], apple_input, changed, f['tools']), '')
        for target in component.carrier.TARGETS:
            for field in ('compiler', 'nativeMap', 'headerSwiftTypecheckScope', 'graphFingerprint', 'effectiveTags', 'externalInputs'):
                changed = copy.deepcopy(apple_input); changed['targets'][target].pop(field)
                new_policy = copy.deepcopy(policy); new_policy['appleInputFingerprint'] = component.input_fingerprint(changed)
                rejected(lambda: component.inspect_component(framework, f['receipt'], changed, new_policy, f['tools']), '')
        for row in component.inventory(framework):
            if row['type'] != 'file':
                continue
            file = framework / row['path']; original = file.read_bytes(); file.write_bytes(original + b'x')
            for entrance in ('inspect', 'verify'):
                action = (lambda: component.inspect_component(framework, f['receipt'], apple_input, policy, f['tools'])) if entrance == 'inspect' else (
                          lambda: component.verify_component(framework, receipt, apple_input, policy, f['tools']))
                rejected(action, 'inventory differs')
            file.write_bytes(original)
        # Recomputed sidecars cannot conceal a stale second architecture from C1.
        binary = framework / 'ios-arm64_x86_64-simulator/Libbox.framework/Versions/A/Libbox'
        original = binary.read_bytes()
        binary.write_bytes(fixtures.fat(f['archives']['ios-x86_64-simulator'].replace(f['id'].encode(), b'0' * len(f['id']), 1), f['archives']['ios-arm64-simulator']))
        changed = copy.deepcopy(policy); changed['componentInventory'] = component.inventory(framework)
        rejected(lambda: component.inspect_component(framework, f['receipt'], apple_input, changed, f['tools']), 'source BuildID differs')
        binary.write_bytes(original)
        extra = framework / 'unregistered'; extra.write_text('extra')
        rejected(lambda: component.verify_component(framework, receipt, apple_input, policy, f['tools']), 'inventory differs')
        extra.unlink()
        link = framework / 'ios-arm64/Libbox.framework/Headers'
        link.unlink(); os.symlink('/tmp', link)
        rejected(lambda: component.inventory(framework), 'symlink escapes')
        link.unlink(); os.symlink('Versions/Current/Headers', link)
        return f, ios, apple_input, policy, receipt


def publication_tests(root):
    stage = root / 'stage'; stage.mkdir()
    f, ios, apple_input, policy = framework_fixture(stage)
    output = root / 'output'; output.mkdir()
    (output / 'Libbox.xcframework').mkdir(); (output / 'Libbox.xcframework/history').write_text('user history')
    (output / 'user-file').write_text('keep')
    with mock.patch.object(component, 'driver', return_value=ios):
        a = component.make_receipt(stage / 'Libbox.xcframework', f['receipt'], apple_input, policy, f['tools'])
        (stage / 'libbox-build-receipt.json').write_text(json.dumps(a))
        snapshot_a = component.publish_component(stage, a, output, build_policy=policy, tools=f['tools'])
        check(component.resolve_component(output, policy, f['tools']) == snapshot_a, 'published resolver differs')
        with mock.patch.object(component.os, 'readlink', wraps=os.readlink) as links:
            component.resolve_component(output, policy, f['tools'])
            check(sum(call.args[0] == output / '.libbox-current' for call in links.call_args_list) == 1, 'resolver reread mutable pointer')
        check(component.publish_component(stage, a, output, build_policy=policy, tools=f['tools']) == snapshot_a, 'identical generation cannot be reused')
        policy_b = copy.deepcopy(policy); policy_b['sourceTests'][0]['logSha256'] = 'b' * 64
        b = component.make_receipt(stage / 'Libbox.xcframework', f['receipt'], apple_input, policy_b, f['tools'])
        target_a = os.readlink(output / '.libbox-current')
        for failure in ('copy', 'receipt-write', 'hash', 'file-fsync', 'generation-rename', 'pointer-replace', 'readback', 'parent-fsync'):
            with ExitStack() as stack:
                if failure == 'copy':
                    original = component.shutil.copytree
                    def partial(source, target, **kwargs):
                        Path(target).mkdir(); (Path(target) / 'partial').write_text('partial')
                        raise OSError('injected copy')
                    stack.enter_context(mock.patch.object(component.shutil, 'copytree', side_effect=partial))
                elif failure == 'receipt-write':
                    original_write = Path.write_text
                    def fail_write(path, *args, **kwargs):
                        if path.name == 'libbox-build-receipt.json' and '.pending-' in str(path):
                            raise OSError('injected receipt-write')
                        return original_write(path, *args, **kwargs)
                    stack.enter_context(mock.patch.object(Path, 'write_text', new=fail_write))
                elif failure == 'hash':
                    original_verify = component.verify_component
                    def fail_verify(framework, *args, **kwargs):
                        if '.pending-' in str(framework):
                            raise RuntimeError('injected hash')
                        return original_verify(framework, *args, **kwargs)
                    stack.enter_context(mock.patch.object(component, 'verify_component', side_effect=fail_verify))
                elif failure == 'file-fsync':
                    stack.enter_context(mock.patch.object(component, 'flush_generation', side_effect=OSError('injected file-fsync')))
                elif failure == 'generation-rename':
                    stack.enter_context(mock.patch.object(component.os, 'rename', side_effect=OSError('injected generation-rename')))
                elif failure == 'pointer-replace':
                    stack.enter_context(mock.patch.object(component.os, 'replace', side_effect=OSError('injected pointer-replace')))
                elif failure == 'readback':
                    original_snapshot, reads = component.snapshot, []
                    def fail_readback(generation, *args, **kwargs):
                        reads.append(generation.name)
                        if reads.count(b['outputFingerprint']) == 2:
                            raise RuntimeError('injected readback')
                        return original_snapshot(generation, *args, **kwargs)
                    stack.enter_context(mock.patch.object(component, 'snapshot', side_effect=fail_readback))
                else:
                    original_flush, failed = component.flush_directory, []
                    def fail_flush(path):
                        if path == output and os.readlink(output / '.libbox-current') != target_a and not failed:
                            failed.append(True); raise OSError('injected parent-fsync')
                        return original_flush(path)
                    stack.enter_context(mock.patch.object(component, 'flush_directory', side_effect=fail_flush))
                rejected(lambda: component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools'], previous_policy=policy), 'injected')
            check(os.readlink(output / '.libbox-current') == target_a, failure + ' did not preserve old pointer')
            check(component.snapshot(Path(snapshot_a['generationRoot']), policy, f['tools']) == snapshot_a, 'A reader bytes lost')
            check(not list((output / '.libbox-generations').glob('.pending-*')), 'partial pending directory leaked')
        snapshot_b = component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools'], previous_policy=policy)
        check(component.resolve_component(output, policy_b, f['tools']) == snapshot_b, 'B resolver differs')
        check(component.snapshot(Path(snapshot_a['generationRoot']), policy, f['tools']) == snapshot_a, 'A snapshot lost after B')
        check((output / 'Libbox.xcframework/history').read_text() == 'user history' and (output / 'user-file').read_text() == 'keep', 'user history modified')
        for invalid in ('../stage', '.libbox-generations/.pending-test', '/tmp', '.libbox-generations/' + 'f' * 64):
            current = output / '.libbox-current'; current.unlink(); os.symlink(invalid, current)
            rejected(lambda: component.resolve_component(output, policy_b, f['tools']), '')
            rejected(lambda: component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools']), '')
            check(os.readlink(current) == invalid, 'invalid old pointer was repaired implicitly')
        current.unlink(); current.write_text('user-owned pointer file')
        rejected(lambda: component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools']), 'non-symlink')
        check(current.read_text() == 'user-owned pointer file', 'non-symlink current changed')
        current.unlink(); os.symlink('.libbox-generations/' + b['outputFingerprint'], current)
        with component.publisher_lock(output):
            rejected(lambda: component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools']), '')




def historical_producer_tests(root):
    stage = root / 'stage'; stage.mkdir()
    f, ios, apple_input, policy = framework_fixture(stage)
    output = root / 'output'; output.mkdir()
    helper = ios.source_helpers()
    with mock.patch.object(component, 'driver', return_value=ios):
        a = component.make_receipt(stage / 'Libbox.xcframework', f['receipt'], apple_input, policy, f['tools'])
        snap_a = component.publish_component(stage, a, output, build_policy=policy, tools=f['tools'])
        fake_root = root / 'changed-producer'; fake_root.mkdir()
        for name in policy['producer']['codeHashes']:
            path = fake_root / name; path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(component.ROOT / name, path)
        with (fake_root / 'scripts/apple-component.py').open('a') as changed:
            changed.write('\n# synthetic prior/new producer seam only\n')
        policy_b = copy.deepcopy(policy)
        policy_b['producer']['codeHashes'] = {name: helper.file_hash(fake_root / name) for name in policy['producer']['codeHashes']}
        check(policy_b['producer']['codeHashes'] != policy['producer']['codeHashes'], 'producer seam did not change actual bytes')
        with mock.patch.object(component, 'ROOT', fake_root):
            check(component.snapshot(Path(snap_a['generationRoot']), policy, f['tools']) == snap_a, 'A cache required current producer bytes')
            rejected(lambda: component.publish_component(stage, a, output, build_policy=policy, tools=f['tools']), 'input hash drift')
            b = component.make_receipt(stage / 'Libbox.xcframework', f['receipt'], apple_input, policy_b, f['tools'])
            snap_b = component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools'], previous_policy=policy)
            check(component.snapshot(Path(snap_a['generationRoot']), policy, f['tools']) == snap_a, 'A snapshot lost after changed-producer B')
            check(component.resolve_component(output, policy_b, f['tools']) == snap_b, 'new B policy did not select complete B')


def rollback_tests(root):
    stage = root / 'stage'; stage.mkdir()
    f, ios, apple_input, policy = framework_fixture(stage)
    with mock.patch.object(component, 'driver', return_value=ios):
        a = component.make_receipt(stage / 'Libbox.xcframework', f['receipt'], apple_input, policy, f['tools'])
        policy_b = copy.deepcopy(policy); policy_b['sourceTests'][0]['logSha256'] = 'b' * 64
        b = component.make_receipt(stage / 'Libbox.xcframework', f['receipt'], apple_input, policy_b, f['tools'])
        for failure in ('rollback-replace', 'rollback-fsync'):
            output = root / failure; output.mkdir()
            snap_a = component.publish_component(stage, a, output, build_policy=policy, tools=f['tools'])
            original_snapshot, reads = component.snapshot, []
            def fail_readback(generation, *args, **kwargs):
                reads.append(generation.name)
                if reads.count(b['outputFingerprint']) == 2:
                    raise RuntimeError('injected late readback')
                return original_snapshot(generation, *args, **kwargs)
            with ExitStack() as stack:
                stack.enter_context(mock.patch.object(component, 'snapshot', side_effect=fail_readback))
                if failure == 'rollback-replace':
                    original_replace, replaces = component.os.replace, []
                    def fail_replace(source, target):
                        replaces.append(source)
                        if len(replaces) == 2:
                            raise OSError('injected rollback replace')
                        return original_replace(source, target)
                    stack.enter_context(mock.patch.object(component.os, 'replace', side_effect=fail_replace))
                else:
                    original_flush = component.flush_directory
                    def fail_rollback_flush(path):
                        if path == output and len(reads) >= 3 and os.readlink(output / '.libbox-current').endswith(a['outputFingerprint']):
                            raise OSError('injected rollback fsync')
                        return original_flush(path)
                    stack.enter_context(mock.patch.object(component, 'flush_directory', side_effect=fail_rollback_flush))
                rejected(lambda: component.publish_component(stage, b, output, build_policy=policy_b, tools=f['tools'], previous_policy=policy), 'rollback pending')
            actual = os.readlink(output / '.libbox-current')
            check(actual.endswith(b['outputFingerprint'] if failure == 'rollback-replace' else a['outputFingerprint']), 'rollback pending reported incorrect current outcome')
            check(component.snapshot(Path(snap_a['generationRoot']), policy, f['tools']) == snap_a, 'rollback failure deleted A')
            snap_b = component.snapshot(output / '.libbox-generations' / b['outputFingerprint'], policy_b, f['tools'])
            check(snap_b['componentOutputFingerprint'] == b['outputFingerprint'], 'rollback failure deleted B')
        first = root / 'first'; first.mkdir()
        original_flush, attempts = component.flush_directory, []
        def fail_first_flush(path):
            if path == first and (first / '.libbox-current').is_symlink() and not attempts:
                attempts.append(True); raise OSError('injected first commit fsync')
            return original_flush(path)
        with mock.patch.object(component, 'flush_directory', side_effect=fail_first_flush):
            rejected(lambda: component.publish_component(stage, a, first, build_policy=policy, tools=f['tools']), 'first commit fsync')
        check(not (first / '.libbox-current').exists() and not (first / '.libbox-current').is_symlink(), 'first publish rollback did not withdraw own pointer')
        check((first / '.libbox-generations' / a['outputFingerprint']).is_dir(), 'first rollback deleted complete generation')


def process_tests(root):
    owned = component.OwnedPaths()
    scratch = owned.mkdir(root / 'scratch')
    copy_dir = owned.mkdir(root / 'copy')
    with mock.patch.object(component.shutil, 'copytree', side_effect=OSError('partial copy')):
        rejected(lambda: component.shutil.copytree(root, copy_dir, dirs_exist_ok=True), 'partial copy')
    check(any(row['path'] == str(copy_dir) and row['inode'] == copy_dir.stat().st_ino for row in owned.rows), 'partial copy was not preregistered')
    commands = component.Commands(root, {}, {'/go': 'go'}, root, scratch)
    rejected(lambda: owned.cleanup(SimpleNamespace(cleanup_allowed=False, rows=[])), 'drain unknown')
    check(copy_dir.exists() and scratch.exists(), 'unknown drained inputs removed')
    for argv in [['/go', 'build', '.'], ['/usr/bin/xcrun', 'libtool', '-static', '-o', '/tmp/new.a', '/tmp/input.a'],
                 ['/usr/bin/xcrun', 'lipo', '/tmp/a', '-create', '-o', '/tmp/b'], ['/go', 'test', './...'], ['/bin/sh', '-c', 'true']]:
        rejected(lambda argv=argv: commands.run('forbidden', argv, root), 'forbidden tool')
    child = SimpleNamespace(pid=4707)
    def normal_exit(timeout):
        check(commands.rows[-1]['registeredProcessGroup'] == child.pid and commands.rows[-1]['groupDrained'] is False,
              'PGID not registered before parent wait')
        return 0
    child.wait = mock.Mock(side_effect=normal_exit)
    with mock.patch.object(component.inputs.subprocess, 'Popen', return_value=child), \
         mock.patch.object(commands, 'group_exists', side_effect=[True, True]), \
         mock.patch.object(commands, 'stop_group', side_effect=lambda child, row: row.update(groupDrained=False)):
        rejected(lambda: commands.run('parent-exit', ['/go', 'version'], root), 'descendants remain')
    check(commands.rows[-1]['groupPresentAfterParentExit'] and not commands.cleanup_allowed, 'ordinary parent exit admitted group drain')
    rejected(lambda: owned.cleanup(commands), 'drain unknown')
    commands.rows[-1]['groupDrained'] = True; commands.cleanup_allowed = True
    removed = owned.cleanup(commands)
    check(str(copy_dir) in removed and str(scratch) in removed, 'owned exact cleanup failed')
    owned = component.OwnedPaths(); directory = owned.mkdir(root / 'swap')
    directory.rename(root / 'preserved'); directory.mkdir()
    rejected(lambda: owned.cleanup(SimpleNamespace(cleanup_allowed=True, rows=[])), 'identity changed')
    check((root / 'preserved').exists() and directory.exists(), 'changed inode deleted')



def compiler_tests(root):
    helper = component.carrier.source_helpers()
    scratch = root / 'scratch'; scratch.mkdir()
    work = scratch / 'go-build-owned'; work.mkdir()
    main_dir = root / 'main'; main_dir.mkdir()
    reverse_dir = scratch / 'ios/src/ObjC/message'; reverse_dir.mkdir(parents=True)
    packages = []
    lines = ['WORK=' + str(work)]
    for index, (name, directory, mfile) in enumerate([('example.invalid/generated/main', main_dir, 'seq_darwin.m'),
                                                    ('ObjC/message', reverse_dir, 'message.m')], 1):
        objdir = work / ('b00' + str(index)); objdir.mkdir()
        (directory / 'bridge.go').write_text('package main\n')
        (directory / 'go_main.go').write_text('package main\n')
        (directory / mfile).write_text('@import ObjectiveC.message;\n' if index == 2 else '#include "_cgo_export.h"\n')
        (objdir / '_cgo_export.h').write_text('real mock cgo export header\n')
        (objdir / '_cgo_gotypes.go').write_text('package main\n')
        (objdir / 'bridge.cgo1.go').write_text('package main\n')
        (objdir / '_cgo_export.c').write_text('cgo generated C\n')
        (objdir / '_x001.o').write_bytes(fixtures.mach_object('ios-arm64'))
        (objdir / '_x002.o').write_bytes(fixtures.mach_object('ios-arm64', extra=[('__TEXT', '__cstring', b'B')]))
        package = {'ImportPath': name, 'Name': 'main' if index == 1 else 'message', 'Dir': str(directory),
                   'CgoFiles': ['bridge.go'], 'GoFiles': ['go_main.go'], 'MFiles': [mfile]}
        packages.append(package)
        lines += ['cd ' + str(directory), '/go/pkg/tool/cgo -importpath ' + name + ' -objdir ' + str(objdir) +
                  ' -- -D__GOBIND_DARWIN__ ' + str(directory / 'bridge.go'),
                  '/go/pkg/tool/compile -p ' + ('main' if index == 1 else name) + ' ' + str(directory / 'go_main.go') +
                  ' ' + str(objdir / '_cgo_gotypes.go') + ' ' + str(objdir / 'bridge.cgo1.go'),
                  'TERM=dumb /usr/bin/clang -I ' + str(objdir) + ' -x objective-c -fobjc-arc -fmodules -fblocks -Werror -c ' +
                  str(directory / mfile) + ' -o ' + str(objdir / '_x002.o'), 'cd ' + str(objdir),
                  '/usr/bin/clang -c _cgo_export.c -o ' + str(objdir / '_x001.o')]
    lines += ['/usr/bin/clang -framework Foundation -o ' + str(work / 'link.o')]
    observation = {'packages': packages, 'main': packages[0]}
    log = '\n'.join(lines) + '\n'
    compiled = component.compiler_facts(log, observation, scratch, helper)
    check(len(compiled['implementations']) == 2 and len(compiled['cgoPackages']) == 2, 'generated/support/reverse compilation incomplete')
    check(all(row['exportHeader']['path'].endswith('_cgo_export.h') for row in compiled['implementations']), 'real include/header binding lost')
    check(len(compiled['generatedCompilerSources']) == 8, 'generated C/Go/header inventory missing')
    for fragment in ('WORK=' + str(work), '/go/pkg/tool/cgo -importpath example.invalid/generated/main',
                     '/go/pkg/tool/compile -p main', '-D__GOBIND_DARWIN__', '-fobjc-arc', '-fmodules', '-fblocks', '-Werror',
                     '-x objective-c', '-framework Foundation', '/go/pkg/tool/cgo -importpath ObjC/message',
                     str(main_dir / 'go_main.go'), str(work / 'b001/bridge.cgo1.go')):
        changed = log.replace(fragment, 'invalid', 1)
        rejected(lambda changed=changed: component.compiler_facts(changed, observation, scratch, helper), '')
    changed = log.replace(' -I ' + str(work / 'b001'), ' -I ' + str(scratch))
    rejected(lambda: component.compiler_facts(changed, observation, scratch, helper), 'include chain')
    header = work / 'b001/_cgo_export.h'; header.unlink()
    rejected(lambda: component.compiler_facts(log, observation, scratch, helper), 'export.h missing')
    header.write_text('real mock cgo export header\n')
    native = fixtures.archive([('repeat.o', (work / 'b001/_x001.o').read_bytes()),
                               ('repeat.o', (work / 'b002/_x001.o').read_bytes())])
    mapping = component.native_map(native, [], compiled, helper)
    check(len(mapping['members']) == 2 and all(len(row['origins']) == 2 for row in mapping['members']), 'identical native origins collapsed')
    over = fixtures.archive([('repeat.o', (work / 'b001/_x001.o').read_bytes())] * 3)
    rejected(lambda: component.native_map(over, [], compiled, helper), 'multiplicity')
    unknown = fixtures.archive([('foreign.o', fixtures.mach_object('ios-arm64', extra=[('__TEXT', '__cstring', b'foreign')]))])
    rejected(lambda: component.native_map(unknown, [], compiled, helper), '')


def allowance_tests(root):
    f, ios, apple_input, policy = framework_fixture(root)
    scratch = root / 'scratch'; scratch.mkdir()
    with mock.patch.object(component, 'driver', return_value=ios):
        commands = component.Commands(root, {'GOPATH': '/synthetic-gopath'}, {'/go': 'go'}, root, scratch, f['receipt'], '/go')
        sdk_tools = {name: {'path': '/synthetic-sdk/' + name, 'clang': {'path': '/synthetic-clang/' + name}}
                     for name in ('iphoneos', 'iphonesimulator')}
        commands.configure_targets(sdk_tools, {name: scratch / name for name in ('ios', 'iossimulator')})
        def allowed(argv, cwd=None, env=None):
            target = Path(argv[4]).name.removesuffix('.a') if len(argv) > 4 and argv[1] == 'build' else None
            expected = commands.target_inputs.get(target, {'cwd': root, 'env': {}})
            return commands._allowed_command(list(argv), expected['cwd'] if cwd is None else cwd,
                                              expected['env'] if env is None else env)
        check(len(commands.archive_commands) == 6, 'target/recompile tuples differ')
        for argv in commands.archive_commands:
            check(not allowed(argv, cwd=root), 'wrong compiler cwd admitted')
            context = commands.target_inputs[Path(argv[4]).name.removesuffix('.a')]
            rejected(lambda: commands.run('wrong-cwd', argv, root, context['env']), 'forbidden tool')
            target_id = Path(argv[4]).name.removesuffix('.a')
            context = commands.target_inputs[target_id]
            for field in ('GOOS', 'GOARCH', 'DARWIN_SDK', 'CGO_CFLAGS', 'GOPATH', 'GOENV'):
                changed_env = dict(context['env']); changed_env[field] = 'wrong'
                with mock.patch.object(component.inputs.subprocess, 'Popen', side_effect=AssertionError('rejection spawned compiler')):
                    rejected(lambda changed_env=changed_env: commands.run('wrong-context', argv, context['cwd'], changed_env), 'forbidden tool')
            check(allowed(list(argv)), 'fixed full-target build denied')
            for index in (0, 2, 4, 5, 7, 8, 9, 10, 11):
                changed = list(argv); changed[index] = 'changed'
                check(not allowed(changed), 'mutated compiler tuple accepted')
                rejected(lambda changed=changed: commands.run('wrong-tuple', changed, context['cwd'], context['env']), 'forbidden tool')
        for argv in (['/usr/bin/xcrun', '--find', 'lipo'], ['/usr/bin/xcrun', '--find', 'libtool'], ['/usr/bin/xcrun', '--find', 'ld']):
            check(allowed(argv), 'fixed tool identity query denied')
        check(not allowed(['/usr/bin/xcrun', '--find', 'arbitrary']), 'arbitrary tool query allowed')
        check(not allowed(['/xcodebuild', '-create-xcframework', '-framework', '/a', '-framework', '/b', '-output', '/out']), 'observer allowed Framework assembly')
        check('-extldflags' not in commands.archive_commands[0][7] and commands.archive_commands[0][7].endswith(f['id']), 'Apple flags lost common BuildID or inherited ELF flags')


def retained_generated_tests(root):
    f, ios, apple_input, policy = framework_fixture(root)
    helper = ios.source_helpers()
    headers = ('Libbox.objc.h', 'Universe.objc.h', 'ref.h')
    for name, slice_name in [('ios', 'ios-arm64'), ('iossimulator', 'ios-arm64_x86_64-simulator')]:
        generated = root / (name + '-generated')
        source = generated / 'src/gobind'
        source.mkdir(parents=True)
        for filename in headers:
            shutil.copyfile(root / 'Libbox.xcframework' / slice_name / 'Libbox.framework/Headers' / filename, source / filename)
        (source / 'main.go').write_text('package main // synthetic retained generated input\n')
        apple_input[name + 'Generated'] = component.inputs.tree_identity(generated, helper)
    for target, observed in apple_input['targets'].items():
        selected = root / 'target-copies' / target
        selected.mkdir(parents=True)
        bridge = selected / 'bridge.m'
        bridge.write_text('synthetic target copy ' + target)
        source_hashes = {str(bridge): helper.file_hash(bridge)}
        name = 'ios' if target == 'ios-arm64' else 'iossimulator'
        for filename in headers:
            copied = selected / filename
            shutil.copyfile(Path(apple_input[name + 'Generated']['root']) / 'src/gobind' / filename, copied)
            source_hashes[str(copied)] = helper.file_hash(copied)
        observed['sourceHashes'] = source_hashes
        observed['compiler']['implementations'][0].update(source=str(bridge), sourceSha256=source_hashes[str(bridge)])
        archive = selected / 'retained.a'
        archive.write_bytes(f['archives'][target])
        observed.update(archivePath=str(archive), archiveSha256=helper.file_hash(archive))
    selected_input = root / 'selected-input'
    selected_input.write_text('synthetic retained input')
    apple_input['inputHashes'] = {str(selected_input): helper.file_hash(selected_input)}
    tool_root = root / 'selected-tools'
    tool_root.mkdir()
    tools = {}
    for name in ('go', 'gomobile', 'gobind', 'xcode', 'xcrun', 'libtool', 'lipo', 'ld'):
        path = tool_root / name
        path.write_text('synthetic tool bytes ' + name)
        tools[name] = {'path': str(path), 'sha256': helper.file_hash(path)}
    for name in ('iphoneos', 'iphonesimulator'):
        sdk = tool_root / name
        sdk.mkdir()
        tools[name] = {'path': str(sdk)}
        for compiler in ('clang', 'clang++', 'swiftc'):
            path = sdk / compiler
            path.write_text('synthetic SDK compiler bytes ' + compiler)
            tools[name][compiler] = {'path': str(path), 'sha256': helper.file_hash(path)}
        tools[name]['sysroot'] = component.inputs.tree_identity(sdk, helper)
    for name in ('go-distribution', 'gomobile-source'):
        path = tool_root / name
        path.mkdir()
        (path / 'retained-source').write_text('synthetic distribution/source bytes')
    tools['host'] = {'GOROOT': str(tool_root / 'go-distribution')}
    tools['go']['distribution'] = component.inputs.tree_identity(tools['host']['GOROOT'], helper)
    tools['gomobileSource'] = {'module': {'Dir': str(tool_root / 'gomobile-source')},
                             'inventory': component.inputs.tree_identity(tool_root / 'gomobile-source', helper)}
    apple_input.update(tools=tools, commands=[{'env': {'DEVELOPER_DIR': str(tool_root)}}])
    policy.update(appleInputFingerprint=component.input_fingerprint(apple_input), componentInventory=[], assembly={})
    output = root / 'published'
    generation = output / '.libbox-generations/A'
    generation.mkdir(parents=True)
    (generation / 'reader-pinned-input').write_text('preserve existing reader generation')
    os.symlink('.libbox-generations/A', output / '.libbox-current')
    evidence = root / 'm1-evidence'
    target_hashes = {target: copy.deepcopy(observed['sourceHashes']) for target, observed in apple_input['targets'].items()}

    def gate(reason, *, stage_boundary=False, input_gate=True):
        before = component.inventory(root)
        pointer = os.readlink(output / '.libbox-current')
        effects = [(Path, 'mkdir'), (Path, 'write_text'), (Path, 'write_bytes'), (component.tempfile, 'mkdtemp'),
                   (component.shutil, 'copyfile'), (component.os, 'symlink'), (component.os, 'replace'),
                   (component.AssemblyCommands, 'run'), (component, 'publish_component'),
                   (component.inputs.subprocess, 'run'), (component.inputs.subprocess, 'Popen')]
        with mock.patch.object(component, 'driver', return_value=ios), \
             mock.patch.object(component, 'source_candidate', return_value={'candidate': 'a' * 40, 'tree': 'a' * 40}), \
             mock.patch.object(component, 'verify_compiler_inputs', wraps=component.verify_compiler_inputs) as actual, ExitStack() as stack:
            guards = []
            for owner, name in effects:
                failure = RuntimeError('expected M1 stage boundary') if stage_boundary and name == 'mkdir' else AssertionError('M1 effect before retained-input rejection: ' + name)
                guards.append(stack.enter_context(mock.patch.object(owner, name, side_effect=failure, autospec=owner is Path)))
            rejected(lambda: component.assemble_component(apple_input, f['receipt'], policy, f['tools'], evidence), reason)
            check(actual.call_count == int(input_gate), 'actual assemble caller bypassed retained-input gate')
            check([guard.call_count for guard in guards] == [int(stage_boundary)] + [0] * (len(guards) - 1), 'M1 made stage/tool/publish effects')
            if stage_boundary:
                check(guards[0].call_args.args == (evidence,), 'positive caller stopped at wrong stage boundary')
        check(not evidence.exists() and component.inventory(root) == before, 'M1 changed producer files before rejection')
        check(os.readlink(output / '.libbox-current') == pointer, 'M1 changed current pointer before rejection')
        for target, hashes in target_hashes.items():
            component.inputs.verify_hashes(hashes, helper)
            check(apple_input['targets'][target]['sourceHashes'] == hashes, 'generated origin test mutated target-copy observations')

    check(component.verify_compiler_inputs(apple_input, helper) is None, 'valid retained generated inputs rejected')
    gate('expected M1 stage boundary', stage_boundary=True)
    for name in ('ios', 'iossimulator'):
        generated = Path(apple_input[name + 'Generated']['root'])
        source = generated / 'src/gobind'
        reason = 'selected generated origin drift: ' + name
        for filename in headers:
            path = source / filename
            original, mode = path.read_bytes(), path.stat().st_mode & 0o777
            path.write_bytes(original + b'drift')
            gate(reason)
            path.write_bytes(original)
            path.unlink()
            gate(reason)
            path.write_bytes(original)
            path.chmod(mode ^ 0o100)
            gate(reason)
            path.chmod(mode)
        extra = source / 'extra-generated.h'
        extra.write_text('unexpected generated file')
        gate(reason)
        extra.unlink()
        path = source / 'main.go'
        original = path.read_bytes()
        path.write_bytes(original + b'// drift\n')
        gate(reason)
        path.write_bytes(original)
        held = root / (name + '-held')
        generated.rename(held)
        gate('')
        os.symlink(held.name, generated)
        gate(reason)
        generated.unlink()
        held.rename(generated)
        alias = root / (name + '-alias')
        os.symlink(generated.name, alias)
        apple_input[name + 'Generated']['root'] = str(alias)
        rejected(lambda: component.verify_compiler_inputs(apple_input, helper), reason)
        gate('independent component input freeze differs', input_gate=False)
        apple_input[name + 'Generated']['root'] = str(generated)
        alias.unlink()
        replacement = root / (name + '-replacement')
        shutil.copytree(generated, replacement)
        apple_input[name + 'Generated']['root'] = str(replacement)
        gate('independent component input freeze differs', input_gate=False)
        apple_input[name + 'Generated']['root'] = str(generated)
        shutil.rmtree(replacement)
    check(component.verify_compiler_inputs(apple_input, helper) is None, 'restored retained generated inputs rejected')


def actual_cgo_path_tests(root):
    """Finite -x shape from failed 031: captured cwd plus ./Go operands, no tools."""
    helper = component.carrier.source_helpers()
    scratch = root / 'owned scratch'; scratch.mkdir()
    work = scratch / 'go-build-owned'; work.mkdir()
    main_dir = root / 'canonical source/build/ios-arm64/Libbox'; main_dir.mkdir(parents=True)
    reverse_dir = scratch / 'ios/src/ObjC/message'; reverse_dir.mkdir(parents=True)
    quote = component.shlex.quote
    main_names = ['go_libboxmain.go', 'go_main.go', 'seq.go', 'seq_darwin.go']
    packages, lines = [], ['WORK=' + str(work)]
    for index, (importpath, directory, names, mfile) in enumerate([
            ('github.com/sagernet/sing-box/build/ios-arm64/Libbox', main_dir, main_names, 'seq_darwin.m'),
            ('ObjC/message', reverse_dir, ['message.go'], 'message.m')], 1):
        objdir = work / ('b00' + str(index)); objdir.mkdir()
        for name in names:
            (directory / name).write_text('package main\n')
            (objdir / (Path(name).stem + '.cgo1.go')).write_text('package main\n')
        (directory / mfile).write_text('@import ObjectiveC.message;\n' if index == 2 else '#include "_cgo_export.h"\n')
        (objdir / '_cgo_export.h').write_text('synthetic export header\n')
        (objdir / '_cgo_gotypes.go').write_text('package main\n')
        (objdir / '_cgo_export.c').write_text('synthetic generated C\n')
        (objdir / '_x001.o').write_bytes(fixtures.mach_object('ios-arm64'))
        (objdir / '_x002.o').write_bytes(fixtures.mach_object('ios-arm64', extra=[('__TEXT', '__cstring', b'B')]))
        packages.append({'ImportPath': importpath, 'Name': 'main' if index == 1 else 'message',
                         'Dir': str(directory), 'CgoFiles': names, 'GoFiles': [], 'MFiles': [mfile]})
        obj = '$WORK/b00' + str(index)
        lines += ['cd ' + quote(str(directory)),
                  "TERM='dumb' CGO_LDFLAGS='' /go/pkg/tool/darwin_arm64/cgo -objdir " + quote(obj + '/') +
                  ' -importpath ' + importpath + ' -exportheader=' + quote(obj + '/_cgo_install.h') +
                  ' "-ldflags=\\\"-framework\\\" \\\"Foundation\\\" \\\"-lobjc\\\"" -- -I ' + quote(obj + '/') +
                  ' -D__GOBIND_DARWIN__ -x objective-c -fobjc-arc -fmodules -fblocks -Werror ' +
                  ' '.join('./' + name for name in names),
                  '/go/pkg/tool/darwin_arm64/compile -p ' + ('main' if index == 1 else importpath) + ' ' +
                  ' '.join(quote(str(path)) for path in sorted(objdir.glob('*.go'))),
                  'TERM=dumb /usr/bin/clang -I ' + quote(str(objdir)) +
                  ' -x objective-c -fobjc-arc -fmodules -fblocks -Werror -c ' + quote(str(directory / mfile)) +
                  ' -o ' + quote(str(objdir / '_x002.o')),
                  'cd ' + quote(obj), '/usr/bin/clang -c _cgo_export.c -o ' + quote(str(objdir / '_x001.o'))]
    lines += ['/usr/bin/clang -framework Foundation -o ' + quote(str(work / 'link.o'))]
    observation = {'packages': packages, 'main': packages[0]}
    log = '\n'.join(lines) + '\n'
    def facts(value):
        return component.compiler_facts(value, observation, scratch, helper)
    compiled = facts(log)
    main = compiled['cgoPackages'][packages[0]['ImportPath']]
    check(main['command']['cwd'] == str(main_dir), 'actual-shape cgo cwd lost')
    check(main['command']['argv'][-4:] == ['./' + name for name in main_names], 'actual-shape ./Go operands lost')
    check(set(str(main_dir / name) for name in main_names) <= set(compiled['sourceHashes']), 'complete CgoFiles hashes lost')
    check(len(compiled['implementations']) == 2 and len(compiled['cgoPackages']) == 2,
          'actual-shape generated/support/reverse predicates lost')
    native = fixtures.archive([('main.o', (work / 'b001/_x001.o').read_bytes()),
                               ('reverse.o', (work / 'b002/_x001.o').read_bytes())])
    check(len(component.native_map(native, [], compiled, helper)['members']) == 2, 'native archive consumption lost')
    absolute_log = log
    for package in packages:
        for name in package['CgoFiles']:
            absolute_log = absolute_log.replace('./' + name, quote(str(Path(package['Dir']) / name)))
    check(facts(absolute_log)['sourceHashes'] == compiled['sourceHashes'], 'absolute operand consumption differs')
    bare_log = log
    for package in packages:
        for name in package['CgoFiles']:
            bare_log = bare_log.replace('./' + name, name)
    check(facts(bare_log)['sourceHashes'] == compiled['sourceHashes'], 'bare operand requires captured cwd')
    nested = main_dir / 'nested'; nested.mkdir()
    check(facts(log.replace('./go_libboxmain.go', './nested/../go_libboxmain.go'))['sourceHashes'] == compiled['sourceHashes'],
          'existing relative path components do not resolve to same file')
    alias = root / 'main alias'; alias.symlink_to(main_dir, target_is_directory=True)
    check(facts(log.replace('cd ' + quote(str(main_dir)), 'cd ' + quote(str(alias)), 1))['sourceHashes'] == compiled['sourceHashes'],
          'proven same physical cwd alias differs')
    wrong = root / 'wrong cwd'; wrong.mkdir()
    for name in main_names:
        (wrong / name).write_bytes((main_dir / name).read_bytes())
    wrong_alias = root / 'wrong alias'; wrong_alias.symlink_to(wrong, target_is_directory=True)
    for wrong_cwd in (wrong, wrong_alias):
        for value in (log, bare_log):
            rejected(lambda value=value, wrong_cwd=wrong_cwd: facts(value.replace(
                'cd ' + quote(str(main_dir)), 'cd ' + quote(str(wrong_cwd)), 1)), 'cgo source not consumed')
    for value in (log, bare_log):
        for cwd_line in ('', 'cd .', 'cd ' + quote(str(main_dir.relative_to(root))), 'cd "$UNKNOWN"'):
            rejected(lambda value=value, cwd_line=cwd_line: facts(value.replace(
                'cd ' + quote(str(main_dir)), cwd_line, 1)), 'cgo source cwd')
    for token in (quote(str(wrong / 'go_libboxmain.go')), quote(str(wrong_alias / 'go_libboxmain.go')),
                  './nested/go_libboxmain.go', './not-created/../go_libboxmain.go'):
        rejected(lambda token=token: facts(log.replace('./go_libboxmain.go', token, 1)), '')
    # A source-looking option value is not a cgo Go operand; Go scans the final .go run.
    option_log = absolute_log.replace(quote(str(main_dir / 'go_libboxmain.go')), '', 1).replace(
        ' -- -I ', ' -trimpath ' + quote(str(main_dir / 'go_libboxmain.go')) + ' -- -I ', 1)
    rejected(lambda: facts(option_log), 'cgo source not consumed')
    interrupted_tail = absolute_log.replace(quote(str(main_dir / 'go_main.go')),
                                           '-DNOT_A_GO_OPERAND ' + quote(str(main_dir / 'go_main.go')), 1)
    rejected(lambda: facts(interrupted_tail), 'cgo source not consumed')
    for changed in (log.replace(' -- -I ', ' -I ', 1), log.replace(' -- -I ', ' -- -- -I ', 1),
                    log.replace(' -importpath ', ' -srcdir ' + quote(str(wrong)) + ' -importpath ', 1),
                    log.replace(' -importpath ', ' -srcdir=' + quote(str(wrong)) + ' -importpath ', 1)):
        rejected(lambda changed=changed: facts(changed), '')
    for changed in (log.replace(' -objdir ', ' -not-objdir ', 1),
                    log.replace(' -importpath ' + packages[0]['ImportPath'], ' -importpath foreign/main', 1),
                    log.replace('-c _cgo_export.c', '-c unrelated.c', 1),
                    log.replace(quote(str(work / 'b001/go_libboxmain.cgo1.go')), 'unconsumed.go', 1),
                    log.replace('-framework Foundation', '-framework Foreign', 1)):
        rejected(lambda changed=changed: facts(changed), '')
    header = work / 'b001/_cgo_export.h'; original = header.read_bytes(); header.unlink()
    rejected(lambda: facts(log), 'export.h missing'); header.write_bytes(original)
    object_file = work / 'b001/_x001.o'; original = object_file.read_bytes(); object_file.unlink()
    rejected(lambda: facts(log), ''); object_file.write_bytes(original)
    check(facts(log) == compiled, 'finite mutations failed to restore the valid fixture')


def run_tests():
    with mock.patch.object(component, 'source_candidate', return_value={'candidate': 'a' * 40, 'tree': 'a' * 40}), \
            mock.patch.object(component, 'verify_compiler_inputs'), \
            tempfile.TemporaryDirectory(prefix='polaris-c3-pure-') as name:
        root = Path(name)
        for folder, action in [('predicate', predicate_tests), ('publication', publication_tests), ('rollback', rollback_tests), ('historical-producer', historical_producer_tests), ('process', process_tests), ('compiler', compiler_tests), ('allowance', allowance_tests)]:
            path = root / folder; path.mkdir(); action(path)
    with mock.patch.object(component.platform, 'system', return_value='Linux'):
        rejected(lambda: component.observe_component('/checkout', {}, '/go', '/mobile', '/developer', '/evidence'), 'Mac source compiler window')
    rejected(lambda: component.read_json('/nonexistent'), '')
    check(component.Commands.run is component.inputs.Commands.run, 'C3 duplicated shared execution algorithm')
    check(component.Commands.stop_group is component.inputs.Commands.stop_group, 'C3 duplicated group drain')
    check(component.Commands.group_exists is component.inputs.Commands.group_exists, 'C3 duplicated group observation')
    with tempfile.TemporaryDirectory(prefix='polaris-c3-retained-generated-') as name:
        retained_generated_tests(Path(name))
    with tempfile.TemporaryDirectory(prefix='polaris-c3-actual-cgo-path-') as name:
        actual_cgo_path_tests(Path(name))


if __name__ == '__main__':
    with mock.patch.object(component.inputs.subprocess, 'run', side_effect=AssertionError('unexpected compiler/tool')), \
         mock.patch.object(component.inputs.subprocess, 'Popen', side_effect=AssertionError('unexpected compiler spawn')), \
         mock.patch.object(component.inputs.os, 'killpg', side_effect=AssertionError('unexpected process group signal')), \
         mock.patch.object(component.inputs.shutil, 'disk_usage', return_value=SimpleNamespace(free=10 * 1024**3)):
        run_tests()
    print(f'{COUNT} Apple component unit cases passed; no compiler, Framework or App built.')
