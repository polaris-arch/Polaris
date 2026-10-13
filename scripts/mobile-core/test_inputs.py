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

    def test_apple_contains_three_ios_slices_and_no_default_tv_or_macos(self):
        args = bind_command('apple', self.value, '/isolated/gomobile', '/stage/Libbox.xcframework', 'b' * 64)
        self.assertEqual(args[args.index('-target') + 1], 'ios/arm64,iossimulator/arm64,iossimulator/amd64')
        self.assertEqual(args[args.index('-iosversion') + 1], '15.0')
        for wrong in ['macos', 'tvos']:
            self.assertNotIn(wrong, args[args.index('-target') + 1])

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


if __name__ == '__main__':
    unittest.main()
