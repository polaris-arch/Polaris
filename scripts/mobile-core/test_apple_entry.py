"""Pure controls only: authored for the eventual whole-candidate Node gate."""
import unittest
import tempfile
from pathlib import Path
from unittest import mock

import apple_rust
import ci_apple


class AppleEntryTests(unittest.TestCase):
    def test_exact_three_actual_target_mappings(self):
        self.assertEqual(apple_rust.target_for('iphoneos', 'arm64', 'release'), 'aarch64-apple-ios')
        self.assertEqual(apple_rust.target_for('iphonesimulator', 'arm64', 'release'), 'aarch64-apple-ios-sim')
        self.assertEqual(apple_rust.target_for('iphonesimulator', 'x86_64', 'release'), 'x86_64-apple-ios')

    def test_host_arch_does_not_replace_explicit_intel_simulator(self):
        with mock.patch.dict(apple_rust.os.environ, {'HOST_ARCH': 'arm64'}):
            self.assertEqual(apple_rust.target_for('iphonesimulator', 'x86_64', 'release'), 'x86_64-apple-ios')

    def test_partial_or_wrong_platform_arch_profile_refuses(self):
        for platform, arch, profile in [('iphoneos', 'x86_64', 'release'), ('macosx', 'arm64', 'release'),
                                        ('iphonesimulator', 'arm64 x86_64', 'release'), ('iphoneos', 'arm64', 'debug')]:
            with self.subTest(platform=platform, arch=arch, profile=profile), self.assertRaises(RuntimeError):
                apple_rust.target_for(platform, arch, profile)

    def test_candidate_chain_has_all_three_finite_carriers(self):
        self.assertEqual(ci_apple.CARRIERS, [('device-arm64', None), ('sim-arm64', 'arm64'), ('sim-amd64', 'amd64')])

    def test_local_entry_does_not_claim_hosted_admission(self):
        with mock.patch.dict(ci_apple.os.environ, {'GITHUB_ACTIONS': 'false'}), self.assertRaises(RuntimeError):
            ci_apple.location()

    def test_actual_rust_link_verifier_refuses_changed_archive_and_drops_xcode_fd(self):
        from artifacts import file_pin
        from fork_input import canonical, sha
        from producer import write_json
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sdk = root / 'sdk'; sdk.mkdir()
            work = root / 'work'; work.mkdir(mode=0o700)
            archive = root / 'src-tauri/gen/apple/Externals/arm64/release/libapp.a'
            archive.parent.mkdir(parents=True); archive.write_bytes(b'!<arch>\ninert control')
            for name in ['Cargo.lock', 'src-tauri/tauri.ios.conf.json']:
                path = root / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_bytes(b'inert source')
            tool = root / 'tool'; tool.write_bytes(b'inert tool')
            link = {'App': {'candidate': 'a' * 40}}
            receipt = {'schema': 'polaris-formal-apple-rust-v1', 'target': 'aarch64-apple-ios',
                       'App': link['App'], 'linkInputSha256': 'b' * 64, 'sdk': str(sdk),
                       'linkedArchive': str(archive), 'archive': file_pin(archive),
                       'cargoLock': file_pin(root / 'Cargo.lock'),
                       'iosConfig': file_pin(root / 'src-tauri/tauri.ios.conf.json'),
                       'tools': {'cargo': {'path': str(tool), **file_pin(tool)}}, 'devicePassed': False}
            receipt['fingerprint'] = sha(canonical(receipt))
            write_json(work / 'rust-aarch64-apple-ios.json', receipt)
            env = {'POLARIS_FORMAL_APP_BUILD': '1', 'POLARIS_APP_WORK': str(work),
                   'PLATFORM_NAME': 'iphoneos', 'ARCHS': 'arm64', 'CONFIGURATION': 'release',
                   'SDKROOT': str(sdk), 'POLARIS_MOBILE_LINK_INPUT': str(work / 'link.json'),
                   'POLARIS_MOBILE_LINK_INPUT_SHA256': 'b' * 64, 'POLARIS_BUILD_RESERVATION_FD': '999999'}
            with mock.patch.object(apple_rust, 'ROOT', root), mock.patch.dict(apple_rust.os.environ, env), \
                    mock.patch.object(apple_rust, 'verify_link', return_value=link):
                apple_rust.verify()
                self.assertNotIn('POLARIS_BUILD_RESERVATION_FD', apple_rust.os.environ)
                archive.write_bytes(b'!<arch>\nchanged')
                with self.assertRaisesRegex(RuntimeError, 'input differs'):
                    apple_rust.verify()


if __name__ == '__main__':
    unittest.main()
