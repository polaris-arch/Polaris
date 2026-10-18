#!/usr/bin/env python3
"""Android admission and identities; source replay belongs to the shared provider."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

PATCH_DIR = Path(__file__).resolve().parent
ROOT = PATCH_DIR.parent.parent
PROVIDER = ROOT / 'scripts/core-source-provision.py'
ABIS = {'arm64-v8a': ('arm64', 2, 183, {'GOARM64': 'v8.0'}),
        'armeabi-v7a': ('arm', 1, 40, {'GOARM': '7'}),
        'x86': ('386', 1, 3, {'GO386': 'sse2'}),
        'x86_64': ('amd64', 2, 62, {'GOAMD64': 'v1'})}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True).encode()


def digest(data):
    return hashlib.sha256(data).hexdigest()


def file_hash(path):
    value = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def match(pattern, value):
    return isinstance(value, str) and re.fullmatch(pattern, value) is not None


def partition(value, inventory, absent):
    require(isinstance(value, dict) and set(value) == {'requiredLinked', absent}, 'Android module partition is not frozen')
    required, omitted = value['requiredLinked'], value[absent]
    require(isinstance(required, list) and isinstance(omitted, list)
            and all(isinstance(item, str) for item in required + omitted)
            and len(set(required + omitted)) == len(required + omitted)
            and sorted(required + omitted) == sorted(inventory), 'Android module partition inventory differs')


def admit():
    """Pure config admission precedes repository, tool, checkout and cache access."""
    source_path = PATCH_DIR / 'source-manifest.json'
    source = json.loads(source_path.read_bytes())
    core = json.loads((ROOT / 'src-tauri/core-manifest.json').read_bytes())
    policy_path = PATCH_DIR / 'android-source-policy.json'
    policy = json.loads(policy_path.read_bytes())
    spec = core.get('sourceBuild')
    require(isinstance(spec, dict) and all(match('[0-9a-f]{64}', spec.get(field)) for field in
            ('sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint', 'moduleGraphSha256'))
            and all(match('[0-9a-f]{40}', spec.get(field)) for field in ('patchedSourceTree', 'buildTree')),
            'Android shared source graph is not frozen: complete common pins required')
    require(match(re.escape(core['bundledCoreVersion']) + r'\.polaris\.[1-9][0-9]*', spec.get('version')),
            'Patched Android version is not frozen')
    inventory = spec.get('dependencyModules')
    transport = spec.get('transportPins')
    require(isinstance(inventory, list) and len(set(inventory)) == len(inventory)
            and all(match('[A-Za-z0-9._/-]+', item) for item in inventory), 'Patched inventory is not frozen')
    require(isinstance(transport, dict) and transport and all(match('[A-Za-z0-9._/-]+', module)
            and match('v[0-9A-Za-z.+-]+', version) for module, version in transport.items()), 'Transport pins are not frozen')
    require(set(spec.get('platforms', {})) == {'linux', 'win', 'mac-x64', 'mac-arm64'}, 'Desktop target inventory changed')
    dependencies = source.get('dependencyPatches')
    require(isinstance(dependencies, list), 'Patched dependency graph is not frozen')
    require(sorted(item['module'] for item in dependencies) == sorted(inventory)
            and len({item['name'] for item in dependencies}) == len(dependencies), 'Dependency inventory differs')
    for dep in dependencies:
        require(match('[a-z0-9][a-z0-9-]*', dep.get('name'))
                and match('v\\S+', dep.get('upstreamVersion'))
                and match('[0-9a-f]{40}', dep.get('upstreamCommit'))
                and match('[0-9a-f]{40}', dep.get('patchedTree'))
                and match(r'https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:\.git)?', dep.get('sourceURL')),
                'Invalid dependency source declaration')
    require(match('[0-9a-f]{40}', source.get('sourceCommit')) and match(r'\d+\.\d+\.\d+', source.get('goVersion')), 'Invalid source/toolchain pin')
    require(isinstance(source.get('patches'), list) and source['patches']
            and len({patch['file'] for patch in source['patches']}) == len(source['patches']), 'Invalid ordered core patches')
    require(file_hash(source_path) == spec['sourceManifestSha256'] and file_hash(PROVIDER) == spec['provisionerSha256'], 'Shared manifest/provider hash differs')
    for patch in source['patches'] + dependencies:
        filename = patch.get('file', patch.get('patchFile'))
        expected = patch.get('sha256', patch.get('patchSha256'))
        require(match('[a-z0-9-]+\\.patch', filename) and match('[0-9a-f]{64}', expected)
                and file_hash(PATCH_DIR / filename) == expected, 'Ordered patch hash differs')
    require(policy.get('schema') == 'polaris-android-source-policy-v1' and isinstance(policy.get('requiredPatchedModules'), list)
            and len(set(policy['requiredPatchedModules'])) == len(policy['requiredPatchedModules'])
            and set(policy['requiredPatchedModules']) == set(inventory), 'Unsupported Android source policy')
    sdk = policy.get('sdkBootclasspath', {})
    require(match('android-[1-9][0-9]*', sdk.get('platform')) and match('[0-9a-f]{64}', sdk.get('androidJarSha256')), 'SDK bootclasspath pin missing')
    require(sorted(source['abis']) == sorted(ABIS) and len(source['abis']) == 4 and set(policy.get('abis', {})) == set(ABIS), 'Four Android ABIs required')
    for abi, (arch, elf_class, machine, architecture) in ABIS.items():
        target = policy['abis'][abi]
        require({field: target.get(field) for field in ('goos', 'goarch', 'cgo', 'architecture', 'elfClass', 'elfMachine')}
                == dict(goos='android', goarch=arch, cgo='1', architecture=architecture, elfClass=elf_class, elfMachine=machine), 'Android ABI policy differs')
        partition(target.get('patchedModules'), inventory, 'allowedAbsent')
        partition(target.get('transportModules'), list(transport), 'confirmedAbsent')
        require(not target['patchedModules']['allowedAbsent'], 'Android patched modules must link in every ABI')
    return source, core, policy


def provider():
    module_spec = importlib.util.spec_from_file_location('polaris_shared_source', PROVIDER)
    module = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(module)
    return module


def validate_source_receipt(receipt, source, core):
    spec = core['sourceBuild']
    scope = ('dependencies-patched', 'declared-patched-modules') if source['dependencyPatches'] else ('source-only', 'core-source-only')
    require(receipt.get('schema') == 'polaris-core-source-v1'
            and (receipt.get('sourceGraphState'), receipt.get('graphScope')) == scope, 'Shared receipt scope differs')
    queries = sorted(dep['module'] for dep in source['dependencyPatches'])
    require(receipt.get('moduleGraphQueries') == queries and sorted(item['Path'] for item in receipt.get('moduleGraph', [])) == queries,
            'Shared receipt declared queries differ')
    require(all(match('[0-9a-f]{64}', receipt.get(field)) for field in ('mainGoModSha256', 'mainGoSumSha256')), 'Complete main module hashes missing')
    facts = {key: value for key, value in receipt.items() if key != 'fingerprint'}
    require(digest(canonical(facts)) == receipt.get('fingerprint') == spec['sourceReceiptFingerprint'], 'Shared receipt fingerprint differs')
    for field in ('sourceManifestSha256', 'provisionerSha256', 'patchedSourceTree', 'buildTree', 'moduleGraphSha256'):
        require(receipt.get(field) == spec[field], 'Shared receipt binding differs: ' + field)
    require(receipt.get('sourceCommit') == source['sourceCommit'] and receipt.get('sourceURL') == source.get('sourceURL', 'https://github.com/SagerNet/sing-box')
            and match('[0-9a-f]{40}', receipt.get('upstreamTree')) and receipt.get('patches') == source['patches'], 'Shared source declaration differs')
    require(digest(canonical(receipt['moduleGraph'])) == spec['moduleGraphSha256'], 'Shared module graph hash differs')
    require(len(receipt.get('dependencies', [])) == len(source['dependencyPatches']), 'Shared dependency inventory differs')
    for dep, actual in zip(source['dependencyPatches'], receipt['dependencies']):
        require(all(actual.get(key) == value for key, value in dep.items()) and match('[0-9a-f]{40}', actual.get('upstreamTree'))
                and actual.get('replacement') == './polaris-dependencies/' + dep['name'], 'Shared dependency provenance differs')
        rows = [row for row in receipt['moduleGraph'] if row['Path'] == dep['module']]
        require(len(rows) == 1 and rows[0].get('Version') == dep['upstreamVersion']
                and rows[0].get('Replace', {}).get('Path') == actual['replacement'], 'Shared module replacement differs')


def module_sources(values, source):
    repositories = {}
    for value in values:
        module, separator, path = value.partition('=')
        require(separator and module not in repositories and path, 'Invalid/duplicate explicit module repository')
        repository = Path(path).expanduser().resolve()
        require(repository.is_dir() and (repository / '.git').exists(), 'Explicit module repository missing')
        require(not any(part in ('pkg/mod', 'polaris-dependencies') for part in repository.parts)
                and not str(repository).startswith(str(Path.home() / 'go/pkg/mod') + '/'), 'Module cache is not an upstream repository')
        repositories[module] = repository
    require(set(repositories) == {dep['module'] for dep in source['dependencyPatches']}, 'Explicit module repository map must exactly cover declarations')
    return repositories


def verify_checkout(checkout, receipt, shared):
    shared.verify_checkout(checkout, receipt)
    require(json.loads((checkout / '.polaris-source-receipt.json').read_bytes()) == receipt, 'Checkout source receipt differs')
    ignored = shared.run(['git', 'ls-files', '--others', '--ignored', '--exclude-standard'], cwd=checkout).splitlines()
    require(not any(Path(path).suffix in {'.go', '.c', '.cc', '.cpp', '.h', '.s', '.S', '.syso'}
                    or Path(path).name in {'go.mod', 'go.sum', 'go.work', 'go.work.sum'}
                    or 'vendor' in Path(path).parts for path in ignored), 'Ignored compiler input present (generated bind sources must be cleaned)')


def parse_build_info(raw):
    lines = raw.rstrip().splitlines()
    version = re.search(r': go(\d+\.\d+\.\d+)$', lines.pop(0) if lines else '')
    require(version, 'Missing Go BuildInfo')
    facts = {'goVersion': version[1], 'modules': {}, 'settings': {}, 'path': None}
    previous = None
    for line in lines:
        if line == '\t':
            previous = None
            continue
        columns = line.split('\t')
        require(columns[0] == '' and len(columns) >= 3, 'Malformed Go BuildInfo row')
        kind, value = columns[1:3]
        if kind in ('mod', 'dep'):
            require(len(columns) >= 4 and value not in facts['modules'], 'Duplicate/malformed module row')
            previous = {'kind': kind, 'version': columns[3], **({'sum': columns[4]} if len(columns) > 4 and columns[4] else {})}
            facts['modules'][value] = previous
        elif kind == '=>':
            require(previous is not None and 'replacement' not in previous and value, 'Unbound/duplicate replacement')
            previous['replacement'] = value
            if len(columns) > 3 and columns[3]:
                previous['replacementVersion'] = columns[3]
            if len(columns) > 4 and columns[4]:
                previous['replacementSum'] = columns[4]
            previous = None
        elif kind == 'build':
            key, sep, setting = value.partition('=')
            require(sep and key and key not in facts['settings'], 'Malformed/duplicate build setting')
            facts['settings'][key] = json.loads(setting) if setting.startswith('"') else setting
            previous = None
        elif kind == 'path':
            require(facts['path'] is None, 'Duplicate BuildInfo path')
            facts['path'], previous = value, None
        else:
            raise RuntimeError('Unsupported Go BuildInfo row')
    return facts


def validate_binary(raw, abi, source, core, policy):
    facts, target = parse_build_info(raw), policy['abis'][abi]
    require(facts['goVersion'] == source['goVersion'], 'Target Go version differs')
    expected = {'GOOS': target['goos'], 'GOARCH': target['goarch'], 'CGO_ENABLED': target['cgo'],
                '-buildmode': 'c-shared', **target['architecture']}
    for field, value in expected.items():
        require(facts['settings'].get(field) == value, 'Android target BuildInfo differs: ' + field)
    require(sorted(facts['settings'].get('-tags', '').split(',')) == sorted(source['buildTags']), 'Android target tags differ')
    require(all('replacement' not in actual or module in core['sourceBuild']['dependencyModules']
                for module, actual in facts['modules'].items()), 'Undeclared target module replacement')
    for dep in source['dependencyPatches']:
        actual = facts['modules'].get(dep['module'])
        if actual is None and dep['module'] in target['patchedModules']['allowedAbsent']:
            continue
        require(actual and actual['version'] == dep['upstreamVersion'] and actual.get('replacement') == './polaris-dependencies/' + dep['name'], 'Target patched provenance differs: ' + dep['module'])
    for module, version in core['sourceBuild']['transportPins'].items():
        actual = facts['modules'].get(module)
        if module in target['transportModules']['confirmedAbsent']:
            require(actual is None, 'Confirmed absent transport appeared: ' + module)
        else:
            require(actual and actual['version'] == version, 'Target transport pin differs: ' + module)
            if module not in core['sourceBuild']['dependencyModules']:
                require('replacement' not in actual, 'Unreviewed transport replacement: ' + module)
    return facts


def directory_identity(directory):
    """Hash installed distribution bytes, including symlink targets and executable modes."""
    directory = Path(directory).resolve()
    rows = []
    for path in sorted(directory.rglob('*')):
        if path.is_file() or path.is_symlink():
            rows.append({'path': path.relative_to(directory).as_posix(), 'sha256': file_hash(path),
                         'executable': bool(path.stat().st_mode & 0o111),
                         'symlink': path.is_symlink()})
    require(rows, 'Empty tool distribution')
    return {'sha256': digest(canonical(rows)), 'files': len(rows)}


def sdk_bootclasspath(sdk, policy):
    jar = Path(sdk) / 'platforms' / policy['sdkBootclasspath']['platform'] / 'android.jar'
    require(file_hash(jar) == policy['sdkBootclasspath']['androidJarSha256'], 'Actual SDK bootclasspath SHA differs')
    return jar


def tools(source, policy, run):
    cached = Path.home() / f"go/pkg/mod/golang.org/toolchain@v0.0.1-go{source['goVersion']}.linux-amd64/bin/go"
    go = Path(os.environ.get('LIBBOX_GO', str(cached if cached.is_file() else shutil.which('go') or ''))).resolve()
    jdk = Path(os.environ.get('JDK17', str(Path.home() / '.local/jdk17'))).resolve()
    sdk = Path(os.environ.get('ANDROID_HOME', str(Path.home() / 'Android/Sdk'))).resolve()
    ndk = Path(os.environ.get('LIBBOX_NDK_HOME', str(sdk / 'ndk' / source['ndkDirectory']))).resolve()
    mobile = Path(os.environ.get('LIBBOX_MOBILE_BIN', str(Path.home() / 'go/bin'))).resolve()
    env = os.environ.copy()
    env.update({'JAVA_HOME': str(jdk), 'ANDROID_HOME': str(sdk), 'ANDROID_NDK_HOME': str(ndk), 'NDK_HOME': str(ndk),
                'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOFLAGS': '', 'GOEXPERIMENT': '', 'GOPROXY': 'off', 'GOSUMDB': 'off',
                'GOENV': 'off', 'GO111MODULE': 'on',
                'CGO_CFLAGS': '', 'CGO_CPPFLAGS': '', 'CGO_CXXFLAGS': '',
                'CGO_LDFLAGS': '-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384',
                'PATH': os.pathsep.join([str(go.parent), str(jdk / 'bin'), str(mobile), os.environ['PATH']])})
    for field in ('GOOS', 'GOARCH', 'GOARM', 'GOARM64', 'GO386', 'GOAMD64', 'CC', 'CXX', 'CGO_ENABLED', 'ANDROID_NDK_ROOT', 'GOROOT', 'GOCACHEPROG'):
        env.pop(field, None)
    # Gomobile supplies GOOS/GOARCH/CGO per ABI; explicitly freeze architecture defaults.
    for target in policy['abis'].values():
        env.update(target['architecture'])
    def output(args):
        return run([str(arg) for arg in args], env=env, capture=True).strip()
    go_version = output([go, 'version'])
    require(f" go{source['goVersion']} " in go_version, 'Pinned target Go toolchain differs')
    java, javac = output([jdk / 'bin/java', '--version']), output([jdk / 'bin/javac', '--version'])
    require(re.search(r'^openjdk ' + str(source['javaMajor']) + r'\.', java) and re.search(r'^javac ' + str(source['javaMajor']) + r'\.', javac), 'Pinned Java major differs')
    properties = (ndk / 'source.properties').read_text()
    require(re.search(r'^Pkg.Revision\s*=\s*' + re.escape(source['ndkVersion']) + r'\s*$', properties, re.M), 'NDK revision differs')
    jar = sdk_bootclasspath(sdk, policy)
    goroot = Path(output([go, 'env', 'GOROOT']))
    llvm = ndk / 'toolchains/llvm/prebuilt/linux-x86_64/bin'
    identity = {'go': {'version': go_version, 'binarySha256': file_hash(go), 'distribution': directory_identity(goroot)},
                'java': {'version': java, 'javacVersion': javac, 'javaSha256': file_hash(jdk / 'bin/java'),
                         'javacSha256': file_hash(jdk / 'bin/javac'), 'distribution': directory_identity(jdk)},
                'ndk': {'revision': source['ndkVersion'], 'declaredArchive': json.loads((PATCH_DIR / 'ndk-linux-archive.json').read_bytes()),
                        'installedDistribution': directory_identity(ndk), 'archiveVerification': 'not-asserted-by-consumer',
                        'clangSha256': file_hash(llvm / 'clang'), 'linkerSha256': file_hash(llvm / 'ld.lld')},
                'sdkBootclasspath': policy['sdkBootclasspath']}
    for name in ('gomobile', 'gobind'):
        binary = mobile / name
        raw = output([go, 'version', '-m', binary])
        info = parse_build_info(raw)
        main = info['modules'].get('github.com/sagernet/gomobile', {})
        require(main.get('kind') == 'mod' and main.get('version') == source['gomobileVersion']
                and 'replacement' not in main and info['path'] == 'github.com/sagernet/gomobile/cmd/' + name,
                'Pinned gomobile tool differs')
        identity[name] = {'sha256': file_hash(binary), 'buildInfo': info}
    return {'go': go, 'jdk': jdk, 'ndk': ndk, 'gomobile': mobile / 'gomobile', 'jar': jar, 'env': env, 'identity': identity}


def linker_flags(core, shared, receipt):
    return (f"-X github.com/sagernet/sing-box/constant.Version={core['sourceBuild']['version']} "
            '-X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w '
            + shared.source_linker_flag(receipt)
            + ' -extldflags=-Wl,-z,max-page-size=16384,-z,common-page-size=16384')


def input_identity(source, core, policy, tool_identity):
    spec = core['sourceBuild']
    code = ['scripts/build-libbox.sh', 'scripts/core-source-provision.py', 'scripts/libbox-patches/build.py',
            'scripts/libbox-patches/verify-receipt.py', 'scripts/libbox-patches/android-source.py',
            'scripts/libbox-patches/prepare-ndk-linux.py', 'scripts/libbox-patches/ndk-linux-archive.json']
    facts = {'schema': 'polaris-android-producer-input-v1', 'commonSource': {key: spec[key] for key in
             ('sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint', 'moduleGraphSha256', 'patchedSourceTree', 'buildTree', 'version', 'dependencyModules', 'transportPins')},
             'sourceManifest': source, 'androidPolicy': policy, 'androidPolicySha256': file_hash(PATCH_DIR / 'android-source-policy.json'),
             'tools': tool_identity, 'code': {path: file_hash(ROOT / path) for path in code},
             'bind': {'target': 'android', 'androidAPI': source['androidAPI'], 'javapkg': 'io.nekohasekai', 'libname': 'box',
                      'trimpath': True, 'buildVCS': False, 'tags': source['buildTags'],
                      'linkerFlags': linker_flags(core, provider(), {'fingerprint': spec['sourceReceiptFingerprint']}),
                      'cgoLDFLAGS': '-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384'}}
    return {'facts': facts, 'fingerprint': digest(canonical(facts))}


def candidate(run):
    require(not run(['git', 'status', '--porcelain', '--untracked-files=normal'], cwd=ROOT, capture=True).strip(), 'App source candidate must be clean')
    commit = run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, capture=True).strip()
    require(match('[0-9a-f]{40}', commit), 'Invalid App candidate')
    return commit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('admit', 'identity'))
    args = parser.parse_args()
    source, core, policy = admit()
    if args.action == 'admit':
        print('Android shared source configuration admitted')
    else:
        def run(arguments, **kwargs):
            kwargs.pop('capture', None)
            return subprocess.run(arguments, check=True, text=True, stdout=subprocess.PIPE, **kwargs).stdout
        print(input_identity(source, core, policy, tools(source, policy, run)['identity'])['fingerprint'])


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.CalledProcessError) as error:
        print('Android source admission failed: ' + str(error), file=__import__('sys').stderr)
        raise SystemExit(1)
