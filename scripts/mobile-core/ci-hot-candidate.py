#!/usr/bin/env python3
"""One frozen final-kernel / Android arm64 diagnostic consumer. Never a formal producer.

Only GitHub's direct, exact-SHA manual Android job may execute prepare/build.
The receipt is evidence for this build, not a mobile component acceptance receipt.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import struct
import subprocess
import sys
import zipfile
from urllib.request import urlopen
from xml.etree import ElementTree as ET

sys.dont_write_bytecode = True
from fork_input import ROOT, canonical, decode, git, git_environment, require, sha, tool_environment
from source_policy import candidate_vendors, verify_vendor_module_declarations, verify_vendor_sources, verify_vendor_graph
from artifacts import android_api, elf_loads, file_pin, hot_jni_contract, inspect_hot_java, module, module_stream, tree_inventory, zip_entries

APP_REPOSITORY = 'polaris-arch/Polaris'
APP_REPOSITORY_ID = '1347028220'
REF = 'refs/heads/hot/app-runtime-consumer-20261020'
WORKFLOW_REF = APP_REPOSITORY + '/.github/workflows/android.yml@' + REF
KERNEL_REPOSITORY = 'polaris-arch/sing-box-polaris'
KERNEL_REPOSITORY_ID = 1421657509
KERNEL_COMMIT = '0cfa24573ff34c26608151068334329268cfdf14'
KERNEL_TREE = '412b1640c777831b1e8811b4c596cfb6df27fdb0'
PROTO = 'b6bb4642d1081f238e5b4f3563b7ddab8e5a1d66b65a306fbd215153164fac5f'
SOURCE_HASHES = {
    'go.mod': 'd16c533c97de535cf38985a62a78993c1b755a8f723a78ea8a0d75d585d7201d',
    'go.sum': '04f10f1f02bd405b9d0e84407e44b35d45af49db7e10a96630679cedaefadeaa',
    'daemon/runtime_config.proto': PROTO,
    'cmd/internal/build_libbox/main.go': 'f32a147fe0f3f763dd2719887a805ec0c058d87297b0cbddadeeaddfdac32d06',
    'experimental/libbox/command_server.go': 'cf146f2ba3cc38b1e143b40438c4176547c2c4147e88bc02d1745a16d606a99f',
    'experimental/libbox/command_runtime.go': '3041a85bbda753e961b2b9b19ce1881b5afe000e3bcd6352d786679118055dc9',
    'experimental/libbox/runtime_persistence_handler.go': '837c8c664f1383e1f618babe6134f3509c6e96c1ed0344e7b654536b4e3ef867',
    'release/dependency-sources/metacubex-utls-replacement.json': 'bb44c0350b1d8ae65f364f986085760c024ccd9c9653bd89cb0e407de850888b',
    'release/dependency-sources/metacubex-utls-reality-alpn.patch': 'a0f7bcf8429b003e008ac38fa4a4c083f8c8397b4d75f5e3bd992e24d8fcc932',
}

# Exact source-owned replacements. Remote submodules (including Cronet native
# archives) still need their own version and h1; local sources never inherit h1.
VENDORS = {'github.com/sagernet/sing-tun': ('sing-tun', '29567e35d908ee81c82061dccc8815e51011fc33', 267), 'github.com/sagernet/tailscale': ('tailscale', '595e6f60e106780e60f55e20753da07b62ac0d7f', 1520), 'github.com/metacubex/utls': ('metacubex-utls', '40304faa4cfb50c928732bbbb476e22fb9119735', 355), 'github.com/sagernet/cronet-go': ('cronet-go', 'd61da6bb9082f951fb5e57503fc1eb840e07949b', 127), 'github.com/sagernet/fswatch': ('fswatch', '5c887b88e4d68bf5db7924610952d25117d2ec11', 8), 'github.com/sagernet/sing': ('sing', 'a2015b78131415904c500b2ea3d76bb8aca12fd5', 352), 'github.com/sagernet/sing-quic': ('sing-quic', '35dca4b40ca8b33ac5d7f697271fac5f14aea9da', 81), 'github.com/sagernet/wireguard-go': ('wireguard-go', '8f2e9708ac31b6453f94ce68837dca6f277c5fff', 85), 'github.com/database64128/tfo-go/v2': ('tfo-go', '97b124004140efa84aeb0f57d9bd41d238f0c9b5', 45), 'github.com/sagernet/sing-vmess': ('sing-vmess', 'c06c20c35da9997e56b5f6169143e3c0261da672', 36)}

TAGS = ('with_gvisor with_quic with_wireguard with_utls with_naive_outbound with_clash_api '
        'with_usbip with_openvpn with_openconnect badlinkname tfogo_checklinkname0 with_tailscale '
        'ts_omit_logtail ts_omit_ssh ts_omit_drive ts_omit_taildrop ts_omit_webclient ts_omit_doctor '
        'ts_omit_capture ts_omit_kube ts_omit_aws ts_omit_synology ts_omit_bird').split()
ANDROID = {'ndkRevision': '30.0.16248370', 'ndkSourceRevision': '30.0.16248370',
           'ndkPackage': 'android-ndk-r30', 'licenseId': 'android-sdk-license',
           'sourcePropertiesSha256': 'e73b7dc557cea2da9212c09ee1795434957d62e63b7550eef69fd1887a355d94',
           'archive': {'url': 'https://dl.google.com/android/repository/android-ndk-r30-linux.zip',
                       'bytes': 738633529, 'sha1': '5107f898313790e449e87eee2183d9a20602dee9',
                       'sha256': '753611f410d002cfcd3f3dc2ef49aad532089d3180b436c060a90bf0fcb64df2',
                       'archiveRoot': 'android-ndk-r30'},
           'bootclasspath': {'platform': 'android-36', 'sha256': 'd9eb9da824d9e247a352f570f01e1169e725b2954bca9e283a71786c59b59f9a'}}
BUILD = ['bash', 'scripts/build-android-apk.sh', '--debug', '--apk', '--split-per-abi',
         '--target', 'aarch64', '--ci', '--config', 'src-tauri/tauri.android.conf.json']
UNIT_TEST_CLASSES = ('RuntimeProtocolAdmissionTest', 'RuntimePersistenceSourceTest')
UNIT_TEST_BUILD = ['bash', 'src-tauri/gen/android/gradlew', '--project-dir', 'src-tauri/gen/android',
                   ':app:cleanTestArm64DebugUnitTest', ':app:testArm64DebugUnitTest',
                   *[arg for name in UNIT_TEST_CLASSES for arg in ('--tests', 'com.polaris2.app.vpn.' + name)],
                   '-x', ':app:rustBuildArm64Debug', '--rerun-tasks', '--max-workers=2', '--no-daemon']

FORMAL_HASH = '3740b0fe0739ae8f811b2a2a3665cfb0d0dc0d7882ffaec796956fed7714e5b6'
OVERRIDES = set(('GOFLAGS GOENV GOWORK GOTOOLCHAIN GOOS GOARCH GOARM GOARM64 GOEXPERIMENT '
                 'GOBIN GOMODCACHE GOCACHE GONOSUMDB GOPRIVATE GONOPROXY GOPROXY GOSUMDB GOINSECURE CC CXX CPP AR LD CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH '
                 'LIBRARY_PATH COMPILER_PATH GCC_EXEC_PREFIX JAVA_TOOL_OPTIONS _JAVA_OPTIONS '
                 'JDK_JAVA_OPTIONS GRADLE_OPTS JAVA_OPTS GRADLE_USER_HOME ORG_GRADLE_PROJECT '
                 'RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER '
                 'CARGO_TARGET_DIR CARGO_BUILD_TARGET ANDROID_NDK_HOME ANDROID_NDK_ROOT NDK_HOME NDK_RUST '
                 'NODE_OPTIONS NPM_CONFIG_REGISTRY npm_config_registry POLARIS_FORMAL_MOBILE_ROOT POLARIS_CORE_SOURCE POLARIS_CORE_PATH').split())


def validate_request(env, inputs, event):
    require(isinstance(inputs, dict), 'candidate inputs object required')
    candidate = inputs.get('hot_consumer_candidate', '')
    require(isinstance(candidate, str) and re.fullmatch('[0-9a-f]{40}', candidate), 'exact lowercase candidate App SHA required')
    expected = {'GITHUB_ACTIONS': 'true', 'GITHUB_REPOSITORY': APP_REPOSITORY,
                'GITHUB_REPOSITORY_ID': APP_REPOSITORY_ID, 'GITHUB_EVENT_NAME': 'workflow_dispatch',
                'GITHUB_REF': REF, 'GITHUB_WORKFLOW_REF': WORKFLOW_REF,
                'GITHUB_SHA': candidate, 'GITHUB_WORKFLOW_SHA': candidate, 'GITHUB_JOB': 'release_check'}
    require(all(env.get(k) == v for k, v in expected.items()), 'direct exact-SHA Android candidate context required')
    require(set(inputs) <= {'hot_consumer_candidate', 'preserve_run'} and
            inputs.get('preserve_run') is True, 'candidate must preserve run; no publish/formal/other input')
    require(isinstance(event, dict), 'direct dispatch event object required')
    # GitHub's workflow_dispatch event carries a full branch ref, unlike the
    # short branch name accepted by the REST dispatch request's ref parameter.
    # https://github.com/octokit/webhooks/blob/main/payload-examples/api.github.com/workflow_dispatch/payload.json
    require(event.get('ref') == REF, 'direct dispatch event ref differs from pinned full branch ref')
    repository = event.get('repository')
    require(isinstance(repository, dict), 'direct dispatch event repository object required')
    require(repository.get('full_name') == APP_REPOSITORY, 'direct dispatch event repository full_name differs')
    require(str(repository.get('id')) == APP_REPOSITORY_ID, 'direct dispatch event repository id differs')
    require(event.get('inputs') == {'hot_consumer_candidate': candidate, 'preserve_run': 'true'},
            'inherited or conflicting dispatch inputs refused')
    require(env.get('POLARIS_LIGHT_BUILD', '0') == env.get('POLARIS_PUBLISH_RELEASE', '0') == '0',
            'candidate cannot use release or light profile')
    reject_overrides(env)
    return candidate


def reject_overrides(env, prepared=False):
    for key, value in env.items():
        if (key in {'ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT'} or (prepared and key == 'NDK_RUST')) and value:
            expected = Path(env['ANDROID_HOME']).resolve(strict=True) / 'ndk/27.3.13750724'
            require(Path(value).resolve(strict=True) == expected.resolve(strict=True) and
                    re.search(r'^Pkg.Revision\s*=\s*27[.]3[.]13750724\s*$', (expected / 'source.properties').read_text(), re.M),
                    'ambient runner Rust NDK differs: ' + key)
            continue
        forbidden = key in OVERRIDES or key.startswith(('CARGO_PROFILE_', 'CGO_', 'PKG_CONFIG_', 'ORG_GRADLE_PROJECT_', 'TAURI_'))
        forbidden |= key.startswith('POLARIS_') and key not in {'POLARIS_LIGHT_BUILD', 'POLARIS_PUBLISH_RELEASE'}
        # actions/setup-go exports exactly local after the early admission gate.
        # Keep injected pre-setup values and every other toolchain selection closed.
        if prepared and key == 'GOTOOLCHAIN' and value == 'local':
            forbidden = False
        require(not (forbidden and value), 'ambient candidate override refused: ' + key)


def write_json(path, value):
    require(not path.is_symlink(), 'evidence output redirected')
    path.write_bytes(canonical(value) + b'\n')


def clean(root, *, generated=False, allow_ignored=False):
    require(not git(root, '-c', 'core.filemode=true', '-c', 'core.fsmonitor=false', 'status', '--porcelain=v1', '--untracked-files=no'), 'tracked source changed')
    require(all(row.startswith('H ') for row in git(root, 'ls-files', '-v').splitlines()), 'hidden index flags refused')
    others = git(root, 'ls-files', '--others', '--exclude-standard').splitlines()
    if not allow_ignored:
        others += git(root, 'ls-files', '--others', '--ignored', '--exclude-standard').splitlines()
    require(all(generated and name.startswith('build/') for name in others), 'unexpected untracked/ignored source input')


def slot():
    env = os.environ
    require(env.get('GITHUB_ACTIONS') == 'true' and env.get('GITHUB_JOB') == 'release_check', 'hosted candidate only')
    require(all(re.fullmatch('[1-9][0-9]*', env.get(k, '')) for k in ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT')), 'run identity required')
    temp = Path(env['RUNNER_TEMP']).resolve(strict=True)
    root = temp / ('polaris-hot-android-' + env['GITHUB_RUN_ID'] + '-' + env['GITHUB_RUN_ATTEMPT'])
    require(not root.is_symlink(), 'candidate work slot redirected')
    return root


def admit():
    candidate = validate_request(os.environ, decode(os.environ.get('HOT_CONSUMER_INPUTS', '{}').encode()),
                                 decode(Path(os.environ['GITHUB_EVENT_PATH']).read_bytes(), 1024 * 1024))
    require(git(ROOT, 'rev-parse', 'HEAD') == candidate, 'checkout differs from candidate')
    clean(ROOT)
    require(sha((ROOT / 'scripts/mobile-core/source-contract.json').read_bytes()) == FORMAL_HASH, 'formal pin changed')
    require(sha((ROOT / 'crates/singbox-grpc/proto/runtime_config.proto').read_bytes()) == PROTO, 'App/kernel proto differs')
    root = slot()
    require(not root.exists(), 'candidate slot already exists')
    root.mkdir(mode=0o700)
    (root / 'evidence').mkdir(mode=0o700)
    write_json(root / 'evidence/admission.json', {'candidate': candidate, 'tree': git(ROOT, 'rev-parse', 'HEAD^{tree}'),
        'workflow': WORKFLOW_REF, 'runId': os.environ['GITHUB_RUN_ID'], 'runAttempt': os.environ['GITHUB_RUN_ATTEMPT'],
        'diagnosticOnly': True, 'publicationEligible': False,
        'runnerNdk': {key: os.environ.get(key) for key in ('ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT')}})
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as out:
        out.write('evidence=' + str(root / 'evidence') + '\n')


def sdk_license_value(text):
    # SDK global.xjb binds xsd:string to TrimStringAdapter before License hashes
    # getValue() as UTF-8. Match its three replaceAll calls and Java String.trim;
    # Python's default Unicode \s and strip() would normalize additional bytes.
    # https://android.googlesource.com/platform/tools/base/+/mirror-goog-studio-main/repository/src/main/java/com/android/repository/impl/meta/TrimStringAdapter.java
    text = re.sub(r'(?<=[ \t\n\x0b\f\r])[ \t]*', '', text)
    text = re.sub(r'(?<!\n)\n(?!\n)', ' ', text)
    text = re.sub(r' +', ' ', text)
    return text.strip(''.join(chr(code) for code in range(0x21)))


def accepted_licenses(xml, package, home):
    licenses = {el.attrib['id']: sdk_license_value(''.join(el.itertext())) for el in xml.iter() if el.tag.rsplit('}', 1)[-1] == 'license'}
    refs = [el.attrib['ref'] for el in package.iter() if el.tag.rsplit('}', 1)[-1] == 'uses-license']
    require(refs and all(ref in licenses for ref in refs), 'SDK package lacks its exact license text')
    result = {}
    for ref in refs:
        require(re.fullmatch('[A-Za-z0-9_-]+', ref), 'invalid license name')
        accepted = home / 'licenses' / ref
        digest = hashlib.sha1(licenses[ref].encode()).hexdigest()
        # Text I/O normalizes CR/CRLF to LF, matching SDK readLines. Do not use
        # splitlines(), which would also accept Unicode/control separators.
        require(accepted.is_file() and digest in accepted.read_text(encoding='utf-8').split('\n'),
                'SDK license is not already accepted: ' + ref + ' (' + digest + ')')
        result[ref] = digest
    return result


def ndk_archive_contract():
    # Candidate-owned stable r30 profile; formal source identity stays separate.
    return dict(ANDROID['archive'])


def ndk_metadata(raw, home):
    archive = ndk_archive_contract()
    xml = ET.fromstring(raw)
    packages = [el for el in xml if el.tag.rsplit('}', 1)[-1] == 'remotePackage' and
                el.get('path') == 'ndk;' + ANDROID['ndkRevision']]
    require(len(packages) == 1, 'exact stable NDK package metadata unavailable')
    package = packages[0]
    revisions = [el for el in package if el.tag.rsplit('}', 1)[-1] == 'revision']
    require(len(revisions) == 1 and len(revisions[0]) == 3 and
            {el.tag.rsplit('}', 1)[-1]: el.text for el in revisions[0]} ==
            dict(zip(('major', 'minor', 'micro'), ANDROID['ndkRevision'].split('.'))), 'stable NDK revision differs')
    require([el.get('ref') for el in package if el.tag.rsplit('}', 1)[-1] == 'channelRef'] == ['channel-0'],
            'stable NDK channel differs')
    require([el.get('ref') for el in package.iter() if el.tag.rsplit('}', 1)[-1] == 'uses-license'] == [ANDROID['licenseId']],
            'stable NDK must use only the standard SDK license')
    downloads = [el for el in package.iter() if el.tag.rsplit('}', 1)[-1] == 'archive' and
                 [node.text for node in el if node.tag.rsplit('}', 1)[-1] == 'host-os'] == ['linux']]
    require(len(downloads) == 1, 'exact Linux NDK archive metadata unavailable')
    complete = [el for el in downloads[0] if el.tag.rsplit('}', 1)[-1] == 'complete']
    require(len(complete) == 1 and len(complete[0]) == 3, 'exact NDK archive metadata unavailable')
    fields = {el.tag.rsplit('}', 1)[-1]: el.text for el in complete[0]}
    require(fields == {'checksum': archive['sha1'], 'size': str(archive['bytes']), 'url': archive['url'].rsplit('/', 1)[1]} and
            [el.get('type') for el in complete[0] if el.tag.rsplit('}', 1)[-1] == 'checksum'] == ['sha1'],
            'official stable NDK archive metadata differs')
    return {'metadataSha256': sha(raw), 'profile': ANDROID, 'archive': archive,
            'licenses': accepted_licenses(xml, package, home)}


def verify_ndk_archive(download):
    archive = ndk_archive_contract()
    require(download.is_file() and not download.is_symlink() and download.stat().st_size == archive['bytes'], 'candidate NDK archive size/type differs')
    helper = module('candidate_ndk_hashes', 'scripts/libbox-patches/prepare-ndk-linux.py')
    actual_sha1, actual_sha256 = helper.hashes(download)
    require((actual_sha1, actual_sha256) == (archive['sha1'], archive['sha256']), 'candidate NDK archive hashes differ')
    with zipfile.ZipFile(download) as bundle:
        names = bundle.namelist()
        require(names and len(names) == len(set(names)), 'candidate NDK ZIP inventory empty/duplicate')
        for name in names:
            path = PurePosixPath(name)
            require(name.startswith(archive['archiveRoot'] + '/') and '..' not in path.parts and
                    not path.is_absolute() and '\\' not in name, 'candidate NDK ZIP escapes its root')
    return {'bytes': download.stat().st_size, 'sha1': actual_sha1, 'sha256': actual_sha256}


def prepare_candidate_ndk(root, evidence, run):
    from android_sdk import validate_ndk_properties
    archive = ndk_archive_contract()
    download, unpack = root / 'candidate-ndk.zip', root / 'candidate-ndk-unpacked'
    require(not any(path.exists() or path.is_symlink() for path in (download, unpack)), 'retained candidate NDK input refused')
    unpack.mkdir(mode=0o700)
    run(['curl', '--disable', '-f', '--proto', '=https', '--retry', '3', '--retry-all-errors', '--connect-timeout', '20',
         '--max-time', '900', '-sS', '-o', download, archive['url']], log=evidence / 'ndk-download.log', capture='none')
    actual = verify_ndk_archive(download)
    # The exact official archive needs unzip to preserve its symlinks/executables.
    run(['unzip', '-q', download, '-d', unpack], log=evidence / 'ndk-unpack.log', capture='none')
    ndk = unpack / archive['archiveRoot']
    properties = ndk / 'source.properties'
    require(properties.is_file() and not properties.is_symlink(), 'candidate NDK source.properties must be regular')
    require(file_pin(properties)['sha256'] == ANDROID['sourcePropertiesSha256'], 'candidate NDK source.properties bytes differ')
    values = validate_ndk_properties(properties.read_text(), ANDROID)
    facts = {'profile': ANDROID, 'archive': actual, 'path': str(ndk),
             'sourceProperties': file_pin(properties), 'properties': values}
    write_json(evidence / 'candidate-ndk.json', facts)
    download.unlink()
    return ndk, facts


def sdk():
    """No sdkmanager, no license acceptance, no fallback version or package install."""
    root = slot()
    require((root / 'evidence/admission.json').is_file(), 'candidate admission missing')
    home = Path(os.environ['ANDROID_HOME']).resolve(strict=True)
    if os.environ.get('ANDROID_SDK_ROOT'):
        require(Path(os.environ['ANDROID_SDK_ROOT']).resolve(strict=True) == home, 'conflicting SDK roots')
    rust_version = (ROOT / 'scripts/android-rust-ndk.version').read_text().strip()
    require(rust_version == '27.3.13750724', 'Rust NDK pin changed')
    packages = ['platforms/android-36', 'build-tools/36.0.0', 'ndk/' + rust_version]
    facts = {}
    for name in packages:
        package = home / name / 'package.xml'
        require(package.is_file(), 'preinstalled licensed SDK package required: ' + name)
        xml = ET.fromstring(package.read_bytes())
        local = [el for el in xml if el.tag.rsplit('}', 1)[-1] == 'localPackage']
        require(len(local) == 1 and local[0].get('path') == name.replace('/', ';'), 'installed SDK package path differs')
        facts[name] = {'package': file_pin(package), 'licenses': accepted_licenses(xml, local[0], home),
                       'sourceProperties': file_pin(home / name / 'source.properties')}
    jar = home / 'platforms/android-36/android.jar'
    require(file_pin(jar)['sha256'] == ANDROID['bootclasspath']['sha256'], 'Android SDK bootclasspath differs')
    for key in ('JAVA_HOME_17_X64', 'JAVA_HOME_21_X64'):
        require((Path(os.environ[key]) / 'bin/java').is_file(), 'preinstalled JDK required: ' + key)
    # Read official metadata only. Archive installation occurs later only if its
    # exact standard package license is already accepted; no preview fallback.
    url = 'https://dl.google.com/android/repository/repository2-3.xml'
    with urlopen(url, timeout=45) as response:
        require(response.geturl() == url, 'SDK metadata redirect refused')
        raw = response.read(8 * 1024 * 1024 + 1)
    require(len(raw) <= 8 * 1024 * 1024, 'SDK metadata exceeds bound')
    facts['libboxNdk'] = ndk_metadata(raw, home)
    write_json(root / 'evidence/sdk.json', facts)
    with Path(os.environ['GITHUB_ENV']).open('a') as out:
        out.write('JAVA_HOME=' + os.environ['JAVA_HOME_21_X64'] + '\nJDK17=' + os.environ['JAVA_HOME_17_X64'] + '\nNDK_RUST=' + str(home / 'ndk' / rust_version) + '\n')
    with Path(os.environ['GITHUB_PATH']).open('a') as out:
        out.write(os.environ['JAVA_HOME_21_X64'] + '/bin\n')


def tracked_mirror(source):
    rows = git(source, 'ls-tree', '-r', 'HEAD').splitlines()
    require(0 < len(rows) <= 65536, 'tracked source inventory bound')
    gitlinks = []
    for row in rows:
        info, name = row.split('\t'); mode, kind, blob = info.split()
        path = source / name
        require(not Path(name).is_absolute() and '..' not in Path(name).parts, 'tracked source path escaped')
        if mode == '160000':
            require(kind == 'commit' and (not path.exists() or (path.is_dir() and not path.is_symlink() and not any(path.iterdir()))), 'gitlink must stay uninitialized')
            gitlinks.append({'path': name, 'commit': blob})
            continue
        if mode == '120000':
            require(path.is_symlink(), 'source symlink mode differs')
            raw = os.readlink(path).encode()
        else:
            require(mode in ('100644', '100755') and path.is_file() and not path.is_symlink() and
                    bool(path.stat().st_mode & 0o111) == (mode == '100755') and path.stat().st_size <= 128 * 1024 * 1024,
                    'tracked source mode/size differs')
            raw = path.read_bytes()
        require(kind == 'blob' and hashlib.sha1(b'blob ' + str(len(raw)).encode() + b'\0' + raw).hexdigest() == blob, 'tracked source bytes differ: ' + name)
    return {'leaves': len(rows), 'gitlinks': gitlinks, 'inventorySha256': sha(canonical(rows))}


def source_identity(identity=None):
    """Reuse the byte/graph verifier with an explicit reviewed desktop identity.

    Android callers omit the argument and keep their original frozen identity.
    This is input validation only; it grants no build, runtime or release access.
    """
    if identity is None:
        return {'repository': KERNEL_REPOSITORY, 'repositoryId': KERNEL_REPOSITORY_ID,
                'commit': KERNEL_COMMIT, 'tree': KERNEL_TREE, 'files': SOURCE_HASHES,
                'vendors': VENDORS}
    require(isinstance(identity, dict) and set(identity) ==
            {'repository', 'repositoryId', 'commit', 'tree', 'files', 'vendors'},
            'exact source identity fields required')
    require(identity['repository'] == KERNEL_REPOSITORY and identity['repositoryId'] == KERNEL_REPOSITORY_ID,
            'only the paired Polaris kernel repository is allowed')
    require(all(isinstance(identity[k], str) and re.fullmatch('[0-9a-f]{40}', identity[k])
                for k in ('commit', 'tree')), 'exact kernel commit/tree required')
    files = identity['files']
    require(isinstance(files, dict) and {'go.mod', 'go.sum', 'daemon/runtime_config.proto'} <= set(files)
            and 3 <= len(files) <= 64, 'bounded kernel source files required')
    for name, digest in files.items():
        require(isinstance(name, str) and not PurePosixPath(name).is_absolute()
                and '..' not in PurePosixPath(name).parts and str(PurePosixPath(name)) == name
                and isinstance(digest, str) and re.fullmatch('[0-9a-f]{64}', digest),
                'invalid kernel source file pin')
    require(files['daemon/runtime_config.proto'] == PROTO, 'paired runtime protocol bytes differ')
    candidate_vendors(identity['vendors'], schema='polaris-reviewed-kernel-source-v3')
    return identity


def source_facts(source, generated=False, *, identity=None):
    selected = source_identity(identity)
    require(git(source, 'rev-parse', 'HEAD') == selected['commit'] and git(source, 'rev-parse', 'HEAD^{tree}') == selected['tree'],
            'kernel commit/tree differs')
    clean(source, generated=generated)
    for name, digest in selected['files'].items():
        require(file_pin(source / name)['sha256'] == digest, 'kernel file differs: ' + name)
    declarations = candidate_vendors(selected['vendors'], schema='polaris-reviewed-kernel-source-v3')
    verify_vendor_module_declarations((source / 'go.mod').read_bytes(), declarations)
    vendors, _, _, _ = verify_vendor_sources(source, declarations, git)
    return {'repository': KERNEL_REPOSITORY, 'repositoryId': KERNEL_REPOSITORY_ID,
            'commit': selected['commit'], 'tree': selected['tree'], 'files': selected['files'], 'vendors': vendors,
            'trackedMirror': tracked_mirror(source)}


def graph_facts(graph, source, *, identity=None):
    selected = source_identity(identity)
    return verify_vendor_graph(graph, source, candidate_vendors(selected['vendors'], schema='polaris-reviewed-kernel-source-v3'))


def native_modules(facts, graph, source):
    require(facts['goVersion'] == '1.27.2', 'native Go version differs')
    settings = facts['settings']
    for key, value in {'GOOS': 'android', 'GOARCH': 'arm64', 'CGO_ENABLED': '1', '-buildmode': 'c-shared', '-trimpath': 'true'}.items():
        require(settings.get(key) == value, 'native build setting differs: ' + key)
    require(set(settings.get('-tags', '').split(',')) == set(TAGS), 'native tags differ')
    declared = {row['Path']: row for row in graph}
    require('github.com/sagernet/sing-box' in facts['modules'], 'source module absent from native library')
    for name, observed in facts['modules'].items():
        if name in ('command-line-arguments', 'gobind') and observed['kind'] == 'mod':
            continue
        require(name in declared, 'native module absent from frozen graph: ' + name)
        row = declared[name]
        if row.get('Main'):
            if observed['kind'] == 'mod':
                require(observed['version'] == '(devel)' and 'replacement' not in observed and
                        facts['path'] == 'github.com/sagernet/sing-box/build/arm64/libbox', 'native generated main path differs')
            else:
                require(observed['kind'] == 'dep' and observed.get('replacement') == str(source) and
                        observed['version'] == 'v0.0.0-00010101000000-000000000000', 'native source module path differs')
            continue
        require(observed['version'] == row.get('Version'), 'native dependency version differs')
        replacement = row.get('Replace')
        if replacement:
            aliases = {replacement['Path']}
            if not replacement.get('Version'):
                aliases.add(str((source / replacement['Path']).resolve(strict=True)))
            require(observed.get('replacement') in aliases and observed.get('replacementVersion', '') in
                    ({replacement['Version']} if replacement.get('Version') else {'', '(devel)'}) and
                    observed.get('replacementSum', '') == replacement.get('Sum', ''), 'native replacement differs')
        else:
            require('replacement' not in observed and observed.get('sum', '') == row.get('Sum', '') and row.get('Sum'), 'native remote h1 differs')


def inspect_component(aar, scratch, tools, run, graph, source):
    with zipfile.ZipFile(aar) as archive:
        names = {row.filename for row in zip_entries(archive)}
        require({n for n in names if n.endswith('.so')} == {'jni/arm64-v8a/libbox.so'}, 'candidate must contain only arm64 libbox')
        require(re.search(r'minSdkVersion\s*=\s*[\"\']24[\"\']', archive.read('AndroidManifest.xml').decode()), 'candidate minimum API differs')
        raw = archive.read('jni/arm64-v8a/libbox.so')
        require(raw[:6] == b'\x7fELF\x02\x01' and struct.unpack_from('<HH', raw, 16) == (3, 183), 'candidate ELF ABI differs')
        loads = elf_loads(raw)
        require(all(row['alignment'] >= 16384 and (row['virtualAddress'] - row['offset']) % 16384 == 0 for row in loads), 'candidate requires 16KiB PT_LOAD')
        require(android_api(raw) == 24, 'candidate ELF API differs')
        so = scratch / 'libbox.so'; so.write_bytes(raw)
        classes = scratch / 'classes.jar'; classes.write_bytes(archive.read('classes.jar'))
        members = {name: {'bytes': len(archive.read(name)), 'sha256': sha(archive.read(name))} for name in names if not name.endswith('/')}
    java = inspect_hot_java(classes, tools, run)
    symbols = run([tools['readelf'], '--dyn-syms', '--wide', so])
    exported = {m[1] for m in re.finditer(r'GLOBAL\s+DEFAULT\s+(?!UND\b)\S+\s+(\S+)', symbols)}
    expected = hot_jni_contract(exported)
    needed = re.findall(r'Shared library: \[([^]]+)\]', run([tools['readelf'], '-d', so]))
    require(set(needed) <= {'liblog.so', 'libdl.so', 'libm.so', 'libandroid.so', 'libc.so'}, 'unpackaged native dependency')
    raw_info = run([tools['go'], 'version', '-m', so])
    facts = module('hot_buildinfo', 'scripts/libbox-patches/android-source.py').parse_build_info(raw_info)
    native_modules(facts, graph, source)
    notes = run([tools['readelf'], '--notes', so])
    return {'artifact': file_pin(aar), 'members': members, 'java': java, 'nativeBuildInfo': raw_info,
            'parsedBuildInfo': facts, 'buildId': run([tools['go'], 'tool', 'buildid', so]).strip(), 'buildIdObservation': {'readelfNotes': notes,
                'goNotePresent': '.note.go.buildid' in notes, 'gnuNotePresent': '.note.gnu.build-id' in notes},
            'ptLoad': loads, 'androidAPI': 24, 'jniExports': sorted(expected), 'needed': needed}


def inspect_apk(apk, component, rust):
    with zipfile.ZipFile(apk) as bundle:
        names = {row.filename for row in zip_entries(bundle)}
        libs = {n for n in names if n.endswith('.so')}
        require(libs and all(n.startswith('lib/arm64-v8a/') for n in libs), 'APK has wrong ABI/native path')
        expected = {'lib/arm64-v8a/libbox.so': component['members']['jni/arm64-v8a/libbox.so'],
                    'lib/arm64-v8a/libpolaris_lib.so': file_pin(rust)}
        for name, pin in expected.items():
            raw = bundle.read(name)
            require({'bytes': len(raw), 'sha256': sha(raw)} == pin, 'APK did not consume this build input: ' + name)
        require(any(re.fullmatch(r'classes(?:[2-9][0-9]*)?\.dex', n) for n in names), 'actual compiled APK DEX missing')
        return {'artifact': file_pin(apk), 'nativeLibraries': expected, 'dex': {n: sha(bundle.read(n)) for n in names if n.endswith('.dex')}}


def unit_test_reports(app, evidence):
    """Retain original reports even on a failed Gradle invocation; no acceptance."""
    reports = {}
    for name in UNIT_TEST_CLASSES:
        filename = 'TEST-com.polaris2.app.vpn.' + name + '.xml'
        original = app / 'build/test-results/testArm64DebugUnitTest' / filename
        if not original.exists() and not original.is_symlink():
            continue
        require(original.is_file() and not original.is_symlink() and
                0 < original.stat().st_size <= 2 * 1024 * 1024, 'bounded regular HOT JUnit report required')
        raw = original.read_bytes()
        require(0 < len(raw) <= 2 * 1024 * 1024, 'HOT JUnit report byte budget')
        retained = evidence / filename
        with retained.open('xb') as output:
            output.write(raw)
        reports[name] = file_pin(retained)
        require(file_pin(original) == reports[name], 'HOT JUnit report changed during retention')
    return reports


def inspect_unit_results(app, evidence, candidate, component_pin, rust_pin):
    rows = []
    for name in UNIT_TEST_CLASSES:
        source = app / 'src/test/java/com/polaris2/app/vpn' / (name + '.kt')
        source_pin = file_pin(source)
        raw = source.read_bytes()
        require(sha(raw) == source_pin['sha256'], 'HOT test source changed during read')
        methods = re.findall(r'^\s*@Test(?:\([^\n)]*\))?\s+fun\s+(\w+)\s*\(', raw.decode(), re.M)
        require(methods and len(methods) == len(set(methods)), 'empty/duplicate HOT test source inventory')
        filename = 'TEST-com.polaris2.app.vpn.' + name + '.xml'
        retained = evidence / filename
        report_pin = file_pin(retained)
        require(report_pin['bytes'] <= 2 * 1024 * 1024 and
                file_pin(app / 'build/test-results/testArm64DebugUnitTest' / filename) == report_pin,
                'original/retained HOT JUnit report differs')
        result = retained.read_bytes()
        require(sha(result) == report_pin['sha256'], 'HOT JUnit report changed during read')
        suite = ET.fromstring(result)
        classname = 'com.polaris2.app.vpn.' + name
        require(suite.tag == 'testsuite' and suite.get('name') == classname, 'different HOT JUnit suite')
        cases = suite.findall('testcase')
        require(len(cases) == len(methods) == int(suite.get('tests', '-1')), 'HOT test methods missing or duplicated')
        require(all(int(suite.get(k, '-1')) == 0 for k in ('failures', 'errors', 'skipped')), 'HOT JUnit failure/error/skip')
        require(sorted(c.get('name') for c in cases) == sorted(methods), 'HOT test results differ from complete source methods')
        require(all(c.get('classname') == classname and not any(c.find(k) is not None for k in ('failure', 'error', 'skipped'))
                    for c in cases), 'different/failed/skipped HOT JUnit case')
        rows.append({'class': classname, 'tests': len(cases), 'methods': sorted(methods),
                     'sourceSha256': source_pin['sha256'], 'reportSha256': report_pin['sha256']})
    return {'schema': 'polaris-hot-android-gradle-unit-results-v1', 'candidate': candidate,
            'kernelCommit': KERNEL_COMMIT, 'variant': 'arm64Debug', 'classes': rows,
            'component': component_pin, 'rustApp': rust_pin, 'command': UNIT_TEST_BUILD,
            'scope': 'Real Gradle JVM unit tests with the same actual staged AAR and existing debug Rust output; no JNI/callback/device execution claim'}


def build():
    from types import SimpleNamespace
    from producer import run, private_dir, tool_facts, admit_module_cache
    from android_sdk import prepare_view
    from native_inputs import collect, verify
    root = slot(); evidence = root / 'evidence'
    admission = decode((evidence / 'admission.json').read_bytes())
    require(git(ROOT, 'rev-parse', 'HEAD') == admission['candidate'], 'App source changed after admission')
    clean(ROOT, allow_ignored=True)
    reject_overrides(os.environ, prepared=True)
    result = {'schema': 'polaris-hot-android-diagnostic-build-v1', **admission, 'status': 'failed',
              'devicePassed': False, 'runtimeTestsRun': 0, 'formalProducerReceipt': False,
              'unverified': ['device/runtime behavior', 'four ABI formal producer acceptance',
                             'full native archive provenance and reproducibility', 'release profile/R8/signing']}
    stage = ROOT / 'src-tauri/gen/android/app/libs/libbox.aar'
    cli_link = ROOT / 'ui/node_modules/@tauri-apps/cli'
    staged = linked = False
    try:
        require(not stage.exists() and not stage.is_symlink(), 'existing libbox staging input refused')
        require(not (ROOT / 'target/aarch64-linux-android/debug/libpolaris_lib.so').exists(), 'cached Rust App artifact refused')
        source = root / 'kernel'
        require(not source.exists(), 'retained kernel checkout refused')
        run(['git', 'init', '--quiet', source], env=git_environment())
        run(['git', '-C', source, 'fetch', '--depth=1', '--no-tags', 'https://github.com/' + KERNEL_REPOSITORY + '.git', KERNEL_COMMIT], env=git_environment())
        run(['git', '-C', source, 'checkout', '--detach', KERNEL_COMMIT], env=git_environment())
        result['sourceBefore'] = source_facts(source)
        work = private_dir(root / 'work'); cache = private_dir(root / 'cache'); scratch = private_dir(work / 'scratch')
        tools_dir = private_dir(root / 'tools')
        go = str(Path(shutil.which('go')).resolve(strict=True))
        install_env = dict(git_environment(), GOENV='off', GOWORK='off', GOTOOLCHAIN='local', GOFLAGS='-mod=readonly',
                           GOBIN=str(tools_dir), GOPATH=str(cache / 'go-path'), GOCACHE=str(cache / 'go-build'), GOMODCACHE=str(cache / 'go-mod'))
        require(run([go, 'version']).startswith('go version go1.27.2 '), 'actual Go differs')
        run([go, 'install', 'github.com/sagernet/gomobile/cmd/gomobile@v0.1.13', 'github.com/sagernet/gomobile/cmd/gobind@v0.1.13'], env=install_env, log=evidence / 'tool-install.log', capture='none')
        ndk_admission = decode((evidence / 'sdk.json').read_bytes(), 1024 * 1024)['libboxNdk']
        require(ndk_admission['archive'] == ndk_archive_contract() and ndk_admission['profile'] == ANDROID and
                set(ndk_admission['licenses']) == {ANDROID['licenseId']}, 'NDK license/metadata admission missing')
        result['sdkNdkAdmission'] = ndk_admission
        ndk, result['candidateNdk'] = prepare_candidate_ndk(root, evidence, run)
        jdk = Path(os.environ['JAVA_HOME_17_X64']).resolve(strict=True)
        sdk_view, jar = prepare_view(work, os.environ['ANDROID_HOME'], ndk, ANDROID)
        env = tool_environment(go, tools_dir / 'gomobile', tools_dir / 'gobind', cache, scratch, sdk=sdk_view, ndk=ndk, jdk=jdk)
        env.update(GOENV='off', CGO_LDFLAGS='-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384')
        private_dir(cache / 'go-path/pkg/gomobile')
        args = SimpleNamespace(platform='android', go=go, gomobile=tools_dir / 'gomobile', gobind=tools_dir / 'gobind', ndk=ndk, jdk=jdk)
        values = {'goVersion': '1.27.2', 'gomobileVersion': 'v0.1.13', 'gobindVersion': 'v0.1.13'}
        tools, identities = tool_facts(args, values, env)
        result['tools'] = identities
        result['bootclasspath'] = file_pin(jar)
        graph = module_stream(run([go, 'list', '-m', '-json', 'all'], cwd=source, env=env))
        admit_module_cache(source, graph, go, env, work)
        shutil.copyfile(work / 'module-cache-admission-observation.json', evidence / 'module-cache-admission-observation.json')
        run([go, 'mod', 'verify'], cwd=source, env=env, log=evidence / 'module-verify.log', capture='none')
        graph = graph_facts(module_stream(run([go, 'list', '-m', '-json', 'all'], cwd=source, env=env)), source)
        write_json(evidence / 'module-graph.json', graph)
        require(source_facts(source) == result['sourceBefore'], 'source changed during module preparation')
        ldflags = (source / 'release/LDFLAGS').read_text().strip()
        require(ldflags == '-X runtime.godebugDefault=multipathtcp=0,tlssha1=1 -checklinkname=0', 'source linker flags differ')
        flags = ldflags + ' -s -w -X github.com/sagernet/sing-box/constant.DirectoryRetirementSourceIdentity=' + KERNEL_COMMIT
        aar = work / ('libbox-' + KERNEL_COMMIT[:12] + '-arm64.aar')
        command = [tools['gomobile'], 'bind', '-o', str(aar), '-target', 'android/arm64', '-androidapi', '24',
                   '-bootclasspath', str(jar), '-javapkg=io.nekohasekai', '-libname=box', '-trimpath', '-buildvcs=false',
                   '-work', '-x', '-ldflags', flags, '-tags', ','.join(TAGS), './experimental/libbox']
        result.update(command=command, bindEnvironment={k: env[k] for k in ('GOENV', 'GOWORK', 'GOTOOLCHAIN', 'GOFLAGS', 'CGO_ENABLED', 'CGO_LDFLAGS', 'GOPATH', 'GOMODCACHE', 'GOCACHE', 'TMPDIR', 'ANDROID_HOME', 'ANDROID_NDK_HOME', 'JAVA_HOME')})
        run(command, cwd=source, env=env, log=evidence / 'bind.log', capture='none')
        result['sourceAfter'] = source_facts(source, generated=True)
        if (source / 'build').exists():
            result['generatorOutputs'] = tree_inventory(source / 'build')
        require(result['sourceBefore'] == result['sourceAfter'], 'kernel changed during bind')
        component = inspect_component(aar, scratch, tools, run, graph, source)
        result['component'] = component
        origins = collect(scratch, evidence / 'bind.log', 'android', component, cache)
        origins['candidateGeneratorOutputs'] = result.get('generatorOutputs', [])
        verify(origins, scratch=scratch, log=evidence / 'bind.log')
        if (source / 'build').exists():
            require(tree_inventory(source / 'build') == origins['candidateGeneratorOutputs'], 'generator output bytes changed')
        write_json(evidence / 'native-inputs.json', origins)
        require(tool_facts(args, values, env)[1] == identities, 'tool bytes changed during bind')
        # BuildTask uses this exact local entry. Bind it to the already installed,
        # pinned global CLI; never install a second package or skip the Rust task.
        global_modules = Path(run(['npm', 'root', '-g']).strip()).resolve(strict=True)
        cli = global_modules / '@tauri-apps/cli'
        metadata = decode((cli / 'package.json').read_bytes())
        require(metadata['name'] == '@tauri-apps/cli' and metadata['version'] == (ROOT / 'scripts/tauri-cli.version').read_text().strip() == '2.12.1', 'actual global Tauri package differs')
        cli_pin = file_pin(cli / 'tauri.js')
        cli_inventory = tree_inventory(cli)
        result['tauriCliInventory'] = cli_inventory
        result['uiInputs'] = {name: file_pin(ROOT / 'ui' / name) for name in ('package.json', 'pnpm-lock.yaml')}
        result['appTools'] = {}
        for name in ('node', 'pnpm', 'tauri', 'protoc', 'rustc', 'cargo'):
            actual = Path(run(['rustup', 'which', name]).strip() if name in ('rustc', 'cargo') else shutil.which(name)).resolve(strict=True)
            result['appTools'][name] = {'path': str(actual), **file_pin(actual), 'version': run([str(actual), '--version'])}
        result['appTools']['java21'] = file_pin((Path(os.environ['JAVA_HOME_21_X64']) / 'bin/java').resolve(strict=True))
        rust_bin = Path(os.environ['NDK_RUST']) / 'toolchains/llvm/prebuilt/linux-x86_64/bin'
        result['appTools']['rustNdk'] = {name: file_pin((rust_bin / name).resolve(strict=True)) for name in ('clang', 'ld.lld', 'llvm-ar')}
        result['appTools']['rustNdkProperties'] = file_pin(Path(os.environ['NDK_RUST']) / 'source.properties')
        if cli_link.exists() or cli_link.is_symlink():
            require(cli_link.resolve(strict=True) == cli.resolve(strict=True), 'local Tauri CLI differs')
        else:
            cli_link.parent.mkdir(parents=True, exist_ok=True)
            cli_link.symlink_to(cli, target_is_directory=True); linked = True
        result['tauriCli'] = {'declaredInstall': 'npm install -g @tauri-apps/cli@2.12.1',
                              'versionFile': file_pin(ROOT / 'scripts/tauri-cli.version'),
                              'workflow': file_pin(ROOT / '.github/workflows/android.yml'), 'globalPath': str(cli), 'localPath': str(cli_link), 'entry': cli_pin, 'package': file_pin(cli / 'package.json'), 'createdLink': linked}
        stage.parent.mkdir(parents=True, exist_ok=True)
        with stage.open('xb') as output, aar.open('rb') as source_file:
            shutil.copyfileobj(source_file, output)
        staged = True
        require(file_pin(stage) == component['artifact'], 'staged AAR differs')
        result['stagedBefore'] = file_pin(stage)
        app_env = dict(os.environ, JAVA_HOME=os.environ['JAVA_HOME_21_X64'], ANDROID_NDK_HOME=os.environ['NDK_RUST'], NDK_HOME=os.environ['NDK_RUST'], ANDROID_NDK_ROOT=os.environ['NDK_RUST'])
        gradle_home = private_dir(root / 'gradle-home')
        (gradle_home / 'gradle.properties').write_text('org.gradle.daemon=false\norg.gradle.workers.max=2\nkotlin.compiler.execution.strategy=in-process\nandroid.builder.sdkDownload=false\n')
        app_env['GRADLE_USER_HOME'] = str(gradle_home)
        result['gradleProperties'] = file_pin(gradle_home / 'gradle.properties')
        result['appCommand'] = BUILD
        run(BUILD, cwd=ROOT, env=app_env, log=evidence / 'app-build.log', capture='none')
        require(file_pin(stage) == component['artifact'] and tree_inventory(cli) == cli_inventory, 'App inputs changed during consumer build')
        rust = ROOT / 'target/aarch64-linux-android/debug/libpolaris_lib.so'
        apk = ROOT / 'src-tauri/gen/android/app/build/outputs/apk/arm64/debug/app-arm64-debug.apk'
        result['apk'] = inspect_apk(apk, component, rust)
        rust_pin = file_pin(rust)
        result['unitTestCommand'] = UNIT_TEST_BUILD
        try:
            # The debug APK already built the actual Rust target. Exclude only
            # that completed task, then force this selected JVM suite to execute.
            run(UNIT_TEST_BUILD, cwd=ROOT, env=app_env, log=evidence / 'unit-tests.log', capture='none')
        finally:
            result['unitTestReports'] = unit_test_reports(ROOT / 'src-tauri/gen/android/app', evidence)
        require(file_pin(stage) == component['artifact'] and file_pin(rust) == rust_pin and
                tree_inventory(cli) == cli_inventory, 'App inputs changed during unit tests')
        result['unitTests'] = inspect_unit_results(ROOT / 'src-tauri/gen/android/app', evidence,
                                                   admission['candidate'], component['artifact'], rust_pin)
        write_json(evidence / 'unit-results.json', result['unitTests'])
        clean(ROOT, allow_ignored=True)
        require(git(ROOT, 'rev-parse', 'HEAD') == admission['candidate'], 'App candidate changed during build')
        require(file_pin(ROOT / 'scripts/mobile-core/source-contract.json')['sha256'] == FORMAL_HASH, 'formal pin changed')
        result['status'] = 'compiled-statically-verified-and-jvm-unit-tested'
    except BaseException as error:
        result['failure'] = str(error)
        raise
    finally:
        try:
            if staged:
                result['stagedAfter'] = file_pin(stage)
                require(result['stagedAfter'] == result['stagedBefore'], 'staged AAR changed; refusing cleanup')
                stage.unlink()
            if linked:
                require(cli_link.is_symlink() and cli_link.resolve(strict=True) == cli.resolve(strict=True), 'candidate CLI staging changed; refusing cleanup')
                cli_link.unlink()
        except BaseException as error:
            result.update(status='failed', cleanupFailure=str(error))
            raise
        finally:
            write_json(evidence / 'result.json', result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['admit', 'sdk', 'build'])
    action = parser.parse_args().action
    globals()[action]()


if __name__ == '__main__':
    main()
