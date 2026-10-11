#!/usr/bin/env python3
"""CI-only evidence from final deb/AppImage/ZIP and actual temporary NSIS install.
Never run an App UI, uninstaller, UAC cleaner or host device configuration.
"""
import argparse
import json
import os
from pathlib import Path, PurePosixPath
import stat
import struct
import re
import unicodedata
import subprocess
import tempfile
import zipfile
import importlib.util
import hashlib
import shutil
os.sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('native_payload_proof', Path(__file__).with_name('native-payload-proof.py'))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)
ci_context, prove, regular, require, sha = (getattr(native, n) for n in ['ci_context', 'prove', 'regular', 'require', 'sha'])


def payload_files(directory, key):
    cores = [p for p in directory.rglob('*') if p.name in ['sing-box', 'sing-box.exe']]
    libs = [p for p in directory.rglob('*') if p.name in ['libcronet.so', 'libcronet.dll']]
    require(len(cores) == len(libs) == 1, 'final artifact must contain exactly one core/library')
    core = regular(cores[0]); lib = regular(libs[0])
    require(core.name == ('sing-box.exe' if key == 'win' else 'sing-box')
            and lib.name == ('libcronet.dll' if key == 'win' else 'libcronet.so')
            and core.parent == lib.parent, 'wrong-platform or split final payload')
    return core, lib


def windows_zip_key(name, is_directory):
    # ZIP paths are canonical POSIX names, then compared with Windows aliases.
    # Reject first: extractall otherwise normalizes dot/repeated separators and
    # Windows trims trailing dots/spaces, allowing overwrite before counting.
    require(isinstance(name, str) and name and '\\' not in name and ':' not in name
            and not name.startswith('/'), 'unsafe ZIP member')
    path = name[:-1] if is_directory and name.endswith('/') else name
    parts = path.split('/')
    require(parts and all(part and part not in ['.', '..'] for part in parts)
            and name == '/'.join(parts) + ('/' if is_directory else ''), 'non-canonical ZIP path')
    for part in parts:
        require(part == part.rstrip(' .') and not any(ord(c) < 32 for c in part)
                and not any(c in '<>"|?*' for c in part)
                and unicodedata.normalize('NFC', part) == part, 'Windows ZIP path alias')
        base = part.split('.', 1)[0].upper()
        require(not re.fullmatch(r'(?:CON|PRN|AUX|NUL|COM[1-9¹²³]|LPT[1-9¹²³])', base), 'Windows ZIP device name')
    return '/'.join(part.casefold() for part in parts)


def extract_zip(archive, destination):
    with zipfile.ZipFile(archive) as z:
        members = {}; implied_directories = set()
        for member in z.infolist():
            mode = member.external_attr >> 16
            require(not stat.S_ISLNK(mode), 'unsafe ZIP symlink')
            # orig_filename retains an embedded NUL that ZipInfo.filename trims.
            require(member.orig_filename == member.filename and '\0' not in member.orig_filename, 'ZIP NUL name')
            key = windows_zip_key(member.filename, member.is_dir())
            require(key not in members, 'ZIP normalized alias collision')
            members[key] = member.is_dir()
            implied_directories.update('/'.join(key.split('/')[:i]) for i in range(1, len(key.split('/'))))
        require(all(members.get(key, True) for key in implied_directories), 'ZIP file/directory alias collision')
        # All path and collision checks finish before the first member is written.
        z.extractall(destination)
    require((destination / 'portable.marker').is_file() and (destination / 'polaris.exe').is_file(), 'final ZIP marker/app missing')
    for name in ['LICENSE', 'NOTICE', 'THIRD-PARTY-LICENSES.md']:
        require((destination / name).is_file() and (destination / name).stat().st_size > 0, 'final ZIP license missing')


def squashfs_offset(path):
    data = path.read_bytes(); offsets = []; start = 0
    while True:
        at = data.find(b'hsqs', start)
        if at < 0: break
        start = at + 4
        if at + 96 > len(data): continue
        inodes, block = struct.unpack_from('<I4xI', data, at + 4)
        major, minor = struct.unpack_from('<HH', data, at + 28)
        used = struct.unpack_from('<Q', data, at + 40)[0]
        if inodes > 0 and block in [1 << n for n in range(12, 21)] and major == 4 and minor == 0 and 96 <= used <= len(data) - at:
            offsets.append(at)
    require(len(offsets) == 1, 'final AppImage lacks unique valid SquashFS superblock')
    return offsets[0]


def pwsh(code, args=()):
    return subprocess.check_output(['pwsh', '-NoProfile', '-NonInteractive', '-Command', code, *map(str, args)], timeout=180)


def windows_signature(path):
    return native.observe_unsigned_windows(path)


TAURI_CLI_VERSION = '2.12.1'
BUNDLE_PREFIX = b'__TAURI_BUNDLE_TYPE_VAR_'
BUNDLE_UNK = BUNDLE_PREFIX + b'UNK'
BUNDLE_NSS = BUNDLE_PREFIX + b'NSS'
TAURI_PATCH_SOURCE = 'https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.1/crates/tauri-bundler/src/bundle.rs'


def expected_windows_app(source, kind, version):
    require(version == TAURI_CLI_VERSION, 'unreviewed Tauri CLI app transform version')
    require(kind in ['nsis', 'zip'], 'unknown Windows app carrier')
    if kind == 'zip': return source
    # Fixed 2.12.1 bundler: patch first UNK token to NSS, bundle, restore source.
    # Refuse ambiguity rather than accepting its first-match behavior generally.
    # Runtime comparison constants (NSS/MSI/etc.) share the prefix and remain
    # ordinary protected bytes. Only the patch target UNK must be unique.
    require(source.count(BUNDLE_UNK) == 1,
            'NSIS source requires one unique unpatched bundle token')
    offset = source.index(BUNDLE_UNK)
    return source[:offset] + BUNDLE_NSS + source[offset + len(BUNDLE_UNK):]


def byte_differences(before, after):
    # Complete contiguous difference ranges, with both original byte sequences.
    # Full samples are retained as well; no offset is ignored during comparison.
    if before == after: return []
    result = []; start = None; shared = min(len(before), len(after)); total = max(len(before), len(after))
    for offset in range(total + 1):
        differs = offset < total and (offset >= shared or before[offset] != after[offset])
        if differs and start is None: start = offset
        if not differs and start is not None:
            result.append({'offset': start, 'endExclusive': offset,
                           'beforeHex': before[start:offset].hex(), 'afterHex': after[start:offset].hex()})
            start = None
    return result


def pe_certificate_table(data):
    require(len(data) >= 64 and data[:2] == b'MZ', 'App lacks PE DOS header')
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    require(pe + 24 <= len(data) and data[pe:pe+4] == b'PE\0\0', 'App lacks PE header')
    opt = pe + 24; size = struct.unpack_from('<H', data, pe + 20)[0]
    require(size >= 152 and opt + size <= len(data)
            and struct.unpack_from('<H', data, opt)[0] == 0x20b, 'App must be complete PE32+ amd64')
    require(struct.unpack_from('<H', data, pe + 4)[0] == 0x8664
            and struct.unpack_from('<I', data, opt + 108)[0] >= 5, 'App PE machine/directories differ')
    offset, length = struct.unpack_from('<II', data, opt + 112 + 4 * 8)
    return {'directoryEntryOffset': opt + 144, 'fileOffset': offset, 'bytes': length}


def app_bytes_observation(data, path=None):
    tokens = {}
    for token in [BUNDLE_UNK, BUNDLE_NSS, BUNDLE_PREFIX + b'MSI']:
        offsets = []; start = 0
        while True:
            at = data.find(token, start)
            if at < 0: break
            offsets.append(at); start = at + 1
        tokens[token.decode('ascii')] = {'count': len(offsets), 'offsets': offsets}
    result = {'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data), 'tokens': tokens}
    try: result['PECertificateTable'] = pe_certificate_table(data)
    except ValueError as error: result['PECertificateTableError'] = str(error)
    if path is not None:
        result.update(path=str(path), mtimeNs=path.stat().st_mtime_ns)
        try: result['Authenticode'] = windows_signature(path)
        except Exception as error: result['AuthenticodeError'] = str(error)
    return result


def write_app_evidence(directory, evidence):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'app-byte-evidence.json').write_text(json.dumps(evidence, indent=2) + '\n', encoding='utf-8')


def windows_app_baseline(root, kind, directory, evidence):
    source_path = regular(root / 'target/release/polaris.exe'); source = source_path.read_bytes()
    directory.mkdir(parents=True, exist_ok=True); (directory / 'source.exe').write_bytes(source)
    evidence['source'] = app_bytes_observation(source, source_path)
    version = (root / 'scripts/tauri-cli.version').read_text(encoding='utf-8').strip()
    actual_version = pwsh("$ErrorActionPreference='Stop'; & tauri --version; if($LASTEXITCODE -ne 0) { throw 'tauri version failed' }").decode('utf-8-sig').strip()
    evidence.update(pinnedTauriVersion=version, actualTauriVersion=actual_version, fixedTransformSource=TAURI_PATCH_SOURCE)
    require(actual_version == 'tauri-cli ' + TAURI_CLI_VERSION, 'actual Tauri CLI app transform version differs')
    expected = expected_windows_app(source, kind, version)
    expected_path = directory / 'expected.exe'; expected_path.write_bytes(expected)
    evidence['expected'] = app_bytes_observation(expected, expected_path)
    evidence['sourceToExpectedDifferences'] = byte_differences(source, expected)
    require(evidence['source'].get('Authenticode') == {'status': 'NotSigned', 'signerThumbprint': None}
            and pe_certificate_table(source)['fileOffset'] == pe_certificate_table(source)['bytes'] == 0,
            'source App must retain unsigned/no-certificate policy')
    if kind == 'nsis':
        script = regular(root / 'target/release/nsis/x64/installer.nsi')
        shutil.copyfile(script, directory / 'installer.nsi')
        text = script.read_text(encoding='utf-8-sig')
        paths = re.findall(r'^!define MAINBINARYSRCPATH "([^"\r\n]+)"\s*$', text, re.MULTILINE)
        evidence['generatedNSISMainFileInput'] = {'scriptPath': str(script), 'scriptSha256': sha(script), 'inputPaths': paths}
        require(len(paths) == 1 and '$' not in paths[0]
                and regular(paths[0]) == source_path
                and len(re.findall(r'^\s*File "\$\{MAINBINARYSRCPATH\}"\s*$', text, re.MULTILINE)) == 1,
                'generated NSIS main File input differs')
        evidence['generatedNSISMainFileInput'] = {'scriptPath': str(script), 'scriptSha256': sha(script), 'inputPath': paths[0]}
        # Preserve the portable member even when NSIS fails before ZIP deep proof.
        zips = list((root / 'dist-win').glob('*-Portable.zip'))
        require(len(zips) == 1, 'expected one Windows portable carrier for static evidence')
        with zipfile.ZipFile(regular(zips[0])) as archive:
            require(archive.namelist().count('polaris.exe') == 1, 'portable App member differs')
            portable = archive.read('polaris.exe')
        portable_path = directory / 'portable.exe'; portable_path.write_bytes(portable)
        evidence['portableStaticMember'] = app_bytes_observation(portable, portable_path)
        evidence['sourceToPortableDifferences'] = byte_differences(source, portable)
        # This is static App evidence; ZIP inventory and native proof still run separately.
        require(portable == source, 'portable App bytes differ from original release-profile source')
    write_app_evidence(directory, evidence)
    return source, expected


def record_windows_app(app, source, expected, directory, evidence):
    directory.mkdir(parents=True, exist_ok=True)
    actual = regular(app).read_bytes(); (directory / 'actual.exe').write_bytes(actual)
    evidence['actual'] = app_bytes_observation(actual, app)
    evidence['sourceToActualDifferences'] = byte_differences(source, actual)
    evidence['expectedToActualDifferences'] = byte_differences(expected, actual)
    evidence['completeFileBytesEqualExpected'] = actual == expected
    write_app_evidence(directory, evidence)
    return actual


def verify_windows_app(app, source, expected, directory, evidence):
    actual = record_windows_app(app, source, expected, directory, evidence)
    require(actual == expected, 'final App bytes differ from exact carrier-stage expectation')
    require(evidence['actual'].get('Authenticode') == {'status': 'NotSigned', 'signerThumbprint': None}
            and pe_certificate_table(actual)['fileOffset'] == pe_certificate_table(actual)['bytes'] == 0,
            'final App must retain unsigned/no-certificate policy')
    return {'schema': 'polaris-windows-carrier-stage-app-identity-v1',
            'sha256': sha(app), 'sourceSha256': hashlib.sha256(source).hexdigest(),
            'expectedSha256': hashlib.sha256(expected).hexdigest(),
            'sourceSignature': evidence['source']['Authenticode'], 'finalSignature': evidence['actual']['Authenticode'],
            'carrierStage': evidence['kind'], 'byteEvidence': evidence}


def nsis_install(artifact, destination, root):
    ci_context('win')
    require(not destination.exists() and destination.parent.is_dir(), 'NSIS requires new isolated prefix')
    conf = json.loads((root / 'src-tauri/tauri.conf.json').read_text(encoding='utf-8'))
    require(conf['bundle']['windows']['nsis']['installMode'] == 'currentUser'
            and conf['bundle']['windows']['certificateThumbprint'] is None, 'reviewed NSIS install/signature policy differs')
    # Disposable runner preconditions prevent an old installation/update/uninstall path.
    code = r'''$ErrorActionPreference='Stop'
if (Get-Process -Name polaris,sing-box,polaris-helper,polaris-cleaner -ErrorAction SilentlyContinue) { throw 'existing Polaris/core process' }
if (Get-Service -Name PolarisHelper -ErrorAction SilentlyContinue) { throw 'existing Polaris helper service' }
$roots=@('HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall','HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall','HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall')
foreach($r in $roots) { if(Test-Path $r) { foreach($p in Get-ChildItem $r) { $v=Get-ItemProperty $p.PSPath; if($v.DisplayName -match '(?i)polaris' -or $v.InstallLocation -match '(?i)polaris') { throw 'existing Polaris install registry entry' } } } }
$webview=$false
foreach($r in @('HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients','HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients','HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients')) { if(Test-Path $r) { foreach($p in Get-ChildItem $r) { $v=Get-ItemProperty $p.PSPath; if($v.name -eq 'Microsoft Edge WebView2 Runtime' -and $v.pv -and $v.pv -ne '0.0.0.0') { $webview=$true } } } }
if(-not $webview) { throw 'WebView2 must already exist; no bootstrapper download in payload proof' }
'''
    pwsh(code)
    # /D must be last. No /UPDATE, elevation, uninstall or app launch arguments.
    result = subprocess.run([str(artifact), '/S', '/D=' + str(destination)], timeout=180)
    require(result.returncode == 0 and destination.is_dir(), 'actual NSIS install failed')
    require(not (destination / 'portable.marker').exists(), 'NSIS retained portable form marker')
    require((destination / 'polaris.exe').is_file(), 'actual installed app missing')


def final_proof(root, key, artifact, kind, output):
    context = ci_context(key); artifact = regular(artifact); before = sha(artifact)
    require((key == 'linux' and kind in ['deb', 'AppImage']) or (key == 'win' and kind in ['nsis', 'zip']), 'wrong final artifact/platform')
    signature = windows_signature(artifact) if kind == 'nsis' else {'state': 'not-applicable', 'kind': kind}
    evidence = {'schema': 'polaris-windows-app-stage-bytes-v1', 'transport': context, 'kind': kind,
                'carrier': {'path': str(artifact), 'sha256': before, 'bytes': artifact.stat().st_size,
                            'mtimeNs': artifact.stat().st_mtime_ns, 'signature': signature}}
    directory = output.parent / 'app-bytes' / kind
    try:
        baseline = windows_app_baseline(root, kind, directory, evidence) if key == 'win' else None
        return extracted_final_proof(root, key, artifact, kind, output, context, before, signature, baseline, directory, evidence)
    except Exception as error:
        if key == 'win':
            evidence['failure'] = {'type': type(error).__name__, 'message': str(error)}
            write_app_evidence(directory, evidence)
        raise


def extracted_final_proof(root, key, artifact, kind, output, context, before, signature, baseline, directory, evidence):
    with tempfile.TemporaryDirectory(prefix='polaris-final-payload-') as tmp:
        destination = Path(tmp) / 'payload'
        if kind == 'zip': destination.mkdir(); extract_zip(artifact, destination)
        elif kind == 'deb': subprocess.run(['dpkg-deb', '--extract', str(artifact), str(destination)], check=True, timeout=90)
        elif kind == 'AppImage': subprocess.run(['unsquashfs', '-no-progress', '-offset', str(squashfs_offset(artifact)), '-dest', str(destination), str(artifact)], check=True, timeout=90)
        else:
            try: nsis_install(artifact, destination, root)
            finally:
                # Copy before TemporaryDirectory unwinds, even on install/identity failure.
                app = destination / 'polaris.exe'
                if app.is_file() and not app.is_symlink():
                    record_windows_app(app, *baseline, directory, evidence)
        core, library = payload_files(destination, key)
        app_identity = None
        if key == 'win':
            app = regular(destination / 'polaris.exe')
            app_identity = verify_windows_app(app, *baseline, directory, evidence)
            require(sha(root / 'target/release/polaris.exe') == hashlib.sha256(baseline[0]).hexdigest(),
                    'source App changed during final payload proof')
        payload_signature = {str(p.relative_to(destination)): windows_signature(p) for p in [core, library]} if key == 'win' else None
        native_proof = prove(root, key, core, library)
        if key == 'win':
            require(sha(root / 'target/release/polaris.exe') == app_identity['sourceSha256']
                    and sha(app) == app_identity['expectedSha256'], 'App bytes changed during native proof')
        require(sha(artifact) == before, 'final artifact changed while inspected')
        result = {'schema': 'polaris-final-package-proof-v1', 'transport': context,
                  'artifact': {'path': str(artifact), 'kind': kind, 'sha256': before, 'bytes': artifact.stat().st_size},
                  'signature': signature, 'payloadSignatures': payload_signature, 'appIdentity': app_identity,
                  'extraction': 'actual NSIS installed payload' if kind == 'nsis' else 'actual final archive members',
                  'relativeCore': str(core.relative_to(destination)), 'relativeLibrary': str(library.relative_to(destination)),
                  'native': native_proof, 'macFinalSignatureAcceptance': 'not observed by Linux/Windows batch',
                  'deviceAcceptance': 'not observed'}
        output.parent.mkdir(parents=True, exist_ok=True); output.write_text(json.dumps(result, indent=2) + '\n', encoding='utf-8')
    # Temporary files removed directly. The uninstaller is never executed.
    return result


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--platform', choices=['linux', 'win'], required=True)
    p.add_argument('--artifact', type=Path, required=True)
    p.add_argument('--kind', choices=['deb', 'AppImage', 'nsis', 'zip'], required=True)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args(); ci_context(a.platform)
    final_proof(Path(__file__).resolve().parent.parent, a.platform, a.artifact.absolute(), a.kind, a.output)


if __name__ == '__main__':
    main()
