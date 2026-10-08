#!/usr/bin/env python3
"""Finite host fixtures: real provider/Git replay; tool, bind and JNI callees are stubs."""
import ast
import copy
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.dont_write_bytecode = True


def load(name, file):
    spec = importlib.util.spec_from_file_location(name, file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


builder = load('fixture_builder', HERE / 'build.py')
android = builder.android
verifier = load('fixture_verifier', HERE / 'verify-receipt.py')
FIXTURE_PATCHED = ['example.com/patched', 'example.com/second']
EMPTY_GRAPH_SHA256 = '4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945'


def git(*args, cwd):
    return subprocess.run(['git', *args], cwd=cwd, check=True, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE).stdout.strip()


def repository(path, files):
    path.mkdir()
    git('init', '-q', cwd=path)
    for name, data in files.items():
        target = path / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(data)
    git('add', '-A', cwd=path)
    git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@invalid', 'commit', '-qm', 'fixture', cwd=path)
    return git('rev-parse', 'HEAD', cwd=path)


class AndroidSourceFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='polaris-android-source-fixture-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.app = self.root / 'app'
        self.patches = self.app / 'scripts/libbox-patches'
        self.patches.mkdir(parents=True)
        (self.app / 'src-tauri').mkdir()
        for filename in ('build-libbox.sh', 'core-source-provision.py'):
            target = self.app / 'scripts' / filename
            target.write_bytes((ROOT / 'scripts' / filename).read_bytes())
        for filename in ('build.py', 'verify-receipt.py', 'android-source.py', 'prepare-ndk-linux.py', 'ndk-linux-archive.json'):
            (self.patches / filename).write_bytes((HERE / filename).read_bytes())
        for module in (android, verifier.android):
            for field, value in [('ROOT', self.app), ('PATCH_DIR', self.patches), ('PROVIDER', self.app / 'scripts/core-source-provision.py')]:
                context = patch.object(module, field, value)
                context.start()
                self.addCleanup(context.stop)
        for module in (builder, verifier.builder, verifier):
            for field, value in [('ROOT', self.app), ('PATCH_DIR', self.patches)]:
                context = patch.object(module, field, value)
                context.start()
                self.addCleanup(context.stop)
        self.source = json.loads((HERE / 'source-manifest.json').read_bytes())
        self.core = json.loads((ROOT / 'src-tauri/core-manifest.json').read_bytes())
        self.policy = json.loads((HERE / 'android-source-policy.json').read_bytes())
        self.upstream = self.root / 'upstream'
        tag_source = '\n'.join('sharedTags = append(sharedTags, "' + tag + '")' for tag in self.source['buildTags'])
        self.source['sourceCommit'] = repository(self.upstream, {'go.mod': 'module github.com/sagernet/sing-box\n\ngo 1.25.5\n',
                          'go.sum': 'fixture\n', 'core.go': 'package core\nconst fixture = 1\n',
                          'cmd/internal/build_libbox/main.go': tag_source})
        git('tag', 'v' + self.core['bundledCoreVersion'], cwd=self.upstream)
        core_patch = 'diff --git a/core.go b/core.go\n--- a/core.go\n+++ b/core.go\n@@ -1,2 +1,2 @@\n package core\n-const fixture = 1\n+const fixture = 2\n'
        (self.patches / 'fixture-core.patch').write_text(core_patch)
        self.source['patches'] = [{'file': 'fixture-core.patch', 'sha256': android.digest(core_patch.encode())}]
        self.repositories = {}
        self.source['dependencyPatches'] = []
        for index, module in enumerate(FIXTURE_PATCHED):
            name = ('patched', 'second')[index]
            path = self.root / name
            commit = repository(path, {'go.mod': 'module ' + module + '\n\ngo 1.25.5\n', 'input.go': 'package input\nconst value = 1\n'})
            content = 'diff --git a/input.go b/input.go\n--- a/input.go\n+++ b/input.go\n@@ -1,2 +1,2 @@\n package input\n-const value = 1\n+const value = 2\n'
            (self.patches / (name + '.patch')).write_text(content)
            git('apply', str(self.patches / (name + '.patch')), cwd=path)
            git('add', '-A', cwd=path)
            tree = git('write-tree', cwd=path)
            git('reset', '--hard', commit, cwd=path)
            self.source['dependencyPatches'].append({'name': name, 'module': module, 'upstreamVersion': 'v1.0.0',
                       'upstreamCommit': commit, 'patchedTree': tree, 'patchFile': name + '.patch',
                       'patchSha256': android.digest(content.encode()), 'sourceURL': 'https://github.com/fixture/' + name})
            self.repositories[module] = path
        self.go = self.root / 'go'
        self.go.write_text('tool stub, never executed')
        self.shared = android.provider()
        original_run = self.shared.run
        self.calls = []
        def provider_run(args, **kwargs):
            if args[0] == str(self.go):
                self.calls.append(('provider-go-stub', args))
                if args[1:] == ['version']:
                    return 'go version go1.25.5 linux/amd64\n'
                if args[1:3] == ['mod', 'edit']:
                    dep = next(dep for dep in self.source['dependencyPatches'] if args[3].startswith('-replace=' + dep['module'] + '='))
                    with (Path(kwargs['cwd']) / 'go.mod').open('a') as file:
                        file.write('\nreplace ' + dep['module'] + ' => ./polaris-dependencies/' + dep['name'] + '\n')
                    return ''
                if args[1:3] == ['list', '-m']:
                    return '\n'.join(json.dumps({'Path': dep['module'], 'Version': dep['upstreamVersion'],
                              'Replace': {'Path': './polaris-dependencies/' + dep['name'],
                                          'Dir': str(Path(kwargs['cwd']) / 'polaris-dependencies' / dep['name'])}})
                                     for dep in self.source['dependencyPatches'])
                if args[1:3] == ['version', '-m']:
                    abi = next(abi for abi in android.ABIS if Path(args[3]).name == 'jni_' + abi + '_libbox.so')
                    return self.build_info(abi)
                if args[1:3] == ['tool', 'buildid']:
                    return self.shared.source_linker_flag(self.source_receipt).removeprefix('-buildid=')
                raise AssertionError(args)
            return original_run(args, **kwargs)
        self.shared.run = provider_run
        self.write_configuration()
        baseline = self.root / 'baseline'
        self.source_receipt = self.shared.provision(self.patches / 'source-manifest.json', self.upstream, baseline, self.repositories, self.go)
        spec = self.core['sourceBuild']
        for field in ('sourceManifestSha256', 'provisionerSha256', 'moduleGraphSha256', 'patchedSourceTree', 'buildTree'):
            spec[field] = self.source_receipt[field]
        spec.update(sourceReceiptFingerprint=self.source_receipt['fingerprint'], version=self.core['bundledCoreVersion'] + '.polaris.1',
                    dependencyModules=list(FIXTURE_PATCHED), transportPins={'github.com/sagernet/gomobile': 'v0.1.12'})
        self.policy['requiredPatchedModules'] = list(FIXTURE_PATCHED)
        for target in self.policy['abis'].values():
            target['patchedModules'] = {'requiredLinked': list(FIXTURE_PATCHED), 'allowedAbsent': []}
            target['transportModules'] = {'requiredLinked': ['github.com/sagernet/gomobile'], 'confirmedAbsent': []}
        self.write_configuration()
        self.tool = {'go': self.go, 'jdk': self.root / 'jdk', 'ndk': self.root / 'ndk', 'gomobile': self.root / 'gomobile',
                     'jar': self.root / 'android.jar', 'env': {}, 'identity': {'toolStub': True, 'sdkBootclasspath': self.policy['sdkBootclasspath']}}
        self.identity = android.input_identity(self.source, self.core, self.policy, self.tool['identity'])
        self.calls.clear()

    def use_empty_dependencies(self):
        self.source['dependencyPatches'], self.repositories = [], {}
        self.core['sourceBuild']['dependencyModules'] = []
        self.policy['requiredPatchedModules'] = []
        for target in self.policy['abis'].values():
            target['patchedModules'] = {'requiredLinked': [], 'allowedAbsent': []}
        self.write_configuration()
        self.source_receipt = self.shared.provision(self.patches / 'source-manifest.json', self.upstream, self.root / 'baseline-empty', {}, None)
        spec = self.core['sourceBuild']
        for field in ('sourceManifestSha256', 'provisionerSha256', 'moduleGraphSha256', 'patchedSourceTree', 'buildTree'):
            spec[field] = self.source_receipt[field]
        spec['sourceReceiptFingerprint'] = self.source_receipt['fingerprint']
        self.write_configuration()
        self.identity = android.input_identity(self.source, self.core, self.policy, self.tool['identity'])

    def write_configuration(self):
        (self.patches / 'source-manifest.json').write_text(json.dumps(self.source))
        (self.patches / 'android-source-policy.json').write_text(json.dumps(self.policy))
        (self.app / 'src-tauri/core-manifest.json').write_text(json.dumps(self.core))

    def build_info(self, abi):
        target = self.policy['abis'][abi]
        rows = ['fixture.so: go1.25.5', '\tpath\tgithub.com/sagernet/sing-box/build/' + target['goarch'] + '/libbox']
        for dep in self.source['dependencyPatches']:
            rows += ['\tdep\t' + dep['module'] + '\t' + dep['upstreamVersion'], '\t=>\t./polaris-dependencies/' + dep['name'] + '\t(devel)\t', '\t']
        rows += ['\tdep\tgithub.com/sagernet/gomobile\tv0.1.12']
        for key, value in {'GOOS': 'android', 'GOARCH': target['goarch'], 'CGO_ENABLED': '1', '-buildmode': 'c-shared',
                           '-tags': ','.join(self.source['buildTags']), **target['architecture']}.items():
            rows.append('\tbuild\t' + key + '=' + value)
        return '\n'.join(rows) + '\n'

    def artifact(self, path, alignment=16384):
        with zipfile.ZipFile(path, 'w') as archive:
            for abi, target in self.policy['abis'].items():
                data = bytearray(b'\x7fELF' + bytes([target['elfClass'], 1, 1]) + bytes(9))
                if target['elfClass'] == 2:
                    data += struct.pack('<HHIQQQIHHHHHH', 3, target['elfMachine'], 1, 0, 64, 0, 0, 64, 56, 1, 0, 0, 0)
                    data += struct.pack('<IIQQQQQQ', 1, 5, 0, 0, 0, 0, 0, alignment)
                else:
                    data += struct.pack('<HHIIIIIHHHHHH', 3, target['elfMachine'], 1, 0, 52, 0, 0, 52, 32, 1, 0, 0, 0)
                    data += struct.pack('<IIIIIIII', 1, 0, 0, 0, 0, 0, 5, alignment)
                data += self.core['sourceBuild']['version'].encode() + b'cronet' * 101 + b'naive' * 11
                archive.writestr('jni/' + abi + '/libbox.so', data)
            archive.writestr('classes.jar', b'fake Java archive: JNI signatures are stubbed')

    def signatures(self, name):
        return {
            'Libbox': '\n'.join(['newTransientCommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface)',
                        'newStrictCommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface)',
                        'java.lang.String interfaceUpdateListenerIdentity(io.nekohasekai.libbox.InterfaceUpdateListener)',
                        'java.lang.String version()', 'checkConfigWithResult(java.lang.String, java.lang.String, long)', 'ConfigValidationContractVersion = "polaris-validation-v1"']),
            'CommandServer': 'startOrReloadService(java.lang.String, io.nekohasekai.libbox.OverrideOptions)\npublic native java.lang.String exportTailscaleStoreRetirement();',
            'PlatformInterface': 'void bindInterfaceControl(int, java.lang.String) throws java.lang.Exception',
            'ConfigValidationResult': '\n'.join('java.lang.String get' + getter + '()' for getter in ['RequestID','ConfigDigest','ContractVersion','Validation','Cleanup','ValidationError','CleanupError']) + '\npublic native java.lang.String getTailscaleStoreRetirement();\npublic native java.lang.String getTailscaleStoreMembership();',
            'ExchangeContext': '\n'.join(['void errnoCode(int)','void errorCode(int)','void onCancel(io.nekohasekai.libbox.Func)','void rawSuccess(byte[])','void success(java.lang.String)']),
            'LocalDNSTransport': '\n'.join(['void exchange(io.nekohasekai.libbox.ExchangeContext, byte[]) throws java.lang.Exception',
                              'void lookup(io.nekohasekai.libbox.ExchangeContext, java.lang.String, java.lang.String) throws java.lang.Exception','boolean raw()']),
            'Func': 'void invoke() throws java.lang.Exception'}[name]

    def callee(self, args, **kwargs):
        self.calls.append(('builder-callee-stub', args, kwargs.get('cwd')))
        if 'bind' in args:
            self.artifact(Path(args[args.index('-o') + 1]))
            return ''
        if '-constants' in args:
            return self.signatures(args[-1].split('.')[-1])
        if '-d' in args:
            return 'Shared library: [libc.so]\n'
        if '-f' in args:
            return str(Path(kwargs['cwd']) / 'experimental/libbox/config.go')
        if 'test' in args:
            return ''
        if args[0] == 'git':
            return subprocess.run(args, check=True, text=True, stdout=subprocess.PIPE).stdout
        raise AssertionError(args)

    def test_admission_refusal_order(self):
        self.core['sourceBuild']['sourceReceiptFingerprint'] = None
        self.write_configuration()
        with patch.object(builder.android, 'tools', side_effect=AssertionError('tool accessed')), patch.object(builder, 'run', side_effect=AssertionError('repo/Go accessed')), patch.object(sys, 'argv', ['build.py']):
            with self.assertRaisesRegex(RuntimeError, 'not frozen'):
                builder.main()
        with patch.object(verifier.android, 'tools', side_effect=AssertionError('tool/cache accessed')), patch.object(sys, 'argv', ['verify-receipt.py']):
            with self.assertRaisesRegex(RuntimeError, 'not frozen'):
                verifier.main()
        self.assertEqual(self.calls, [])

    def test_dependency_inventory_partial_policy_and_provider_hash(self):
        android.admit()
        # Declarations, the common inventory and the Android policy name one module set.
        declared = self.source['dependencyPatches']
        for dependencies, expected in [([], 'Dependency inventory differs'), (declared[:1], 'Dependency inventory differs'),
                                       (None, 'dependency graph')]:
            self.source['dependencyPatches'] = dependencies
            self.write_configuration()
            with self.assertRaisesRegex(RuntimeError, expected):
                android.admit()
        self.source['dependencyPatches'] = declared
        for required in ([], FIXTURE_PATCHED[:1], FIXTURE_PATCHED + FIXTURE_PATCHED[:1], FIXTURE_PATCHED + ['example.com/extra'], None):
            self.policy['requiredPatchedModules'] = required
            self.write_configuration()
            with self.assertRaisesRegex(RuntimeError, 'Unsupported Android source policy'):
                android.admit()
        self.policy['requiredPatchedModules'] = list(FIXTURE_PATCHED)
        self.write_configuration()
        android.admit()
        self.policy['abis']['x86']['patchedModules'] = None
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'partition'):
            android.admit()
        self.policy['abis']['x86']['patchedModules'] = self.policy['abis']['x86_64']['patchedModules']
        self.core['sourceBuild']['provisionerSha256'] = '0' * 64
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'provider hash'):
            android.admit()

    def test_empty_dependency_graph_admits_replays_and_builds_without_module_sources(self):
        self.use_empty_dependencies()
        self.assertEqual([row for row in self.calls if row[0] == 'provider-go-stub'], [], 'empty replay must not need Go')
        self.calls.clear()
        android.admit()
        receipt = self.source_receipt
        self.assertEqual((receipt['dependencies'], receipt['moduleGraph'], receipt['moduleGraphQueries'], receipt['moduleGraphSha256'],
                          receipt['graphScope'], receipt['sourceGraphState']),
                         ([], [], [], EMPTY_GRAPH_SHA256, 'core-source-only', 'source-only'))
        self.assertEqual(android.digest(android.canonical([])), EMPTY_GRAPH_SHA256)
        android.validate_source_receipt(receipt, self.source, self.core)
        for field, value in [('graphScope', 'declared-patched-modules'), ('sourceGraphState', 'dependencies-patched')]:
            broken = copy.deepcopy(receipt)
            broken[field] = value
            with self.assertRaisesRegex(RuntimeError, 'scope', msg=field):
                android.validate_source_receipt(broken, self.source, self.core)
        self.assertEqual(android.module_sources([], self.source), {})
        with self.assertRaisesRegex(RuntimeError, 'exactly cover'):
            android.module_sources([FIXTURE_PATCHED[0] + '=' + str(self.upstream)], self.source)
        for abi in android.ABIS:
            raw = self.build_info(abi)
            self.assertNotIn('=>', raw)
            android.validate_binary(raw, abi, self.source, self.core, self.policy)
            replaced = raw.replace('\tdep\tgithub.com/sagernet/gomobile\tv0.1.12',
                                   '\tdep\tgithub.com/sagernet/sing-tun\tv1.0.0\n\t=>\t./polaris-dependencies/sing-tun\t(devel)\t\n\t\n\tdep\tgithub.com/sagernet/gomobile\tv0.1.12')
            self.assertNotEqual(replaced, raw)
            with self.assertRaisesRegex(RuntimeError, 'Undeclared target module replacement'):
                android.validate_binary(replaced, abi, self.source, self.core, self.policy)
        # A policy or inventory that still names a module is refused for an empty declaration.
        self.policy['requiredPatchedModules'] = FIXTURE_PATCHED[:1]
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'Unsupported Android source policy'):
            android.admit()
        self.policy['requiredPatchedModules'] = []
        self.core['sourceBuild']['dependencyModules'] = FIXTURE_PATCHED[:1]
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'Dependency inventory differs'):
            android.admit()
        self.core['sourceBuild']['dependencyModules'] = []
        self.write_configuration()
        android.admit()
        temporary = tempfile.TemporaryDirectory
        def checkout_directory(**kwargs):
            return temporary(prefix='actual-builder-checkout-', dir=self.root)
        with patch.object(android, 'provider', return_value=self.shared), patch.object(android, 'tools', return_value=self.tool), patch.object(android, 'candidate', return_value=self.source['sourceCommit']), patch.object(builder, 'run', side_effect=self.callee), patch.object(builder.tempfile, 'TemporaryDirectory', side_effect=checkout_directory), patch.object(sys, 'argv', ['build.py', str(self.upstream)]):
            builder.main()
            built = json.loads((self.patches / 'build-receipt.json').read_bytes())
            self.assertEqual(len([row for row in self.calls if row[0] == 'builder-callee-stub' and 'bind' in row[1]]), 1)
            scratch = self.root / 'verify-empty'
            scratch.mkdir()
            builder.verify_component(self.app / 'src-tauri/gen/android/app/libs/libbox.aar', built, self.source, self.core,
                                     self.policy, self.tool, self.identity, scratch)
        with patch.object(sys, 'argv', ['build.py', str(self.upstream), '']):
            with self.assertRaises(SystemExit) as refused:
                builder.main()
            self.assertEqual(refused.exception.code, 2)

    def test_source_receipt_scope_query_manifest_and_buildtree(self):
        android.admit()
        android.validate_source_receipt(self.source_receipt, self.source, self.core)
        for field, value in [('graphScope', 'core-source-only'), ('sourceGraphState', 'source-only')]:
            broken = copy.deepcopy(self.source_receipt)
            broken[field] = value
            with self.assertRaisesRegex(RuntimeError, 'scope', msg=field):
                android.validate_source_receipt(broken, self.source, self.core)
        for field, value in [('graphScope','complete-graph'),('moduleGraphQueries',[]),('mainGoModSha256',''),('buildTree','0'*40),('patches',[]),('fingerprint','0'*64)]:
            broken = copy.deepcopy(self.source_receipt)
            broken[field] = value
            with self.assertRaises(RuntimeError, msg=field):
                android.validate_source_receipt(broken, self.source, self.core)

    def test_presence_target_stock_replacement_tool_version(self):
        for abi in android.ABIS:
            facts = android.validate_binary(self.build_info(abi), abi, self.source, self.core, self.policy)
            self.assertEqual(facts['modules']['github.com/sagernet/gomobile']['version'], 'v0.1.12')
        raw = self.build_info('arm64-v8a')
        for before, after in [('GOOS=android','GOOS=linux'),('CGO_ENABLED=1','CGO_ENABLED=0'),('GOARM64=v8.0','GOARM64=v9.0'),('v0.1.12','v0.1.13'),('./polaris-dependencies/patched','v1.0.0')]:
            with self.assertRaises(RuntimeError):
                android.validate_binary(raw.replace(before, after), 'arm64-v8a', self.source, self.core, self.policy)
        policy = copy.deepcopy(self.policy)
        policy['abis']['arm64-v8a']['transportModules'] = {'requiredLinked': [], 'confirmedAbsent': ['github.com/sagernet/gomobile']}
        with self.assertRaisesRegex(RuntimeError, 'absent transport'):
            android.validate_binary(raw, 'arm64-v8a', self.source, self.core, policy)
        # A complete, disjoint partition is refused once any declared module is absent, in every ABI.
        for abi in android.ABIS:
            for module in FIXTURE_PATCHED:
                changed = copy.deepcopy(self.policy)
                changed['abis'][abi]['patchedModules'] = {'requiredLinked': [item for item in FIXTURE_PATCHED if item != module], 'allowedAbsent': [module]}
                (self.patches / 'android-source-policy.json').write_text(json.dumps(changed))
                with self.assertRaisesRegex(RuntimeError, 'must link', msg=abi + ' ' + module):
                    android.admit()
        self.write_configuration()
        android.admit()

    def test_ignored_and_generated_source_boundary(self):
        checkout = self.root / 'baseline'
        with (checkout / '.git/info/exclude').open('a') as file:
            file.write('\nbuild/\n')
        android.verify_checkout(checkout, self.source_receipt, self.shared)
        generated = checkout / 'build/arm64/libbox/extra.go'
        generated.parent.mkdir(parents=True)
        generated.write_text('package gobind\n')
        with self.assertRaisesRegex(RuntimeError, 'Ignored compiler input'):
            android.verify_checkout(checkout, self.source_receipt, self.shared)
        generated.unlink()
        android.verify_checkout(checkout, self.source_receipt, self.shared)
        (checkout / 'go.mod').write_text('module stock\n')
        with self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
            android.verify_checkout(checkout, self.source_receipt, self.shared)

    def test_explicit_module_map(self):
        values = [module + '=' + str(repo) for module, repo in self.repositories.items()]
        self.assertEqual(android.module_sources(values, self.source), self.repositories)
        for broken in (values[:1], values + values[:1], values + ['extra=' + str(self.upstream)]):
            with self.assertRaises(RuntimeError):
                android.module_sources(broken, self.source)

    def test_sdk_bytes_and_canonical_identity(self):
        sdk = self.root / 'sdk'
        jar = sdk / 'platforms/android-36/android.jar'
        jar.parent.mkdir(parents=True)
        jar.write_bytes(b'original SDK jar fixture')
        policy = copy.deepcopy(self.policy)
        policy['sdkBootclasspath']['androidJarSha256'] = android.file_hash(jar)
        self.assertEqual(android.sdk_bootclasspath(sdk, policy), jar)
        jar.write_bytes(b'changed SDK jar fixture')
        with self.assertRaisesRegex(RuntimeError, 'SDK bootclasspath SHA'):
            android.sdk_bootclasspath(sdk, policy)
        original = android.input_identity(self.source, self.core, self.policy, self.tool['identity'])
        changed = copy.deepcopy(self.tool['identity'])
        changed['sdkBootclasspath']['androidJarSha256'] = '0' * 64
        self.assertNotEqual(original['fingerprint'], android.input_identity(self.source, self.core, self.policy, changed)['fingerprint'])
        self.assertNotIn(str(self.root), android.canonical(original).decode())
        original_dir = self.root / 'installed-tool'
        original_dir.mkdir()
        (original_dir / 'compiler').write_text('original')
        first = android.directory_identity(original_dir)
        (original_dir / 'compiler').write_text('changed')
        self.assertNotEqual(first, android.directory_identity(original_dir))

    def test_actual_tool_identity_separates_host_tool_from_target(self):
        goroot, jdk, ndk, sdk, mobile = [self.root / name for name in ('installed-go','jdk','ndk','sdk','mobile')]
        paths = [goroot / 'bin/go', jdk / 'bin/java', jdk / 'bin/javac', jdk / 'lib/modules',
                 ndk / 'toolchains/llvm/prebuilt/linux-x86_64/bin/clang',
                 ndk / 'toolchains/llvm/prebuilt/linux-x86_64/bin/ld.lld',
                 mobile / 'gomobile', mobile / 'gobind', sdk / 'platforms/android-36/android.jar']
        for path in paths:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('installed tool bytes fixture, never executed')
        (ndk / 'source.properties').write_text('Pkg.Revision = ' + self.source['ndkVersion'] + '\n')
        policy = copy.deepcopy(self.policy)
        policy['sdkBootclasspath']['androidJarSha256'] = android.file_hash(paths[-1])
        environments = []
        def tool_callee(args, **kwargs):
            environments.append(kwargs['env'])
            if args[1:] == ['version']:
                return 'go version go1.25.5 linux/amd64'
            if args[1:] == ['env','GOROOT']:
                return str(goroot)
            if args[1:] == ['--version']:
                return 'openjdk 17.0.12' if Path(args[0]).name == 'java' else 'javac 17.0.12'
            if args[1:3] == ['version','-m']:
                name = Path(args[3]).name
                return str(args[3]) + ': go1.24.3\n\tpath\tgithub.com/sagernet/gomobile/cmd/' + name + '\n\tmod\tgithub.com/sagernet/gomobile\tv0.1.13\th1:fixture\n'
            raise AssertionError(args)
        with patch.dict(os.environ, {'LIBBOX_GO': str(paths[0]), 'JDK17': str(jdk), 'ANDROID_HOME': str(sdk),
                      'LIBBOX_NDK_HOME': str(ndk), 'LIBBOX_MOBILE_BIN': str(mobile), 'GOARM': '5', 'GOENV': 'unreviewed'}):
            observed = android.tools(self.source, policy, tool_callee)
            self.assertEqual(observed['identity']['gomobile']['buildInfo']['goVersion'], '1.24.3')
            self.assertEqual(observed['identity']['gobind']['buildInfo']['modules']['github.com/sagernet/gomobile']['version'], 'v0.1.13')
            self.assertTrue(all(environment['GOARM'] == '7' and environment['GOENV'] == 'off' for environment in environments))
            self.assertNotIn(str(self.root), android.canonical(observed['identity']).decode())
            paths[-1].write_text('changed SDK bytes')
            with self.assertRaisesRegex(RuntimeError, 'SDK bootclasspath SHA'):
                android.tools(self.source, policy, tool_callee)

    def test_real_provider_to_original_tests_bind_and_shared_cache_predicate(self):
        original_provision = self.shared.provision
        def provision(*args):
            self.calls.append(('actual-provider', args))
            return original_provision(*args)
        self.shared.provision = provision
        temporary = tempfile.TemporaryDirectory
        def checkout_directory(**kwargs):
            return temporary(prefix='actual-builder-checkout-', dir=self.root)
        with patch.object(android, 'provider', return_value=self.shared), patch.object(android, 'tools', return_value=self.tool), patch.object(android, 'candidate', return_value=self.source['sourceCommit']), patch.object(builder, 'run', side_effect=self.callee), patch.object(builder.tempfile, 'TemporaryDirectory', side_effect=checkout_directory), patch.object(sys, 'argv', ['build.py', str(self.upstream), *['--module-source=' + module + '=' + str(path) for module,path in self.repositories.items()]]):
            builder.main()
            receipt = json.loads((self.patches / 'build-receipt.json').read_bytes())
            actual_provider = next(row for row in self.calls if row[0] == 'actual-provider')
            checkout = actual_provider[1][2]
            bind = [row for row in self.calls if row[0] == 'builder-callee-stub' and 'bind' in row[1]]
            self.assertEqual(len(bind), 1)
            self.assertEqual(bind[0][2], checkout)
            self.assertEqual(bind[0][1][bind[0][1].index('-bootclasspath') + 1], str(self.tool['jar']))
            self.assertIn(self.shared.source_linker_flag(self.source_receipt), bind[0][1][bind[0][1].index('-ldflags') + 1])
            tests = [row for row in self.calls if row[0] == 'builder-callee-stub' and 'test' in row[1]]
            self.assertEqual(len(tests), 7)
            self.assertTrue(all(row[2] == checkout for row in tests))
            scratch = self.root / 'verify'
            scratch.mkdir()
            aar = self.app / 'src-tauri/gen/android/app/libs/libbox.aar'
            builder.verify_component(aar, receipt, self.source, self.core, self.policy, self.tool, self.identity, scratch)
            for field, value in [('schema',None),('androidInput',{'fingerprint':'stock'}),('sourceReceipt',{}),('outputFingerprint','0'*64)]:
                wrong = copy.deepcopy(receipt)
                wrong[field] = value
                with self.assertRaises((RuntimeError, KeyError)):
                    builder.verify_component(aar, wrong, self.source, self.core, self.policy, self.tool, self.identity, scratch)
            original_source = self.shared.run
            def wrong_build_id(args, **kwargs):
                if args[1:3] == ['tool','buildid']:
                    return ''
                return original_source(args, **kwargs)
            with patch.object(self.shared, 'run', side_effect=wrong_build_id):
                with self.assertRaisesRegex(RuntimeError, 'source fingerprint'):
                    builder.verify_component(aar, receipt, self.source, self.core, self.policy, self.tool, self.identity, scratch)
            def wrong_java(args, **kwargs):
                result = self.callee(args, **kwargs)
                if args[-1] == 'io.nekohasekai.libbox.ConfigValidationResult':
                    return result + '\nvoid setCleanup(java.lang.String)'
                return result
            with patch.object(builder, 'run', side_effect=wrong_java):
                with self.assertRaisesRegex(RuntimeError, 'read-only'):
                    builder.verify_component(aar, receipt, self.source, self.core, self.policy, self.tool, self.identity, scratch)
            self.artifact(aar, alignment=4096)
            with self.assertRaisesRegex(RuntimeError, '16K'):
                builder.verify_component(aar, receipt, self.source, self.core, self.policy, self.tool, self.identity, scratch)

    def test_host_libbox_registry_tags_match_file_selection_and_test(self):
        self.test_real_provider_to_original_tests_bind_and_shared_cache_predicate()
        expected = ['with_tailscale', 'with_tailscale,with_ccm',
                    'with_tailscale,with_ocm', 'with_tailscale,with_ccm,with_ocm']
        calls = [row[1] for row in self.calls if row[0] == 'builder-callee-stub']
        lists = [args for args in calls if args[1] == 'list' and './experimental/libbox' in args]
        tests = [args for args in calls if args[1] == 'test' and
                 any(str(value).endswith('/config_validation_test.go') for value in args)]
        self.assertEqual(len(lists), 4)
        self.assertEqual(len(tests), 4)
        for commands in (lists, tests):
            self.assertEqual([args[args.index('-tags') + 1] if '-tags' in args else ''
                              for args in commands], expected)
        for args in tests:
            self.assertIn('-race', args)
            self.assertIn('-ldflags=-checklinkname=0', args)
            self.assertEqual(args[args.index('-count=1')], '-count=1')
        ordinary = [args for args in calls if args[1] == 'test' and args not in tests]
        self.assertEqual(len(ordinary), 3)
        self.assertTrue(all('-tags' not in args for args in ordinary))
        bind = next(args for args in calls if 'bind' in args)
        self.assertEqual(bind[bind.index('-tags') + 1], ','.join(self.source['buildTags']))

    def test_host_libbox_tags_reject_missing_capability_and_unknown_optionals(self):
        self.assertEqual(builder.libbox_test_tags(self.source), 'with_tailscale')
        self.assertEqual(builder.libbox_test_tags(self.source, 'with_ccm,with_ocm'),
                         'with_tailscale,with_ccm,with_ocm')
        for tags in ([], ['with_ccm'], ['with_tailscale', 'with_tailscale']):
            with self.subTest(tags=tags), self.assertRaisesRegex(RuntimeError, 'Tailscale registry capability'):
                builder.libbox_test_tags({'buildTags': tags})
        for optional in ('with_quic', 'with_tailscale', 'with_ocm,with_ccm',
                         'with_ccm,with_ccm', 'with_ccm,', 'with_ccm,with_ocm,with_quic'):
            with self.subTest(optional=optional), self.assertRaisesRegex(RuntimeError, 'optional registry test profile'):
                builder.libbox_test_tags(self.source, optional)
        self.assertNotIn('with_ccm', self.source['buildTags'])
        self.assertNotIn('with_ocm', self.source['buildTags'])
        self.assertEqual(builder.libbox_test_tags(self.source, 'with_ccm'), 'with_tailscale,with_ccm')
        self.assertEqual(builder.libbox_test_tags(self.source, 'with_ocm'), 'with_tailscale,with_ocm')

    def test_scoped_tailscale_java_methods_fresh_and_forged_cache(self):
        aar, scratch = self.root / 'scoped.aar', self.root / 'scoped-inspection'
        scratch.mkdir()
        self.artifact(aar)
        with patch.object(android, 'provider', return_value=self.shared), patch.object(builder, 'run', side_effect=self.callee):
            observed = builder.inspect_aar(aar, scratch, self.source, self.core, self.policy, self.tool, self.source_receipt)
            receipt = {'schema': 'polaris-android-libbox-receipt-v1',
                       'componentProducerCandidate': self.source['sourceCommit'],
                       'sourceCommit': self.source['sourceCommit'], 'officialTag': 'v' + self.core['bundledCoreVersion'],
                       'version': self.core['sourceBuild']['version'], 'sourceReceipt': self.source_receipt,
                       'androidInput': self.identity, 'linkerFlags': android.linker_flags(self.core, self.shared, self.source_receipt),
                       'buildVCS': False, 'tests': 'fixture callees only; no actual SDK or native execution',
                       'validationCleanupContract': {'version': 'polaris-validation-v1', 'constructedBoxCleanup': 'CleanupUnknown', 'exactCleanupEnabled': False},
                       **observed}
            receipt['aar']['path'] = 'src-tauri/gen/android/app/libs/libbox.aar'
            receipt['outputFingerprint'] = android.digest(android.canonical(receipt))
            builder.verify_component(aar, receipt, self.source, self.core, self.policy, self.tool, self.identity, scratch)
        aar_before, receipt_before = aar.read_bytes(), android.canonical(receipt)
        methods = [('ConfigValidationResult', 'getTailscaleStoreRetirement'),
                   ('ConfigValidationResult', 'getTailscaleStoreMembership'),
                   ('CommandServer', 'exportTailscaleStoreRetirement')]
        cases = []
        for name, method in methods:
            declaration = 'public native java.lang.String ' + method + '();'
            self.assertIn(declaration, observed['javaInterfaces'][name])
            for kind, replacement in [('missing', ''),
                                      ('wrong-return', declaration.replace('java.lang.String', 'void')),
                                      ('wrong-arity', declaration.replace('()', '(java.lang.String)')),
                                      ('private', declaration.replace('public', 'private')),
                                      ('static', declaration.replace('public native', 'public static native')),
                                      ('qualified-return', declaration.replace('java.lang.String', 'fixture.java.lang.String'))]:
                cases.append((name, method, kind, observed['javaInterfaces'][name].replace(declaration, replacement),
                              'Scoped Tailscale Java ABI differs: ' + name + '.' + method))
        for method in ('getTailscaleStoreRetirement', 'getTailscaleStoreMembership'):
            setter = 'set' + method[3:]
            changed = observed['javaInterfaces']['ConfigValidationResult'] + '\npublic native void ' + setter + '(java.lang.String);'
            cases.append(('ConfigValidationResult', method, 'setter', changed, 'must be read-only'))
        for name, method, kind, changed, error in cases:
            forged = copy.deepcopy(receipt)
            forged['javaInterfaces'][name] = changed
            forged.pop('outputFingerprint')
            forged['outputFingerprint'] = android.digest(android.canonical(forged))
            def wrong_java(args, **kwargs):
                if '-constants' in args and args[-1] == 'io.nekohasekai.libbox.' + name:
                    self.calls.append(('builder-callee-stub', args, kwargs.get('cwd')))
                    return changed
                return self.callee(args, **kwargs)
            with patch.object(android, 'provider', return_value=self.shared), patch.object(builder, 'run', side_effect=wrong_java):
                for leg in ('fresh', 'cache'):
                    with self.subTest(name=name, method=method, kind=kind, leg=leg):
                        with self.assertRaisesRegex(RuntimeError, re.escape(error)):
                            if leg == 'fresh':
                                builder.inspect_aar(aar, scratch, self.source, self.core, self.policy, self.tool, self.source_receipt)
                            else:
                                builder.verify_component(aar, forged, self.source, self.core, self.policy, self.tool, self.identity, scratch)
            self.assertEqual(aar.read_bytes(), aar_before)
            self.assertEqual(android.canonical(receipt), receipt_before)
        self.assertEqual(len(cases), 20)
        self.assertFalse(any(row[0] == 'builder-callee-stub' and 'bind' in row[1] for row in self.calls))

    def test_apk_consumption_preserves_component_candidate(self):
        apk = self.root / 'fixture.apk'
        native = b'fixture libbox bytes, not an executable'
        with zipfile.ZipFile(apk, 'w') as package:
            package.writestr('lib/arm64-v8a/libbox.so', native)
        r8 = self.root / 'r8'
        r8.mkdir()
        (r8 / 'mapping.txt').write_text('fixture R8 evidence, original verifier is stubbed')
        receipt = {'componentProducerCandidate': 'b' * 40, 'androidInput': {'fingerprint': 'c' * 64},
                   'sourceReceipt': {'fingerprint': 'd' * 64}, 'aar': {'sha256': 'e' * 64},
                   'nativeLibraries': {'jni/arm64-v8a/libbox.so': {'sha256': android.digest(native)}}}
        receipt_path = self.patches / 'build-receipt.json'
        receipt_path.write_text(json.dumps(receipt))
        original = receipt_path.read_bytes()
        for file in ('verify-apk.mjs','assert-r8-evidence.mjs'):
            (self.app / 'scripts' / file).write_text('fixture verifier identity')
        calls = []
        with patch.object(verifier.android, 'candidate', return_value='a' * 40), patch.object(verifier.builder, 'run', side_effect=lambda args, **kwargs: calls.append(args)):
            facts = verifier.consumption(apk, 'arm64-v8a', r8, receipt, receipt_path)
            self.assertEqual(facts['currentAppCandidate'], 'a' * 40)
            self.assertEqual(facts['componentProducerCandidate'], 'b' * 40)
            self.assertEqual(facts['apk']['sha256'], android.file_hash(apk))
            self.assertEqual(facts['componentReceiptSha256'], android.file_hash(receipt_path))
            self.assertEqual(len(calls), 2)
            self.assertTrue(calls[0][1].endswith('verify-apk.mjs'))
            self.assertTrue(calls[1][1].endswith('assert-r8-evidence.mjs'))
            self.assertEqual(receipt_path.read_bytes(), original)
            with zipfile.ZipFile(apk, 'w') as package:
                package.writestr('lib/arm64-v8a/libbox.so', b'stock replacement')
            with self.assertRaisesRegex(RuntimeError, 'embedded libbox differs'):
                verifier.consumption(apk, 'arm64-v8a', r8, receipt, receipt_path)
        self.assertEqual(receipt_path.read_bytes(), original)

    def test_publish_failure_preserves_original_pair(self):
        output, receipt_output, aar = self.root / 'output.aar', self.root / 'receipt.json', self.root / 'fresh.aar'
        output.write_bytes(b'old AAR')
        receipt_output.write_bytes(b'old receipt')
        aar.write_bytes(b'new AAR')
        replace = Path.replace
        def fail_receipt(path, target):
            if path.name == 'receipt.json.tmp':
                raise OSError('late receipt replace fixture')
            return replace(path, target)
        with patch.object(Path, 'replace', fail_receipt):
            with self.assertRaisesRegex(OSError, 'late receipt'):
                builder.publish(aar, {'aar': {'sha256': android.file_hash(aar)}}, output, receipt_output)
        self.assertEqual(output.read_bytes(), b'old AAR')
        self.assertEqual(receipt_output.read_bytes(), b'old receipt')
        self.assertFalse(output.with_suffix('.aar.tmp').exists())
        self.assertFalse(receipt_output.with_suffix('.json.tmp').exists())

    def test_workflow_prelookup_mirrors(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        mirrors = text.split('      - name: Cache libbox.aar')
        self.assertEqual(len(mirrors), 3)
        for before, after in zip(mirrors, mirrors[1:]):
            preparation = before[before.rindex('      - name: Prepare libbox tools'):]
            self.assertIn('id: libbox_input', preparation)
            self.assertIn('android-source.py identity', preparation)
            prior = before[:before.rindex('      - name: Prepare libbox tools')]
            self.assertIn('android-source.py admit', prior)
            cache, remaining = after.split('      - name: Build libbox.aar', 1)
            self.assertIn('key: libbox-source-v1-${{ runner.os }}-${{ steps.libbox_input.outputs.fingerprint }}', cache)
            self.assertNotIn('restore-keys:', cache)
            self.assertIn('verify-receipt.py', remaining)
        # Shell syntax is verified without executing tool/fetch/build commands.
        for block in re.findall(r'(?m)^        run: \|\n((?:^          .*\n|^\n)+)', text):
            shell = '\n'.join(line[10:] if line.startswith('          ') else '' for line in block.splitlines())
            shell = re.sub(r'\$\{\{.*?\}\}', 'fixture', shell)
            subprocess.run(['bash', '-n'], input=shell, text=True, check=True)


class CISourceFetchFixture(unittest.TestCase):
    def programs(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        blocks = re.findall(r'(?m)^          python3 - "\$RUNNER_TEMP" <<\'PYDEPS\'\n(.*?)^          PYDEPS$', text, re.S)
        self.assertEqual(len(blocks), 2)
        programs = ['\n'.join(line[10:] if line.startswith('          ') else '' for line in block.splitlines()) for block in blocks]
        self.assertEqual(programs[0], programs[1])
        self.assertGreater(len(programs[0]), 1000)
        return programs

    def test_actual_yaml_exact_fetch_and_refusal_controls(self):
        actual = json.loads((HERE / 'source-manifest.json').read_text())
        core = json.loads((ROOT / 'src-tauri/core-manifest.json').read_text())
        # The committed manifest decides the product fetch set; a synthetic
        # declaration keeps the retained dependency branch of the YAML covered.
        declared = copy.deepcopy(actual)
        declared['dependencyPatches'] = [{'name': 'fixture', 'module': 'example.com/fixture',
                                          'sourceURL': 'https://github.com/fixture/fixture', 'upstreamCommit': 'd' * 40}]
        cases = ['lightweight-tag', 'annotated-tag', 'fetch-failure', 'wrong-object', 'wrong-ref', 'missing-ref', 'wrong-tag', 'missing-tag']
        checked = 0
        for label, source in (('actual', actual), ('declared-dependency', declared)):
            core_url = source.get('sourceURL', 'https://github.com/SagerNet/sing-box')
            declarations = {'sing-box': (core_url, source['sourceCommit'])}
            declarations.update({'polaris-upstream-' + dep['name']: (dep['sourceURL'], dep['upstreamCommit']) for dep in source['dependencyPatches']})
            failing = sorted(declarations)[0]
            self.assertEqual(failing, 'polaris-upstream-fixture' if source['dependencyPatches'] else 'sing-box')
            for mirror, program in enumerate(self.programs()):
                for case in cases:
                    with self.subTest(manifest=label, mirror=mirror, case=case), tempfile.TemporaryDirectory(prefix='polaris-ci-source-fetch-fixture-') as temporary:
                        directory = Path(temporary) / 'runner'
                        workspace = Path(temporary) / 'workspace'
                        directory.mkdir()
                        for relative, value in [('scripts/libbox-patches/source-manifest.json', source), ('src-tauri/core-manifest.json', core)]:
                            (workspace / relative).parent.mkdir(parents=True)
                            (workspace / relative).write_text(json.dumps(value))
                        calls, refs = [], {}
                        tag = 'refs/tags/v' + core['bundledCoreVersion']
                        def run(arguments, **kwargs):
                            self.assertTrue(kwargs.get('check'), 'Git failures must stop source preparation')
                            calls.append(arguments)
                            if arguments[:3] == ['git', 'init', '--quiet']:
                                refs[arguments[3]] = {}
                            else:
                                self.assertEqual(arguments[:2], ['git', '-C'])
                                repository, command = arguments[2], arguments[3]
                                url, commit = declarations[Path(repository).name]
                                if command == 'fetch':
                                    self.assertEqual(arguments[4:6], ['--no-tags', '--depth=1'])
                                    self.assertEqual(arguments[6], url)
                                    target = arguments[7]
                                    if target == tag + ':' + tag:
                                        self.assertEqual(Path(repository).name, 'sing-box')
                                        if case != 'missing-tag':
                                            refs[repository][tag] = 'e' * 40 if case == 'annotated-tag' else source['sourceCommit']
                                            refs[repository][tag + '^{commit}'] = 'f' * 40 if case == 'wrong-tag' else source['sourceCommit']
                                    else:
                                        self.assertEqual(target, commit + ':refs/heads/polaris-source')
                                        if case == 'fetch-failure' and Path(repository).name == failing:
                                            raise subprocess.CalledProcessError(128, arguments)
                                        refs[repository][commit + '^{commit}'] = 'f' * 40 if case == 'wrong-object' else commit
                                        if case != 'missing-ref':
                                            refs[repository]['refs/heads/polaris-source'] = 'f' * 40 if case == 'wrong-ref' else commit
                                else:
                                    self.assertEqual(command, 'checkout')
                                    self.assertEqual(Path(repository).name, 'sing-box')
                                    self.assertEqual(arguments[4:], ['--detach', source['sourceCommit']])
                            return subprocess.CompletedProcess(arguments, 0)
                        def check_output(arguments, **kwargs):
                            self.assertEqual(arguments[:2], ['git', '-C'])
                            self.assertEqual(arguments[3], 'rev-parse')
                            self.assertTrue(kwargs.get('text'))
                            value = refs[arguments[2]].get(arguments[4])
                            if value is None:
                                raise subprocess.CalledProcessError(128, arguments)
                            return value + '\n'
                        cwd = Path.cwd()
                        try:
                            os.chdir(workspace)
                            with patch.object(sys, 'argv', ['yaml-source-fixture', str(directory)]), patch.object(subprocess, 'run', side_effect=run), patch.object(subprocess, 'check_output', side_effect=check_output):
                                if case in ('lightweight-tag', 'annotated-tag'):
                                    exec(compile(program, '<actual-android-yaml-source>', 'exec'), {})
                                else:
                                    with self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
                                        exec(compile(program, '<actual-android-yaml-source>', 'exec'), {})
                        finally:
                            os.chdir(cwd)
                        output = directory / 'polaris-module-sources.json'
                        if case in ('lightweight-tag', 'annotated-tag'):
                            self.assertEqual(json.loads(output.read_text()), ['--module-source=' + dep['module'] + '=' + str(directory / ('polaris-upstream-' + dep['name'])) for dep in source['dependencyPatches']])
                            self.assertEqual(len([args for args in calls if len(args) > 3 and args[3] == 'fetch']), len(declarations) + 1)
                        else:
                            self.assertFalse(output.exists(), 'A failed source fetch must not publish a builder handoff')
                        checked += 1
        self.assertEqual(checked, 32)
        print('Actual YAML source fetch controls: 32/32 PASS (two mirrors, committed and declared-dependency manifests; Git/Go/network callees not executed)')

    def test_actual_yaml_module_source_handoff_passes_no_empty_argument(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        handoffs = re.findall(r'(?m)^          (mapfile -t module_sources < <\(.*\)\n)          (bash scripts/build-libbox\.sh "\$src" "\$\{module_sources\[@\]\}"\n)', text)
        self.assertEqual(len(handoffs), 2)
        self.assertEqual(handoffs[0], handoffs[1])
        # Only the builder entry is replaced; the expansion itself is the YAML's.
        callee = 'bash() { printf "%s\\n" "$#"; printf "<%s>\\n" "$@"; }\nsrc=SOURCE\n'
        checked = 0
        for mirror, handoff in enumerate(handoffs):
            for values in ([], ['--module-source=example.com/one=/runner/one'],
                           ['--module-source=example.com/one=/runner/one', '--module-source=example.com/two=/runner/with space']):
                with self.subTest(mirror=mirror, values=values), tempfile.TemporaryDirectory(prefix='polaris-ci-module-handoff-fixture-') as directory:
                    (Path(directory) / 'polaris-module-sources.json').write_text(json.dumps(values))
                    result = subprocess.run(['bash', '-c', 'set -euo pipefail\n' + callee + ''.join(handoff)], text=True,
                                            env=os.environ | {'RUNNER_TEMP': directory}, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    arguments = ['scripts/build-libbox.sh', 'SOURCE', *values]
                    self.assertEqual(result.stdout, str(len(arguments)) + '\n' + ''.join('<' + value + '>\n' for value in arguments))
                    checked += 1
        self.assertEqual(checked, 6)

    def test_two_preludes_and_actual_shell_syntax(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        check, signed = text.split('\n  release-apk:\n', 1)
        start = '      - uses: actions/setup-node@v7'
        end = '          python3 scripts/libbox-patches/verify-receipt.py'
        preludes = [body[body.index(start):body.index(end) + len(end)] for body in (check, signed)]
        self.assertGreater(len(preludes[0]), 2000)
        self.assertEqual(preludes[0], preludes[1])
        for block in re.findall(r'(?m)^        run: \|\n((?:^          .*\n|^\n)+)', text):
            shell = '\n'.join(line[10:] if line.startswith('          ') else '' for line in block.splitlines())
            shell = re.sub(r'\$\{\{.*?\}\}', 'fixture', shell)
            subprocess.run(['bash', '-n'], input=shell, text=True, check=True)


class CIIdentityFixture(unittest.TestCase):
    def test_actual_yaml_identity_refusal_before_output_and_lookup(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        preparation_blocks = re.findall(
            r'(?m)^      - name: Prepare libbox tools and source cache identity\n((?:^        .*\n|^\n)+)', text)
        self.assertEqual(len(preparation_blocks), 2)
        cases = [('exit7', '', 7, 7), ('empty', '', 0, 1), ('uppercase64', 'A' * 64, 0, 1),
                 ('nonhex64', 'g' * 64, 0, 1), ('short63', 'a' * 63, 0, 1),
                 ('long65', 'a' * 65, 0, 1), ('valid64', '0123456789abcdef' * 4, 0, 0)]
        checked = 0
        for index, block in enumerate(preparation_blocks):
            run_body = block.split('        run: |\n', 1)[1]
            actual_lines = [line[10:] for line in run_body.splitlines() if line.startswith('          ')]
            identity_line = next(i for i, line in enumerate(actual_lines) if 'android-source.py identity' in line)
            # Execute the actual YAML suffix and its actual shell failure policy.
            # Only the identity callee is replaced; preceding SDK/tool preparation is outside this finite fixture.
            source = actual_lines[0] + '\n' + '\n'.join(actual_lines[identity_line:]) + '\n'
            for name, value, identity_exit, expected_exit in cases:
                with self.subTest(mirror=index, case=name), tempfile.TemporaryDirectory(prefix='polaris-ci-identity-fixture-') as directory:
                    output = Path(directory) / 'github-output'
                    original = 'preexisting=retained\n'
                    output.write_text(original)
                    environment = os.environ.copy()
                    environment.update(GITHUB_OUTPUT=str(output), IDENTITY_FIXTURE_VALUE=value,
                                       IDENTITY_FIXTURE_EXIT=str(identity_exit))
                    callee = ('python3() { [[ "$*" == "scripts/libbox-patches/android-source.py identity" ]] || return 97; '
                              'printf "%s" "$IDENTITY_FIXTURE_VALUE"; return "$IDENTITY_FIXTURE_EXIT"; }\n')
                    result = subprocess.run(['bash', '-c', callee + source + 'printf "cache_lookup_reached\\n"\n'],
                                            env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    self.assertEqual(result.returncode, expected_exit, result.stderr)
                    self.assertEqual(output.read_text(), original + ('fingerprint=' + value + '\n' if expected_exit == 0 else ''))
                    self.assertEqual(result.stdout, 'cache_lookup_reached\n' if expected_exit == 0 else '')
                    checked += 1
        self.assertEqual(checked, 14)
        print('Actual YAML identity shell controls: 14/14 PASS (two mirrors; no tool/cache execution)')


class CIAppCleanFixture(unittest.TestCase):
    def bytecode_environment(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        match = re.search(r"(?m)^env:\n  PYTHONDONTWRITEBYTECODE: '1'\n", text)
        self.assertIsNotNone(match, 'Both Android jobs must inherit no-bytecode before any provider import')
        environment = os.environ.copy()
        environment.pop('PYTHONPYCACHEPREFIX', None)
        environment.update(PYTHONDONTWRITEBYTECODE='1', POLARIS_NO_KERNEL_RUN='1')
        return environment

    def app(self, directory):
        app = Path(directory) / 'app'
        files = ['.gitignore', 'scripts/libbox-patches/.gitignore',
                 'scripts/libbox-patches/android-source.py', 'scripts/core-source-provision.py']
        candidate = repository(app, {name: (ROOT / name).read_text() for name in files})
        git('config', 'core.excludesFile', os.devnull, cwd=app)
        return app, candidate

    def load_provider(self, app, environment):
        # Execute the actual identity provider import and actual clean predicate.
        # No admission/tools/build action is called; the provider itself only imports here.
        code = """import importlib.util, pathlib, subprocess
path = pathlib.Path('scripts/libbox-patches/android-source.py')
spec = importlib.util.spec_from_file_location('clean_fixture', path)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
module.provider()
def run(arguments, **kwargs):
    kwargs.pop('capture', None)
    return subprocess.run(arguments, check=True, text=True, stdout=subprocess.PIPE, **kwargs).stdout
print(module.candidate(run))
"""
        return subprocess.run([sys.executable, '-c', code], cwd=app, env=environment,
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def test_real_provider_import_keeps_candidate_clean_without_ignoring_bytecode(self):
        environment = self.bytecode_environment()
        for disabled in (False, True):
            with self.subTest(no_bytecode=disabled), tempfile.TemporaryDirectory(prefix='polaris-ci-app-clean-') as directory:
                app, candidate = self.app(directory)
                current = environment.copy()
                if not disabled:
                    current.pop('PYTHONDONTWRITEBYTECODE')
                result = self.load_provider(app, current)
                self.assertEqual(git('diff', '--name-only', cwd=app), '')
                status = git('status', '--porcelain', '--untracked-files=all', cwd=app)
                if disabled:
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout.strip(), candidate)
                    self.assertEqual(status, '')
                    self.assertFalse((app / 'scripts/__pycache__').exists())
                else:
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('App source candidate must be clean', result.stderr)
                    self.assertRegex(status, r'^\?\? scripts/__pycache__/core-source-provision\.cpython-[0-9]+\.pyc$')

    def test_real_candidate_still_rejects_tracked_and_untracked_compiler_inputs(self):
        environment = self.bytecode_environment()
        for kind in ('tracked', 'untracked'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory(prefix='polaris-ci-app-dirty-') as directory:
                app, _ = self.app(directory)
                path = app / ('scripts/core-source-provision.py' if kind == 'tracked' else 'scripts/unowned-compiler.py')
                path.write_text(path.read_text() + '\n# changed compiler input\n' if kind == 'tracked' else '# unowned compiler input\n')
                before = git('status', '--porcelain', '--untracked-files=all', cwd=app)
                result = self.load_provider(app, environment)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('App source candidate must be clean', result.stderr)
                self.assertEqual(git('status', '--porcelain', '--untracked-files=all', cwd=app), before)

    def test_actual_build_preludes_diagnose_exact_dirty_paths_and_stop(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        blocks = re.findall(r'(?m)^      - name: Build libbox.aar \(缓存未命中\)\n((?:^        .*\n|^\n)+)', text)
        self.assertEqual(len(blocks), 2)
        for mirror, block in enumerate(blocks):
            body = block.split('        run: |\n', 1)[1]
            shell = '\n'.join(line[10:] for line in body.splitlines() if line.startswith('          '))
            prelude = shell.split('src="$RUNNER_TEMP/sing-box"', 1)[0]
            self.assertIn('git status --porcelain --untracked-files=normal', prelude)
            self.assertIn('git status --porcelain --untracked-files=all', prelude)
            for kind in ('clean', 'tracked', 'untracked'):
                with self.subTest(mirror=mirror, kind=kind), tempfile.TemporaryDirectory(prefix='polaris-ci-clean-prelude-') as directory:
                    app, _ = self.app(directory)
                    path = app / ('scripts/core-source-provision.py' if kind == 'tracked' else 'scripts/unowned-compiler.py')
                    if kind == 'tracked':
                        path.write_text(path.read_text() + '\n# changed compiler input\n')
                    elif kind == 'untracked':
                        path.write_text('# unowned compiler input\n')
                    before = git('status', '--porcelain', '--untracked-files=all', cwd=app)
                    result = subprocess.run(['bash', '-c', prelude + '\nprintf "build_reached\\n"\n'],
                                            cwd=app, env=self.bytecode_environment(), text=True,
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    self.assertEqual(result.returncode, 0 if kind == 'clean' else 1, result.stderr)
                    self.assertEqual(git('status', '--porcelain', '--untracked-files=all', cwd=app), before)
                    if kind == 'clean':
                        self.assertEqual(result.stdout, 'build_reached\n')
                    else:
                        self.assertNotIn('build_reached', result.stdout)
                        self.assertIn('App source candidate must be clean before libbox build', result.stdout)
                        self.assertIn(str(path.relative_to(app)), result.stdout)


class CIReleaseResourceFixture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.temp = tempfile.TemporaryDirectory(prefix='polaris-release-resource-fixture-')
        cls.addClassCleanup(cls.temp.cleanup)
        cls.root = Path(cls.temp.name)
        manifest = cls.root / 'src-tauri'
        manifest.mkdir()
        source = (ROOT / 'src-tauri/build.rs').read_text()
        # Compile the actual std-only resource guards, not a second implementation.
        count = re.search(r'^const EXPECTED_SRS_COUNT: usize = \d+;$', source, re.M).group()
        guards = source[source.index('fn assert_bundled_dashboard()'):]
        probe = cls.root / 'probe.rs'
        probe.write_text('use std::path::Path;\n' + count + '\n' + guards +
                         '\nfn main() { assert_bundled_geo_data(); assert_bundled_dashboard(); }\n')
        cls.binary = cls.root / ('probe.exe' if os.name == 'nt' else 'probe')
        environment = os.environ.copy()
        environment['CARGO_MANIFEST_DIR'] = str(manifest)
        subprocess.run(['rustc', '--edition=2021', '-Dwarnings', str(probe), '-o', str(cls.binary)],
                       env=environment, check=True, capture_output=True, text=True)

    def test_actual_android_and_desktop_release_guards(self):
        cases = [
            ('android', 'release', 28, None, None, True),
            ('android', 'release', 28, None, b'', True),
            ('linux', 'release', 28, None, None, False),
            ('windows', 'release', 28, None, None, False),
            ('macos', 'release', 28, None, None, False),
            ('linux', 'release', 28, None, b'', False),
            ('linux', 'release', 28, None, b'<html>fixture</html>', True),
            ('unknown', 'release', 28, None, None, False),
            ('android', 'release', None, None, None, False),
            ('android', 'release', 27, None, None, False),
            ('android', 'release', 28, b'', None, False),
            ('android', 'release', 28, b'404 HTML', None, False),
            ('android', 'debug', None, None, None, True),
            ('linux', 'debug', None, None, None, True),
        ]
        passed = 0
        for target, profile, count, bad, dashboard, allowed in cases:
            with self.subTest(target=target, profile=profile, geo=count, bad=bad, dashboard=dashboard):
                resources = self.root / 'resources'
                shutil.rmtree(resources, ignore_errors=True)
                if count is not None:
                    data = resources / 'data'
                    data.mkdir(parents=True)
                    for index in range(count):
                        (data / (str(index) + '.srs')).write_bytes(b'SRSfixture')
                    if bad is not None:
                        (data / 'bad.srs').write_bytes(bad)
                if dashboard is not None:
                    directory = resources / 'dashboard'
                    directory.mkdir(parents=True)
                    (directory / 'index.html').write_bytes(dashboard)
                environment = os.environ.copy()
                environment.update(CARGO_CFG_TARGET_OS=target, PROFILE=profile)
                result = subprocess.run([str(self.binary)], env=environment, text=True,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                self.assertEqual(result.returncode == 0, allowed, result.stderr)
                if not allowed:
                    self.assertIn('拒绝出包', result.stderr)
                passed += 1
        print(f'Actual Rust resource guards: {passed}/14 passed (owned fixtures; no Tauri/backend execution)')

    def test_android_ownership_and_unconditional_geo_call(self):
        source = (ROOT / 'src-tauri/build.rs').read_text()
        self.assertIn('    export_product_name();\n    assert_bundled_geo_data();\n    assert_bundled_dashboard();', source)
        self.assertIn('cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS', source)
        android_config = json.loads((ROOT / 'src-tauri/tauri.android.conf.json').read_bytes())
        self.assertEqual(android_config['bundle']['resources'], [
            '../resources/data/', '../THIRD-PARTY-LICENSES.md', '../NOTICE', '../LICENSE'])
        desktop_config = json.loads((ROOT / 'src-tauri/tauri.conf.json').read_bytes())
        self.assertIn('../resources/dashboard/', desktop_config['bundle']['resources'])


class CIVerifiedCacheFixture(unittest.TestCase):
    def legs(self):
        text = (ROOT / '.github/workflows/android.yml').read_text()
        unsigned, signed = text.split('\n  release-apk:\n', 1)
        return [re.findall(r'(?m)^      - name: ([^\n]+)\n((?:^        .*\n|^\n)+)', body)
                for body in (unsigned, signed)]

    def cache_steps(self, steps):
        names = [name for name, _ in steps]
        restore_index = names.index('Cache libbox.aar')
        verify_index = names.index('libbox.aar 必须在位')
        save_index = names.index('Save verified libbox.aar')
        self.assertLess(restore_index, names.index('Build libbox.aar (缓存未命中)'))
        self.assertEqual(save_index, verify_index + 1)
        self.assertLess(save_index, next(i for i, name in enumerate(names) if 'APK' in name and name.startswith('Build')))
        restore, verify, save = [steps[i][1] for i in (restore_index, verify_index, save_index)]
        self.assertIn('uses: actions/cache/restore@v6', restore)
        self.assertIn('id: libbox\n', restore)
        self.assertIn('key: libbox-source-v1-${{ runner.os }}-${{ steps.libbox_input.outputs.fingerprint }}', restore)
        self.assertNotIn('restore-keys:', restore)
        self.assertIn('id: libbox_verified\n', verify)
        self.assertNotIn('continue-on-error:', verify)
        self.assertIn('python3 scripts/libbox-patches/verify-receipt.py', verify)
        self.assertIn('uses: actions/cache/save@v6', save)
        self.assertIn('key: ${{ steps.libbox.outputs.cache-primary-key }}', save)
        expected_paths = ['src-tauri/gen/android/app/libs/libbox.aar', 'scripts/libbox-patches/build-receipt.json']
        for block in (restore, save):
            paths = re.search(r'(?m)^          path: \|\n((?:^            .*\n)+)', block).group(1)
            self.assertEqual([line.strip() for line in paths.splitlines()], expected_paths)
        condition = re.search(r'(?m)^        if: (.*)$', save).group(1)
        return verify.split('        run: |\n', 1)[1], condition

    @staticmethod
    def admitted(condition, hit, outcome, success):
        # Evaluate only the closed literal expression taken from the actual YAML.
        expression = condition.replace('success()', repr(success))
        expression = expression.replace('steps.libbox.outputs.cache-hit', repr(hit))
        expression = expression.replace('steps.libbox_verified.outcome', repr(outcome)).replace('&&', 'and')
        parsed = ast.parse(expression, mode='eval')
        if any(not isinstance(node, (ast.Expression, ast.BoolOp, ast.And, ast.Compare,
                                     ast.NotEq, ast.Eq, ast.Constant)) for node in ast.walk(parsed)):
            raise AssertionError('Cache admission is outside the closed success/miss/verification expression')
        return eval(compile(parsed, '<actual-cache-if>', 'eval'), {'__builtins__': {}}, {})

    def test_actual_verifier_failure_and_save_admission(self):
        cases = [('false', 'valid', 0, True), ('', 'valid', 0, True), ('true', 'valid', 0, False),
                 ('false', 'missing', 0, False), ('false', 'empty', 0, False),
                 ('false', 'valid', 23, False), ('true', 'valid', 23, False)]
        passed = 0
        for leg, steps in enumerate(self.legs()):
            body, condition = self.cache_steps(steps)
            for hit, state, verify_exit, saved in cases:
                with self.subTest(leg=leg, hit=hit, state=state, verify_exit=verify_exit), \
                        tempfile.TemporaryDirectory(prefix='polaris-cache-verifier-fixture-') as directory:
                    app = Path(directory)
                    aar = app / 'src-tauri/gen/android/app/libs/libbox.aar'
                    aar.parent.mkdir(parents=True)
                    if state != 'missing':
                        aar.write_bytes(b'private fixture only' if state == 'valid' else b'')
                    trace = app / 'verifier-called'
                    environment = os.environ.copy()
                    environment.update(VERIFY_EXIT=str(verify_exit), VERIFY_TRACE=str(trace))
                    run = '\n'.join(line[10:] if line.startswith('          ') else '' for line in body.splitlines())
                    run = run.replace('${{ steps.libbox.outputs.cache-hit }}', hit)
                    callee = ('python3() { [[ "$*" == "scripts/libbox-patches/verify-receipt.py" ]] || return 97; '
                              'printf "actual verifier invocation\\n" >> "$VERIFY_TRACE"; return "$VERIFY_EXIT"; }\n')
                    result = subprocess.run(['bash', '-c', callee + run], cwd=app, env=environment,
                                            text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    self.assertEqual(result.returncode, verify_exit if state == 'valid' else 1, result.stderr)
                    self.assertEqual(trace.exists(), state == 'valid')
                    outcome = 'success' if result.returncode == 0 else 'failure'
                    self.assertEqual(self.admitted(condition, hit, outcome, result.returncode == 0), saved)
                    self.assertFalse(self.admitted(condition, 'false', 'skipped', True))
                    self.assertFalse(self.admitted(condition, 'false', 'cancelled', True))
                    self.assertFalse(self.admitted(condition, 'false', 'success', False))
                    passed += 1
        print(f'Actual YAML verification shell/cache admission: {passed}/14 passed (callee stub; cache/backend not executed)')

    def test_two_legs_and_save_predicate_mutations(self):
        values = [self.cache_steps(steps) for steps in self.legs()]
        self.assertEqual(values[0], values[1])
        condition = values[0][1]
        for target, hit, outcome, success in [
                ("steps.libbox.outputs.cache-hit != 'true'", 'true', 'success', True),
                ("steps.libbox_verified.outcome == 'success'", 'false', 'failure', True),
                ('success()', 'false', 'success', False)]:
            with self.subTest(removed=target):
                self.assertFalse(self.admitted(condition, hit, outcome, success))
                self.assertTrue(self.admitted(condition.replace(target, 'True'), hit, outcome, success))


if __name__ == '__main__':
    unittest.main(verbosity=2)
