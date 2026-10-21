#!/usr/bin/env python3
"""One frozen a567 / Android arm64 diagnostic consumer. Never a formal producer.

Only GitHub's direct, exact-SHA manual Android job may execute prepare/build.
The receipt is evidence for this build, not a mobile component acceptance receipt.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
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
from artifacts import android_api, elf_loads, file_pin, module, module_stream, tree_inventory, zip_entries

APP_REPOSITORY = 'polaris-arch/Polaris'
APP_REPOSITORY_ID = '1347028220'
REF = 'refs/heads/hot/app-runtime-consumer-20261020'
WORKFLOW_REF = APP_REPOSITORY + '/.github/workflows/android.yml@' + REF
KERNEL_REPOSITORY = 'polaris-arch/sing-box-polaris'
KERNEL_REPOSITORY_ID = 1421657509
KERNEL_COMMIT = 'a5674b5f4dcb5ef2647192e05d891269e56bdae4'
KERNEL_TREE = '7006d05e46ea0c5e8299c0aea56a011c62d24fc7'
PROTO = 'b6bb4642d1081f238e5b4f3563b7ddab8e5a1d66b65a306fbd215153164fac5f'
SOURCE_HASHES = {
    'go.mod': '2c41a5a45e22341d2f8ca750134edeb5e09cf79d3366575a5a0966cc10a6162c',
    'go.sum': '04f10f1f02bd405b9d0e84407e44b35d45af49db7e10a96630679cedaefadeaa',
    'daemon/runtime_config.proto': PROTO,
    'cmd/internal/build_libbox/main.go': 'f32a147fe0f3f763dd2719887a805ec0c058d87297b0cbddadeeaddfdac32d06',
    'experimental/libbox/command_server.go': 'cf146f2ba3cc38b1e143b40438c4176547c2c4147e88bc02d1745a16d606a99f',
    'experimental/libbox/runtime_persistence_handler.go': '837c8c664f1383e1f618babe6134f3509c6e96c1ed0344e7b654536b4e3ef867',
    'release/dependency-sources/metacubex-utls-replacement.json': 'bb44c0350b1d8ae65f364f986085760c024ccd9c9653bd89cb0e407de850888b',
    'release/dependency-sources/metacubex-utls-reality-alpn.patch': 'a0f7bcf8429b003e008ac38fa4a4c083f8c8397b4d75f5e3bd992e24d8fcc932',
}
# Exact source-owned replacements. Remote submodules (including Cronet native
# archives) still need their own version and h1; local sources never inherit h1.
VENDORS = {
    'github.com/sagernet/sing-tun': ('sing-tun', '29567e35d908ee81c82061dccc8815e51011fc33', 267),
    'github.com/sagernet/tailscale': ('tailscale', '595e6f60e106780e60f55e20753da07b62ac0d7f', 1520),
    'github.com/metacubex/utls': ('metacubex-utls', '40304faa4cfb50c928732bbbb476e22fb9119735', 355),
    'github.com/sagernet/cronet-go': ('cronet-go', 'd61da6bb9082f951fb5e57503fc1eb840e07949b', 127),
    'github.com/sagernet/fswatch': ('fswatch', '5c887b88e4d68bf5db7924610952d25117d2ec11', 8),
    'github.com/sagernet/sing': ('sing', 'bdc317ad143368210802cba500a414102f677125', 350),
    'github.com/sagernet/sing-quic': ('sing-quic', '35dca4b40ca8b33ac5d7f697271fac5f14aea9da', 81),
    'github.com/sagernet/wireguard-go': ('wireguard-go', '8f2e9708ac31b6453f94ce68837dca6f277c5fff', 85),
    'github.com/database64128/tfo-go/v2': ('tfo-go', '0a46fb2e32fa6082c6c993706fe7c8d03a76db84', 42),
}
TAGS = ('with_gvisor with_quic with_wireguard with_utls with_naive_outbound with_clash_api '
        'with_usbip with_openvpn with_openconnect badlinkname tfogo_checklinkname0 with_tailscale '
        'ts_omit_logtail ts_omit_ssh ts_omit_drive ts_omit_taildrop ts_omit_webclient ts_omit_doctor '
        'ts_omit_capture ts_omit_kube ts_omit_aws ts_omit_synology ts_omit_bird').split()
ANDROID = {'ndkRevision': '30.0.16138531', 'ndkSourceRevision': '30.0.16138531-beta3',
           'ndkPackage': 'android-ndk-r30-beta3',
           'bootclasspath': {'platform': 'android-36', 'sha256': 'd9eb9da824d9e247a352f570f01e1169e725b2954bca9e283a71786c59b59f9a'}}
BUILD = ['bash', 'scripts/build-android-apk.sh', '--debug', '--apk', '--split-per-abi',
         '--target', 'aarch64', '--ci', '--config', 'src-tauri/tauri.android.conf.json']
FORMAL_HASH = '139780a725a46694e783e458d2ab8666d6fbb2d7a5c00eed7958828948a1df4d'
OVERRIDES = set(('GOFLAGS GOENV GOWORK GOTOOLCHAIN GOOS GOARCH GOARM GOARM64 GOEXPERIMENT '
                 'GOBIN GOMODCACHE GOCACHE GONOSUMDB GOPRIVATE GONOPROXY GOPROXY GOSUMDB GOINSECURE CC CXX CPP AR LD CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH '
                 'LIBRARY_PATH COMPILER_PATH GCC_EXEC_PREFIX JAVA_TOOL_OPTIONS _JAVA_OPTIONS '
                 'JDK_JAVA_OPTIONS GRADLE_OPTS JAVA_OPTS GRADLE_USER_HOME ORG_GRADLE_PROJECT '
                 'RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER '
                 'CARGO_TARGET_DIR CARGO_BUILD_TARGET ANDROID_NDK_HOME ANDROID_NDK_ROOT NDK_HOME '
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
        if key in {'ANDROID_NDK_HOME', 'ANDROID_NDK_ROOT'} and value:
            expected = Path(env['ANDROID_HOME']).resolve(strict=True) / 'ndk/27.3.13750724'
            require(Path(value).resolve(strict=True) == expected.resolve(strict=True) and
                    re.search(r'^Pkg.Revision\s*=\s*27[.]3[.]13750724\s*$', (expected / 'source.properties').read_text(), re.M),
                    'ambient runner Rust NDK differs: ' + key)
            continue
        forbidden = key in OVERRIDES or key.startswith(('CARGO_PROFILE_', 'CGO_', 'PKG_CONFIG_', 'ORG_GRADLE_PROJECT_', 'TAURI_'))
        forbidden |= key.startswith('POLARIS_') and key not in {'POLARIS_LIGHT_BUILD', 'POLARIS_PUBLISH_RELEASE'}
        # The candidate SDK step alone writes this selected, exact Rust NDK.
        if prepared and key == 'NDK_RUST':
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
    archive = decode((ROOT / 'scripts/libbox-patches/ndk-linux-archive.json').read_bytes())
    require(archive['url'] == 'https://dl.google.com/android/repository/android-ndk-r30-beta3-linux.zip' and
            archive['sha256'] == '2698cca1e9f161048ecd84e1e70a556e1aa00b78409473d4c1e87969d40c3efc' and
            archive['sha1'] == 'f66e852ac01a57f74bcf6fd96dcea04dda5ea392' and archive['bytes'] == 738755979,
            'pinned official NDK archive differs')
    return archive


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
    # exact package license is already accepted; never accept a preview license.
    archive = ndk_archive_contract()
    url = 'https://dl.google.com/android/repository/repository2-3.xml'
    with urlopen(url, timeout=45) as response:
        require(response.geturl() == url, 'SDK metadata redirect refused')
        raw = response.read(8 * 1024 * 1024 + 1)
    require(len(raw) <= 8 * 1024 * 1024, 'SDK metadata exceeds bound')
    xml = ET.fromstring(raw)
    packages = [el for el in xml if el.tag.rsplit('}', 1)[-1] == 'remotePackage' and
                any(node.tag.rsplit('}', 1)[-1] == 'url' and node.text == archive['url'].rsplit('/', 1)[1] for node in el.iter())]
    require(len(packages) == 1, 'exact official NDK package metadata unavailable')
    package = packages[0]
    downloads = [el for el in package.iter() if el.tag.rsplit('}', 1)[-1] == 'complete' and
                 any(node.tag.rsplit('}', 1)[-1] == 'url' and node.text == archive['url'].rsplit('/', 1)[1] for node in el)]
    require(len(downloads) == 1, 'exact NDK archive metadata unavailable')
    fields = {el.tag.rsplit('}', 1)[-1]: el.text for el in downloads[0]}
    require(fields.get('checksum') == archive['sha1'] and fields.get('size') == str(archive['bytes']), 'official NDK archive metadata differs')
    facts['libboxNdk'] = {'metadataSha256': sha(raw), 'archive': archive,
                          'licenses': accepted_licenses(xml, package, home)}
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


def source_facts(source, generated=False):
    require(git(source, 'rev-parse', 'HEAD') == KERNEL_COMMIT and git(source, 'rev-parse', 'HEAD^{tree}') == KERNEL_TREE,
            'kernel commit/tree differs')
    clean(source, generated=generated)
    for name, digest in SOURCE_HASHES.items():
        require(file_pin(source / name)['sha256'] == digest, 'kernel file differs: ' + name)
    from source_policy import source_module_declarations
    directives, dependencies, replacements = source_module_declarations((source / 'go.mod').read_bytes())
    require(directives == {'module': 'github.com/sagernet/sing-box', 'go': '1.27.0', 'toolchain': 'go1.27.2'}, 'kernel Go directives differ')
    local = {key: target[0] for key, (version, target) in replacements.items() if len(target) == 1 and not version}
    require(local == {key: './third_party/' + row[0] for key, row in VENDORS.items()}, 'exact nine local replacements required')
    vendors = {}
    for name, (directory, tree, count) in VENDORS.items():
        prefix = 'third_party/' + directory
        require(git(source, 'rev-parse', 'HEAD:' + prefix) == tree, 'vendor subtree differs')
        rows = git(source, 'ls-tree', '-r', 'HEAD', '--', prefix).splitlines()
        require(len(rows) == count, 'vendor inventory differs')
        for row in rows:
            info, path = row.split('\t'); mode, kind, blob = info.split()
            target = source / path
            if mode == '120000':
                require(path == 'third_party/tailscale/.stignore' and target.is_symlink() and os.readlink(target) == '.gitignore', 'unexpected vendor symlink')
                raw = os.readlink(target).encode()
            else:
                require(mode in ('100644', '100755') and target.is_file() and not target.is_symlink() and
                        bool(target.stat().st_mode & 0o111) == (mode == '100755'), 'vendor mode differs')
                require(target.stat().st_size <= 32 * 1024 * 1024, 'vendor file exceeds byte budget')
                raw = target.read_bytes()
            require(kind == 'blob' and hashlib.sha1(b'blob ' + str(len(raw)).encode() + b'\0' + raw).hexdigest() == blob, 'vendor bytes differ')
        vendors[name] = {'path': './' + prefix, 'tree': tree, 'files': count}
    return {'repository': KERNEL_REPOSITORY, 'repositoryId': KERNEL_REPOSITORY_ID,
            'commit': KERNEL_COMMIT, 'tree': KERNEL_TREE, 'files': SOURCE_HASHES, 'vendors': vendors,
            'trackedMirror': tracked_mirror(source)}


def graph_facts(graph, source):
    require(len(graph) == len({row['Path'] for row in graph}), 'duplicate module graph row')
    local = {r['Path']: r for r in graph if r.get('Replace') and not r['Replace'].get('Version')}
    require(set(local) == set(VENDORS), 'actual graph replacements differ')
    for name, row in local.items():
        replacement = row['Replace']; expected = './third_party/' + VENDORS[name][0]
        require(replacement['Path'] == expected and Path(replacement['Dir']).resolve(strict=True) == (source / expected).resolve(strict=True), 'local graph path differs')
        require(not row.get('Sum') and not replacement.get('Sum') and not replacement.get('Version'), 'local replacement must not invent h1/version')
    return graph


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
    declarations = {
        'CommandServer': [('startOrReloadServiceWithRetryableRuntimePersistence', '(Ljava/lang/String;Lio/nekohasekai/libbox/OverrideOptions;Lio/nekohasekai/libbox/RuntimePersistenceHandler;)V')],
        'RuntimePersistenceHandler': [('runtimePersistenceMode', '()Ljava/lang/String;'), ('persistRuntimeConfiguration', '(Ljava/lang/String;JLjava/lang/String;Ljava/lang/String;)V')],
        'Libbox$proxyRuntimePersistenceHandler': [('runtimePersistenceMode', '()Ljava/lang/String;'), ('persistRuntimeConfiguration', '(Ljava/lang/String;JLjava/lang/String;Ljava/lang/String;)V')],
        'OverrideOptions': [('io.nekohasekai.libbox.OverrideOptions', '()V')],
    }
    java = {}
    for name, methods in declarations.items():
        text = run([tools['javap'], '-s', '-classpath', classes, 'io.nekohasekai.libbox.' + name])
        for method, descriptor in methods:
            modifier = 'public native ' if name == 'CommandServer' else ('public abstract ' if name == 'RuntimePersistenceHandler' else 'public ')
            require(re.search(re.escape(modifier) + r'(?!static\b)[^\n]*\b' + re.escape(method) + r'\([^\n]*\)[^\n]*;\s*descriptor: ' + re.escape(descriptor), text), 'actual Java method/descriptor missing: ' + method)
        if name == 'RuntimePersistenceHandler':
            require('public interface io.nekohasekai.libbox.RuntimePersistenceHandler' in text, 'handler must be an interface')
        if name in ('CommandServer', 'RuntimePersistenceHandler'):
            method = 'startOrReloadServiceWithRetryableRuntimePersistence' if name == 'CommandServer' else 'persistRuntimeConfiguration'
            require(re.search(re.escape(method) + r'\([^\n]*\) throws java.lang.Exception;', text), 'Java error channel missing')
        java[name] = text
    symbols = run([tools['readelf'], '--dyn-syms', '--wide', so])
    exported = {m[1] for m in re.finditer(r'GLOBAL\s+DEFAULT\s+(?!UND\b)\S+\s+(\S+)', symbols)}
    expected = {'JNI_OnLoad', 'Java_io_nekohasekai_libbox_CommandServer_startOrReloadServiceWithRetryableRuntimePersistence',
                'Java_io_nekohasekai_libbox_Libbox_00024proxyRuntimePersistenceHandler_persistRuntimeConfiguration',
                'Java_io_nekohasekai_libbox_Libbox_00024proxyRuntimePersistenceHandler_runtimePersistenceMode'}
    require(expected <= exported, 'actual JNI exports missing')
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
        require(decode((evidence / 'sdk.json').read_bytes(), 1024 * 1024)['libboxNdk']['archive'] == ndk_archive_contract(), 'NDK license/metadata admission missing')
        ndk = Path(run([sys.executable, ROOT / 'scripts/libbox-patches/prepare-ndk-linux.py', root]).strip())
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
        aar = work / 'libbox-a567-arm64.aar'
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
        clean(ROOT, allow_ignored=True)
        require(git(ROOT, 'rev-parse', 'HEAD') == admission['candidate'], 'App candidate changed during build')
        require(file_pin(ROOT / 'scripts/mobile-core/source-contract.json')['sha256'] == FORMAL_HASH, 'formal pin changed')
        result['status'] = 'compiled-and-statically-verified'
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
