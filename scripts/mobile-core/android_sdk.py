"""Scoped SDK view for the formal producer; does not install/upgrade any tool."""
import os
from pathlib import Path
import stat
import re

from fork_input import ROOT, require, sha


def properties_values(properties):
    values = {}
    for line in properties.splitlines():
        if not line.strip() or line.lstrip().startswith(('#', '!')):
            continue
        require('=' in line, 'NDK source.properties field malformed')
        key, value = (part.strip() for part in line.split('=', 1))
        require(key and key not in values, 'NDK source.properties duplicate/empty key')
        values[key] = value
    return values


def revision_from_properties(properties):
    revision = properties_values(properties).get('Pkg.Revision', '')
    require(re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+(?:-(?:alpha|beta|rc)[0-9]+)?', revision) is not None,
            'NDK source.properties revision malformed/missing')
    return revision


def validate_ndk_properties(properties, profile):
    values = properties_values(properties)
    require(revision_from_properties(properties) == profile['ndkSourceRevision'] and
            values.get('Pkg.BaseRevision') == profile['ndkRevision'] and
            values.get('Pkg.ReleaseName') == profile['ndkPackage'].removeprefix('android-ndk-'),
            'actual pinned NDK revision/base/release differs')
    return values


def observe_ndk_input(root, ndk, profile):
    """Retain the actual small input even when later SDK admission fails."""
    from artifacts import file_pin
    from fork_input import canonical
    properties = Path(ndk) / 'source.properties'
    require(properties.is_file() and not properties.is_symlink(), 'regular NDK source.properties required')
    raw = properties.read_bytes()
    snapshot = Path(root) / 'ndk-source.properties'
    require(not snapshot.is_symlink(), 'NDK source observation redirected')
    if snapshot.exists():
        require(snapshot.read_bytes() == raw, 'previous NDK source observation differs')
    else:
        with snapshot.open('xb') as stream:
            stream.write(raw)
    revision = revision_from_properties(raw.decode('utf-8'))
    facts = {'schema': 'polaris-mobile-ndk-input-observation-v1', 'path': str(properties.resolve(strict=True)),
             'sourceProperties': file_pin(snapshot), 'revision': revision,
             'requiredRevision': profile['ndkSourceRevision'],
             'baseRevision': properties_values(raw.decode('utf-8')).get('Pkg.BaseRevision'),
             'requiredBaseRevision': profile['ndkRevision'],
             'package': profile['ndkPackage'], 'archivePath': profile['ndkArchivePath'],
             'archiveContract': file_pin(ROOT / profile['ndkArchivePath']),
             'devicePassed': False, 'publicationEligible': False}
    facts['fingerprint'] = sha(canonical(facts))
    output = Path(root) / 'sdk-input-observation.json'
    encoded = canonical(facts)
    require(not output.is_symlink(), 'NDK input observation redirected')
    if output.exists():
        require(output.read_bytes() == encoded, 'previous NDK input observation differs')
    else:
        with output.open('xb') as stream:
            stream.write(encoded)
    require(properties.read_bytes() == raw, 'NDK source.properties changed during observation')
    validate_ndk_properties(raw.decode('utf-8'), profile)
    return facts


def prepare_view(work, sdk, ndk, profile):
    work, sdk, ndk = (Path(path).resolve(strict=True) for path in [work, sdk, ndk])
    info = work.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and
            stat.S_IMODE(info.st_mode) == 0o700, 'fixed private caller-owned work slot required')
    properties = (ndk / 'source.properties').read_text()
    validate_ndk_properties(properties, profile)
    jar = sdk / 'platforms' / profile['bootclasspath']['platform'] / 'android.jar'
    require(jar.is_file() and sha(jar.read_bytes()) == profile['bootclasspath']['sha256'],
            'actual Android bootclasspath differs')
    view = work / 'android-sdk-view'
    if not view.exists():
        view.mkdir(mode=0o700)
    info = view.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and
            stat.S_IMODE(info.st_mode) == 0o700, 'SDK view must be a private real directory')
    # The caller holds this work slot's exclusive producer lease. Existing links
    # are compared, never silently retargeted or recursively removed.
    expected = {'platforms': sdk / 'platforms', 'build-tools': sdk / 'build-tools',
                'platform-tools': sdk / 'platform-tools'}
    require(set(path.name for path in view.iterdir()) <= set(expected) | {'ndk'}, 'unexpected SDK view entry')
    for name, target in expected.items():
        require(target.is_dir(), 'actual SDK directory missing: ' + name)
        link = view / name
        if link.is_symlink():
            require(link.resolve(strict=True) == target, 'retained SDK link differs')
        else:
            require(not link.exists(), 'SDK link destination is occupied')
            link.symlink_to(target, target_is_directory=True)
    ndks = view / 'ndk'
    if not ndks.exists():
        ndks.mkdir(mode=0o700)
    require(not ndks.is_symlink() and ndks.is_dir() and
            set(path.name for path in ndks.iterdir()) <= {profile['ndkRevision']},
            'SDK view may contain only the pinned NDK; ambient NDK28 is forbidden')
    link = ndks / profile['ndkRevision']
    if link.is_symlink():
        require(link.resolve(strict=True) == ndk, 'retained NDK view differs')
    else:
        require(not link.exists(), 'NDK link destination is occupied')
        link.symlink_to(ndk, target_is_directory=True)
    return view, jar
