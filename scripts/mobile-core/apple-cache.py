#!/usr/bin/env python3
"""Bounded dependency-only cache transport; never an Apple acceptance receipt.

Actions cache archives preserve their stored paths and hash the requested paths.
The native target lives under a run/attempt-specific private root, so transport
uses a stable ignored path and same-filesystem renames, never a second snapshot.
"""
import argparse
import json
import os
from pathlib import Path
import re
import stat
import sys

sys.dont_write_bytecode = True
from artifacts import file_pin, read_receipt
from apple_handoff import context, required_slots, scope_fields, transport_pin
from ci_apple import location
from fork_input import ROOT, canonical, require, sha
from producer import private_dir, run, slot_locks, write_json

STAGE = Path('target/apple-ci-cache')
TARGETS = {'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim'}
GROUPS = ('deps', 'build', '.fingerprint')
MAX_BYTES, MAX_ENTRIES = 4 * 1024 ** 3, 131072
REGISTRY_MAX_BYTES = 2 * 1024 ** 3
HEADROOM = 1024 ** 3
# Actual Cargo.toml package=polaris and lib=polaris_lib. Keep all their compiled
# outputs in the original target, including the hashed static archive in deps.
APP_ENTRY = re.compile(r'^(?:lib)?polaris(?:_lib)?(?:-[0-9a-f]{16}(?:\.|$)|\.)')


class ExternalCacheAlias(RuntimeError):
    """A dependency has aliases outside the selected entry; leave it uncached."""
    def __init__(self, entries):
        super().__init__('cache alias leaves selected dependency entry')
        self.entries = entries


def checked_tree(path, *, maximum=MAX_BYTES):
    """Bounded owned regular bytes; hardlink aliases must all stay in this tree."""
    path = Path(path)
    require(path.resolve(strict=True) == path, 'cache ancestry cannot be redirected')
    size, entries, aliases = 0, 0, {}
    pending = [path]
    while pending:
        current = pending.pop()
        info = current.lstat()
        entries += 1
        require(entries <= MAX_ENTRIES and info.st_uid == os.getuid(), 'cache ownership/count differs')
        require(not stat.S_ISLNK(info.st_mode), 'cache symlink refused')
        if stat.S_ISDIR(info.st_mode):
            with os.scandir(current) as children:
                for child in children:
                    pending.append(Path(child.path))
                    require(entries + len(pending) <= MAX_ENTRIES, 'cache entry count exceeds bound')
        else:
            require(stat.S_ISREG(info.st_mode), 'cache regular file required')
            key = (info.st_dev, info.st_ino)
            seen, links = aliases.get(key, (0, info.st_nlink))
            require(links == info.st_nlink, 'cache inode links changed during scan')
            aliases[key] = (seen + 1, links)
            size += info.st_size
            require(size <= maximum, 'cache bytes exceed transport bound')
    if any(seen != links for seen, links in aliases.values()):
        raise ExternalCacheAlias(entries)
    return size, entries


def check_space(path, amount):
    space = os.statvfs(path)
    free = space.f_bavail * space.f_frsize
    require(free >= amount + HEADROOM, 'cache archive/extraction headroom unavailable; no snapshot made')
    return free


def note(root, action, slot=None, **facts):
    row = {'schema': 'polaris-apple-dependency-cache-observation-v1', 'action': action,
           'slot': slot, **scope_fields(), 'actualCargoTargetDir': str(root / 'cache/rust-target'),
           'stableCachePath': str(STAGE / slot) if slot else str(STAGE),
           'carrierAccepted': False, 'devicePassed': False,
           'spaceCheckIsReservation': False, **facts}
    print('APPLE_DEPENDENCY_CACHE=' + json.dumps(row, sort_keys=True), flush=True)
    write_json(root / ('apple-cache-' + action + ('-' + slot if slot else '') + '.json'), row)


def state(root, stage):
    row = read_receipt(stage / 'owner.json')
    require(row['context'] == context() and row['scope'] == scope_fields(), 'cache transport owner/scope differs')
    require(row['actualCargoTargetDir'] == str(root / 'cache/rust-target'), 'cache target path differs')
    require(stage.stat().st_dev == (root / 'cache').stat().st_dev, 'cache transport requires same filesystem')
    return row


def prepare():
    root = location()
    stage = private_dir(ROOT / STAGE)
    cache = private_dir(root / 'cache')
    require(run(['git', 'check-ignore', '--', STAGE.as_posix()], cwd=ROOT).strip() == STAGE.as_posix(),
            'cache transport must be ignored by the frozen checkout')
    with slot_locks(stage, cache):
        require(set(p.name for p in stage.iterdir()) <= {'.mobile-producer.lock'}, 'prior cache transport retained')
        require(stage.stat().st_dev == cache.stat().st_dev == Path(os.environ['RUNNER_TEMP']).stat().st_dev,
                'cache transport/archive temporary files require the admitted filesystem')
        # Restore peaks include both downloaded archive and extracted bytes.
        free = check_space(stage, (MAX_BYTES + REGISTRY_MAX_BYTES) * 9 // 4)
        tools = read_receipt(root / 'tools.json')
        rustc = run(['rustc', '-vV'])
        require(rustc == tools['rustc'], 'cache actual Rust toolchain differs from admission')
        tracked = run(['git', 'ls-files', '-z'], cwd=ROOT).split('\0')
        config = {name: file_pin(ROOT / name) for name in tracked if name and (
            Path(name).name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'rust-toolchain.toml') or
            name.endswith(('/.cargo/config', '/.cargo/config.toml')) or
            name in ('.cargo/config', '.cargo/config.toml', '.github/workflows/apple.yml',
                     'src-tauri/tauri.conf.json', 'src-tauri/tauri.ios.conf.json'))}
        require('Cargo.lock' in config and 'src-tauri/Cargo.toml' in config, 'cache lock/config inputs missing')
        tool_pins = {name: file_pin(run(['rustup', 'which', name]).strip()) for name in ('cargo', 'rustc')}
        overrides = {k: v for k, v in os.environ.items() if k.startswith((
            'CARGO_PROFILE_', 'CARGO_TARGET_', 'RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
            'CFLAGS', 'CXXFLAGS', 'OBJC_INCLUDE_PATH')) and k != 'CARGO_TARGET_DIR'}
        identity = sha(canonical({'rustc': rustc, 'cargo': run(['cargo', '-V']), 'tools': tool_pins,
            'xcode': tools['xcode'], 'producerFiles': tools['App']['files'], 'config': config,
            'profile': 'release', 'features': ['tauri/custom-protocol'], 'crateType': 'staticlib',
            'deploymentTarget': '17.0', 'overrides': overrides}))
        home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo'))).resolve(strict=True)
        require(home == Path.home() / '.cargo', 'registry cache path differs from actual Cargo home')
        write_json(stage / 'owner.json', {'context': context(), 'scope': scope_fields(),
            'actualCargoTargetDir': str(root / 'cache/rust-target'), 'identity': identity})
        with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
            output.write('identity=' + identity + '\n')
        note(root, 'prepare', identity=identity, freeBytes=free,
             registryPaths=['~/.cargo/registry/index', '~/.cargo/registry/cache', '~/.cargo/git/db'])


def allowed_roles(slot):
    require(slot in required_slots(), 'unrequested cache carrier slot')
    return ('host', 'target') if slot == 'device-arm64' else ('target',)


def owned_dir(path):
    path = Path(path)
    require(not path.is_symlink() and path.resolve(strict=False) == path, 'cache directory redirected')
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid(), 'cache directory ownership differs')
    return path


def native_base(root, slot, role):
    target = root / 'cache/rust-target'
    return (target if role == 'host' else target / TARGETS[slot]) / 'release'


def transfer(entries):
    """Rename the already validated, Apple-specific plan, rolling back failures."""
    moved = []
    try:
        for source, destination in entries:
            require(not destination.exists() and not destination.is_symlink(), 'cache destination already exists')
            require(source.stat().st_dev == destination.parent.stat().st_dev, 'cache copy fallback forbidden')
        for source, destination in entries:
            require(not destination.exists() and not destination.is_symlink(), 'cache destination became occupied')
            os.rename(source, destination)
            moved.append((source, destination))
    except BaseException as error:
        failures = []
        for source, destination in reversed(moved):
            try:
                require(not source.exists() and not source.is_symlink(), 'cache rollback source occupied')
                os.rename(destination, source)
            except BaseException as rollback:
                failures.append(str(rollback))
        if failures:
            raise RuntimeError('cache rename failed; originals retained at reported source/stage paths; rollback: ' +
                               '; '.join(failures)) from error
        raise


def manifest(owner, slot):
    return {'schema': 'polaris-apple-dependency-cache-v1', 'slot': slot,
            'scope': owner['scope'], 'identity': owner['identity'], 'profile': 'release'}


def import_cache(slot, restored):
    root = location()
    stage = private_dir(ROOT / STAGE)
    with slot_locks(stage, private_dir(root / 'cache')):
        owner = state(root, stage)
        roles = allowed_roles(slot)
        payload = stage / slot
        if not payload.exists():
            require(restored == 'miss', 'reported cache hit/export has no dependency payload')
            note(root, 'import', slot, cacheState='miss', restoredBytes=0)
            return
        require(restored in ('hit', 'staged'), 'unexpected payload on a cache miss')
        size, _ = checked_tree(payload)
        require(read_receipt(payload / 'cache.json') == manifest(owner, slot), 'cache scope/tool/profile identity differs')
        require(set(p.name for p in payload.iterdir()) <= set(roles) | {'cache.json'}, 'unexpected cache payload role')
        plan = []
        for role in roles:
            folder = payload / role
            if not folder.exists():
                continue
            require(folder.is_dir() and set(p.name for p in folder.iterdir()) <= set(GROUPS), 'unexpected cache dependency group')
            for group in sorted(folder.iterdir()):
                require(group.is_dir(), 'cache dependency group must be directory')
                dest = owned_dir(native_base(root, slot, role) / group.name)
                for source in sorted(group.iterdir()):
                    require(not APP_ENTRY.match(source.name), 'App crate artifacts are not reusable dependency cache')
                    plan.append((source, dest / source.name))
        transfer(plan)
        # Remove only empty transport directories and its non-acceptance marker.
        (payload / 'cache.json').unlink()
        for role in roles:
            folder = payload / role
            if folder.exists():
                for group in folder.iterdir():
                    group.rmdir()
                folder.rmdir()
        payload.rmdir()
        note(root, 'return' if restored == 'staged' else 'import', slot,
             cacheState='local-staged-return' if restored == 'staged' else 'remote-hit',
             restoredBytes=size, movedEntries=len(plan))


def bounded_children(folder):
    children = []
    with os.scandir(folder) as entries:
        for entry in entries:
            require(len(children) < MAX_ENTRIES, 'cache directory entry count exceeds bound')
            children.append(Path(entry.path))
    return sorted(children)


def export_cache(slot):
    root = location()
    stage = private_dir(ROOT / STAGE)
    with slot_locks(stage, private_dir(root / 'cache')):
        owner = state(root, stage)
        roles = allowed_roles(slot)
        pin = read_receipt(root / ('evidence-' + slot + '-pin.json'))
        require(pin['slot'] == slot and pin['context'] == context() and not pin['receiptReadErrors'] and
                pin['archive'] == transport_pin(root / ('apple-original-' + slot + '-evidence.zip')),
                'cache export requires intact preserved carrier originals')
        require((root / 'summaries' / slot / ('carrier-' + slot + '.json')).is_file(),
                'cache export requires current carrier summary')
        payload = stage / slot
        require(not payload.exists() and not payload.is_symlink(), 'prior dependency payload retained')
        sources, size, count, skipped_aliases = [], 0, 0, []
        examined, skipped_alias_count = 0, 0
        for role in roles:
            for group in GROUPS:
                folder = native_base(root, slot, role) / group
                if not folder.exists():
                    continue
                require(not folder.is_symlink() and folder.is_dir(), 'native dependency directory redirected')
                for source in bounded_children(folder):
                    examined += 1
                    require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound')
                    if APP_ENTRY.match(source.name):
                        continue
                    try:
                        amount, entries = checked_tree(source)
                    except ExternalCacheAlias as alias:
                        examined += alias.entries - 1
                        require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound')
                        skipped_alias_count += 1
                        if len(skipped_aliases) < 64:
                            skipped_aliases.append(str(source.relative_to(root / 'cache/rust-target')))
                        continue
                    examined += entries - 1
                    require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound')
                    size += amount; count += entries
                    require(size <= MAX_BYTES and count <= MAX_ENTRIES, 'combined dependency cache exceeds bound')
                    sources.append((source, role, group))
        require(sources, 'no dependency entries available for cache export')
        free = check_space(stage, size * 9 // 8 + count * 4096)
        payload = private_dir(payload)
        plan = [(source, private_dir(payload / role / group) / source.name) for source, role, group in sources]
        # The manifest is cache metadata only. A failed move leaves it for
        # diagnosis and the move operation rolls bytes back to their source.
        write_json(payload / 'cache.json', manifest(owner, slot))
        transfer(plan)
        note(root, 'export', slot, dependencyBytes=size, movedEntries=len(plan), freeBytes=free,
             uncachedExternalAliasEntries=skipped_aliases, uncachedExternalAliasCount=skipped_alias_count,
             uncachedExternalAliasSampleLimit=64, examinedEntries=examined,
             archiveHeadroomBytes=size * 9 // 8 + count * 4096,
             scopeNote='same-filesystem dependency moves after original and summary upload; no App products')


def registry_ready():
    root = location()
    size, count = 0, 0
    for name in ('registry/index', 'registry/cache', 'git/db'):
        folder = Path.home() / '.cargo' / name
        if folder.exists():
            amount, entries = checked_tree(folder, maximum=REGISTRY_MAX_BYTES)
            size += amount; count += entries
    require(size and size <= REGISTRY_MAX_BYTES and count <= MAX_ENTRIES, 'bounded nonempty registry cache required')
    free = check_space(Path(os.environ['RUNNER_TEMP']), size * 9 // 8 + count * 4096)
    note(root, 'registry-save', dependencyBytes=size, freeBytes=free,
         scopeNote='registry downloads only; separate from target dependency cache and carrier acceptance')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('prepare', 'import', 'export', 'registry-ready'))
    parser.add_argument('--slot', choices=tuple(TARGETS))
    parser.add_argument('--restored', choices=('hit', 'miss', 'staged'), default='hit')
    args = parser.parse_args()
    if args.action == 'prepare':
        prepare()
    elif args.action == 'registry-ready':
        registry_ready()
    else:
        require(args.slot is not None, 'exact cache slot required')
        if args.action == 'import':
            import_cache(args.slot, args.restored)
        else:
            export_cache(args.slot)
