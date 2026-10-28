"""Authored formal source/target controls; no Go, SDK, bind or native tool executes."""
from copy import deepcopy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from fork_input import CONTRACT, bind_command, tool_environment, validate, verify_source, sha


class MobileInputs(unittest.TestCase):
    def setUp(self):
        from test_source_policy import future_mobile_fixture
        self.value, _ = future_mobile_fixture()

    def test_selected_patch_source_has_real_final_tag_but_no_artifact_acceptance(self):
        value = json.loads(CONTRACT.read_bytes())
        self.assertEqual(validate(value)['sourceCommit'], 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e')
        self.assertEqual(value['sourceTree'], '4c0fdb31d59967f41c808ea6b63f87ca9661c2cf')
        self.assertEqual(value['sourceTagObject'], '8a230c1d34cb4855b261944e2935ce36f9cd14de')
        self.assertEqual((value['version'], value['sourceTag']),
                         ('1.15.0-polaris-alpha.12.6', 'v1.15.0-polaris-alpha.12.6'))
        self.assertEqual(value['sourcePolicy']['schema'], 'polaris-reviewed-kernel-source-v3')
        self.assertEqual(len(value['sourcePolicy']['vendors']), 10)
        self.assertEqual(sum(pin['replacementFileCount'] for pin in value['sourcePolicy']['vendors'].values()), 2876)
        for key in ('artifactPins', 'producerReceipt'):
            self.assertIsNone(value[key])
        for key in ('AppAccepted', 'devicePassed', 'publicationEligible'):
            self.assertIs(value[key], False)
        from fork_input import linker_flags
        self.assertIn('DirectoryRetirementSourceIdentity=' + value['sourceCommit'], linker_flags(value, 'a' * 64))
        for previous in ('332935c310e78e15319d434f33a9103329200931',
                         '0cfa24573ff34c26608151068334329268cfdf14',
                         'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a',
                         '368b2489784dbe06498a18e83cc2257f94868c46',
                         '814da362838e96e54f904836a92c2b0d8c3ae29b',
                         'ba73fcfca84f257680f6f46b87fdcce98b5ab94d',
                         'a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae',
                         'c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e',
                         'fe182431495711c09bdb3b074ed6aa6309c00cf8',
                         '68ffb209ded1f9337f9115741cba24f9f4c63421'):
            wrong = deepcopy(value); wrong['sourceCommit'] = previous
            with self.subTest(previous=previous), self.assertRaises(RuntimeError): validate(wrong)

    def test_final_desktop_and_mobile_source_bind_without_relabeling_history(self):
        import tempfile
        from unittest.mock import patch
        pending_path = Path(__file__).resolve().parents[1] / 'desktop-core/pending-release-selection.json'
        pending = json.loads(pending_path.read_bytes())['nextDelivery']
        selected = json.loads(CONTRACT.read_bytes())
        historical_bytes = (CONTRACT.parent / 'fixtures/selected-source-contract-a4d.json').read_bytes()
        self.assertEqual(sha(historical_bytes), 'd2defda63a5b8609edf8d4e86c5dc9c6c49bd9ad3dbfe437bdc863b6bd2345c0')
        historical = validate(json.loads(historical_bytes))
        self.assertEqual(historical['sourceCommit'], 'a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae')
        self.assertEqual(historical['sourceTree'], '830f7256af499e5ad3e7da9a79f0a377ca38fc77')
        self.assertEqual(historical['sourceTagObject'], '916d9533ba0a6092e34bc53f63dace63e11fa85e')
        self.assertEqual(pending['sourceCommit'], selected['sourceCommit'])
        self.assertEqual(pending['sourceTree'], selected['sourceTree'])
        self.assertTrue(pending['activationEligible'])
        contract = Path(__file__).resolve().parents[1] / 'desktop-core/release-contract.json'
        self.assertEqual(pending['contractSha256'], sha(contract.read_bytes()))
        published = json.loads(contract.read_bytes())
        self.assertIs(type(published['releaseId']), int)
        self.assertGreater(published['releaseId'], 0)
        self.assertEqual(pending['releaseId'], published['releaseId'])
        self.assertEqual(published['sourceCommit'], selected['sourceCommit'])
        self.assertEqual(published['version'], selected['version'])
        self.assertEqual(pending['tagObject'], selected['sourceTagObject'])
        self.assertEqual(pending['mobileSourceContractSha256'], sha(CONTRACT.read_bytes()))
        self.assertEqual(pending['sourcePolicy'], selected['sourcePolicy'])
        self.assertEqual(selected['sourceCommit'], 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e')
        self.assertNotEqual(pending['sourceCommit'], historical['sourceCommit'])
        self.assertNotEqual(pending['tagObject'], historical['sourceTagObject'])
        self.assertNotEqual(pending['mobileSourceContractSha256'], sha(historical_bytes))
        future = deepcopy(selected)
        future['sourceTagObject'] = None
        with self.assertRaisesRegex(RuntimeError, 'actual immutable source field required: sourceTagObject'):
            validate(future)
        # A well-shaped historical tag hash is still not the new source's tag.
        # Actual peel verification must reject it before module files or bind.
        for tag, commit in [('76e4f7db6767472952d09e4cf35b2cf90c223fbf', '332935c310e78e15319d434f33a9103329200931'),
                            ('db094db3498ea6982ccc601289acbd0c8adcecac', 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a'),
                            ('8ecda357ea755a1295336a35ec7eaf1fe2daebc2', '814da362838e96e54f904836a92c2b0d8c3ae29b'),
                            ('2fee2965920261ce7b0486fd94b3c9c3c00f6643', 'ba73fcfca84f257680f6f46b87fdcce98b5ab94d'),
                            (historical['sourceTagObject'], historical['sourceCommit']),
                            ('a85a5332432a92e1de502cfc9c680206371540b8', 'c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e')]:
            future['sourceTagObject'] = tag
            with self.subTest(tag=tag), tempfile.TemporaryDirectory() as directory:
                def historical_tag(_root, *args):
                    if args[0] == 'cat-file': return 'tag'
                    if args[1].endswith('^{tag}'): return tag
                    if args[1].endswith('^{commit}'): return commit
                    self.fail('must reject the old tag peel before additional source reads')
                with patch('fork_input.git', side_effect=historical_tag):
                    with self.assertRaisesRegex(RuntimeError, 'mobile source tag/object/commit/tree differs'):
                        verify_source(directory, future)
        self.assertIsNone(selected['artifactPins'])
        self.assertIsNone(selected['producerReceipt'])

    def test_final_source_keeps_reverified_module_meta_and_libbox_declaration_pins(self):
        value = validate(json.loads(CONTRACT.read_bytes()))
        for field, digest in (
            ('goModSha256', 'd16c533c97de535cf38985a62a78993c1b755a8f723a78ea8a0d75d585d7201d'),
            ('goSumSha256', '04f10f1f02bd405b9d0e84407e44b35d45af49db7e10a96630679cedaefadeaa'),
        ):
            self.assertEqual(value[field], digest)
            self.assertEqual(value['sourcePolicy'][field], digest)
        self.assertEqual(value['declaredTagsSource'], {
            'path': 'cmd/internal/build_libbox/main.go',
            'sha256': 'f32a147fe0f3f763dd2719887a805ec0c058d87297b0cbddadeeaddfdac32d06',
        })
        meta = value['sourcePolicy']['metacubex']
        self.assertEqual(meta['replacementTree'], '40304faa4cfb50c928732bbbb476e22fb9119735')
        self.assertEqual(meta['replacementFileCount'], 355)
        self.assertEqual(meta['bindingSha256'], 'bb44c0350b1d8ae65f364f986085760c024ccd9c9653bd89cb0e407de850888b')
        self.assertEqual(meta['patchSha256'], 'a0f7bcf8429b003e008ac38fa4a4c083f8c8397b4d75f5e3bd992e24d8fcc932')

    def test_same_version_repack_changes_mobile_input_identity_without_accepting_all_archives(self):
        from fork_input import linker_flags
        from plan import plan
        historical_bytes = (CONTRACT.parent / 'fixtures/selected-source-contract-a4d.json').read_bytes()
        self.assertEqual(sha(historical_bytes), 'd2defda63a5b8609edf8d4e86c5dc9c6c49bd9ad3dbfe437bdc863b6bd2345c0')
        value = validate(json.loads(historical_bytes))
        previous = deepcopy(value)
        previous.update(sourceCommit='c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e',
                        sourceTree='cf520419eafa63bc789a67f12417b7a82e3d7bd9',
                        sourceTagObject='a85a5332432a92e1de502cfc9c680206371540b8')
        for field in ('sourceCommit', 'sourceTree'):
            previous['sourcePolicy'][field] = previous[field]
        self.assertEqual(value['version'], previous['version'])
        self.assertEqual(value['sourceTag'], previous['sourceTag'])
        for platform in ('android', 'apple'):
            args = dict(gomobile='/not-executed/gomobile', output='/not-written/component',
                        bootclasspath='/not-read/android.jar' if platform == 'android' else None)
            current_plan, previous_plan = plan(platform, value, **args), plan(platform, previous, **args)
            self.assertNotEqual(current_plan['fingerprint'], previous_plan['fingerprint'])
            self.assertIn('-buildid=polaris-mobile-' + current_plan['fingerprint'],
                          linker_flags(value, current_plan['fingerprint']))
            self.assertIsNone(current_plan['producerReceipt'])
            self.assertFalse(current_plan['facts']['artifactAcceptance'])
            for key in ('AppAccepted', 'devicePassed', 'publicationEligible'):
                self.assertIs(current_plan[key], False)

    def test_final_reviewed_toolchain_keeps_minimum_go_distinct_from_actual_compiler(self):
        from test_source_policy import future_mobile_fixture
        value, _ = future_mobile_fixture()
        self.assertEqual(validate(value)['goVersion'], '1.27.2')
        self.assertEqual(value['sourcePolicy']['goDirective'], '1.27.0')
        for field, bad in [('sourceCommit', '0' * 40), ('sourceTree', '0' * 40), ('goVersion', '1.28.0')]:
            changed = deepcopy(value); changed[field] = bad
            with self.subTest(field=field), self.assertRaises(RuntimeError): validate(changed)
        workflow = (Path(__file__).resolve().parents[2] / '.github/workflows/apple.yml').read_text()
        self.assertIn('fork_input.py --go-version', workflow)
        self.assertIn('go-version: ${{ steps.mobile-go.outputs.version }}', workflow)
        self.assertLess(workflow.index('fork_input.py --go-version'), workflow.index('actions/setup-go@'))

    def test_exact_new_fork_identity_rejects_old_or_mixed_repository(self):
        self.assertEqual(self.value['repository'], 'polaris-arch/sing-box-polaris')
        self.assertEqual(self.value['repositoryId'], 1421657509)
        self.assertEqual(self.value['sourceURL'], 'https://github.com/polaris-arch/sing-box-polaris')
        validate(self.value)
        for field, bad in [('repository', 'polaris-arch/polaris-box'),
                           ('repositoryId', 1407705262),
                           ('sourceURL', 'https://github.com/polaris-arch/polaris-box'),
                           ('repository', 'SagerNet/sing-box')]:
            value = deepcopy(self.value)
            value[field] = bad
            with self.subTest(field=field), self.assertRaisesRegex(RuntimeError, 'repository differs'):
                validate(value)

    def test_polaris_release_version_and_exact_tag_are_required(self):
        self.assertEqual(self.value['version'], '1.15.0-polaris-alpha.12')
        self.assertEqual(self.value['sourceTag'], 'v1.15.0-polaris-alpha.12')
        validate(self.value)
        for version, tag in [
            ('1.15.0-alpha.12', 'v1.15.0-alpha.12'),
            ('1.15.0-polaris-alpha.12', 'v1.15.0-alpha.12'),
            ('1.15.0-polaris-alpha.12', 'v1.15.0-polaris-alpha.13'),
            ('1.15.0-polaris-alpha.12', '1.15.0-polaris-alpha.12'),
            ('1.15.0-polaris-alpha.12-extra', 'v1.15.0-polaris-alpha.12-extra'),
        ]:
            value = deepcopy(self.value)
            value.update(version=version, sourceTag=tag)
            with self.subTest(version=version, tag=tag), self.assertRaisesRegex(RuntimeError, 'tag/version differs'):
                validate(value)

    def test_shared_patch_version_corpus_keeps_alpha_only_and_full_raw_identity(self):
        from fork_input import linker_flags
        cases = json.loads((Path(__file__).resolve().parents[1] / 'lib/polaris-version-fixtures.json').read_bytes())
        for version in cases['accepted'] + cases['rejected']:
            value = deepcopy(self.value)
            old = value['version']
            value.update(version=version, sourceTag='v' + version)
            for platform in ('android', 'apple'):
                value[platform]['outputName'] = value[platform]['outputName'].replace(old, version)
            with self.subTest(version=version):
                if version in cases['accepted'] and '-polaris-alpha.' in version:
                    self.assertEqual(validate(value)['version'], version)
                    self.assertIn('constant.Version=' + version, linker_flags(value, 'a' * 64))
                    self.assertEqual(value['sourceTag'], 'v' + version)
                    self.assertFalse(value['AppAccepted'])
                    self.assertFalse(value['publicationEligible'])
                    value['sourceTag'] = 'v1.15.0-polaris-alpha.12.999'
                    with self.assertRaisesRegex(RuntimeError, 'tag/version differs'): validate(value)
                else:
                    with self.assertRaisesRegex(RuntimeError, 'tag/version differs'): validate(value)

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
        self.assertIn('-X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' +
                      self.value['sourceCommit'], args[args.index('-ldflags') + 1])
        self.assertEqual(args[args.index('-target') + 1], 'android/arm64,android/arm,android/386,android/amd64')
        self.assertEqual(args[args.index('-androidapi') + 1], '24')
        self.assertIn('with_gvisor', args[args.index('-tags') + 1].split(','))
        self.assertIn('with_naive_outbound', args[args.index('-tags') + 1].split(','))
        self.assertNotIn('21', args)
        self.assertNotIn('build_libbox', ' '.join(args))
        self.assertIn('constant.Version=1.15.0-polaris-alpha.12', args[args.index('-ldflags') + 1])
        self.assertNotIn('git describe', ' '.join(args))

    def test_apple_contains_two_arm64_ios_platform_slices_and_no_default_tv_or_macos(self):
        args = bind_command('apple', self.value, '/isolated/gomobile', '/stage/Libbox.xcframework', 'b' * 64)
        self.assertIn('-X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' +
                      self.value['sourceCommit'], args[args.index('-ldflags') + 1])
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

    def test_apple_actual_environment_excludes_host_pkgconfig_search_extensions(self):
        import os
        from unittest.mock import patch
        for host_path in ('/Users/runner/hostedtoolcache/Python/3.12.10/arm64/lib/pkgconfig',
                          '/custom/untracked/pkgconfig'):
            with self.subTest(host_path=host_path), patch.dict(os.environ, {
                    'PKG_CONFIG_PATH': host_path, 'CPATH': '/custom/headers',
                    'PKG_CONFIG_LIBDIR': '/custom/libconfig'}, clear=True):
                apple = tool_environment('/tools/go', '/tools/gomobile', '/tools/gobind',
                                         '/cache', '/scratch', developer='/verified/Xcode')
                self.assertNotIn('PKG_CONFIG_PATH', apple)
                self.assertEqual(apple['CPATH'], '/custom/headers')
                self.assertEqual(apple['PKG_CONFIG_LIBDIR'], '/custom/libconfig')
                self.assertEqual(os.environ['PKG_CONFIG_PATH'], host_path)
                android = tool_environment('/tools/go', '/tools/gomobile', '/tools/gobind',
                                           '/cache', '/scratch', sdk='/sdk', ndk='/ndk', jdk='/jdk')
                self.assertEqual(android['PKG_CONFIG_PATH'], host_path)

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
            value = json.loads((Path(__file__).with_name('fixtures') / 'legacy-source-contract.json').read_bytes())
            module = f"module {value['goModule']}\ngo {value['goVersion']}\nrequire (\n\tgithub.com/sagernet/gomobile {value['gomobileVersion']}\n)\n".encode()
            (root / 'go.mod').write_bytes(module); value['goModSha256'] = sha(module)
            (root / 'go.sum').write_bytes(b'fixture-sum'); value['goSumSha256'] = sha(b'fixture-sum')
            declared = root / value['declaredTagsSource']['path']; declared.parent.mkdir(parents=True)
            declared.write_bytes(b'fixture-tag-declaration'); value['declaredTagsSource']['sha256'] = sha(declared.read_bytes())
            ignored = []
            tracked_status = ''
            observed_head = value['sourceCommit']
            def git(_root, *args):
                if args[0] == 'cat-file': return 'tag'
                if args[0] == 'status': return tracked_status
                if args[0] == 'ls-files': return '\n'.join(ignored) if '--ignored' in args else ''
                ref = args[1]
                if ref == 'HEAD': return observed_head
                return value['sourceTagObject'] if ref.endswith('^{tag}') else value['sourceTree'] if ref.endswith('^{tree}') else value['sourceCommit']
            with patch('fork_input.git', side_effect=git):
                self.assertEqual(verify_source(root, value)['sourceOverlay'], 0)
                for tracked_status in [' M go.mod', 'M  experimental/libbox/command_server.go']:
                    with self.assertRaises(RuntimeError): verify_source(root, value)
                tracked_status = ''
                observed_head = '0' * 40
                with self.assertRaises(RuntimeError): verify_source(root, value)
                observed_head = value['sourceCommit']
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


class FormalStableNdk(unittest.TestCase):
    """Installer/source data only. Commands are mocked and native execution forbidden."""
    @classmethod
    def setUpClass(cls):
        cls.root = Path(__file__).resolve().parents[2]
        spec = importlib.util.spec_from_file_location('formal_ndk_fixture', cls.root / 'scripts/libbox-patches/prepare-ndk-linux.py')
        cls.installer = importlib.util.module_from_spec(spec); spec.loader.exec_module(cls.installer)
        cls.profile = cls.installer.formal_profile(cls.installer.FORMAL_PROFILE)
        cls.properties = (b'Pkg.Desc = Android NDK\nPkg.Revision = 30.0.16248370\n'
                          b'Pkg.BaseRevision = 30.0.16248370\nPkg.ReleaseName = r30\n')

    def setUp(self):
        guard = patch('subprocess.Popen', side_effect=AssertionError('fixture may not execute tools'))
        guard.start(); self.addCleanup(guard.stop)

    def sdk(self, root):
        home = root / 'sdk'; (home / 'licenses').mkdir(parents=True)
        accepted = home / 'licenses/android-sdk-license'
        accepted.write_text(self.profile['licenseSha1'] + '\n')
        return home, accepted

    def archive(self, names=None, properties=None):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as bundle:
            for name in names or ['android-ndk-r30/source.properties']:
                bundle.writestr(name, self.properties if properties is None else properties)
        raw = stream.getvalue(); profile = deepcopy(self.profile)
        profile['archive'].update(bytes=len(raw), sha1=hashlib.sha1(raw).hexdigest(), sha256=sha(raw))
        return raw, profile

    def test_current_and_historical_profiles_are_exact_and_cannot_mix(self):
        from fork_input import load
        current = load()
        legacy = validate(json.loads((Path(__file__).with_name('fixtures') / 'legacy-source-contract.json').read_bytes()))
        keys = ('ndkRevision', 'ndkSourceRevision', 'ndkPackage', 'ndkArchivePath')
        self.assertEqual(tuple(current['android'][key] for key in keys),
                         ('30.0.16248370', '30.0.16248370', 'android-ndk-r30', 'scripts/mobile-core/ndk-r30-linux.json'))
        for key in keys:
            for original, other in ((current, legacy), (legacy, current)):
                changed = deepcopy(original); changed['android'][key] = other['android'][key]
                with self.subTest(key=key, repository=original['repository']), self.assertRaisesRegex(RuntimeError, 'formal Android NDK differs'):
                    validate(changed)
        self.assertEqual(current['sourceCommit'], 'bfcdf96b244f63e0fe84736ee4a4edd58f6b209e')
        self.assertEqual((self.root / 'scripts/android-rust-ndk.version').read_text().strip(), '27.3.13750724')

    def test_exact_reviewed_profile_rejects_alternate_path_bytes_keys_and_types(self):
        profile = self.profile
        self.assertEqual(profile['archive'], {'url': 'https://dl.google.com/android/repository/android-ndk-r30-linux.zip',
            'bytes': 738633529, 'sha1': '5107f898313790e449e87eee2183d9a20602dee9',
            'sha256': '753611f410d002cfcd3f3dc2ef49aad532089d3180b436c060a90bf0fcb64df2', 'archiveRoot': 'android-ndk-r30'})
        self.assertEqual(profile['licenseId'], 'android-sdk-license')
        self.assertEqual(profile['licenseSha1'], '24333f8a63b6825ea9c5514f83c2829b004d1fee')
        self.assertEqual(len(self.properties), profile['sourcePropertiesBytes'])
        self.assertEqual(sha(self.properties), profile['sourcePropertiesSha256'])
        raw = self.installer.FORMAL_PROFILE.read_bytes()
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'profile.json'; path.write_bytes(raw)
            with self.assertRaisesRegex(ValueError, 'exact reviewed formal NDK profile'):
                self.installer.formal_profile(path)
            with patch.object(self.installer, 'FORMAL_PROFILE', path):
                self.assertEqual(self.installer.formal_profile(path), profile)
                for changed in [raw + b' ', raw.replace(b'738633529', b'"738633529"'),
                                raw.replace(b'738633529', b'NaN'), raw.replace(b'{', b'{"extra":1,', 1),
                                raw.replace(b'{', b'{"ndkRevision":"30.0.16248370",', 1)]:
                    path.write_bytes(changed)
                    with self.subTest(raw=changed[:30]), self.assertRaisesRegex(ValueError, 'profile bytes differ'):
                        self.installer.formal_profile(path)

    def test_license_gate_is_read_only_and_stops_before_download_or_slot_creation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); home, accepted = self.sdk(root)
            original = accepted.read_bytes()
            self.installer.accepted_formal_license(home, self.profile)
            self.assertEqual(accepted.read_bytes(), original)
            for bad in ['0' * 40 + '\n', ' ' + self.profile['licenseSha1'] + '\n', self.profile['licenseSha1'] + '\u2028']:
                accepted.write_text(bad)
                with patch.object(self.installer.subprocess, 'run') as run, self.assertRaisesRegex(ValueError, 'not already accepted'):
                    self.installer.prepare_formal(root, home, self.installer.FORMAL_PROFILE)
                run.assert_not_called(); self.assertEqual(accepted.read_text(), bad)
                self.assertFalse((root / 'polaris-mobile-ndk-30.0.16248370').exists())
            accepted.unlink()
            with self.assertRaisesRegex(ValueError, 'not already accepted'):
                self.installer.accepted_formal_license(home, self.profile)
            outside = root / 'accepted'; outside.write_bytes(original); accepted.symlink_to(outside)
            with self.assertRaisesRegex(ValueError, 'redirected'):
                self.installer.accepted_formal_license(home, self.profile)

    def test_retained_nonprivate_and_redirected_slots_are_refused_untouched(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); home, _ = self.sdk(root)
            target = root / 'polaris-mobile-ndk-30.0.16248370'; target.mkdir(mode=0o700)
            marker = target / 'keep'; marker.write_text('preserve retained slot')
            with self.assertRaisesRegex(ValueError, 'retained formal NDK slot'):
                self.installer.prepare_formal(root, home, self.installer.FORMAL_PROFILE)
            self.assertEqual(marker.read_text(), 'preserve retained slot')
            link = root / 'redirected'; link.symlink_to(root, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, 'redirected'):
                self.installer.prepare_formal(link, home, self.installer.FORMAL_PROFILE)
            root.chmod(0o755)
            with self.assertRaisesRegex(ValueError, 'current-user private'):
                self.installer.prepare_formal(root, home, self.installer.FORMAL_PROFILE)
            root.chmod(0o700)

    def test_archive_hashes_inventory_and_actual_properties_all_fail_closed(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); download = root / 'ndk.zip'
            raw, profile = self.archive(); download.write_bytes(raw)
            self.installer.verify_formal_archive(download, profile)
            for key, bad in [('bytes', len(raw) + 1), ('sha1', '0' * 40), ('sha256', '0' * 64)]:
                changed = deepcopy(profile); changed['archive'][key] = bad
                with self.subTest(key=key), self.assertRaises(ValueError):
                    self.installer.verify_formal_archive(download, changed)
            for names in [['other/file'], ['/android-ndk-r30/file'], ['android-ndk-r30/../file'],
                          ['android-ndk-r30/dir\\file'], ['android-ndk-r30/file', 'android-ndk-r30/file']]:
                raw, profile = self.archive(names); download.write_bytes(raw)
                with self.subTest(names=names), self.assertRaises(ValueError):
                    self.installer.verify_formal_archive(download, profile)
            properties = root / 'source.properties'; properties.write_bytes(self.properties)
            self.installer.verify_formal_properties(root, self.profile)
            for old, new in [(b'Pkg.Revision = 30.0.16248370', b'Pkg.Revision = 30.0.16138531-beta3'),
                             (b'Pkg.BaseRevision = 30.0.16248370', b'Pkg.BaseRevision = 30.0.0'),
                             (b'Pkg.ReleaseName = r30', b'Pkg.ReleaseName = r30-rc1')]:
                properties.write_bytes(self.properties.replace(old, new))
                with self.assertRaisesRegex(ValueError, 'source.properties bytes differ'):
                    self.installer.verify_formal_properties(root, self.profile)
            properties.unlink(); properties.symlink_to(download)
            with self.assertRaisesRegex(ValueError, 'redirected'):
                self.installer.verify_formal_properties(root, self.profile)

    def test_fresh_formal_install_is_explicit_isolated_and_preserves_legacy_slot(self):
        raw, profile = self.archive(); calls = []
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); home, accepted = self.sdk(root); original = accepted.read_bytes()
            legacy = root / 'polaris-libbox-ndk'; legacy.mkdir(); (legacy / 'keep').write_text('legacy bytes')
            def fake_run(args, **kwargs):
                calls.append(args)
                if args[0] == 'curl':
                    self.assertEqual(args[-1], profile['archive']['url']); self.assertNotIn('-L', args)
                    self.assertEqual(args[1], '--disable'); Path(args[args.index('-o') + 1]).write_bytes(raw)
                elif args[0] == 'unzip':
                    with zipfile.ZipFile(args[2]) as bundle: bundle.extractall(args[4])
                else: self.fail('unexpected external operation')
            with patch.object(self.installer, 'formal_profile', return_value=profile), \
                    patch.object(self.installer.subprocess, 'run', side_effect=fake_run):
                ndk = self.installer.prepare_formal(root, home, self.installer.FORMAL_PROFILE)
                self.assertEqual(ndk, root / 'polaris-mobile-ndk-30.0.16248370/android-ndk-r30')
                self.assertEqual((ndk / 'source.properties').read_bytes(), self.properties)
                with self.assertRaisesRegex(ValueError, 'retained formal NDK slot'):
                    self.installer.prepare_formal(root, home, self.installer.FORMAL_PROFILE)
            self.assertEqual([args[0] for args in calls], ['curl', 'unzip'])
            self.assertEqual((legacy / 'keep').read_text(), 'legacy bytes'); self.assertEqual(accepted.read_bytes(), original)
            self.assertEqual(list(ndk.parent.iterdir()), [ndk])

    def test_cli_default_stays_legacy_and_license_only_never_installs(self):
        with patch.object(sys, 'argv', ['prepare-ndk-linux.py', '/legacy/temporary']), \
                patch.object(self.installer, 'legacy_main') as legacy, patch.object(self.installer, 'prepare_formal') as formal:
            self.installer.main(); legacy.assert_called_once(); formal.assert_not_called()
        with tempfile.TemporaryDirectory() as temporary:
            home, accepted = self.sdk(Path(temporary)); before = accepted.read_bytes()
            with patch.object(sys, 'argv', ['prepare-ndk-linux.py', '--profile', str(self.installer.FORMAL_PROFILE),
                    '--sdk', str(home), '--check-license-only']), patch.object(self.installer, 'prepare_formal') as formal:
                self.installer.main(); formal.assert_not_called()
            self.assertEqual(accepted.read_bytes(), before)

    def test_formal_workflow_license_gate_precedes_all_noninteractive_package_installs(self):
        source = (self.root / '.github/workflows/android.yml').read_text()
        gate = 'python3 scripts/libbox-patches/prepare-ndk-linux.py --profile scripts/mobile-core/ndk-r30-linux.json --sdk "$ANDROID_HOME" --check-license-only'
        self.assertEqual(source.count(gate), 2)
        self.assertNotIn('--licenses', source)
        for block in source.split(gate)[1:]:
            installs = [line.strip() for line in block.splitlines()[:20] if line.strip().startswith('"$sdkmanager" ')]
            self.assertEqual(len(installs), 2)
            self.assertTrue(all(line.endswith(' </dev/null') for line in installs))
            self.assertFalse(any('yes' in line or '|| true' in line for line in installs))

    def test_hosted_prepare_checks_license_before_tool_install(self):
        import ci_android
        from fork_input import load
        value = load()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); home, _ = self.sdk(root)
            expected = [sys.executable, self.root / 'scripts/libbox-patches/prepare-ndk-linux.py',
                        '--profile', self.root / value['android']['ndkArchivePath'], '--sdk', home, '--check-license-only']
            with patch.dict(os.environ, {'ANDROID_HOME': str(home)}, clear=True), \
                    patch.object(ci_android, 'producer_code', return_value={}), patch.object(ci_android, 'location', return_value=root), \
                    patch.object(ci_android, 'run', side_effect=ValueError('unaccepted license')) as run:
                with self.assertRaisesRegex(ValueError, 'unaccepted license'): ci_android.prepare()
            run.assert_called_once_with(expected)
            self.assertFalse((root / 'tools').exists())


if __name__ == '__main__':
    unittest.main()
