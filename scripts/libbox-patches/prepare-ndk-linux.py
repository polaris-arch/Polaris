#!/usr/bin/env python3
"""Fetch the exact archived NDK required by the libbox source manifest."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

PATCH_DIR = Path(__file__).resolve().parent


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


def main():
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


if __name__ == '__main__':
    try:
        main()
    except (OSError, KeyError, ValueError, subprocess.CalledProcessError, zipfile.BadZipFile) as error:
        print(f'libbox NDK preparation failed: {error}', file=sys.stderr)
        sys.exit(1)
