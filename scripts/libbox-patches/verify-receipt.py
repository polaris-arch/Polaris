#!/usr/bin/env python3
"""Reject a cached libbox AAR that does not match this checkout's build inputs."""
import hashlib
import json
from pathlib import Path
import sys

PATCH_DIR = Path(__file__).resolve().parent
ROOT = PATCH_DIR.parent.parent
AAR = ROOT / 'src-tauri/gen/android/app/libs/libbox.aar'


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            value.update(chunk)
    return value.hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def main():
    manifest_path = PATCH_DIR / 'source-manifest.json'
    manifest = json.loads(manifest_path.read_text())
    receipt = json.loads((PATCH_DIR / 'build-receipt.json').read_text())
    version = json.loads((ROOT / 'src-tauri/core-manifest.json').read_text())['bundledCoreVersion']
    require(receipt['sourceCommit'] == manifest['sourceCommit'], 'source commit differs')
    require(receipt['officialTag'] == f'v{version}', 'core version differs')
    require(receipt['sourceManifestSha256'] == digest(manifest_path), 'manifest differs')
    require(receipt['buildScriptSha256'] == digest(PATCH_DIR / 'build.py'), 'builder differs')
    require(receipt['patches'] == manifest['patches'], 'patch list differs')
    for patch in manifest['patches']:
        require(digest(PATCH_DIR / patch['file']) == patch['sha256'], f"patch differs: {patch['file']}")
    require(receipt['toolchain']['ndk'] == manifest['ndkVersion'], 'NDK revision differs')
    require(f" go{manifest['goVersion']} " in receipt['toolchain']['go'], 'Go version differs')
    for tool in ('gomobile', 'gobind'):
        require(receipt['toolchain'][tool]['version'] == manifest['gomobileVersion'],
                f'{tool} version differs')
    require(receipt['buildTags'] == manifest['buildTags'], 'feature tags differ')
    require(receipt['androidAPI'] == manifest['androidAPI'], 'Android API differs')
    require(AAR.is_file(), 'AAR missing')
    require(receipt['aar']['path'] == str(AAR.relative_to(ROOT)), 'AAR path differs')
    require(receipt['aar']['bytes'] == AAR.stat().st_size, 'AAR size differs')
    require(receipt['aar']['sha256'] == digest(AAR), 'AAR hash differs')
    print(f"libbox receipt verified: {receipt['aar']['sha256']}")


if __name__ == '__main__':
    try:
        main()
    except (OSError, KeyError, ValueError, TypeError, json.JSONDecodeError) as error:
        print(f'libbox receipt verification failed: {error}', file=sys.stderr)
        sys.exit(1)
