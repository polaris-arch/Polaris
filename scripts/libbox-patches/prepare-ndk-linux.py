#!/usr/bin/env python3
"""Fetch an exact NDK: legacy default or explicitly selected formal stable profile."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile

PATCH_DIR = Path(__file__).resolve().parent
FORMAL_PROFILE = PATCH_DIR.parent / 'mobile-core/ndk-r30-linux.json'
FORMAL_PROFILE_SHA256 = 'db468c29e87569f09ca04dc3b3d2137554124cf755ebdf68371cfdff466f6a30'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def hashes(path):
    sha1 = hashlib.sha1()
    sha256 = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            sha1.update(chunk)
            sha256.update(chunk)
    return sha1.hexdigest(), sha256.hexdigest()


def revision(path):
    match = re.search(r'^Pkg.Revision\s*=\s*(\S+)\s*$',
                      (path / 'source.properties').read_text(), re.M)
    require(match is not None, 'NDK source.properties has no revision')
    return match.group(1)


def legacy_main():
    require(2 <= len(sys.argv) <= 3,
            'Usage: prepare-ndk-linux.py <runner temporary directory> [verified local archive]')
    archive = json.loads((PATCH_DIR / 'ndk-linux-archive.json').read_text())
    manifest = json.loads((PATCH_DIR / 'source-manifest.json').read_text())
    runner_temp = Path(sys.argv[1]).resolve()
    require(runner_temp.is_dir(), 'Runner temporary directory does not exist')
    target = runner_temp / 'polaris-libbox-ndk'
    if target.exists():
        require(revision(target) == manifest['ndkVersion'], 'Existing libbox NDK revision differs')
        print(target)
        return

    with tempfile.TemporaryDirectory(prefix='polaris-libbox-ndk-', dir=runner_temp) as temp_name:
        temp = Path(temp_name)
        download = Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else temp / 'ndk.zip'
        if len(sys.argv) == 2:
            subprocess.run(['curl', '-fL', '--retry', '3', '--retry-all-errors',
                            '--connect-timeout', '20', '--max-time', '900', '-sS',
                            '-o', str(download), archive['url']], check=True)
        require(download.stat().st_size == archive['bytes'], 'NDK archive size differs')
        actual_sha1, actual_sha256 = hashes(download)
        require(actual_sha1 == archive['sha1'], 'NDK archive official SHA1 differs')
        require(actual_sha256 == archive['sha256'], 'NDK archive SHA256 differs')
        with zipfile.ZipFile(download) as bundle:
            prefix = archive['archiveRoot'] + '/'
            for member in bundle.namelist():
                parts = PurePosixPath(member).parts
                require(member.startswith(prefix) and '..' not in parts,
                        'NDK archive contains a path outside its root')
        # zipfile.extractall loses Unix symlinks and executable bits; the NDK
        # compiler launchers need both. The checked archive is safe to unzip.
        subprocess.run(['unzip', '-q', str(download), '-d', str(temp / 'unpacked')], check=True)
        unpacked = temp / 'unpacked' / archive['archiveRoot']
        require(revision(unpacked) == manifest['ndkVersion'], 'Downloaded NDK revision differs')
        shutil.move(str(unpacked), str(target))
    print(target)


def real_path(path):
    path = Path(path).absolute()
    require(all(not parent.is_symlink() for parent in (path, *path.parents)),
            'formal NDK input or slot cannot be redirected')
    return path


def formal_profile(path):
    path = real_path(path)
    require(path == FORMAL_PROFILE and path.is_file(), 'exact reviewed formal NDK profile required')
    raw = path.read_bytes()
    # The byte pin admits only this reviewed schema, values and types. Unknown,
    # duplicate, weakened or alternate profiles cannot select another installer.
    require(hashlib.sha256(raw).hexdigest() == FORMAL_PROFILE_SHA256, 'formal NDK profile bytes differ')
    return json.loads(raw)


def accepted_formal_license(sdk, profile):
    sdk = real_path(sdk)
    require(sdk.is_dir(), 'explicit existing Android SDK required')
    accepted = real_path(sdk / 'licenses' / profile['licenseId'])
    require(accepted.is_file(), 'standard Android SDK license is not already accepted')
    # This is the reviewed SDK TrimStringAdapter-normalized license digest.
    # Read only; never run sdkmanager --licenses or create/append a license file.
    require(profile['licenseSha1'] in accepted.read_text(encoding='utf-8').split('\n'),
            'standard Android SDK license is not already accepted')


def verify_formal_archive(download, profile):
    download = real_path(download)
    archive = profile['archive']
    require(download.is_file() and download.stat().st_size == archive['bytes'], 'formal NDK archive size/type differs')
    require(hashes(download) == (archive['sha1'], archive['sha256']), 'formal NDK archive hashes differ')
    with zipfile.ZipFile(download) as bundle:
        names = bundle.namelist()
        require(names and len(names) == len(set(names)), 'formal NDK ZIP inventory empty/duplicate')
        for name in names:
            path = PurePosixPath(name)
            require(name.startswith(archive['archiveRoot'] + '/') and '..' not in path.parts and
                    not path.is_absolute() and '\\' not in name, 'formal NDK ZIP escapes its root')


def verify_formal_properties(ndk, profile):
    sys.path.insert(0, str(FORMAL_PROFILE.parent))
    from android_sdk import validate_ndk_properties
    properties = real_path(ndk / 'source.properties')
    require(properties.is_file(), 'regular formal NDK source.properties required')
    raw = properties.read_bytes()
    require(len(raw) == profile['sourcePropertiesBytes'] and
            hashlib.sha256(raw).hexdigest() == profile['sourcePropertiesSha256'],
            'formal NDK source.properties bytes differ')
    validate_ndk_properties(raw.decode('utf-8'), profile)


def prepare_formal(runner_temp, sdk, profile_path, local_archive=None):
    profile = formal_profile(profile_path)
    root = real_path(runner_temp)
    info = root.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and
            stat.S_IMODE(info.st_mode) == 0o700, 'formal NDK root must be current-user private')
    accepted_formal_license(sdk, profile)
    target = real_path(root / ('polaris-mobile-ndk-' + profile['ndkRevision']))
    # A properties/compiler subset cannot prove all NDK headers and libraries.
    # Preserve and refuse retained slots rather than silently reusing/deleting.
    require(not target.exists(), 'retained formal NDK slot refused; fresh private runner slot required')
    target.mkdir(mode=0o700)
    with tempfile.TemporaryDirectory(prefix='verified-archive-', dir=target) as temp_name:
        temp = Path(temp_name)
        download = real_path(local_archive) if local_archive is not None else temp / 'ndk.zip'
        if local_archive is None:
            subprocess.run(['curl', '--disable', '-f', '--proto', '=https', '--retry', '3', '--retry-all-errors',
                            '--connect-timeout', '20', '--max-time', '900', '-sS',
                            '-o', str(download), profile['archive']['url']], check=True)
        verify_formal_archive(download, profile)
        # Only the exact reviewed archive reaches unzip, preserving official
        # Unix symlinks/executable modes. No arbitrary archive is accepted.
        subprocess.run(['unzip', '-q', str(download), '-d', str(temp / 'unpacked')], check=True)
        unpacked = real_path(temp / 'unpacked' / profile['archive']['archiveRoot'])
        require(unpacked.is_dir() and unpacked.stat().st_uid == os.getuid(), 'formal NDK extraction ownership differs')
        verify_formal_properties(unpacked, profile)
        ndk = target / profile['archive']['archiveRoot']
        unpacked.rename(ndk)
    return ndk


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '--profile':
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--profile', type=Path, required=True)
        parser.add_argument('--sdk', type=Path, required=True)
        parser.add_argument('--check-license-only', action='store_true')
        parser.add_argument('runner_temp', type=Path, nargs='?')
        parser.add_argument('local_archive', type=Path, nargs='?')
        args = parser.parse_args()
        if args.check_license_only:
            require(args.runner_temp is None and args.local_archive is None, 'license check cannot prepare an NDK slot')
            accepted_formal_license(args.sdk, formal_profile(args.profile))
            return
        require(args.runner_temp is not None, 'explicit private runner slot required')
        print(prepare_formal(args.runner_temp, args.sdk, args.profile, args.local_archive))
    else:
        legacy_main()


if __name__ == '__main__':
    try:
        main()
    except (OSError, KeyError, ValueError, RuntimeError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        print(f'libbox NDK preparation failed: {error}', file=sys.stderr)
        sys.exit(1)
