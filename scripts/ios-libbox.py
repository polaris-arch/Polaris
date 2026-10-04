#!/usr/bin/env python3
"""Final iOS source admission and explicit historical six-patch verification."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
PATCH_DIR = ROOT / 'scripts/libbox-ios-patches'
MANIFEST = PATCH_DIR / 'source-manifest.json'
OUTPUT = ROOT / 'src-tauri/gen/apple/Frameworks'
RECEIPT = OUTPUT / 'libbox-build-receipt.json'
SCRIPT = Path(__file__).resolve()
SHARED_MANIFEST = ROOT / 'scripts/libbox-patches/source-manifest.json'
PROVIDER = ROOT / 'scripts/core-source-provision.py'
HISTORICAL_MANIFEST_SHA256 = '1468d594ca51c4ae82823fae0a4d363453962b1c3ab0a15a8c14963d6589f942'
HISTORICAL_SCRIPT_SHA256 = '2c8dd8e6ad7a8b2c1906f3cf51f5a9787ebc077d7a04d110cd4b298a45f5657e'
HISTORICAL_ENTRY_SHA256 = '1768d8b0e0426ce5d036533fd6b85379e975dfb968739d69bdabea242923790e'
LINKER_FLAGS = ('-X github.com/sagernet/sing-box/constant.Version={version} '
                '-X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w -buildid=')


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(args, cwd=None, env=None, capture=False):
    return subprocess.run([str(a) for a in args], cwd=cwd, env=env, check=True,
                          text=True, stdout=subprocess.PIPE if capture else None).stdout


def final_preflight(manifest=None):
    """Read declarations only; no tool, repository, cache or artifact access.

    Matching declarations cannot supply the missing Apple provider integration,
    source receipt validation or linked-module policy. This gate stays closed
    until that implementation is reviewed; no manifest flag can enable it.
    """
    try:
        manifest = manifest if manifest is not None else json.loads(MANIFEST.read_bytes())
        shared = json.loads(SHARED_MANIFEST.read_bytes())
        core = json.loads((ROOT / 'src-tauri/core-manifest.json').read_bytes())
        differences = []
        for key in ('sourceCommit', 'goVersion'):
            if manifest.get(key) != shared.get(key):
                differences.append('shared ' + key + ' differs')
        series = lambda source: [(patch['file'], patch['sha256']) for patch in source.get('patches', [])]
        if series(manifest) != series(shared):
            differences.append('ordered shared patch series differs')
        dependency_fields = ('name', 'module', 'sourceURL', 'upstreamVersion', 'upstreamCommit',
                             'patchFile', 'patchSha256', 'patchedTree', 'candidateCommit')
        dependencies = lambda source: [tuple(dep.get(key) for key in dependency_fields)
                                       for dep in source.get('dependencyPatches', [])]
        if dependencies(manifest) != dependencies(shared):
            differences.append('shared dependency source declarations differ')
        spec = core.get('sourceBuild', {})
        if (digest(SHARED_MANIFEST.read_bytes()) != spec.get('sourceManifestSha256')
                or digest(PROVIDER.read_bytes()) != spec.get('provisionerSha256')):
            differences.append('shared source manifest/provider hash differs')
        apple_spec = manifest.get('sourceBuild', {})
        for key in ('sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint',
                    'moduleGraphSha256', 'patchedSourceTree', 'buildTree', 'version',
                    'dependencyModules', 'transportPins', 'graphScope',
                    'mainGoModSha256', 'mainGoSumSha256'):
            if key not in spec or apple_spec.get(key) != spec[key]:
                differences.append('Apple common source pins differ: ' + key)
        # Apple tags, gomobile, SDK, deployment and linker settings are not
        # Android source-admission credentials and are intentionally not copied.
        differences.append('Apple provider/dependency adapter and linked-module policy are not implemented')
        raise RuntimeError('Apple final source inputs not migrated: ' + '; '.join(differences))
    except (OSError, KeyError, TypeError, ValueError, AttributeError) as error:
        raise RuntimeError('Apple final source inputs not migrated: invalid source declarations') from error


def inputs(historical=False):
    if not historical:
        final_preflight()
    raw = MANIFEST.read_bytes()
    require(digest(raw) == HISTORICAL_MANIFEST_SHA256, 'Historical six-patch source manifest differs')
    manifest = json.loads(raw)
    version = json.loads((ROOT / 'src-tauri/core-manifest.json').read_text())['bundledCoreVersion']
    require(re.fullmatch('[0-9a-f]{40}', manifest['sourceCommit']), 'Invalid pinned source commit')
    for key in ['frozenPatchCommit', 'sharedGoCommit', 'replacementReviewCommit']:
        require(re.fullmatch('[0-9a-f]{40}', manifest[key]), f'Invalid provenance commit: {key}')
    require(manifest['bindTarget'] == 'ios,iossimulator', 'Build target must contain only iOS and its simulator')
    for patch in manifest['patches']:
        require(Path(patch['file']).name == patch['file'], 'Patch path must stay in libbox-ios-patches')
        require(digest((PATCH_DIR / patch['file']).read_bytes()) == patch['sha256'],
                f'Pinned patch hash mismatch: {patch["file"]}')
    require(manifest['validationCleanupContract'] == {
        'version': 'polaris-validation-v1', 'constructedBoxCleanup': 'CleanupUnknown',
        'exactCleanupEnabled': False, 'noOwnerCleanupEnabled': False}, 'Unsupported cleanup contract')
    return manifest, version


def source_check(source, manifest, version):
    tag_commit = run(['git', '-C', source, 'rev-parse', f'v{version}^{{commit}}'], capture=True).strip()
    require(tag_commit == manifest['sourceCommit'], 'Official source tag differs from pinned commit')
    # The supplied repository is an object source only; its working tree is never used.
    return tag_commit


def source_settings(checkout, manifest):
    mod = (checkout / 'go.mod').read_text()
    require(re.search(r'^go\s+' + re.escape(manifest['goVersion']) + r'\s*$', mod, re.M), 'Source Go version mismatch')
    require(re.search(r'^\s*github.com/sagernet/gomobile\s+' + re.escape(manifest['gomobileVersion']) + r'\s*$', mod, re.M), 'Source gomobile version mismatch')
    text = (checkout / 'cmd/internal/build_libbox/main.go').read_text()
    tags = []
    for key in ['sharedTags', 'darwinTags']:
        for expression in re.findall(r'(?m)^\s*' + key + r' = append\(' + key + r', ([^\n]+)\)', text):
            tags.extend(re.findall(r'"([^"\n]+)"', expression))
    require(tags == manifest['buildTags'], 'Apple feature tags differ from pinned upstream source')
    require('"-tags-not-macos=with_low_memory"' in text and manifest['nonMacOSTags'] == ['with_low_memory'], 'Apple low memory tags mismatch')
    require(f'"-iosversion={manifest["iosMinimumVersion"]}"' in text, 'Upstream iOS deployment target mismatch')


def framework_evidence(framework, manifest, version, native=False):
    info = plistlib.loads((framework / 'Info.plist').read_bytes())
    libraries = info['AvailableLibraries']
    expected = {'ios-arm64': (['arm64'], None), 'ios-arm64_x86_64-simulator': (['arm64', 'x86_64'], 'simulator')}
    require({x['LibraryIdentifier'] for x in libraries} == set(expected), 'Unexpected iOS XCFramework slice set')
    slices = {}
    for item in libraries:
        name = item['LibraryIdentifier']
        archs, variant = expected[name]
        require(item['SupportedPlatform'] == 'ios' and item.get('SupportedPlatformVariant') == variant,
                f'Non-iOS platform or wrong variant: {name}')
        require(sorted(item['SupportedArchitectures']) == sorted(archs), f'Wrong architectures: {name}')
        require(item['LibraryPath'] == 'Libbox.framework', 'Unexpected framework path')
        base = framework / name / item['LibraryPath']
        binary = base / 'Libbox'
        raw = binary.read_bytes()
        tags = [x.decode().split(',') for x in re.findall(rb'build\t-tags=([^\n\x00]+)', raw)]
        expected_tags = set(manifest['buildTags'] + manifest['nonMacOSTags'] + ['ios'] + (['iossimulator'] if variant else []))
        require(tags and all(set(x) == expected_tags for x in tags), f'Binary Apple tags mismatch: {name}')
        require(('go' + manifest['goVersion']).encode() in raw, f'Binary Go version missing: {name}')
        require(version.encode() in raw, f'Binary core version missing: {name}')
        require(raw.count(b'naive') > 10 and raw.count(b'cronet') > 100, f'naive/cronet evidence missing: {name}')
        go_archs = sorted({x.decode() for x in re.findall(rb'build\tGOARCH=([^\n\x00]+)', raw)})
        require(go_archs == sorted('amd64' if x == 'x86_64' else x for x in archs), f'Binary Go architectures mismatch: {name}')
        require(set(re.findall(rb'build\tGOOS=([^\n\x00]+)', raw)) == {b'ios'}, f'Binary GOOS differs from iOS: {name}')
        header = (base / 'Headers/Libbox.objc.h').read_text()
        api = ['LibboxNewStrictCommandServer(', 'LibboxNewTransientCommandServer(', 'LibboxCheckConfigWithResult(',
               'LibboxConfigValidationContractVersion', 'LibboxConfigValidationResult', 'bindInterfaceControl:',
               'getRequestID', 'getConfigDigest', 'getContractVersion', 'getValidation', 'getCleanup',
               'getValidationError', 'getCleanupError']
        require(all(x in header for x in api), f'Patched Objective-C API missing: {name}')
        require(not re.search(r'\bset(?:RequestID|ConfigDigest|ContractVersion|Validation|Cleanup|ValidationError|CleanupError):', header), 'Config validation result must be read-only')
        if native:
            actual_archs = run(['xcrun', 'lipo', '-archs', binary], capture=True).split()
            require(sorted(actual_archs) == sorted(archs), f'Mach-O architecture mismatch: {name}')
        slices[name] = {'architectures': archs, 'variant': variant, 'binarySha256': digest(raw),
                        'binaryBytes': len(raw), 'headerSha256': digest(header.encode()),
                        'effectiveBuildTags': sorted(expected_tags), 'goArchitectures': go_archs}
    files, links = {}, {}
    for path in sorted(framework.rglob('*')):
        relative = path.relative_to(framework).as_posix()
        if path.is_symlink():
            require(path.resolve().is_relative_to(framework.resolve()), f'Framework symlink escapes artifact: {relative}')
            links[relative] = os.readlink(path)
        elif path.is_file():
            files[relative] = {'sha256': digest(path.read_bytes()), 'bytes': path.stat().st_size}
    return {'slices': slices, 'files': files, 'symlinks': links}


def verify(framework, receipt_path, native=False, historical=False):
    manifest, version = inputs(historical=historical)
    if native is None:
        native = platform.system() == 'Darwin'
    receipt = json.loads(receipt_path.read_text())
    require(receipt['schemaVersion'] == 1 and receipt['evidenceScope'] in ('build-only', 'historical-only'),
            'Invalid historical receipt evidence scope')
    # Preserve the exact old receipt and its producer identity. Only newly
    # generated historical receipts bind this revised driver; neither is final.
    if receipt['evidenceScope'] == 'build-only':
        producer, entry = HISTORICAL_SCRIPT_SHA256, HISTORICAL_ENTRY_SHA256
    else:
        producer, entry = digest(SCRIPT.read_bytes()), digest((ROOT / 'scripts/build-libbox-ios.sh').read_bytes())
    require(receipt['buildScriptSha256'] == producer, 'Receipt mismatch: buildScriptSha256')
    require(receipt['buildEntrySha256'] == entry, 'Receipt mismatch: buildEntrySha256')
    for key, value in [('sourceCommit', manifest['sourceCommit']), ('officialTag', f'v{version}'),
                       ('frozenPatchCommit', manifest['frozenPatchCommit']),
                       ('sharedGoCommit', manifest['sharedGoCommit']),
                       ('replacementReviewCommit', manifest['replacementReviewCommit']),
                       ('patchedSourceTree', manifest['patchedSourceTree']),
                       ('sourceManifestSha256', digest(MANIFEST.read_bytes())),
                       ('patches', manifest['patches']),
                       ('buildTags', manifest['buildTags']), ('nonMacOSTags', manifest['nonMacOSTags']),
                       ('validationCleanupContract', manifest['validationCleanupContract']),
                       ('unresolvedRuntimeEvidence', manifest['unresolvedRuntimeEvidence']),
                       ('bindTarget', manifest['bindTarget']), ('iosMinimumVersion', manifest['iosMinimumVersion'])]:
        require(receipt[key] == value, f'Receipt mismatch: {key}')
    require(receipt['linkerFlags'] == LINKER_FLAGS.format(version=version), 'Receipt mismatch: linkerFlags')
    require(re.fullmatch('[0-9a-f]{40}', receipt['patchedSourceTree']), 'Invalid patched source tree receipt')
    require(receipt['toolchain']['goVersion'] == manifest['goVersion'], 'Receipt Go toolchain mismatch')
    for name in ['gomobile', 'gobind']:
        tool = receipt['toolchain'][name]
        require(tool['version'] == manifest['gomobileVersion'] and f'github.com/sagernet/gomobile\t{manifest["gomobileVersion"]}\t' in tool['buildInfo'], f'Receipt {name} toolchain mismatch')
        require(re.fullmatch('[0-9a-f]{64}', tool['sha256']), f'Invalid {name} binary receipt')
    for key in ['xcodeVersion', 'xcodeBuild', 'iosSDKVersion', 'simulatorSDKVersion']:
        require(receipt['toolchain'][key] == manifest[key], f'Receipt {key} mismatch')
    require(len(receipt['sourceTests']) == 5 and all(x['passed'] is True for x in receipt['sourceTests']), 'Required source test groups did not pass')
    actual = framework_evidence(framework, manifest, version, native)
    require(receipt['framework'] == actual, 'Framework contents differ from build receipt')
    return receipt


def build(source, offline=False, historical=False):
    manifest, version = inputs(historical=historical)
    require(platform.system() == 'Darwin', 'iOS Libbox must be built on macOS with Xcode')
    source = source or Path.home() / f'Code/sing-box-ios-v{version}'
    source = source.expanduser().resolve()
    source_check(source, manifest, version)
    host_arch = {'arm64': 'arm64', 'x86_64': 'amd64'}.get(platform.machine())
    require(host_arch is not None, 'Unsupported Mac host architecture')
    cached = Path.home() / f'go/pkg/mod/golang.org/toolchain@v0.0.1-go{manifest["goVersion"]}.darwin-{host_arch}/bin/go'
    go = Path(os.environ.get('LIBBOX_GO', str(cached if cached.is_file() else shutil.which('go') or '')))
    go_version = run([go, 'version'], capture=True).strip()
    require(f' go{manifest["goVersion"]} ' in go_version, f'Wrong Go toolchain: {go_version}')
    xcode = run(['xcodebuild', '-version'], capture=True).splitlines()
    toolchain = {'goVersion': manifest['goVersion'], 'go': go_version,
                 'xcodeVersion': xcode[0].removeprefix('Xcode '), 'xcodeBuild': xcode[1].removeprefix('Build version '),
                 'iosSDKVersion': run(['xcrun', '--sdk', 'iphoneos', '--show-sdk-version'], capture=True).strip(),
                 'simulatorSDKVersion': run(['xcrun', '--sdk', 'iphonesimulator', '--show-sdk-version'], capture=True).strip()}
    for key in ['xcodeVersion', 'xcodeBuild', 'iosSDKVersion', 'simulatorSDKVersion']:
        require(toolchain[key] == manifest[key], f'Installed {key} differs from pinned iOS toolchain')
    env = os.environ.copy()
    env.update({'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOFLAGS': '', 'GOEXPERIMENT': '',
                'GOPROXY': 'off' if offline else 'https://proxy.golang.org', 'GOSUMDB': 'sum.golang.org', 'CGO_CFLAGS': '', 'CGO_CPPFLAGS': '',
                'CGO_CXXFLAGS': '', 'CGO_LDFLAGS': ''})
    for name in ['GOOS', 'GOARCH', 'GOARM', 'GOAMD64', 'GOARM64', 'CC', 'CXX', 'CGO_ENABLED',
                 'ANDROID_HOME', 'ANDROID_NDK_HOME', 'NDK_HOME', 'JAVA_HOME']:
        env.pop(name, None)
    with tempfile.TemporaryDirectory(prefix='polaris-ios-libbox-', dir=Path.home() / 'Code') as temp:
        checkout = Path(temp)
        run(['git', 'clone', '--quiet', '--shared', '--no-checkout', source, checkout])
        run(['git', 'checkout', '--quiet', '--detach', manifest['sourceCommit']], cwd=checkout)
        source_settings(checkout, manifest)
        for patch in manifest['patches']:
            run(['git', 'apply', '--check', PATCH_DIR / patch['file']], cwd=checkout)
            run(['git', 'apply', PATCH_DIR / patch['file']], cwd=checkout)
        run(['git', 'add', '--all'], cwd=checkout)
        source_tree = run(['git', 'write-tree'], cwd=checkout, capture=True).strip()
        require(source_tree == manifest['patchedSourceTree'], 'Patched source tree differs from frozen iOS candidate')
        # Install the fork from the source module into an isolated bin directory.
        # The supplied stock core and user's existing mobile tools are never modified.
        tools_dir = checkout / '.ios-tools'
        tools_dir.mkdir()
        env['GOBIN'] = str(tools_dir)
        env['PATH'] = os.pathsep.join([str(tools_dir), str(go.parent), os.environ['PATH']])
        run([go, 'install', 'github.com/sagernet/gomobile/cmd/gomobile', 'github.com/sagernet/gomobile/cmd/gobind'], cwd=checkout, env=env)
        for name in ['gomobile', 'gobind']:
            binary = tools_dir / name
            info = run([go, 'version', '-m', binary], capture=True)
            require(f'github.com/sagernet/gomobile\t{manifest["gomobileVersion"]}\t' in info, f'Wrong {name} fork')
            require(f'go{manifest["goVersion"]}' in info.splitlines()[0], f'Wrong {name} Go compiler')
            toolchain[name] = {'version': manifest['gomobileVersion'], 'sha256': digest(binary.read_bytes()), 'buildInfo': info.strip()}
        tests = []
        # Run only tests supplied by the audited frozen patches. Exclude the two
        # API tests that bind a wildcard listener and the dialer test issuing a
        # native setsockopt on a synthetic descriptor. Remaining network fixtures
        # use loopback/temporary Unix sockets and never start a system TUN.
        excluded = {'TestCloseAfterHTTPServerServesClosesListenerOnce': 'Wildcard HTTP listener fixture',
                    'TestCloseBeforeHTTPServeStillClosesListener': 'Wildcard HTTP listener fixture',
                    'TestNamedInterfaceBindingPreservesNonAndroidAndAutomaticDialers': 'Native setsockopt with synthetic descriptor',
                    'TestUnstartedTUNValidationCannotDeleteOtherInstanceRules': 'Linux fake iptables fixture; Darwin auto_redirect construction rejects invalid argument'}
        test_names = sorted({name for patch in manifest['patches']
                             for name in re.findall(r'^\+func (Test\w+)\(', (PATCH_DIR / patch['file']).read_text(), re.M)} - set(excluded))
        test_pattern = '^(' + '|'.join(test_names) + ')$'
        def test(arguments):
            command = [go, 'test', '-race', '-ldflags=-checklinkname=0', '-count=1', '-run', test_pattern, *arguments]
            run(command, cwd=checkout, env=env)
            tests.append({'arguments': [str(x) for x in arguments], 'testPattern': test_pattern, 'passed': True})
        test(['.', './daemon', './adapter/endpoint', './adapter/inbound', './adapter/outbound',
              './adapter/service', './adapter/certificate', './common/construction', './common/certificate',
              './log', './route', './dns', './service/api', './service/ssmapi', './service/ccm', './service/ocm',
              './experimental/clashmode', './common/dialer'])
        files = run([go, 'list', '-f', '{{range .GoFiles}}{{$.Dir}}/{{.}} {{end}}', './experimental/libbox'], cwd=checkout, env=env, capture=True).split()
        test([*files, *[checkout / 'experimental/libbox' / name for name in [
            'command_server_transient_test.go', 'interface_binding_test.go', 'config_validation_test.go',
            'config_construction_persistence_test.go']]])
        for tags in ['with_ccm', 'with_ocm', 'with_ccm,with_ocm']:
            files = run([go, 'list', '-tags', tags, '-f', '{{range .GoFiles}}{{$.Dir}}/{{.}} {{end}}', './experimental/libbox'], cwd=checkout, env=env, capture=True).split()
            names = ['config_validation_test.go', 'config_construction_persistence_test.go']
            if tags == 'with_ccm,with_ocm':
                names.append('config_optional_persistence_test.go')
            test(['-tags', tags, *files, *[checkout / 'experimental/libbox' / name for name in names]])
        require(not run(['git', 'diff', '--name-only'], cwd=checkout, capture=True).strip(), 'Go tooling changed pinned source or module locks')
        flags = LINKER_FLAGS.format(version=version)
        command = [tools_dir / 'gomobile', 'bind', '-v', '-o', checkout / 'Libbox.xcframework',
                   '-target', manifest['bindTarget'], '-libname=box',
                   '-tags-not-macos=' + ','.join(manifest['nonMacOSTags']),
                   '-iosversion=' + manifest['iosMinimumVersion'], '-trimpath', '-buildvcs=false',
                   '-ldflags', flags, '-tags', ','.join(manifest['buildTags']), './experimental/libbox']
        print(f'Building historical-only iOS Libbox {version} from {manifest["sourceCommit"]}', flush=True)
        run(command, cwd=checkout, env=env)
        framework = checkout / 'Libbox.xcframework'
        receipt = {'schemaVersion': 1, 'evidenceScope': 'historical-only',
                   'frozenPatchCommit': manifest['frozenPatchCommit'],
                   'sharedGoCommit': manifest['sharedGoCommit'],
                   'replacementReviewCommit': manifest['replacementReviewCommit'],
                   'sourceCommit': manifest['sourceCommit'], 'officialTag': f'v{version}', 'patchedSourceTree': source_tree,
                   'sourceManifestSha256': digest(MANIFEST.read_bytes()), 'buildScriptSha256': digest(SCRIPT.read_bytes()),
                   'buildEntrySha256': digest((ROOT / 'scripts/build-libbox-ios.sh').read_bytes()),
                   'patches': manifest['patches'], 'toolchain': toolchain, 'buildTags': manifest['buildTags'],
                   'nonMacOSTags': manifest['nonMacOSTags'], 'bindTarget': manifest['bindTarget'],
                   'iosMinimumVersion': manifest['iosMinimumVersion'], 'linkerFlags': flags,
                   'sourceTests': tests, 'excludedSourceTests': excluded, 'validationCleanupContract': manifest['validationCleanupContract'],
                   'unresolvedRuntimeEvidence': manifest['unresolvedRuntimeEvidence'],
                   'framework': framework_evidence(framework, manifest, version, native=True)}
        pending_receipt = checkout / 'libbox-build-receipt.json'
        pending_receipt.write_text(json.dumps(receipt, indent=2) + '\n')
        verify(framework, pending_receipt, native=True, historical=True)
        OUTPUT.mkdir(parents=True, exist_ok=True)
        # Copy only after all build, API, tags, and receipt checks pass.
        run(['rsync', '-a', '--delete', str(framework) + '/', str(OUTPUT / 'Libbox.xcframework') + '/'])
        shutil.copyfile(pending_receipt, RECEIPT)
        verify(OUTPUT / 'Libbox.xcframework', RECEIPT, native=True, historical=True)
        print(f'Verified historical-only iOS XCFramework and build receipt: {RECEIPT}', flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    builder = sub.add_parser('build')
    builder.add_argument('source', nargs='?', type=Path)
    builder.add_argument('--offline', action='store_true', help='Require all locked modules to be cached')
    checker = sub.add_parser('verify')
    checker.add_argument('--framework', type=Path, default=OUTPUT / 'Libbox.xcframework')
    checker.add_argument('--receipt', type=Path, default=RECEIPT)
    input_checker = sub.add_parser('check-inputs')
    for command in (builder, checker, input_checker):
        command.add_argument('--historical', action='store_true',
                             help='Explicitly use the frozen six-patch history; never final source evidence')
    args = parser.parse_args(argv)
    if args.action == 'build':
        build(args.source, args.offline, historical=args.historical)
    elif args.action == 'verify':
        verify(args.framework, args.receipt, native=None, historical=args.historical)
        print('Verified historical-only iOS build receipt; runtime cleanup remains unknown.')
    else:
        inputs(historical=args.historical)
        print('Verified historical-only six-patch hashes and source manifest.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        print(f'iOS Libbox failed: {error}', file=sys.stderr)
        sys.exit(1)
