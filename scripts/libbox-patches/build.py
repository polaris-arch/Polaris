#!/usr/bin/env python3
"""Retired Android producer; retained helpers inspect historical component receipts."""
import argparse
import hashlib
import importlib.util
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

_helper_spec = importlib.util.spec_from_file_location('android_source', PATCH_DIR / 'android-source.py')
android = importlib.util.module_from_spec(_helper_spec)
_helper_spec.loader.exec_module(android)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def run(args, *, cwd=None, env=None, capture=False):
    return subprocess.run(args, cwd=cwd, env=env, check=True, text=True,
                          stdout=subprocess.PIPE if capture else None).stdout


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def libbox_test_tags(manifest, optional=''):
    # Explicit libbox tests construct real Tailscale endpoints through include.
    # Keep this host registry capability separate from SDK/native feature tags.
    required = [tag for tag in manifest['buildTags'] if tag == 'with_tailscale']
    require(required == ['with_tailscale'], 'Host libbox tests require the pinned Tailscale registry capability')
    require(optional in ('', 'with_ccm', 'with_ocm', 'with_ccm,with_ocm'),
            'Unknown optional registry test profile')
    return ','.join(required + (optional.split(',') if optional else []))


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


def inspect_aar(aar, checkout, manifest, core, policy, tool, source_receipt):
    """Read-only artifact predicate shared by fresh producers and cached consumers."""
    android.validate_source_receipt(source_receipt, manifest, core)
    shared = android.provider()
    go, jdk, ndk, version = tool['go'], tool['jdk'], tool['ndk'], core['sourceBuild']['version']
    receipt = {'nativeLibraries': {}}
    readelf = ndk / 'toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf'
    with zipfile.ZipFile(aar) as archive:
        require(len(archive.namelist()) == len(set(archive.namelist())), 'Duplicate AAR entries')
        native = sorted(name for name in archive.namelist() if re.fullmatch(r'jni/[^/]+/[^/]+\.so', name))
        abis = sorted({name.split('/')[1] for name in native})
        require(abis == sorted(manifest['abis']), f'Unexpected ABI set: {abis}')
        require(all(f'jni/{abi}/libbox.so' in native for abi in abis), 'Missing libbox ABI')
        for name in native:
            data = archive.read(name)
            target = policy['abis'][name.split('/')[1]]
            require(data[4] == target['elfClass'] and struct.unpack_from('<H', data, 18)[0] == target['elfMachine'], f'{name} ELF ABI differs')
            loads = elf_loads(data)
            require(all(load['alignment'] >= manifest['minimumLoadAlignment'] and (load['virtualAddress'] - load['offset']) % manifest['minimumLoadAlignment'] == 0 for load in loads), f'{name} lacks 16K PT_LOAD alignment')
            so = checkout / (name.replace('/', '_'))
            so.write_bytes(data)
            dynamic = run([str(readelf), '-d', str(so)], capture=True)
            needed = re.findall(r'Shared library: \[([^]]+)\]', dynamic)
            for library in needed:
                require(library in {'liblog.so', 'libdl.so', 'libm.so', 'libandroid.so', 'libc.so'} or f'jni/{name.split("/")[1]}/{library}' in native, f'{name} requires unbundled runtime {library}')
            info = ''
            binary_facts = {}
            if name.endswith('/libbox.so'):
                binary_facts = shared.verify_binary(go, so, source_receipt, policy['abis'][name.split('/')[1]]['patchedModules']['requiredLinked'])
                info = binary_facts['goBuildInfo']
                parsed = android.validate_binary(info, name.split('/')[1], manifest, core, policy)
                binary_facts['parsedBuildInfo'] = parsed
                binary_facts['buildInfoFingerprint'] = android.digest(android.canonical(parsed))
                require('go' + manifest['goVersion'] in info, 'SO Go version differs from pinned toolchain')
                tags = re.search(r'build\s+-tags=([^\n]+)', info)
                require(tags and set(tags.group(1).split(',')) == set(manifest['buildTags']), 'SO feature tags mismatch')
                require(version.encode() in data, 'SO version marker missing')
                require(data.count(b'cronet') > 100 and data.count(b'naive') > 10, 'naive/cronet feature evidence missing')
            receipt['nativeLibraries'][name] = {'sha256': sha256(data), 'bytes': len(data), 'ptLoad': loads, 'needed': needed, 'goBuildInfo': info.strip(), **binary_facts}
        classes = checkout / 'classes.jar'
        classes.write_bytes(archive.read('classes.jar'))
        signatures = {}
        for name in ['Libbox', 'CommandServer', 'PlatformInterface', 'ConfigValidationResult', 'ExchangeContext', 'LocalDNSTransport', 'Func']:
            signatures[name] = run([str(jdk / 'bin/javap'), '-constants', '-classpath', str(classes), f'io.nekohasekai.libbox.{name}'], capture=True)
        require('newTransientCommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface)' in signatures['Libbox'], 'Transient Java factory is missing')
        require('newStrictCommandServer(io.nekohasekai.libbox.CommandServerHandler, io.nekohasekai.libbox.PlatformInterface)' in signatures['Libbox'], 'Strict primary Java factory is missing')
        require('java.lang.String interfaceUpdateListenerIdentity(io.nekohasekai.libbox.InterfaceUpdateListener)' in signatures['Libbox'], 'Native monitor incarnation helper is missing')
        require('startOrReloadService(java.lang.String, io.nekohasekai.libbox.OverrideOptions)' in signatures['CommandServer'], 'Service Java signature changed')
        require('void bindInterfaceControl(int, java.lang.String) throws java.lang.Exception' in signatures['PlatformInterface'], 'Named interface Java platform contract is missing')
        require('checkConfigWithResult(java.lang.String, java.lang.String, long)' in signatures['Libbox'], 'Bound config validation Java API is missing')
        require('ConfigValidationContractVersion = "polaris-validation-v1"' in signatures['Libbox'], 'Config validation contract version differs')
        require('java.lang.String version()' in signatures['Libbox'], 'Libbox JNI version getter is missing')
        for getter in ['RequestID', 'ConfigDigest', 'ContractVersion', 'Validation', 'Cleanup', 'ValidationError', 'CleanupError']:
            require(f'java.lang.String get{getter}()' in signatures['ConfigValidationResult'], f'Config validation getter {getter} is missing')
            require(f'set{getter}(' not in signatures['ConfigValidationResult'], f'Config validation field {getter} must be read-only')
        # Accepted Go receiver APIs return only string; pinned gomobile emits
        # public native instance methods. Require declarations, not suffix text.
        for name, method in [('ConfigValidationResult', 'getTailscaleStoreRetirement'),
                             ('ConfigValidationResult', 'getTailscaleStoreMembership'),
                             ('CommandServer', 'exportTailscaleStoreRetirement')]:
            declaration = r'(?m)^[ \t]*public native java\.lang\.String ' + re.escape(method) + r'\(\);[ \t]*$'
            require(re.search(declaration, signatures[name]), f'Scoped Tailscale Java ABI differs: {name}.{method}')
        for getter in ['TailscaleStoreRetirement', 'TailscaleStoreMembership']:
            require(f'set{getter}(' not in signatures['ConfigValidationResult'], f'Config validation field {getter} must be read-only')
        dns_methods = {
            'ExchangeContext': ['void errnoCode(int)', 'void errorCode(int)',
                                'void onCancel(io.nekohasekai.libbox.Func)',
                                'void rawSuccess(byte[])', 'void success(java.lang.String)'],
            'LocalDNSTransport': ['void exchange(io.nekohasekai.libbox.ExchangeContext, byte[]) throws java.lang.Exception',
                                 'void lookup(io.nekohasekai.libbox.ExchangeContext, java.lang.String, java.lang.String) throws java.lang.Exception',
                                 'boolean raw()'],
            'Func': ['void invoke() throws java.lang.Exception'],
        }
        for name, methods in dns_methods.items():
            for method in methods:
                require(method in signatures[name], f'DNS Java ABI changed: {name}.{method}')
        receipt['javaInterfaces'] = signatures
        receipt['classes'] = {'sha256': sha256(classes.read_bytes()), 'bytes': classes.stat().st_size}
    receipt['aar'] = {'sha256': android.file_hash(aar), 'bytes': aar.stat().st_size}
    return receipt


def verify_component(aar, receipt, manifest, core, policy, tool, identity, scratch):
    require(receipt.get('schema') == 'polaris-android-libbox-receipt-v1', 'Old/stock component receipt rejected')
    require(android.match('[0-9a-f]{40}', receipt.get('componentProducerCandidate')), 'Component producer candidate missing')
    require(receipt.get('androidInput') == identity, 'Android producer input identity differs')
    require(receipt.get('sourceCommit') == manifest['sourceCommit'] and receipt.get('officialTag') == 'v' + core['bundledCoreVersion']
            and receipt.get('version') == core['sourceBuild']['version'], 'Upstream/patched version binding differs')
    require(receipt.get('validationCleanupContract') == {'version': 'polaris-validation-v1', 'constructedBoxCleanup': 'CleanupUnknown', 'exactCleanupEnabled': False}, 'Validation cleanup contract differs')
    android.validate_source_receipt(receipt['sourceReceipt'], manifest, core)
    require(receipt.get('linkerFlags') == android.linker_flags(core, android.provider(), receipt['sourceReceipt'])
            and receipt.get('buildVCS') is False and isinstance(receipt.get('tests'), str) and receipt['tests'], 'Producer bind/test contract differs')
    facts = {key: value for key, value in receipt.items() if key != 'outputFingerprint'}
    require(receipt.get('outputFingerprint') == android.digest(android.canonical(facts)), 'Component output fingerprint differs')
    actual = inspect_aar(aar, scratch, manifest, core, policy, tool, receipt['sourceReceipt'])
    expected_path = 'src-tauri/gen/android/app/libs/libbox.aar'
    require(receipt['aar'].get('path') == expected_path, 'AAR destination binding differs')
    require({key: value for key, value in receipt['aar'].items() if key != 'path'} == actual['aar'], 'AAR bytes differ')
    for field in ('nativeLibraries', 'javaInterfaces', 'classes'):
        # Raw BuildInfo keeps every real row; its filename header is transport-dependent.
        recorded, observed = receipt[field], actual[field]
        if field == 'nativeLibraries':
            def portable(rows):
                return {name: {key: value for key, value in row.items() if key != 'goBuildInfo'} for name, row in rows.items()}
            require(portable(recorded) == portable(observed), 'AAR native observations differ')
            for name, row in recorded.items():
                if name.endswith('/libbox.so'):
                    require(android.parse_build_info(row['goBuildInfo']) == observed[name]['parsedBuildInfo'], 'Stored full native BuildInfo differs')
        else:
            require(recorded == observed, 'AAR observation differs: ' + field)
    return receipt


def publish(aar, receipt, output, receipt_output):
    staged_aar, staged_receipt = output.with_suffix('.aar.tmp'), receipt_output.with_suffix('.json.tmp')
    try:
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(aar, staged_aar)
        require(android.file_hash(staged_aar) == receipt['aar']['sha256'], 'Staged AAR differs')
        staged_receipt.write_text(json.dumps(receipt, indent=2) + '\n')
        # A consumer rejects the transient mismatched pair. On failure restore both originals.
        with tempfile.TemporaryDirectory(prefix='.libbox-publish-', dir=output.parent) as saved:
            originals = {}
            for destination in (output, receipt_output):
                backup = Path(saved) / destination.name
                if destination.exists():
                    shutil.copyfile(destination, backup)
                    originals[destination] = backup
            try:
                staged_aar.replace(output)
                staged_receipt.replace(receipt_output)
            except OSError:
                for destination in (output, receipt_output):
                    if destination in originals:
                        originals[destination].replace(destination)
                    else:
                        destination.unlink(missing_ok=True)
                raise
    finally:
        staged_aar.unlink(missing_ok=True)
        staged_receipt.unlink(missing_ok=True)


def main():
    # Refuse before argument parsing, source/tool/cache access or publication.
    # Importable historical receipt predicates above deliberately remain available.
    raise RuntimeError(
        'Legacy Android libbox build is retired. Use bash scripts/build-mobile-core.sh android '
        'with the required source/tool/work/cache arguments; see scripts/mobile-core/README.md.')



if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print(f'libbox build failed: {error}', file=sys.stderr)
        sys.exit(1)
