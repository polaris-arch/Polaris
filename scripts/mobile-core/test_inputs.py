"""Authored formal source/target controls; no Go, SDK, bind or native tool executes."""
from copy import deepcopy
import json
from pathlib import Path
import unittest

from fork_input import CONTRACT, bind_command, tool_environment, validate, verify_source, sha


class MobileInputs(unittest.TestCase):
    def setUp(self):
        self.value = json.loads(CONTRACT.read_bytes())

    def test_zero_overlay_does_not_require_historical_patch_inventory(self):
        self.assertEqual(validate(self.value)['patches'], [])
        for field, bad in [('sourceOverlay', 1), ('patches', [{'file': 'legacy.patch'}]),
                           ('artifactPins', {'cliReceipt': 'not-mobile'}), ('gomobileVersion', 'v0.1.12')]:
            value = deepcopy(self.value)
            value[field] = bad
            with self.subTest(field=field), self.assertRaises(RuntimeError):
                validate(value)

    def test_android_is_one_explicit_four_abi_api24_bind_without_helper_defaults(self):
        args = bind_command('android', self.value, '/isolated/gomobile', '/stage/core.aar', 'a' * 64, '/sdk/android.jar')
        self.assertEqual(args[args.index('-target') + 1], 'android/arm64,android/arm,android/386,android/amd64')
        self.assertEqual(args[args.index('-androidapi') + 1], '24')
        self.assertIn('with_gvisor', args[args.index('-tags') + 1].split(','))
        self.assertIn('with_naive_outbound', args[args.index('-tags') + 1].split(','))
        self.assertNotIn('21', args)
        self.assertNotIn('build_libbox', ' '.join(args))
        self.assertIn('constant.Version=1.15.0-alpha.12', args[args.index('-ldflags') + 1])
        self.assertNotIn('git describe', ' '.join(args))

    def test_apple_contains_two_arm64_ios_platform_slices_and_no_default_tv_or_macos(self):
        args = bind_command('apple', self.value, '/isolated/gomobile', '/stage/Libbox.xcframework', 'b' * 64)
        self.assertEqual(args[args.index('-target') + 1], 'ios/arm64,iossimulator/arm64')
        self.assertEqual(args[args.index('-iosversion') + 1], '15.0')
        for wrong in ['macos', 'tvos', 'amd64', 'x86_64']:
            self.assertNotIn(wrong, args[args.index('-target') + 1])

    def test_apple_plan_and_contract_reject_missing_duplicate_or_legacy_intel_targets(self):
        from plan import plan
        result = plan('apple', self.value, gomobile='/isolated/gomobile', output='/stage/Libbox.xcframework')
        self.assertEqual(result['facts']['targets'], ['ios/arm64', 'iossimulator/arm64'])
        self.assertEqual(self.value['apple']['slices'], ['ios-arm64', 'ios-arm64-simulator'])
        self.assertIn('two exact arm64 iOS platform slices', result['requiredOutputChecks'])
        self.assertFalse(result['AppAccepted'] or result['devicePassed'] or result['publicationEligible'])
        for field, alternatives in {
            'targets': [['ios/arm64'], ['ios/arm64', 'ios/arm64'],
                        ['ios/arm64', 'iossimulator/arm64', 'iossimulator/amd64'],
                        ['ios/arm64', 'iossimulator/amd64']],
            'slices': [['ios-arm64'], ['ios-arm64', 'ios-arm64'],
                       ['ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'],
                       ['ios-arm64', 'ios-x86_64-simulator']],
        }.items():
            for bad in alternatives:
                value = deepcopy(self.value); value['apple'][field] = bad
                with self.subTest(field=field, targets=bad), self.assertRaises(RuntimeError):
                    bind_command('apple', value, '/never-executed', '/never-written', 'b' * 64)

    def test_environment_does_not_upgrade_or_inherit_global_go_ndk_selection(self):
        from unittest.mock import patch
        with patch.dict('os.environ', {'GOTOOLCHAIN': 'auto', 'GOFLAGS': '-mod=mod',
                                     'ANDROID_HOME': '/global/sdk', 'ANDROID_NDK_HOME': '/global/ndk28'}, clear=True):
            env = tool_environment('/tools/go', '/tools/gomobile', '/tools/gobind', '/cache', '/scratch',
                                   sdk='/private/sdk-view', ndk='/verified/ndk30', jdk='/verified/jdk17')
        self.assertEqual(env['GOTOOLCHAIN'], 'local')
        self.assertEqual(env['GOFLAGS'], '-mod=readonly')
        self.assertEqual(env['ANDROID_HOME'], '/private/sdk-view')
        self.assertEqual(env['ANDROID_NDK_HOME'], '/verified/ndk30')
        self.assertNotIn('GOBIN', env)

    def test_pinned_javac_precedes_ambient_java21_path(self):
        import os
        import shutil
        import tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for name in ['jdk17', 'jdk21']:
                (root / name / 'bin').mkdir(parents=True)
                (root / name / 'bin/javac').write_text('inert executable fixture')
                (root / name / 'bin/javac').chmod(0o700)
            with patch.dict(os.environ, {'PATH': str(root / 'jdk21/bin')}, clear=True):
                env = tool_environment('/tools/go', '/tools/gomobile', '/tools/gobind', '/cache', '/scratch',
                                       sdk='/sdk', ndk='/ndk', jdk=root / 'jdk17')
            self.assertEqual(shutil.which('javac', path=env['PATH']), str(root / 'jdk17/bin/javac'))

    def test_ignored_objective_c_and_embed_inputs_refuse_zero_overlay(self):
        import tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = deepcopy(self.value)
            module = f"module {value['goModule']}\ngo {value['goVersion']}\nrequire (\n\tgithub.com/sagernet/gomobile {value['gomobileVersion']}\n)\n".encode()
            (root / 'go.mod').write_bytes(module); value['goModSha256'] = sha(module)
            (root / 'go.sum').write_bytes(b'fixture-sum'); value['goSumSha256'] = sha(b'fixture-sum')
            declared = root / value['declaredTagsSource']['path']; declared.parent.mkdir(parents=True)
            declared.write_bytes(b'fixture-tag-declaration'); value['declaredTagsSource']['sha256'] = sha(declared.read_bytes())
            ignored = []
            def git(_root, *args):
                if args[0] == 'cat-file': return 'tag'
                if args[0] == 'status': return ''
                if args[0] == 'ls-files': return '\n'.join(ignored) if '--ignored' in args else ''
                ref = args[1]
                return value['sourceTagObject'] if ref.endswith('^{tag}') else value['sourceTree'] if ref.endswith('^{tree}') else value['sourceCommit']
            with patch('fork_input.git', side_effect=git):
                self.assertEqual(verify_source(root, value)['sourceOverlay'], 0)
                for name in ['experimental/libbox/extra_darwin.m', 'experimental/libbox/extra.cxx', 'experimental/libbox/embedded.data']:
                    ignored[:] = [name]
                    with self.assertRaises(RuntimeError): verify_source(root, value)
                    with self.assertRaises(RuntimeError): verify_source(root, value, generator_outputs=True)
                ignored[:] = ['build/ios/Libbox.h']
                with self.assertRaises(RuntimeError): verify_source(root, value)
                self.assertEqual(verify_source(root, value, generator_outputs=True)['sourceOverlay'], 0)


class SourceStatusOpenRace(unittest.TestCase):
    @staticmethod
    def observe(root, raw):
        import consume
        from unittest.mock import patch
        facts = {'statusComplete': True, 'reason': None, 'observedStdoutBytes': len(raw),
                 'observedStderrBytes': 0, 'observedStatusPrefixSha256': sha(raw)}
        with patch.object(consume, 'ROOT', root), \
                patch.object(consume, 'bounded_source_status', return_value=(raw, facts)):
            return consume.source_status_observation()

    def test_lstat_open_fifo_replacement_is_nonblocking_incomplete_safe_metadata(self):
        import consume
        import os
        import tempfile
        import time
        from unittest.mock import patch
        secret = 'DO_NOT_EXPORT_SOURCE_BODY_OR_PATH'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / ('private-' + secret); path.write_bytes(secret.encode())
            opened = []
            real_open = os.open
            def replaced_open(target, flags, *args, **kwargs):
                self.assertEqual(Path(target), path)
                # Fail before opening if the nonblocking guard regresses, so
                # this negative control itself cannot wait on a FIFO writer.
                self.assertTrue(flags & os.O_NONBLOCK)
                self.assertTrue(flags & os.O_NOFOLLOW)
                path.unlink(); os.mkfifo(path)
                fd = real_open(target, flags, *args, **kwargs)
                opened.append(fd)
                return fd
            started = time.monotonic()
            with patch.object(consume.os, 'open', side_effect=replaced_open):
                observed = self.observe(root, b'?? ' + path.name.encode() + b'\0')
            self.assertLess(time.monotonic() - started, consume.STATUS_SECONDS)
            self.assertEqual(observed['reason'], 'changed-file')
            self.assertFalse(observed['complete'] or observed['clean'])
            self.assertEqual(observed['hashBytesRead'], 0)
            self.assertIsNone(observed['paths'][0]['sha256'])
            self.assertIsNone(observed['paths'][0]['path'])
            self.assertNotIn(secret, json.dumps(observed))
            self.assertEqual(len(opened), 1)
            with self.assertRaises(OSError): os.fstat(opened[0])
            with patch.object(consume, 'source_status_observation', return_value=observed), \
                    self.assertRaisesRegex(RuntimeError, 'source is not frozen'):
                consume.app_identity(status_log=root / 'status.json')
            self.assertNotIn(secret, (root / 'status.json').read_text())

    def test_opened_nonregular_object_is_rejected_even_if_inode_numbers_match(self):
        import consume
        import os
        import stat
        import tempfile
        from types import SimpleNamespace
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / 'Cargo.toml'; path.write_bytes(b'before replacement')
            original = path.stat()
            real_open, real_fstat = os.open, os.fstat
            opened = []
            def replaced_open(target, flags, *args, **kwargs):
                self.assertTrue(flags & os.O_NONBLOCK)
                path.unlink(); os.mkfifo(path)
                fd = real_open(target, flags, *args, **kwargs); opened.append(fd); return fd
            def reused_inode(fd):
                actual = real_fstat(fd)
                self.assertTrue(stat.S_ISFIFO(actual.st_mode))
                # Inode reuse must not replace the opened-object type check.
                return SimpleNamespace(st_dev=original.st_dev, st_ino=original.st_ino,
                    st_mode=actual.st_mode, st_size=actual.st_size, st_mtime_ns=actual.st_mtime_ns)
            with patch.object(consume.os, 'open', side_effect=replaced_open), \
                    patch.object(consume.os, 'fstat', side_effect=reused_inode):
                observed = self.observe(root, b' M Cargo.toml\0')
            self.assertEqual(observed['reason'], 'changed-file')
            self.assertEqual(observed['hashBytesRead'], 0)
            self.assertIsNone(observed['paths'][0]['sha256'])
            with self.assertRaises(OSError): real_fstat(opened[0])

    def test_regular_file_hash_and_symlink_boundaries_remain_fail_closed(self):
        import consume
        import os
        import tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / 'Cargo.toml'; path.write_bytes(b'reviewed bytes')
            observed = self.observe(root, b' M Cargo.toml\0')
            self.assertTrue(observed['complete'])
            self.assertFalse(observed['clean'])
            self.assertEqual(observed['paths'][0]['sha256'], sha(b'reviewed bytes'))
            other = root / 'private-target'; other.write_bytes(b'NEVER_EXPORT_SYMLINK_TARGET')
            path.unlink(); path.symlink_to(other)
            with patch.object(consume.os, 'open') as opened:
                observed = self.observe(root, b' M Cargo.toml\0')
                opened.assert_not_called()
            self.assertEqual(observed['paths'][0]['type'], 'symlink')
            self.assertIsNone(observed['paths'][0]['sha256'])
            path.unlink(); path.write_bytes(b'before replacement')
            real_open = os.open
            def symlink_open(target, flags, *args, **kwargs):
                path.unlink(); path.symlink_to(other)
                return real_open(target, flags, *args, **kwargs)
            with patch.object(consume.os, 'open', side_effect=symlink_open):
                observed = self.observe(root, b' M Cargo.toml\0')
            self.assertFalse(observed['complete'] or observed['clean'])
            self.assertEqual(observed['reason'], 'metadata-error')
            self.assertEqual(observed['hashBytesRead'], 0)
            self.assertNotIn('NEVER_EXPORT_SYMLINK_TARGET', json.dumps(observed))


if __name__ == '__main__':
    unittest.main()
