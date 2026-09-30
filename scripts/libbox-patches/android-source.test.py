#!/usr/bin/env python3
"""Finite host fixtures: real provider/Git replay; tool, bind and JNI callees are stubs."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import re
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
        for index, module in enumerate(android.REQUIRED_PATCHED):
            name = ('tun', 'nft')[index]
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
                    dependencyModules=list(android.REQUIRED_PATCHED), transportPins={'github.com/sagernet/gomobile': 'v0.1.12'})
        for target in self.policy['abis'].values():
            target['patchedModules'] = {'requiredLinked': list(android.REQUIRED_PATCHED), 'allowedAbsent': []}
            target['transportModules'] = {'requiredLinked': ['github.com/sagernet/gomobile'], 'confirmedAbsent': []}
        self.write_configuration()
        self.tool = {'go': self.go, 'jdk': self.root / 'jdk', 'ndk': self.root / 'ndk', 'gomobile': self.root / 'gomobile',
                     'jar': self.root / 'android.jar', 'env': {}, 'identity': {'toolStub': True, 'sdkBootclasspath': self.policy['sdkBootclasspath']}}
        self.identity = android.input_identity(self.source, self.core, self.policy, self.tool['identity'])
        self.calls.clear()

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
            'CommandServer': 'startOrReloadService(java.lang.String, io.nekohasekai.libbox.OverrideOptions)',
            'PlatformInterface': 'void bindInterfaceControl(int, java.lang.String) throws java.lang.Exception',
            'ConfigValidationResult': '\n'.join('java.lang.String get' + getter + '()' for getter in ['RequestID','ConfigDigest','ContractVersion','Validation','Cleanup','ValidationError','CleanupError']),
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

    def test_empty_dependencies_partial_policy_and_provider_hash(self):
        self.source['dependencyPatches'] = []
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'dependency graph'):
            android.admit()
        self.source['dependencyPatches'] = self.source_receipt['dependencies']
        # Restore exact declarations without provider-added fields.
        self.source['dependencyPatches'] = [{key: value for key, value in dep.items() if key not in ('upstreamTree','replacement')} for dep in self.source['dependencyPatches']]
        self.core['sourceBuild']['sourceManifestSha256'] = android.file_hash(self.patches / 'source-manifest.json')
        self.write_configuration()
        self.core['sourceBuild']['sourceManifestSha256'] = android.file_hash(self.patches / 'source-manifest.json')
        self.policy['abis']['x86']['patchedModules'] = None
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'partition'):
            android.admit()
        self.policy['abis']['x86']['patchedModules'] = self.policy['abis']['x86_64']['patchedModules']
        self.core['sourceBuild']['provisionerSha256'] = '0' * 64
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'provider hash'):
            android.admit()

    def test_source_receipt_scope_query_manifest_and_buildtree(self):
        android.admit()
        android.validate_source_receipt(self.source_receipt, self.source, self.core)
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
        for before, after in [('GOOS=android','GOOS=linux'),('CGO_ENABLED=1','CGO_ENABLED=0'),('GOARM64=v8.0','GOARM64=v9.0'),('v0.1.12','v0.1.13'),('./polaris-dependencies/tun','v1.0.0')]:
            with self.assertRaises(RuntimeError):
                android.validate_binary(raw.replace(before, after), 'arm64-v8a', self.source, self.core, self.policy)
        policy = copy.deepcopy(self.policy)
        policy['abis']['arm64-v8a']['transportModules'] = {'requiredLinked': [], 'confirmedAbsent': ['github.com/sagernet/gomobile']}
        with self.assertRaisesRegex(RuntimeError, 'absent transport'):
            android.validate_binary(raw, 'arm64-v8a', self.source, self.core, policy)
        self.policy['abis']['x86']['patchedModules']['requiredLinked'].remove('github.com/sagernet/nftables')
        self.policy['abis']['x86']['patchedModules']['allowedAbsent'].append('github.com/sagernet/nftables')
        self.write_configuration()
        with self.assertRaisesRegex(RuntimeError, 'must link'):
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
        source = json.loads((HERE / 'source-manifest.json').read_text())
        core = json.loads((ROOT / 'src-tauri/core-manifest.json').read_text())
        core_url = source.get('sourceURL', 'https://github.com/SagerNet/sing-box')
        declarations = {'sing-box': (core_url, source['sourceCommit'])}
        declarations.update({'polaris-upstream-' + dep['name']: (dep['sourceURL'], dep['upstreamCommit']) for dep in source['dependencyPatches']})
        cases = ['lightweight-tag', 'annotated-tag', 'fetch-failure', 'wrong-object', 'wrong-ref', 'missing-ref', 'wrong-tag', 'missing-tag']
        checked = 0
        for mirror, program in enumerate(self.programs()):
            for case in cases:
                with self.subTest(mirror=mirror, case=case), tempfile.TemporaryDirectory(prefix='polaris-ci-source-fetch-fixture-') as temporary:
                    directory = Path(temporary)
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
                                    if case == 'fetch-failure' and Path(repository).name == 'polaris-upstream-sing-tun':
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
                        os.chdir(ROOT)
                        with patch.object(sys, 'argv', ['yaml-source-fixture', temporary]), patch.object(subprocess, 'run', side_effect=run), patch.object(subprocess, 'check_output', side_effect=check_output):
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
        self.assertEqual(checked, 16)
        print('Actual YAML source fetch controls: 16/16 PASS (two mirrors; Git/Go/network callees not executed)')

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


if __name__ == '__main__':
    unittest.main(verbosity=2)
