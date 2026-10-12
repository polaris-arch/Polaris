"""Formal mobile input declarations; no build, publication or global tool install.

The common Git source is shared with the published CLI/OpenWrt input. Mobile
binding targets, tools and receipts are separate. An input declaration cannot
promote a diagnostic AAR/framework or a CLI receipt into App acceptance.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = Path(__file__).with_name('source-contract.json')
ANDROID_TARGETS = ['android/arm64', 'android/arm', 'android/386', 'android/amd64']
APPLE_TARGETS = ['ios/arm64', 'iossimulator/arm64', 'iossimulator/amd64']
ANDROID_ABIS = ['arm64-v8a', 'armeabi-v7a', 'x86', 'x86_64']


def require(condition, reason):
    if not condition:
        raise RuntimeError(reason)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False,
                      allow_nan=False).encode()


def decode(raw, maximum=65536):
    def pairs(rows):
        value = {}
        for key, item in rows:
            require(key not in value, 'duplicate mobile contract key')
            value[key] = item
        return value
    require(len(raw) <= maximum, 'mobile input contract is too large')
    return json.loads(raw, object_pairs_hook=pairs,
                      parse_constant=lambda _: (_ for _ in ()).throw(RuntimeError('nonfinite input')))


def validate(value):
    require(value.get('schema') == 'polaris-mobile-fork-input-v1' and value.get('role') == 'mobile',
            'formal mobile source role is required')
    require(value.get('repository') == 'polaris-arch/polaris-box' and value.get('repositoryId') == 1407705262
            and value.get('sourceURL') == 'https://github.com/polaris-arch/polaris-box', 'mobile source repository differs')
    require(value.get('goModule') == 'github.com/sagernet/sing-box', 'file branding must not rename the Go module')
    for key, width in [('sourceCommit', 40), ('sourceTree', 40), ('sourceTagObject', 40),
                       ('goModSha256', 64), ('goSumSha256', 64)]:
        require(isinstance(value.get(key), str) and re.fullmatch('[0-9a-f]{%d}' % width, value[key]),
                'actual immutable source field required: ' + key)
    require(re.fullmatch(r'\d+\.\d+\.\d+-alpha\.\d+', value.get('version', '')) and
            value.get('sourceTag') == 'v' + value['version'], 'mobile source tag/version differs')
    require(type(value.get('sourceOverlay')) is int and value['sourceOverlay'] == 0 and
            value.get('patches') == [] and value.get('dependencyPatches') == [],
            'formal mobile source must be zero-overlay')
    require(value.get('goVersion') == '1.25.5' and
            value.get('gomobileVersion') == value.get('gobindVersion') == 'v0.1.13',
            'formal generator must match the actual source module and API')
    require(value.get('artifactPins') is None and value.get('producerReceipt') is None and
            value.get('AppAccepted') is False and value.get('devicePassed') is False and
            value.get('publicationEligible') is False, 'input declarations cannot issue artifact acceptance')
    android, apple = value['android'], value['apple']
    require(android['targets'] == ANDROID_TARGETS and android['abis'] == ANDROID_ABIS and
            android['minimumAPI'] == 24 and android['minimumLoadAlignment'] == 16384 and
            android['javaMajor'] == 17 and android['legacyAPI21Enabled'] is False,
            'formal Android requires one API24/four-ABI AAR and no legacy variant')
    require(android['ndkRevision'] == '30.0.16138531' and android['ndkPackage'] == 'android-ndk-r30-beta3',
            'formal Android NDK differs')
    require(apple['targets'] == APPLE_TARGETS and
            apple['slices'] == ['ios-arm64', 'ios-arm64-simulator', 'ios-x86_64-simulator'] and
            apple['minimumOS'] == '15.0' and apple['appMinimumOS'] == '17.0' and
            apple['macOSTvOSIncluded'] is False, 'formal Apple requires exactly three iOS architecture slices')
    require(apple['frameworkName'] == 'Libbox.xcframework' and
            android['libname'] == apple['libname'] == 'box' and android['javaPackage'] == 'io.nekohasekai',
            'existing SDK bindings must be retained')
    for platform in [android, apple]:
        tags = platform['tags']
        require(isinstance(tags, list) and tags and len(tags) == len(set(tags)) and
                all(isinstance(tag, str) and re.fullmatch('[a-z0-9_]+', tag) for tag in tags) and
                {'with_gvisor', 'with_naive_outbound', 'with_tailscale', 'with_clash_api'} <= set(tags),
                'formal mobile feature tags differ')
        require(platform['outputName'].startswith('polaris-box-' + value['version'] + '-'),
                'mobile asset filename must use the polaris-box prefix')
        require(Path(platform['outputName']).name == platform['outputName'], 'asset basename cannot escape output slot')
    require(set(apple['tags']) == set(android['tags']) | {'with_dhcp', 'grpcnotrace', 'with_low_memory'},
            'Apple target-only tags differ')
    return value


def load(path=CONTRACT):
    return validate(decode(Path(path).read_bytes()))


def git(source, *args):
    # Read-only source verification. Do not run the upstream build helper: it
    # chooses extra platforms/legacy variants and a git-describe version.
    return subprocess.check_output(['git', '-C', str(source), *args], text=True).strip()


def verify_source(source, value):
    value = validate(value)
    source = Path(source).resolve(strict=True)
    require(git(source, 'cat-file', '-t', value['sourceTagObject']) == 'tag', 'annotated source tag required')
    require(git(source, 'rev-parse', value['sourceTag'] + '^{tag}') == value['sourceTagObject'] and
            git(source, 'rev-parse', value['sourceTagObject'] + '^{commit}') == value['sourceCommit'] and
            git(source, 'rev-parse', value['sourceCommit'] + '^{tree}') == value['sourceTree'],
            'mobile source tag/object/commit/tree differs')
    require(git(source, 'rev-parse', 'HEAD') == value['sourceCommit'] and
            not git(source, 'status', '--porcelain=v1', '--untracked-files=all'),
            'source checkout must be exact and clean before mobile bind')
    for name, key in [('go.mod', 'goModSha256'), ('go.sum', 'goSumSha256')]:
        require(sha((source / name).read_bytes()) == value[key], 'source module bytes differ')
    module = (source / 'go.mod').read_text()
    require(re.search(r'^module\s+' + re.escape(value['goModule']) + r'\s*$', module, re.M) and
            re.search(r'^go\s+' + re.escape(value['goVersion']) + r'\s*$', module, re.M) and
            re.search(r'^\s*github\.com/sagernet/gomobile\s+' + re.escape(value['gomobileVersion']) + r'\s*$', module, re.M),
            'actual source Go/module/generator declaration differs')
    declared = value['declaredTagsSource']
    require(declared['path'] == 'cmd/internal/build_libbox/main.go' and
            sha((source / declared['path']).read_bytes()) == declared['sha256'], 'source-owned tags declaration differs')
    # Ignore only non-compiler evidence/output objects. A dirty ignored compiler
    # input could bypass the tracked tree check and must not enter a producer.
    ignored = git(source, 'ls-files', '--others', '--ignored', '--exclude-standard').splitlines()
    require(not any(Path(path).suffix in {'.go', '.c', '.cc', '.cpp', '.h', '.s', '.S', '.syso'} or
                    Path(path).name in {'go.mod', 'go.sum', 'go.work', 'go.work.sum'} for path in ignored),
            'untracked ignored compiler input is not a zero-overlay source')
    return {'schema': 'polaris-mobile-source-observation-v1', 'sourceCommit': value['sourceCommit'],
            'sourceTree': value['sourceTree'], 'sourceTag': value['sourceTag'],
            'sourceTagObject': value['sourceTagObject'], 'sourceOverlay': 0,
            'inputContractSha256': sha(canonical(value)), 'artifactAcceptance': False}


def linker_flags(value, input_fingerprint):
    validate(value)
    require(isinstance(input_fingerprint, str) and re.fullmatch('[0-9a-f]{64}', input_fingerprint),
            'actual binding input fingerprint required')
    # Version is explicit. Never use ReadTag/git describe or a reused CLI BuildID.
    return ('-X github.com/sagernet/sing-box/constant.Version=' + value['version'] +
            ' -X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0 -s -w' +
            ' -buildid=polaris-mobile-' + input_fingerprint)


def bind_command(platform, value, gomobile, output, input_fingerprint, bootclasspath=None):
    validate(value)
    require(platform in ('android', 'apple'), 'unknown formal mobile platform')
    profile = value[platform]
    args = [str(gomobile), 'bind', '-o', str(output), '-target', ','.join(profile['targets']),
            '-libname=' + profile['libname'], '-trimpath', '-buildvcs=false', '-work', '-x',
            '-ldflags', linker_flags(value, input_fingerprint), '-tags', ','.join(profile['tags'])]
    if platform == 'android':
        require(bootclasspath is not None, 'explicit Android bootclasspath required')
        args += ['-androidapi', str(profile['minimumAPI']), '-bootclasspath', str(bootclasspath),
                 '-javapkg=' + profile['javaPackage']]
    else:
        args += ['-iosversion', profile['minimumOS']]
    return args + ['./experimental/libbox']


def tool_environment(go, gomobile, gobind, cache, scratch, *, sdk=None, ndk=None,
                     jdk=None, developer=None):
    # Caller passes explicit scoped inputs. Do not inherit GOWORK/GOTOOLCHAIN,
    # ambient Android NDK selection, global GOFLAGS or an unrelated Go on PATH.
    import os
    env = dict(os.environ)
    for key in ['GOWORK', 'GOFLAGS', 'GOTOOLCHAIN', 'GOROOT', 'GOOS', 'GOARCH', 'GOARM', 'GOARM64',
                'GOBIN', 'GOPATH', 'CGO_CFLAGS', 'CGO_CPPFLAGS', 'CGO_LDFLAGS', 'CC', 'CXX',
                'ANDROID_HOME', 'ANDROID_SDK_ROOT', 'ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT',
                'DEVELOPER_DIR', 'SDKROOT', 'MACOSX_DEPLOYMENT_TARGET', 'IPHONEOS_DEPLOYMENT_TARGET']:
        env.pop(key, None)
    paths = [str(Path(p).parent) for p in [go, gomobile, gobind]]
    env.update(PATH=os.pathsep.join(paths + [env.get('PATH', '')]), GOWORK='off',
               GOFLAGS='-mod=readonly', GOTOOLCHAIN='local', CGO_ENABLED='1',
               GOCACHE=str(Path(cache) / 'go-build'), GOMODCACHE=str(Path(cache) / 'go-mod'),
               GOPATH=str(Path(cache) / 'go-path'),
               TMPDIR=str(scratch))
    if sdk is not None:
        require(ndk is not None and jdk is not None and developer is None, 'explicit Android tools required')
        # This SDK view has only the pinned NDK. Even a generator that prefers
        # SDK/ndk over ANDROID_NDK_HOME cannot fall back to an installed NDK28.
        env.update(ANDROID_HOME=str(sdk), ANDROID_SDK_ROOT=str(sdk),
                   ANDROID_NDK_HOME=str(ndk), ANDROID_NDK_ROOT=str(ndk), JAVA_HOME=str(jdk))
    else:
        require(developer is not None and ndk is None and jdk is None, 'explicit Apple developer directory required')
        env['DEVELOPER_DIR'] = str(developer)
    return env
