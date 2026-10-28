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
from producer import PRODUCER_FILES, private_dir, run, slot_locks, write_json

STAGE = Path('target/apple-ci-cache')
TARGETS = {'device-arm64': 'aarch64-apple-ios', 'sim-arm64': 'aarch64-apple-ios-sim'}
GROUPS = ('deps', 'build', '.fingerprint')
REGISTRY_DIRS = ('registry/index', 'registry/cache', 'git/db')
MAX_BYTES, MAX_ENTRIES = 4 * 1024 ** 3, 131072
REGISTRY_MAX_BYTES = 2 * 1024 ** 3
HEADROOM = 1024 ** 3
SAMPLE_LIMIT, SAMPLE_PATH_BYTES = 64, 1024
# Actual Cargo.toml package=polaris and lib=polaris_lib. Keep all their compiled
# outputs in the original target, including the hashed static archive in deps.
APP_ENTRY = re.compile(r'^(?:lib)?polaris(?:_lib)?(?:-[0-9a-f]{16}(?:\.|$)|\.)')
APPLE_RUST_BUILD_CODE_POLICY = 'polaris-apple-rust-build-code-v1'
APPLE_RUST_UNRELATED_CODE = frozenset({
    'scripts/mobile-core/android_sdk.py',
    'scripts/libbox-patches/prepare-ndk-linux.py',
    'scripts/libbox-patches/ndk-linux-archive.json',
})


def apple_rust_build_code(identity, policy=APPLE_RUST_BUILD_CODE_POLICY):
    """Reviewed lookup projection only; never replace full producer provenance."""
    require(policy == APPLE_RUST_BUILD_CODE_POLICY, 'unknown Apple Rust build code policy')
    files = identity.get('files') if isinstance(identity, dict) else None
    require(isinstance(files, dict) and set(PRODUCER_FILES) <= set(files),
            'complete original producer code inventory required')
    for name, pin in files.items():
        require(isinstance(name, str) and isinstance(pin, dict) and set(pin) == {'bytes', 'sha256'} and
                type(pin['bytes']) is int and pin['bytes'] > 0 and isinstance(pin['sha256'], str) and
                re.fullmatch('[0-9a-f]{64}', pin['sha256']), 'producer code file pin differs')
    # android_sdk has imports/function definitions only; its calls and the NDK
    # installer/archive belong to the Android branch. ci_android.prepare_source
    # and android-source's native parsers ARE used by Apple and stay relevant.
    # Unknown future producer entries stay relevant by default.
    return {'policy': policy, 'files': {name: dict(pin) for name, pin in files.items()
                                      if name not in APPLE_RUST_UNRELATED_CODE}}


def cargo_source_config(name):
    # Keep whole manifests/configs, including unknown future source directives.
    # This deliberately also invalidates on unrelated manifest/config edits.
    return (Path(name).name in ('Cargo.toml', 'Cargo.lock') or
            name.endswith(('/.cargo/config', '/.cargo/config.toml')) or
            name in ('.cargo/config', '.cargo/config.toml'))


def registry_source_overrides(environment):
    # Registry credentials are transport authority, never identity inputs.
    return {key: value for key, value in environment.items() if
            key in ('CARGO_REGISTRY_DEFAULT', 'CARGO_REGISTRY_INDEX', 'CARGO_REGISTRY_PROTOCOL') or
            (key.startswith('CARGO_REGISTRIES_') and key.endswith(('_INDEX', '_PROTOCOL')))}


def cache_identities(tools, rustc, cargo, tool_pins, config, environment):
    require('Cargo.lock' in config and 'src-tauri/Cargo.toml' in config, 'cache lock/config inputs missing')
    sources = registry_source_overrides(environment)
    overrides = {key: value for key, value in environment.items() if key.startswith((
        'CARGO_PROFILE_', 'CARGO_TARGET_', 'RUSTFLAGS', 'RUSTDOCFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
        'CFLAGS', 'CXXFLAGS', 'OBJC_INCLUDE_PATH')) and key != 'CARGO_TARGET_DIR'}
    dependency = sha(canonical({'policy': 'polaris-apple-rust-dependencies-v2',
        'rustc': rustc, 'cargo': cargo, 'tools': tool_pins, 'xcode': tools['xcode'],
        'producerCode': apple_rust_build_code(tools['App']), 'config': config,
        'profile': 'release', 'features': ['tauri/custom-protocol'], 'crateType': 'staticlib',
        'deploymentTarget': '17.0', 'overrides': overrides, 'sourceOverrides': sources}))
    # Downloads have no compiler output. Retain exact Cargo format/tool bytes,
    # lock/manifests and source configuration; omit App scope, producer code,
    # workflow, Xcode, Rust/native flags and compiled-dependency transport policy.
    registry = sha(canonical({'policy': 'polaris-apple-registry-downloads-v2',
        'cargo': cargo, 'cargoTool': tool_pins['cargo'],
        'config': {name: pin for name, pin in config.items() if cargo_source_config(name)},
        'sourceOverrides': sources}))
    return dependency, registry


def registry_directory(path):
    """True absence is a cold cache, not a redirected or unowned directory."""
    home = Path.home()
    require(home.is_absolute() and home.resolve(strict=True) == home,
            'registry home ancestry redirected')
    info = home.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid(),
            'registry home ownership/type differs')
    parts = Path(path).relative_to(home).parts
    require(parts and parts[0] == '.cargo' and '..' not in parts,
            'registry directory outside fixed Cargo home')
    current = home
    for part in parts:
        current = current / part
        try:
            info = current.lstat()
        except FileNotFoundError:
            # Do not create a cache or resolve a missing leaf. Restore or the
            # actual Cargo invocation may populate this fixed path afterwards.
            return False
        require(not stat.S_ISLNK(info.st_mode), 'registry cache directory symlink refused')
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid(),
                'registry cache directory ownership/type differs')
    return True


def cargo_home():
    home = Path.home() / '.cargo'
    require(Path(os.environ.get('CARGO_HOME', str(home))) == home,
            'registry cache path differs from actual Cargo home')
    if registry_directory(home):
        for name in ('config', 'config.toml'):
            try:
                (home / name).lstat()
            except FileNotFoundError:
                continue
            # Only tracked source configurations enter cache_identities. Never
            # read, hash or silently admit external config (possibly secrets).
            require(False, 'unbound Cargo home configuration refused')
    return home


class ExternalCacheAlias(RuntimeError):
    """A dependency has aliases outside the selected entry; leave it uncached."""
    def __init__(self, entries):
        super().__init__('cache alias leaves selected dependency entry')
        self.entries = entries


class UncacheableSymlink(RuntimeError):
    """Local dependency entry contains a link; leave the whole entry native."""
    def __init__(self, entries):
        super().__init__('cache symlink entry left uncached')
        self.entries = entries


def checked_tree(path, *, maximum=MAX_BYTES, local_export=False):
    """Bounded owned regular bytes; hardlink aliases must all stay in this tree."""
    path = Path(path)
    # Only local export may classify a candidate link without resolving it.
    # Restored payloads and registry trees retain the strict ancestry contract.
    ancestry = path.parent if local_export else path
    require(ancestry.resolve(strict=True) == ancestry, 'cache ancestry cannot be redirected')
    if local_export:
        parent = ancestry.lstat()
        require(stat.S_ISDIR(parent.st_mode) and parent.st_uid == os.getuid(),
                'cache parent ownership/type differs')
    size, entries, aliases = 0, 0, {}
    symlink_entry = False
    pending = [path]
    while pending:
        current = pending.pop()
        info = current.lstat()
        entries += 1
        require(entries <= MAX_ENTRIES and info.st_uid == os.getuid(), 'cache ownership/count differs')
        if stat.S_ISLNK(info.st_mode):
            require(local_export, 'cache symlink refused')
            symlink_entry = True
            # Never inspect the link target. Finish checking all non-link siblings
            # so ownership, special files and scan budgets cannot become skips.
            continue
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
    if symlink_entry:
        raise UncacheableSymlink(entries)
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
        tool_pins = {name: file_pin(run(['rustup', 'which', name]).strip()) for name in ('cargo', 'rustc')}
        # Keep the full workflow in compiled identity: arbitrary future shell
        # steps can affect Cargo after prepare. There is no safe YAML omission
        # policy here. Android-only file edits still avoid unrelated cold misses.
        identity, registry_identity = cache_identities(
            tools, rustc, run(['cargo', '-V']), tool_pins, config, os.environ)
        home = cargo_home()
        # Restore runs next. Reject unsafe existing roots before the cache
        # action can touch them; legitimate absent branches stay absent.
        for name in REGISTRY_DIRS:
            registry_directory(home / name)
        write_json(stage / 'owner.json', {'context': context(), 'scope': scope_fields(),
            'actualCargoTargetDir': str(root / 'cache/rust-target'), 'identity': identity,
            'registryIdentity': registry_identity})
        with Path(os.environ['GITHUB_OUTPUT']).open('a') as output:
            output.write('identity=' + identity + '\nregistry_identity=' + registry_identity + '\n')
        note(root, 'prepare', identity=identity, registryIdentity=registry_identity, freeBytes=free,
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


def cache_sample(samples, relative):
    """Bound diagnostics by count and UTF-8 bytes; never include link targets."""
    if len(samples) >= SAMPLE_LIMIT:
        return True
    raw = str(relative).encode('utf-8', errors='replace')
    samples.append(raw[:SAMPLE_PATH_BYTES].decode('utf-8', errors='ignore'))
    return len(raw) > SAMPLE_PATH_BYTES


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
        skipped_symlinks, skipped_symlink_count = [], 0
        alias_truncated, symlink_truncated = False, False
        for role in roles:
            base = native_base(root, slot, role)
            # Admit the fixed role ancestry even when no GROUP/candidate exists.
            # A legitimate missing native directory stays missing; no mkdir here.
            require(not base.is_symlink() and base.resolve(strict=False) == base,
                    'native dependency base/ancestry redirected')
            for group in GROUPS:
                folder = base / group
                require(not folder.is_symlink(), 'native dependency directory redirected')
                if not folder.exists():
                    continue
                require(folder.is_dir(), 'native dependency directory redirected')
                for source in bounded_children(folder):
                    examined += 1
                    require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound')
                    if APP_ENTRY.match(source.name):
                        continue
                    try:
                        amount, entries = checked_tree(source, local_export=True)
                    except UncacheableSymlink as link:
                        examined += link.entries - 1
                        require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound')
                        skipped_symlink_count += 1
                        symlink_truncated = cache_sample(skipped_symlinks,
                            source.relative_to(root / 'cache/rust-target')) or symlink_truncated
                        continue
                    except ExternalCacheAlias as alias:
                        examined += alias.entries - 1
                        require(examined <= MAX_ENTRIES, 'examined cache entries exceed bound')
                        skipped_alias_count += 1
                        alias_truncated = cache_sample(skipped_aliases,
                            source.relative_to(root / 'cache/rust-target')) or alias_truncated
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
             uncachedExternalAliasSampleLimit=SAMPLE_LIMIT, uncachedExternalAliasTruncated=alias_truncated,
             uncachedSymlinkEntries=skipped_symlinks, uncachedSymlinkEntryCount=skipped_symlink_count,
             uncachedSymlinkSampleLimit=SAMPLE_LIMIT, uncachedSymlinkTruncated=symlink_truncated,
             examinedEntries=examined,
             archiveHeadroomBytes=size * 9 // 8 + count * 4096,
             scopeNote='same-filesystem dependency moves after original and summary upload; no App products')


def registry_ready():
    root = location()
    home = cargo_home()
    size, count = 0, 0
    for name in REGISTRY_DIRS:
        folder = home / name
        if registry_directory(folder):
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
