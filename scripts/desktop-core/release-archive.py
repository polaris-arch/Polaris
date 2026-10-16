#!/usr/bin/env python3
"""Extract one pinned native archive into a new private directory, never run it."""
import hashlib
import gzip
import io
import json
import re
import stat
import shlex
import sys
import tarfile
import zipfile
from pathlib import Path


def require(ok, message):
    if not ok:
        raise ValueError(message)


def extract(path, destination, record, root, target=None):
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
        if 'memberModes' in record:
            require(set(record['memberModes']) == set(expected)
                    and record['memberModes'][name] == mode & 0o777,
                    'member permissions differ')
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
                # The frozen Windows publisher stamps regular files 0100666
                # and its sing-box.exe 0100777. Only this explicit target and
                # these exact regular-file modes create safe private copies;
                # all archive/member hashes, link/special/path/collision guards
                # remain in force. Never restore group/other write bits.
                if target == 'windows/amd64':
                    if mode == (stat.S_IFREG | 0o666):
                        mode = stat.S_IFREG | 0o644
                    elif info.filename == root + '/sing-box.exe' and mode == (stat.S_IFREG | 0o777):
                        mode = stat.S_IFREG | 0o755
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


def extract_wire_transport(path, destination, record):
    """Current-run Actions outer ZIP: pin whole bytes, then two flat members."""
    source, destination = Path(path), Path(destination)
    require(0 < record['bytes'] <= 512 * 1024 * 1024
            and source.stat().st_size == record['bytes'], 'wire ZIP bytes differ')
    data = source.read_bytes()
    require(hashlib.sha256(data).hexdigest() == record['sha256'], 'wire ZIP SHA differs')
    names = {'archive.json', 'core-wire-inputs.tar.gz'}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        infos = archive.infolist()
        require(len(infos) == 2 and {i.filename for i in infos} == names, 'wire ZIP inventory differs')
        contents = {}
        for info in infos:
            mode = info.external_attr >> 16
            require(info.orig_filename == info.filename and not info.flag_bits & 1
                    and stat.S_IFMT(mode) in [0, stat.S_IFREG]
                    and 0 < info.file_size <= 512 * 1024 * 1024, 'unsafe wire ZIP member')
            contents[info.filename] = archive.read(info)  # full CRC before any write
        require(sum(map(len, contents.values())) <= 512 * 1024 * 1024, 'wire ZIP expanded limit')
    require(not destination.exists(), 'wire ZIP destination already exists')
    for parent in destination.absolute().parents:
        require(parent.is_dir() and not parent.is_symlink()
                and not getattr(parent, 'is_junction', lambda: False)(), 'linked wire ZIP ancestor')
    destination.mkdir(mode=0o700)
    for name, content in contents.items():
        with (destination / name).open('xb') as stream:
            stream.write(content)
        (destination / name).chmod(0o600)


def verify_source_archive(path, record, commit, expected_tree, gitlinks, source_identity, build_replacements):
    """Hash the full Git tree from source tar bytes without extracting anything.

    This independently links the downloaded source archive to the public commit
    tree. Tar metadata is never trusted as a commit assertion. Symlink targets
    are hashed as Git blob bytes and are never followed.
    """
    require(re.fullmatch(r'[a-f0-9]{40}', commit) and re.fullmatch(r'[a-f0-9]{40}', expected_tree),
            'invalid source object identity')
    source = Path(path)
    require(0 < record['bytes'] <= 512 * 1024 * 1024
            and source.stat().st_size == record['bytes'], 'source archive size differs')
    data = source.read_bytes()
    require(hashlib.sha256(data).hexdigest() == record['sha256'], 'source archive SHA differs')
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        expanded = compressed.read(1024 ** 3 + 1)
    require(len(expanded) <= 1024 ** 3, 'source tar expanded size limit')
    prefix = 'sing-box-polaris-source-' + commit
    entries, seen, source_files = {}, set(), {}
    def object_hash(kind, content):
        return hashlib.sha1(kind.encode() + b' ' + str(len(content)).encode() + b'\0' + content).digest()
    with tarfile.open(fileobj=io.BytesIO(expanded), mode='r:') as archive:
        for count, info in enumerate(archive, 1):
            require(count <= 200000 and (info.isfile() or info.isdir() or info.issym()),
                    'source special/gitlink or entry limit')
            name = info.name.removesuffix('/') if info.isdir() else info.name
            parts = name.split('/')
            require(parts[0] == prefix and all(p and p not in ['.', '..'] and '\0' not in p for p in parts)
                    and name not in seen, 'source archive path/duplicate differs')
            seen.add(name)
            if len(parts) == 1:
                require(info.isdir(), 'source prefix must be directory')
                continue
            node = entries
            for part in parts[1:-1]:
                node = node.setdefault(part, {})
                require(isinstance(node, dict), 'source file/directory collision')
            leaf = parts[-1]
            if info.isdir():
                require(isinstance(node.setdefault(leaf, {}), dict), 'source directory/file collision')
                continue
            require(leaf not in node and info.mode & 0o7000 == 0 and info.size <= 200 * 1024 * 1024,
                    'source duplicate/oversized/special-mode file')
            if info.issym():
                content, mode = info.linkname.encode('utf-8'), b'120000'
            else:
                with archive.extractfile(info) as stream:
                    content = stream.read(info.size + 1)
                require(len(content) == info.size, 'source file length differs')
                mode = b'100755' if info.mode & 0o111 else b'100644'
            node[leaf] = (mode, object_hash('blob', content))
            if parts[1:] in [['go.mod'], ['go.sum']]:
                require(info.isfile(), 'source module files must be regular')
                source_files[leaf] = content
    # Git archive records gitlinks as empty directories, never submodule
    # contents. Only separately fetched public Git tree objects may fill them.
    require(isinstance(gitlinks, list) and len(gitlinks) <= 1024, 'source gitlink inventory invalid')
    linked = set()
    for link in gitlinks:
        name, oid = link['path'], link['sha']
        parts = name.split('/')
        require(all(p and p not in ['.', '..'] and '\0' not in p for p in parts)
                and name not in linked and re.fullmatch(r'[a-f0-9]{40}', oid), 'source gitlink identity invalid')
        linked.add(name)
        node = entries
        for part in parts[:-1]:
            require(part in node and isinstance(node[part], dict), 'source gitlink parent absent')
            node = node[part]
        require(node.get(parts[-1]) == {}, 'source gitlink is not an empty archive directory')
        node[parts[-1]] = (b'160000', bytes.fromhex(oid))
    def tree_hash(node):
        require(node, 'empty source directory/gitlink cannot prove a Git tree')
        rows = []
        for name, value in node.items():
            raw = name.encode('utf-8')
            directory = isinstance(value, dict)
            mode, oid = (b'40000', tree_hash(value)) if directory else value
            rows.append((raw + (b'/' if directory else b''), mode + b' ' + raw + b'\0' + oid))
        return object_hash('tree', b''.join(value for _, value in sorted(rows)))
    observed = tree_hash(entries).hex()
    require(observed == expected_tree, 'downloaded source archive Git tree differs')
    for name, field in [('go.mod', 'goModSha256'), ('go.sum', 'goSumSha256')]:
        require(name in source_files and hashlib.sha256(source_files[name]).hexdigest() == source_identity[field],
                'downloaded source module bytes differ')
    require(source_identity['sourceOverlay'] == 0, 'source overlay unsupported')
    for replacement in source_identity['replacements']:
        require(replacement['path'].startswith('./'), 'source replacement path differs')
        node = entries
        for part in replacement['path'][2:].split('/'):
            require(part not in ['', '.', '..'] and isinstance(node, dict) and part in node,
                    'source replacement subtree missing')
            node = node[part]
        require(isinstance(node, dict) and tree_hash(node).hex() == replacement['tree'],
                'downloaded local replacement tree differs')
    # Check actual linked replacements against actual source go.mod/go.sum,
    # rather than treating a normalized producer record as source proof.
    declared, block = [], False
    for raw in source_files['go.mod'].decode('utf-8').splitlines():
        line = raw.split('//', 1)[0].strip()
        if not line:
            continue
        directive = re.fullmatch(r'replace\s+(.+)', line)
        if directive:
            line = directive.group(1).strip()
            if line == '(':
                block = True
                continue
        elif not block:
            continue
        elif line == ')':
            block = False
            continue
        tokens = shlex.split(line)
        require(tokens.count('=>') == 1, 'source replacement syntax unsupported')
        arrow = tokens.index('=>')
        require(arrow in [1, 2] and len(tokens) - arrow in [2, 3], 'source replacement syntax unsupported')
        declared.append((tokens[:arrow], tokens[arrow + 1:]))
    sums = set(source_files['go.sum'].decode('utf-8').splitlines())
    for observed_replacement in build_replacements:
        module, version = observed_replacement['module'], observed_replacement['version']
        target = observed_replacement['replacement']
        specific = [(old, new) for old, new in declared if old == [module, version]]
        matches = specific or [(old, new) for old, new in declared if old == [module]]
        require(len(matches) == 1, 'linked replacement absent/ambiguous in source go.mod')
        new = matches[0][1]
        require(new[0] == target['module'], 'linked replacement module differs from source')
        if new[0].startswith('./'):
            require(len(new) == 1 and target['version'] == '(devel)' and target['h1'] == '',
                    'linked local replacement identity differs')
        else:
            require(len(new) == 2 and new[1] == target['version']
                    and f"{new[0]} {new[1]} {target['h1']}" in sums,
                    'linked remote replacement version/checksum differs from source')
    return observed


if __name__ == '__main__':
    request = json.load(sys.stdin)
    if 'sourceArchive' in request:
        verify_source_archive(sys.argv[1], request['sourceArchive'], request['sourceCommit'], request['sourceTree'], request['sourceGitlinks'], request['sourceIdentity'], request['buildReplacements'])
    elif 'wireTransport' in request:
        extract_wire_transport(sys.argv[1], sys.argv[2], request['wireTransport'])
    else:
        extract(sys.argv[1], sys.argv[2], request['archive'], request['root'], request.get('target'))
