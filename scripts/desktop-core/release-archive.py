#!/usr/bin/env python3
"""Extract one pinned native archive into a new private directory, never run it."""
import hashlib
import gzip
import io
import json
import re
import stat
import sys
import tarfile
import zipfile
from pathlib import Path


def require(ok, message):
    if not ok:
        raise ValueError(message)


def extract(path, destination, record, root):
    require(0 < record['bytes'] <= 512 * 1024 * 1024 and Path(path).stat().st_size == record['bytes'],
            'archive bytes differ')
    data = Path(path).read_bytes()
    require(len(data) == record['bytes'] and hashlib.sha256(data).hexdigest() == record['sha256'],
            'archive bytes differ')
    expected = record['members']
    require(0 < len(expected) <= 1024, 'member inventory limit')
    seen, aliases, members = set(), set(), {}

    def member(name, directory, mode, size, read):
        name = name.removesuffix('/') if directory else name
        parts = name.split('/')
        require(parts[0] == root and all(re.fullmatch(r'[A-Za-z0-9_+.-]+', p)
                and p not in ['.', '..'] and not p.endswith('.')
                and p.split('.')[0].upper() not in ['CON', 'PRN', 'AUX', 'NUL',
                    *['COM' + str(i) for i in range(1, 10)], *['LPT' + str(i) for i in range(1, 10)]]
                for p in parts), 'unsafe archive path')
        require(name not in seen and name.lower() not in aliases, 'duplicate archive path')
        seen.add(name)
        aliases.add(name.lower())
        require(mode & 0o7000 == 0 and mode & 0o022 == 0, 'unsafe member permissions')
        if directory:
            require(any(n.startswith(name + '/') for n in expected), 'unexpected directory')
            return
        require(name in expected and 0 <= size <= 200 * 1024 * 1024, 'unexpected/oversized member')
        require(sum(len(v[0]) for v in members.values()) + size <= 1024 ** 3, 'expanded size limit')
        content = read(size + 1)
        require(len(content) == size and hashlib.sha256(content).hexdigest() == expected[name],
                'member bytes differ')
        members[name] = (content, mode & 0o777)

    if path.endswith('.zip'):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            require(len(archive.infolist()) <= 2048, 'entry limit')
            for info in archive.infolist():
                require('\x00' not in info.orig_filename, 'raw NUL ZIP member name')
                mode = info.external_attr >> 16
                directory = info.is_dir()
                kind = stat.S_IFMT(mode)
                require(not info.flag_bits & 1 and kind in [0, stat.S_IFDIR if directory else stat.S_IFREG],
                        'encrypted/link/special ZIP member')
                with archive.open(info) as stream:
                    member(info.filename, directory, mode or (0o755 if directory else 0o644),
                           info.file_size, stream.read)
    else:
        require(path.endswith('.tar.gz'), 'unsupported archive format')
        # Bound decompression before tarfile interprets PAX/longname headers.
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
            expanded = compressed.read(1024 ** 3 + 1)
        require(len(expanded) <= 1024 ** 3, 'expanded tar size limit')
        with tarfile.open(fileobj=io.BytesIO(expanded), mode='r:') as archive:
            count = 0
            for info in archive:
                count += 1
                require(count <= 2048 and (info.isfile() or info.isdir()), 'link/special/entry limit')
                if info.isdir():
                    member(info.name, True, info.mode, info.size, None)
                else:
                    with archive.extractfile(info) as stream:
                        member(info.name, False, info.mode, info.size, stream.read)
    require(set(members) == set(expected), 'missing archive member')
    prefixes = {}
    files_lower = {name.lower() for name in members}
    for name in seen:
        for i in range(1, len(name.split('/'))):
            parent = '/'.join(name.split('/')[:i])
            require(parent.lower() not in files_lower, 'file/directory collision')
            require(prefixes.setdefault(parent.lower(), parent) == parent, 'directory case alias')
    destination = Path(destination)
    require(not destination.exists(), 'extract destination already exists')
    for parent in destination.absolute().parents:
        require(parent.is_dir() and not parent.is_symlink()
                and not getattr(parent, 'is_junction', lambda: False)(), 'linked extraction ancestor')
    destination.mkdir()
    for name, (content, mode) in members.items():
        target = destination / name
        target.parent.mkdir(parents=True, exist_ok=True)
        with target.open('xb') as stream:
            stream.write(content)
        target.chmod(mode)


if __name__ == '__main__':
    request = json.load(sys.stdin)
    extract(sys.argv[1], sys.argv[2], request['archive'], request['root'])
