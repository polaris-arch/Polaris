"""Authored formal source/target controls; no Go, SDK, bind or native tool executes."""
from copy import deepcopy
import json
from pathlib import Path
import unittest

from fork_input import CONTRACT, bind_command, tool_environment, validate, verify_source, sha


class MobileInputs(unittest.TestCase):
    def setUp(self):
        from test_source_policy import future_mobile_fixture
        self.value, _ = future_mobile_fixture()

    def test_selected_repack_source_has_real_final_tag_but_no_artifact_acceptance(self):
        value = json.loads(CONTRACT.read_bytes())
        self.assertEqual(validate(value)['sourceCommit'], 'a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae')
        self.assertEqual(value['sourceTree'], '830f7256af499e5ad3e7da9a79f0a377ca38fc77')
        self.assertEqual(value['sourceTagObject'], '916d9533ba0a6092e34bc53f63dace63e11fa85e')
        for key in ('artifactPins', 'producerReceipt'):
            self.assertIsNone(value[key])
        for key in ('AppAccepted', 'devicePassed', 'publicationEligible'):
            self.assertIs(value[key], False)
        from fork_input import linker_flags
        self.assertIn('DirectoryRetirementSourceIdentity=' + value['sourceCommit'], linker_flags(value, 'a' * 64))
        for previous in ('c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e',
                         'fe182431495711c09bdb3b074ed6aa6309c00cf8',
                         '68ffb209ded1f9337f9115741cba24f9f4c63421'):
            wrong = deepcopy(value); wrong['sourceCommit'] = previous
            with self.subTest(previous=previous), self.assertRaises(RuntimeError): validate(wrong)

    def test_public_desktop_repack_cannot_borrow_the_old_mobile_tag_or_receipt(self):
        import tempfile
        from unittest.mock import patch
        pending_path = Path(__file__).resolve().parents[1] / 'desktop-core/pending-release-selection.json'
        pending = json.loads(pending_path.read_bytes())['nextDelivery']
        selected = json.loads(CONTRACT.read_bytes())
        self.assertEqual(pending['sourceCommit'], 'a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae')
        self.assertEqual(pending['sourceTree'], '830f7256af499e5ad3e7da9a79f0a377ca38fc77')
        self.assertTrue(pending['activationEligible'])
        self.assertEqual(pending['releaseId'], 416392475)
        contract = Path(__file__).resolve().parents[1] / 'desktop-core/release-contract.json'
        self.assertEqual(pending['contractSha256'], sha(contract.read_bytes()))
        self.assertEqual(pending['tagObject'], selected['sourceTagObject'])
        self.assertEqual(pending['mobileSourceContractSha256'], sha(CONTRACT.read_bytes()))
        for field in ('goVersion', 'goDirective', 'toolchain', 'goModSha256', 'goSumSha256', 'metacubex'):
            self.assertEqual(pending['sourcePolicy'][field], selected['sourcePolicy'][field])
        future = deepcopy(selected)
        for field in ('sourceCommit', 'sourceTree'):
            future[field] = future['sourcePolicy'][field] = pending[field]
        future['sourceTagObject'] = None
        with self.assertRaisesRegex(RuntimeError, 'actual immutable source field required: sourceTagObject'):
            validate(future)
        # A well-shaped historical tag hash is still not the new source's tag.
        # Actual peel verification must reject it before module files or bind.
        future['sourceTagObject'] = 'a85a5332432a92e1de502cfc9c680206371540b8'
        with tempfile.TemporaryDirectory() as directory:
            def historical_tag(_root, *args):
                if args[0] == 'cat-file': return 'tag'
                if args[1].endswith('^{tag}'): return future['sourceTagObject']
                if args[1].endswith('^{commit}'): return 'c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e'
                self.fail('must reject the old tag peel before additional source reads')
            with patch('fork_input.git', side_effect=historical_tag):
                with self.assertRaisesRegex(RuntimeError, 'mobile source tag/object/commit/tree differs'):
                    verify_source(directory, future)
        self.assertIsNone(selected['artifactPins'])
        self.assertIsNone(selected['producerReceipt'])

    def test_final_source_keeps_reverified_module_meta_and_libbox_declaration_pins(self):
        value = validate(json.loads(CONTRACT.read_bytes()))
        for field, digest in (
            ('goModSha256', '27275444718d394dcc31040304ccea65e050c417ef8a85ea7fa6ee6a351e0915'),
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
        value = json.loads(CONTRACT.read_bytes())
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


if __name__ == '__main__':
    unittest.main()
