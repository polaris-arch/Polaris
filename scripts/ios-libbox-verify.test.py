#!/usr/bin/env python3
"""Test pure final admission, or explicitly test a real historical framework."""
import argparse
import copy
from contextlib import ExitStack, redirect_stdout
import importlib.util
import io
import json
import os
from pathlib import Path
import plistlib
import shutil
import sys
import tempfile
from unittest import mock

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('ios_libbox', Path(__file__).with_name('ios-libbox.py'))
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)
spec = importlib.util.spec_from_file_location('ios_archive', Path(__file__).with_name('verify-ios-archive.py'))
archive_checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(archive_checker)
archive_checker.libbox = builder


def rejected(action, expected):
    try:
        action()
    except (RuntimeError, KeyError, ValueError) as error:
        if expected not in str(error):
            raise AssertionError(f'Wrong rejection: {error}; expected {expected}') from error
    else:
        raise AssertionError(f'Counterexample was accepted: {expected}')


def preflight_tests():
    """No Mac, source checkout, Framework or IPA is needed for these tests."""
    count = 0
    missing = Path('/nonexistent/polaris-ios-preflight-artifact')

    def no_side_effects(actions, expected):
        nonlocal count
        with ExitStack() as stack:
            for owner, name in [(builder, 'run'), (builder, 'source_check'),
                                (builder, 'framework_evidence'), (builder.platform, 'system'),
                                (builder.platform, 'machine'), (builder.shutil, 'which'),
                                (builder.shutil, 'copyfile'), (builder.tempfile, 'TemporaryDirectory'),
                                (Path, 'mkdir'), (Path, 'write_text'), (Path, 'write_bytes'),
                                (archive_checker.zipfile, 'ZipFile')]:
                stack.enter_context(mock.patch.object(owner, name,
                    side_effect=AssertionError('Final admission reached ' + name)))
            read_text, read_bytes = Path.read_text, Path.read_bytes

            def guarded_read(method):
                def read(path, *args, **kwargs):
                    if path == missing or missing in path.parents:
                        raise AssertionError('Final admission consumed an artifact')
                    return method(path, *args, **kwargs)
                return read

            stack.enter_context(mock.patch.object(Path, 'read_text', guarded_read(read_text)))
            stack.enter_context(mock.patch.object(Path, 'read_bytes', guarded_read(read_bytes)))
            for action in actions:
                rejected(action, expected)
                count += 1

    actions = [lambda: builder.build(None), lambda: builder.verify(missing, missing, native=None),
               builder.inputs, lambda: archive_checker.verify_archive(missing),
               lambda: builder.main(['build']), lambda: builder.main(['verify', '--framework', str(missing), '--receipt', str(missing)]),
               lambda: builder.main(['check-inputs']), lambda: archive_checker.main([str(missing)])]
    no_side_effects(actions, 'Apple final source inputs not migrated')
    manifest, version = builder.inputs(historical=True)
    shared = json.loads(builder.SHARED_MANIFEST.read_bytes())
    core = json.loads((builder.ROOT / 'src-tauri/core-manifest.json').read_bytes())
    with tempfile.TemporaryDirectory(prefix='polaris-ios-preflight-test-') as temp:
        root = Path(temp)
        declaration = root / 'source-manifest.json'
        aligned = copy.deepcopy(manifest)
        for key in ('sourceCommit', 'goVersion', 'patches', 'dependencyPatches'):
            aligned[key] = copy.deepcopy(shared[key])
        aligned['sourceBuild'] = copy.deepcopy(core['sourceBuild'])
        # Copying all nine declarations and common pins, while keeping Apple's
        # own SDK/tags/gomobile, still cannot supply an implemented adapter.
        declaration.write_text(json.dumps(aligned))
        with mock.patch.object(builder, 'MANIFEST', declaration):
            no_side_effects(actions, 'Apple provider/dependency adapter and linked-module policy are not implemented')
            try:
                builder.inputs()
            except RuntimeError as error:
                assert str(error) == ('Apple final source inputs not migrated: Apple provider/dependency '
                                      'adapter and linked-module policy are not implemented')
            else:
                raise AssertionError('Copied final declarations enabled an absent adapter')
            rejected(lambda: builder.inputs(historical=True), 'Historical six-patch source manifest differs')
            count += 2
            cases = [('sourceCommit', '0' * 40, 'sourceCommit'), ('goVersion', '0.0.0', 'goVersion'),
                     ('patches', aligned['patches'][:-1], 'ordered shared patch series'),
                     ('patches', list(reversed(aligned['patches'])), 'ordered shared patch series'),
                     ('dependencyPatches', [], 'dependency source declarations')]
            bad_patches = copy.deepcopy(aligned['patches'])
            bad_patches[-1]['sha256'] = '0' * 64
            cases.append(('patches', bad_patches, 'ordered shared patch series'))
            for field in ('module', 'upstreamCommit', 'patchSha256', 'patchedTree'):
                bad_dependencies = copy.deepcopy(aligned['dependencyPatches'])
                bad_dependencies[0][field] = 'incorrect'
                cases.append(('dependencyPatches', bad_dependencies, 'dependency source declarations'))
            bad_pins = copy.deepcopy(aligned['sourceBuild'])
            bad_pins['mainGoModSha256'] = '0' * 64
            cases.append(('sourceBuild', bad_pins, 'mainGoModSha256'))
            for key, value, expected in cases:
                bad = copy.deepcopy(aligned)
                bad[key] = value
                declaration.write_text(json.dumps(bad))
                no_side_effects([builder.inputs], expected)
        patch_copy = root / 'patches'
        shutil.copytree(builder.PATCH_DIR, patch_copy)
        patch = patch_copy / 'construction-validation.patch'
        patch.write_bytes(patch.read_bytes() + b'\n')
        with mock.patch.object(builder, 'PATCH_DIR', patch_copy):
            rejected(lambda: builder.inputs(historical=True), 'Pinned patch hash mismatch')
            count += 1

        # Unit-test producer provenance without creating or claiming a real
        # Framework. Actual framework evidence remains the separate Mac test.
        receipt = {key: copy.deepcopy(manifest[key]) for key in (
            'sourceCommit', 'frozenPatchCommit', 'sharedGoCommit', 'replacementReviewCommit',
            'patchedSourceTree', 'patches', 'buildTags', 'nonMacOSTags', 'validationCleanupContract',
            'unresolvedRuntimeEvidence', 'bindTarget', 'iosMinimumVersion')}
        receipt.update(schemaVersion=1, evidenceScope='build-only', officialTag=f'v{version}',
                       sourceManifestSha256=builder.digest(builder.MANIFEST.read_bytes()),
                       buildScriptSha256=builder.HISTORICAL_SCRIPT_SHA256,
                       buildEntrySha256=builder.HISTORICAL_ENTRY_SHA256,
                       linkerFlags=builder.LINKER_FLAGS.format(version=version),
                       sourceTests=[{'passed': True} for _ in range(5)], framework={})
        receipt['toolchain'] = {key: manifest[key] for key in (
            'goVersion', 'xcodeVersion', 'xcodeBuild', 'iosSDKVersion', 'simulatorSDKVersion')}
        for name in ('gomobile', 'gobind'):
            receipt['toolchain'][name] = dict(version=manifest['gomobileVersion'], sha256='1' * 64,
                buildInfo=f'github.com/sagernet/gomobile\t{manifest["gomobileVersion"]}\t')
        receipt_path = root / 'receipt.json'
        with mock.patch.object(builder, 'framework_evidence', return_value={}):
            receipt_path.write_text(json.dumps(receipt))
            before = receipt_path.read_bytes()
            assert builder.verify(missing, receipt_path, historical=True) == receipt
            assert receipt_path.read_bytes() == before
            count += 1
            for key in ('buildScriptSha256', 'buildEntrySha256'):
                bad = copy.deepcopy(receipt)
                bad[key] = '0' * 64
                receipt_path.write_text(json.dumps(bad))
                rejected(lambda: builder.verify(missing, receipt_path, historical=True), key)
                count += 1
            new = copy.deepcopy(receipt)
            new['evidenceScope'] = 'historical-only'
            new['buildScriptSha256'] = builder.digest(builder.SCRIPT.read_bytes())
            new['buildEntrySha256'] = builder.digest((builder.ROOT / 'scripts/build-libbox-ios.sh').read_bytes())
            receipt_path.write_text(json.dumps(new))
            assert builder.verify(missing, receipt_path, historical=True) == new
            count += 1
            new['evidenceScope'] = 'build-only'
            receipt_path.write_text(json.dumps(new))
            rejected(lambda: builder.verify(missing, receipt_path, historical=True), 'buildScriptSha256')
            count += 1
            new['evidenceScope'] = 'final'
            receipt_path.write_text(json.dumps(new))
            rejected(lambda: builder.verify(missing, receipt_path, historical=True), 'evidence scope')
            count += 1

    # Both checked-in Xcode hooks must consume the default final verifier.
    for file in ('src-tauri/gen/apple/project.yml', 'src-tauri/gen/apple/polaris.xcodeproj/project.pbxproj'):
        hook = (builder.ROOT / file).read_text()
        assert 'scripts/ios-libbox.py' in hook and ' verify' in hook and '--historical' not in hook
        count += 1

    # In-memory layout fixture tests the historical checker and its evidence
    # label; no IPA, signing, core linkage or device acceptance is produced.
    app_path = 'Payload/Polaris.app/'
    extension_path = app_path + 'PlugIns/PacketTunnel.appex/'
    app = dict(CFBundleIdentifier='com.example.polaris', PolarisAppGroup='group.com.example.polaris',
               UIDeviceFamily=[1, 2], CFBundleSupportedPlatforms=['iPhoneOS'], MinimumOSVersion='17.0',
               CFBundleExecutable='Polaris')
    extension = dict(app, CFBundleIdentifier=app['CFBundleIdentifier'] + '.PacketTunnel',
                     CFBundleExecutable='PacketTunnel',
                     NSExtension=dict(NSExtensionPointIdentifier='com.apple.networkextension.packet-tunnel'))
    files = {app_path + 'Info.plist': plistlib.dumps(app), extension_path + 'Info.plist': plistlib.dumps(extension),
             app_path + 'Polaris': b'\xcf\xfa\xed\xfe\x0c\x00\x00\x01',
             extension_path + 'PacketTunnel': b'\xcf\xfa\xed\xfe\x0c\x00\x00\x01'}
    files.update({app_path + p.name: b'' for p in (builder.ROOT / 'resources/data').glob('*.srs')})
    fake_archive = mock.MagicMock()
    fake_archive.__enter__.return_value = fake_archive
    fake_archive.namelist.return_value = list(files)
    fake_archive.read.side_effect = files.__getitem__
    fake_archive.open.side_effect = lambda name: io.BytesIO(files[name])
    with mock.patch.object(archive_checker.zipfile, 'ZipFile', return_value=fake_archive), redirect_stdout(io.StringIO()) as output:
        archive_checker.verify_archive(missing, historical=True)
    assert 'Historical structure-only' in output.getvalue() and 'final source/linkage' in output.getvalue()
    count += 1
    print(f'{count} source-admission/producer/structure unit cases passed; no Framework or App built.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--framework', type=Path, default=builder.OUTPUT / 'Libbox.xcframework')
    parser.add_argument('--receipt', type=Path, default=builder.RECEIPT)
    parser.add_argument('--source', type=Path)
    parser.add_argument('--historical', action='store_true', help='Explicitly test the real frozen six-patch framework')
    parser.add_argument('--preflight-only', action='store_true', help='Run pure admission and mocked provenance tests')
    args = parser.parse_args()
    if args.preflight_only:
        preflight_tests()
        return
    manifest, version = builder.inputs(historical=args.historical)
    source = args.source or Path.home() / f'Code/sing-box-ios-v{version}'
    builder.source_check(source, manifest, version)
    original = builder.verify(args.framework.resolve(), args.receipt.resolve(), historical=args.historical)
    count = 0
    with tempfile.TemporaryDirectory(prefix='polaris-ios-receipt-test-', dir=Path.home() / 'Code') as temp:
        root = Path(temp)
        receipt = root / 'receipt.json'
        cases = [
            ('sourceCommit', '0' * 40), ('patchedSourceTree', '0' * 40),
            ('frozenPatchCommit', '0' * 40), ('sharedGoCommit', '0' * 40),
            ('replacementReviewCommit', '0' * 40),
            ('officialTag', 'v0.0.0'), ('sourceManifestSha256', '0' * 64),
            ('buildScriptSha256', '0' * 64), ('buildEntrySha256', '0' * 64),
            ('patches', original['patches'][:-1]), ('buildTags', original['buildTags'][:-1]),
            ('nonMacOSTags', []), ('bindTarget', 'ios,macos'),
            ('iosMinimumVersion', '17.0'), ('unresolvedRuntimeEvidence', []),
            ('linkerFlags', '-X github.com/sagernet/sing-box/constant.Version=0.0.0')]
        for key, value in cases:
            bad = copy.deepcopy(original)
            bad[key] = value
            receipt.write_text(json.dumps(bad))
            rejected(lambda: builder.verify(args.framework, receipt, historical=True), key)
            count += 1
        for cleanup in ['DisposedExact', 'NoOwner']:
            bad = copy.deepcopy(original)
            bad['validationCleanupContract']['constructedBoxCleanup'] = cleanup
            receipt.write_text(json.dumps(bad))
            rejected(lambda: builder.verify(args.framework, receipt, historical=True), 'validationCleanupContract')
            count += 1
        for key in ['goVersion', 'xcodeBuild', 'iosSDKVersion', 'simulatorSDKVersion']:
            bad = copy.deepcopy(original)
            bad['toolchain'][key] = 'incorrect'
            receipt.write_text(json.dumps(bad))
            rejected(lambda: builder.verify(args.framework, receipt, historical=True), 'mismatch')
            count += 1
        bad = copy.deepcopy(original)
        bad['toolchain']['gomobile']['version'] = 'v0.1.13'
        receipt.write_text(json.dumps(bad))
        rejected(lambda: builder.verify(args.framework, receipt, historical=True), 'gomobile toolchain mismatch')
        count += 1
        bad = copy.deepcopy(original)
        bad['sourceTests'][0]['passed'] = False
        receipt.write_text(json.dumps(bad))
        rejected(lambda: builder.verify(args.framework, receipt, historical=True), 'test groups did not pass')
        count += 1
        bad = copy.deepcopy(manifest)
        bad['sourceCommit'] = '0' * 40
        rejected(lambda: builder.source_check(source, bad, version), 'Official source tag differs')
        count += 1
        # Copy only patch files to exercise hash corruption without changing
        # repository files or the framework receipt used by the main build.
        patch_copy = root / 'patches'
        shutil.copytree(builder.PATCH_DIR, patch_copy)
        patch = patch_copy / 'construction-validation.patch'
        patch.write_bytes(patch.read_bytes() + b'\n')
        saved_dir = builder.PATCH_DIR
        builder.PATCH_DIR = patch_copy
        try:
            rejected(lambda: builder.inputs(historical=True), 'Pinned patch hash mismatch')
            count += 1
        finally:
            builder.PATCH_DIR = saved_dir
        # Hard link a private copy of the real artifact. Break the specific link
        # before modifying its bytes, so the build artifact stays untouched.
        framework = root / 'Libbox.xcframework'
        shutil.copytree(args.framework, framework, symlinks=True, copy_function=os.link)
        info_path = framework / 'Info.plist'
        info = plistlib.loads(info_path.read_bytes())
        info['AvailableLibraries'][0]['SupportedPlatform'] = 'android'
        info_path.unlink()
        info_path.write_bytes(plistlib.dumps(info))
        rejected(lambda: builder.verify(framework, args.receipt, historical=True), 'Non-iOS platform')
        count += 1
        info_path.write_bytes((args.framework / 'Info.plist').read_bytes())
        header = next(framework.glob('ios-arm64/Libbox.framework/Versions/A/Headers/Libbox.objc.h'))
        text = header.read_text().replace('LibboxNewStrictCommandServer(', 'UntrustedFactory(')
        header.unlink()
        header.write_text(text)
        rejected(lambda: builder.verify(framework, args.receipt, historical=True), 'Patched Objective-C API missing')
        count += 1
    print(f'Positive historical-only real framework receipt and {count} rejection counterexamples passed.')


if __name__ == '__main__':
    main()
