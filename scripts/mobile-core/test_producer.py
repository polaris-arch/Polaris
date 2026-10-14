"""Producer call controls with inert output bytes; no Go/SDK/compiler executes."""
from contextlib import contextmanager, ExitStack
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import patch
import json

import producer
from artifacts import tree_inventory
from fork_input import load


class ProducerCalls(unittest.TestCase):
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
                alternate = Path(command[1].removeprefix('-modfile='))
                self.assertEqual(alternate.read_bytes(), originals['go.mod'])
                self.assertEqual(alternate.with_suffix('.sum').read_bytes(), originals['go.sum'])
                alternate.with_suffix('.sum').write_bytes(b'owned download sum update')
                return json.dumps({'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:fixed',
                                   'Dir': str(root/'cache/go-mod/known')})
            with patch.object(producer, 'run', side_effect=run):
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][0][-4:], ['mod', 'download', '-json', 'example.invalid/known@v1.0.0'])
            self.assertEqual(calls[0][1]['cwd'], source)
            self.assertEqual({name:(source/name).read_bytes() for name in originals}, originals)
            graph[1]['Dir'] = '/actual/warm/cache/known'
            with patch.object(producer, 'run') as run:
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            run.assert_not_called()

    def test_changed_download_checksum_or_retained_admission_view_fails_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); source = root / 'source'; source.mkdir(); work = root / 'work'; work.mkdir()
            (source/'go.mod').write_bytes(b'module example.invalid/main\n')
            (source/'go.sum').write_bytes(b'example.invalid/known v1.0.0 h1:fixed\n')
            graph = [{'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:fixed'}]
            changed = {'Path': 'example.invalid/known', 'Version': 'v1.0.0', 'Sum': 'h1:changed', 'Dir': '/fake/cache'}
            with patch.object(producer, 'run', return_value=json.dumps(changed)), \
                    self.assertRaisesRegex(RuntimeError, 'downloaded module identity differs'):
                producer.admit_module_cache(source, graph, 'inert-go', {}, work)
            with patch.object(producer, 'run') as run, self.assertRaisesRegex(RuntimeError, 'prior module admission inputs retained'):
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

    def test_actual_apple_bind_preserves_module_basename_and_isolates_source_mutation(self):
        value = load()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            supplied = root / 'source'; supplied.mkdir()
            developer = root / 'developer'; developer.mkdir()
            tool = root / 'inert-tool'; tool.write_bytes(b'not executed')
            bindings = []
            @contextmanager
            def lease(work, cache, _value):
                work.mkdir(mode=0o700, exist_ok=True); cache.mkdir(mode=0o700, exist_ok=True)
                yield work, cache
            def run(command, *, cwd=None, env=None, log=None):
                command = [str(item) for item in command]
                if command[:2] == ['git', 'clone']:
                    Path(command[-1]).mkdir()
                elif len(command) > 1 and command[1] == 'bind':
                    out = Path(command[command.index('-o') + 1])
                    # Pinned gomobile's framework name comes from output basename.
                    module = out.stem
                    self.assertEqual(module, 'Libbox')
                    out.mkdir(); (out / (module + '.h')).write_bytes(b'inert generated API')
                    generated = Path(cwd) / 'build/ios'; generated.mkdir(parents=True)
                    (generated / 'generated.m').write_bytes(b'inert generated Objective-C')
                    bindings.append((Path(cwd), out))
                    Path(log).write_text('inert original bind log')
                elif command[1:4] == ['list', '-m', '-json']:
                    return '{"Path":"github.com/sagernet/sing-box","Main":true}'
                return ''
            with ExitStack() as stack:
                stack.enter_context(patch.object(producer.os, 'uname', return_value=SimpleNamespace(sysname='Darwin')))
                for name, replacement in {
                    'lease': lease, 'producer_code': lambda: {'candidate': 'a'*40, 'files': {}},
                    'verify_source': lambda *a, **k: {'sourceOverlay': 0}, 'run': run,
                    'tool_facts': lambda *a: ({'go': str(tool), 'gomobile': str(tool), 'gobind': str(tool)}, {}),
                    'inspect_framework': lambda path, *a: {'inventory': tree_inventory(path), 'slices': {}},
                    'verify_receipt': lambda *a: None,
                }.items(): stack.enter_context(patch.object(producer, name, replacement))
                stack.enter_context(patch('native_inputs.collect', return_value={'files': {}}))
                stack.enter_context(patch('native_inputs.verify'))
                stack.enter_context(patch('builtins.print'))
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
                    producer.execute(args)
                    self.assertEqual(len(bindings), count)
                    self.assertEqual((args.work/'component-receipt.json').read_bytes(), saved)
                    self.assertEqual(generated.read_bytes(), original_generated)
                    with patch.object(producer, 'tool_facts', return_value=(
                            {'go':str(tool),'gomobile':str(tool),'gobind':str(tool)}, {'compiler':'changed'})), \
                            self.assertRaisesRegex(RuntimeError, 'original generator outputs retained'):
                        producer.execute(args)
                    self.assertEqual(len(bindings), count)
                    self.assertEqual((args.work/'component-receipt.json').read_bytes(), saved)
                    self.assertEqual(generated.read_bytes(), original_generated)
                    delta = producer.read_receipt(args.work/'cache-binding-observation.json')
                    self.assertEqual(delta['changedFields'], ['tools'])
            self.assertEqual(list(supplied.iterdir()), [])
            self.assertNotEqual(bindings[0][0], bindings[1][0])
            self.assertTrue(all(cwd.name == 'build-view' for cwd, _ in bindings))

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
                if parts[1:3] == ['version', '-m']: return 'github.com/sagernet/gomobile '+value['gomobileVersion']+'\n'
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

    def test_work_and_shared_cache_have_exclusive_generator_custody(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with producer.slot_locks(root/'one', root/'cache'):
                with self.assertRaises(BlockingIOError):
                    with producer.slot_locks(root/'one', root/'other-cache'): pass
                with self.assertRaises(BlockingIOError):
                    with producer.slot_locks(root/'two', root/'cache'): pass
