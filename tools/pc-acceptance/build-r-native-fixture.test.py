#!/usr/bin/env python3
"""Finite producer counterexamples with mocked Go. No compiler/list/native execution."""
import contextlib
import copy
import importlib.util
import io
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('r_fixture_producer', Path(__file__).with_name('build-r-native-fixture.py'))
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
H = M.helper_module()


def original(name):
    data = (ROOT / 'scripts/libbox-patches/sing-tun-owned-native.patch').read_bytes()
    marker = f'diff --git a/{name} b/{name}\n'.encode()
    section = data.split(marker, 1)[1]
    boundary = section.find(b'\ndiff --git ')
    if boundary != -1:
        section = section[:boundary + 1]  # Keep the actual final added line's LF.
    return b''.join(line[1:] for line in section.splitlines(keepends=True)
                    if line.startswith(b'+') and not line.startswith(b'+++'))


class ProducerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='polaris-r-producer-pure-')
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.checkout, self.output = self.base / 'canonical-double', self.base / 'observations'
        self.checkout.mkdir()
        self.go = self.base / 'toolchain/bin/go'
        for name in ('bin/go', 'pkg/tool/linux_amd64/compile', 'pkg/tool/linux_amd64/link',
                     'pkg/tool/linux_amd64/asm', 'src/testing/testing.go'):
            file = self.base / 'toolchain' / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_text('pure tool/source double, never executable\n')
        self.cache, self.module_cache = self.base / 'cache', self.base / 'modules'
        self.cache.mkdir()
        self.module_cache.mkdir()
        (self.cache / 'keep').write_text('shared cache sentinel')
        (self.cache / '_testmain.go').write_text('package main // mock generated input\n')
        (self.checkout / 'go.mod').write_text('module example.test/main\n')
        (self.checkout / 'go.sum').write_text('fixed pure double\n')
        (self.checkout / 'production.go').write_text('package main\n')
        self.dependencies = []
        for name, module in (('sing-tun', M.TARGET), ('nftables', 'github.com/sagernet/nftables')):
            root = self.checkout / 'polaris-dependencies' / name
            root.mkdir(parents=True)
            (root / 'go.mod').write_text('module ' + module + '\n')
            self.dependencies.append({'module': module, 'name': name, 'replacement': './polaris-dependencies/' + name,
                                      'upstreamVersion': 'v1.0.0'})
        tun = self.checkout / 'polaris-dependencies/sing-tun'
        for name in ('route_native_receipt_linux_test.go', 'route_native_kernel_linux_test.go', 'testdata/r-native-profile-v1.json'):
            file = tun / name
            file.parent.mkdir(parents=True, exist_ok=True)
            file.write_bytes(original(name))
        self.receipt = {'dependencies': self.dependencies, 'buildTree': '1' * 40,
                        'mainGoModSha256': H.file_hash(self.checkout / 'go.mod'),
                        'mainGoSumSha256': H.file_hash(self.checkout / 'go.sum')}
        self.receipt_path = self.base / 'receipt.json'
        self.receipt_path.write_text(json.dumps(self.receipt))
        self.calls, self.mutate, self.bad_elf = [], None, None
        self.command_envs, self.effective_json = [], []
        self.effective_goenv, self.omit_goenv = '', False
        self.version = 'go version go1.25.5 linux/amd64\n'
        self.loader = patch.object(M, 'load_inputs', return_value=({'goVersion': '1.25.5'}, self.receipt,
                                                                 {str(self.receipt_path): H.file_hash(self.receipt_path)}))
        self.loader.start()
        self.addCleanup(self.loader.stop)
        self.verify = patch.object(H, 'verify_checkout')
        self.verify.start()
        self.addCleanup(self.verify.stop)
        self.helper = patch.object(M, 'helper_module', return_value=H)
        self.helper.start()
        self.addCleanup(self.helper.stop)

    def module_rows(self, root):
        return [{'Path': 'example.test/main', 'Main': True, 'Dir': str(root), 'GoMod': str(root / 'go.mod')}] + [
            {'Path': dep['module'], 'Version': dep['upstreamVersion'],
             'Replace': {'Path': dep['replacement'], 'Dir': str(root / dep['replacement']),
                         'GoMod': str(root / dep['replacement'] / 'go.mod')}} for dep in self.dependencies]

    def package_rows(self, root):
        module = self.module_rows(root)[1]
        tun = root / 'polaris-dependencies/sing-tun'
        augmented = M.TARGET + ' [' + M.TARGET + '.test]'
        return [
            {'ImportPath': 'testing', 'Standard': True, 'Dir': str(self.base / 'toolchain/src/testing'), 'GoFiles': ['testing.go']},
            {'ImportPath': M.TARGET, 'Dir': str(tun), 'Module': module, 'Imports': ['testing'],
             'GoFiles': ['route_native_receipt_linux_test.go'], 'TestGoFiles': ['route_native_kernel_linux_test.go']},
            {'ImportPath': augmented, 'ForTest': M.TARGET, 'Dir': str(tun), 'Module': module, 'Imports': ['testing'],
             'GoFiles': ['route_native_receipt_linux_test.go', 'route_native_kernel_linux_test.go']},
            {'ImportPath': M.TARGET + '.test', 'Name': 'main', 'Dir': str(tun), 'Module': module,
             'GoFiles': [str(self.cache / '_testmain.go')], 'Imports': [M.TARGET, 'testing'], 'ImportMap': {M.TARGET: augmented}},
        ]

    def runner(self, output, env):
        test = self
        default_env = env

        class MockGo:
            def run(self, args, *, cwd, env=None, raw=False):
                argv = [str(value) for value in args]
                test.calls.append(argv)
                test.command_envs.append(dict(env or default_env))
                if argv[0] == 'git':
                    result = subprocess.run(argv, cwd=cwd, capture_output=True, timeout=5)
                    if result.returncode:
                        raise RuntimeError(result.stderr.decode())
                    return result.stdout if raw else result.stdout.decode()
                if argv[1:] == ['version']:
                    return test.version
                if argv[1:] == ['env', '-json']:
                    host = {**(env or default_env), 'GOENV': test.effective_goenv, 'GOROOT': str(test.base / 'toolchain'),
                            'GOTOOLDIR': str(test.base / 'toolchain/pkg/tool/linux_amd64'),
                            'GOCACHE': str(test.cache), 'GOMODCACHE': str(test.module_cache),
                            'GOHOSTOS': 'linux', 'GOHOSTARCH': 'amd64'}
                    if test.omit_goenv:
                        del host['GOENV']
                    raw = json.dumps(host)
                    test.effective_json.append(raw)
                    return raw
                if argv[1:4] == ['list', '-m', '-json']:
                    return '\n'.join(json.dumps(row) for row in test.module_rows(Path(cwd)))
                if argv[1:5] == ['list', '-deps', '-test', '-json']:
                    return '\n'.join(json.dumps(row) for row in test.package_rows(Path(cwd)))
                if argv[1:3] == ['test', '-c']:
                    if test.mutate:
                        test.mutate(Path(cwd))
                    binary = Path(argv[argv.index('-o') + 1])
                    header = bytearray(64)
                    header[:7] = b'\x7fELF\x02\x01\x01'
                    header[16:20] = b'\x02\x00\x3e\x00'
                    binary.write_bytes(header if test.bad_elf is None else test.bad_elf)
                    binary.chmod(0o700)
                    return ''
                raise AssertionError('unexpected mock tool operation: ' + repr(argv))
        return MockGo()

    def build(self):
        return M.build(self.checkout, self.receipt_path, self.go, self.output,
                       source_compiler=True, runner_factory=self.runner)

    def assert_clean_failure(self):
        self.assertFalse((self.output / 'source').exists())
        self.assertFalse((self.output / 'tmp').exists())
        self.assertFalse((self.output / 'r-native.test').exists())
        self.assertEqual((self.cache / 'keep').read_text(), 'shared cache sentinel')
        self.assertEqual(M.json_object(self.output / 'observations.json')['runtimeAdmission'], False)

    def test_import_and_default_cli_do_not_call_tools_or_create_output(self):
        with patch.object(M, 'helper_module') as helper, patch.object(M.subprocess, 'Popen') as child, contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(M.main(['--checkout', '/missing', '--output', str(self.output)]), 0)
            fresh = importlib.util.module_from_spec(SPEC)
            SPEC.loader.exec_module(fresh)
            child.assert_not_called()
            helper.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_api_requires_explicit_compiler_before_input_access(self):
        with patch.object(M, 'input_paths') as paths:
            with self.assertRaisesRegex(RuntimeError, 'explicit'):
                M.build(None, None, None, None)
            paths.assert_not_called()

    def test_explicit_cli_missing_arguments_has_no_side_effect(self):
        with patch.object(M, 'build') as build, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                M.main(['--source-compiler'])
            build.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_empty_preexisting_missing_parent_and_cache_outputs_rejected(self):
        existing = self.base / 'existing'
        existing.mkdir()
        for value in ('', '.', existing, self.base / 'missing/child', self.checkout / 'new', self.go.parent / 'new'):
            with self.subTest(value=str(value)), self.assertRaises(RuntimeError):
                M.input_paths(self.checkout, self.receipt_path, self.go, value)
        with patch.dict(os.environ, {'GOCACHE': str(self.cache)}), self.assertRaises(RuntimeError):
            M.input_paths(self.checkout, self.receipt_path, self.go, self.cache / 'new')
        self.assertFalse(self.output.exists())

    def test_path_and_source_symlinks_cannot_escape(self):
        link = self.base / 'redirect'
        link.symlink_to(self.checkout, target_is_directory=True)
        with self.assertRaises(RuntimeError):
            M.input_paths(link, self.receipt_path, self.go, self.output)
        (self.checkout / 'escape.go').symlink_to(self.cache / 'keep')
        with self.assertRaises(RuntimeError):
            M.source_inventory(self.checkout, H)

    def test_fake_empty_and_duplicate_receipt_rejected_by_original_contract(self):
        self.loader.stop()
        for raw in ('{}', '{"schema":"fake"}', '{"schema":"a","schema":"b"}'):
            self.receipt_path.write_text(raw)
            with self.subTest(raw=raw), self.assertRaises(RuntimeError):
                self.build()
            self.assertFalse(self.output.exists())
        self.assertEqual(self.calls, [])

    def test_extra_or_partial_overlay_rejected_before_tools(self):
        self.loader.stop()
        fake_receipt = {'dependencies': [{'module': M.TARGET, 'replacement': './polaris-dependencies/sing-tun'}]}
        self.receipt_path.write_text(json.dumps(fake_receipt))
        for suffix in (b'\nextra diff\n', b''):
            overlay = self.base / ('extra.patch' if suffix else 'partial.patch')
            original_patch = M.OVERLAY.read_bytes()
            overlay.write_bytes(original_patch + suffix if suffix else original_patch[:-10])
            with patch.object(H, 'validate_source_receipt'), patch.object(M, 'OVERLAY', overlay), self.assertRaisesRegex(RuntimeError, 'overlay differs'):
                M.load_inputs(self.receipt_path, H, H.provider())
        self.assertEqual(self.calls, [])

    def test_actual_patch_copy_preserves_original_source_and_protected_fixtures(self):
        before = M.source_inventory(self.checkout, H)
        self.assertTrue((self.checkout / M.TEST_FILE).read_bytes().endswith(b'\n'))
        self.assertEqual(before[M.TEST_FILE]['bytes'], 60675)
        self.assertEqual(before[M.TEST_FILE]['sha256'], M.BEFORE_SHA)
        def require_final_lf(root):
            self.assertTrue((root / M.TEST_FILE).read_bytes().endswith(b'\n'))
        self.mutate = require_final_lf
        report = self.build()
        self.assertEqual(M.source_inventory(self.checkout, H), before)
        changed = [name for name in before if report['fixtureInventory'][name] != before[name]]
        self.assertEqual(changed, [M.TEST_FILE])
        self.assertEqual(report['fixtureInventory'][M.TEST_FILE]['sha256'], M.AFTER_SHA)
        self.assertEqual(report['fixtureInventory'][M.TEST_FILE]['bytes'], 61263)
        self.assertTrue(all(row['removedOwnedDirectory'] for row in report['cleanup']))
        self.assertFalse((self.output / 'source').exists())
        self.assertEqual((self.cache / 'keep').read_text(), 'shared cache sentinel')

    def test_source_preimage_drift_rejected_before_output_creation(self):
        target = self.checkout / M.TEST_FILE
        original_bytes = target.read_bytes()
        self.assertTrue(original_bytes.endswith(b'\n'))
        target.write_bytes(original_bytes[:-1])
        with self.assertRaisesRegex(RuntimeError, 'preimage'):
            self.build()
        self.assertFalse(self.output.exists())
        self.assertEqual(self.calls, [])
        target.write_bytes(original_bytes)
        with (self.checkout / M.TEST_FILE).open('ab') as file:
            file.write(b'\nchanged\n')
        with self.assertRaisesRegex(RuntimeError, 'preimage'):
            self.build()
        self.assertFalse(self.output.exists())

    def test_partial_copy_failure_is_registered_and_cleaned(self):
        def partial(source, target, **kwargs):
            (target / 'partial').write_text('copy interrupted')
            raise OSError('injected partial copy')
        with patch.object(M.shutil, 'copytree', side_effect=partial), self.assertRaises(OSError):
            self.build()
        self.assert_clean_failure()

    def test_copy_mutation_is_rejected_without_go(self):
        original_copy = shutil.copytree
        def changed(source, target, *args, **kwargs):
            result = original_copy(source, target, *args, **kwargs)
            if Path(source) == self.checkout:
                (target / 'unexpected.go').write_text('package changed')
            return result
        with patch.object(M.shutil, 'copytree', side_effect=changed), self.assertRaisesRegex(RuntimeError, 'source copy'):
            self.build()
        self.assert_clean_failure()
        self.assertEqual(self.calls, [])

    def test_mock_compiler_failure_and_timeout_leave_no_partial_elf(self):
        for reason in ('compile failure', 'compiler command timed out'):
            with self.subTest(reason=reason):
                self.output = self.base / reason.replace(' ', '-')
                def fail(root):
                    (root.parent / 'r-native.test').write_text('partial compiler output')
                    raise RuntimeError(reason)
                self.mutate = fail
                with self.assertRaisesRegex(RuntimeError, reason):
                    self.build()
                self.assert_clean_failure()

    def test_wrong_elf_cannot_be_an_observed_compiled_fixture(self):
        for data in (b'not ELF', b'\x7fELF' + bytes(60), b'\x7fELF\x02\x01\x01' + bytes(57)):
            with self.subTest(data=data):
                self.output = self.base / ('elf-' + str(len(list(self.base.glob('elf-*')))))
                self.bad_elf = data
                with self.assertRaisesRegex(RuntimeError, 'ELF'):
                    self.build()
                self.assert_clean_failure()

    def test_empty_duplicate_and_truncated_json_stream_rejected(self):
        provider = H.provider()
        for raw in ('', '  ', '[]', '{}', '{"Path":"a","Path":"b"}', '{"Path":'):
            with self.subTest(raw=raw), self.assertRaises((RuntimeError, ValueError)):
                M.stream(raw, provider)

    def test_mvs_cannot_be_declared_only_duplicate_missing_or_escaping(self):
        provider = H.provider()
        good = self.module_rows(self.checkout)
        mutations = [good[1:], good + [good[0]], good[:2], copy.deepcopy(good)]
        mutations[-1][1]['Replace']['Dir'] = str(self.cache)
        for rows in mutations:
            with self.subTest(rows=rows), self.assertRaises(RuntimeError):
                M.module_graph('\n'.join(map(json.dumps, rows)), self.checkout, self.receipt, provider, self.module_cache, H)

    def test_complete_test_graph_preserves_bracket_instances_and_import_map(self):
        host = {'GOROOT': str(self.base / 'toolchain'), 'GOCACHE': str(self.cache), 'GOMODCACHE': str(self.module_cache)}
        rows, hashes = M.package_graph('\n'.join(map(json.dumps, self.package_rows(self.checkout))), self.checkout,
                                      self.receipt, self.module_rows(self.checkout), H.provider(), H, host, self.base)
        self.assertEqual(len(rows), 4)
        self.assertIn(M.TARGET + ' [' + M.TARGET + '.test]', [row['ImportPath'] for row in rows])
        self.assertIn(str(self.cache / '_testmain.go'), hashes)

    def test_truncated_duplicate_unreachable_and_escaped_test_graph_rejected(self):
        good = self.package_rows(self.checkout)
        bad = [good[1:], good + [good[0]], good[:-1]]
        for change in (('Dir', str(self.cache)), ('GoFiles', ['../../cache/keep']), ('Imports', ['missing']),
                       ('GoFiles', ['missing.go']), ('GoFiles', ['a', 'a'])):
            rows = copy.deepcopy(good)
            rows[1][change[0]] = change[1]
            bad.append(rows)
        rows = copy.deepcopy(good)
        rows[0]['ImportPath'] = 'unreachable'
        bad.append(rows)
        rows = copy.deepcopy(good)
        rows[1]['Module'] = self.module_rows(self.checkout)[0]
        bad.append(rows)
        host = {'GOROOT': str(self.base / 'toolchain'), 'GOCACHE': str(self.cache), 'GOMODCACHE': str(self.module_cache)}
        for rows in bad:
            with self.subTest(rows=rows), self.assertRaises(RuntimeError):
                M.package_graph('\n'.join(map(json.dumps, rows)), self.checkout, self.receipt,
                                self.module_rows(self.checkout), H.provider(), H, host, self.base)

    def test_compile_source_drift_keeps_unknown_and_removes_elf(self):
        self.mutate = lambda root: (root / 'production.go').write_text('changed during compile')
        with self.assertRaisesRegex(RuntimeError, 'source changed'):
            self.build()
        self.assert_clean_failure()

    def test_directory_identity_swap_retains_foreign_directory(self):
        original_copy = shutil.copytree
        def swapped(source, target, *args, **kwargs):
            result = original_copy(source, target, *args, **kwargs)
            if Path(source) != self.checkout:
                return result
            target.rename(target.with_name('original-owned-directory'))
            target.mkdir()
            (target / 'foreign').write_text('must retain')
            raise OSError('copy interrupted after replacement')
        with patch.object(M.shutil, 'copytree', side_effect=swapped), self.assertRaisesRegex(RuntimeError, 'identity changed'):
            self.build()
        self.assertEqual((self.output / 'source/foreign').read_text(), 'must retain')
        self.assertEqual((self.cache / 'keep').read_text(), 'shared cache sentinel')

    def test_missing_lists_never_issue_g_ready_plan_or_runtime_admission(self):
        report = self.build()
        self.assertEqual(report['status'], 'CompiledIncomplete_NoRuntime_NoSelectedTopLists')
        self.assertIsNone(report['selectedTopLists'])
        self.assertFalse(report['GPlanIssued'])
        self.assertFalse(report['runtimeAdmission'])
        self.assertNotIn('pcMetadataAck', report)
        self.assertFalse(any('-test.list' in word for call in self.calls for word in call))
        self.assertEqual(report['fixtureTree']['files'], len(report['fixtureInventory']))
        self.assertEqual(report['fixtureTree']['sha256'], H.digest(H.canonical(report['fixtureInventory'])))

    def test_actual_command_timeout_path_kills_and_waits_the_mock_process_group(self):
        self.output.mkdir()
        child = unittest.mock.Mock()
        child.pid = 424242
        child.poll.return_value = None
        child.wait.side_effect = [subprocess.TimeoutExpired('mock-go', 900), -9]
        def signal_group(pgid, sig):
            if sig == 0:
                raise ProcessLookupError()
        with patch.object(M.subprocess, 'Popen', return_value=child) as spawn, patch.object(M.os, 'killpg', side_effect=signal_group) as kill:
            with self.assertRaisesRegex(RuntimeError, 'timed out'):
                M.Commands(self.output, {}).run([self.go, 'version'], cwd=self.checkout)
            self.assertIn(unittest.mock.call(child.pid, M.signal.SIGKILL), kill.call_args_list)
            self.assertEqual(child.wait.call_count, 2)
            self.assertTrue(spawn.call_args.kwargs['start_new_session'])
        rows = json.loads((self.output / 'commands.json').read_text())
        self.assertTrue(rows[0]['timeout'])
        self.assertTrue(rows[0]['groupDrained'])
        self.assertIsNone(rows[0]['exit'])

    def test_parent_exit_with_live_descendants_is_unknown_despite_parent_wait(self):
        self.output.mkdir()
        child = unittest.mock.Mock(pid=424242)
        child.wait.return_value = 0
        child.poll.return_value = 0
        with patch.object(M.subprocess, 'Popen', return_value=child), patch.object(M.os, 'killpg') as kill, \
                patch.object(M.time, 'sleep'), patch.object(M.time, 'monotonic', side_effect=itertools.count(step=0.25)):
            observer = M.Commands(self.output, {})
            with self.assertRaisesRegex(RuntimeError, 'descendants remain'):
                observer.run([self.go, 'version'], cwd=self.checkout)
            self.assertFalse(observer.cleanup_allowed)
            self.assertIn(unittest.mock.call(child.pid, M.signal.SIGKILL), kill.call_args_list)
        row = json.loads((self.output / 'commands.json').read_text())[0]
        self.assertEqual(row['exit'], 0)
        self.assertFalse(row['groupDrained'])
        self.assertTrue(row['groupPresentAfterParentExit'])

    def test_unresolved_compiler_group_retains_source_tmp_and_partial_target(self):
        def factory(output, env):
            runner = self.runner(output, env)
            original_run = runner.run
            def run(args, **kwargs):
                if [str(arg) for arg in args][1:3] == ['test', '-c']:
                    (output / 'r-native.test').write_text('potentially active output')
                    runner.cleanup_allowed = False
                    raise RuntimeError('unresolved own compiler group')
                return original_run(args, **kwargs)
            runner.run = run
            return runner
        with self.assertRaisesRegex(RuntimeError, 'compiler group unresolved'):
            M.build(self.checkout, self.receipt_path, self.go, self.output, source_compiler=True, runner_factory=factory)
        self.assertTrue((self.output / 'source').is_dir())
        self.assertTrue((self.output / 'tmp').is_dir())
        self.assertEqual((self.output / 'r-native.test').read_text(), 'potentially active output')
        self.assertEqual((self.cache / 'keep').read_text(), 'shared cache sentinel')
        report = M.json_object(self.output / 'observations.json')
        self.assertFalse(report['runtimeAdmission'])
        self.assertTrue(all(not row['removedOwnedDirectory'] for row in report['cleanup']))

    def test_disabled_goenv_keeps_command_off_and_exact_observed_empty_output(self):
        report = self.build()
        self.assertTrue(self.command_envs)
        self.assertTrue(all(env['GOENV'] == 'off' for env in self.command_envs))
        self.assertEqual(report['toolchain']['env']['GOENV'], '')
        self.assertEqual(json.loads(self.effective_json[-1]), report['toolchain']['env'])
        self.assertEqual(report['status'], 'CompiledIncomplete_NoRuntime_NoSelectedTopLists')

    def test_default_goenv_file_missing_and_pseudo_off_outputs_rejected(self):
        for index, value in enumerate(('/home/mock/.config/go/env', 'off', None, False, ' ')):
            with self.subTest(value=value):
                self.output = self.base / ('goenv-reject-' + str(index))
                self.effective_goenv, self.omit_goenv = value, value is None
                with self.assertRaisesRegex(RuntimeError, 'GOENV must be disabled'):
                    self.build()
                self.assert_clean_failure()
                self.assertTrue(all(env['GOENV'] == 'off' for env in self.command_envs))
                self.assertFalse(any(call[1:2] == ['list'] for call in self.calls))


if __name__ == '__main__':
    unittest.main()
