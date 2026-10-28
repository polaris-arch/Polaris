"""Bounded same-run Apple handoff. Original receipts and paths are never rewritten.

Separate hosted filesystems must expose the same canonical private root. A host
with a different layout fails admission; this is not arbitrary-path relocation.
"""
import contextlib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import shutil
import zipfile

from artifacts import MAX_ARTIFACT, file_pin, read_receipt, receipt_digest, receipt_chunks, tree_inventory
from fork_input import ROOT, decode, load, require
from producer import lease, slot_locks, private_dir, producer_code, run, write_json

SLOTS = ('device-arm64', 'sim-arm64')
MAX_FILES, MAX_BYTES = 65536, 4 * 1024 ** 3


# Closed execution profiles. Component receipts still prove both Libbox slices;
# these profiles describe only the App + PacketTunnel carrier obligations.
SCOPES = {'device-only': ('device-arm64',), 'device-and-simulator': SLOTS}


def required_slots(scope=None):
    scope = os.environ.get('POLARIS_APPLE_SCOPE', 'device-only') if scope is None else scope
    require(isinstance(scope, str) and scope in SCOPES, 'unknown Apple carrier scope')
    return SCOPES[scope]


def scope_fields(scope=None):
    slots = required_slots(scope)
    return {'carrierScope': next(name for name, expected in SCOPES.items() if slots == expected),
            'requiredSlots': list(slots)}


def validate_scope_record(row, kind, scope=None):
    expected = scope_fields(scope)
    schema = row.get('schema')
    if schema == 'polaris-apple-' + kind + '-v1':
        # Historical wrappers mean exactly two carriers. Never reinterpret them
        # as the new default, even if somebody adds a single-slot declaration.
        require(expected['requiredSlots'] == list(SLOTS) and
                'carrierScope' not in row and 'requiredSlots' not in row,
                'legacy Apple handoff requires exact two-slot scope')
        return False
    require(schema == 'polaris-apple-' + kind + '-v2' and
            row.get('carrierScope') == expected['carrierScope'] and
            row.get('requiredSlots') == expected['requiredSlots'],
            'Apple handoff declared scope/requiredSlots differs')
    return True


def transport_pin(path):
    path = Path(path)
    require(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= MAX_BYTES,
            'bounded regular handoff transport required')
    before = path.stat()
    digest, size = hashlib.sha256(), 0
    with path.open('rb') as stream:
        while True:
            raw = stream.read(1024 ** 2)
            if not raw:
                break
            size += len(raw)
            require(size <= MAX_BYTES, 'handoff transport grew beyond budget')
            digest.update(raw)
        after = os.fstat(stream.fileno())
    require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) ==
            (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns) and size == before.st_size,
            'handoff transport changed during hashing')
    return {'bytes': size, 'sha256': digest.hexdigest()}


def context():
    candidate = os.environ.get('POLARIS_APP_CANDIDATE')
    require(candidate and producer_code()['candidate'] == candidate, 'handoff candidate differs')
    require(os.environ.get('GITHUB_ACTIONS') == 'true', 'same-run hosted handoff required')
    run_id, attempt = os.environ.get('GITHUB_RUN_ID'), os.environ.get('GITHUB_RUN_ATTEMPT')
    require(run_id and run_id.isdigit() and attempt and attempt.isdigit(), 'actual run/attempt required')
    return {'candidate': candidate, 'runId': run_id, 'attempt': attempt,
            'repository': os.environ['GITHUB_REPOSITORY']}


def checked_relative(name):
    path = PurePosixPath(name)
    require(isinstance(name, str) and name and not path.is_absolute() and
            '\\' not in name and len(name.encode()) <= 4096 and
            all(p not in ('', '.', '..') and len(p.encode()) <= 255 for p in name.split('/')),
            'unsafe handoff member')
    return Path(name)


def checkout_member(name):
    return name in ('NOTICE', 'LICENSE', 'THIRD-PARTY-LICENSES.md') or any(name.startswith(prefix) for prefix in (
        'resources/data/', 'resources/dashboard/', 'ui/dist/', 'src-tauri/gen/apple/assets/'))


def selected_row(path, base):
    require(path.is_relative_to(base) and path.resolve(strict=True).is_relative_to(base),
            'selected handoff byte escaped owned root')
    mode = stat.S_IMODE(path.lstat().st_mode)
    if path.is_symlink():
        target = os.readlink(path)
        require(not Path(target).is_absolute(), 'absolute framework link cannot be transported')
        return {'link': target, 'mode': mode}
    if path.is_dir():
        return {'directory': True, 'mode': mode}
    return {**file_pin(path, allow_empty=True), 'mode': mode}


def selected_inventory(paths, base):
    """Retain empty directories and every selected parent mode, not files alone."""
    base = Path(base).resolve(strict=True)
    complete = set(paths)
    for path in tuple(complete):
        require(path.is_relative_to(base), 'selected inventory escaped base')
        for parent in path.parents:
            if parent == base:
                break
            require(parent.is_relative_to(base), 'selected parent escaped base')
            complete.add(parent)
    require(len(complete) <= MAX_FILES, 'selected inventory file/directory budget')
    return {p.relative_to(base).as_posix(): selected_row(p, base) for p in sorted(complete)}


def replacement_roots(receipt, component):
    """Use original moduleGraph Main.Dir/Replace.Dir, never the supplied clone."""
    graph = receipt['input']['moduleGraph']
    main = [r for r in graph if r.get('Main') is True]
    require(len(main) == 1 and isinstance(main[0].get('Dir'), str), 'one actual main source view required')
    source = Path(main[0]['Dir'])
    require(source.is_absolute() and source.is_relative_to(component) and
            source.resolve(strict=True) == source and source.is_dir(), 'actual source view redirected/missing')
    roots = []
    from artifacts import local_replacement_aliases
    for row in graph:
        replacement = row.get('Replace')
        if not replacement or replacement.get('Version'):
            continue
        local_replacement_aliases(replacement, str(source), replacement['Path'], row['Path'])
        directory = Path(replacement['Dir'])
        for parent in (directory, *directory.parents):
            require(not parent.is_symlink(), 'local replacement source redirected')
            if parent == source:
                break
        require(directory.is_dir(), 'actual local replacement source missing')
        roots.append({'module': row['Path'], 'path': replacement['Path'],
                      'sourceRoot': str(source), 'directory': str(directory)})
    require(len(roots) <= 1024, 'local replacement source count budget')
    return roots


def replacement_snapshots(receipt, component):
    snapshots = []
    for row in replacement_roots(receipt, component):
        source, directory = Path(row['sourceRoot']), Path(row['directory'])
        inventory = tree_inventory(directory)
        require(any(r['type'] == 'file' for r in inventory), 'local replacement source bytes required')
        snapshots.append({**row, 'inventory': inventory, 'goMod': file_pin(source / 'go.mod'),
                          'goSum': file_pin(source / 'go.sum')})
    return snapshots


def verify_replacements(snapshots, receipt, component):
    expected = replacement_roots(receipt, component)
    require(len(snapshots) == len(expected), 'actual local replacement sources omitted')
    for snapshot, row in zip(snapshots, expected):
        require({k: snapshot[k] for k in row} == row, 'local replacement source identity changed')
        source, directory = Path(row['sourceRoot']), Path(row['directory'])
        require(file_pin(source / 'go.mod') == snapshot['goMod'] and
                file_pin(source / 'go.sum') == snapshot['goSum'] and
                tree_inventory(directory) == snapshot['inventory'], 'actual local replacement source bytes/modes changed')
        value = receipt['input']['sourceContract']
        require(snapshot['goMod']['sha256'] == value['goModSha256'] and
                snapshot['goSum']['sha256'] == value['goSumSha256'], 'retained source identity bytes differ')


def write_payload(archive, inventories, root, checkout):
    for namespace, entries in inventories.items():
        base = root if namespace == 'root' else checkout
        for name, row in entries.items():
            source = base / name
            info = zipfile.ZipInfo(namespace + '/' + name)
            info.create_system = 3
            info.compress_type = zipfile.ZIP_DEFLATED
            kind = stat.S_IFDIR if row.get('directory') is True else stat.S_IFLNK if 'link' in row else stat.S_IFREG
            info.external_attr = ((kind | row['mode']) << 16) | (0x10 if kind == stat.S_IFDIR else 0)
            if kind == stat.S_IFDIR:
                archive.writestr(info, b'')
                continue
            with archive.open(info, 'w', force_zip64=True) as stream:
                if 'link' in row:
                    stream.write(row['link'].encode())
                else:
                    with source.open('rb') as incoming:
                        shutil.copyfileobj(incoming, stream, 1024 ** 2)


def restore_payload(archive, inventories, root, checkout):
    """Create each owned parent explicitly; apply original directory modes last."""
    directories = []
    for namespace, rows in inventories.items():
        require(namespace in ('root', 'checkout'), 'unknown handoff namespace')
        base = root if namespace == 'root' else checkout
        for name, row in sorted(rows.items(), key=lambda pair: (len(Path(pair[0]).parts), pair[0])):
            dest = base / checked_relative(name)
            if namespace == 'checkout':
                require(checkout_member(name) or any(other.startswith(name + '/') and checkout_member(other)
                        for other in rows), 'unexpected checkout handoff member')
            for parent in dest.parents:
                require(not parent.is_symlink(), 'handoff destination parent redirected')
                if parent == base:
                    break
            if row.get('directory') is not True:
                continue
            require(set(row) == {'directory', 'mode'} and type(row['mode']) is int and 0 <= row['mode'] <= 0o777,
                    'invalid directory row')
            if namespace == 'root' and name in ('component', 'cache', 'tools', 'component/scratch'):
                require(row['mode'] == 0o700, 'actual private owned slot mode differs')
            info = archive.getinfo(namespace + '/' + name)
            require(info.file_size == 0 and not info.flag_bits & 1 and
                    stat.S_IFMT(info.external_attr >> 16) == stat.S_IFDIR and
                    stat.S_IMODE(info.external_attr >> 16) == row['mode'], 'handoff directory member differs')
            if dest.exists() or dest.is_symlink():
                require(namespace == 'checkout' and not dest.is_symlink() and selected_row(dest, base) == row,
                        'handoff directory occupied or original mode changed')
            else:
                require(dest.parent.is_dir(), 'handoff directory parent inventory missing')
                dest.mkdir(mode=0o700)
                dest.chmod(0o700)  # Explicitly defeat any caller umask.
                directories.append((dest, row['mode']))
    pending_links = []
    for links in (False, True):
        for namespace, rows in inventories.items():
            base = root if namespace == 'root' else checkout
            for name, row in rows.items():
                if row.get('directory') is True or ('link' in row) != links:
                    continue
                dest = base / checked_relative(name)
                require(dest.parent.is_dir() and not dest.parent.is_symlink(), 'handoff file parent inventory missing')
                info = archive.getinfo(namespace + '/' + name)
                require(not info.flag_bits & 1 and stat.S_IMODE(info.external_attr >> 16) == row['mode'] and
                        stat.S_IFMT(info.external_attr >> 16) == (stat.S_IFLNK if links else stat.S_IFREG),
                        'handoff member mode/kind differs')
                if dest.exists() or dest.is_symlink():
                    require(namespace == 'checkout' and not dest.is_symlink() and selected_row(dest, base) == row,
                            'handoff destination occupied or original source changed')
                    continue
                if links:
                    require(info.file_size <= 4096, 'handoff symlink target budget')
                    target = archive.read(info).decode()
                    require(target == row['link'] and not Path(target).is_absolute(),
                            'handoff symlink escaped bound root')
                    pending_links.append((dest, target, base, row['mode']))
                else:
                    require(info.file_size == row['bytes'], 'handoff member length differs')
                    with archive.open(info) as source, dest.open('xb') as output:
                        shutil.copyfileobj(source, output, 1024 ** 2)
                    dest.chmod(row['mode'])
    # Versioned frameworks contain Headers -> Versions/Current/Headers before
    # Versions/Current -> A in the sorted inventory. Restore only links whose
    # targets already exist; each pass must remove at least one dependency.
    # No link is installed before strict containment succeeds. Missing targets
    # and dependency cycles fail closed rather than becoming dangling links.
    while pending_links:
        unresolved = []
        for dest, target, base, mode in pending_links:
            try:
                resolved = (dest.parent / target).resolve(strict=True)
            except FileNotFoundError:
                unresolved.append((dest, target, base, mode))
                continue
            require(resolved.is_relative_to(base.resolve(strict=True)),
                    'handoff symlink escaped bound root')
            dest.symlink_to(target)
            if stat.S_IMODE(dest.lstat().st_mode) != mode:
                dest.lchmod(mode)  # The link itself, never its framework target.
        require(len(unresolved) < len(pending_links), 'handoff symlink dependency missing or cyclic')
        pending_links = unresolved
    for path, mode in reversed(directories):
        path.chmod(mode)
    for namespace, rows in inventories.items():
        base = root if namespace == 'root' else checkout
        for name, row in rows.items():
            require(selected_row(base / name, base) == row, 'restored directory/file/link inventory differs')


# Actions supplies the trust boundary, not these self-reported integrity hashes.
# Every eligible main writer is trusted; no prefix restore or package URL exists.
COMPONENT_CACHE_SCHEMA = 'polaris-apple-complete-component-cache-v1'


def component_cache_eligible():
    repository = os.environ.get('GITHUB_REPOSITORY')
    return (repository == 'polaris-arch/Polaris' and os.environ.get('GITHUB_ACTIONS') == 'true' and
            os.environ.get('GITHUB_EVENT_NAME') == 'push' and os.environ.get('GITHUB_REF') == 'refs/heads/main' and
            os.environ.get('POLARIS_APP_CANDIDATE') == os.environ.get('GITHUB_SHA') and
            os.environ.get('GITHUB_WORKFLOW_REF') == repository + '/.github/workflows/release-risk.yml@refs/heads/main')


def component_cache_path():
    return ROOT / 'target/apple-go-component-cache'


def original_component_root(root, current):
    """Only an unredirected sibling run namespace, never an arbitrary archive path."""
    import re
    root, current = Path(root), Path(current)
    base = Path.home() / 'Code/polaris-mobile-apple-ci'
    require(root.is_absolute() and root.parent == base and re.fullmatch(r'[1-9][0-9]{0,19}-[1-9][0-9]{0,9}', root.name)
            and root != current and root.resolve(strict=False) == root,
            'cached component original root is not an independent canonical hosted slot')
    return root


def component_members(receipt, component):
    paths = {component / 'component-receipt.json', component / 'original-bind.log'}
    paths.update(Path(p) for p in receipt['nativeInputs']['files'])
    snapshots = replacement_snapshots(receipt, component)
    for directory in [component / 'Libbox.xcframework', Path(receipt['nativeInputs']['generatorOutputRoot']),
                      *[Path(row['directory']) for row in snapshots]]:
        paths.add(directory)
        paths.update(directory.rglob('*'))
    for row in snapshots:
        paths.update(Path(row['sourceRoot']) / name for name in ('go.mod', 'go.sum'))
    return paths, snapshots


class ComponentArchive:
    """Read-only root namespace adapter for the existing original closure reader."""
    def __init__(self, archive):
        self.archive = archive

    def infolist(self):
        import copy
        result = []
        for row in self.archive.infolist():
            if row.filename.startswith('root/'):
                row = copy.copy(row)
                row.filename = row.filename[len('root/'):]
                result.append(row)
        return result

    def getinfo(self, name):
        return self.archive.getinfo('root/' + name)

    def open(self, name):
        return self.archive.open(name if isinstance(name, zipfile.ZipInfo) else 'root/' + name)

    def read(self, name):
        return self.archive.read('root/' + name)


def read_component_package(archive, current_root, binding):
    """Pure bounded cache validation. No original/current filesystem is changed."""
    from artifacts import MAX_APPLE_RECEIPT
    from apple_merge_reuse import zip_inventory, json_member, member_pin, validate_native_closure
    from producer import (APPLE_COMPONENT_CACHE_POLICY, apple_cache_binding, equivalent_apple_cache_binding,
                          cacheable_native_environment)
    entries = zip_inventory(archive)
    manifest = json_member(archive, 'component-cache.json')
    require(isinstance(manifest, dict), 'component cache manifest must be an object')
    require(manifest.get('schema') == COMPONENT_CACHE_SCHEMA and
            manifest.get('policy') == APPLE_COMPONENT_CACHE_POLICY and
            manifest.get('fingerprint') == receipt_digest({k: v for k, v in manifest.items() if k != 'fingerprint'}),
            'complete component cache manifest differs')
    original_root = original_component_root(manifest['root'], current_root)
    require(manifest['identity'] == receipt_digest(apple_cache_binding(binding)), 'component cache key differs')
    require(set(manifest['files']) == {'root'} and isinstance(manifest['files']['root'], dict),
            'component cache namespace differs')
    rows = manifest['files']['root']
    require(all(isinstance(name, str) and isinstance(row, dict) for name, row in rows.items()),
            'component cache inventory rows required')
    require(len(rows) <= MAX_FILES and sum(row.get('bytes', 0) for row in rows.values()) <= MAX_BYTES,
            'component cache inventory budget')
    require(set(entries) == {'component-cache.json'} | {'root/' + name for name in rows},
            'component cache extra/missing member')
    # Restore topology is checked before creating the historical namespace.
    # Files cannot have missing/non-directory parents, and framework links
    # must resolve using only this inventory, including Versions/Current links.
    for name, row in rows.items():
        checked_relative(name)
        for parent in Path(name).parents:
            if str(parent) != '.':
                require(rows.get(parent.as_posix(), {}).get('directory') is True,
                        'component cache parent directory inventory missing')
        if name in ('component', 'cache', 'component/scratch'):
            require(row.get('directory') is True and row.get('mode') == 0o700,
                    'component cache private slot mode differs')
    def link_target(name, visited):
        parts = Path(name).parts
        for count in range(1, len(parts) + 1):
            prefix = '/'.join(parts[:count])
            row = rows.get(prefix)
            require(row is not None, 'component cache link target missing')
            if 'link' in row:
                require(prefix not in visited and len(visited) < 128, 'component cache link dependency cyclic')
                checked_relative(row['link'])
                target = (Path(prefix).parent / row['link']).as_posix()
                if count < len(parts): target += '/' + '/'.join(parts[count:])
                return link_target(target, visited | {prefix})
            require(count == len(parts) or row.get('directory') is True,
                    'component cache link target parent is not a directory')
        return name
    for name, row in rows.items():
        if 'link' in row:
            link_target(name, set())
    original = {}
    for name, row in rows.items():
        checked_relative(name)
        require(name == 'component' or name.startswith('component/') or name == 'cache' or name.startswith('cache/'),
                'component cache contains an unrelated root')
        require(type(row['mode']) is int and 0 <= row['mode'] <= 0o777, 'component cache member mode')
        info = entries['root/' + name]
        require(stat.S_IMODE(info.external_attr >> 16) == row['mode'], 'component cache archive mode differs')
        if row.get('directory') is True:
            require(set(row) == {'directory', 'mode'} and info.file_size == 0 and
                    stat.S_ISDIR(info.external_attr >> 16), 'component cache directory differs')
        elif 'link' in row:
            require(set(row) == {'link', 'mode'} and stat.S_ISLNK(info.external_attr >> 16) and
                    isinstance(row['link'], str) and not Path(row['link']).is_absolute() and
                    '..' not in Path(row['link']).parts and len(row['link'].encode()) <= 4096 and
                    info.file_size == len(row['link'].encode()) and archive.read(info) == row['link'].encode(),
                    'component cache symlink differs')
            original[name] = {'originalPath': str(original_root / name), 'link': row['link']}
        else:
            require(set(row) == {'bytes', 'sha256', 'mode'} and stat.S_ISREG(info.external_attr >> 16) and
                    type(row['bytes']) is int and 0 <= row['bytes'] <= 1024 ** 3 and
                    member_pin(archive, 'root/' + name, row['bytes']) == {k: row[k] for k in ('bytes', 'sha256')},
                    'component cache bytes differ')
            original[name] = {'originalPath': str(original_root / name), **{k: row[k] for k in ('bytes', 'sha256')}}
    receipt = json_member(archive, 'root/component/component-receipt.json', MAX_APPLE_RECEIPT)
    require(isinstance(receipt, dict), 'component cache receipt must be an object')
    require(receipt.get('schema') == 'polaris-mobile-component-v1' and receipt.get('platform') == 'apple' and
            type(receipt.get('sourceOverlay')) is int and receipt.get('sourceOverlay') == 0 and
            receipt.get('AppAccepted') is False and receipt.get('devicePassed') is False and
            receipt.get('fingerprint') == receipt_digest({k: v for k, v in receipt.items() if k != 'fingerprint'}) and
            manifest['componentFingerprint'] == receipt['fingerprint'] and
            manifest['componentReceipt'] == member_pin(archive, 'root/component/component-receipt.json', MAX_APPLE_RECEIPT)
            and cacheable_native_environment(receipt['input']) and cacheable_native_environment(binding)
            and equivalent_apple_cache_binding(receipt['input'], binding), 'cached component input differs')
    require(manifest['originalContext']['repository'] == 'polaris-arch/Polaris' and
            manifest['originalContext']['candidate'] == receipt['input']['producer']['candidate'] and
            original_root.name == manifest['originalContext']['runId'] + '-' + manifest['originalContext']['attempt'],
            'cached original informational context differs')
    validate_native_closure(ComponentArchive(archive), {'files': original}, manifest, receipt)
    return manifest, receipt


def component_reuse_record(facts):
    """Pure aggregate edge checks, independent of either producer filesystem."""
    row = facts['componentReuse']
    require(row.get('schema') == 'polaris-apple-component-cache-observation-v1' and
            row.get('fingerprint') == receipt_digest({k: v for k, v in row.items() if k != 'fingerprint'}) and
            row.get('context') == facts['context'] and row.get('currentVerifier') == facts['App'] and
            row.get('trust') == 'trusted-main-cache' and row.get('inputsRevalidated') is True and
            row.get('historicalProviderAuthenticated') is False and row.get('currentRunProducedComponent') is False,
            'current component reuse observation differs')
    require(row.get('devicePassed') is False and row.get('publicationEligible') is False and
            isinstance(row.get('originalProducer'), dict) and
            row['originalProducer'].get('candidate') == row.get('originalContextInformational', {}).get('candidate') and
            row['originalContextInformational'].get('repository') == facts['context']['repository'],
            'component reuse original identity or claims differ')
    import re
    historical = row['originalContextInformational']
    require(re.fullmatch(r'[0-9a-f]{40}', row['originalProducer'].get('candidate', '')) and
            re.fullmatch(r'[1-9][0-9]{0,19}', historical.get('runId', '')) and
            re.fullmatch(r'[1-9][0-9]{0,9}', historical.get('attempt', '')) and
            Path(row['originalRoot']).parent == Path(facts['root']).parent and
            Path(row['originalRoot']).name == historical['runId'] + '-' + historical['attempt'] and
            row['originalRoot'] != facts['root'] and
            {k: v for k, v in row['originalProducer'].items() if k != 'candidate'} ==
            {k: v for k, v in facts['App'].items() if k != 'candidate'} and
            re.fullmatch(r'polaris-mobile-[0-9a-f]{64}', row.get('originalBuildID', '')) and
            re.fullmatch(r'[0-9a-f]{64}', row.get('identity', '')),
            'component reuse historical/current namespace or producer differs')
    for name, key in (('retained-component.zip', 'package'), ('component-current-binding.json', 'currentBinding')):
        inventory = facts['files']['root'].get(name, {})
        require(not inventory.get('directory') and 'link' not in inventory and
                row[key] == {k: inventory.get(k) for k in ('bytes', 'sha256')},
                'component reuse package/admission inventory edge differs')
    return row


def validate_component_reuse(facts, root):
    """A current observation, never a claim of authenticated historical production."""
    from producer import apple_cache_binding, equivalent_apple_cache_binding
    row = component_reuse_record(facts)
    original_root = original_component_root(row['originalRoot'], root)
    require(file_pin(root / 'retained-component.zip') == row['package'] and
            file_pin(root / 'component-current-binding.json') == row['currentBinding'],
            'retained original package/current admission changed')
    binding = read_receipt(root / 'component-current-binding.json')
    receipt = read_receipt(original_root / 'component/component-receipt.json')
    require(binding['producer'] == facts['App'] and binding['sourceContract'] == facts['sourceContract'] and
            row['identity'] == receipt_digest(apple_cache_binding(binding)) and
            equivalent_apple_cache_binding(receipt['input'], binding) and
            receipt['input']['producer'] == row['originalProducer'] and
            receipt['fingerprint'] == facts['componentFingerprint'] and
            row['originalBuildID'] == 'polaris-mobile-' + receipt_digest(receipt['input']),
            'reused original/current component identities differ')
    return original_root, receipt


def export_component_package(root):
    from producer import APPLE_COMPONENT_CACHE_POLICY, apple_cache_binding, cacheable_native_environment
    require(component_cache_eligible(), 'component cache export requires trusted ordinary main push')
    root = Path(root)
    require(read_receipt(root / 'import-observation.json')['completed'] is True,
            'complete local component verification required before cache export')
    binding = read_receipt(root / 'component-current-binding.json')
    require(binding['producer'] == producer_code(), 'current component cache producer changed')
    if not cacheable_native_environment(binding):
        with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
            output.write('exported=false\n')
        print('Custom native environment requires fresh production; package not exported', flush=True)
        return
    component = root / 'component'
    receipt = read_receipt(component / 'component-receipt.json')
    require(receipt['input'] == binding, 'only the newly produced exact component may be exported')
    paths, snapshots = component_members(receipt, component)
    facts = {'schema': COMPONENT_CACHE_SCHEMA, 'policy': APPLE_COMPONENT_CACHE_POLICY,
             'identity': receipt_digest(apple_cache_binding(binding)), 'root': str(root),
             'originalContext': context(), 'componentReceipt': file_pin(component / 'component-receipt.json'),
             'componentFingerprint': receipt['fingerprint'], 'files': {'root': selected_inventory(paths, root)},
             'localReplacementSources': snapshots}
    facts['fingerprint'] = receipt_digest(facts)
    folder = component_cache_path()
    # Invalid/partial restored bytes remain diagnostic evidence. Do not save
    # foreign extra files, follow unsafe paths, or turn a successful bind into
    # failure merely because Actions left an occupied restore directory.
    occupied = (any(parent.is_symlink() for parent in (folder, *folder.parents)) or
                (folder.exists() and (not folder.is_dir() or any(folder.iterdir()))))
    if occupied:
        with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
            output.write('exported=false\n')
        print('Existing restored package retained; immutable cache key not replaced', flush=True)
        return
    require(folder.resolve(strict=False) == folder, 'cache export path redirected')
    folder.mkdir(parents=True, exist_ok=True)
    path = folder / 'component.zip'
    with lease(private_dir(root / 'component-cache-export'), root / 'cache', load()):
        require(selected_inventory(paths, root) == facts['files']['root'], 'original package inputs changed before export')
        class BoundedPackage:
            def __init__(self, stream): self.stream = stream
            def write(self, raw):
                require(self.stream.tell() + len(raw) <= MAX_ARTIFACT, 'component cache compressed byte budget')
                return self.stream.write(raw)
            def __getattr__(self, name): return getattr(self.stream, name)
        with path.open('xb') as stream, zipfile.ZipFile(BoundedPackage(stream), 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            write_payload(archive, facts['files'], root, ROOT)
            archive.writestr('component-cache.json', b''.join(receipt_chunks(facts)))
        require(selected_inventory(paths, root) == facts['files']['root'], 'original package inputs changed during export')
        file_pin(path)  # Also keep retained nested originals under the existing artifact bound.
        with zipfile.ZipFile(path) as archive:
            # Validation is read-only here. Excluding a real consumer namespace
            # is meaningful only for import; this is still the original run.
            read_component_package(archive, root / 'export-verification', binding)
    with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
        output.write('exported=true\n')
    print('Complete verified original component cache prepared; cache hashes prove integrity only', flush=True)


def validate_manifest(manifest, root, checkout, expected=None, *, scope=None):
    validate_scope_record(manifest, 'component-handoff', scope)
    require(manifest.get('fingerprint') == receipt_digest({k: v for k, v in manifest.items() if k != 'fingerprint'}),
            'Apple handoff manifest changed')
    require(manifest['context'] == (expected or context()) and manifest['producerJob'] == 'component',
            'handoff is not this run/attempt/component job')
    require(manifest['root'] == str(root.resolve()) and manifest['checkout'] == str(checkout.resolve()),
            'host canonical layout differs; original absolute paths cannot be relabelled')
    require(manifest['App'] == producer_code() and manifest['sourceContract'] == load(),
            'handoff App/source contract differs')
    require(set(manifest['files']) == {'root', 'checkout'}, 'handoff root inventory differs')
    total, count = 0, 0
    for namespace, rows in manifest['files'].items():
        require(isinstance(rows, dict), 'handoff inventory required')
        base = root if namespace == 'root' else checkout
        for name, wanted in rows.items():
            checked_relative(name)
            if namespace == 'checkout':
                require(checkout_member(name) or wanted.get('directory') is True and
                        any(other.startswith(name + '/') and checkout_member(other) for other in rows),
                        'unexpected checkout handoff member')
            path = base / name
            count += 1
            total += wanted.get('bytes', 0)
            require(count <= MAX_FILES and total <= MAX_BYTES, 'handoff selected bytes budget')
            require(selected_row(path, base) == wanted, 'imported handoff bytes/mode/link changed')
    if 'componentReuse' in manifest:
        validate_component_reuse(manifest, root)
    for snapshot in manifest.get('localReplacementSources', []):
        require(tree_inventory(Path(snapshot['directory'])) == snapshot['inventory'],
                'retained local replacement inventory changed')
    return manifest


def freeze(root):
    root = Path(root).resolve(strict=True)
    tools = read_receipt(root / 'tools.json')
    require(all(tools.get(k) == v for k, v in scope_fields().items()),
            'prepared Apple carrier scope differs before freeze')
    component = Path(tools['paths']['component-work'])
    receipt = read_receipt(component / 'component-receipt.json')
    reuse = (read_receipt(root / 'component-cache-observation.json')
             if (root / 'component-cache-observation.json').is_file() else None)
    require(receipt['input']['sourceContract'] == load() and
            ((reuse is None and component == root / 'component' and receipt['input']['producer'] == producer_code()) or
             (reuse is not None and component == Path(reuse['originalRoot']) / 'component' and
              receipt['input']['producer'] == reuse['originalProducer'])), 'original component producer differs')
    from consume import verify_original, apple_resource_inputs, apple_resources
    with lease(private_dir(root / 'handoff-work'), Path(tools['paths']['cache']), load()), \
            (slot_locks(component, component.parent / 'cache') if reuse is not None else contextlib.nullcontext()):
        verify_original(component / 'Libbox.xcframework', component / 'component-receipt.json', 'apple', {},
                        private_dir(root / 'handoff-work/check'))
        resources = apple_resources()
        paths = {root / 'tools.json', Path(tools['paths']['gomobile']), Path(tools['paths']['gobind'])}
        if reuse is not None:
            paths.update(root / name for name in ('retained-component.zip', 'component-current-binding.json',
                                                 'component-cache-observation.json'))
            snapshots = replacement_snapshots(receipt, component)
            verify_replacements(snapshots, receipt, component)
        else:
            originals, snapshots = component_members(receipt, component)
            paths.update(originals)
            from fork_input import verify_source
            for source_view in {row['sourceRoot'] for row in snapshots}:
                require(verify_source(Path(source_view), load(), generator_outputs=True) ==
                        receipt['input']['sourceObservation'], 'replacement source view differs from original producer')
        checkout_paths = {ROOT / row['source'] for row in apple_resource_inputs().values()}
        for generated in (ROOT / 'ui/dist', ROOT / 'resources/dashboard'):
            require(generated.is_dir() and not generated.is_symlink(), 'actual generated frontend/dashboard required')
            checkout_paths.add(generated)
            checkout_paths.update(generated.rglob('*'))
        assets = ROOT / 'src-tauri/gen/apple/assets'
        checkout_paths.add(assets)
        checkout_paths.update(assets.rglob('*'))
        rows = {'root': selected_inventory(paths, root), 'checkout': selected_inventory(checkout_paths, ROOT)}
        require(sum(len(v) for v in rows.values()) <= MAX_FILES and
                sum(row.get('bytes', 0) for v in rows.values() for row in v.values()) <= MAX_BYTES,
                'selected Apple handoff too large')
        facts = {'schema': 'polaris-apple-component-handoff-v2', **scope_fields(), 'context': context(),
                 'producerJob': os.environ['GITHUB_JOB'], 'root': str(root), 'checkout': str(ROOT),
                 'App': producer_code(), 'sourceContract': load(), 'files': rows,
                 'componentReceipt': file_pin(component / 'component-receipt.json'),
                 'componentFingerprint': receipt['fingerprint'], 'localReplacementSources': snapshots,
                 'tools': tools['tools'], 'resources': resources, 'devicePassed': False,
                 'signatureTrustValidated': False, 'publicationEligible': False}
        if reuse is not None:
            facts['componentReuse'] = reuse
        facts['fingerprint'] = receipt_digest(facts)
        validate_manifest(facts, root, ROOT)
        write_json(root / 'component-handoff.json', facts)
        archive_path = root / 'component-handoff.zip'
        require(not archive_path.exists(), 'previous handoff must not be overwritten')
        with zipfile.ZipFile(archive_path, 'x', compression=zipfile.ZIP_DEFLATED) as archive:
            write_payload(archive, rows, root, ROOT)
            archive.write(root / 'component-handoff.json', 'component-handoff.json')
        # Re-read after packaging to preserve the mutation boundary, not merely
        # reuse the first inventory as if the bytes could not change.
        validate_manifest(facts, root, ROOT)
        transport = {'schema': 'polaris-apple-handoff-transport-v1', 'context': context(),
                     'archive': transport_pin(archive_path), 'manifest': file_pin(root / 'component-handoff.json')}
        transport['fingerprint'] = receipt_digest(transport)
        write_json(root / 'component-transport.json', transport)


def import_bundle(folder, root):
    folder, root = Path(folder), private_dir(root)
    transport = read_receipt(folder / 'component-transport.json')
    require(transport.get('schema') == 'polaris-apple-handoff-transport-v1' and transport['context'] == context() and
            transport['fingerprint'] == receipt_digest({k: v for k, v in transport.items() if k != 'fingerprint'}) and
            transport_pin(folder / 'component-handoff.zip') == transport['archive'], 'handoff transport changed')
    write_json(root / 'import-observation.json', {'schema': 'polaris-apple-import-observation-v1',
               'context': context(), 'transport': transport, 'completed': False, 'devicePassed': False})
    with zipfile.ZipFile(folder / 'component-handoff.zip') as archive:
        infos = archive.infolist()
        require(len(infos) <= MAX_FILES + 1 and len({i.filename for i in infos}) == len(infos) and
                sum(i.file_size for i in infos) <= MAX_BYTES + 16 * 1024 ** 2, 'handoff archive bounds')
        require(archive.getinfo('component-handoff.json').file_size <= 16 * 1024 ** 2,
                'handoff manifest byte budget')
        raw = archive.read('component-handoff.json')
        require({'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()} == transport['manifest'],
                'handoff manifest bytes differ')
        facts = decode(raw, maximum=16 * 1024 ** 2)
        validate_scope_record(facts, 'component-handoff')
        require(facts['context'] == context() and facts['root'] == str(root.resolve()) and
                facts['checkout'] == str(ROOT.resolve()), 'host layout/context differs before extraction')
        expected = {namespace + '/' + name for namespace, rows in facts['files'].items() for name in rows}
        require({i.filename for i in infos} == expected | {'component-handoff.json'}, 'extra/missing handoff members')
        restore_payload(archive, facts['files'], root, ROOT)
        (root / 'component-handoff.json').write_bytes(raw)
        write_json(root / 'component-transport.json', transport)
    if 'componentReuse' in facts:
        binding = read_receipt(root / 'component-current-binding.json')
        with lease(private_dir(root / 'original-import-work'), private_dir(root / 'cache'), load()):
            with zipfile.ZipFile(root / 'retained-component.zip') as original:
                old, _ = read_component_package(original, root, binding)
                original_root = Path(old['root'])
                require(not original_root.exists(), 'original import namespace occupied')
                private_dir(original_root)
                with slot_locks(original_root, original_root):
                    restore_payload(original, old['files'], original_root, ROOT)
    validate_manifest(facts, root, ROOT)
    with lease(private_dir(root / 'import-work'), private_dir(root / 'cache'), load()):
        imported_paths = read_receipt(root / 'tools.json')['paths']
        verify_import(Path(imported_paths['component-work']), root / 'cache', *(imported_paths[n]
                      for n in ('source', 'go', 'gomobile', 'gobind', 'developer')), handoff_root=root)
    write_json(root / 'import-observation.json', {'schema': 'polaris-apple-import-observation-v1',
               'context': context(), 'transport': transport, 'completed': True, 'devicePassed': False})
    return read_receipt(root / 'tools.json')


def verify_import(component, cache, source, go, gomobile, gobind, developer, *, handoff_root=None):
    root = Path(handoff_root or os.environ.get('POLARIS_FORMAL_APPLE_ROOT') or Path(component).parent)
    facts = validate_manifest(read_receipt(root / 'component-handoff.json'), root, ROOT)
    paths = read_receipt(root / 'tools.json')['paths']
    actual = {'component-work': str(Path(component)), 'cache': str(Path(cache)), 'source': str(Path(source)),
              'go': str(Path(go)), 'gomobile': str(Path(gomobile)), 'gobind': str(Path(gobind)),
              'developer': str(Path(developer))}
    require(all(paths[name] == path for name, path in actual.items()), 'imported producer paths changed')
    for name, path in [('go', go), ('gomobile', gomobile), ('gobind', gobind)]:
        require(file_pin(path) == facts['tools'][name], 'actual imported tool bytes differ')
    value = load()
    receipt = read_receipt(Path(component) / 'component-receipt.json')
    require(file_pin(Path(component) / 'component-receipt.json') == facts['componentReceipt'] and
            receipt['input']['producer'] == facts.get('componentReuse', {}).get('originalProducer', facts['App']) and
            receipt['input']['sourceContract'] == value and
            receipt['input']['tools']['executables'] == facts['tools'],
            'original producer receipt differs')
    # Same source/tool result can be handed off, but this host must supply the
    # same actual generator, SDK settings and compiler bytes, not merely a label.
    from producer import tool_facts
    from types import SimpleNamespace
    env = dict(os.environ, DEVELOPER_DIR=str(developer))
    _, observed_tools = tool_facts(SimpleNamespace(platform='apple', go=go, gomobile=gomobile,
                                                   gobind=gobind), value, env)
    expected_tools = (read_receipt(root / 'component-current-binding.json')['tools']
                      if 'componentReuse' in facts else receipt['input']['tools'])
    require(observed_tools == expected_tools, 'actual consumer tool/SDK bytes differ')
    current_binding = (read_receipt(root / 'component-current-binding.json')
                       if 'componentReuse' in facts else receipt['input'])
    if 'nativeEnvironment' in current_binding:
        from producer import apple_native_environment
        from fork_input import tool_environment
        native_env = tool_environment(go, gomobile, gobind, cache, root / 'import-check', developer=Path(developer))
        require(apple_native_environment(go, native_env) == current_binding['nativeEnvironment'],
                'actual consumer Go/native environment differs')
    with (slot_locks(Path(component), Path(component).parent / 'cache')
          if 'componentReuse' in facts else contextlib.nullcontext()):
        verify_replacements(facts['localReplacementSources'], receipt, Path(component))
        from consume import verify_original
        verify_original(Path(component) / 'Libbox.xcframework', Path(component) / 'component-receipt.json',
                        'apple', {}, private_dir(root / 'import-check'))
    return receipt


def carrier_summary(root, slot):
    require(slot in required_slots(), 'unrequested Apple carrier slot')
    root = Path(root)
    receipt_path = root / slot / 'app-carrier.json'
    receipt = read_receipt(receipt_path)
    require(receipt['App']['candidate'] == context()['candidate'] and receipt['devicePassed'] is False,
            'carrier candidate/device scope differs')
    row = {'schema': 'polaris-apple-carrier-handoff-v2', **scope_fields(), 'context': context(), 'slot': slot,
           'componentTransport': read_receipt(root / 'component-transport.json'),
           'receipt': receipt, 'receiptPin': file_pin(receipt_path),
           'devicePassed': False, 'signatureTrustValidated': False, 'publicationEligible': False}
    row['fingerprint'] = receipt_digest(row)
    rust_path = Path(receipt['rustInput']['receipt'])
    require(rust_path.is_relative_to(root / slot) and file_pin(rust_path) ==
            {k: receipt['rustInput'][k] for k in ('bytes', 'sha256')}, 'original Rust receipt changed')
    # One host executes several slots. Neither a later slot nor a repeated
    # export may replace an earlier slot's accepted original bytes.
    summary = root / 'summaries' / slot
    private_dir(summary.parent)
    summary.mkdir(mode=0o700)
    write_json(summary / ('carrier-' + slot + '.json'), row)
    for source, name in [(receipt_path, 'summary-app-carrier.json'),
                         (rust_path, 'summary-rust-receipt.json')]:
        expected = file_pin(source)
        with source.open('rb') as original, (summary / name).open('xb') as output:
            shutil.copyfileobj(original, output)
        require(file_pin(source) == expected and file_pin(summary / name) == expected,
                'original summary bytes changed during export')


def validate_aggregate(folder, *, scope, expected_context, expected_producer, expected_app, expected_source_contract):
    """Pure historical-or-current evidence check; never rewrites producer identity.

    The current-run entry below supplies current identities. A separate merge
    verifier must prove eligibility before supplying historical expectations.
    """
    require(isinstance(scope, str) and scope in SCOPES, 'explicit aggregate scope required')
    slots = required_slots(scope)
    folder = Path(folder)
    require({p.name for p in folder.iterdir()} == {'component', *slots},
            'aggregate requires exactly component and declared carrier slots')
    rows = {}
    current_app = expected_app
    compiler_identity = None
    transport = read_receipt(folder / 'component' / 'component-transport.json')
    manifest_path = folder / 'component' / 'component-handoff.json'
    require(transport.get('schema') == 'polaris-apple-handoff-transport-v1' and
            transport['context'] == expected_context and file_pin(manifest_path) == transport['manifest'] and
            transport['fingerprint'] == receipt_digest({k: v for k, v in transport.items() if k != 'fingerprint'}),
            'aggregate component run differs')
    manifest = read_receipt(manifest_path)
    scoped = validate_scope_record(manifest, 'component-handoff', scope)
    require(manifest['App'] == expected_producer and manifest['sourceContract'] == expected_source_contract and
            manifest['context'] == expected_context and manifest['producerJob'] == 'component' and
            manifest['fingerprint'] == receipt_digest({k: v for k, v in manifest.items() if k != 'fingerprint'}),
            'aggregate original component/source differs')
    if 'componentReuse' in manifest:
        component_reuse_record(manifest)
    for slot in slots:
        row = read_receipt(folder / slot / ('carrier-' + slot + '.json'))
        require(validate_scope_record(row, 'carrier-handoff', scope) == scoped,
                'aggregate mixed legacy/scoped wrappers')
        require(row['slot'] == slot and
                row['context'] == expected_context and row['componentTransport'] == transport and
                row.get('devicePassed') is False and row.get('signatureTrustValidated') is False and
                row.get('publicationEligible') is False and
                row['fingerprint'] == receipt_digest({k: v for k, v in row.items() if k != 'fingerprint'}),
                'aggregate slot/context/component/scope differs')
        require(file_pin(folder / slot / 'summary-app-carrier.json') == row['receiptPin'] and
                read_receipt(folder / slot / 'summary-app-carrier.json') == row['receipt'],
                'aggregate original carrier receipt bytes differ')
        receipt = row['receipt']
        rust_path = folder / slot / 'summary-rust-receipt.json'
        require(file_pin(rust_path) == {k: receipt['rustInput'][k] for k in ('bytes', 'sha256')},
                'aggregate original Rust receipt bytes differ')
        rust = read_receipt(rust_path)
        require(rust['schema'] == 'polaris-formal-apple-rust-v1' and rust['App'] == current_app and
                rust['fingerprint'] == receipt_digest({k: v for k, v in rust.items() if k != 'fingerprint'}) and
                rust['target'] == {'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim'}[slot] and
                rust['linkInputSha256'] == receipt['linkInputSha256'] and rust['devicePassed'] is False,
                'aggregate Rust target/App/link identity differs')
        if compiler_identity is None:
            compiler_identity = {'tools': rust['tools'], 'rustc': rust['rustc']}
        require(compiler_identity == {'tools': rust['tools'], 'rustc': rust['rustc']},
                'aggregate actual Rust tool bytes/version differ')
        require(receipt.get('schema') == 'polaris-mobile-apple-carrier-v1' and
                receipt['fingerprint'] == receipt_digest({k: v for k, v in receipt.items() if k != 'fingerprint'}) and
                receipt['App'] == current_app and
                receipt['producerCandidate'] == manifest.get('componentReuse', {}).get('originalProducer', expected_producer)['candidate'] and
                receipt['componentFingerprint'] == manifest['componentFingerprint'] and
                receipt['selectedTarget'] == {'device-arm64': 'ios-arm64', 'sim-arm64': 'ios-arm64-simulator'}[slot] and
                receipt.get('AppCarrierValidated') is True and receipt.get('AppAccepted') is False and
                receipt.get('devicePassed') is False and receipt.get('signatureTrustValidated') is False,
                'aggregate actual carrier identity/scope differs')
        rows[slot] = row
    require({p.name for p in folder.iterdir()} == {'component', *slots}, 'aggregate contains extra slot')
    facts = {'schema': 'polaris-apple-unsigned-chain-v3' if scoped else 'polaris-apple-unsigned-chain-v2', 'context': expected_context, 'App': expected_producer,
             'componentTransport': transport, 'carriers': rows, 'completeSlots': list(slots),
             'devicePassed': False, 'signatureTrustValidated': False, 'publicationEligible': False}
    if scoped:
        facts.update(scope_fields(scope), validatedSlots=list(slots),
                     notRequestedSlots=[slot for slot in SLOTS if slot not in slots])
    facts['fingerprint'] = receipt_digest(facts)
    return facts


def aggregate(folder, output, *, scope):
    require(isinstance(scope, str) and scope in SCOPES, 'explicit aggregate scope required')
    from consume import app_identity
    facts = validate_aggregate(folder, scope=scope, expected_context=context(),
                               expected_producer=producer_code(), expected_app=app_identity(),
                               expected_source_contract=load())
    write_json(output, facts)
