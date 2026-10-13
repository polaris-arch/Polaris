"""Pure controls only: authored for the eventual whole-candidate Node gate."""
import unittest
import tempfile
import json
import struct
import io
import zipfile
from pathlib import Path
from unittest import mock

import apple_rust
import ci_apple
import consume
from carriers import verify_apple_resources


def object_archive(cpu, subtype, platform):
    obj = struct.pack('<8I', 0xfeedfacf, cpu, subtype, 1, 1, 24, 0, 0)
    obj += struct.pack('<6I', 0x32, 24, platform, 15 << 16, 27 << 16, 0)
    body = b'go.o' + obj
    header = f'{"#1/4":<16}{0:<12}{0:<6}{0:<6}{"644":<8}{len(body):<10}`\n'.encode()
    return b'!<arch>\n' + header + body


def fat_archive(arm_platform=7, intel_platform=7):
    arm = object_archive(0x100000c, 0, arm_platform)
    intel = object_archive(0x1000007, 3, intel_platform)
    header = b'\xca\xfe\xba\xbe' + struct.pack('>I', 2)
    header += struct.pack('>5I', 0x100000c, 0, 64, len(arm), 4)
    header += struct.pack('>5I', 0x1000007, 3, 64 + len(arm), len(intel), 4)
    return header + bytes(64 - len(header)) + arm + intel, arm, intel


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
            framework = root / 'Libbox.xcframework'
            original = framework / 'ios-arm64/Libbox.framework/Libbox'
            original.parent.mkdir(parents=True); original.write_bytes(object_archive(0x100000c, 0, 2))
            selected = {'libraryIdentifier': 'ios-arm64', 'binary': file_pin(original)}
            link = {'App': {'candidate': 'a' * 40}, 'framework': str(framework),
                    'componentOutput': {'slices': {'ios-arm64': selected}}}
            thin, selection = apple_rust.selected_wire_bytes(original.read_bytes(), 'aarch64-apple-ios')
            wire = work / 'wire-aarch64-apple-ios.a'; wire.write_bytes(thin)
            receipt = {'schema': 'polaris-formal-apple-rust-v1', 'target': 'aarch64-apple-ios',
                       'App': link['App'], 'linkInputSha256': 'b' * 64, 'sdk': str(sdk),
                       'linkedArchive': str(archive), 'archive': file_pin(archive),
                       'cargoLock': file_pin(root / 'Cargo.lock'),
                       'iosConfig': file_pin(root / 'src-tauri/tauri.ios.conf.json'),
                       'tools': {'cargo': {'path': str(tool), **file_pin(tool)}}, 'devicePassed': False,
                       'originalWireArchive': {'path': str(original), **file_pin(original)},
                       'wireArchive': {'path': str(wire), **file_pin(wire)}, 'wireSelection': selection}
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

    def test_fat_simulator_selects_exact_cargo_architecture_not_first_descriptor(self):
        raw, arm, intel = fat_archive()
        actual, selection = apple_rust.selected_wire_bytes(raw, 'x86_64-apple-ios')
        self.assertEqual(actual, intel)
        self.assertEqual(selection['offset'], 64 + len(arm))
        self.assertEqual(apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios-sim')[0], arm)
        with self.assertRaisesRegex(RuntimeError, 'exact Cargo'):
            apple_rust.selected_wire_bytes(raw, 'aarch64-apple-ios')
        wrong_header = bytearray(raw); struct.pack_into('>I', wrong_header, 8, 0x1000007)
        with self.assertRaises(RuntimeError):
            apple_rust.selected_wire_bytes(bytes(wrong_header), 'x86_64-apple-ios')
        overlap = bytearray(raw); struct.pack_into('>I', overlap, 36, 64)
        with self.assertRaises(RuntimeError):
            apple_rust.selected_wire_bytes(bytes(overlap), 'x86_64-apple-ios')
        for invalid in [raw[:-1], b'!<arch>\n', b'\xcf\xfa\xed\xfe']:
            with self.subTest(raw=invalid[:8]), self.assertRaises(RuntimeError):
                apple_rust.selected_wire_bytes(invalid, 'x86_64-apple-ios')

    def test_real_resource_materialization_and_final_carrier_require_28_geo_and_licenses(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            data = root / 'resources/data'; data.mkdir(parents=True)
            for n in range(28):
                (data / f'geo-{n}.srs').write_bytes(b'SRS' + bytes([n]))
            for name in ('LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md'):
                (root / name).write_bytes(('actual ' + name).encode())
            config = root / 'src-tauri/tauri.ios.conf.json'; config.parent.mkdir(parents=True)
            config.write_text(json.dumps({'bundle': {'resources': ['../resources/data/',
                '../THIRD-PARTY-LICENSES.md', '../NOTICE', '../LICENSE']}}))
            (root / 'src-tauri/gen/apple').mkdir(parents=True)
            with mock.patch.object(consume, 'ROOT', root):
                expected = consume.materialize_apple_resources()
                self.assertEqual(len(expected), 31)
                self.assertEqual(consume.materialize_apple_resources(), expected)
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, 'w') as package:
                    for name, row in expected.items():
                        info = zipfile.ZipInfo('Payload/Polaris.app/assets/' + name)
                        info.create_system = 3; info.external_attr = (0o100000 | row['mode']) << 16
                        package.writestr(info, (root / row['source']).read_bytes())
                with zipfile.ZipFile(io.BytesIO(stream.getvalue())) as package:
                    observed = verify_apple_resources(package, 'Payload/Polaris.app/', expected)
                    self.assertEqual(len(observed), 31)
                    changed = {key: dict(row) for key, row in expected.items()}
                    key = next(iter(changed)); changed[key]['sha256'] = '0' * 64
                    with self.assertRaisesRegex(RuntimeError, 'bytes/mode'):
                        verify_apple_resources(package, 'Payload/Polaris.app/', changed)
                    with self.assertRaisesRegex(RuntimeError, 'inventory'):
                        verify_apple_resources(package, 'Payload/Polaris.app/', {})
                output = root / 'src-tauri/gen/apple/assets/_up_/NOTICE'
                output.write_bytes(b'wrong retained output')
                with self.assertRaisesRegex(RuntimeError, 'resource differs'):
                    consume.materialize_apple_resources()
                self.assertEqual(output.read_bytes(), b'wrong retained output')
                (data / 'geo-0.srs').unlink()
                with self.assertRaisesRegex(RuntimeError, '28 geo'):
                    consume.apple_resource_inputs()

    def test_resource_unknown_stage_is_preserved_and_never_replaced(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stage = root / 'src-tauri/gen/apple/assets.formal-stage'; stage.mkdir(parents=True)
            retained = stage / 'unknown'; retained.write_bytes(b'retained')
            with mock.patch.object(consume, 'ROOT', root), \
                    mock.patch.object(consume, 'apple_resource_inputs', return_value={}):
                with self.assertRaisesRegex(RuntimeError, 'stage retained'):
                    consume.materialize_apple_resources()
            self.assertEqual(retained.read_bytes(), b'retained')


if __name__ == '__main__':
    unittest.main()
