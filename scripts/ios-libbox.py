#!/usr/bin/env python3
"""Apple shared-source admission, carrier inspection and explicit six-patch history."""
import argparse
import hashlib
import importlib.util
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

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent.parent
PATCH_DIR = ROOT / 'scripts/libbox-ios-patches'
MANIFEST = PATCH_DIR / 'source-manifest.json'
OUTPUT = ROOT / 'src-tauri/gen/apple/Frameworks'
RECEIPT = OUTPUT / 'libbox-build-receipt.json'
SCRIPT = Path(__file__).resolve()
SHARED_MANIFEST = ROOT / 'scripts/libbox-patches/source-manifest.json'
PROVIDER = ROOT / 'scripts/core-source-provision.py'
SOURCE_HELPERS = ROOT / 'scripts/libbox-patches/android-source.py'
CORE_MANIFEST = ROOT / 'src-tauri/core-manifest.json'
APPLE_POLICY = PATCH_DIR / 'apple-source-policy.json'
COMMON_FIELDS = ('sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint',
                 'moduleGraphSha256', 'patchedSourceTree', 'buildTree', 'version',
                 'dependencyModules', 'transportPins', 'graphScope',
                 'mainGoModSha256', 'mainGoSumSha256')
ARTIFACT_REQUIREMENTS = ('Apple SDK/compiler identities', 'per-target linked module partitions',
                         'generated Objective-C/Swift ABI', 'Framework carrier source BuildID',
                         'final appex static-link provenance')
APPLE_TARGETS = {
    'ios-arm64': {'goos': 'ios', 'goarch': 'arm64', 'cgo': '1', 'sdk': 'iphoneos', 'variant': None},
    'ios-arm64-simulator': {'goos': 'ios', 'goarch': 'arm64', 'cgo': '1', 'sdk': 'iphonesimulator', 'variant': 'simulator'},
    'ios-x86_64-simulator': {'goos': 'ios', 'goarch': 'amd64', 'cgo': '1', 'sdk': 'iphonesimulator', 'variant': 'simulator'},
}
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


def source_helpers():
    """Reuse existing platform-neutral validators without Android tool/admit calls."""
    specification = importlib.util.spec_from_file_location('apple_source_contract', SOURCE_HELPERS)
    helper = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(helper)
    return helper


def final_preflight():
    """Admit source declarations only; no tools, repositories, caches or artifacts."""
    try:
        shared = json.loads(SHARED_MANIFEST.read_bytes())
        core = json.loads(CORE_MANIFEST.read_bytes())
        policy = json.loads(APPLE_POLICY.read_bytes())
        helper = source_helpers()
        spec = core['sourceBuild']
        require(all(helper.match('[0-9a-f]{64}', spec.get(key)) for key in
                    ('sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint',
                     'moduleGraphSha256', 'mainGoModSha256', 'mainGoSumSha256'))
                and all(helper.match('[0-9a-f]{40}', spec.get(key)) for key in ('patchedSourceTree', 'buildTree')),
                'complete common source pins required')
        require(helper.match(re.escape(core['bundledCoreVersion']) + r'\.polaris\.[1-9][0-9]*', spec.get('version'))
                and spec.get('graphScope') == 'declared-patched-modules', 'common source version/scope differs')
        require(helper.file_hash(SHARED_MANIFEST) == spec['sourceManifestSha256']
                and helper.file_hash(PROVIDER) == spec['provisionerSha256'], 'shared manifest/provider hash differs')
        require(set(policy) == {'schema', 'evidenceScope', 'sourceHelperSha256', 'commonSource',
                               'binding', 'targets', 'moduleInventory', 'validationCleanupContract',
                               'unresolvedArtifactEvidence'}, 'unsupported Apple source policy fields')
        require(policy['schema'] == 'polaris-apple-source-policy-v1'
                and policy['evidenceScope'] == 'source-inputs-only', 'unsupported Apple source policy scope')
        require(policy['sourceHelperSha256'] == helper.file_hash(SOURCE_HELPERS), 'shared source helper hash differs')
        require(policy['commonSource'] == {key: spec[key] for key in COMMON_FIELDS}, 'Apple common source pins differ')
        require(helper.match('[0-9a-f]{40}', shared.get('sourceCommit'))
                and helper.match(r'\d+\.\d+\.\d+', shared.get('goVersion')), 'invalid common source/toolchain pin')
        patches, dependencies = shared['patches'], shared['dependencyPatches']
        require(isinstance(patches, list) and patches and len({p['file'] for p in patches}) == len(patches),
                'invalid ordered common patch inventory')
        require(isinstance(dependencies, list) and dependencies
                and len({d['name'] for d in dependencies}) == len(dependencies)
                and len({d['module'] for d in dependencies}) == len(dependencies), 'invalid dependency inventory')
        inventory, transport = spec['dependencyModules'], spec['transportPins']
        require(isinstance(inventory, list) and inventory and len(set(inventory)) == len(inventory)
                and sorted(d['module'] for d in dependencies) == sorted(inventory), 'common dependency inventory differs')
        require(isinstance(transport, dict) and transport and all(helper.match('[A-Za-z0-9._/-]+', module)
                and helper.match('v[0-9A-Za-z.+-]+', version) for module, version in transport.items()),
                'common transport pins differ')
        for dependency in dependencies:
            require(helper.match('[a-z0-9][a-z0-9-]*', dependency['name'])
                    and helper.match(r'v\S+', dependency['upstreamVersion'])
                    and all(helper.match('[0-9a-f]{40}', dependency[key]) for key in
                            ('upstreamCommit', 'patchedTree', 'candidateCommit'))
                    and helper.match(r'https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?', dependency['sourceURL']),
                    'invalid dependency source declaration')
        for patch in patches + dependencies:
            filename, expected = patch.get('file', patch.get('patchFile')), patch.get('sha256', patch.get('patchSha256'))
            require(helper.match(r'[a-z0-9-]+\.patch', filename) and helper.match('[0-9a-f]{64}', expected)
                    and helper.file_hash(SHARED_MANIFEST.parent / filename) == expected, 'ordered common patch hash differs')
        historical, _ = inputs(historical=True)
        binding = policy['binding']
        require(helper.canonical(binding) == helper.canonical({
                            'target': historical['bindTarget'], 'libname': 'box', 'trimpath': True,
                            'buildVCS': False, 'gomobileVersion': historical['gomobileVersion'],
                            'iosMinimumVersion': historical['iosMinimumVersion'], 'appMinimumVersion': '17.0',
                            'buildTags': historical['buildTags'], 'nonMacOSTags': historical['nonMacOSTags']}),
                'Apple binding/tags/deployment policy differs')
        require(shared['goVersion'] == historical['goVersion'], 'Apple Go declaration differs')
        require(policy['targets'] == APPLE_TARGETS, 'Apple source target policy differs')
        require(policy['moduleInventory'] == {'patched': inventory, 'transport': sorted(transport)},
                'Apple source module inventory differs')
        require(helper.canonical(policy['validationCleanupContract']) == helper.canonical(historical['validationCleanupContract']),
                'Apple cleanup contract differs')
        require(policy['unresolvedArtifactEvidence'] == list(ARTIFACT_REQUIREMENTS),
                'Apple artifact evidence requirements differ')
        return shared, core, policy
    except (OSError, KeyError, TypeError, ValueError, AttributeError) as error:
        raise RuntimeError('Apple final source inputs not migrated: invalid source declarations') from error


def artifact_preflight():
    final_preflight()
    # Source declarations do not prove production carrier/ABI or final linkage.
    raise RuntimeError('Apple final artifact evidence not implemented: ' + '; '.join(ARTIFACT_REQUIREMENTS))


def inspect_source_carrier(binary, expected_targets, source_receipt, build_policy, tools=None, scratch=None):
    """Producer/cache share C1 inspection; C2 policy and final admission remain separate."""
    shared, core, _ = final_preflight()
    spec = importlib.util.spec_from_file_location('apple_carrier_contract', SCRIPT.with_name('apple-carrier.py'))
    carrier = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(carrier)
    return carrier.inspect_carrier(binary, expected_targets, source_receipt, shared, core,
                                   build_policy, tools, scratch)


def provision_source(source, checkout, module_source, go):
    """Prepare exactly the shared graph, without binding or publishing a Framework."""
    shared, core, _ = final_preflight()
    helper = source_helpers()
    repositories = helper.module_sources(module_source, shared)
    require(go is not None and Path(go).is_file(), 'pinned Go executable required')
    provider = helper.provider()
    receipt = provider.provision(SHARED_MANIFEST, source, checkout, repositories, go)
    helper.validate_source_receipt(receipt, shared, core)
    require(all(receipt.get(key) == core['sourceBuild'][key] for key in ('graphScope', 'mainGoModSha256', 'mainGoSumSha256')),
            'shared receipt main module/scope binding differs')
    helper.verify_checkout(checkout, receipt, provider)
    return receipt


def observe_source(checkout, receipt_path, go, mobile, developer, evidence):
    """Explicit C2 source window only; default/cache artifact admission stays closed."""
    final_preflight()
    raw = json.loads(Path(receipt_path).read_bytes())
    require(isinstance(raw, dict), 'source receipt object required')
    receipt = raw.get('sourceReceipt', raw)
    specification = importlib.util.spec_from_file_location('apple_source_inputs', SCRIPT.with_name('apple-source-inputs.py'))
    collector = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(collector)
    return collector.collect(final_preflight, source_helpers, checkout, receipt, go, mobile, developer, evidence)



def component_module():
    specification = importlib.util.spec_from_file_location('apple_component', SCRIPT.with_name('apple-component.py'))
    component = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(component)
    return component


def observe_component(checkout, receipt_path, go, mobile, developer, evidence):
    final_preflight()
    raw = component_module().read_json(receipt_path)
    return component_module().observe_component(checkout, raw.get('sourceReceipt', raw), go, mobile, developer, evidence)


def approved_component_json(component, path, expected, name):
    """Use the caller's independent raw hash; never infer approval from a receipt."""
    require(isinstance(path, Path) and path.is_absolute() and path.is_file() and not path.is_symlink(),
            'approved ' + name + ' file required')
    require(isinstance(expected, str) and re.fullmatch('[0-9a-f]{64}', expected),
            'approved ' + name + ' SHA256 required')
    raw = path.read_bytes()
    require(digest(raw) == expected, 'approved ' + name + ' SHA256 differs')
    rows = component.inputs.json_stream(raw.decode('utf-8'))
    require(len(rows) == 1 and isinstance(rows[0], dict), 'one approved ' + name + ' object required')
    return rows[0]


def approved_component_inputs(component, args):
    return (approved_component_json(component, args.build_policy, args.build_policy_sha256, 'build policy'),
            approved_component_json(component, args.tools, args.tools_sha256, 'tools'))


def build_component(args):
    require(not args.historical and args.source is None and not args.offline,
            'component build cannot use historical/source/offline options')
    require(args.phase in ('assemble', 'publish'), 'explicit component assemble/publish phase required')
    final_preflight()
    component = component_module()
    policy, tools = approved_component_inputs(component, args)
    require(args.source_receipt is not None and args.apple_input is not None, 'original source receipt/Apple input required')
    raw = component.read_json(args.source_receipt)
    require(isinstance(raw, dict), 'original source receipt object required')
    source_receipt, apple_input = raw.get('sourceReceipt', raw), component.read_json(args.apple_input)
    if args.phase == 'assemble':
        require(args.evidence_dir is not None and args.staged_generation is None and args.output_root is None
                and args.output_fingerprint is None and not any((args.previous_build_policy, args.previous_build_policy_sha256,
                                                               args.previous_tools, args.previous_tools_sha256)),
                'assemble requires fresh evidence only; publish inputs forbidden')
        # First assembly is retained for independent inventory/command approval.
        # It is never repeated to manufacture a receipt with different scratch argv.
        require(policy.get('componentInventory') == [] and policy.get('assembly') == {},
                'first assembly requires pending inventory/assembly policy')
        return component.assemble_component(apple_input, source_receipt, policy, tools, args.evidence_dir)
    require(args.evidence_dir is None and isinstance(args.staged_generation, Path)
            and args.staged_generation.is_absolute() and args.staged_generation.is_dir()
            and not args.staged_generation.is_symlink() and args.staged_generation.resolve(strict=True) == args.staged_generation
            and args.output_root is not None and isinstance(args.output_fingerprint, str)
            and re.fullmatch('[0-9a-f]{64}', args.output_fingerprint), 'retained stage/output/fingerprint required')
    previous = (args.previous_build_policy, args.previous_build_policy_sha256, args.previous_tools, args.previous_tools_sha256)
    require(not any(previous) or all(previous), 'complete approved previous policy/tools required')
    previous_policy = approved_component_json(component, previous[0], previous[1], 'previous build policy') if all(previous) else None
    previous_tools = approved_component_json(component, previous[2], previous[3], 'previous tools') if all(previous) else None
    receipt = component.make_receipt(args.staged_generation / 'Libbox.xcframework', source_receipt, apple_input, policy, tools)
    require(receipt['outputFingerprint'] == args.output_fingerprint, 'approved component output fingerprint differs')
    return component.publish_component(args.staged_generation, receipt, args.output_root, build_policy=policy, tools=tools,
                                       previous_policy=previous_policy, previous_tools=previous_tools)


def component_for_link(args, resolve=False):
    """Pin an approved immutable component only; final App/appex admission stays closed."""
    require(not getattr(args, 'historical', False), 'historical is not component-for-link evidence')
    require(isinstance(args.output_fingerprint, str) and re.fullmatch('[0-9a-f]{64}', args.output_fingerprint)
            and isinstance(args.receipt_sha256, str) and re.fullmatch('[0-9a-f]{64}', args.receipt_sha256),
            'approved component output fingerprint/receipt SHA256 required')
    final_preflight()
    component = component_module()
    policy, tools = approved_component_inputs(component, args)
    if resolve:
        # The original resolver reads current exactly once and returns a fixed generation.
        result = component.resolve_component(args.output_root, policy, tools)
    else:
        require(args.framework is None and args.receipt is None, 'component-for-link consumes only its fixed generation pair')
        generation = args.generation_root
        require(isinstance(generation, Path) and generation.is_absolute() and generation.parent.name == '.libbox-generations'
                and generation.name == args.output_fingerprint, 'approved immutable generation required')
        root, _ = component.safe_output_root(generation.parent.parent)
        selected = component.selected_generation(root, '.libbox-generations/' + generation.name)
        require(selected == generation, 'approved generation path differs')
        require(digest((selected / 'libbox-build-receipt.json').read_bytes()) == args.receipt_sha256,
                'approved component receipt SHA256 differs')
        result = component.snapshot(selected, policy, tools)
    require(result['componentOutputFingerprint'] == args.output_fingerprint
            and digest(Path(result['receiptPath']).read_bytes()) == args.receipt_sha256,
            'approved component output fingerprint/receipt SHA256 differs')
    return result


def inputs(historical=False):
    if not historical:
        artifact_preflight()
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
    builder.add_argument('--component', action='store_true', help='Explicit M1 assemble/publish; no final App admission')
    builder.add_argument('--phase', choices=('assemble', 'publish'))
    for flag in ('source-receipt', 'apple-input', 'evidence-dir', 'staged-generation', 'output-root',
                 'previous-build-policy', 'previous-tools'):
        builder.add_argument('--' + flag, type=Path)
    for flag in ('previous-build-policy-sha256', 'previous-tools-sha256'):
        builder.add_argument('--' + flag)
    checker = sub.add_parser('verify')
    checker.add_argument('--framework', type=Path)
    checker.add_argument('--receipt', type=Path)
    checker.add_argument('--component-for-link', action='store_true', help='Strict fixed component prerequisite; no final App/appex admission')
    checker.add_argument('--generation-root', type=Path)
    checker.add_argument('--receipt-sha256')
    for command in (builder, checker):
        for flag in ('build-policy', 'tools'):
            command.add_argument('--' + flag, type=Path)
        for flag in ('build-policy-sha256', 'tools-sha256', 'output-fingerprint'):
            command.add_argument('--' + flag)
    input_checker = sub.add_parser('check-inputs')
    preparer = sub.add_parser('prepare-source', help='Prepare shared source only; no Framework or App build')
    preparer.add_argument('source', type=Path)
    preparer.add_argument('--checkout', type=Path, required=True)
    preparer.add_argument('--module-source', action='append', default=[], metavar='MODULE=REPOSITORY')
    preparer.add_argument('--go', type=Path, required=True)
    observer = sub.add_parser('observe-source', help='Explicit C2 source compiler window; no Framework/App or carrier admission')
    observer.add_argument('checkout', type=Path)
    observer.add_argument('--source-receipt', type=Path, required=True)
    observer.add_argument('--go', type=Path, required=True)
    observer.add_argument('--mobile-bin', type=Path, required=True)
    observer.add_argument('--developer-dir', type=Path, required=True)
    observer.add_argument('--evidence-dir', type=Path, required=True)
    component_observer = sub.add_parser('observe-component', help='Explicit C3 archive compiler observations; no Framework/publish/App')
    component_observer.add_argument('checkout', type=Path)
    for flag in ('source-receipt', 'go', 'mobile-bin', 'developer-dir', 'evidence-dir'):
        component_observer.add_argument('--' + flag, type=Path, required=True)
    for action in ('verify-component', 'resolve-component'):
        command = sub.add_parser(action, help='Strict independently frozen component predicate; no final App admission')
        command.add_argument('--build-policy', type=Path, required=True)
        command.add_argument('--tools', type=Path, required=True)
        if action == 'verify-component':
            command.add_argument('--framework', type=Path, required=True)
            command.add_argument('--receipt', type=Path, required=True)
        else:
            command.add_argument('--output-root', type=Path, required=True)
            for flag in ('build-policy-sha256', 'tools-sha256', 'output-fingerprint', 'receipt-sha256'):
                command.add_argument('--' + flag, required=True)
    for command in (builder, checker, input_checker):
        command.add_argument('--historical', action='store_true',
                             help='Explicitly use the frozen six-patch history; never final source evidence')
    args = parser.parse_args(argv)
    if args.action == 'observe-component':
        observation = observe_component(args.checkout, args.source_receipt, args.go, args.mobile_bin, args.developer_dir, args.evidence_dir)
        print(json.dumps({'evidenceScope': observation['evidenceScope'], 'status': observation['status'],
                          'evidenceDirectory': str(args.evidence_dir), 'pending': observation['pending'], 'carrierAdmission': False}, sort_keys=True))
    elif args.action == 'verify-component':
        final_preflight()
        component = component_module()
        policy, tools = component.read_json(args.build_policy), component.read_json(args.tools)
        receipt = component.read_json(args.receipt)
        result = component.verify_component(args.framework, receipt, receipt['appleInput'], policy, tools)
        print(json.dumps({'evidenceScope': result['evidenceScope'], 'outputFingerprint': result['outputFingerprint']}, sort_keys=True))
    elif args.action == 'resolve-component':
        print(json.dumps(component_for_link(args, resolve=True), sort_keys=True))
    elif args.action == 'observe-source':
        observation = observe_source(args.checkout, args.source_receipt, args.go, args.mobile_bin,
                                     args.developer_dir, args.evidence_dir)
        print(json.dumps({'evidenceScope': observation['evidenceScope'], 'status': observation['status'],
                          'evidenceDirectory': str(args.evidence_dir), 'C3Required': observation['C3Required'],
                          'carrierAdmission': False}, sort_keys=True))
    elif args.action == 'prepare-source':
        receipt = provision_source(args.source, args.checkout, args.module_source, args.go)
        print(json.dumps({'evidenceScope': 'source-only', 'sourceReceipt': receipt,
                          'unresolvedArtifactEvidence': list(ARTIFACT_REQUIREMENTS)}, sort_keys=True))
    elif args.action == 'build':
        if args.component:
            print(json.dumps(build_component(args), sort_keys=True))
        else:
            require(not any((args.phase, args.source_receipt, args.apple_input, args.evidence_dir, args.staged_generation,
                             args.output_root, args.build_policy, args.build_policy_sha256, args.tools, args.tools_sha256,
                             args.output_fingerprint, args.previous_build_policy, args.previous_build_policy_sha256,
                             args.previous_tools, args.previous_tools_sha256)), 'component build requires explicit --component')
            build(args.source, args.offline, historical=args.historical)
    elif args.action == 'verify':
        if args.component_for_link:
            result = component_for_link(args)
            print(json.dumps({'evidenceScope': 'component-for-link-only', 'generationRoot': result['generationRoot'],
                              'componentOutputFingerprint': result['componentOutputFingerprint'], 'finalArtifactAdmission': False}, sort_keys=True))
        else:
            require(not any((args.generation_root, args.receipt_sha256, args.build_policy, args.build_policy_sha256,
                             args.tools, args.tools_sha256, args.output_fingerprint)), 'component verification requires explicit --component-for-link')
            verify(args.framework or OUTPUT / 'Libbox.xcframework', args.receipt or RECEIPT, native=None, historical=args.historical)
            print('Verified historical-only iOS build receipt; runtime cleanup remains unknown.')
    else:
        if args.historical:
            inputs(historical=True)
            print('Verified historical-only six-patch hashes and source manifest.')
        else:
            final_preflight()
            print('Apple shared source inputs admitted (source-inputs-only); Framework/App linkage and runtime remain unverified.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, KeyError, ValueError, subprocess.CalledProcessError) as error:
        print(f'iOS Libbox failed: {error}', file=sys.stderr)
        sys.exit(1)
