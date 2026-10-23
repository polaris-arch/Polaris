#!/usr/bin/env python3
"""Verify an Actions ZIP digest before safe private extraction; never execute it."""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import sys
import zipfile


def require(ok, message):
    if not ok:
        raise ValueError(message)


def extract(archive, destination, pin):
    archive, destination = Path(archive), Path(destination)
    require(not destination.exists() and not destination.is_symlink(), 'fresh destination required')
    require(type(pin.get('bytes')) is int and 0 < pin['bytes'] <= 1024 ** 3
            and archive.stat().st_size == pin['bytes'], 'ZIP size differs')
    hasher = hashlib.sha256()
    with archive.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 ** 2), b''):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    require(re.fullmatch(r'[a-f0-9]{64}', pin.get('sha256', ''))
            and digest == pin['sha256'], 'ZIP digest differs')
    with zipfile.ZipFile(archive) as source:
        members = source.infolist()
        require(0 < len(members) <= 128, 'ZIP member limit')
        names, parents, size = set(), set(), 0
        for member in members:
            name = member.filename.removesuffix('/') if member.is_dir() else member.filename
            parts = name.split('/')
            require(member.orig_filename == member.filename and '\0' not in name
                    and not PurePosixPath(name).is_absolute()
                    and all(re.fullmatch(r'[A-Za-z0-9_.+-]+', p) and p not in ('.', '..') for p in parts),
                    'unsafe ZIP name')
            mode = member.external_attr >> 16
            require(not stat.S_ISLNK(mode) and stat.S_IFMT(mode) in (0, stat.S_IFREG, stat.S_IFDIR)
                    and not member.flag_bits & 1, 'unsafe ZIP entry')
            require(name not in names, 'duplicate ZIP member')
            names.add(name)
            parents.update('/'.join(parts[:i]) for i in range(1, len(parts)))
            size += member.file_size
            require(member.file_size <= 512 * 1024 ** 2 and size <= 1024 ** 3, 'expanded ZIP limit')
        require(all(m.is_dir() or m.filename not in parents for m in members), 'ZIP path collision')
        destination.mkdir(mode=0o700)
        for member in members:
            target = destination / member.filename
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True, mode=0o700)
            else:
                target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                with source.open(member) as src, target.open('xb') as dst:
                    while chunk := src.read(1024 ** 2):
                        dst.write(chunk)
                target.chmod(0o600)


if __name__ == '__main__':
    require(len(sys.argv) == 3, 'archive and destination required')
    extract(sys.argv[1], sys.argv[2], json.load(sys.stdin))
