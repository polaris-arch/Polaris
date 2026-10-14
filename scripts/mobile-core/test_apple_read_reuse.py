"""Inert controls for per-invocation original-byte reuse; no native build."""
from collections import Counter
from pathlib import Path
import os
import plistlib
import tempfile
import unittest
from unittest.mock import patch

import artifacts
import carriers
from fork_input import load, sha
from artifacts import file_pin, module, read_pinned


class AppleReadReuse(unittest.TestCase):
    def test_parse_failure_remains_primary_when_observation_channel_breaks(self):
        primary = RuntimeError('controlled parse failure')
        observer = BrokenPipeError('controlled observer failure')
        with patch('producer.inspection_progress', side_effect=observer) as progress:
            with self.assertRaises(RuntimeError) as raised:
                with artifacts.inspection_stage('ipa-parse'):
                    raise primary
        self.assertIs(raised.exception, primary)
        self.assertEqual(raised.exception.__notes__,
                         ['Apple inspection observation failed; original failure preserved'])
        self.assertNotIn('controlled observer failure', str(raised.exception.__notes__))
        progress.assert_called_once()

    def test_cancellation_remains_primary_when_observation_channel_breaks(self):
        for primary in (KeyboardInterrupt(), SystemExit(130)):
            with self.subTest(kind=type(primary).__name__), \
                    patch('producer.inspection_progress', side_effect=BrokenPipeError('controlled observer failure')):
                with self.assertRaises(type(primary)) as raised:
                    with artifacts.inspection_stage('static-linkmap', {'fileReads': 1, 'readBytes': 7}):
                        raise primary
                self.assertIs(raised.exception, primary)
                self.assertEqual(raised.exception.__notes__,
                                 ['Apple inspection observation failed; original failure preserved'])

    def test_observation_only_failure_preserves_existing_failure_policy(self):
        observer = BrokenPipeError('controlled observer failure')
        with patch('producer.inspection_progress', side_effect=observer):
            with self.assertRaises(BrokenPipeError) as raised:
                with artifacts.inspection_stage('framework-inspection', {'fileReads': 2, 'readBytes': 9}):
                    pass
        self.assertIs(raised.exception, observer)
        self.assertFalse(hasattr(raised.exception, '__notes__'))

    def test_handled_outer_exception_does_not_suppress_observation_only_failure(self):
        observer = BrokenPipeError('controlled observer failure')
        try:
            raise ValueError('already handled outside inspection')
        except ValueError:
            with patch('producer.inspection_progress', side_effect=observer):
                with self.assertRaises(BrokenPipeError) as raised:
                    with artifacts.inspection_stage('ipa-parse'):
                        pass
            self.assertIs(raised.exception, observer)
            self.assertFalse(hasattr(raised.exception, '__notes__'))

    def link_fixture(self, root):
        framework = root / 'Libbox.xcframework'
        lib = framework / 'ios-arm64/Libbox.framework'
        (lib / 'Headers').mkdir(parents=True)
        binary = lib / 'Libbox'; binary.write_bytes(b'controlled archive bytes')
        header = lib / 'Headers/Libbox.h'; header.write_bytes(b'controlled header bytes')
        derived = root / 'derived'; derived.mkdir()
        selected = {'libraryIdentifier': 'ios-arm64', 'binary': file_pin(binary),
                    'headerSha256': {'Libbox.h': sha(header.read_bytes())},
                    'members': [{'name': 'go.o', 'ordinal': 1, 'goCarrier': True},
                                {'name': 'native.o', 'ordinal': 2, 'goCarrier': False}]}
        return framework, binary, header, derived, selected

    def link_text(self, rows):
        return '# Object files:\n' + '\n'.join(
            f'[ {ordinal}] {binary}({name})' for ordinal, binary, name in rows
        ) + '\n# Sections:\n# Symbols:\n'

    def test_members_share_only_canonical_file_reads_with_final_boundary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework, binary, header, derived, selected = self.link_fixture(root)
            text = self.link_text([(i + 1, binary, 'go.o' if i % 2 else 'native.o') for i in range(80)])
            calls = Counter()
            def pin(path, **kwargs):
                calls[Path(path)] += 1
                return file_pin(path, **kwargs)
            with patch('carriers.file_pin', side_effect=pin):
                rows = carriers.static_linkmap(text, framework, selected, derived)
            self.assertEqual(len(rows), 80)
            self.assertEqual(calls, Counter({binary: 2, header: 2}))
            self.assertEqual(rows[0]['archivePin'], selected['binary'])
            self.assertEqual([r['linkOrdinal'] for r in rows], list(range(1, 81)))

    def test_same_basename_in_distinct_derived_frameworks_never_reuses_pin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework, binary, header, derived, selected = self.link_fixture(root)
            other = derived / 'different/Libbox.framework/Libbox'
            other.parent.mkdir(parents=True)
            (other.parent / 'Headers').mkdir()
            other.write_bytes(b'different archive bytes')
            (other.parent / 'Headers/Libbox.h').write_bytes(header.read_bytes())
            text = self.link_text([(1, binary, 'go.o'), (2, other, 'native.o')])
            with self.assertRaisesRegex(RuntimeError, 'different/unbound'):
                carriers.static_linkmap(text, framework, selected, derived)
            other.write_bytes(binary.read_bytes())
            (other.parent / 'Headers/Libbox.h').write_bytes(b'different header bytes')
            with self.assertRaisesRegex(RuntimeError, 'headers changed'):
                carriers.static_linkmap(text, framework, selected, derived)

    def test_final_boundary_rejects_archive_or_header_changed_after_first_use(self):
        for changed in ('binary', 'header'):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temporary:
                framework, binary, header, derived, selected = self.link_fixture(Path(temporary).resolve())
                text = self.link_text([(1, binary, 'go.o'), (2, binary, 'native.o')])
                mutated = False
                def pin(path, **kwargs):
                    nonlocal mutated
                    result = file_pin(path, **kwargs)
                    if Path(path) == header and not mutated:
                        mutated = True
                        (binary if changed == 'binary' else header).write_bytes(b'changed after first read')
                    return result
                with patch('carriers.file_pin', side_effect=pin), \
                        self.assertRaisesRegex(RuntimeError, 'bytes changed during member'):
                    carriers.static_linkmap(text, framework, selected, derived)

    def test_reuse_preserves_per_line_bounds_members_coverage_and_ordinals(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            framework, binary, header, derived, selected = self.link_fixture(root)
            outside = root / 'unowned/Libbox.framework/Libbox'
            outside.parent.mkdir(parents=True); outside.write_bytes(binary.read_bytes())
            controls = [
                ([(1, binary, 'go.o'), (2, outside, 'native.o')], 'different/unbound'),
                ([(1, binary, 'go.o'), (2, binary, 'unknown.o')], 'different/unbound'),
                ([(1, binary, 'go.o'), (1, binary, 'native.o')], 'ordinals duplicate'),
                ([(1, binary, 'go.o')], 'generated/native'),
                ([(1, binary, 'native.o')], 'Go carrier'),
            ]
            for rows, message in controls:
                with self.subTest(message=message), self.assertRaisesRegex(RuntimeError, message):
                    carriers.static_linkmap(self.link_text(rows), framework, selected, derived)

    def framework_fixture(self, root):
        fixtures = module('read_reuse_framework_fixture', 'scripts/apple-carrier.test.py')
        from test_apple_entry import single_fat_archive
        framework = root / 'Libbox.xcframework'; framework.mkdir()
        expected_id = 'polaris-mobile-' + 'a' * 64
        def archive(target):
            return fixtures.archive([
                ('go.o', fixtures.mach_object(target, fixtures.inline('path\tsynthetic/main\nmod\tgithub.com/sagernet/sing-box\t(devel)\t\n'), expected_id)),
                ('native.o', fixtures.mach_object(target))])
        device = single_fat_archive(archive('ios-arm64'))
        sim = fixtures.fat(archive('ios-x86_64-simulator'), archive('ios-arm64-simulator'))
        entries, binaries, headers = [], [], []
        symbols = ['NewTransientCommandServer', 'NewStrictCommandServer', 'InterfaceUpdateListenerIdentity',
                   'CheckConfigWithResult', 'ConfigValidationContractVersion', 'exportTailscaleStoreRetirement',
                   'wakeNow', 'recordScreenState']
        getters = ['Cleanup', 'CleanupError', 'ConfigDigest', 'ContractVersion', 'RequestID',
                   'TailscaleStoreMembership', 'TailscaleStoreRetirement', 'Validation', 'ValidationError']
        header = '\n'.join(symbols) + '\n@interface LibboxConfigValidationResult : NSObject <goSeqRefInterface> {}\n'
        header += '\n'.join('- (NSString* _Nonnull)get' + name + ';' for name in getters) + '\n@end\n'
        for identifier, variant, arches, raw in [
                ('ios-arm64', None, ['arm64'], device),
                ('ios-arm64_x86_64-simulator', 'simulator', ['arm64', 'x86_64'], sim)]:
            entry = {'LibraryIdentifier': identifier, 'LibraryPath': 'Libbox.framework',
                     'SupportedPlatform': 'ios', 'SupportedArchitectures': arches}
            if variant: entry['SupportedPlatformVariant'] = variant
            entries.append(entry)
            lib = framework / identifier / 'Libbox.framework'
            (lib / 'Resources').mkdir(parents=True); (lib / 'Headers').mkdir()
            (lib / 'Resources/Info.plist').write_bytes(plistlib.dumps({}))
            binary = lib / 'Libbox'; binary.write_bytes(raw); binaries.append(binary)
            h = lib / 'Headers/Libbox.h'; h.write_text(header); headers.append(h)
        (framework / 'Info.plist').write_bytes(plistlib.dumps({'AvailableLibraries': entries}))
        return framework, binaries, headers, expected_id

    def test_framework_fat_parse_and_pin_share_raw_and_final_inventory_is_fresh(self):
        with tempfile.TemporaryDirectory() as temporary:
            framework, binaries, headers, expected_id = self.framework_fixture(Path(temporary).resolve())
            reads = Counter()
            def read(path, **kwargs):
                reads[Path(path)] += 1
                return read_pinned(path, **kwargs)
            with patch('artifacts.read_pinned', side_effect=read), patch('artifacts.check_buildinfo'):
                result = artifacts.inspect_framework(framework, load(), [], expected_id)
            for path in binaries + headers:
                self.assertEqual(reads[path], 2, 'first parse/pin plus final fresh boundary, not per-slice hashing')
            sim_arm = result['slices']['ios-arm64-simulator']['binary']
            sim_intel = result['slices']['ios-x86_64-simulator']['binary']
            self.assertEqual(sim_arm, sim_intel)
            self.assertEqual(sim_arm, file_pin(binaries[1]))
            inventory = {r['path']: r for r in result['inventory']}
            self.assertEqual(sim_arm, {k: inventory[binaries[1].relative_to(framework).as_posix()][k]
                                      for k in ('bytes', 'sha256')})

    def test_framework_final_boundary_rejects_binary_header_mode_or_layout_change(self):
        for changed in ('binary', 'header', 'mode', 'layout'):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temporary:
                framework, binaries, headers, expected_id = self.framework_fixture(Path(temporary).resolve())
                mutated = False
                def buildinfo(*args, **kwargs):
                    nonlocal mutated
                    if mutated: return
                    mutated = True
                    if changed == 'binary': binaries[0].write_bytes(b'changed binary')
                    elif changed == 'header': headers[0].write_bytes(b'changed header')
                    elif changed == 'mode': os.chmod(binaries[0], binaries[0].stat().st_mode ^ 0o100)
                    else: (framework / 'unexpected').write_bytes(b'added')
                with patch('artifacts.check_buildinfo', side_effect=buildinfo), \
                        self.assertRaisesRegex(RuntimeError, 'framework .*changed during'):
                    artifacts.inspect_framework(framework, load(), [], expected_id)

    def test_bounded_read_does_not_pin_growing_file(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'artifact'; path.write_bytes(b'first')
            original = Path.open
            def opened(p, *args, **kwargs):
                with original(path, 'wb') as stream:
                    stream.write(b'grew after size observation')
                return original(p, *args, **kwargs)
            with patch.object(Path, 'open', new=opened), \
                    self.assertRaisesRegex(RuntimeError, 'size changed'):
                read_pinned(path)


if __name__ == '__main__':
    unittest.main()
