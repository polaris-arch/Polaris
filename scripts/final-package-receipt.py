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
import subprocess
import tempfile
import zipfile
import importlib.util
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


def extract_zip(archive, destination):
    with zipfile.ZipFile(archive) as z:
        names = set()
        for member in z.infolist():
            name = member.filename
            p = PurePosixPath(name)
            mode = member.external_attr >> 16
            require(name not in names and not p.is_absolute() and '..' not in p.parts
                    and '\\' not in name and ':' not in name and not stat.S_ISLNK(mode), 'unsafe ZIP member')
            names.add(name)
        # Windows rejects canonical-name aliases before extraction.
        require(len({name.lower() for name in names}) == len(names), 'ZIP case alias collision')
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


def nsis_install(artifact, destination, root):
    ci_context('win')
    require(not destination.exists() and destination.parent.is_dir(), 'NSIS requires new isolated prefix')
    conf = json.loads((root / 'src-tauri/tauri.conf.json').read_text())
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
    with tempfile.TemporaryDirectory(prefix='polaris-final-payload-') as tmp:
        destination = Path(tmp) / 'payload'
        if kind == 'zip': destination.mkdir(); extract_zip(artifact, destination)
        elif kind == 'deb': subprocess.run(['dpkg-deb', '--extract', str(artifact), str(destination)], check=True, timeout=90)
        elif kind == 'AppImage': subprocess.run(['unsquashfs', '-no-progress', '-offset', str(squashfs_offset(artifact)), '-dest', str(destination), str(artifact)], check=True, timeout=90)
        else: nsis_install(artifact, destination, root)
        core, library = payload_files(destination, key)
        app_identity = None
        if key == 'win':
            app = regular(destination / 'polaris.exe')
            source_app = regular(root / 'target/release/polaris.exe')
            require(sha(app) == sha(source_app), 'final app bytes differ from release-profile source')
            app_identity = {'sha256': sha(app), 'sourceSha256': sha(source_app),
                            'sourceSignature': windows_signature(source_app), 'finalSignature': windows_signature(app)}
        payload_signature = {str(p.relative_to(destination)): windows_signature(p) for p in [core, library]} if key == 'win' else None
        native_proof = prove(root, key, core, library)
        require(sha(artifact) == before, 'final artifact changed while inspected')
        result = {'schema': 'polaris-final-package-proof-v1', 'transport': context,
                  'artifact': {'path': str(artifact), 'kind': kind, 'sha256': before, 'bytes': artifact.stat().st_size},
                  'signature': signature, 'payloadSignatures': payload_signature, 'appIdentity': app_identity,
                  'extraction': 'actual NSIS installed payload' if kind == 'nsis' else 'actual final archive members',
                  'relativeCore': str(core.relative_to(destination)), 'relativeLibrary': str(library.relative_to(destination)),
                  'native': native_proof, 'macFinalSignatureAcceptance': 'not observed by Linux/Windows batch',
                  'deviceAcceptance': 'not observed'}
        output.parent.mkdir(parents=True, exist_ok=True); output.write_text(json.dumps(result, indent=2) + '\n')
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
