"""Pure dependency-cache controls; no Cargo, Xcode, network or actions executed."""
import errno
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import apple_cache
from apple_handoff import scope_fields, transport_pin


class AppleDependencyCacheTests(unittest.TestCase):
    def test_actual_app_package_and_library_outputs_are_never_cache_dependencies(self):
        for name in ('polaris-0123456789abcdef', 'libpolaris_lib-0123456789abcdef.a',
                     'polaris_lib-0123456789abcdef.d', 'libpolaris_lib.a', 'polaris.d'):
            self.assertIsNotNone(apple_cache.APP_ENTRY.match(name), name)
        for name in ('serde-0123456789abcdef', 'libserde-0123456789abcdef.rlib',
                     'polaris-config-engine-0123456789abcdef', 'libpolaris_config_engine-0123456789abcdef.rlib'):
            self.assertIsNone(apple_cache.APP_ENTRY.match(name), name)

    def test_tree_allows_only_complete_internal_hardlink_alias_sets(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            work = root / 'build'; work.mkdir()
            first = work / 'build_script_build-0123456789abcdef'; first.write_bytes(b'host build executable')
            os.link(first, work / 'build-script-build')
            self.assertEqual(apple_cache.checked_tree(work), (2 * first.stat().st_size, 3))
            os.link(first, root / 'external-alias')
            with self.assertRaises(apple_cache.ExternalCacheAlias):
                apple_cache.checked_tree(work)

    def test_tree_rejects_symlinks_special_files_and_byte_or_entry_overflow(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            data = root / 'data'; data.write_bytes(b'four')
            with self.assertRaisesRegex(RuntimeError, 'bytes'):
                apple_cache.checked_tree(root, maximum=3)
            with mock.patch.object(apple_cache, 'MAX_ENTRIES', 1):
                with self.assertRaisesRegex(RuntimeError, 'count'):
                    apple_cache.checked_tree(root)
            link = root / 'link'; link.symlink_to(data)
            with self.assertRaisesRegex(RuntimeError, 'symlink'):
                apple_cache.checked_tree(root)
            link.unlink()
            fifo = root / 'pipe'; os.mkfifo(fifo)
            with self.assertRaisesRegex(RuntimeError, 'regular'):
                apple_cache.checked_tree(root)

    def test_rename_failure_rolls_back_without_copy_or_overwrite(self):
        for failure in (OSError('interrupted rename'), OSError(errno.EXDEV, 'cross-device move')):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp).resolve()
                source = root / 'source'; source.mkdir()
                dest = root / 'stage'; dest.mkdir()
                one, two = source / 'one', source / 'two'
                one.write_bytes(b'first'); two.write_bytes(b'second')
                original = os.rename
                def rename(old, new):
                    if old == two:
                        raise failure
                    return original(old, new)
                with mock.patch.object(apple_cache.os, 'rename', side_effect=rename):
                    with self.assertRaises(OSError):
                        apple_cache.transfer([(one, dest / 'one'), (two, dest / 'two')])
                self.assertEqual(one.read_bytes(), b'first')
                self.assertEqual(two.read_bytes(), b'second')
                self.assertEqual(list(dest.iterdir()), [])
                (dest / 'two').write_bytes(b'existing bytes')
                with mock.patch.object(apple_cache.os, 'rename') as move:
                    with self.assertRaisesRegex(RuntimeError, 'already exists'):
                        apple_cache.transfer([(one, dest / 'one'), (two, dest / 'two')])
                    move.assert_not_called()
                self.assertEqual((dest / 'two').read_bytes(), b'existing bytes')

    def fixture(self, base):
        source = base / 'checkout'; source.mkdir()
        root = base / 'run-11-2'; root.mkdir(mode=0o700)
        cache = root / 'cache'; cache.mkdir(mode=0o700)
        stage = source / apple_cache.STAGE; stage.mkdir(mode=0o700, parents=True)
        context = {'candidate': 'a' * 40, 'runId': '11', 'runAttempt': '2'}
        owner = {'context': context, 'scope': scope_fields(), 'identity': 'b' * 64,
                 'actualCargoTargetDir': str(root / 'cache/rust-target')}
        (stage / 'owner.json').write_text(json.dumps(owner))
        target = root / 'cache/rust-target/aarch64-apple-ios/release'
        deps = target / 'deps'; deps.mkdir(parents=True, mode=0o755)
        deps.chmod(0o755)
        dep = deps / 'libserde-0123456789abcdef.rlib'; dep.write_bytes(b'compiled dependency')
        app = deps / 'libpolaris_lib-0123456789abcdef.a'; app.write_bytes(b'App native archive')
        final = target / 'libpolaris_lib.a'; final.write_bytes(b'original final staticlib')
        fingerprint = target / '.fingerprint/polaris-0123456789abcdef'
        fingerprint.mkdir(parents=True); (fingerprint / 'lib-polaris_lib').write_bytes(b'App fingerprint')
        host = root / 'cache/rust-target/release/build/serde-0123456789abcdef'
        host.mkdir(parents=True)
        build = host / 'build_script_build-0123456789abcdef'; build.write_bytes(b'host compiler output')
        os.link(build, host / 'build-script-build')
        original = root / 'apple-original-device-arm64-evidence.zip'; original.write_bytes(b'immutable retained original')
        (root / 'evidence-device-arm64-pin.json').write_text(json.dumps({
            'slot': 'device-arm64', 'context': context, 'archive': transport_pin(original), 'receiptReadErrors': {}}))
        summary = root / 'summaries/device-arm64'; summary.mkdir(parents=True)
        (summary / 'carrier-device-arm64.json').write_text('{}')
        return source, root, stage, context, dep, app, final, host

    def patched(self, source, root, context):
        return (mock.patch.object(apple_cache, 'ROOT', source),
                mock.patch.object(apple_cache, 'location', return_value=root),
                mock.patch.object(apple_cache, 'context', return_value=context),
                mock.patch.object(apple_cache, 'check_space', return_value=40 * 1024 ** 3))

    def test_roundtrip_preserves_original_app_bytes_inode_aliases_and_native_directory_mode(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            inode = dep.stat().st_ino
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                apple_cache.export_cache('device-arm64')
                self.assertFalse(dep.exists())
                self.assertEqual(app.read_bytes(), b'App native archive')
                self.assertEqual(final.read_bytes(), b'original final staticlib')
                self.assertFalse(any(p.name.startswith('libpolaris_lib') for p in (stage / 'device-arm64').rglob('*')))
                self.assertEqual((stage / 'device-arm64/host/build/serde-0123456789abcdef/build-script-build').stat().st_nlink, 2)
                apple_cache.import_cache('device-arm64', 'staged')
                observation = json.loads((root / 'apple-cache-return-device-arm64.json').read_text())
                self.assertEqual(observation['cacheState'], 'local-staged-return')
            self.assertEqual(dep.stat().st_ino, inode)
            self.assertEqual(dep.parent.stat().st_mode & 0o777, 0o755)
            self.assertEqual((host / 'build-script-build').stat().st_nlink, 2)
            self.assertEqual(app.read_bytes(), b'App native archive')
            self.assertEqual(final.read_bytes(), b'original final staticlib')
            self.assertFalse((stage / 'device-arm64').exists())

    def test_external_dependency_alias_stays_native_and_is_explicitly_reported_uncached(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            os.link(dep, root / 'external-dependency-alias')
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                apple_cache.export_cache('device-arm64')
                observation = json.loads((root / 'apple-cache-export-device-arm64.json').read_text())
                self.assertEqual(observation['uncachedExternalAliasEntries'],
                                 ['aarch64-apple-ios/release/deps/' + dep.name])
                self.assertTrue(dep.exists())
                apple_cache.import_cache('device-arm64', 'hit')
            self.assertEqual(dep.read_bytes(), b'compiled dependency')

    def test_import_refuses_cross_scope_identity_extra_roles_app_products_and_false_hits(self):
        for mutation in ('scope', 'identity', 'extra-role', 'app', 'unexpected-miss'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as tmp, \
                    mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
                source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
                p1, p2, p3, p4 = self.patched(source, root, context)
                with p1, p2, p3, p4:
                    apple_cache.export_cache('device-arm64')
                    payload = stage / 'device-arm64'
                    if mutation in ('scope', 'identity'):
                        marker = payload / 'cache.json'; row = json.loads(marker.read_text())
                        row[mutation] = 'wrong'
                        marker.write_text(json.dumps(row))
                    elif mutation == 'extra-role':
                        (payload / 'simulator').mkdir()
                    elif mutation == 'app':
                        (payload / 'target/deps/libpolaris_lib-1111111111111111.a').write_bytes(b'never reuse App')
                    with self.assertRaises(RuntimeError):
                        apple_cache.import_cache('device-arm64', 'miss' if mutation == 'unexpected-miss' else 'hit')
                    self.assertFalse(dep.exists())
                    self.assertEqual(app.read_bytes(), b'App native archive')
                    self.assertEqual(final.read_bytes(), b'original final staticlib')

    def test_low_space_wrong_owner_and_active_cache_lock_refuse_before_moves(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            with mock.patch.object(apple_cache.os, 'statvfs', return_value=mock.Mock(f_bavail=0, f_frsize=4096)), \
                    mock.patch.object(apple_cache.os, 'rename') as move:
                with self.assertRaisesRegex(RuntimeError, 'headroom'):
                    apple_cache.check_space(root, 1024)
                move.assert_not_called()
            wrong_owner = os.getuid() + 1
            with mock.patch.object(apple_cache.os, 'getuid', return_value=wrong_owner):
                with self.assertRaisesRegex(RuntimeError, 'ownership'):
                    apple_cache.checked_tree(dep)
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4, \
                    mock.patch.object(apple_cache, 'slot_locks', side_effect=BlockingIOError('active native cache')), \
                    mock.patch.object(apple_cache.os, 'rename') as move:
                with self.assertRaisesRegex(BlockingIOError, 'active native cache'):
                    apple_cache.export_cache('device-arm64')
                move.assert_not_called()
            self.assertEqual(dep.read_bytes(), b'compiled dependency')
            self.assertEqual(app.read_bytes(), b'App native archive')
            self.assertEqual(final.read_bytes(), b'original final staticlib')
            self.assertFalse((stage / 'device-arm64').exists())

    def test_export_requires_preserved_originals_and_never_accepts_unrequested_simulator(self):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}):
            source, root, stage, context, dep, app, final, host = self.fixture(Path(tmp).resolve())
            (root / 'apple-original-device-arm64-evidence.zip').write_bytes(b'changed original')
            p1, p2, p3, p4 = self.patched(source, root, context)
            with p1, p2, p3, p4:
                with self.assertRaisesRegex(RuntimeError, 'preserved'):
                    apple_cache.export_cache('device-arm64')
                self.assertTrue(dep.exists())
                self.assertFalse((stage / 'device-arm64').exists())
                with self.assertRaisesRegex(RuntimeError, 'unrequested'):
                    apple_cache.import_cache('sim-arm64', 'miss')
                with self.assertRaisesRegex(RuntimeError, 'no dependency payload'):
                    apple_cache.import_cache('device-arm64', 'hit')
                apple_cache.import_cache('device-arm64', 'miss')


if __name__ == '__main__':
    unittest.main()
