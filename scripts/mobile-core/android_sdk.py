"""Scoped SDK view for the formal producer; does not install/upgrade any tool."""
import os
from pathlib import Path
import stat

from fork_input import require, sha


def prepare_view(work, sdk, ndk, profile):
    work, sdk, ndk = (Path(path).resolve(strict=True) for path in [work, sdk, ndk])
    info = work.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and
            stat.S_IMODE(info.st_mode) == 0o700, 'fixed private caller-owned work slot required')
    properties = (ndk / 'source.properties').read_text()
    values = dict(line.split('=', 1) for line in properties.splitlines() if '=' in line)
    require(values.get('Pkg.Revision', '').strip() == profile['ndkRevision'], 'actual pinned NDK revision differs')
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
