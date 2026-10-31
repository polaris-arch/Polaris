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
        if kind == 'zip':
            # Same complete canonical inventory as the unconditional static gate.
            integrity_spec = importlib.util.spec_from_file_location('windows_integrity', Path(__file__).with_name('windows-package-integrity.py'))
            integrity = importlib.util.module_from_spec(integrity_spec)
            integrity_spec.loader.exec_module(integrity)
            integrity.verify_existing_zip(root, artifact)
            destination.mkdir(); extract_zip(artifact, destination)
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


def reject_local_selection(artifact):
    """Read actual compressed members; never execute/install or trust a lane label.

    Reuse the Windows carrier reader's bounded child/namespace budgets. The
    native-proof mode below retains its original context and execution gates.
    """
    import time, tarfile
    spec = importlib.util.spec_from_file_location('selection_integrity', Path(__file__).with_name('windows-package-integrity.py'))
    integrity = importlib.util.module_from_spec(spec); spec.loader.exec_module(integrity)
    marker = b'POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1'
    deadline = time.monotonic() + integrity.EXTRACT_TIMEOUT
    budget = {'bytes': 0}; checked = []
    def remaining():
        seconds = deadline - time.monotonic(); require(seconds > 0, 'formal asset inspection deadline exceeded'); return seconds
    def role(name):
        leaf = PurePosixPath(name).name
        if leaf in ['polaris', 'Polaris', 'polaris.exe', 'Polaris.exe']: return 'app'
        if leaf in ['polaris-helper', 'polaris-helper.exe']: return 'helper'
        if artifact.suffix == '.apk' and leaf == 'libpolaris_lib.so': return 'app'
        return None
    def observe(name, emit, expected):
        state = {'bytes': 0, 'tail': b''}; digest = hashlib.sha256()
        def consume(block):
            remaining(); state['bytes'] += len(block); budget['bytes'] += len(block)
            require(state['bytes'] <= integrity.MAX_FILE and budget['bytes'] <= integrity.MAX_TOTAL, 'formal executable expansion budget exceeded')
            data = state['tail'] + block
            require(marker not in data, 'Formal package/publisher rejects a LocalDevelopmentSelection executable: ' + name)
            state['tail'] = data[-len(marker)+1:]; digest.update(block)
        emit(consume)
        require(expected is None or state['bytes'] == expected, 'formal executable member size differs')
        checked.append({'member': name, 'role': role(name), 'bytes': state['bytes'], 'sha256': digest.hexdigest()})
    def paths(records):
        integrity.validate_names([(name+'/' if row['kind']=='dir' else name, row['kind'] == 'dir') for name,row in records.items()])
        require(sum(row.get('bytes', 0) for row in records.values()) <= integrity.MAX_TOTAL, 'formal archive expanded budget exceeded')
        result = []
        for name in records:
            if not role(name) or records[name]['kind']=='dir': continue
            target = name; visited = set()
            while records[target]['kind'] == 'link':
                require(target not in visited and len(visited) < 16, 'formal executable link cycle'); visited.add(target)
                link = records[target]['target']; require(not link.startswith('/') and '\\' not in link, 'formal executable link escapes archive')
                parts = list(PurePosixPath(target).parent.parts)
                for part in link.split('/'):
                    if part in ['', '.']: continue
                    if part == '..': require(parts, 'formal executable link escapes archive'); parts.pop()
                    else: parts.append(part)
                target = '/'.join(parts); require(target in records, 'formal executable link target absent')
            require(records[target]['kind'] == 'file', 'formal executable is not a regular member')
            result.append((name,target,records[target].get('bytes')))
        return result
    def copy_stream(source, consume):
        while True:
            block = source.read(64*1024)
            if not block: return
            consume(block)
    def record(records,name,row):
        require(name not in records and len(records) < integrity.MAX_MEMBERS, 'formal archive duplicate/member count budget')
        records[name] = row
    def scan_zip(source):
        with zipfile.ZipFile(source) as archive:
            records = {}
            integrity.validate_names([(m.orig_filename,m.is_dir()) for m in archive.infolist()])
            for member in archive.infolist():
                mode=member.external_attr >> 16
                require(member.orig_filename == member.filename and '\0' not in member.filename and not stat.S_ISLNK(mode), 'formal ZIP alias/link')
                require(stat.S_IFMT(mode) in [0,stat.S_IFREG,stat.S_IFDIR], 'formal ZIP nonregular member')
                record(records, member.filename.rstrip('/'), {'kind':'dir' if member.is_dir() else 'file','bytes':member.file_size})
            for name,target,size in paths(records):
                def emit(consume,target=target):
                    with archive.open(target) as stream: copy_stream(stream,consume)
                observe(name,emit,size)
    def scan_tar(source):
        with tarfile.open(fileobj=source,mode='r:') as archive:
            records = {}; originals = {}
            for member in archive:
                name = member.name.removeprefix('./').rstrip('/')
                if name in ['', '.'] and member.isdir(): continue
                kind = 'dir' if member.isdir() else 'link' if member.issym() or member.islnk() else 'file' if member.isfile() else None
                require(kind is not None, 'formal tar nonregular member')
                target = member.linkname
                if member.islnk(): target = os.path.relpath(target, str(PurePosixPath(name).parent)).replace('\\','/')
                record(records,name,{'kind':kind,'bytes':member.size,'target':target}); originals[name]=member
            for name,target,size in paths(records):
                def emit(consume,target=target):
                    with archive.extractfile(originals[target]) as stream: copy_stream(stream,consume)
                observe(name,emit,size)
    def scan_squash(path):
        offset = str(squashfs_offset(path))
        listing = integrity.bounded_command(['unsquashfs','-lln','-no-progress','-offset',offset,str(path)],integrity.MAX_LISTING,timeout=remaining()).decode('utf-8')
        records = {}
        for line in listing.splitlines():
            if not re.match(r'^[dl-][rwxstST-]{9}\s',line): continue
            fields=line.split(None,5);require(len(fields)==6 and fields[2].isdecimal(),'formal SquashFS listing differs')
            name=fields[5];require(name=='squashfs-root' or name.startswith('squashfs-root/'),'formal SquashFS root differs')
            if name=='squashfs-root': continue
            name=name[len('squashfs-root/'):];target=''
            if line[0]=='l':name,target=name.split(' -> ',1)
            record(records,name,{'kind':{'d':'dir','l':'link','-':'file'}[line[0]],'bytes':int(fields[2]),'target':target})
        for name,target,size in paths(records):
            observe(name,lambda consume,target=target:integrity.stream_child(['unsquashfs','-cat','-offset',offset,str(path),target],consume,integrity.MAX_FILE,timeout=remaining()),size)
    def scan_sevenzip(path, temporary, depth=0):
        tool,_version=integrity.find_sevenzip(timeout=min(30,remaining()))
        listing=integrity.bounded_command([tool,'l','-slt','-sccUTF-8',str(path)],integrity.MAX_LISTING,timeout=remaining()).decode('utf-8-sig')
        records={}
        if 'Type = Nsis' in listing:
            for name,size in integrity.nsis_listing(listing.replace('\r\n','\n')):record(records,name,{'kind':'file','bytes':size or 0,'unknownSize':size is None})
        else:
            require(any('Type = '+kind in listing for kind in ['Dmg','HFS','APFS']),'formal archive type is not supported')
            spec=importlib.util.spec_from_file_location('dmg_selection',Path(__file__).with_name('desktop-core')/'dmg-selection.py')
            dmg=importlib.util.module_from_spec(spec);spec.loader.exec_module(dmg)
            def read_link(name,expected):
                data=bytearray()
                def consume(block):
                    budget['bytes']+=len(block);require(budget['bytes']<=integrity.MAX_TOTAL,'formal link expansion budget exceeded');data.extend(block)
                integrity.stream_child([tool,'x','-so','-spd','-sns-',str(path),name],consume,4096,timeout=remaining())
                require(expected is None or len(data)==expected,'formal DMG link size differs')
                return bytes(data)
            records=dmg.member_records(listing,read_link,integrity.MAX_MEMBERS)
        data_flags=[] if 'Type = Nsis' in listing else ['-sns-']
        selected=paths(records)
        if 'Type = Nsis' not in listing:dmg.require_executable_data(selected)
        if not selected:
            volumes=[(name,row)for name,row in records.items() if row['kind']=='file' and name.lower().endswith(('.hfs','.apfs','.img'))]
            require(depth==0 and len(volumes)==1 and len([r for r in records.values()if r['kind']=='file'])==1,'formal archive contains no actual App/Helper')
            name,row=volumes[0];volume=temporary/'volume.img'
            with volume.open('xb') as output:
                def consume(block):
                    budget['bytes']+=len(block);require(budget['bytes']<=integrity.MAX_TOTAL,'formal volume expansion budget exceeded');output.write(block)
                result=integrity.stream_child([tool,'x','-so','-spd',*data_flags,str(path),name],consume,integrity.MAX_TOTAL,timeout=remaining())
            require(result['stdoutBytes']==row['bytes'],'formal volume bytes differ');scan_sevenzip(volume,temporary,depth+1)
        else:
            # Native HFS/APFS -so otherwise appends extended-attribute streams
            # to the selected data fork. Preserve exact byte/size verification.
            for name,target,size in selected:
                if records[target].get('unknownSize'):size=None
                observe(name,lambda consume,target=target:integrity.stream_child([tool,'x','-so','-spd',*data_flags,str(path),target],consume,integrity.MAX_FILE,timeout=remaining()),size)
    artifact=integrity.regular(artifact) if os.name == 'nt' else regular(artifact)
    before=artifact.stat();require(0<before.st_size<=integrity.MAX_FILE,'formal asset byte budget exceeded')
    with artifact.open('rb') as source, tempfile.TemporaryDirectory(prefix='polaris-formal-selection-') as temporary_name:
        opened=os.fstat(source.fileno());require((opened.st_dev,opened.st_ino)==(before.st_dev,before.st_ino),'formal asset replaced before open')
        if os.name == 'nt':
            require(integrity.app_object_identity(integrity.app_identity(opened)) ==
                    integrity.app_object_identity(integrity.app_identity(before)),
                    'formal asset replaced before open')
        temporary=Path(temporary_name)
        # Linux consumers open the retained object via proc, not its mutable name.
        retained=Path('/proc')/str(os.getpid())/'fd'/str(source.fileno())
        if not retained.exists():
            retained=temporary/('carrier'+artifact.suffix)
            with retained.open('xb') as output:
                snapshot_bytes=0
                def snapshot_block(block):
                    nonlocal snapshot_bytes
                    remaining()
                    require(len(block)<=before.st_size-snapshot_bytes,'formal asset snapshot byte budget exceeded')
                    output.write(block);snapshot_bytes+=len(block)
                copy_stream(source,snapshot_block)
            require(snapshot_bytes==before.st_size and retained.stat().st_size==before.st_size,'formal asset snapshot differs');source.seek(0)
        if artifact.suffix in ['.zip','.apk']:scan_zip(source)
        elif artifact.suffix=='.deb':
            with tempfile.TemporaryFile(dir=temporary) as data:
                integrity.stream_child(['dpkg-deb','--fsys-tarfile',str(retained)],data.write,integrity.MAX_TOTAL,timeout=remaining());data.seek(0);scan_tar(data)
        elif artifact.suffix=='.AppImage':scan_squash(retained)
        elif artifact.suffix in ['.exe','.dmg']:scan_sevenzip(retained,temporary)
        else:raise ValueError('unknown formal asset container')
        roles={row['role']for row in checked};require('app' in roles and (artifact.suffix=='.apk' or 'helper' in roles),'formal asset actual App/Helper members absent')
        source.seek(0);artifact_digest=hashlib.sha256()
        def hash_block(block):remaining();artifact_digest.update(block)
        copy_stream(source,hash_block)
        after=os.fstat(source.fileno());current=artifact.stat()
        if os.name == 'nt':
            # Windows stat/fstat expose different ctime and executable-bit
            # projections. Compare every field within its original API view,
            # after binding the common opened object facts above.
            require(integrity.app_identity(after) == integrity.app_identity(opened) and
                    integrity.app_identity(current) == integrity.app_identity(before),
                    'formal asset changed during actual object check')
        else:
            require((after.st_size,after.st_mtime_ns,after.st_ctime_ns)==(before.st_size,before.st_mtime_ns,before.st_ctime_ns) and (current.st_dev,current.st_ino,current.st_size,current.st_mtime_ns,current.st_ctime_ns)==(before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns),'formal asset changed during actual object check')
    return {'artifact':str(artifact),'artifactBytes':before.st_size,'artifactSha256':artifact_digest.hexdigest(),'checkedExecutables':checked,'expandedExecutableBytes':budget['bytes']}

def main():
    p = argparse.ArgumentParser()
    p.add_argument('--platform', choices=['linux', 'win'], required=True)
    p.add_argument('--artifact', type=Path, required=True)
    p.add_argument('--kind', choices=['deb', 'AppImage', 'nsis', 'zip'], required=True)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args(); ci_context(a.platform)
    final_proof(Path(__file__).resolve().parent.parent, a.platform, a.artifact.absolute(), a.kind, a.output)


if __name__ == '__main__':
    if len(os.sys.argv)==3 and os.sys.argv[1]=='--reject-local-selection':
        print(json.dumps(reject_local_selection(Path(os.sys.argv[2])),sort_keys=True))
    else: main()
