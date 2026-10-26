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
import time
import zipfile
from pathlib import Path


def require(ok, message):
    if not ok:
        raise ValueError(message)


WIRE_ROOT = 'core-wire-inputs'
WIRE_PLATFORMS = ('linux', 'win', 'mac-x64', 'mac-arm64')


def archive_name(name, directory, root):
    require(isinstance(name, str), 'unsafe archive path')
    name = name.removesuffix('/') if directory else name
    parts = name.split('/')
    require(parts[0] == root and all(re.fullmatch(r'[A-Za-z0-9_+.-]+', p)
            and p not in ['.', '..'] and not p.endswith('.')
            and p.split('.')[0].upper() not in ['CON', 'PRN', 'AUX', 'NUL',
                *['COM' + str(i) for i in range(1, 10)], *['LPT' + str(i) for i in range(1, 10)]]
            for p in parts), 'unsafe archive path')
    return name


def wire_group(name, directory):
    if directory and name in (WIRE_ROOT, WIRE_ROOT + '/resources', WIRE_ROOT + '/resources/.release-receipts'):
        return 'control'
    if not directory and name == WIRE_ROOT + '/handoff.json':
        return 'control'
    parts = name.split('/')
    require(len(parts) >= 3 and parts[:2] == [WIRE_ROOT, 'resources'], 'wire archive group differs')
    if parts[2] in WIRE_PLATFORMS and (directory or len(parts) > 3):
        return parts[2]
    require(not directory and len(parts) == 4 and parts[2] == '.release-receipts', 'wire archive group differs')
    return 'control'


def wire_inventory(record, root, directories=None, member_bytes=None):
    """Fixed four-platform handoff domain; never a caller-supplied larger cap.

    Both producer planning and consumption use this pure check. Count required
    parent directories too, so omitting tar directory headers cannot evade the
    per-platform entry budget or create an unbounded extraction tree.
    """
    require(root == WIRE_ROOT, 'wire archive root differs')
    require(set(record) <= {'name', 'bytes', 'sha256', 'members', 'memberModes'}, 'unexpected wire archive fields')
    expected = record['members']
    require(isinstance(expected, dict) and 0 < len(expected) <= 7 + len(WIRE_PLATFORMS) * 1024,
            'wire member inventory limit')
    modes = record.get('memberModes')
    require(isinstance(modes, dict) and set(modes) == set(expected), 'member permissions differ')
    if member_bytes is not None:
        require(isinstance(member_bytes, dict) and set(member_bytes) == set(expected), 'wire member sizes differ')
        require(all(type(size) is int and 0 <= size <= 200 * 1024 * 1024
                    for size in member_bytes.values()), 'unexpected/oversized member')
        require(sum(member_bytes.values()) <= 1024 ** 3, 'expanded size limit')
    controls, parents = set(), set()
    counts = dict.fromkeys(WIRE_PLATFORMS, 0)
    for name, sha256 in expected.items():
        archive_name(name, False, root)
        require(isinstance(sha256, str) and re.fullmatch(r'[a-f0-9]{64}', sha256), 'wire member hash differs')
        require(type(modes[name]) is int and 0 <= modes[name] <= 0o777 and not modes[name] & 0o022,
                'unsafe member permissions')
        group = wire_group(name, False)
        if group == 'control':
            controls.add(name)
            require(modes[name] == 0o600, 'wire control permissions differ')
        else:
            counts[group] += 1
        parts = name.split('/')
        parents.update('/'.join(parts[:i]) for i in range(1, len(parts)))
    receipts = root + '/resources/.release-receipts/'
    fixed = {root + '/handoff.json', receipts + 'public-readback.json',
             *(receipts + key + '.json' for key in WIRE_PLATFORMS)}
    indices = {receipts + 'polaris-box-assets.json', receipts + 'polaris-upstream-native-assets.json'}
    require(fixed <= controls and len(controls) == 7 and len(controls & indices) == 1
            and controls <= fixed | indices, 'wire control inventory differs')
    require(all(0 < count <= 1024 for count in counts.values()), 'wire platform member inventory limit')
    require(not parents & set(expected), 'file/directory collision')
    aliases = {}
    for name in set(expected) | parents:
        require(aliases.setdefault(name.lower(), name) == name, 'directory case alias')
    if directories is not None:
        require(isinstance(directories, list) and all(isinstance(name, str) for name in directories)
                and len(directories) == len(set(directories)) and set(directories) == parents,
                'wire directory inventory differs')
    entries = dict.fromkeys((*WIRE_PLATFORMS, 'control'), 0)
    for names, directory in ((parents, True), (expected, False)):
        for name in names:
            group = wire_group(name, directory)
            entries[group] += 1
            require(entries[group] <= (10 if group == 'control' else 2048), 'wire group entry limit')
    return 10 + len(WIRE_PLATFORMS) * 2048


def extract(path, destination, record, root, target=None, *, wire_handoff=False):
    require(0 < record['bytes'] <= 512 * 1024 * 1024 and Path(path).stat().st_size == record['bytes'],
            'archive bytes differ')
    data = Path(path).read_bytes()
    require(len(data) == record['bytes'] and hashlib.sha256(data).hexdigest() == record['sha256'],
            'archive bytes differ')
    expected = record['members']
    if wire_handoff:
        require(target is None and path.endswith('.tar.gz'), 'wire archive format differs')
        entry_limit = wire_inventory(record, root)
    else:
        require(0 < len(expected) <= 1024, 'member inventory limit')
        entry_limit = 2048
    if 'memberModes' in record:
        require(set(record['memberModes']) == set(expected), 'member permissions differ')
    seen, aliases, members = set(), set(), {}
    groups = dict.fromkeys((*WIRE_PLATFORMS, 'control'), 0)

    def member(name, directory, mode, size, read):
        name = archive_name(name, directory, root)
        if wire_handoff:
            group = wire_group(name, directory)
            groups[group] += 1
            require(groups[group] <= (10 if group == 'control' else 2048), 'wire group entry limit')
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
            require(record['memberModes'][name] == mode & 0o777,
                    'member permissions differ')
        members[name] = (content, mode & 0o777)

    if path.endswith('.zip'):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            require(len(archive.infolist()) <= entry_limit, 'entry limit')
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
                require(count <= entry_limit and (info.isfile() or info.isdir()), 'link/special/entry limit')
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


def verify_source_archive(path, record, commit, expected_tree, gitlinks, source_identity, build_replacements, reviewed_source=None, legal_files=None, *, legal_pins=False, progress=None):
    """Hash the full Git tree from source tar bytes without extracting anything.

    This independently links the downloaded source archive to the public commit
    tree. Tar metadata is never trusted as a commit assertion. Symlink targets
    are hashed as Git blob bytes and are never followed.
    """
    mark = progress or (lambda stage: None)
    mark('source-start')
    require(re.fullmatch(r'[a-f0-9]{40}', commit) and re.fullmatch(r'[a-f0-9]{40}', expected_tree),
            'invalid source object identity')
    vendors = None
    if reviewed_source is not None:
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'mobile-core'))
        from source_policy import validate_source_policy, verify_source_policy_files
        validate_source_policy(reviewed_source, {**source_identity, 'sourceCommit': commit, 'sourceTree': expected_tree})
        if reviewed_source['schema'] in ('polaris-reviewed-kernel-source-v2', 'polaris-reviewed-kernel-source-v3'):
            vendors = reviewed_source['vendors']
    vendor_prefixes = tuple(pin['replacementPath'][2:] + '/' for pin in vendors.values()) if vendors else ()
    vendor_modes, vendor_bytes, proof_bytes = {}, 0, 0
    source = Path(path)
    require(0 < record['bytes'] <= 512 * 1024 * 1024
            and source.stat().st_size == record['bytes'], 'source archive size differs')
    data = source.read_bytes()
    require(hashlib.sha256(data).hexdigest() == record['sha256'], 'source archive SHA differs')
    mark('archive-hash-verified')
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        expanded = compressed.read(1024 ** 3 + 1)
    require(len(expanded) <= 1024 ** 3, 'source tar expanded size limit')
    mark('archive-expanded')
    if 'transport' in record:
        require(record['transport'] == 'github-commit-archive-v1'
                and record.get('name') == 'sing-box-polaris-' + commit + '.tar.gz'
                and record.get('url') == 'https://github.com/polaris-arch/sing-box-polaris/archive/' + commit + '.tar.gz',
                'fixed commit source transport differs')
        prefix = 'sing-box-polaris-' + commit
        require(isinstance(legal_files, list) and 0 < len(legal_files) <= 1024, 'commit source legal inventory missing')
    else:
        prefix = 'sing-box-polaris-source-' + commit
    if legal_pins:
        require(isinstance(legal_files, list) and 0 < len(legal_files) <= 1024, 'source legal pins inventory missing')
    expected_legal, observed_legal = {}, {}
    if legal_files is not None:
        for legal in legal_files:
            if legal_pins:
                require(isinstance(legal, dict) and set(legal) == {'path', 'bytes', 'sha256'}
                        and type(legal['bytes']) is int and 0 <= legal['bytes'] <= 2 * 1024 * 1024
                        and isinstance(legal['sha256'], str) and re.fullmatch(r'[a-f0-9]{64}', legal['sha256']),
                        'source legal pins identity differs')
            name = legal['path']
            require(isinstance(name, str) and name.startswith('licenses/') and all(
                re.fullmatch(r'[A-Za-z0-9_+.-]+', p) and p not in ['.', '..'] for p in name.split('/')),
                'source legal path differs')
            source_name = 'LICENSE' if name == 'licenses/LICENSE' else 'release/' + name
            require(source_name not in expected_legal, 'source legal identity differs')
            # Only the explicit trusted CLI path omits content. Its Node caller
            # has already checked contract content against these pins; actual
            # tar legal files must still match the entire inventory below.
            if not legal_pins:
                require(isinstance(legal['content'], str)
                        and len(legal['content'].encode()) == legal['bytes']
                        and hashlib.sha256(legal['content'].encode()).hexdigest() == legal['sha256'], 'source legal identity differs')
            expected_legal[source_name] = (legal['bytes'], legal['sha256'])
    mark('legal-inventory-verified')
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
            relative = '/'.join(parts[1:])
            selected_vendor = relative.startswith(vendor_prefixes)
            if selected_vendor:
                require(info.size <= 32 * 1024 * 1024, 'vendor file exceeds byte budget')
                require(vendor_bytes + info.size <= 512 * 1024 * 1024, 'vendor inventory exceeds byte budget')
            if info.issym():
                content, mode = info.linkname.encode('utf-8'), b'120000'
            else:
                with archive.extractfile(info) as stream:
                    content = stream.read(info.size + 1)
                require(len(content) == info.size, 'source file length differs')
                mode = b'100755' if info.mode & 0o111 else b'100644'
            node[leaf] = (mode, object_hash('blob', content))
            if relative in expected_legal:
                require(info.isfile(), 'source legal file is not regular')
                observed_legal[relative] = (len(content), hashlib.sha256(content).hexdigest())
            selected_proof = reviewed_source is not None and ((not vendors and relative.startswith('third_party/metacubex-utls/'))
                or relative in (reviewed_source['metacubex']['bindingPath'], reviewed_source['metacubex']['patchPath']))
            if selected_vendor:
                # Link-target bytes and Git modes are observations, never
                # followed or normalized. The shared verifier permits only
                # the reviewed tailscale/.stignore -> .gitignore link.
                vendor_bytes += len(content)
                require(len(content) <= 32 * 1024 * 1024 and vendor_bytes <= 512 * 1024 * 1024,
                        'vendor inventory exceeds byte budget')
                vendor_modes[relative] = mode.decode('ascii')
                source_files[relative] = content
            elif parts[1:] in [['go.mod'], ['go.sum']] or selected_proof:
                require(info.isfile(), 'source proof files must be regular')
                if selected_proof:
                    require(info.size <= 2 * 1024 * 1024 and
                            (proof_bytes if vendors else sum(len(raw) for raw in source_files.values())) + info.size <= 32 * 1024 * 1024,
                            'reviewed source proof exceeds bound')
                proof_bytes += len(content)
                source_files[relative] = content
    require(observed_legal == expected_legal, 'source legal bytes/inventory differ')
    mark('archive-members-verified')
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
    mark('tree-verified')
    for name, field in [('go.mod', 'goModSha256'), ('go.sum', 'goSumSha256')]:
        require(name in source_files and hashlib.sha256(source_files[name]).hexdigest() == source_identity[field],
                'downloaded source module bytes differ')
    require(source_identity['sourceOverlay'] == 0, 'source overlay unsupported')
    if vendors:
        local_source = source_identity['replacements']
        require(isinstance(local_source, list) and len(local_source) == len(vendors) and
                all(isinstance(item, dict) and set(item) == {'module', 'path', 'tree'} for item in local_source) and
                {item['module']: (item['path'], item['tree']) for item in local_source} ==
                {name: (pin['replacementPath'], pin['replacementTree']) for name, pin in vendors.items()},
                'complete reviewed vendor source replacement inventory differs')
    for replacement in source_identity['replacements']:
        require(replacement['path'].startswith('./'), 'source replacement path differs')
        node = entries
        for part in replacement['path'][2:].split('/'):
            require(part not in ['', '.', '..'] and isinstance(node, dict) and part in node,
                    'source replacement subtree missing')
            node = node[part]
        require(isinstance(node, dict) and tree_hash(node).hex() == replacement['tree'],
                'downloaded local replacement tree differs')
    if reviewed_source is not None:
        meta_tree = entries['third_party']['metacubex-utls']
        vendor_trees = {}
        for name, pin in (vendors or {}).items():
            node = entries
            for part in pin['replacementPath'][2:].split('/'):
                require(isinstance(node, dict) and part in node, 'reviewed vendor subtree missing')
                node = node[part]
            require(isinstance(node, dict), 'reviewed vendor root is not a directory')
            vendor_trees[name] = tree_hash(node).hex()
        verify_source_policy_files(reviewed_source, {**source_identity, 'sourceCommit': commit, 'sourceTree': expected_tree},
            source_files, tree_hash(meta_tree).hex(), **({'vendor_modes': vendor_modes, 'vendor_trees': vendor_trees} if vendors else {}))
    mark('source-policy-verified')
    local_source = source_identity['replacements']
    require(len({item['module'] for item in local_source}) == len(local_source),
            'duplicate source replacement module')
    linked_local = {}
    for item in build_replacements:
        target = item['replacement']
        if vendors and item['module'] in vendors:
            pin = vendors[item['module']]
            require(item['version'] == pin['version'] and target['module'] == pin['replacementPath'],
                    'linked reviewed vendor version/path differs')
        if target['module'].startswith('./'):
            require(item['h1'] == '' and target['version'] == '(devel)' and target['h1'] == '',
                    'local replacement cannot carry an invented module sum')
            previous = linked_local.setdefault(item['module'], target['module'])
            require(previous == target['module'], 'conflicting linked local replacement')
    require(linked_local == {item['module']: item['path'] for item in local_source},
            'complete source/linked local replacement inventory differs')
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
    mark('complete')
    return observed


if __name__ == '__main__':
    # Node sends UTF-8 JSON bytes. Never decode through the Windows console
    # code page or surrogateescape; malformed UTF-8 must remain an error.
    legal_pins = len(sys.argv) == 3 and sys.argv[2] == '--source-legal-pins'
    started = time.monotonic()
    def progress(stage):
        # Fixed stage names and elapsed time only; never emit request contents.
        if legal_pins:
            print(f'SOURCE_ARCHIVE_PROOF stage={stage} elapsed_ms={int((time.monotonic() - started) * 1000)}',
                  file=sys.stderr, flush=True)
    progress('stdin-start')
    raw_request = sys.stdin.buffer.read()
    progress('stdin-read')
    request = json.loads(raw_request.decode('utf-8', errors='strict'))
    progress('request-decoded')
    require(not legal_pins or isinstance(request, dict) and 'sourceArchive' in request, 'source legal pins require source archive')
    require(isinstance(request, dict) and 'profile' not in request, 'unexpected archive profile field')
    # Only trusted command-line call sites select this distinct transport
    # domain. Archive JSON cannot opt into a larger profile or validation-only.
    if len(sys.argv) == 2 and sys.argv[1] == '--check-wire-handoff':
        require(set(request) == {'archive', 'root', 'directories', 'memberBytes'}, 'unexpected wire check fields')
        require(isinstance(request['memberBytes'], dict), 'wire member sizes differ')
        wire_inventory(request['archive'], request['root'], request['directories'], request['memberBytes'])
    elif len(sys.argv) == 4:
        require(sys.argv[3] == '--wire-handoff', 'unknown archive profile')
        require(set(request) == {'archive', 'root'}, 'unexpected wire extraction fields')
        extract(sys.argv[1], sys.argv[2], request['archive'], request['root'], wire_handoff=True)
    elif 'sourceArchive' in request:
        require((len(sys.argv) == 2 or legal_pins) and not sys.argv[1].startswith('--'), 'unexpected source archive arguments')
        verify_source_archive(sys.argv[1], request['sourceArchive'], request['sourceCommit'], request['sourceTree'], request['sourceGitlinks'], request['sourceIdentity'], request['buildReplacements'], request.get('sourcePolicy'), request.get('legalFiles'), legal_pins=legal_pins, progress=progress)
    elif 'wireTransport' in request:
        require(len(sys.argv) == 3 and not sys.argv[1].startswith('--'), 'unexpected wire ZIP arguments')
        extract_wire_transport(sys.argv[1], sys.argv[2], request['wireTransport'])
    else:
        require(len(sys.argv) == 3 and not sys.argv[1].startswith('--'), 'unexpected native archive arguments')
        extract(sys.argv[1], sys.argv[2], request['archive'], request['root'], request.get('target'))
