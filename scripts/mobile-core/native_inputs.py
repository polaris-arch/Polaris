"""Retain actual compiler/static origins; cache validation rereads original bytes."""
from collections import Counter, defaultdict
from pathlib import Path
import re
import shlex

from fork_input import require
from artifacts import file_pin, module, tree_inventory


def collect(scratch, log, platform, output, cache):
    scratch, cache = Path(scratch).resolve(strict=True), Path(cache).resolve(strict=True)
    text = Path(log).read_text()
    roots = sorted({Path(p).resolve(strict=True) for p in re.findall(r'^WORK=(.+)$', text, re.M)})
    require(roots and len(roots) <= 32 and all(p.is_dir() and p.is_relative_to(scratch) for p in roots),
            'actual compiler WORK roots must remain in the fixed scratch')
    files = {}
    for root in roots:
        for p in root.rglob('*.o'):
            require(not p.is_symlink() and p.resolve().is_relative_to(scratch), 'compiler object escaped scratch')
            files[str(p)] = {'kind': 'compiler-object', **file_pin(p)}
    # The pinned generator prints its merge commands with a $WORK alias.
    # The gomobile workspace is the one containing generated binding sources.
    mobile = [p for p in roots if any((p / target).is_dir() for target in ('ios', 'iossimulator', 'src'))]
    if platform == 'apple':
        require(len(mobile) == 1, 'one actual gomobile workspace required')
        for line in text.splitlines():
            if 'libtool -static -o ' not in line:
                continue
            tokens = shlex.split(line.replace('$WORK', str(mobile[0])).replace('$GOPATH', str(cache / 'go-path'))
                                .replace('$GOMOBILE', str(cache / 'go-path/pkg/gomobile')))
            start = tokens.index('-o') + 3  # output, original Go archive, then external archives
            for token in tokens[start:]:
                p = Path(token).resolve(strict=True)
                require(p.suffix == '.a' and p.is_relative_to(cache), 'external static origin outside verified module cache')
                files[str(p)] = {'kind': 'external-static-archive', **file_pin(p)}
    require(files, 'actual compiler/static input bytes are missing')
    facts = {'schema': 'polaris-mobile-native-inputs-v1', 'scratch': str(scratch), 'cache': str(cache),
             'workRoots': [str(p) for p in roots], 'originalBindLog': file_pin(log), 'files': files}
    if platform == 'apple':
        helper = module('origin_buildinfo', 'scripts/libbox-patches/android-source.py')
        carrier = module('origin_container', 'scripts/apple-carrier.py')
        origins = defaultdict(list)
        for path, pin in files.items():
            if pin['kind'] == 'compiler-object':
                origins[pin['sha256']].append({'kind': pin['kind'], 'path': path, 'sha256': pin['sha256']})
            else:
                parsed = carrier.container(Path(path).read_bytes(), helper)
                slices = parsed['slices'] if parsed['kind'] == 'fat-ar' else [{'ordinal': 0, 'archive': parsed}]
                for row in slices:
                    require(row['archive']['kind'] == 'bsd-ar', 'external runtime must be a static archive')
                    for member in row['archive']['members']:
                        if member['classification'] == 'code':
                            origins[member['sha256']].append({'kind': pin['kind'], 'path': path,
                                'archiveSha256': pin['sha256'], 'sliceOrdinal': row['ordinal'],
                                'memberOrdinal': member['ordinal'], 'sha256': member['sha256']})
        mappings = {}
        for target, row in output['slices'].items():
            used, mappings[target] = Counter(), []
            for member in row['members']:
                if member['goCarrier']:
                    continue
                digest = member['sha256']
                choices = origins[digest]
                used[digest] += 1
                require(choices and used[digest] <= len(choices), 'static member lacks actual compiler/runtime origin')
                mappings[target].append({'memberOrdinal': member['ordinal'], 'sha256': digest, 'origins': choices})
        facts['staticMemberOrigins'] = mappings
    return facts


def verify(facts, scratch=None, log=None):
    require(facts.get('schema') == 'polaris-mobile-native-inputs-v1', 'actual native input receipt required')
    if 'generatorOutputRoot' in facts:
        generated = Path(facts['generatorOutputRoot']).resolve(strict=True)
        require(generated.name == 'build' and generated.parent.name == 'build-view',
                'generator output escaped the fixed build view')
        require(tree_inventory(generated) == facts['generatorOutputInventory'],
                'original generator output bytes differ')
    original_scratch, cache = Path(facts['scratch']).resolve(strict=True), Path(facts['cache']).resolve(strict=True)
    if scratch is not None:
        require(original_scratch == Path(scratch).resolve(strict=True), 'native scratch input changed')
    if log is not None:
        require(file_pin(log) == facts['originalBindLog'], 'original compiler log changed')
    require(0 < len(facts['files']) <= 65536, 'native input inventory bounds')
    for path, pin in facts['files'].items():
        p = Path(path)
        require(not p.is_symlink() and p.resolve(strict=True).is_relative_to(
                original_scratch if pin['kind'] == 'compiler-object' else cache), 'native input escaped owned roots')
        require(file_pin(p) == {key: pin[key] for key in ('bytes', 'sha256')}, 'actual native input bytes changed')
