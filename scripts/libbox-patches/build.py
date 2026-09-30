#!/usr/bin/env python3
"""Build the pinned Android libbox without changing the supplied source repository."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

PATCH_DIR = Path(__file__).resolve().parent
ROOT = PATCH_DIR.parent.parent


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def run(args, *, cwd=None, env=None, capture=False):
    return subprocess.run(args, cwd=cwd, env=env, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None).stdout


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def elf_loads(data):
    require(data[:4] == b'\x7fELF' and data[5] == 1, 'Expected little-endian ELF')
    is64 = data[4] == 2
    header = struct.unpack_from('<HHIQQQIHHHHHH' if is64 else '<HHIIIIIHHHHHH', data, 16)
    offset, size, count = header[4], header[8], header[9]
    loads = []
    for index in range(count):
        segment = struct.unpack_from('<IIQQQQQQ' if is64 else '<IIIIIIII', data, offset + index * size)
        if segment[0] == 1:
            file_offset, address, alignment = (segment[2], segment[3], segment[7]) if is64 else (segment[1], segment[2], segment[7])
            loads.append({'offset': file_offset, 'virtualAddress': address, 'alignment': alignment})
    require(loads, 'ELF contains no PT_LOAD segments')
    return loads


def main():
    require(len(sys.argv) <= 2, 'Usage: bash scripts/build-libbox.sh [local sing-box repository]')
    manifest_path = PATCH_DIR / 'source-manifest.json'
    manifest = json.loads(manifest_path.read_text())
    version = json.loads((ROOT / 'src-tauri/core-manifest.json').read_text())['bundledCoreVersion']
    commit = manifest['sourceCommit']
    require(re.fullmatch('[0-9a-f]{40}', commit), 'Invalid pinned source commit')
    source = Path(sys.argv[1]).expanduser().resolve() if len(sys.argv) == 2 else Path.home() / 'Code/sing-box-v1150a8'
    require(source.is_dir(), f'Local source repository missing: {source}; pass its path explicitly')
    require(run(['git', '-C', str(source), 'rev-parse', f'v{version}^{{commit}}'], capture=True).strip() == commit,
            'Android source commit does not match the core-manifest official version tag')
    for patch in manifest['patches']:
        require(Path(patch['file']).name == patch['file'], 'Patch path must remain in libbox-patches')
        require(sha256((PATCH_DIR / patch['file']).read_bytes()) == patch['sha256'], 'Pinned patch hash mismatch')

    # Prefer the cached exact toolchain; do not let go auto-download another version.
    cached_go = Path.home() / f"go/pkg/mod/golang.org/toolchain@v0.0.1-go{manifest['goVersion']}.linux-amd64/bin/go"
    go = Path(os.environ.get('LIBBOX_GO', str(cached_go if cached_go.is_file() else shutil.which('go') or '')))
    require(go.is_file(), 'Pinned Go toolchain is unavailable')
    go_version = run([str(go), 'version'], capture=True).strip()
    require(f" go{manifest['goVersion']} " in go_version, f'Incorrect Go toolchain: {go_version}')
    jdk = Path(os.environ.get('JDK17', str(Path.home() / '.local/jdk17')))
    java_version = run([str(jdk / 'bin/java'), '--version'], capture=True).strip()
    require(f"openjdk {manifest['javaMajor']}" in java_version, 'libbox requires the pinned JDK 17')
    sdk = Path(os.environ.get('ANDROID_HOME', str(Path.home() / 'Android/Sdk')))
    ndk = Path(os.environ.get('LIBBOX_NDK_HOME', str(sdk / 'ndk' / manifest['ndkDirectory'])))
    properties = (ndk / 'source.properties').read_text()
    require(re.search(r'^Pkg.Revision\s*=\s*' + re.escape(manifest['ndkVersion']) + r'\s*$', properties, re.M), 'NDK revision does not match Android source manifest')
    mobile_tools = {}
    for name in ['gomobile', 'gobind']:
        binary = Path.home() / 'go/bin' / name
        build_info = run([str(go), 'version', '-m', str(binary)], capture=True)
        require(f"github.com/sagernet/gomobile\t{manifest['gomobileVersion']}\t" in build_info, f'{name} must use the pinned SagerNet gomobile fork')
        mobile_tools[name] = {'version': manifest['gomobileVersion'], 'sha256': sha256(binary.read_bytes()), 'buildInfo': build_info.strip()}
    env = os.environ.copy()
    env.update({'JAVA_HOME': str(jdk), 'ANDROID_HOME': str(sdk), 'ANDROID_NDK_HOME': str(ndk),
                'NDK_HOME': str(ndk), 'GOTOOLCHAIN': 'local', 'GOWORK': 'off', 'GOFLAGS': '',
                'GOEXPERIMENT': '', 'GOPROXY': 'off', 'GOSUMDB': 'off',
                'CGO_CFLAGS': '', 'CGO_CPPFLAGS': '', 'CGO_CXXFLAGS': '',
                'CGO_LDFLAGS': '-Wl,-z,max-page-size=16384 -Wl,-z,common-page-size=16384',
                'PATH': os.pathsep.join([str(go.parent), str(jdk / 'bin'), str(Path.home() / 'go/bin'), os.environ['PATH']])})
    for name in ['GOOS', 'GOARCH', 'GOARM', 'GOAMD64', 'CC', 'CXX', 'CGO_ENABLED']:
        env.pop(name, None)
    tmp_root = Path('/var/tmp/polaris-libbox')
    tmp_root.mkdir(exist_ok=True)
    env['TMPDIR'] = str(tmp_root)
    # A fresh checkout under ~/Code keeps shared core trees untouched and patches repeatable.
    with tempfile.TemporaryDirectory(prefix='polaris-libbox-build-', dir=Path.home() / 'Code') as checkout_name:
        checkout = Path(checkout_name)
        run(['git', 'clone', '--shared', '--no-checkout', str(source), str(checkout)])
        run(['git', 'checkout', '--detach', commit], cwd=checkout)
        require(re.search(r'^go\s+' + re.escape(manifest['goVersion']) + r'\s*$', (checkout / 'go.mod').read_text(), re.M), 'Source Go requirement differs from pinned toolchain')
        tag_source = (checkout / 'cmd/internal/build_libbox/main.go').read_text()
        upstream_tags = []
        for expression in re.findall(r'(?m)^\s*sharedTags = append\(sharedTags, ([^\n]+)\)', tag_source):
            upstream_tags.extend(re.findall(r'"([^"\n]+)"', expression))
        require(upstream_tags == manifest['buildTags'], 'Android feature tags differ from upstream source')
        for patch in manifest['patches']:
            patch_path = str(PATCH_DIR / patch['file'])
            run(['git', 'apply', '--check', patch_path], cwd=checkout)
            run(['git', 'apply', patch_path], cwd=checkout)
        # Test the patched close chain as real package behavior before binding.
        # The libbox tests remain explicit because unrelated upstream tests may
        # require extra modules outside this pinned offline build.
        run([str(go), 'test', '-race', '-ldflags=-checklinkname=0', '-count=1', '.', './daemon',
             './adapter/endpoint', './adapter/inbound', './adapter/outbound',
             './adapter/service', './adapter/certificate', './common/construction',
             './common/certificate', './log', './route', './dns', './service/api',
             './service/ssmapi', './service/ccm', './service/ocm'], cwd=checkout, env=env)
        run([str(go), 'test', '-race', '-count=1', './experimental/clashmode'], cwd=checkout, env=env)
        files = run([str(go), 'list', '-f', '{{range .GoFiles}}{{$.Dir}}/{{.}} {{end}}', './experimental/libbox'], cwd=checkout, env=env, capture=True).split()
        run([str(go), 'test', '-race', '-ldflags=-checklinkname=0', '-count=1', *files,
             str(checkout / 'experimental/libbox/command_server_transient_test.go'),
             str(checkout / 'experimental/libbox/interface_binding_test.go'),
             str(checkout / 'experimental/libbox/config_validation_test.go'),
             str(checkout / 'experimental/libbox/config_construction_persistence_test.go'),
             str(checkout / 'experimental/libbox/dns_lifecycle_test.go')], cwd=checkout, env=env)
        # Optional registry services must preserve existing usage files with
        # either tag independently and with both real implementations present.
        for tags in ['with_ccm', 'with_ocm', 'with_ccm,with_ocm']:
            tagged_files = run([str(go), 'list', '-tags', tags, '-f',
                                '{{range .GoFiles}}{{$.Dir}}/{{.}} {{end}}',
                                './experimental/libbox'], cwd=checkout, env=env, capture=True).split()
            tagged_tests = [str(checkout / 'experimental/libbox/config_validation_test.go'),
                            str(checkout / 'experimental/libbox/config_construction_persistence_test.go'),
                            str(checkout / 'experimental/libbox/dns_lifecycle_test.go')]
            if tags == 'with_ccm,with_ocm':
                tagged_tests.append(str(checkout / 'experimental/libbox/config_optional_persistence_test.go'))
            run([str(go), 'test', '-tags', tags, '-race', '-ldflags=-checklinkname=0',
                 '-count=1', *tagged_files, *tagged_tests], cwd=checkout, env=env)
        run([str(go), 'test', '-ldflags=-checklinkname=0', '-count=1', './common/dialer'], cwd=checkout, env=env)
        linker_flags = (f'-X github.com/sagernet/sing-box/constant.Version={version} '
                        '-X runtime.godebugDefault=multipathtcp=0,tlssha1=1 '
                        '-checklinkname=0 -s -w -buildid= '
                        '-extldflags=-Wl,-z,max-page-size=16384,-z,common-page-size=16384')
        aar = checkout / 'libbox.aar'
        command = [str(Path.home() / 'go/bin/gomobile'), 'bind', '-o', str(aar), '-target', 'android',
                   '-androidapi', str(manifest['androidAPI']), '-javapkg=io.nekohasekai', '-libname=box',
                   '-trimpath', '-buildvcs=false', '-ldflags', linker_flags,
                   '-tags', ','.join(manifest['buildTags']), './experimental/libbox']
        print(f'Building Android libbox {version} from {commit}, NDK {manifest["ndkVersion"]}', flush=True)
        run(command, cwd=checkout, env=env)
        receipt = {'sourceCommit': commit, 'officialTag': f'v{version}', 'version': version,
                   'sourceManifestSha256': sha256(manifest_path.read_bytes()), 'buildScriptSha256': sha256(Path(__file__).read_bytes()), 'patches': manifest['patches'],
                   'toolchain': {'go': go_version, 'java': java_version, 'ndk': manifest['ndkVersion'], **mobile_tools},
                   'buildTags': manifest['buildTags'], 'androidAPI': manifest['androidAPI'], 'ndkSelectionReason': manifest['ndkSelectionReason'],
                   'linkerFlags': linker_flags, 'buildVCS': False,
                   'validationCleanupContract': {'version': 'polaris-validation-v1', 'constructedBoxCleanup': 'CleanupUnknown', 'exactCleanupEnabled': False},
                   'tests': 'Construction rollback at each service constructor, constructor error+object/internal acquire, cancellation and panic, six unstarted manager child close/error/panic chains, parse rejection NoConstruction, bound validation/cleanup result, cleanup timeout/late completion with immutable receipt, and these under race detector; Box early-close result, five manager close errors, API listener close ownership, strict transient and primary terminal/sticky lifecycle, concurrent listener close barrier, transient HTTP CONNECT rejects missing/wrong auth, normal CommandServer lifecycle, named interface TCP/UDP binding and failures, dialer regressions, explicit clash default versus stale cache, and concurrent clash mode switching under Go race detector passed',
                   'nativeLibraries': {}}
        readelf = ndk / 'toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf'
        with zipfile.ZipFile(aar) as archive:
            native = sorted(name for name in archive.namelist() if re.fullmatch(r'jni/[^/]+/[^/]+\.so', name))
            abis = sorted({name.split('/')[1] for name in native})
            require(abis == manifest['abis'], f'Unexpected ABI set: {abis}')
            require(all(f'jni/{abi}/libbox.so' in native for abi in abis), 'Missing libbox ABI')
            for name in native:
                data = archive.read(name)
                loads = elf_loads(data)
                require(all(load['alignment'] >= manifest['minimumLoadAlignment'] and (load['virtualAddress'] - load['offset']) % manifest['minimumLoadAlignment'] == 0 for load in loads), f'{name} lacks 16K PT_LOAD alignment')
                so = checkout / Path(name).name
                so.write_bytes(data)
                dynamic = run([str(readelf), '-d', str(so)], capture=True)
                needed = re.findall(r'Shared library: \[([^]]+)\]', dynamic)
                for library in needed:
                    require(library in {'liblog.so', 'libdl.so', 'libm.so', 'libandroid.so', 'libc.so'} or f'jni/{name.split("/")[1]}/{library}' in native, f'{name} requires unbundled runtime {library}')
                info = run([str(go), 'version', '-m', str(so)], capture=True)
                if name.endswith('/libbox.so'):
                    require('go' + manifest['goVersion'] in info, 'SO Go version differs from pinned toolchain')
                    tags = re.search(r'build\s+-tags=([^\n]+)', info)
                    require(tags and set(tags.group(1).split(',')) == set(manifest['buildTags']), 'SO feature tags mismatch')
                    require(version.encode() in data, 'SO version marker missing')
                    require(data.count(b'cronet') > 100 and data.count(b'naive') > 10, 'naive/cronet feature evidence missing')
                receipt['nativeLibraries'][name] = {'sha256': sha256(data), 'bytes': len(data), 'ptLoad': loads, 'needed': needed, 'goBuildInfo': info.strip()}
            classes = checkout / 'classes.jar'
            classes.write_bytes(archive.read('classes.jar'))
            signatures = {}
            for name in ['Libbox', 'CommandServer', 'PlatformInterface', 'ConfigValidationResult']:
                signatures[name] = run([str(jdk / 'bin/javap'), '-constants', '-classpath', str(classes), f'io.nekohasekai.libbox.{name}'], capture=True)
            require('newTransientCommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface)' in signatures['Libbox'], 'Transient Java factory is missing')
            require('newStrictCommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface)' in signatures['Libbox'], 'Strict primary Java factory is missing')
            require('startOrReloadService(java.lang.String, io.nekohasekai.libbox.OverrideOptions)' in signatures['CommandServer'], 'Service Java signature changed')
            require('void bindInterfaceControl(int, java.lang.String) throws java.lang.Exception' in signatures['PlatformInterface'], 'Named interface Java platform contract is missing')
            require('checkConfigWithResult(java.lang.String, java.lang.String, long)' in signatures['Libbox'], 'Bound config validation Java API is missing')
            require('ConfigValidationContractVersion = "polaris-validation-v1"' in signatures['Libbox'], 'Config validation contract version differs')
            for getter in ['RequestID', 'ConfigDigest', 'ContractVersion', 'Validation', 'Cleanup', 'ValidationError', 'CleanupError']:
                require(f'java.lang.String get{getter}()' in signatures['ConfigValidationResult'], f'Config validation getter {getter} is missing')
                require(f'set{getter}(' not in signatures['ConfigValidationResult'], f'Config validation field {getter} must be read-only')
            receipt['javaInterfaces'] = signatures
        output = ROOT / 'src-tauri/gen/android/app/libs/libbox.aar'
        output.parent.mkdir(parents=True, exist_ok=True)
        receipt['aar'] = {'path': str(output.relative_to(ROOT)), 'sha256': sha256(aar.read_bytes()), 'bytes': aar.stat().st_size}
        # Publish only after every artifact check succeeds.
        temp_output = output.with_suffix('.aar.tmp')
        shutil.copyfile(aar, temp_output)
        temp_output.replace(output)
        (PATCH_DIR / 'build-receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print(f'Validated {abis}, 16K PT_LOAD, tags and Java API: {receipt["aar"]["sha256"]}', flush=True)


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'libbox build failed: {error}', file=sys.stderr)
        sys.exit(1)
