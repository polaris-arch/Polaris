"""Authored formal source/target controls; no Go, SDK, bind or native tool executes."""
from copy import deepcopy
import json
from pathlib import Path
import unittest

from fork_input import CONTRACT, bind_command, tool_environment, validate


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


if __name__ == '__main__':
    unittest.main()
