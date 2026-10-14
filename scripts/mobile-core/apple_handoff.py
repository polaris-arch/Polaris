"""Bounded same-run Apple handoff. Original receipts and paths are never rewritten.

Separate hosted filesystems must expose the same canonical private root. A host
with a different layout fails admission; this is not arbitrary-path relocation.
"""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import shutil
import zipfile

from artifacts import file_pin, read_receipt, receipt_digest, tree_inventory
from fork_input import ROOT, decode, load, require
from producer import lease, private_dir, producer_code, run, write_json

SLOTS = ('device-arm64', 'sim-arm64', 'sim-amd64')
MAX_FILES, MAX_BYTES = 65536, 4 * 1024 ** 3


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
            '\\' not in name and all(p not in ('', '.', '..') for p in name.split('/')),
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


def validate_manifest(manifest, root, checkout, expected=None):
    require(manifest.get('schema') == 'polaris-apple-component-handoff-v1' and
            manifest.get('fingerprint') == receipt_digest({k: v for k, v in manifest.items() if k != 'fingerprint'}),
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
    for snapshot in manifest.get('localReplacementSources', []):
        require(tree_inventory(Path(snapshot['directory'])) == snapshot['inventory'],
                'retained local replacement inventory changed')
    return manifest


def freeze(root):
    root = Path(root).resolve(strict=True)
    tools = read_receipt(root / 'tools.json')
    component = root / 'component'
    receipt = read_receipt(component / 'component-receipt.json')
    require(receipt['input']['producer'] == producer_code() and receipt['input']['sourceContract'] == load(),
            'original component producer differs')
    from consume import verify_original, apple_resource_inputs, apple_resources
    with lease(private_dir(root / 'handoff-work'), Path(tools['paths']['cache']), load()):
        verify_original(component / 'Libbox.xcframework', component / 'component-receipt.json', 'apple', {},
                        private_dir(root / 'handoff-work/check'))
        resources = apple_resources()
        paths = {root / 'tools.json', component / 'component-receipt.json', component / 'original-bind.log',
                 Path(tools['paths']['gomobile']), Path(tools['paths']['gobind'])}
        native = receipt['nativeInputs']
        paths.update(Path(p) for p in native['files'])
        snapshots = replacement_snapshots(receipt, component)
        from fork_input import verify_source
        for source_view in {row['sourceRoot'] for row in snapshots}:
            require(verify_source(Path(source_view), load(), generator_outputs=True) ==
                    receipt['input']['sourceObservation'], 'replacement source view differs from original producer')
        for directory in [component / 'Libbox.xcframework', Path(native['generatorOutputRoot']),
                          *[Path(row['directory']) for row in snapshots]]:
            paths.add(directory)
            paths.update(directory.rglob('*'))
        for row in snapshots:
            paths.update(Path(row['sourceRoot']) / name for name in ('go.mod', 'go.sum'))
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
        facts = {'schema': 'polaris-apple-component-handoff-v1', 'context': context(),
                 'producerJob': os.environ['GITHUB_JOB'], 'root': str(root), 'checkout': str(ROOT),
                 'App': producer_code(), 'sourceContract': load(), 'files': rows,
                 'componentReceipt': file_pin(component / 'component-receipt.json'),
                 'componentFingerprint': receipt['fingerprint'], 'localReplacementSources': snapshots,
                 'tools': tools['tools'], 'resources': resources, 'devicePassed': False,
                 'signatureTrustValidated': False, 'publicationEligible': False}
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
    require(transport['schema'] == 'polaris-apple-handoff-transport-v1' and transport['context'] == context() and
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
        require(facts['context'] == context() and facts['root'] == str(root.resolve()) and
                facts['checkout'] == str(ROOT.resolve()), 'host layout/context differs before extraction')
        expected = {namespace + '/' + name for namespace, rows in facts['files'].items() for name in rows}
        require({i.filename for i in infos} == expected | {'component-handoff.json'}, 'extra/missing handoff members')
        restore_payload(archive, facts['files'], root, ROOT)
        (root / 'component-handoff.json').write_bytes(raw)
        write_json(root / 'component-transport.json', transport)
    validate_manifest(facts, root, ROOT)
    with lease(private_dir(root / 'import-work'), private_dir(root / 'cache'), load()):
        verify_import(root / 'component', root / 'cache', *(read_receipt(root / 'tools.json')['paths'][n]
                      for n in ('source', 'go', 'gomobile', 'gobind', 'developer')))
    write_json(root / 'import-observation.json', {'schema': 'polaris-apple-import-observation-v1',
               'context': context(), 'transport': transport, 'completed': True, 'devicePassed': False})
    return read_receipt(root / 'tools.json')


def verify_import(component, cache, source, go, gomobile, gobind, developer):
    root = Path(component).parent
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
            receipt['input']['producer'] == facts['App'] and receipt['input']['sourceContract'] == value and
            receipt['input']['tools']['executables'] == facts['tools'],
            'original producer receipt differs')
    # Same source/tool result can be handed off, but this host must supply the
    # same actual generator, SDK settings and compiler bytes, not merely a label.
    from producer import tool_facts
    from types import SimpleNamespace
    env = dict(os.environ, DEVELOPER_DIR=str(developer))
    _, observed_tools = tool_facts(SimpleNamespace(platform='apple', go=go, gomobile=gomobile,
                                                   gobind=gobind), value, env)
    require(observed_tools == receipt['input']['tools'], 'actual consumer tool/SDK bytes differ')
    verify_replacements(facts['localReplacementSources'], receipt, Path(component))
    from consume import verify_original
    verify_original(Path(component) / 'Libbox.xcframework', Path(component) / 'component-receipt.json',
                    'apple', {}, private_dir(root / 'import-check'))
    return receipt


def carrier_summary(root, slot):
    require(slot in SLOTS, 'unknown Apple slot')
    root = Path(root)
    receipt_path = root / slot / 'app-carrier.json'
    receipt = read_receipt(receipt_path)
    require(receipt['App']['candidate'] == context()['candidate'] and receipt['devicePassed'] is False,
            'carrier candidate/device scope differs')
    row = {'schema': 'polaris-apple-carrier-handoff-v1', 'context': context(), 'slot': slot,
           'componentTransport': read_receipt(root / 'component-transport.json'),
           'receipt': receipt, 'receiptPin': file_pin(receipt_path),
           'devicePassed': False, 'signatureTrustValidated': False, 'publicationEligible': False}
    row['fingerprint'] = receipt_digest(row)
    write_json(root / ('carrier-' + slot + '.json'), row)
    # Preserve actual pretty receipt bytes for aggregate SHA comparison.
    shutil.copyfile(receipt_path, root / 'summary-app-carrier.json')
    rust_path = Path(receipt['rustInput']['receipt'])
    require(rust_path.is_relative_to(root / slot) and file_pin(rust_path) ==
            {k: receipt['rustInput'][k] for k in ('bytes', 'sha256')}, 'original Rust receipt changed')
    shutil.copyfile(rust_path, root / 'summary-rust-receipt.json')


def aggregate(folder, output):
    folder = Path(folder)
    rows = {}
    from consume import app_identity
    current_app = app_identity()
    compiler_identity = None
    transport = read_receipt(folder / 'component' / 'component-transport.json')
    manifest_path = folder / 'component' / 'component-handoff.json'
    require(transport['context'] == context() and file_pin(manifest_path) == transport['manifest'] and
            transport['fingerprint'] == receipt_digest({k: v for k, v in transport.items() if k != 'fingerprint'}),
            'aggregate component run differs')
    manifest = read_receipt(manifest_path)
    require(manifest['App'] == producer_code() and manifest['sourceContract'] == load() and
            manifest['context'] == context() and manifest['producerJob'] == 'component' and
            manifest['fingerprint'] == receipt_digest({k: v for k, v in manifest.items() if k != 'fingerprint'}),
            'aggregate original component/source differs')
    for slot in SLOTS:
        row = read_receipt(folder / slot / ('carrier-' + slot + '.json'))
        require(row['schema'] == 'polaris-apple-carrier-handoff-v1' and row['slot'] == slot and
                row['context'] == context() and row['componentTransport'] == transport and
                row['fingerprint'] == receipt_digest({k: v for k, v in row.items() if k != 'fingerprint'}),
                'aggregate slot/context/component differs')
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
                rust['target'] == {'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim',
                                   'sim-amd64': 'x86_64-apple-ios'}[slot] and
                rust['linkInputSha256'] == receipt['linkInputSha256'] and rust['devicePassed'] is False,
                'aggregate Rust target/App/link identity differs')
        if compiler_identity is None:
            compiler_identity = {'tools': rust['tools'], 'rustc': rust['rustc']}
        require(compiler_identity == {'tools': rust['tools'], 'rustc': rust['rustc']},
                'aggregate actual Rust tool bytes/version differ')
        require(receipt['fingerprint'] == receipt_digest({k: v for k, v in receipt.items() if k != 'fingerprint'}) and
                receipt['App'] == current_app and
                receipt['producerCandidate'] == context()['candidate'] and
                receipt['componentFingerprint'] == manifest['componentFingerprint'] and
                receipt['selectedTarget'] == {'device-arm64': 'ios-arm64', 'sim-arm64': 'ios-arm64-simulator',
                                               'sim-amd64': 'ios-x86_64-simulator'}[slot] and
                receipt.get('AppCarrierValidated') is True and receipt.get('AppAccepted') is False and
                receipt.get('devicePassed') is False and receipt.get('signatureTrustValidated') is False,
                'aggregate actual carrier identity/scope differs')
        rows[slot] = row
    require({p.name for p in folder.iterdir()} == {'component', *SLOTS}, 'aggregate contains extra slot')
    facts = {'schema': 'polaris-apple-unsigned-chain-v2', 'context': context(), 'App': producer_code(),
             'componentTransport': transport, 'carriers': rows, 'completeSlots': list(SLOTS),
             'devicePassed': False, 'signatureTrustValidated': False, 'publicationEligible': False}
    facts['fingerprint'] = receipt_digest(facts)
    write_json(output, facts)
