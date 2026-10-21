"""Producer call controls with inert output bytes; no Go/SDK/compiler executes."""
from contextlib import contextmanager, ExitStack
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch
import ast
import inspect
import re
import json
import os
import sys

import producer
from artifacts import tree_inventory
from budget import native_environment as apply_native_environment
from fork_input import load


class ProducerCalls(unittest.TestCase):
    def test_run_capture_default_and_invalid_modes_do_not_start_or_open_logs(self):
        self.assertEqual(inspect.signature(producer.run).parameters['capture'].default, 'full')
        for mode in ('tail', '', None, False, [], {}):
            with self.subTest(mode=mode), patch.object(producer.subprocess, 'Popen') as spawn, \
                    patch.object(Path, 'open') as opened, self.assertRaisesRegex(RuntimeError, 'unknown tool capture mode'):
                producer.run(['never-executed'], log='never-opened', capture=mode)
            spawn.assert_not_called(); opened.assert_not_called()
        with patch.object(producer.subprocess, 'Popen') as spawn, patch.object(Path, 'open') as opened, \
                self.assertRaisesRegex(RuntimeError, 'requires complete original logs'):
            producer.run(['never-executed'], capture='none')
        spawn.assert_not_called(); opened.assert_not_called()

    def test_only_seventeen_logged_nonconsumers_disable_full_capture(self):
        directory = Path(producer.__file__).parent
        expected = {'producer.py': 3, 'ci_apple.py': 5, 'build_apple_app.py': 8, 'apple_rust.py': 1}
        for filename, count in expected.items():
            tree = ast.parse((directory / filename).read_text())
            selected, symbols = [], []
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Name):
                    continue
                direct = node.func.id == 'run'
                indirect = (node.func.id == 'timed_phase' and len(node.args) > 1 and
                            isinstance(node.args[1], ast.Name) and node.args[1].id == 'run')
                if not (direct or indirect):
                    continue
                keywords = {keyword.arg: keyword.value for keyword in node.keywords}
                capture = keywords.get('capture')
                if 'log' not in keywords:
                    self.assertIsNone(capture, (filename, node.lineno))
                    continue
                if (filename == 'apple_rust.py' and direct and node.args and
                        isinstance(node.args[0], ast.List) and any(
                            isinstance(arg, ast.Constant) and arg.value == 'nm' for arg in node.args[0].elts)):
                    self.assertIsNone(capture, 'nm must retain every symbol')
                    symbols.append(node)
                    continue
                self.assertIsInstance(capture, ast.Constant, (filename, node.lineno))
                self.assertEqual(capture.value, 'none', (filename, node.lineno))
                selected.append(node)
            self.assertEqual(len(selected), count, filename)
            self.assertEqual(len(symbols), 1 if filename == 'apple_rust.py' else 0)
        # These protocol parsers remain on the same full-return contract.
        for filename in ('artifacts.py', 'apple_admission.py', 'ci_android.py', 'consume.py', 'carriers.py'):
            tree = ast.parse((directory / filename).read_text())
            for node in ast.walk(tree):
                if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == 'run':
                    self.assertFalse(any(keyword.arg == 'capture' for keyword in node.keywords),
                                     (filename, node.lineno))

    def test_apple_wrapper_forwards_capture_without_changing_full_results(self):
        import build_apple_app
        for kwargs, result in (({}, 'complete stdout'), ({'capture': 'full'}, 'complete stdout'),
                               ({'capture': 'none', 'log': 'original.log'}, None)):
            with self.subTest(kwargs=kwargs), patch.object(build_apple_app, 'native_run', return_value=result) as native:
                self.assertEqual(build_apple_app.run(['inert'], **kwargs), result)
            self.assertEqual(native.call_args.args, (['inert'],))
            self.assertEqual({key: value for key, value in native.call_args.kwargs.items()
                              if not key.startswith('observation_')}, kwargs)

    def test_bounded_download_pipe_and_structure_failures_never_export_raw_errors(self):
        secret = 'DO_NOT_EXPORT_MODULE_CREDENTIAL'
        good = {'Path':'example.invalid/pkg','Version':'v1.0.0','Sum':'h1:fixed','Dir':'/fixed/cache'}
        cases = [
            ('import os; os.write(1, (b"x"*4000+b"\\n")*1024)', 'stdout-budget'),
            ('import os; os.write(2, b"' + secret + '"*1024)', 'stderr-budget'),
            ('import os; os.write(1, b"x"*20000)', 'line-budget'),
            ('import os; os.write(1, b"\\n"*40000)', 'line-budget'),
            ('import json; print(json.dumps({"Path":"x"*5000}))', 'string-budget'),
            ('print(\'{"Origin":\'+"["*10)', 'structure-or-item-budget'),
            ('import os; os.write(1,b\'{"Origin":[\'+b"0,0,0,\\n"*25000+b"0]}\")', 'structure-budget'),
            ('print(' + repr(json.dumps(good) + '\n' + json.dumps(good)) + ')', 'structure-or-item-budget'),
            ('print(\'{"Path":"one","Path":"two"}\')', 'download-collection-or-json-error'),
            ('print(\'{"Path":NaN}\')', 'download-collection-or-json-error'),
            ('print(' + repr(json.dumps({'Error':'https://user:' + secret + '@example.invalid/private'})) + ')', 'download-json-error'),
            ('print(\'{"unknown":"value"}\')', 'download-json-structure'),
            ('print(' + repr(json.dumps(dict(good, Query=[]))) + ')', 'download-json-structure'),
            ('print(' + repr(json.dumps(dict(good, Origin={'TagPrefix': 7}))) + ')', 'download-json-structure'),
            ('import sys; sys.stderr.write("' + secret + '"); sys.exit(1)', 'download-exit-or-live-group'),
            ('import time; time.sleep(5)', 'deadline'),
        ]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for index, (code, reason) in enumerate(cases):
                observation = root / (str(index) + '.json')
                with self.subTest(reason=reason, index=index), \
                        patch.object(producer, 'DOWNLOAD_SECONDS', 0.1 if reason == 'deadline' else 5), \
                        patch('builtins.print'), self.assertRaisesRegex(RuntimeError, 'module download rejected: ' + reason):
                    producer.bounded_module_download([sys.executable,'-c',code], cwd=root, env=None,
                                                    expected_count=1, observation=observation)
                observed = json.loads(observation.read_bytes())
                self.assertFalse(observed['complete'] or observed['rawOutputRetained'] or observed['inputValuesExported'])
                self.assertTrue(observed['cleanupComplete'])
                self.assertEqual(observed['reason'], reason)
                self.assertLessEqual(observed['stdoutBytesObserved'], producer.DOWNLOAD_STDOUT_BYTES + 1)
                self.assertLessEqual(observed['stderrBytesObserved'], producer.DOWNLOAD_STDERR_BYTES + 1)
                self.assertNotIn(secret, observation.read_text())
                self.assertNotIn('https://', observation.read_text())
            self.assertEqual(list(root.glob('*.log')), [])

    def test_bounded_download_valid_records_and_spawn_failure_are_safe(self):
        row = {'Path':'example.invalid/pkg','Version':'v1.0.0','Query':'v1.0.0','Sum':'h1:fixed','Dir':'/fixed/cache',
               'Origin': {'VCS':'git', 'URL':'https://example.invalid/pkg', 'Hash':'fixed',
                          'TagPrefix':'sub/', 'TagSum':'fixed', 'Ref':'refs/tags/v1.0.0'}}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); observation = root / 'valid.json'
            with patch('builtins.print'):
                rows = producer.bounded_module_download([sys.executable,'-c','print(' + repr(json.dumps(row)) + ')'],
                    cwd=root, env=None, expected_count=1, observation=observation)
            self.assertEqual(rows, [row])
            value = json.loads(observation.read_bytes()); self.assertTrue(value['complete'] and value['cleanupComplete'])
            self.assertEqual(value['itemsDecoded'], 1)
            with patch.object(producer.subprocess, 'Popen', side_effect=OSError('DO_NOT_EXPORT')), \
                    patch('builtins.print'), self.assertRaisesRegex(RuntimeError, 'download-collection-or-json-error'):
                producer.bounded_module_download(['not-executed'], cwd=root, env=None, expected_count=1,
                                                observation=root/'failed.json')
            self.assertNotIn('DO_NOT_EXPORT', (root/'failed.json').read_text())

    def test_bounded_download_unfinished_group_retains_ledger_ownership(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            # The direct child exits. A simulated live descendant is never
            # removed from the reservation merely because the parent returned.
            with patch.object(producer, 'group_alive', return_value=True), \
                    patch.object(producer.os, 'killpg'), patch.object(producer, 'DOWNLOAD_CLEANUP_SECONDS', 0.05), \
                    patch.object(producer, 'register_group') as register, patch('builtins.print'), \
                    self.assertRaisesRegex(RuntimeError, 'download-cleanup-incomplete'):
                producer.bounded_module_download([sys.executable,'-c','pass'], cwd=root, env=None,
                                                expected_count=1, observation=root/'group.json')
            self.assertFalse(json.loads((root/'group.json').read_bytes())['cleanupComplete'])
            self.assertTrue(register.called)
            self.assertFalse(any(call.kwargs.get('finished') for call in register.call_args_list))

    def test_module_admission_uses_only_source_sums_and_alternate_files_then_preserves_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); source = root / 'source'; source.mkdir(); work = root / 'work'; work.mkdir()
            originals = {'go.mod': b'module example.invalid/main\ngo 1.25.5\n',
                         'go.sum': b'example.invalid/known v1.0.0 h1:fixed\nexample.invalid/known v1.0.0/go.mod h1:mod\n'}
            for name, raw in originals.items(): (source / name).write_bytes(raw)
            graph = [{'Path': 'example.invalid/main', 'Main': True, 'Dir': str(source)},
                     {'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:fixed'},
                     {'Path': 'example.invalid/unknown', 'Version': 'v2.0.0'},
                     {'Path': 'example.invalid/local', 'Version': 'v1.0.0', 'Replace': {'Path': './local', 'Dir': str(source/'local')}}]
            calls = []
            def run(command, **kwargs):
                calls.append((command, kwargs))
                self.assertEqual(command[:3], ['inert-go', 'mod', 'download'])
                self.assertTrue(command[3].startswith('-modfile='))
                alternate = Path(command[3].removeprefix('-modfile='))
                self.assertEqual(alternate.read_bytes(), originals['go.mod'])
                self.assertEqual(alternate.with_suffix('.sum').read_bytes(), originals['go.sum'])
                alternate.with_suffix('.sum').write_bytes(b'owned download sum update')
                self.assertEqual(kwargs['expected_count'], 1)
                return [{'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:fixed',
                         'Dir': str(root/'cache/go-mod/known')}]
            with patch.object(producer, 'bounded_module_download', side_effect=run):
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][0], ['inert-go', 'mod', 'download',
                '-modfile=' + str(work/'module-cache-admission/admission.mod'), '-json',
                'example.invalid/known@v1.0.0'])
            self.assertEqual(calls[0][1]['cwd'], source)
            self.assertEqual({name:(source/name).read_bytes() for name in originals}, originals)
            graph[1]['Dir'] = '/actual/warm/cache/known'
            with patch.object(producer, 'bounded_module_download') as run:
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            run.assert_not_called()

    def test_changed_download_checksum_or_retained_admission_view_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); source = root / 'source'; source.mkdir(); work = root / 'work'; work.mkdir()
            (source/'go.mod').write_bytes(b'module example.invalid/main\n')
            (source/'go.sum').write_bytes(b'example.invalid/known v1.0.0 h1:fixed\n')
            graph = [{'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:fixed'}]
            changed = {'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:changed', 'Dir': '/fake/cache'}
            with patch.object(producer, 'bounded_module_download', return_value=[changed]), \
                    self.assertRaisesRegex(RuntimeError, 'downloaded module identity differs'):
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            with patch.object(producer, 'bounded_module_download') as run, self.assertRaisesRegex(RuntimeError, 'prior module admission inputs retained'):
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            run.assert_not_called()
            self.assertEqual((source/'go.sum').read_bytes(), b'example.invalid/known v1.0.0 h1:fixed\n')

    def test_cache_delta_is_safe_metadata_and_never_relaxes_actual_input_identity(self):
        original = {'schema': 'input', 'producer': {'candidate': 'a'*40, 'files': {'source.py': 'fixed'}},
                    'moduleGraph': [{'Path': 'example.invalid/known', 'Version': 'v1', 'Sum': 'h1:fixed'}],
                    'tools': {'private': 'DO_NOT_EXPORT'}, 'sourceOverlay': 0}
        current = json.loads(json.dumps(original)); current['producer']['candidate'] = 'b'*40
        self.assertTrue(producer.equivalent_binding(original, current))
        current['moduleGraph'][0]['Dir'] = '/private/DO_NOT_EXPORT'
        self.assertFalse(producer.equivalent_binding(original, current))
        observation = producer.cache_binding_observation({'input':original,'fingerprint':'c'*64}, current, 'd'*64)
        self.assertEqual(observation['changedFields'], ['moduleGraph'])
        self.assertEqual(observation['originalModuleGraph']['directories'], 0)
        self.assertEqual(observation['currentModuleGraph']['directories'], 1)
        self.assertNotIn('DO_NOT_EXPORT', json.dumps(observation))
        self.assertFalse(observation['inputValuesExported'] or observation['fileContentsExported'])
        for field in ['tools', 'sourceOverlay']:
            altered = json.loads(json.dumps(original)); altered[field] = 'different'
            self.assertFalse(producer.equivalent_binding(original, altered))
        self.assertEqual(original['producer']['candidate'], 'a'*40)

    @staticmethod
    def android_projection_binding():
        return {'schema': 'polaris-mobile-producer-input-v1', 'platform': 'android',
                'producer': {'candidate': 'a'*40, 'files': {
                    name: {'bytes': index + 1, 'sha256': producer.sha(name.encode())}
                    for index, name in enumerate(producer.PRODUCER_FILES)}},
                'sourceContract': {'sourceCommit': 'f'*40, 'android': {'minSDK': 24}},
                'sourceObservation': {'sourceTree': 'e'*40},
                'moduleGraph': [{'Path': 'example.invalid/core', 'Version': 'v1', 'Sum': 'h1:fixed',
                                 'Dir': '/original/native/module'}],
                'tools': {'compiler': {'sha256': 'd'*64}}, 'sourceOverlay': 0}

    def test_android_projection_reuses_only_declared_apple_only_changes(self):
        original = self.android_projection_binding()
        frozen = json.dumps(original, sort_keys=True)
        current = json.loads(frozen); current['producer']['candidate'] = 'b'*40
        for name in producer.ANDROID_UNRELATED_CODE:
            current['producer']['files'][name] = {'bytes': 99, 'sha256': 'c'*64}
        self.assertEqual(producer.android_build_code(original['producer']),
                         producer.android_build_code(current['producer']))
        self.assertTrue(producer.equivalent_binding(original, current))
        self.assertEqual(json.dumps(original, sort_keys=True), frozen)
        apple_original = {**original, 'platform': 'apple'}
        apple_current = {**current, 'platform': 'apple'}
        self.assertFalse(producer.equivalent_binding(apple_original, apple_current))
        for name in set(producer.PRODUCER_FILES) - producer.ANDROID_UNRELATED_CODE:
            altered = json.loads(frozen)
            altered['producer']['files'][name]['sha256'] = 'c'*64
            with self.subTest(relevant=name):
                self.assertNotEqual(producer.android_build_code(original['producer']),
                                    producer.android_build_code(altered['producer']))
                self.assertFalse(producer.equivalent_binding(original, altered))
        for field in ['sourceContract', 'sourceObservation', 'moduleGraph', 'tools', 'sourceOverlay']:
            altered = json.loads(frozen); altered[field] = 'changed'
            with self.subTest(input=field):
                self.assertFalse(producer.equivalent_binding(original, altered))
        extra = json.loads(frozen)
        extra['producer']['files']['scripts/new-component-input.py'] = {'bytes': 1, 'sha256': 'c'*64}
        self.assertFalse(producer.equivalent_binding(original, extra))

    def test_stable_ndk_profile_and_installer_are_full_receipt_and_cache_inputs(self):
        original = self.android_projection_binding()
        for name in ('scripts/mobile-core/ndk-r30-linux.json', 'scripts/libbox-patches/prepare-ndk-linux.py'):
            self.assertIn(name, producer.PRODUCER_FILES)
            self.assertNotIn(name, producer.ANDROID_UNRELATED_CODE)
            changed = json.loads(json.dumps(original))
            changed['producer']['files'][name]['sha256'] = '0' * 64
            with self.subTest(name=name):
                self.assertFalse(producer.equivalent_binding(original, changed))
                self.assertNotEqual(producer.receipt_digest(original), producer.receipt_digest(changed))
                missing = json.loads(json.dumps(original)); del missing['producer']['files'][name]
                with self.assertRaisesRegex(RuntimeError, 'complete original producer code inventory'):
                    producer.android_build_code(missing['producer'])

    def test_apple_target_contract_changes_binding_and_never_relabels_legacy_receipt(self):
        current = self.android_projection_binding()
        current.update(platform='apple', sourceContract=load())
        original = json.loads(json.dumps(current))
        original['sourceContract']['apple']['targets'].append('iossimulator/amd64')
        original['sourceContract']['apple']['slices'].append('ios-x86_64-simulator')
        receipt = {'input': original, 'output': {'slices': original['sourceContract']['apple']['slices']},
                   'fingerprint': 'c' * 64}
        frozen = json.dumps(receipt, sort_keys=True)
        self.assertFalse(producer.equivalent_binding(original, current))
        self.assertNotEqual(producer.receipt_digest(original), producer.receipt_digest(current))
        observation = producer.component_revalidation_observation(
            receipt, current, {'bytes': len(frozen), 'sha256': producer.sha(frozen.encode())}, False)
        self.assertFalse(observation['reused'])
        self.assertEqual(observation['originalBuildID'], 'polaris-mobile-' + producer.receipt_digest(original))
        self.assertEqual(json.dumps(receipt, sort_keys=True), frozen)
        self.assertEqual(producer.OBSERVATION_SLOTS, frozenset(('device-arm64', 'sim-arm64')))

    def test_same_version_new_source_policy_or_tool_bytes_cannot_reuse_old_component(self):
        from test_source_policy import future_mobile_fixture
        value, _ = future_mobile_fixture()
        original = self.android_projection_binding(); original['sourceContract'] = value
        frozen = json.dumps(original, sort_keys=True)
        for field in ('sourceCommit', 'sourceTree', 'metacubex-binding', 'go-tool-bytes'):
            current = json.loads(frozen)
            if field in ('sourceCommit', 'sourceTree'):
                current['sourceContract'][field] = '0' * 40
            elif field == 'metacubex-binding':
                current['sourceContract']['sourcePolicy']['metacubex']['bindingSha256'] = '0' * 64
            else:
                current['tools']['compiler']['sha256'] = '0' * 64
            self.assertEqual(current['sourceContract']['version'], original['sourceContract']['version'])
            with self.subTest(field=field): self.assertFalse(producer.equivalent_binding(original, current))
        self.assertEqual(json.dumps(original, sort_keys=True), frozen)

    def test_android_projection_requires_complete_pins_and_known_policy(self):
        binding = self.android_projection_binding()
        with self.assertRaisesRegex(RuntimeError, 'unknown Android build code policy'):
            producer.android_build_code(binding['producer'], 'unreviewed-policy')
        incomplete = json.loads(json.dumps(binding['producer']))
        del incomplete['files']['scripts/apple-carrier.py']
        with self.assertRaisesRegex(RuntimeError, 'complete original producer code inventory'):
            producer.android_build_code(incomplete)
        for pin in [{'bytes': True, 'sha256': 'a'*64}, {'bytes': 1, 'sha256': 'not-a-hash'},
                    {'bytes': 1, 'sha256': 'a'*64, 'unbound': True}]:
            broken = json.loads(json.dumps(binding['producer']))
            broken['files']['scripts/apple-carrier.py'] = pin
            with self.assertRaisesRegex(RuntimeError, 'producer code file pin differs'):
                producer.android_build_code(broken)

    def test_revalidation_observes_current_verifier_without_relabeling_original(self):
        original = self.android_projection_binding()
        receipt = {'input': original, 'fingerprint': 'c'*64}
        frozen = json.dumps(receipt, sort_keys=True)
        current = json.loads(json.dumps(original)); current['producer']['candidate'] = 'b'*40
        current['producer']['files']['scripts/apple-carrier.py'] = {'bytes': 99, 'sha256': 'd'*64}
        pin = {'bytes': len(frozen), 'sha256': producer.sha(frozen.encode())}
        observed = producer.component_revalidation_observation(receipt, current, pin, True)
        self.assertEqual(observed['comparisonPolicy'], producer.ANDROID_BUILD_CODE_POLICY)
        self.assertEqual(observed['currentVerifier'], current['producer'])
        self.assertEqual(len(observed['currentVerifier']['files']), len(producer.PRODUCER_FILES))
        self.assertEqual(set(observed['currentVerifier']['files']), set(producer.PRODUCER_FILES))
        for helper in ('scripts/mobile-core/apple_admission.py', 'scripts/mobile-core/apple_handoff.py',
                       'scripts/mobile-core/apple-cache.py', 'scripts/mobile-core/apple_tls_fingerprint.py',
                       'scripts/mobile-core/source_policy.py'):
            self.assertEqual(observed['currentVerifier']['files'][helper], current['producer']['files'][helper])
        # The added inventory member is an actual pinned link helper, not a
        # dummy entry introduced merely to match the new file count.
        helper = 'scripts/mobile-core/apple_tls_fingerprint.py'
        actual_pin = producer.file_pin(producer.ROOT / helper)
        with patch.object(producer, 'run', side_effect=['', 'b'*40]):
            actual_producer = producer.producer_code()
        self.assertEqual(actual_producer['files'][helper], actual_pin)
        import consume
        with patch.object(consume, 'source_status_observation',
                          return_value={'complete': True, 'clean': True, 'paths': [], 'reason': None}), \
                patch.object(consume, 'run', return_value='b'*40):
            actual_consumer = consume.app_identity()
        self.assertEqual(actual_consumer['consumerCode'][helper], actual_pin)
        helper = 'scripts/mobile-core/source_policy.py'
        self.assertEqual(actual_producer['files'][helper], producer.file_pin(producer.ROOT / helper))
        self.assertEqual(actual_consumer['consumerCode'][helper], producer.file_pin(producer.ROOT / helper))
        link_helper = 'scripts/mobile-core/apple_tls_fingerprint.py'
        for project in ('src-tauri/gen/apple/project.yml', 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj'):
            source = (producer.ROOT / project).read_text()
            for action in ('install', 'verify-installed'):
                self.assertRegex(source, re.escape(link_helper) + r'\\?"\s+' + action)

        self.assertEqual(observed['originalProducerCandidate'], 'a'*40)
        self.assertEqual(observed['originalReceipt'], pin)
        self.assertEqual(observed['originalComponentFingerprint'], 'c'*64)
        self.assertEqual(observed['originalBuildID'], 'polaris-mobile-' + producer.receipt_digest(original))
        self.assertEqual(observed['fingerprint'], producer.receipt_digest({
            key: value for key, value in observed.items() if key != 'fingerprint'}))
        self.assertEqual(json.dumps(receipt, sort_keys=True), frozen)

    def test_actual_apple_bind_and_low_memory_cache_keep_original_module_and_source(self):
        value = load()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            supplied = root / 'source'; supplied.mkdir()
            developer = root / 'developer'; developer.mkdir()
            tool = root / 'inert-tool'; tool.write_bytes(b'not executed')
            bindings, capacities, bind_caps = [], [], []
            def native_environment(env, observation=None):
                cap = 1 + len(capacities) % 2
                capacities.append(cap)
                return (dict(env, GOMAXPROCS=str(cap), CARGO_BUILD_JOBS=str(cap)),
                        {'jobCap': cap, 'fixture': True})
            @contextmanager
            def lease(work, cache, _value):
                work.mkdir(mode=0o700, exist_ok=True); cache.mkdir(mode=0o700, exist_ok=True)
                yield work, cache
            def run(command, *, cwd=None, env=None, log=None, capture='full'):
                command = [str(item) for item in command]
                self.assertEqual(capture, 'none' if log is not None else 'full')
                if command[:2] == ['git', 'clone']:
                    Path(command[-1]).mkdir()
                elif len(command) > 1 and command[1] == 'bind':
                    out = Path(command[command.index('-o') + 1])
                    # Pinned gomobile's framework name comes from output basename.
                    module = out.stem
                    self.assertEqual(module, 'Libbox')
                    self.assertEqual(command[command.index('-target') + 1], 'ios/arm64,iossimulator/arm64')
                    out.mkdir(); (out / (module + '.h')).write_bytes(b'inert generated API')
                    generated = Path(cwd) / 'build/ios'; generated.mkdir(parents=True)
                    (generated / 'generated.m').write_bytes(b'inert generated Objective-C')
                    bindings.append((Path(cwd), out))
                    bind_caps.append((env['GOMAXPROCS'], env['CARGO_BUILD_JOBS']))
                    Path(log).write_text('inert original bind log')
                elif command[1:3] == ['env', '-json']:
                    return json.dumps({key: '' for key in command[3:]})
                elif command[1:4] == ['list', '-m', '-json']:
                    return '{"Path":"github.com/sagernet/sing-box","Main":true}'
                return None if capture == 'none' else ''
            with ExitStack() as stack:
                stack.enter_context(patch.object(producer.os, 'uname', return_value=SimpleNamespace(sysname='Darwin')))
                for name, replacement in {
                    'lease': lease, 'producer_code': lambda: {'candidate': 'a'*40, 'files': {}},
                    'verify_source': lambda *a, **k: {'sourceOverlay': 0}, 'run': run,
                    'native_capacity': lambda: {'jobCap': 1, 'estimatedMemoryJobCap': 0,
                                                'memoryEstimateRisk': 'below-unmeasured-single-job-estimate',
                                                'availableMemoryBytes': 1024**3,
                                                'scope': 'controlled low-memory observation'},
                    'native_environment': native_environment,
                    'tool_facts': lambda *a: ({'go': str(tool), 'gomobile': str(tool), 'gobind': str(tool)}, {}),
                    'inspect_framework': lambda path, *a: {'inventory': tree_inventory(path), 'slices': {}},
                    'verify_receipt': lambda *a: None,
                }.items(): stack.enter_context(patch.object(producer, name, replacement))
                stack.enter_context(patch('native_inputs.collect', return_value={'files': {}}))
                stack.enter_context(patch('native_inputs.verify'))
                printed = stack.enter_context(patch('builtins.print'))
                stack.enter_context(patch.object(producer.os, 'statvfs', return_value=SimpleNamespace(f_bavail=10**12, f_frsize=1)))
                for slot in ['one', 'two']:
                    args = SimpleNamespace(platform='apple', source=supplied, developer=developer,
                        go=tool, gomobile=tool, gobind=tool, work=root/slot, cache=root/(slot+'-cache'))
                    producer.execute(args)
                    receipt = producer.read_receipt(args.work / 'component-receipt.json')
                    self.assertFalse(receipt['AppAccepted'])
                    self.assertEqual(receipt['command'][receipt['command'].index('-o')+1], str(bindings[-1][1]))
                    self.assertEqual(receipt['nativeInputs']['generatorOutputRoot'], str(args.work/'build-view/build'))
                    saved = (args.work/'component-receipt.json').read_bytes()
                    generated = args.work/'build-view/build/ios/generated.m'
                    original_generated = generated.read_bytes()
                    count = len(bindings)
                    # A matching cache keeps its original receipt and generated
                    # inputs; it does not rebind, relabel or delete them.
                    with patch.object(producer, 'native_environment', side_effect=RuntimeError(
                            'controlled compiler-only path must not run on a matching cache')) as admission:
                        producer.execute(args)
                        admission.assert_not_called()
                    self.assertEqual(len(bindings), count)
                    self.assertEqual((args.work/'component-receipt.json').read_bytes(), saved)
                    self.assertEqual(generated.read_bytes(), original_generated)
                    self.assertNotIn('capacity', receipt['input'])
                    with patch.object(producer, 'tool_facts', return_value=(
                            {'go':str(tool),'gomobile':str(tool),'gobind':str(tool)}, {'compiler':'changed'})), \
                            self.assertRaisesRegex(RuntimeError, 'original generator outputs retained'):
                        producer.execute(args)
                    self.assertEqual(len(bindings), count)
                    self.assertEqual((args.work/'component-receipt.json').read_bytes(), saved)
                    self.assertEqual(generated.read_bytes(), original_generated)
                    delta = producer.read_receipt(args.work/'cache-binding-observation.json')
                    self.assertEqual(delta['changedFields'], ['tools'])
                # A fresh bind uses one child task despite the unmeasured
                # estimate of zero. Native outputs here are entirely inert.
                count = len(bindings)
                args = SimpleNamespace(platform='apple', source=supplied, developer=developer,
                    go=tool, gomobile=tool, gobind=tool, work=root/'low-memory', cache=root/'low-cache')
                with patch.object(producer, 'native_environment', side_effect=apply_native_environment) as applied:
                    producer.execute(args)
                self.assertEqual(applied.call_count, 1)
                self.assertEqual(len(bindings), count + 1)
                self.assertEqual(bind_caps[-1], ('1', '1'))
                self.assertTrue((args.work/'component-receipt.json').is_file())
                self.assertNotIn('capacity', producer.read_receipt(args.work/'component-receipt.json')['input'])
                # Invalid explicit limits still fail before compilation. The
                # flushed sample survives without exporting the child env.
                original_receipts = {(root/slot/'component-receipt.json'):
                    (root/slot/'component-receipt.json').read_bytes() for slot in ('one', 'two', 'low-memory')}
                for key in ('GOMAXPROCS', 'CARGO_BUILD_JOBS'):
                    for bad in ('0', '-1', 'auto', ''):
                        slot = 'invalid-' + key + '-' + str(len(list(root.iterdir())))
                        invalid = SimpleNamespace(platform='apple', source=supplied, developer=developer,
                            go=tool, gomobile=tool, gobind=tool, work=root/slot, cache=root/(slot+'-cache'))
                        def invalid_environment(env, observation=None):
                            return apply_native_environment(dict(env, **{key: bad}), observation)
                        printed.reset_mock(); count = len(bindings)
                        with patch.object(producer, 'native_environment', side_effect=invalid_environment), \
                                self.assertRaisesRegex(RuntimeError, 'explicit native compiler concurrency must be a positive integer'):
                            producer.execute(invalid)
                        samples = [call for call in printed.call_args_list if call.args and
                            'component-native-capacity-sample' in str(call.args[0])]
                        self.assertEqual(len(samples), 1)
                        self.assertTrue(samples[0].kwargs['flush'])
                        sample = json.loads(samples[0].args[0])['capacity']
                        self.assertEqual(sample['jobCap'], 1)
                        self.assertEqual(sample['estimatedMemoryJobCap'], 0)
                        self.assertNotIn(key, sample)
                        self.assertEqual(len(bindings), count)
                        for name in ('component-receipt.json', 'stage-component-receipt.json', 'stage-original-bind.log'):
                            self.assertFalse((invalid.work/name).exists())
                        for path, saved in original_receipts.items():
                            self.assertEqual(path.read_bytes(), saved)
            self.assertEqual(list(supplied.iterdir()), [])
            self.assertNotEqual(bindings[0][0], bindings[1][0])
            self.assertTrue(all(cwd.name == 'build-view' for cwd, _ in bindings))
            self.assertGreater(len(set(capacities)), 1)
            self.assertTrue(all(go == cargo and go in ('1', '2') for go, cargo in bind_caps))

    def test_generator_buildinfo_requires_actual_command_compiler_and_unreplaced_module(self):
        from test_source_policy import future_mobile_fixture
        value, _ = future_mobile_fixture()
        for name in ('gomobile', 'gobind'):
            raw = ('/tools/' + name + ': go1.27.2\n\tpath\tgithub.com/sagernet/gomobile/cmd/' + name +
                   '\n\tmod\tgithub.com/sagernet/gomobile\tv0.1.13\th1:' + 'A' * 43 + '=\n')
            producer.generator_buildinfo(raw, name, value)
            for changed in (raw.replace('go1.27.2', 'go1.25.5'), raw.replace('/cmd/' + name, '/cmd/other'),
                            raw.replace('v0.1.13', 'v0.1.12'), raw.replace('h1:', 'missing:'),
                            raw + '\t=>\t./unreviewed\t(devel)\t\n',
                            raw + '\tpath\tgithub.com/sagernet/gomobile/cmd/' + name + '\n',
                            raw.replace('\tmod\t', '\tdep\t')):
                with self.subTest(name=name, changed=changed), self.assertRaises(RuntimeError):
                    producer.generator_buildinfo(changed, name, value)

    def test_tool_receipt_binds_actual_javac_resolution_version_and_binary(self):
        value = load()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            jdk = root/'jdk'; ndk = root/'ndk'
            paths = [root/name for name in ['go', 'gomobile', 'gobind']]
            paths += [jdk/'bin'/name for name in ['java', 'javac', 'javap']]
            for host in ['linux-x86_64', 'darwin-x86_64']:
                paths += [ndk/'toolchains/llvm/prebuilt'/host/'bin'/name for name in
                          ['clang', 'clang++', 'ld.lld', 'llvm-ar', 'llvm-readelf']]
            paths += [ndk/'source.properties']
            for path in paths:
                path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'inert tool '+path.name.encode())
            args = SimpleNamespace(platform='android', go=root/'go', gomobile=root/'gomobile', gobind=root/'gobind', jdk=jdk, ndk=ndk)
            selected, version = [str(jdk/'bin/javac')], ['javac 17.0.13']
            def run(command, **_kwargs):
                parts = [str(p) for p in command]
                if parts[0] == 'which': return selected[0]
                if parts[0].endswith('/java'): return 'openjdk 17.0.13 fixture'
                if parts[0].endswith('/javac'): return version[0]
                if parts[1:] == ['version']: return 'go version go'+value['goVersion']+' linux/amd64'
                if parts[1:3] == ['version', '-m']:
                    name = Path(parts[-1]).name
                    return (parts[-1] + ': go' + value['goVersion'] + '\n\tpath\tgithub.com/sagernet/gomobile/cmd/' + name +
                            '\n\tmod\tgithub.com/sagernet/gomobile\t' + value[name + 'Version'] + '\th1:' + 'A' * 43 + '=\n')
                return 'inert compiler version'
            with patch.object(producer, 'run', run):
                _, facts = producer.tool_facts(args, value, {})
                self.assertEqual(facts['javac']['path'], str(jdk/'bin/javac'))
                self.assertEqual(facts['javac']['version'], 'javac 17.0.13')
                self.assertEqual(facts['javac']['sha256'], producer.sha((jdk/'bin/javac').read_bytes()))
                version[0] = 'javac 21.0.1'
                with self.assertRaisesRegex(RuntimeError, 'actual javac must be Java17'):
                    producer.tool_facts(args, value, {})
                selected[0] = str(root/'go')
                with self.assertRaisesRegex(RuntimeError, 'resolution differs'):
                    producer.tool_facts(args, value, {})

    def test_tool_receipt_refuses_changed_tool_bytes_during_observation(self):
        value = load()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ('go', 'gomobile', 'gobind'):
                (root / name).write_bytes(('inert ' + name).encode())
            args = SimpleNamespace(platform='apple', go=root/'go', gomobile=root/'gomobile', gobind=root/'gobind')
            def run(command, **_):
                parts = [str(p) for p in command]
                if parts[1:] == ['version']:
                    (root / 'go').write_bytes(b'changed after Go observation')
                    return 'go version go' + value['goVersion'] + ' darwin/arm64'
                if parts[1:3] == ['version', '-m']:
                    name = Path(parts[-1]).name
                    return (parts[-1] + ': go' + value['goVersion'] + '\n\tpath\tgithub.com/sagernet/gomobile/cmd/' + name +
                            '\n\tmod\tgithub.com/sagernet/gomobile\tv0.1.13\th1:' + 'A' * 43 + '=\n')
                if parts == ['xcodebuild', '-version']:
                    return 'Xcode ' + value['apple']['xcodeVersion'] + '\nBuild version ' + value['apple']['xcodeBuild']
                if '--show-sdk-version' in parts:
                    return value['apple']['iPhoneSDK' if 'iphoneos' in parts else 'simulatorSDK']
                if '--show-sdk-path' in parts:
                    return str(root)
                if '--find' in parts:
                    return str(root / 'go')
                return 'inert compiler observation'
            (root / 'SDKSettings.json').write_bytes(b'inert SDK settings')
            with patch.object(producer, 'run', side_effect=run), self.assertRaisesRegex(RuntimeError, 'bytes changed'):
                producer.tool_facts(args, value, {})

    def test_work_and_shared_cache_have_exclusive_generator_custody(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with producer.slot_locks(root/'one', root/'cache'):
                with self.assertRaises(BlockingIOError):
                    with producer.slot_locks(root/'one', root/'other-cache'): pass
                with self.assertRaises(BlockingIOError):
                    with producer.slot_locks(root/'two', root/'cache'): pass


class ProducerLeaseLifecycle(unittest.TestCase):
    def test_contended_second_lock_closes_new_fds_and_keeps_only_outer_active_locks(self):
        with tempfile.TemporaryDirectory() as temporary, patch.object(producer, 'ACTIVE_FDS', []):
            root = Path(temporary).resolve()
            with producer.slot_locks(root / 'owner', root / 'z-cache'):
                outer = list(producer.ACTIVE_FDS)
                opened, waiting = [], []
                real_open, real_flock = os.open, producer.fcntl.flock
                def open_lock(path, *args, **kwargs):
                    fd = real_open(path, *args, **kwargs)
                    opened.append(fd)
                    return fd
                def flock(fd, operation):
                    waiting.append(fd)
                    self.assertNotIn(fd, producer.ACTIVE_FDS)
                    return real_flock(fd, operation)
                with patch.object(producer.os, 'open', side_effect=open_lock), \
                        patch.object(producer.fcntl, 'flock', side_effect=flock), \
                        self.assertRaises(BlockingIOError):
                    with producer.slot_locks(root / 'a-work', root / 'z-cache'):
                        self.fail('contended slot admitted')
                self.assertEqual(len(opened), 2)
                self.assertEqual(waiting, opened)
                self.assertEqual(producer.ACTIVE_FDS, outer)
                for fd in opened:
                    with self.assertRaises(OSError): os.fstat(fd)
                for fd in outer: os.fstat(fd)
                # The first inner lock was acquired, then released on failure.
                with producer.slot_locks(root / 'a-work', root / 'b-cache'): pass
            self.assertEqual(producer.ACTIVE_FDS, [])

    def test_invalid_inode_or_fstat_failure_closes_every_opened_descriptor(self):
        for failure in ('inode', 'fstat'):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary, \
                    patch.object(producer, 'ACTIVE_FDS', []):
                root = Path(temporary).resolve()
                work, cache = producer.private_dir(root / 'a-work'), producer.private_dir(root / 'z-cache')
                if failure == 'inode':
                    original = root / 'retained'; original.write_bytes(b'owned fixture')
                    os.link(original, cache / '.mobile-producer.lock')
                opened = []
                real_open, real_fstat = os.open, os.fstat
                def open_lock(path, *args, **kwargs):
                    fd = real_open(path, *args, **kwargs); opened.append(fd); return fd
                def fstat(fd):
                    if failure == 'fstat' and len(opened) == 2 and fd == opened[-1]:
                        raise OSError('inert fstat failure')
                    return real_fstat(fd)
                with patch.object(producer.os, 'open', side_effect=open_lock), \
                        patch.object(producer.os, 'fstat', side_effect=fstat), \
                        self.assertRaises((RuntimeError, OSError)):
                    with producer.slot_locks(work, cache): self.fail('invalid lock admitted')
                self.assertEqual(len(opened), 2)
                self.assertEqual(producer.ACTIVE_FDS, [])
                for fd in opened:
                    with self.assertRaises(OSError): real_fstat(fd)
                with producer.slot_locks(work, root / 'another-cache'): pass

    def test_evidence_reservation_reuses_ledger_but_cannot_authorize_a_build(self):
        import budget
        value = load()
        build_peak = value['budget']['buildPeakReserveBytes']
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            active, nested = [], []
            with patch.dict(os.environ, {'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                    patch.object(budget.os, 'statvfs', return_value=SimpleNamespace(f_bavail=10 * 1024 ** 3, f_frsize=1)):
                with budget.reservation(root / 'evidence', value, active, reserve_bytes=1024 ** 2):
                    fd = active[0]
                    self.assertEqual(json.loads(os.pread(fd, 8192, 0))['reserveBytes'], 1024 ** 2)
                    with budget.reservation(root / 'nested', value, nested, reserve_bytes=1024):
                        self.assertEqual(nested, [fd])
                        self.assertEqual(len(list((root / 'budget').glob('*.reservation'))), 1)
                    self.assertEqual(nested, [])
                    with self.assertRaisesRegex(RuntimeError, 'inherited reservation identity differs'):
                        with budget.reservation(root / 'native-build', value, nested):
                            self.fail('evidence reservation authorized native build')
                    self.assertEqual(active, [fd]); os.fstat(fd)
                with self.assertRaisesRegex(RuntimeError, 'peak reservations exhaust free space'):
                    with budget.reservation(root / 'native-build', value, active):
                        self.fail('actual build reserve reduced')
                self.assertEqual(active, [])
            self.assertEqual(value['budget']['buildPeakReserveBytes'], build_peak)

    def test_evidence_reservation_keeps_live_group_and_outstanding_slot_exclusion(self):
        import budget
        value = load()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            active = []
            with patch.dict(os.environ, {'POLARIS_BUILD_BUDGET_ROOT': str(root / 'budget')}, clear=True), \
                    patch.object(budget.os, 'statvfs', return_value=SimpleNamespace(f_bavail=10 * 1024 ** 3, f_frsize=1)):
                with budget.reservation(root / 'old-work', value, active, reserve_bytes=9 * 1024 ** 3):
                    fd = active[0]
                    row = json.loads(os.pread(fd, 8192, 0)); row['groups'] = [123456]
                    os.ftruncate(fd, 0); os.pwrite(fd, producer.canonical(row), 0)
                with patch.object(budget, 'group_alive', return_value=True):
                    with self.assertRaisesRegex(RuntimeError, 'retained live/unknown native session'):
                        with budget.reservation(root / 'old-work', value, active, reserve_bytes=1024): pass
                    with self.assertRaisesRegex(RuntimeError, 'peak reservations exhaust free space'):
                        with budget.reservation(root / 'evidence', value, active, reserve_bytes=1024): pass
                self.assertEqual(active, [])
