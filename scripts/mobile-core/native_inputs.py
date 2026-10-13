"""Retain actual compiler/static origins; cache validation rereads original bytes."""
from collections import Counter, defaultdict
from pathlib import Path
import re
import shlex

from fork_input import require
from artifacts import file_pin, module, tree_inventory


def source_table(files):
    require(0 < len(files) <= 65536, 'native source file count budget')
    helper = module('origin_buildinfo', 'scripts/libbox-patches/android-source.py')
    carrier = module('origin_container', 'scripts/apple-carrier.py')
    origins = defaultdict(list)
    rows = characters = 0
    def append(digest, row):
        nonlocal rows, characters
        rows += 1; characters += len(row['path'])
        require(rows <= 65536 and len(row['path']) <= 4096 and characters <= 16 * 1024 ** 2,
                'native source table row/path budget')
        origins[digest].append(row)
    for path, pin in sorted(files.items()):
        if pin['kind'] == 'compiler-object':
            append(pin['sha256'], {'kind': pin['kind'], 'path': path, 'sha256': pin['sha256']})
        else:
            require(pin['kind'] == 'external-static-archive', 'unknown native origin kind')
            raw = Path(path).read_bytes()
            from fork_input import sha
            require(len(raw) == pin['bytes'] and sha(raw) == pin['sha256'], 'static origin changed during inspection')
            parsed = carrier.container(raw, helper)
            slices = parsed['slices'] if parsed['kind'] == 'fat-ar' else [{'ordinal': 0, 'archive': parsed}]
            for row in slices:
                require(row['archive']['kind'] == 'bsd-ar', 'external runtime must be a static archive')
                for member in row['archive']['members']:
                    if member['classification'] == 'code':
                        append(member['sha256'], {'kind': pin['kind'], 'path': path,
                            'archiveSha256': pin['sha256'], 'sliceOrdinal': row['ordinal'],
                            'memberOrdinal': member['ordinal'], 'sha256': member['sha256']})
    return dict(origins)


def member_references(output, origins):
    require(output.get('format') == 'static-XCFramework', 'actual static output required for origins')
    mappings, used_digests, count = {}, set(), 0
    for target, row in output['slices'].items():
        used, mappings[target] = Counter(), []
        ordinals = set()
        for member in row['members']:
            count += 1
            require(count <= 65536, 'static member reference count budget')
            ordinal = member['ordinal']
            require(type(ordinal) is int and ordinal >= 0 and ordinal not in ordinals, 'duplicate/invalid static member ordinal')
            ordinals.add(ordinal)
            if member['goCarrier']: continue
            digest = member['sha256']; choices = origins.get(digest, [])
            used[digest] += 1
            require(choices and used[digest] <= len(choices), 'static member lacks actual compiler/runtime origin')
            mappings[target].append({'memberOrdinal': ordinal, 'sha256': digest})
            used_digests.add(digest)
    return mappings, {digest: origins[digest] for digest in sorted(used_digests)}


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
            require(len(files) <= 65536, 'native source file count budget')
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
                require(len(files) <= 65536, 'native source file count budget')
    require(files, 'actual compiler/static input bytes are missing')
    facts = {'schema': 'polaris-mobile-native-inputs-v1', 'scratch': str(scratch), 'cache': str(cache),
             'workRoots': [str(p) for p in roots], 'originalBindLog': file_pin(log), 'files': files}
    if platform == 'apple':
        mappings, origins = member_references(output, source_table(files))
        facts.update(schema='polaris-mobile-native-inputs-v2', staticMemberOrigins=mappings, originsByDigest=origins)
    return facts


def verify(facts, scratch=None, log=None, output=None):
    require(facts.get('schema') in ('polaris-mobile-native-inputs-v1', 'polaris-mobile-native-inputs-v2'), 'actual native input receipt required')
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
    if facts['schema'] == 'polaris-mobile-native-inputs-v2' or 'staticMemberOrigins' in facts:
        require(isinstance(output, dict), 'actual output must bind static member references')
        refs, actual = member_references(output, source_table(facts['files']))
        if facts['schema'] == 'polaris-mobile-native-inputs-v2':
            require(facts.get('originsByDigest') == actual and facts.get('staticMemberOrigins') == refs,
                    'actual source table/member references/counts differ')
        else:
            legacy = {target: [{**row, 'origins': actual[row['sha256']]} for row in rows] for target, rows in refs.items()}
            from fork_input import canonical
            def ordered(mapping):
                return {target: [{**row, 'origins': sorted(row['origins'], key=canonical)} for row in rows]
                        for target, rows in mapping.items()}
            require(ordered(facts['staticMemberOrigins']) == ordered(legacy), 'legacy actual source/member origins differ')
